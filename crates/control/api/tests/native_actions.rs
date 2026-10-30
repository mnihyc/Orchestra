//! A cold native catalogue is truthful and reveals nothing without current thread View.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
mod rig;
mod support;

use orch_api_contract::native::{
    NativeCatalogState, ThreadNativeActionList, ThreadNativeActionListQuery,
};

#[tokio::test]
async fn a_verified_native_input_is_delivered_once_and_its_trail_has_no_arguments() {
    use orch_api_contract::input_schedules::NativeInvocation;
    use orch_api_contract::native::*;
    use orch_api_contract::threads::*;
    use serde_json::json;
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    scene
        .say(
            &scene.ada,
            thread.base.id,
            "Open the ordinary session",
            Delivery::Queue,
            "bootstrap",
        )
        .await
        .ok();
    scene.rig.run_turn(thread.base.id, 0, "ready").await;
    *scene.rig.commands.native.lock().unwrap() = Some(NativeCatalog {
        revision: "loaded-1".to_owned(),
        state: NativeCatalogState::Ready,
        actions: vec![NativeAction {
            id: "artifact-1".to_owned(),
            name: "review".to_owned(),
            title: "Review".to_owned(),
            kind: NativeActionKind::Skill,
            description: "Verified test artifact".to_owned(),
            arguments_schema: json!({"type":"object","properties":{"target":{"type":"string"}},"required":["target"],"additionalProperties":false}),
            argument_hint: None,
            origin: NativeOrigin {
                kind: NativeOriginKind::Builtin,
                name: None,
                path: None,
            },
            permission: NativePermission::Interact,
            delivery_modes: vec![NativeDelivery::Queue],
            available: true,
            reason: None,
        }],
        reason: None,
        project_skills: NativeProjectSkills {
            available: false,
            reason: Some("No remote project discovery".to_owned()),
        },
        as_of: None,
    });
    let catalog = scene
        .rig
        .world
        .app
        .call(ThreadNativeActionList, &[&thread.base.id])
        .token(&scene.ada.token)
        .query(&ThreadNativeActionListQuery::default())
        .send()
        .await
        .ok();
    let bad = ThreadInputCreateBody {
        plan: false,
        content: vec![ContentPart::Text {
            text: "unchanged words".to_owned(),
        }],
        delivery: Delivery::Queue,
        client_id: "stale-native".to_owned(),
        bypass: None,
        native_action: Some(NativeInvocation {
            action_id: "artifact-1".to_owned(),
            revision: "old-mapping".to_owned(),
            arguments: json!({"target":"private-stale-argument"}),
        }),
    };
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread.base.id])
        .token(&scene.ada.token)
        .body(&bad)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::NativeActionStale
    );
    assert!(
        !serde_json::to_string(&refused)
            .unwrap()
            .contains("private-stale-argument")
    );
    assert_eq!(
        scene.rig.turn_starts(thread.base.id).len(),
        1,
        "an invalid selector never starts another model turn or falls back to its words"
    );
    let invalid = ThreadInputCreateBody {
        plan: false,
        native_action: Some(NativeInvocation {
            revision: catalog.revision.clone(),
            arguments: json!({"target":42}),
            ..bad.native_action.unwrap()
        }),
        client_id: "invalid-native".to_owned(),
        ..bad
    };
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread.base.id])
        .token(&scene.ada.token)
        .body(&invalid)
        .send()
        .await
        .problem();
    assert_eq!(
        refused.code,
        orch_api_contract::common::ErrorCode::RequestInvalid
    );
    let body = ThreadInputCreateBody {
        plan: false,
        content: vec![ContentPart::Text {
            text: "Keep these literal words".to_owned(),
        }],
        delivery: Delivery::Queue,
        client_id: "native-once".to_owned(),
        bypass: None,
        native_action: Some(NativeInvocation {
            action_id: "artifact-1".to_owned(),
            revision: catalog.revision,
            arguments: json!({"target":"private-argument-do-not-audit"}),
        }),
    };
    let accepted = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    let repeated = scene
        .rig
        .world
        .app
        .call(ThreadInputCreate, &[&thread.base.id])
        .token(&scene.ada.token)
        .body(&body)
        .send()
        .await
        .ok();
    assert_eq!(accepted.input.id, repeated.input.id);
    let sent = scene.rig.turn_start(thread.base.id, 1).await;
    assert_eq!(sent.native_action, body.native_action);
    assert!(sent.text.contains("Keep these literal words"));
    scene
        .rig
        .answer(thread.base.id, &sent, "native-reply", "reviewed")
        .await;
    scene
        .rig
        .say(
            thread.base.id,
            rig::turn_ended(
                sent.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    scene.rig.wait_turn(sent.turn_id, "completed").await;
    // A duplicated started event cannot add another factual invocation.
    scene
        .rig
        .say(thread.base.id, rig::turn_began(sent.turn_id))
        .await;
    let events:Vec<(Option<orch_types::Id>,serde_json::Value)>=sqlx::query_as("SELECT actor_id,details FROM audit_log WHERE action='native_action.invoked' AND node_id=$1")
        .bind(thread.base.id).fetch_all(scene.rig.db.pool()).await.unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].0, Some(scene.ada.id()));
    assert_eq!(events[0].1["action_id"], "artifact-1");
    assert_eq!(events[0].1["kind"], "skill");
    assert_eq!(events[0].1["turn"], json!(sent.turn_id));
    assert!(!events[0].1.to_string().contains("private-argument"));
}

#[tokio::test]
async fn reading_or_refreshing_commands_never_opens_a_cold_session_or_a_model_turn() {
    let scene = rig::Scene::new().await;
    let thread = scene.thread().await;
    for refresh in [false, true] {
        let shown = scene
            .rig
            .world
            .app
            .call(ThreadNativeActionList, &[&thread.base.id])
            .token(&scene.ada.token)
            .query(&ThreadNativeActionListQuery {
                refresh: Some(refresh),
            })
            .send()
            .await
            .ok();
        assert_eq!(shown.state, NativeCatalogState::NotReady);
        assert!(shown.actions.is_empty());
        assert!(shown.reason.is_some());
        assert!(!shown.project_skills.available);
    }
    let turns: i64 = sqlx::query_scalar("SELECT count(*) FROM turns WHERE thread_id=$1")
        .bind(thread.base.id)
        .fetch_one(scene.rig.db.pool())
        .await
        .unwrap();
    assert_eq!(turns, 0);
    let native:Option<String>=sqlx::query_scalar("SELECT s.native_session_ref FROM segments s JOIN threads t ON t.current_segment_id=s.id WHERE t.node_id=$1")
        .bind(thread.base.id).fetch_optional(scene.rig.db.pool()).await.unwrap().flatten();
    assert!(native.is_none());
    let hidden = scene
        .rig
        .world
        .app
        .call(ThreadNativeActionList, &[&thread.base.id])
        .token(&scene.bea.token)
        .query(&ThreadNativeActionListQuery::default())
        .send()
        .await;
    assert!(
        hidden.status == 403 || hidden.status == 404,
        "another tree's reader does not receive a catalogue"
    );
}
