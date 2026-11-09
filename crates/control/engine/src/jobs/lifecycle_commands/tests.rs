//! Real stored raw key, native parent queue, claim, command journal and dispatcher checkpoints.
use super::*;
use crate::{Engine, EngineParts, test_common::World};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyScope, KeyTargetKind},
    permissions::ObjectKind,
};
use orch_authz::{Caller, TokenGrant};
use orch_db::queries::{identity, link};
use orch_link_contract::commands::{NodeVolumeRemove, SerialKey};
use orch_types::{Hash, new_id};
use serde_json::json;

mod parent_sources;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Fixture {
    world: World,
    job: Job,
    key: Id,
    machine: Id,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::observed().await;
        let machine = world.add_machine("Native cascade command", &["node"]).await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let host = new_id();
        let key = new_id();
        let digest = Hash::of(key.as_bytes());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let policy: KeyScope = serde_json::from_value(
            json!({"rules":[{"target":{"kind":"host","id":host,"reach":"self"},"resources":["host"],"allow":[{"operation":"HostDeleteCreate","actions":["manage"]}]}]}),
        )?;
        let mut tx = world.pool.begin().await?;
        sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,'Native cascade host',$3,$4,$4,$5,$6)").bind(host).bind(machine.id).bind(world.ada).bind(world.root).bind(json!({"test":{"spec_schemas":{"environment":{}},"sources":["image"]}})).bind(json!({"memory_mb":4096})).execute(&mut *tx).await?;
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: key,
                user_id: world.ada,
                name: "Native cascade confirmation",
                token_hash: digest.as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(policy)),
        )
        .await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))?
                .for_token(key),
        );
        let target = crate::operation_scope::targets::resource_identity(
            &mut tx,
            ObjectRef {
                kind: ObjectKind::Host,
                id: host,
            },
        )
        .await?;
        use crate::operation_scope::sources::{self, Facts};
        use orch_api_contract::host_delete_sources::{CREATE, Source};
        let facts = vec![
            Facts::one(
                Source::Host,
                KeyTargetKind::Object(ObjectKind::Host),
                target,
            ),
            Facts::collection(Source::Managed, vec![]),
            Facts::collection(Source::Used, vec![]),
        ];
        let plan=sources::bind_required(<orch_api_contract::loss::HostDeleteCreate as orch_api_contract::routes::Endpoint>::ROUTE,&CREATE,&facts)?;
        let caller = plan.admit(&caller, false)?;
        let origin = jobs::host_delete::authority::Origin::capture(
            &caller,
            crate::operation_scope::request_origin::Credential::Token {
                id: key,
                hash: digest.as_bytes().to_vec(),
            },
            Some(Time::new(expires)),
        )?;
        let recipe = jobs::host_delete::Recipe {
            host,
            machine: machine.id,
            by: world.ada,
            token: Some(key),
            schedulable: true,
            children: vec![],
            delete_volumes: vec![jobs::host_delete::Volume {
                mount: new_id(),
                name: "native-parent-volume".into(),
                driver: "test".into(),
                storage: None,
            }],
            admin_override: false,
            custody: None,
        };
        let row = rows::insert(
            &mut tx,
            &rows::NewJob {
                id: new_id(),
                kind: jobs::host_delete::KIND,
                node_id: None,
                environment_id: None,
                provider_id: None,
                plan: &json!({"cascade":recipe}),
                progress: &serde_json::to_value(jobs::runner::initial_progress(
                    &jobs::host_delete::PHASES,
                ))?,
                created_by: Some(world.ada),
            },
            world.clock.now(),
        )
        .await?;
        jobs::host_delete::authority::retain(&mut tx, &row, &origin, &caller, world.clock.as_ref())
            .await?;
        let runner = new_id();
        assert!(rows::claim(&mut tx, row.id, runner, expires, world.clock.now()).await?);
        tx.commit().await?;
        Ok(Self {
            job: Job::claimed(engine, row.id, runner),
            world,
            key,
            machine: machine.id,
        })
    }
    async fn proposal(&self) -> TestResult<Outgoing> {
        let mut command = Outgoing::new(
            crate::ids::derived("node.volume.remove", &[self.job.id().as_bytes()]),
            self.machine,
            SerialKey::Volume("native-parent-volume".into()),
            &NodeVolumeRemove {
                driver: "test".into(),
                name: "native-parent-volume".into(),
                storage: None,
                labels: Default::default(),
            },
        )?;
        self.job.remember(&json!({"active_command":{"id":command.id,"machine":command.machine,"kind":command.kind.as_str()}})).await?;
        decorate(&self.job, &mut command, true)
            .await
            .map_err(|stop| format!("native proposal sealing failed: {stop:?}"))?;
        Ok(command)
    }
}
#[tokio::test]
async fn native_lifecycle_proposals_keep_exact_wire_identity_and_current_original_key_at_issue_and_send()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.proposal().await?;
    let authority = command
        .authority
        .as_ref()
        .ok_or("native lifecycle command authority missing")?;
    assert!(matches!(authority, CommandAuthority::Lifecycle(_)));
    let serialized = serde_json::to_value(authority)?;
    let restored: CommandAuthority = serde_json::from_value(serialized.clone())?;
    assert_eq!(&restored, authority);
    let serial = command.key.wire();
    let proposal = link::NewCommand {
        id: command.id,
        machine_id: command.machine,
        kind: command.kind.as_str(),
        body: &command.body,
        serial_key: serial.as_deref(),
        deadline_at: s.world.clock.now() + chrono::Duration::minutes(1),
    };
    let mut tx = s.world.pool.begin().await?;
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &proposal,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_some()
    );
    assert!(
        link::insert_command_with_authority(
            &mut tx,
            &proposal,
            s.world.clock.now(),
            Some(&serialized)
        )
        .await?
    );
    tx.commit().await?;
    let mut tx = s.world.pool.begin().await?;
    let row = link::command(&mut tx, command.id)
        .await?
        .ok_or("native command missing")?;
    assert!(
        authority
            .current_callers(
                &mut tx,
                &row,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_some()
    );
    let mut changed = row.clone();
    changed.body["name"] = json!("another-volume");
    assert!(
        authority
            .current_callers(
                &mut tx,
                &changed,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    tx.commit().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.key)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        authority
            .current_issue_callers(
                &mut tx,
                &proposal,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    assert!(
        authority
            .current_callers(
                &mut tx,
                &row,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none()
    );
    tx.commit().await?;
    sqlx::query("UPDATE jobs SET plan=plan-'host_delete_authority' WHERE id=$1")
        .bind(s.job.id())
        .execute(s.world.pool.pool())
        .await?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        authority
            .current_callers(
                &mut tx,
                &row,
                s.world.clock.as_ref(),
                s.world.access.as_ref()
            )
            .await?
            .is_none(),
        "an already branded proposal cannot fall back to legacy authority"
    );
    Ok(())
}

#[tokio::test]
async fn lifecycle_family_keeps_execution_wire_and_cannot_forge_an_unimplemented_preparation()
-> TestResult {
    let fixture = Fixture::new().await?;
    let command = fixture.proposal().await?;
    let family = command
        .authority
        .ok_or("the real native proposal lost its lifecycle evidence")?;
    let encoded = serde_json::to_value(&family)?;
    assert_eq!(encoded["kind"], json!("lifecycle"));
    let legacy: Command = serde_json::from_value(encoded["authority"].clone())?;
    assert_eq!(legacy.job, fixture.job.id());
    assert_eq!(legacy.actor, fixture.world.ada);
    assert_eq!(legacy.command_kind(), "node.volume.remove");
    assert_eq!(serde_json::from_value::<CommandAuthority>(encoded)?, family);
    for native in [
        json!(null),
        json!({}),
        json!({"operation":"EnvironmentMoveCreate"}),
        json!({"job":legacy.job,"owner":"node_move"}),
    ] {
        let packet = json!({"version":1,"kind":"lifecycle","authority":{"version":"lifecycle_preparation_v1","native":native}});
        assert!(
            serde_json::from_value::<CommandAuthority>(packet).is_err(),
            "a data value or execution identity cannot stand in for an actual native preparation producer"
        );
    }
    Ok(())
}

#[tokio::test]
async fn lifecycle_execution_marker_never_falls_back_to_actor_only_provision() -> TestResult {
    let fixture = Fixture::new().await?;
    let mut tx = fixture.world.pool.begin().await?;
    let row = rows::insert(
        &mut tx,
        &rows::NewJob {
            id: new_id(),
            kind: rows::kind::ENVIRONMENT_PROVISION,
            node_id: None,
            environment_id: Some(fixture.world.environment),
            provider_id: None,
            plan: &json!({"mode":"make","recovery":{"by":fixture.world.ada}}),
            progress: &json!({}),
            created_by: Some(fixture.world.ada),
        },
        fixture.world.clock.now(),
    )
    .await?;
    assert!(!applies(&row));
    assert!(
        jobs::provision::creator_checkpoint(
            &mut tx,
            &row,
            fixture.world.clock.as_ref(),
            fixture.world.access.as_ref()
        )
        .await?
        .is_none()
    );
    for patch in [
        json!({"environment_lifecycle_required":true}),
        json!({"environment_lifecycle_version":1,"environment_lifecycle_required":true,"environment_lifecycle":null}),
        json!({"environment_lifecycle_version":1,"environment_lifecycle_required":true,"environment_lifecycle":{"operation":"EnvironmentRecoverCreate","by":fixture.world.ada}}),
    ] {
        sqlx::query("UPDATE jobs SET plan=$2 WHERE id=$1")
            .bind(row.id)
            .bind(patch)
            .execute(&mut *tx)
            .await?;
        let actual = rows::job(&mut tx, row.id)
            .await?
            .ok_or("the native child disappeared")?;
        assert!(applies(&actual));
        assert!(jobs::provision::retains_creator_authority(&actual));
        let refused = jobs::provision::creator_checkpoint(
            &mut tx,
            &actual,
            fixture.world.clock.as_ref(),
            fixture.world.access.as_ref(),
        )
        .await;
        assert!(matches!(refused, Err(EngineError::BeforeEffect(_))));
    }
    Ok(())
}
