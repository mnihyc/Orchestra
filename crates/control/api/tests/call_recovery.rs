//! Recovery is readable with the thread, controllable only by its drivers, and governed by validated inherited settings.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod places;
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
            value: json!(9),
            locked: Some(true),
            expected_revision: None,
        })
        .send()
        .await
        .ok();
    assert_eq!(saved.value, Some(json!(9)));
    assert!(saved.own.unwrap().locked);
    let refused = scene
        .rig
        .world
        .app
        .call(NodeSettingSet, &[&node, &"calls.repeat_limit"])
        .token(&scene.ada.token)
        .body(&NodeSettingSetBody {
            value: json!(10),
            locked: None,
            expected_revision: None,
        })
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::AccessLocked);
}
