//! Native journal/private evidence and owned compensation under actual stored key withdrawal.

use super::*;
use crate::{EngineParts, test_common::World};
use orch_access_contract::key_scope::{
    KeyAction, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::ObjectKind;
use orch_authz::{Caller, TokenGrant};
use orch_db::queries::{identity, link};
use orch_link_contract::commands::{NodeRuntimeStop, SerialKey};
use orch_types::new_id;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    key: Id,
    host_machine: Id,
    old: Id,
    template: Id,
}
impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::observed().await;
        let host = world
            .add_machine("Environment update native host", &["node"])
            .await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let mut tx = world.pool.begin().await?;
        let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
            .bind(world.environment)
            .fetch_one(&mut *tx)
            .await?;
        let host_id = new_id();
        let old = new_id();
        let batch = new_id();
        let key = new_id();
        sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,'Environment update host',$3,$4,$4,$5,$6)")
            .bind(host_id).bind(host.id).bind(world.ada).bind(world.root)
            .bind(json!({"test":{"spec_schemas":{"environment":{}},"sources":["image"]}})).bind(json!({"memory_mb":4096}))
            .execute(&mut *tx).await?;
        let definition = json!({"image":"environment-update"});
        let hash = crate::updates::definition_hash("test", &definition, &json!({}), &[]);
        envs::insert_runtime(
            &mut tx,
            &envs::NewRuntime {
                id: old,
                host_id,
                unit_name: "environment-update-old",
                environment_id: world.environment,
                driver: "test",
                template_id: template,
                spec_hash: hash.as_bytes(),
                generation: 1,
                volumes: &json!({}),
            },
            world.clock.now(),
        )
        .await?;
        envs::set_runtime_state(
            &mut tx,
            old,
            &["creating"],
            "running",
            None,
            world.clock.now(),
        )
        .await?;
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Environment),
                    id: Some(world.environment),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![KeyTargetKind::Object(ObjectKind::Environment)]),
                allow: vec![KeyOperation {
                    operation: OP.into(),
                    actions: vec![KeyAction::Manage],
                }],
            }],
            ..Default::default()
        };
        let digest = Hash::of(key.as_bytes());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: key,
                user_id: world.ada,
                name: "Environment update native evidence",
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
                kind: ObjectKind::Environment,
                id: world.environment,
            },
        )
        .await?;
        let facts = vec![
            crate::operation_scope::sources::Facts::collection(
                orch_api_contract::environment_sources::update_batch::Source::Environments,
                vec![(KeyTargetKind::Object(ObjectKind::Environment), vec![target])],
            ),
            crate::operation_scope::sources::Facts::collection(
                orch_api_contract::environment_sources::update_batch::Source::Templates,
                Vec::new(),
            ),
        ];
        let plan=crate::operation_scope::sources::bind_required(
            <orch_api_contract::environment::NodeEnvironmentUpdateCreate as orch_api_contract::routes::Endpoint>::ROUTE,
            &orch_api_contract::environment_sources::update_batch::NODE_CREATE,&facts)?;
        let caller = plan.admit(&caller, false)?;
        let source = Input::capture(
            &mut tx,
            &caller,
            crate::operation_scope::request_origin::Credential::Token {
                id: key,
                hash: digest.as_bytes().to_vec(),
            },
            Some(Time::new(expires)),
            world.root,
            world.environment,
        )
        .await?
        .for_execution(template);
        let recipe = Recipe {
            volume_carry: None,
            provider_home_storage: None,
            data_sources: None,
            capability: None,
            capability_reason: None,
            capability_grace_secs: None,
            preserve_files: false,
            keep_estimate: None,
            target: Target::Environment(world.environment),
            old,
            by: world.ada,
            reason: UpdatePreflightReason::Update,
            template,
            definition,
            network_policy: Default::default(),
            resolved_resources: json!({}),
            tools: vec![],
            hash: hash.as_bytes().to_vec(),
            options: crate::updates::options(
                &mut tx,
                Target::Environment(world.environment).place(),
                None,
            )
            .await?,
            threads: vec![],
            was_stopped: false,
            old_facts: json!({}),
            auto_start: false,
            host: None,
        };
        let row = super::super::super::queue(
            &mut tx,
            Draft {
                recipe,
                updates: vec![],
                kept: vec![],
                lost: vec![],
                blockers: vec![],
            },
            Some(batch),
            world.clock.now(),
        )
        .await?;
        retain(
            &mut tx,
            row.id,
            batch,
            &source,
            Time::new(world.clock.now()),
        )
        .await?;
        let runner = new_id();
        assert!(jobs::claim(&mut tx, row.id, runner, expires, world.clock.now()).await?);
        tx.commit().await?;
        let job = Job::claimed(engine.clone(), row.id, runner);
        Ok(Self {
            world,
            engine,
            job,
            key,
            host_machine: host.id,
            old,
            template,
        })
    }
    fn stop(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            crate::ids::derived("node.runtime.stop", &[self.job.id().as_bytes(), b"replace"]),
            self.host_machine,
            SerialKey::Runtime(self.old),
            &NodeRuntimeStop {
                runtime_id: self.old,
                reason: Some(orch_driver_contract::runtime::UnitReason::Recreate),
            },
        )?)
    }
    async fn prepare(&self, command: &mut Outgoing) -> TestResult {
        self.job.remember(&json!({"active_command":{"id":command.id,"machine":command.machine,"kind":command.kind.as_str()}})).await?;
        super::super::remember_command(&self.job, command)
            .await
            .map_err(|stop| format!("native command journaling failed: {stop:?}"))?;
        seal(&self.job, command)
            .await
            .map_err(|stop| format!("native command sealing failed: {stop:?}"))?;
        Ok(())
    }
}

#[tokio::test]
async fn exact_native_update_proposals_keep_a_real_private_source_and_cannot_fall_back_after_metadata_loss()
-> TestResult {
    let s = Fixture::new().await?;
    let mut command = s.stop()?;
    s.prepare(&mut command).await?;
    let authority = command
        .authority
        .as_ref()
        .ok_or("private update authority missing")?;
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
            Some(&serde_json::to_value(authority)?)
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
    changed.body["runtime_id"] = json!(new_id());
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
    sqlx::query("UPDATE jobs SET plan=plan-'environment_update'-'environment_update_commands'-'environment_update_composed_commands' WHERE id=$1")
        .bind(s.job.id()).execute(s.world.pool.pool()).await?;
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
        "the private command remains known even after every job hint was cleared"
    );
    tx.commit().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn withdrawn_update_keys_still_allow_only_the_owned_unpublished_candidate_to_be_compensated()
-> TestResult {
    let s = Fixture::new().await?;
    let next = new_id();
    let mut tx = s.world.pool.begin().await?;
    let old = envs::runtime(&mut tx, s.old)
        .await?
        .ok_or("old runtime missing")?;
    let row = jobs::job(&mut tx, s.job.id()).await?.ok_or("job missing")?;
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    env_changes::successor(
        &mut tx,
        &old,
        next,
        "owned-update-candidate",
        s.template,
        &recipe.hash,
        s.world.clock.now(),
    )
    .await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"runtime":next,"host":old.host_id}),
        s.world.clock.now(),
    )
    .await?;
    tx.commit().await?;
    sqlx::query("UPDATE api_tokens SET revoked_at=$2 WHERE id=$1")
        .bind(s.key)
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        cleanup_guard(&s.job).await.is_ok(),
        "credential withdrawal must not strand the owned candidate"
    );
    let mut tx = s.world.pool.begin().await?;
    assert!(env_changes::commit(&mut tx, s.old, next, None, s.world.clock.now()).await?);
    tx.commit().await?;
    assert!(
        cleanup_guard(&s.job).await.is_err(),
        "a published/foreign generation is never compensation for the old job"
    );
    assert_eq!(
        s.world.commands.issued_of("node.runtime.remove"),
        0,
        "these controls validate ownership without issuing a physical deletion"
    );
    s.engine.stop();
    Ok(())
}

#[test]
fn image_and_volume_bindings_treat_the_retained_driver_as_opaque_native_command_data() -> TestResult
{
    use orch_link_contract::commands::{NodeImageEnsure, NodeVolumeEnsure};
    let job = new_id();
    let spec = json!({"source":"opaque-native-source"});
    for driver in ["first-plugin", "a-new-unlisted-plugin"] {
        let image = serde_json::to_value(NodeImageEnsure {
            driver: driver.into(),
            spec: spec.clone(),
            job_id: Some(job),
        })?;
        assert!(image_matches(driver, &spec, job, &image)?);
        assert!(!image_matches("different-plugin", &spec, job, &image)?);
        assert!(!image_matches(
            driver,
            &json!({"source":"another-source"}),
            job,
            &image
        )?);
        let volume = serde_json::to_value(NodeVolumeEnsure {
            driver: driver.into(),
            name: "native-volume".into(),
            storage: Some("storage-a".into()),
            labels: std::collections::BTreeMap::new(),
        })?;
        let volumes = json!({"workspace":{"name":"native-volume","storage":"storage-a"}});
        let key = SerialKey::Volume("native-volume".into()).wire();
        assert!(volume_matches(driver, &volumes, key.as_deref(), &volume));
        assert!(!volume_matches(
            "different-plugin",
            &volumes,
            key.as_deref(),
            &volume
        ));
        assert!(!volume_matches(
            driver,
            &volumes,
            SerialKey::Volume("different-volume".into())
                .wire()
                .as_deref(),
            &volume
        ));
        assert!(!volume_matches(
            driver,
            &json!({"workspace":{"name":"native-volume","storage":"storage-b"}}),
            key.as_deref(),
            &volume
        ));
    }
    Ok(())
}
