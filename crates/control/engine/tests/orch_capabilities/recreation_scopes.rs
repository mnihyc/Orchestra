//! Real replacement removal custody survives key closure and never restarts the unsafe unit.

use super::*;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyFacet, KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_authz::{CapturedOperation, TokenGrant};
use orch_db::queries::{
    identity::{self, NewApiToken},
    link::{self, NewCommand},
};
use orch_engine::commands::CommandAuthority;
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_types::Time;

async fn actor(world: &World, environment: Id) -> (Caller, Id, Json) {
    let scope: KeyScope = serde_json::from_value(json!({"rules":[{
        "target":{"kind":"environment","id":environment,"reach":"self"},"resources":["environment"],
        "allow":[{"operation":"EnvironmentCapabilityRevoke","actions":["manage"]}]
    }]}))
    .unwrap();
    let id = new_id();
    let expires = world.clock.now() + chrono::Duration::hours(1);
    let mut conn = world.pool.acquire().await.unwrap();
    identity::create_api_token_scoped(
        &mut conn,
        &NewApiToken {
            id,
            user_id: world.ada,
            name: "Recorded replacement removal",
            token_hash: &[43_u8; 32],
            scopes: &[],
            node_id: None,
            at: world.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(scope)),
    )
    .await
    .unwrap();
    let target = orch_engine::operation_scope::targets::page_object(
        &mut conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .unwrap();
    let caller = Caller::token(
        world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expires)))
            .unwrap()
            .for_token(id),
    )
    .admit_facets(&[KeyFacet {
        operation: "EnvironmentCapabilityRevoke".into(),
        action: KeyAction::Manage,
        resource_kind: KeyTargetKind::Object(ObjectKind::Environment),
        target,
    }])
    .unwrap()
    .into_caller();
    let origin = json!(CapturedOperation::capture(&caller).unwrap());
    (caller, id, origin)
}
async fn replacement_only(world: &World, host: Id, environment: Id) {
    sqlx::query("UPDATE hosts SET drivers=jsonb_set(drivers,'{docker,live_changes,capabilities}','[]') WHERE id=$1")
        .bind(host).execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE environments SET granted_capabilities=ARRAY['kvm'] WHERE id=$1")
        .bind(environment)
        .execute(world.pool.pool())
        .await
        .unwrap();
}

#[tokio::test]
async fn actual_replacement_revocation_keeps_exact_private_commands_after_its_key_closes() {
    for closed in [false, true] {
        let world = a_world_with_capabilities().await;
        let (host, environment, runtime, _unit) = a_prepared_live_environment(&world).await;
        replacement_only(&world, host.id, environment).await;
        let (caller, key, origin) = actor(&world, environment).await;
        let stops_before = host.began(CommandKind::NodeRuntimeStop);
        host.added
            .machine
            .handler
            .hold(CommandKind::NodeRuntimeStop);
        let job = world
            .engine()
            .revoke_capability(&caller, environment, "kvm")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(job.plan["capability_revocation"]["origin"], origin);
        assert_eq!(
            job.plan["capability_revocation"]["environment"],
            json!(environment)
        );
        assert_eq!(job.plan["capability_revocation"]["old"], json!(runtime));
        assert_eq!(job.plan["capability_revocation"]["desired"], json!([]));
        assert_eq!(job.plan["capability_revocation_required"], true);
        host.wait_began(CommandKind::NodeRuntimeStop, stops_before + 1)
            .await;
        if closed {
            sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
                .bind(key)
                .bind(world.clock.now())
                .execute(world.pool.pool())
                .await
                .unwrap();
        }
        let mut tx = world.pool.begin().await.unwrap();
        let commands: Vec<Id> = sqlx::query_scalar(
            "SELECT id FROM commands WHERE machine_id=$1 AND kind='node.runtime.stop'
             AND body->>'runtime_id'=$2 AND state IN ('queued','sent','accepted')",
        )
        .bind(host.machine)
        .bind(runtime.to_string())
        .fetch_all(&mut *tx)
        .await
        .unwrap();
        assert_eq!(
            commands.len(),
            1,
            "the actual held removal has one durable command"
        );
        let command = link::command(&mut tx, commands[0]).await.unwrap().unwrap();
        let machine = command.machine_id;
        let private = link::command_authority(&mut tx, command.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(private["kind"], "capability_recreation");
        let authority: CommandAuthority = serde_json::from_value(private).unwrap();
        let mut proposed = NewCommand {
            id: command.id,
            machine_id: machine,
            kind: &command.kind,
            body: &command.body,
            serial_key: command.serial_key(),
            deadline_at: command.deadline_at,
        };
        let current = authority
            .current_issue_callers(
                &mut tx,
                &proposed,
                world.clock.as_ref(),
                world.access.as_ref(),
            )
            .await;
        let current = current.unwrap().unwrap();
        assert_eq!(current.len(), 1);
        assert!(current[0].0.is_none());
        assert!(current[0].1.is_some());
        proposed.id = new_id();
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    world.clock.as_ref(),
                    world.access.as_ref()
                )
                .await
                .unwrap()
                .is_none()
        );
        proposed.id = command.id;
        proposed.machine_id = new_id();
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    world.clock.as_ref(),
                    world.access.as_ref()
                )
                .await
                .unwrap()
                .is_none()
        );
        proposed.machine_id = machine;
        proposed.serial_key = Some("runtime:another");
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    world.clock.as_ref(),
                    world.access.as_ref()
                )
                .await
                .unwrap()
                .is_none()
        );
        proposed.serial_key = command.serial_key();
        let changed = json!({"runtime_id":new_id(),"reason":"recreate"});
        proposed.body = &changed;
        assert!(
            authority
                .current_issue_callers(
                    &mut tx,
                    &proposed,
                    world.clock.as_ref(),
                    world.access.as_ref()
                )
                .await
                .unwrap()
                .is_none()
        );
        tx.rollback().await.unwrap();
        host.added.machine.handler.release();
        let (state, _, error) = hosting::wait_over(&world, job.id).await;
        assert_eq!(state, "succeeded", "{error:?}");
        let grants: Vec<String> = world
            .scalar(
                "SELECT granted_capabilities FROM environments WHERE id=$1",
                environment,
            )
            .await;
        assert!(grants.is_empty());
        assert!(
            host.count(CommandKind::NodeRuntimeCreate) > 1,
            "the real replacement runs after the recorded stop"
        );
    }
}

#[tokio::test]
async fn failed_recorded_removal_cleans_its_candidate_and_keeps_the_old_capable_unit_stopped() {
    let world = a_world_with_capabilities().await;
    let (host, environment, _runtime, _unit) = a_prepared_live_environment(&world).await;
    replacement_only(&world, host.id, environment).await;
    let (caller, key, _) = actor(&world, environment).await;
    let starts = host.count(CommandKind::NodeRuntimeStart);
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeStop);
    host.refuse(
        CommandKind::NodeRuntimeCreate,
        CommandError::new(ErrorCode::Internal, "replacement refused in fixture"),
    );
    let job = world
        .engine()
        .revoke_capability(&caller, environment, "kvm")
        .await
        .unwrap()
        .unwrap();
    host.wait_began(CommandKind::NodeRuntimeStop, 1).await;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(key)
        .bind(world.clock.now())
        .execute(world.pool.pool())
        .await
        .unwrap();
    host.added.machine.handler.release();
    let (state, _, _) = hosting::wait_over(&world, job.id).await;
    assert_eq!(state, "failed");
    assert_eq!(
        host.count(CommandKind::NodeRuntimeStart),
        starts,
        "rollback cannot restore the capability that was removed"
    );
    assert!(
        host.count(CommandKind::NodeRuntimeRemove) > 0,
        "the owned failed candidate is still cleaned up"
    );
    let status: String = world
        .scalar("SELECT status FROM environments WHERE id=$1", environment)
        .await;
    assert_eq!(status, "stopped");
    let grants: Vec<String> = world
        .scalar(
            "SELECT granted_capabilities FROM environments WHERE id=$1",
            environment,
        )
        .await;
    assert!(grants.is_empty());
}
