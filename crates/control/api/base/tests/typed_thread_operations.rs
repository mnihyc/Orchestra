//! Exact key bounds on the existing thread readers and content-free controls.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "tests fail by panicking"
)]

mod places;
mod rig;
mod support;

use http::StatusCode;
use orch_api_contract::calls::ThreadCallRecoveryGet;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::goals::ThreadGoalGet;
use orch_api_contract::settings::{
    EnvironmentSecretDelete, EnvironmentSecretList, EnvironmentSecretSet, NodeSecretSetBody,
};
use orch_api_contract::threads::{
    AdoptHistoryState, ThreadAdoptHistoryGet, ThreadDraftDelete, ThreadDraftGet, ThreadRawList,
    ThreadRawListQuery,
};
use orch_api_core::middleware::authentication::{API_TOKEN_PREFIX, new_token, token_hash};
use orch_db::queries::{identity, settings, threads};
use orch_db::secrets::MasterKey;
use orch_types::{Id, Json, new_id};
use rig::Scene;
use serde_json::json;
use support::World;

async fn key(world: &World, user: Id, kind: &str, id: Id, allow: Json) -> String {
    let scope: orch_access_contract::key_scope::KeyScope = serde_json::from_value(json!({
        "rules":[{"target":{"kind":kind,"id":id,"reach":"self"},"allow":allow}]
    }))
    .unwrap();
    assert!(scope.validate().is_empty());
    let secret = new_token(API_TOKEN_PREFIX);
    let hash = token_hash(&secret);
    let scope = serde_json::to_value(scope).unwrap();
    identity::create_api_token_scoped(
        &mut *world.conn().await,
        &identity::NewApiToken {
            id: new_id(),
            user_id: user,
            name: "Direct thread control",
            token_hash: &hash,
            scopes: &[],
            node_id: None,
            at: world.now(),
            // Fit existing place lifetime ceilings without moving the fixture clock.
            expires_at: Some(world.now() + chrono::Duration::hours(1)),
        },
        Some(&scope),
    )
    .await
    .unwrap();
    secret
}

#[test]
fn six_declarations_preserve_inspection_and_optional_mutation_disclosure() {
    use orch_access_contract::key_scope::{KeyAction as Action, KeyTargetKind};
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::operations::{FacetUsage, TargetResolver, catalogue};
    let operations = catalogue().unwrap();
    for (name, kind, action, mutates) in [
        ("ThreadRawList", ObjectKind::Thread, Action::Manage, false),
        (
            "ThreadCallRecoveryGet",
            ObjectKind::Thread,
            Action::Read,
            false,
        ),
        ("ThreadGoalGet", ObjectKind::Thread, Action::Read, false),
        (
            "ThreadAdoptHistoryGet",
            ObjectKind::Thread,
            Action::Read,
            false,
        ),
        ("ThreadDraftDelete", ObjectKind::Thread, Action::Write, true),
        (
            "EnvironmentSecretDelete",
            ObjectKind::Environment,
            Action::Manage,
            true,
        ),
    ] {
        let op = operations
            .iter()
            .find(|op| op.operation_id == name)
            .unwrap();
        assert_eq!(op.variants.len(), 1);
        let variant = &op.variants[0];
        assert_eq!(variant.operation, name);
        assert_eq!(variant.facets[0].action, action);
        assert!(variant.facets.iter().all(|facet| facet.target
            == TargetResolver::TypedPath {
                parameter: "id",
                kind: KeyTargetKind::Object(kind),
            }));
        assert_eq!(variant.facets.len(), if mutates { 2 } else { 1 });
        if mutates {
            assert!(op.receipt);
            assert_eq!(variant.facets[1].action, Action::Read);
            assert_eq!(variant.facets[1].usage, FacetUsage::ResponseContent);
        }
    }
}

#[tokio::test]
async fn raw_inspection_requires_its_manage_selection_and_current_thread_management() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread().await.base.id;
    s.exchange(thread, 0, "Hello", "Ready").await;
    s.rig.engine.stop();
    let segment = s.rig.segment_of(thread).await;
    let world = &s.rig.world;
    orch_engine::raw_events::install_master_key(MasterKey::new([7; 32]));
    let mut conn = world.conn().await;
    let row = threads::thread(&mut conn, thread).await.unwrap().unwrap();
    orch_engine::raw_events::Keeper::new(&row, segment)
        .keep(
            &mut conn,
            &orch_link_contract::events::RawFrame {
                n: 1,
                direction: "in".into(),
                payload: json!({"result":"Stored diagnostic"}),
            },
            world.now(),
        )
        .await
        .unwrap();
    drop(conn);
    let query = ThreadRawListQuery {
        segment_id: segment,
        ..Default::default()
    };
    let allow = json!([{"operation":"ThreadRawList","actions":["manage"]}]);
    let manage = key(world, s.ada.id(), "thread", thread, allow.clone()).await;
    let page = world
        .app
        .call(ThreadRawList, &[&thread])
        .token(&manage)
        .query(&query)
        .send()
        .await
        .ok();
    assert!(
        page.events
            .iter()
            .any(|event| event.payload["result"] == "Stored diagnostic")
    );
    let stale = ThreadRawListQuery {
        projection_revision: Some(page.projection.unwrap().projection_revision + 1),
        ..query.clone()
    };
    assert_eq!(
        world
            .app
            .call(ThreadRawList, &[&thread])
            .token(&manage)
            .query(&stale)
            .send()
            .await
            .problem()
            .code,
        ErrorCode::ConflictStaleRevision,
        "a mismatched continuation revision still requires a fresh client selection"
    );
    let read = key(
        world,
        s.ada.id(),
        "thread",
        thread,
        json!([{"operation":"ThreadCallRecoveryGet","actions":["read"]}]),
    )
    .await;
    assert_eq!(
        world
            .app
            .call(ThreadRawList, &[&thread])
            .token(&read)
            .query(&query)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let powerless = key(world, s.cai.id(), "thread", thread, allow).await;
    assert_eq!(
        world
            .app
            .call(ThreadRawList, &[&thread])
            .token(&powerless)
            .query(&query)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "scope does not grant ordinary ThreadManage"
    );
    assert_eq!(
        world
            .app
            .call(ThreadRawList, &[&other])
            .token(&manage)
            .query(&query)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn thread_status_readers_keep_their_stored_producers_and_ordinary_visibility() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread().await.base.id;
    s.exchange(thread, 0, "Hello", "Ready").await;
    s.rig.engine.stop();
    let segment = s.rig.segment_of(thread).await;
    let world = &s.rig.world;
    let mut tx = world.app.app.db.begin().await.unwrap();
    // GoalCreate deliberately starts work inline even when the background engine is stopped.
    // Seed its existing stored producer here so a late creation command cannot be attributed
    // to the three read-only operations whose command count this control measures.
    let after_turn: i32 =
        sqlx::query_scalar("SELECT COALESCE(MAX(ordinal), 0) FROM turns WHERE thread_id = $1")
            .bind(thread)
            .fetch_one(&mut *tx)
            .await
            .unwrap();
    let authority = json!({"boundaries": [orch_authz::CapturedCaller::capture(
        &orch_authz::Caller::session(s.ada.id()),
    )]});
    orch_db::queries::goals::insert(
        &mut tx,
        &orch_db::queries::goals::NewGoal {
            id: new_id(),
            thread_id: thread,
            objective: "Stored goal",
            condition: "The check passes",
            set_by: s.ada.id(),
            set_authority: &authority,
            plan_first: false,
            notify_on_end: false,
            max_turns: None,
            max_minutes: None,
            max_credits: None,
            after_turn,
            at: world.now(),
        },
    )
    .await
    .unwrap();
    orch_engine::calls::reserve(
        &mut tx,
        thread,
        orch_provider_contract::calls::Purpose::Agent,
        new_id(),
        &json!({"code":"provider_busy","message":"Stored recovery","retryable":true}),
        json!({"provider_id":s.on.provider,"model":rig::MODEL}),
        world.now(),
    )
    .await
    .unwrap();
    let person = orch_engine::views::user_ref_json(&mut tx, s.ada.id())
        .await
        .unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, world.now());
    orch_engine::ingest::add_marker(
        &mut unit,
        thread,
        segment,
        None,
        &json!({
            "marker":"adopted", "data":{
                "by":person,"native_session_ref":"stored-session","turns":0,"items":0,
                "history_missing":"unreadable","history_error":"Stored history failure",
                "session_model":null,"model_reason":null
            }
        }),
        "adopted",
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    let allow = json!([
        {"operation":"ThreadCallRecoveryGet","actions":["read"]},
        {"operation":"ThreadGoalGet","actions":["read"]},
        {"operation":"ThreadAdoptHistoryGet","actions":["read"]}
    ]);
    let token = key(world, s.ada.id(), "thread", thread, allow.clone()).await;
    let before = s.rig.commands.issued_total();
    let recovery = world
        .app
        .call(ThreadCallRecoveryGet, &[&thread])
        .token(&token)
        .send()
        .await
        .ok()
        .recovery
        .unwrap();
    assert_eq!(recovery.failure.code, "provider_busy");
    let goal = world
        .app
        .call(ThreadGoalGet, &[&thread])
        .token(&token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(goal.objective, "Stored goal");
    let status = world
        .app
        .call(ThreadAdoptHistoryGet, &[&thread])
        .token(&token)
        .send()
        .await
        .ok()
        .unwrap();
    assert_eq!(status.state, AdoptHistoryState::Unreadable);
    assert!(status.can_read_again);
    assert_eq!(status.reason.as_deref(), Some("Stored history failure"));
    assert_eq!(
        status,
        s.rig
            .engine
            .adoption_history_status(thread)
            .await
            .unwrap()
            .unwrap(),
        "the existing Engine wrapper and admitted HTTP reader share one status producer"
    );
    assert_eq!(
        s.rig.commands.issued_total(),
        before,
        "status reads queue no native work"
    );
    let viewer = key(world, s.cai.id(), "thread", thread, allow.clone()).await;
    assert!(
        !world
            .app
            .call(ThreadAdoptHistoryGet, &[&thread])
            .token(&viewer)
            .send()
            .await
            .ok()
            .unwrap()
            .can_read_again,
        "the existing flag still requires ordinary ThreadManage"
    );
    let outsider = key(world, s.bea.id(), "thread", thread, allow).await;
    assert_eq!(
        world
            .app
            .call(ThreadGoalGet, &[&thread])
            .token(&outsider)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(ThreadCallRecoveryGet, &[&other])
            .token(&token)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn draft_and_environment_secret_deletes_return_empty_receipts_without_read_grants() {
    let s = Scene::new().await;
    let thread = s.thread().await.base.id;
    let other = s.thread().await.base.id;
    s.rig.engine.stop();
    let world = &s.rig.world;
    for person in [s.ada.id(), s.dan.id()] {
        threads::set_draft(
            &mut *world.conn().await,
            person,
            thread,
            &json!([{"type":"text","text":"Private draft"}]),
            "queue",
            world.now(),
        )
        .await
        .unwrap();
    }
    let allow = json!([{"operation":"ThreadDraftDelete","actions":["write"]}]);
    let powerless = key(world, s.cai.id(), "thread", thread, allow.clone()).await;
    let denied = world
        .app
        .call(ThreadDraftDelete, &[&thread])
        .token(&powerless)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert!(!denied.extra.contains_key("missing"));
    let browser = world
        .app
        .call(ThreadDraftDelete, &[&thread])
        .token(&s.cai.token)
        .send()
        .await
        .problem();
    assert_eq!(browser.code, denied.code);
    assert!(browser.extra.contains_key("missing"));
    let write = key(world, s.ada.id(), "thread", thread, allow).await;
    assert_eq!(
        world
            .app
            .call(ThreadDraftGet, &[&thread])
            .token(&write)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "discard authority does not grant draft readback"
    );
    assert_eq!(
        world
            .app
            .call(ThreadDraftDelete, &[&other])
            .token(&write)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    let deleted = world
        .app
        .call(ThreadDraftDelete, &[&thread])
        .token(&write)
        .send()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert!(
        threads::draft(&mut *world.conn().await, s.ada.id(), thread)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        threads::draft(&mut *world.conn().await, s.dan.id(), thread)
            .await
            .unwrap()
            .is_some()
    );

    orch_api_core::routes::settings::install_master_key(MasterKey::new([7; 32]));
    let environment = s.on.environment;
    world
        .app
        .call(EnvironmentSecretSet, &[&environment, &"CREDENTIAL"])
        .token(&s.ada.token)
        .body(&NodeSecretSetBody {
            value: "private stored secret".into(),
        })
        .send()
        .await
        .ok();
    let allow = json!([{"operation":"EnvironmentSecretDelete","actions":["manage"]}]);
    let powerless = key(world, s.cai.id(), "environment", environment, allow.clone()).await;
    let denied = world
        .app
        .call(EnvironmentSecretDelete, &[&environment, &"CREDENTIAL"])
        .token(&powerless)
        .send()
        .await
        .problem();
    assert_eq!(denied.code, ErrorCode::AuthzDenied);
    assert!(!denied.extra.contains_key("missing"));
    assert!(
        settings::secret(
            &mut *world.conn().await,
            "environment",
            environment,
            "CREDENTIAL"
        )
        .await
        .unwrap()
        .is_some()
    );
    let manage = key(world, s.ada.id(), "environment", environment, allow).await;
    let other_environment = orch_db::queries::resources::access_fixtures::insert_resource(
        &mut *world.conn().await,
        "environment",
        Some(s.ada.id()),
        Some(s.tree.org.base.id),
        "Other environment",
    )
    .await
    .unwrap();
    assert_eq!(
        world
            .app
            .call(
                EnvironmentSecretDelete,
                &[&other_environment, &"CREDENTIAL"]
            )
            .token(&manage)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        world
            .app
            .call(EnvironmentSecretList, &[&environment])
            .token(&manage)
            .send()
            .await
            .status,
        StatusCode::FORBIDDEN,
        "secret deletion grants no metadata list"
    );
    let deleted = world
        .app
        .call(EnvironmentSecretDelete, &[&environment, &"CREDENTIAL"])
        .token(&manage)
        .send()
        .await;
    assert_eq!(deleted.status, StatusCode::NO_CONTENT);
    assert!(deleted.raw.is_empty());
    assert!(
        settings::secret(
            &mut *world.conn().await,
            "environment",
            environment,
            "CREDENTIAL"
        )
        .await
        .unwrap()
        .is_none()
    );
}
