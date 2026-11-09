//! Actual scoped restore jobs keep original/current key bounds before any new physical work.

use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyFacet, KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::identity::{self, NewApiToken};

#[tokio::test]
async fn captured_retained_restore_keeps_real_files_and_refuses_a_closed_original_scope() {
    for closed in [false, true] {
        let world = world().await;
        let host = hosting::ScriptedHost::on(&world, "Scoped retained files", 32768).await;
        let template = hosting::template(&world).await;
        let (environment, _, _unit) =
            hosting::running(&world, &host, template, "Original kept files").await;
        set(&world, environment, 14).await;
        schedule(&world, environment).await;
        remove(&world, environment).await;
        let kept = promise(&world, environment).await;
        assert_eq!(kept.removal_phase.as_deref(), Some("kept"));
        let scope:KeyScope=serde_json::from_value(json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},
            "resources":["environment"],"allow":[{"operation":"EnvironmentRestore","actions":["manage"]}]}]})).unwrap();
        let key = orch_types::new_id();
        let expiry = world.clock.now() + chrono::Duration::hours(1);
        let mut tx = world.pool.begin().await.unwrap();
        identity::create_api_token_scoped(
            &mut tx,
            &NewApiToken {
                id: key,
                user_id: world.ada,
                name: "Current retained restore source",
                token_hash: &[57_u8; 32],
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expiry),
            },
            Some(&json!(scope)),
        )
        .await
        .unwrap();
        let target = orch_engine::operation_scope::targets::resource_identity(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await
        .unwrap();
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expiry)))
                .unwrap()
                .for_token(key),
        )
        .admit_facets(&[KeyFacet {
            operation: "EnvironmentRestore".into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::Environment),
            target,
        }])
        .unwrap()
        .into_caller();
        let origin = CapturedOperation::capture(&caller).unwrap();
        let starts = host.count(K::NodeRuntimeStart);
        let job = orch_engine::jobs::retained::operation::queue_for_operation(
            &mut tx,
            environment,
            kept.removal_revision,
            false,
            world.ada,
            json!({"token_id":key}),
            &origin,
            world.clock.now(),
            || Ok(()),
        )
        .await
        .unwrap();
        assert_eq!(job.plan["retained_operation"]["original"], json!(origin));
        assert_eq!(
            job.plan["retained_operation"]["revision"],
            kept.removal_revision
        );
        assert_eq!(job.plan["volume_refs"], kept.retained_volumes);
        if closed {
            sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1").bind(key)
                .bind(json!({"rules":[{"target":{"kind":"environment","id":environment,"reach":"self"},"resources":["environment"],
                    "allow":[{"operation":"EnvironmentPurge","actions":["manage"]}]}]})).execute(&mut *tx).await.unwrap();
        }
        tx.commit().await.unwrap();
        world.engine().kick();
        if !closed {
            host.wait_for(K::NodeRuntimeStart, starts + 1).await;
            let candidate = host.started().unwrap();
            let _restored = hosting::enroll(&world, candidate).await;
            let (state, _, error) = hosting::wait_over(&world, job.id).await;
            assert_eq!(state, "succeeded", "{error:?}");
            let status: String = world
                .scalar("SELECT status FROM environments WHERE id=$1", environment)
                .await;
            assert_eq!(status, "stopped");
        } else {
            let (state, _, error) = hosting::wait_over(&world, job.id).await;
            assert_eq!(state, "failed", "{error:?}");
            assert_eq!(
                host.count(K::NodeRuntimeStart),
                starts,
                "a nearby operation cannot revive the original restore"
            );
            let current = promise(&world, environment).await;
            assert_eq!(current.removal_phase.as_deref(), Some("kept"));
            assert_eq!(
                current.retained_volumes, kept.retained_volumes,
                "failure keeps the exact owned files"
            );
        }
    }
}
