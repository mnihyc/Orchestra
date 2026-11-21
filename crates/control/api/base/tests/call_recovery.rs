//! Recovery is readable with the thread, controllable only by its drivers, and governed by validated inherited settings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
#[path = "call_recovery/pools.rs"]
mod pools;
mod rig;
mod support;

use orch_api_contract::calls::{
    CallRecoveryAction, ThreadCallRecoveryGet, ThreadCallRecoveryUpdate,
    ThreadCallRecoveryUpdateBody,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::settings::{NodeSettingSet, NodeSettingSetBody};
use orch_provider_contract::calls::Purpose;
use rig::Scene;
use serde_json::json;

#[tokio::test]
async fn a_viewer_reads_the_recovery_but_cannot_control_it_and_a_stranger_cannot_read_it() {
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    scene.rig.engine.stop();
    let mut tx = scene.rig.db.begin().await.unwrap();
    orch_engine::calls::reserve(
        &mut tx,
        thread,
        Purpose::Agent,
        orch_types::new_id(),
        &json!({"code":"provider_busy","message":"Synthetic capacity","retryable":true}),
        json!({"provider_id":scene.on.provider,"model":rig::MODEL}),
        scene.rig.world.now(),
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();
    let reader = scene
        .rig
        .world
        .app
        .call(ThreadCallRecoveryGet, &[&thread])
        .token(&scene.cai.token)
        .send()
        .await
        .ok();
    let recovery = reader.recovery.unwrap();
    assert_eq!(recovery.failure.code, "provider_busy");
    assert_eq!(recovery.rule_id, "busy");
    assert!(recovery.due_at.is_some());
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadCallRecoveryUpdate, &[&thread])
        .token(&scene.cai.token)
        .body(&ThreadCallRecoveryUpdateBody {
            action: CallRecoveryAction::Stop,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["thread.interact"]);
    let refused = scene
        .rig
        .world
        .app
        .call(ThreadCallRecoveryGet, &[&thread])
        .token(&scene.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(places::missing_permissions(&refused), ["thread.view"]);
    let stopped = scene
        .rig
        .world
        .app
        .call(ThreadCallRecoveryUpdate, &[&thread])
        .token(&scene.ada.token)
        .body(&ThreadCallRecoveryUpdateBody {
            action: CallRecoveryAction::Stop,
        })
        .send()
        .await
        .ok();
    assert_eq!(stopped.recovery.unwrap().state, "stopped");
}

#[tokio::test]
async fn invalid_patterns_and_unconfigured_failover_candidates_are_refused_when_rules_are_saved() {
    let scene = Scene::new().await;
    let node = scene.tree.topic.base.id;
    for value in [
        json!([{"id":"new","when":{"regex":"["},"steps":[{"type":"wait","until":"person"}]}]),
        json!([{"id":"new","when":{},"steps":[{"type":"failover","candidate":{"model":"some-model","params":{}}}]}]),
    ] {
        let refused = scene
            .rig
            .world
            .app
            .call(NodeSettingSet, &[&node, &"calls.rules"])
            .token(&scene.ada.token)
            .body(&NodeSettingSetBody {
                model_choice: None,
                value,
                locked: None,
                expected_revision: None,
            })
            .send()
            .await
            .problem();
        assert_eq!(refused.code, ErrorCode::RequestInvalid);
    }
    let saved = scene
        .rig
        .world
        .app
        .call(
            NodeSettingSet,
            &[&scene.tree.project.base.id, &"calls.repeat_limit"],
        )
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!(9),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    let saved = saved
        .into_full()
        .expect("this ordinary control expects the complete native setting");
    assert_eq!(saved.value, Some(json!(9)));
    assert!(!saved.own.unwrap().locked);
    let child = scene
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &"calls.repeat_limit"])
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!(10),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary writer reads its deferred recovery limit");
    assert_eq!(child.value, Some(json!(9)));
    assert_eq!(child.own.as_ref().map(|own| &own.value), Some(&json!(10)));
    assert!(child.locked_by.is_none());
    let tighter = scene
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &"calls.repeat_limit"])
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            model_choice: None,
            value: json!(4),
            locked: None,
            expected_revision: child.own.as_ref().map(|own| own.revision),
        })
        .send()
        .await
        .ok()
        .into_full()
        .expect("the ordinary writer may tighten its recovery limit");
    assert_eq!(tighter.value, Some(json!(4)));
}

#[tokio::test]
async fn the_model_line_reports_the_active_failover_and_current_return_policy_until_the_choice_changes()
 {
    use orch_api_contract::threads::{Delivery, ThreadSyncGet, ThreadSyncGetQuery};
    use orch_link_contract::events::{Event, TurnCompleted, TurnOutcome};
    use orch_settings::Target;
    let scene = Scene::new().await;
    let thread = scene.thread().await.base.id;
    sqlx::query("INSERT INTO provider_models (id,provider_id,model_name,display_name,parameters,enabled,source,created_by) SELECT $1,provider_id,'route-backup','Backup',parameters,true,'manual',$3 FROM provider_models WHERE provider_id=$2 AND model_name=$4")
        .bind(orch_types::new_id()).bind(scene.on.provider).bind(scene.ada.id()).bind(rig::MODEL).execute(scene.rig.db.pool()).await.unwrap();
    scene.rig.world.set_at(Target::Node(scene.tree.topic.base.id), "calls.rules", json!([{"id":"backup","when":{"kind":"model_refused"},"steps":[{"type":"failover","candidate":{"target":{"type":"single","resource":{"kind":"model","backend":{"route":"generation.model","offering":{"provider":scene.on.provider,"offering":"route-backup"},"config":{}}}},"agent_id":scene.on.agent,"params":{}}},{"type":"wait","until":"person"}]}])).await;
    scene
        .say(
            &scene.ada,
            thread,
            "Use an admitted route",
            Delivery::Queue,
            "route-start",
        )
        .await
        .ok();
    let failed = scene.rig.turn_start(thread, 0).await;
    scene.rig.say(thread, rig::turn_began(failed.turn_id)).await;
    scene.rig.say(thread, Event::TurnCompleted(TurnCompleted { turn_id: failed.turn_id, status: TurnOutcome::Failed,
        error: Some(serde_json::from_value(json!({"code":"model_refused","message":"Synthetic refusal","retryable":true})).unwrap()) })).await;
    scene.rig.wait_turn(failed.turn_id, "failed").await;
    let admitted = scene.rig.turn_start(thread, 1).await;
    assert_eq!(admitted.model, "route-backup");
    scene
        .rig
        .say(thread, rig::turn_began(admitted.turn_id))
        .await;
    scene.rig.wait_turn(admitted.turn_id, "running").await;
    let visible = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    let visible = serde_json::to_value(&visible).unwrap();
    assert_eq!(
        visible["binding"]["call_route"]["active_override"]["model"],
        "route-backup"
    );
    assert_eq!(
        visible["binding"]["call_route"]["original"]["model"],
        rig::MODEL
    );
    scene
        .rig
        .world
        .set_at(
            Target::Node(scene.tree.topic.base.id),
            "calls.return",
            json!("stay"),
        )
        .await;
    let staying = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert_eq!(
        serde_json::to_value(&staying).unwrap()["binding"]["call_route"]["return_state"],
        "stay_on_failover"
    );
    scene
        .rig
        .world
        .set_at(Target::Node(thread), "agent.model", json!("route-backup"))
        .await;
    let chosen = scene
        .rig
        .world
        .app
        .call(ThreadSyncGet, &[&thread])
        .token(&scene.cai.token)
        .query(&ThreadSyncGetQuery::default())
        .send()
        .await
        .ok()
        .thread;
    assert!(
        serde_json::to_value(&chosen).unwrap()["binding"]["call_route"].is_null(),
        "a stale recovery cannot replace a newly selected route"
    );
}
