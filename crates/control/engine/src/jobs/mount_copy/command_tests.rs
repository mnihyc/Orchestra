//! Native Copy operands remain bounded to the exact source, destination and retained archive.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
use super::*;
use crate::mount_copies::{KeptDecision, MountDecisions};
use orch_link_contract::commands::VolumeArchive;
use orch_types::new_id;

fn copy() -> CopyDecision {
    CopyDecision {
        mount_id: new_id(),
        source_machine: new_id(),
        source_membership_id: new_id(),
        source_driver: "docker".into(),
        from: DataRef {
            kind: "path".into(),
            source: "/srv/reviewed".into(),
            storage: None,
        },
        target_machine: new_id(),
        target_membership_id: None,
        target_driver: "docker".into(),
        to: DataRef {
            kind: "volume".into(),
            source: "reviewed-copy".into(),
            storage: None,
        },
        made: true,
        kept: None,
        file_count: 2,
        total_bytes: 4096,
        data_kind: None,
    }
}
#[test]
fn a_copy_command_cannot_follow_another_machine_mount_or_volume() {
    let job = new_id();
    let copy = copy();
    let decisions = MountDecisions {
        detached: vec![],
        copies: vec![copy.clone()],
    };
    let mut command = Outgoing::new(
        new_id(),
        copy.target_machine,
        SerialKey::Volume(copy.to.source.clone()),
        &NodeVolumeEnsure {
            driver: copy.target_driver.clone(),
            name: copy.to.source.clone(),
            storage: None,
            labels: labels(job, &copy),
        },
    )
    .unwrap();
    assert!(expected(job, &json!({}), &decisions, &command, false).unwrap());
    command.machine = new_id();
    assert!(!expected(job, &json!({}), &decisions, &command, false).unwrap());
    command.machine = copy.target_machine;
    command.body["labels"]["orch.mount_id"] = json!(new_id());
    assert!(!expected(job, &json!({}), &decisions, &command, false).unwrap());
    command.body["labels"]["orch.mount_id"] = json!(copy.mount_id);
    command.body["name"] = json!("another-volume");
    assert!(!expected(job, &json!({}), &decisions, &command, false).unwrap());
}
#[test]
fn compensation_can_remove_only_the_copys_owned_new_volumes() {
    let job = new_id();
    let mut copy = copy();
    copy.made = false;
    let aside = DataRef {
        kind: "volume".into(),
        source: "kept-copy".into(),
        storage: None,
    };
    copy.kept = Some(KeptDecision {
        previous: copy.to.clone(),
        copied_into: Some(aside.clone()),
        file_count: None,
        total_bytes: None,
        data_kind: None,
    });
    let decisions = MountDecisions {
        detached: vec![],
        copies: vec![copy.clone()],
    };
    let remove = |volume: &DataRef| {
        Outgoing::new(
            new_id(),
            copy.target_machine,
            SerialKey::Volume(volume.source.clone()),
            &NodeVolumeRemove {
                driver: copy.target_driver.clone(),
                name: volume.source.clone(),
                storage: volume.storage.clone(),
                labels: labels(job, &copy),
            },
        )
        .unwrap()
    };
    assert!(
        !expected(job, &json!({}), &decisions, &remove(&copy.to), true).unwrap(),
        "undo never erases the original destination"
    );
    assert!(expected(job, &json!({}), &decisions, &remove(&aside), true).unwrap());
    assert!(
        !expected(job, &json!({}), &decisions, &remove(&aside), false).unwrap(),
        "forward Copy has no deletion act"
    );
}
#[test]
fn restoring_a_replaced_destination_requires_its_original_retained_archive() {
    let job = new_id();
    let mut copy = copy();
    copy.made = false;
    let decisions = MountDecisions {
        detached: vec![],
        copies: vec![copy.clone()],
    };
    let archive = VolumeArchive {
        parts: vec![],
        size: 0,
    };
    let plan = json!({(mount_copies::done::KEPT_ARCHIVES):{copy.mount_id.to_string():archive},(mount_copies::done::UNPACKING):[copy.mount_id]});
    let mut command = Outgoing::new(
        new_id(),
        copy.target_machine,
        SerialKey::Volume(copy.to.source.clone()),
        &NodeMountImport {
            work: None,
            driver: copy.target_driver.clone(),
            job_id: job,
            mount_id: copy.mount_id,
            name: copy.to.source.clone(),
            storage: None,
            archive,
        },
    )
    .unwrap();
    assert!(expected(job, &plan, &decisions, &command, true).unwrap());
    command.body["archive"]["size"] = json!(1);
    assert!(!expected(job, &plan, &decisions, &command, true).unwrap());
    assert!(
        !expected(job, &json!({}), &decisions, &command, true).unwrap(),
        "an unrecorded restore has no authority"
    );
}

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct NativeCopy {
    world: crate::test_common::World,
    job: crate::jobs::runner::Job,
    copy: CopyDecision,
    key: orch_types::Id,
}
impl NativeCopy {
    async fn new() -> TestResult<Self> {
        use orch_access_contract::{key_scope::KeyScope, permissions::ResourceKind};
        use orch_authz::{Caller, CapturedOperation, TokenGrant};
        use orch_db::queries::{identity, resources::access_fixtures};
        use orch_types::{Hash, Time};
        let world = crate::test_common::World::observed().await;
        world.engine().stop();
        let engine = crate::Engine::new(crate::EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let reader = world
            .add_machine("Native copy source", &["environment"])
            .await;
        let writer = world
            .add_machine("Native copy destination", &["environment"])
            .await;
        let mut tx = world.pool.begin().await?;
        let mount = access_fixtures::insert_resource(
            &mut tx,
            "mount",
            Some(world.ada),
            None,
            "Native copy",
        )
        .await?;
        orch_authz::ownership::start_resource(
            &mut tx,
            orch_access_contract::decisions::ObjectRef {
                kind: ResourceKind::Mount.into(),
                id: mount,
            },
            world.ada,
            orch_authz::ownership::NewAccess::OnlyMe,
            world.clock.time(),
        )
        .await?;
        for id in [reader.id, writer.id] {
            sqlx::query("UPDATE machines SET owner_id=$2,online=true,facts=$3 WHERE id=$1")
                .bind(id).bind(world.ada).bind(json!({"policy":{"environment.roots":["/srv"]},"environment_descriptor":{"name":"directory","lifecycle":"attach","data_storage":{"made_by_orch":"directory","existing_directory":true,"reason":null}},"storages":[]}))
                .execute(&mut *tx).await?;
        }
        orch_db::queries::resources::mounts::set_location(
            &mut tx,
            &orch_db::queries::resources::mounts::LocationRow {
                mount_id: mount,
                machine_id: reader.id,
                host_id: None,
                host_name: None,
                kind: "path".into(),
                source: "/srv/reviewed".into(),
                storage: None,
                data_kind: None,
                created_by: world.ada,
                created_at: world.clock.now(),
            },
        )
        .await?;
        let review =
            crate::mount_copies::location::Review::capture(&mut tx, mount, reader.id, writer.id)
                .await?;
        let membership = sqlx::query_scalar(
            "SELECT membership_id FROM mount_locations WHERE mount_id=$1 AND machine_id=$2",
        )
        .bind(mount)
        .bind(reader.id)
        .fetch_one(&mut *tx)
        .await?;
        let copy = CopyDecision {
            mount_id: mount,
            source_machine: reader.id,
            source_membership_id: membership,
            source_driver: "directory".into(),
            from: DataRef {
                kind: "path".into(),
                source: "/srv/reviewed".into(),
                storage: None,
            },
            target_machine: writer.id,
            target_membership_id: None,
            target_driver: "directory".into(),
            to: DataRef {
                kind: "volume".into(),
                source: format!("orch-mount-{mount}"),
                storage: None,
            },
            made: true,
            kept: None,
            file_count: 2,
            total_bytes: 4096,
            data_kind: None,
        };
        let key = new_id();
        let hash = Hash::of(key.as_bytes());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let scope: KeyScope = serde_json::from_value(json!({"rules":[
            {"target":{"kind":"mount","id":mount,"reach":"self"},"allow":[{"operation":"MountCopyCreate","actions":["write"]}]},
            {"target":{"kind":"machine","id":reader.id,"reach":"self"},"allow":[{"operation":"MountCopyCreate","actions":["manage"]}]},
            {"target":{"kind":"machine","id":writer.id,"reach":"self"},"allow":[{"operation":"MountCopyCreate","actions":["manage"]}]}
        ]}))?;
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: key,
                user_id: world.ada,
                name: "Native copy",
                token_hash: hash.as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(scope)),
        )
        .await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(scope), Some(Time::new(expires)))?
                .for_token(key),
        );
        let facts = crate::mount_copies::location::operation::facts(&mut tx, &review).await?;
        let plan = crate::operation_scope::sources::bind_required(
            <orch_api_contract::mounts::MountCopyCreate as orch_api_contract::routes::Endpoint>::ROUTE,
            &orch_api_contract::mount_copy_sources::CREATE,&facts)?;
        let admitted = plan.admit(&caller, false)?;
        let origin = crate::operation_scope::http_origin::HttpOrigin::capture(
            CapturedOperation::capture(&admitted)?,
            crate::operation_scope::request_origin::Credential::Token {
                id: key,
                hash: hash.as_bytes().to_vec(),
            },
            false,
            false,
            Some(Time::new(expires)),
        )?;
        let recipe = super::super::Recipe {
            review,
            decisions: MountDecisions {
                detached: vec![],
                copies: vec![copy.clone()],
            },
            origin,
        };
        let row = engine.queue_mount_copy(&mut tx, &recipe).await?;
        let runner = new_id();
        assert!(
            orch_db::queries::jobs::claim(&mut tx, row.id, runner, expires, world.clock.now())
                .await?
        );
        assert!(crate::mount_copies::claims::acquire(&mut tx, &row, &[&copy]).await?);
        tx.commit().await?;
        Ok(Self {
            job: crate::jobs::runner::Job::claimed(engine, row.id, runner),
            world,
            copy,
            key,
        })
    }
    async fn remove(&self) -> TestResult<Outgoing> {
        self.job
            .begin_undo(&crate::jobs::runner::Stop::Cancelled)
            .await?;
        let mut command = Outgoing::new(
            new_id(),
            self.copy.target_machine,
            SerialKey::Volume(self.copy.to.source.clone()),
            &NodeVolumeRemove {
                driver: self.copy.target_driver.clone(),
                name: self.copy.to.source.clone(),
                storage: self.copy.to.storage.clone(),
                labels: labels(self.job.id(), &self.copy),
            },
        )?;
        decorate(&self.job, &mut command, false)
            .await
            .map_err(|e| format!("copy compensation sealing: {e:?}"))?;
        Ok(command)
    }
}

#[tokio::test]
async fn a_copys_owned_cleanup_keeps_one_exact_deadline_and_survives_withdrawn_forward_permission()
-> TestResult {
    let scene = NativeCopy::new().await?;
    let command = scene.remove().await?;
    let until = command
        .deadline
        .ok_or("the exact native cleanup deadline was not retained")?;
    let authority = command.authority.as_ref().ok_or("copy source missing")?;
    let serial = command.key.wire();
    let proposal = orch_db::queries::link::NewCommand {
        native_target: None,
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        body: &command.body,
        serial_key: serial.as_deref(),
        deadline_at: until.get(),
    };
    let mut tx = scene.world.pool.begin().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(scene.key)
        .bind(scene.world.clock.now())
        .execute(&mut *tx)
        .await?;
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &proposal,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_some(),
        "domain-owned compensation never borrows or renews the withdrawn forward grant"
    );
    let mut changed = proposal.clone();
    changed.deadline_at += chrono::Duration::seconds(1);
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &changed,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_none(),
        "another deadline is another native command"
    );
    tx.commit().await?;
    Ok(())
}

#[tokio::test]
async fn a_queued_copy_refuses_a_changed_recipient_and_a_withdrawn_original_key_at_dispatch()
-> TestResult {
    let scene = NativeCopy::new().await?;
    let mut command = Outgoing::new(
        new_id(),
        scene.copy.target_machine,
        SerialKey::Volume(scene.copy.to.source.clone()),
        &NodeVolumeEnsure {
            driver: scene.copy.target_driver.clone(),
            name: scene.copy.to.source.clone(),
            storage: scene.copy.to.storage.clone(),
            labels: labels(scene.job.id(), &scene.copy),
        },
    )?;
    scene
        .job
        .remember(&json!({"active_command":{"id":command.id,"machine":command.machine}}))
        .await?;
    decorate(&scene.job, &mut command, true)
        .await
        .map_err(|e| format!("forward copy sealing: {e:?}"))?;
    let authority = command
        .authority
        .as_ref()
        .ok_or("copy native authority missing")?;
    let serial = command.key.wire();
    let proposal = orch_db::queries::link::NewCommand {
        native_target: None,
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        body: &command.body,
        serial_key: serial.as_deref(),
        deadline_at: command
            .deadline
            .ok_or("copy native deadline missing")?
            .get(),
    };
    let mut tx = scene.world.pool.begin().await?;
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &proposal,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_some()
    );
    let mut foreign = proposal.clone();
    foreign.machine_id = scene.copy.source_machine;
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &foreign,
                scene.world.clock.as_ref(),
                scene.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    orch_db::queries::link::insert_command_with_authority(
        &mut tx,
        &proposal,
        scene.world.clock.now(),
        Some(&serde_json::to_value(authority)?),
    )
    .await?;
    tx.commit().await?;
    let mut tx = scene.world.pool.begin().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(scene.key)
        .bind(scene.world.clock.now())
        .execute(&mut *tx)
        .await?;
    let retained = orch_db::queries::link::command(&mut tx, command.id)
        .await?
        .ok_or("copy command not retained")?;
    let current = authority
        .current_callers(
            &mut tx,
            &retained,
            scene.world.clock.as_ref(),
            scene.world.access.as_ref(),
        )
        .await;
    assert!(
        matches!(current, Ok(None) | Err(crate::EngineError::BeforeEffect(_))),
        "queueing is not permission to send after the original key is withdrawn"
    );
    tx.commit().await?;
    Ok(())
}
