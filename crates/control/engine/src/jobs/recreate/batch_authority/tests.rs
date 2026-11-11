//! Queued provider batch members keep independent preview/create origins and exact native bindings.

mod owner_host_use;
mod receipt_identity;
use super::*;
use crate::commands::Outgoing;
use crate::jobs::recreate::{Recreating, queue};
use crate::jobs::runner::{Failure, Job, Stop};
use crate::operation_scope::targets;
use crate::updates::{Draft, Recipe, Target};
use crate::{
    Engine, EngineParts,
    test_common::{Added, World},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::environment::UpdatePreflightReason;
use orch_api_contract::workspace::PhaseState;
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::environments::{changes, hosting as envs};
use orch_db::queries::resources::{hosting as hosts, provider_runtime as providers};
use orch_db::queries::{identity, jobs};
use orch_link_contract::commands::{
    CommandKind, CommandResult, Empty, NodeImageEnsure, NodeImageEnsureResult, NodeRuntimeStop,
    SerialKey, TurnSteer,
};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;
const OPERATIONS: [&str; 2] = [
    "ProviderRuntimeUpdateBatchPreflight",
    "ProviderRuntimeUpdateBatchCreate",
];

#[tokio::test]
async fn owned_candidate_health_reaches_the_existing_enrollment_failure_boundary() -> TestResult {
    let s = Fixture::new().await?;
    sqlx::query(
        "UPDATE runtimes SET state='failed',state_detail='Synthetic candidate failure' WHERE id=$1",
    )
    .bind(s.candidate)
    .execute(s.world.pool.pool())
    .await?;
    assert!(
        s.current().await?,
        "health failure does not erase this job's unchanged owned identity"
    );
    let row = s.job.row().await?;
    let provision = crate::jobs::provider_provision::ProviderProvision::of(s.engine.clone(), &row);
    let result = crate::jobs::runner::Phased::run(&provision, &s.job, "enroll").await;
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="unit_failed"),
        "the normal handler reports unit failure instead of a fabricated scope refusal: {result:?}"
    );
    let mut conn = s.world.pool.acquire().await?;
    assert_eq!(
        providers::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    assert_eq!(s.world.commands.issued_of("node.runtime.start"), 0);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    batch: Id,
    keys: [Id; 2],
    origins: Vec<CapturedOperation>,
    policies: Vec<KeyScope>,
    old: Id,
    candidate: Id,
    old_host: Added,
    target_host: Added,
    candidate_connector: Added,
    notice_turn: Option<(Id, Id)>,
    same_host: bool,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        Self::with_legacy_preview(false).await
    }

    async fn ordinary() -> TestResult<Self> {
        Self::placed_with_origins(false, false, false, false).await
    }

    async fn with_legacy_preview(legacy: bool) -> TestResult<Self> {
        Self::configured(legacy, false).await
    }

    async fn with_notice() -> TestResult<Self> {
        Self::configured(false, true).await
    }

    async fn configured(legacy: bool, with_notice: bool) -> TestResult<Self> {
        Self::placed(legacy, with_notice, false).await
    }

    async fn same_host() -> TestResult<Self> {
        Self::placed(false, false, true).await
    }

    async fn placed(legacy: bool, with_notice: bool, same_host: bool) -> TestResult<Self> {
        Self::placed_with_origins(legacy, with_notice, same_host, true).await
    }

    async fn placed_with_origins(
        legacy: bool,
        with_notice: bool,
        same_host: bool,
        scoped: bool,
    ) -> TestResult<Self> {
        let world = World::observed().await;
        let notice_turn = if with_notice {
            let thread = world.thread("Provider batch notice recipient").await;
            let (turn, _) = world
                .running_turn(thread, "Continue while the provider update is reviewed")
                .await;
            Some((thread, turn.turn_id))
        } else {
            None
        };
        let old_host = world
            .add_machine("Old provider batch host", &["node"])
            .await;
        let target_host = world
            .add_machine("Target provider batch host", &["node"])
            .await;
        let candidate_connector = world
            .add_machine("Provider batch candidate", &["provider", "agent"])
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
        let (old_host_id, target_host_id, old_template, template, old, candidate, batch) = (
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
            new_id(),
        );
        let mut tx = world.pool.begin().await?;
        for (id, machine, name) in [
            (old_host_id, old_host.id, "Old batch host"),
            (target_host_id, target_host.id, "Target batch host"),
        ] {
            sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,$3,$4,$5,$5,$6,$7)")
                .bind(id).bind(machine).bind(name).bind(world.ada).bind(world.root)
                .bind(json!({"test":{"spec_schemas":{"provider":{}},"sources":["image"]}}))
                .bind(json!({"memory_mb":4096})).execute(&mut *tx).await?;
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Host.into(),
                    id,
                },
                world.ada,
                NewAccess::OnlyMe,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        let old_definition = json!({"image":"provider-batch-old"});
        let definition = json!({"image":"provider-batch-new"});
        for (id, name, spec) in [
            (old_template, "Old batch template", &old_definition),
            (template, "Target batch template", &definition),
        ] {
            sqlx::query("INSERT INTO env_templates(id,name,owner_id,org_id,home_id,kind,driver,spec) VALUES($1,$2,$3,$4,$4,'provider','test',$5)")
                .bind(id).bind(name).bind(world.ada).bind(world.root).bind(spec).execute(&mut *tx).await?;
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Template.into(),
                    id,
                },
                world.ada,
                NewAccess::OnlyMe,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        let selected_template = if legacy { old_template } else { template };
        let selected_host = if same_host {
            old_host_id
        } else {
            target_host_id
        };
        if legacy {
            // Preserve ordinary legacy acceptance: update the existing template's definition,
            // without selecting a new template that a scalar key cannot directly reach.
            sqlx::query("UPDATE env_templates SET spec=$2 WHERE id=$1")
                .bind(old_template)
                .bind(&definition)
                .execute(&mut *tx)
                .await?;
        }
        let old_hash = crate::updates::definition_hash("test", &old_definition, &json!({}), &[]);
        providers::insert_runtime(
            &mut tx,
            &providers::NewRuntime {
                id: old,
                host_id: old_host_id,
                unit_name: "batch-old-provider",
                provider_id: world.provider,
                driver: "test",
                template_id: old_template,
                spec_hash: old_hash.as_bytes(),
                generation: 1,
                volumes: &json!({"state":{"name":"batch-old-provider-state","storage":null}}),
            },
            world.clock.now(),
        )
        .await?;
        assert!(hosts::adopt_machine(&mut tx, world.machine_id, old, world.clock.now()).await?);
        assert!(
            envs::set_runtime_state(
                &mut tx,
                old,
                &["creating"],
                "running",
                None,
                world.clock.now()
            )
            .await?
        );
        sqlx::query("UPDATE providers SET status='ready',runtime_config=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(json!({"runtime.host_id":selected_host,"runtime.template_id":selected_template}))
            .execute(&mut *tx)
            .await?;
        let hash = crate::updates::definition_hash("test", &definition, &json!({}), &[]);
        let recipe = Recipe {
            volume_carry: None,
            provider_home_storage: None,
            data_sources: None,
            capability: None,
            capability_reason: None,
            capability_grace_secs: None,
            preserve_files: false,
            keep_estimate: None,
            target: Target::Provider(world.provider),
            old,
            by: world.ada,
            reason: UpdatePreflightReason::Update,
            template: selected_template,
            definition,
            network_policy: Default::default(),
            resolved_resources: json!({}),
            tools: vec![],
            hash: hash.as_bytes().to_vec(),
            options: crate::updates::options(
                &mut tx,
                Target::Provider(world.provider).place(),
                None,
            )
            .await?,
            threads: vec![],
            was_stopped: false,
            old_facts: providers::state(&mut tx, world.provider)
                .await?
                .ok_or("provider absent")?
                .facts,
            auto_start: false,
            host: (!same_host).then_some(target_host_id),
        };
        let row = queue(
            &mut tx,
            Draft {
                recipe,
                updates: vec![],
                kept: vec![],
                lost: vec![],
                blockers: vec![],
            },
            scoped.then_some(batch),
            world.clock.now(),
        )
        .await?;
        let keys = [new_id(), new_id()];
        let mut origins = Vec::new();
        let mut policies = Vec::new();
        let operations = if scoped { OPERATIONS.as_slice() } else { &[] };
        for (index, operation) in operations.iter().copied().enumerate() {
            let policy = KeyScope {
                rules: vec![KeyRule {
                    target: KeyTarget {
                        kind: KeyTargetKind::Object(ObjectKind::Provider),
                        id: Some(world.provider),
                        reach: KeyReach::SelfOnly,
                    },
                    resources: Some(vec![KeyTargetKind::Object(ObjectKind::Provider)]),
                    allow: vec![KeyOperation {
                        operation: operation.into(),
                        actions: vec![KeyAction::Manage],
                    }],
                }],
                ..KeyScope::default()
            };
            // Legacy preview keeps its existing template, as ordinary draft permits;
            // this test does not borrow ProviderCreate's internal-placement compatibility rule.
            let legacy = index == 0 && legacy;
            let scopes = if legacy {
                vec!["provider.manage".to_owned()]
            } else {
                vec![]
            };
            let subtree = legacy.then_some(world.root);
            let explicit = (!legacy).then(|| policy.clone());
            let stored = explicit.as_ref().map(|scope| json!(scope));
            let expires = world.clock.now() + chrono::Duration::hours(1);
            identity::create_api_token_scoped(
                &mut tx,
                &identity::NewApiToken {
                    id: keys[index],
                    user_id: world.ada,
                    name: operation,
                    token_hash: Hash::of(keys[index].as_bytes()).as_bytes(),
                    scopes: &scopes,
                    node_id: subtree,
                    at: world.clock.now(),
                    expires_at: Some(expires),
                },
                stored.as_ref(),
            )
            .await?;
            let target = targets::page_object(
                &mut tx,
                ObjectRef {
                    kind: ResourceKind::Provider.into(),
                    id: world.provider,
                },
            )
            .await?;
            let caller = Caller::token(
                world.ada,
                TokenGrant::from_stored_scope(
                    &scopes,
                    subtree,
                    explicit,
                    Some(Time::new(expires)),
                )?
                .for_token(keys[index]),
            )
            .admit_facets(&[KeyFacet {
                operation: operation.into(),
                action: KeyAction::Manage,
                resource_kind: KeyTargetKind::Object(ObjectKind::Provider),
                target,
            }])?
            .into_caller();
            origins.push(CapturedOperation::capture(&caller)?);
            policies.push(policy);
        }
        if scoped {
            retain(
                &mut tx,
                row.id,
                batch,
                &origins,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        if !with_notice {
            let current = envs::runtime(&mut tx, old)
                .await?
                .ok_or("old runtime absent")?;
            let mut placed = current.clone();
            placed.host_id = selected_host;
            placed.machine_id = None;
            if !same_host {
                placed.volumes =
                    json!({"state":{"name":"batch-candidate-provider-state","storage":null}});
            }
            changes::successor(
                &mut tx,
                &placed,
                candidate,
                "batch-provider-candidate",
                selected_template,
                &hash.as_bytes()[..],
                world.clock.now(),
            )
            .await?;
            assert!(
                hosts::adopt_machine(
                    &mut tx,
                    candidate_connector.id,
                    candidate,
                    world.clock.now()
                )
                .await?
            );
            assert!(
                envs::set_runtime_state(
                    &mut tx,
                    old,
                    &["running"],
                    "stopped",
                    None,
                    world.clock.now()
                )
                .await?
            );
            jobs::merge_plan(&mut tx,row.id,&json!({"runtime":candidate,"host":selected_host,"carry":!same_host,"fresh":false,"mode":"make","old_stop_requested":true}),world.clock.now()).await?;
        }
        let runner = new_id();
        assert!(
            jobs::claim(
                &mut tx,
                row.id,
                runner,
                world.clock.now() + chrono::Duration::hours(1),
                world.clock.now()
            )
            .await?
        );
        tx.commit().await?;
        let job = Job::claimed(engine.clone(), row.id, runner);
        // Reconstruct the ordinary post-allocation checkpoint. Both earlier phases have no
        // affected threads here, so neither notices nor an interrupt decision is invented.
        if !with_notice {
            job.amend(|progress, _| {
                progress.phase = "replace".into();
                for phase in &mut progress.phases {
                    if matches!(phase.name.as_str(), "notify" | "lock") {
                        phase.state = PhaseState::Done;
                    } else if phase.name == "replace" {
                        phase.state = PhaseState::Running;
                    }
                }
            })
            .await?;
        }
        Ok(Self {
            world,
            engine,
            job,
            batch,
            keys,
            origins,
            policies,
            old,
            candidate,
            old_host,
            target_host,
            candidate_connector,
            notice_turn,
            same_host,
        })
    }

    async fn current(&self) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        let row = jobs::job(&mut tx, self.job.id())
            .await?
            .ok_or("batch job absent")?;
        let allowed = current(&mut tx, &row, Time::new(self.world.clock.now())).await?;
        tx.rollback().await?;
        Ok(allowed)
    }

    fn image(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            crate::ids::derived("node.image.ensure", &[self.job.id().as_bytes()]),
            if self.same_host {
                self.old_host.id
            } else {
                self.target_host.id
            },
            SerialKey::Image("provider-batch-new".into()),
            &NodeImageEnsure {
                driver: "test".into(),
                spec: json!({"image":"provider-batch-new"}),
                job_id: Some(self.job.id()),
            },
        )?)
    }

    fn old_stop(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            crate::ids::derived("node.runtime.stop", &[self.job.id().as_bytes(), b"replace"]),
            self.old_host.id,
            SerialKey::Runtime(self.old),
            &NodeRuntimeStop {
                runtime_id: self.old,
                reason: Some(orch_driver_contract::runtime::UnitReason::Recreate),
            },
        )?)
    }

    async fn reserve(&self, command: &Outgoing) -> TestResult {
        let phase = if command.kind == CommandKind::TurnSteer {
            "notify"
        } else {
            "replace"
        };
        self.job.remember(&json!({"active_command":{"id":command.id,"machine":command.machine,"kind":command.kind.as_str(),"phase":phase}})).await?;
        remember_command(&self.job, command)
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    }

    async fn allows(&self, command: &Outgoing) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        let body: Json = sqlx::query_scalar("SELECT $1::jsonb")
            .bind(&command.body)
            .fetch_one(&mut *tx)
            .await?;
        let allowed = command_authorized(
            &mut tx,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &body,
            Time::new(self.world.clock.now()),
        )
        .await?;
        tx.rollback().await?;
        Ok(allowed)
    }
}

#[tokio::test]
async fn queued_member_requires_both_distinct_original_operations_and_their_current_keys()
-> TestResult {
    let s = Fixture::new().await?;
    assert_eq!(
        s.origins
            .iter()
            .map(CapturedOperation::operation)
            .collect::<Vec<_>>(),
        OPERATIONS
    );
    assert!(s.current().await?);
    let before = s.job.row().await?.plan;
    for index in 0..2 {
        for restriction in ["revoked", "narrowed", "wrong_operation"] {
            let mut tx = s.world.pool.begin().await?;
            match restriction {
                "revoked" => {
                    assert!(
                        identity::revoke_api_token(
                            &mut tx,
                            s.keys[index],
                            s.world.ada,
                            s.world.clock.now()
                        )
                        .await?
                    );
                }
                "narrowed" => {
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(s.keys[index])
                        .bind(json!(KeyScope::default()))
                        .execute(&mut *tx)
                        .await?;
                }
                "wrong_operation" => {
                    let mut wrong = s.policies[index].clone();
                    wrong.rules[0].allow[0].operation = OPERATIONS[1 - index].into();
                    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
                        .bind(s.keys[index])
                        .bind(json!(wrong))
                        .execute(&mut *tx)
                        .await?;
                }
                _ => unreachable!(),
            }
            let row = jobs::job(&mut tx, s.job.id())
                .await?
                .ok_or("batch job absent")?;
            assert!(
                !current(&mut tx, &row, Time::new(s.world.clock.now())).await?,
                "origin {index}, {restriction}"
            );
            tx.rollback().await?;
            assert!(s.current().await?);
        }
    }
    assert_eq!(s.job.row().await?.plan, before);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn member_keeps_current_home_account_rights_and_exact_batch_old_and_candidate_associations()
-> TestResult {
    let s = Fixture::with_legacy_preview(true).await?;
    assert!(s.current().await?);
    for mismatch in [
        "scalar_home",
        "account_manage",
        "batch",
        "old",
        "candidate_generation",
        "candidate_host",
    ] {
        let mut tx = s.world.pool.begin().await?;
        match mismatch {
            "scalar_home" => {
                sqlx::query("UPDATE api_tokens SET node_id=$2 WHERE id=$1")
                    .bind(s.keys[0])
                    .bind(s.world.topic)
                    .execute(&mut *tx)
                    .await?;
            }
            "account_manage" => {
                let page =
                    orch_db::queries::access::PageKey::new("provider", s.world.provider, "live");
                let entries = orch_db::queries::access::entries_of(&mut tx, &page).await?;
                assert!(!entries.is_empty());
                for entry in entries {
                    orch_db::queries::access::remove_entry(
                        &mut tx,
                        entry.id,
                        s.world.ada,
                        s.world.clock.now(),
                    )
                    .await?;
                }
                orch_db::queries::access::testing::set_row(
                    &mut tx,
                    &page,
                    "any",
                    Permission::ProviderManage.as_str(),
                    false,
                    false,
                )
                .await?;
            }
            "batch" => {
                sqlx::query("UPDATE jobs SET batch_id=NULL WHERE id=$1")
                    .bind(s.job.id())
                    .execute(&mut *tx)
                    .await?;
            }
            "old" => {
                jobs::merge_plan(
                    &mut tx,
                    s.job.id(),
                    &json!({"old":s.candidate}),
                    s.world.clock.now(),
                )
                .await?;
            }
            "candidate_generation" => {
                sqlx::query("UPDATE runtimes SET generation=3 WHERE id=$1")
                    .bind(s.candidate)
                    .execute(&mut *tx)
                    .await?;
            }
            "candidate_host" => {
                let old = envs::runtime(&mut tx, s.old)
                    .await?
                    .ok_or("old runtime absent")?;
                sqlx::query("UPDATE runtimes SET host_id=$2 WHERE id=$1")
                    .bind(s.candidate)
                    .bind(old.host_id)
                    .execute(&mut *tx)
                    .await?;
            }
            _ => unreachable!(),
        }
        let row = jobs::job(&mut tx, s.job.id())
            .await?
            .ok_or("batch job absent")?;
        assert!(
            !current(&mut tx, &row, Time::new(s.world.clock.now())).await?,
            "{mismatch}"
        );
        tx.rollback().await?;
        assert!(s.current().await?);
    }
    assert_eq!(s.job.row().await?.batch_id, Some(s.batch));
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn canonical_member_commands_keep_old_ids_without_lending_source_or_candidate_identity()
-> TestResult {
    let s = Fixture::new().await?;
    let source = s.old_stop()?;
    s.reserve(&source).await?;
    assert!(s.allows(&source).await?);
    let target = s.image()?;
    s.reserve(&target).await?;
    assert!(s.allows(&target).await?);
    assert!(
        !s.allows(&source).await?,
        "a retained older source ID fails the current active fence"
    );
    for mismatch in ["machine", "key", "kind", "body"] {
        let mut changed = target.clone();
        match mismatch {
            "machine" => changed.machine = s.old_host.id,
            "key" => changed.key = SerialKey::Image("different-image".into()),
            "kind" => changed.kind = CommandKind::NodeRuntimeStart,
            "body" => changed.body["spec"]["image"] = json!("different-image"),
            _ => unreachable!(),
        }
        assert!(!s.allows(&changed).await?, "{mismatch}");
    }
    let mut changed = target.clone();
    changed.body["spec"]["image"] = json!("different-image");
    assert!(remember_command(&s.job, &changed).await.is_err());
    assert!(s.allows(&target).await?);
    let mut tx = s.world.pool.begin().await?;
    sqlx::query("UPDATE jobs SET plan=plan-'provider_update_origins' WHERE id=$1")
        .bind(s.job.id())
        .execute(&mut *tx)
        .await?;
    assert!(
        !command_authorized(
            &mut tx,
            target.id,
            target.machine,
            target.key.wire().as_deref(),
            target.kind.as_str(),
            &target.body,
            Time::new(s.world.clock.now())
        )
        .await?,
        "retained command evidence without its origins cannot become ordinary work"
    );
    tx.rollback().await?;
    s.job.remember(&json!({"active_command":null})).await?;
    assert!(!s.allows(&target).await?);
    s.reserve(&target).await?;
    let why = Stop::Failed(Failure::new(
        "access.denied",
        "The accepted batch authority was withdrawn.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    assert!(!s.allows(&target).await?);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn accepted_and_terminal_member_results_survive_revocation_but_new_work_does_not()
-> TestResult {
    let s = Fixture::new().await?;
    s.target_host
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    s.target_host
        .machine
        .handler
        .on(CommandKind::NodeImageEnsure, |_| {
            Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                digest: Some("retained-provider-batch-image".into()),
                built: None,
                programs: std::collections::BTreeMap::new(),
            }))
        });
    let command = s.image()?;
    let replay = command.clone();
    let id = command.id;
    let job = s.job.clone();
    let running = tokio::spawn(async move { job.command(command).await });
    s.target_host.machine.handler.started(id).await;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.keys[0], s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&replay).await?);
    s.target_host.machine.handler.release();
    let result = running.await?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result["digest"], json!("retained-provider-batch-image"));
    assert_eq!(
        s.job
            .command(replay)
            .await
            .map_err(|error| format!("{error:?}"))?,
        result
    );
    assert_eq!(s.target_host.machine.handler.runs_of(id), 1);
    assert!(s.job.command(s.old_stop()?).await.is_err());
    assert_eq!(s.world.commands.issued_of("node.runtime.stop"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn lost_member_authority_does_not_prevent_owned_candidate_cleanup_and_old_restart()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.keys[1], s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&command).await?);
    assert!(s.job.command(s.old_stop()?).await.is_err());
    let why = Stop::Failed(Failure::new(
        "access.denied",
        "The batch consumer credential was revoked.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    s.target_host
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Ok(CommandResult::NodeRuntimeRemove(Empty {}))
        });
    s.old_host
        .machine
        .handler
        .on(CommandKind::NodeRuntimeStart, |_| {
            Ok(CommandResult::NodeRuntimeStart(Empty {}))
        });
    let recreate = Recreating::of(&s.job.row().await?)?;
    assert_eq!(
        recreate
            .rollback(&s.job)
            .await
            .map_err(|error| format!("{error:?}"))?,
        None
    );
    let removed = s.world.commands.issued_commands("node.runtime.remove");
    assert_eq!(removed.len(), 1);
    assert_eq!(removed[0].0, s.target_host.id);
    assert_eq!(removed[0].1.body["runtime_id"], json!(s.candidate));
    let started = s.world.commands.issued_commands("node.runtime.start");
    assert_eq!(started.len(), 1);
    assert_eq!(started[0].0, s.old_host.id);
    assert_eq!(started[0].1.body["runtime_id"], json!(s.old));
    let mut conn = s.world.pool.acquire().await?;
    assert_eq!(
        providers::live_runtime(&mut conn, s.world.provider)
            .await?
            .ok_or("old provider absent")?
            .id,
        s.old
    );
    assert_eq!(
        providers::state(&mut conn, s.world.provider)
            .await?
            .ok_or("provider absent")?
            .machine_id,
        Some(s.world.machine_id)
    );
    assert_eq!(
        envs::runtime(&mut conn, s.candidate)
            .await?
            .ok_or("candidate absent")?
            .state,
        "removed"
    );
    assert_eq!(s.candidate_connector.machine.handler.runs_of(command.id), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn committed_notice_keeps_only_its_exact_steer_intent_after_consumer_revocation() -> TestResult
{
    let s = Fixture::with_notice().await?;
    let (thread, turn) = s.notice_turn.ok_or("notice recipient absent")?;
    let message = "Its provider runtime is being updated.";
    let command = Outgoing::new(
        crate::ids::derived(
            "turn.steer",
            &[s.job.id().as_bytes(), thread.as_bytes(), message.as_bytes()],
        ),
        s.world.machine_id,
        SerialKey::Thread(thread),
        &TurnSteer {
            thread_id: thread,
            turn_id: turn,
            text: message.into(),
            images: vec![],
            native_action: None,
        },
    )?;
    let mut tx = s.world.pool.begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let owner = s
        .job
        .lock_owned(&mut tx)
        .await?
        .ok_or("notice job claim absent")?;
    let recipe: Recipe = serde_json::from_value(owner.plan.clone())?;
    assert!(
        recipe.threads.contains(&thread),
        "the normal queue froze the actual affected running thread"
    );
    assert!(current(&mut tx, &owner, Time::new(s.world.clock.now())).await?);
    changes::notice(&mut tx, thread, message, s.world.clock.now()).await?;
    retain_notice(&mut tx, &owner, &command, s.world.clock.as_ref()).await?;
    tx.commit().await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.keys[1], s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.current().await?,
        "revocation prevents any new forward decision"
    );
    // This native identity has not been issued or acknowledged. Its one authorization was
    // accepted atomically with the notice and must survive only as that exact continuation.
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    for mismatch in ["turn", "body", "machine"] {
        let mut changed = command.clone();
        match mismatch {
            "turn" => changed.body["turn_id"] = json!(new_id()),
            "body" => changed.body["text"] = json!("A different notice"),
            "machine" => changed.machine = s.candidate_connector.id,
            _ => unreachable!(),
        }
        assert!(!s.allows(&changed).await?, "{mismatch}");
    }
    let mut another = command.clone();
    another.id = new_id();
    s.job
        .remember(
            &json!({"active_command":{"id":another.id,"machine":another.machine,
        "kind":another.kind.as_str(),"phase":"notify"}}),
        )
        .await?;
    assert!(
        remember_command(&s.job, &another).await.is_err(),
        "a new native identity was never accepted with the notice"
    );
    assert!(!s.allows(&another).await?);
    let image = s.image()?;
    s.job
        .remember(
            &json!({"active_command":{"id":image.id,"machine":image.machine,
        "kind":image.kind.as_str(),"phase":"replace"}}),
        )
        .await?;
    assert!(
        remember_command(&s.job, &image).await.is_err(),
        "an accepted steer intent cannot authorize later provisioning"
    );
    assert!(!s.allows(&image).await?);
    s.reserve(&command).await?;
    assert!(
        s.allows(&command).await?,
        "refused new work preserves the original accepted intent"
    );
    assert_eq!(s.world.commands.issued_of("turn.steer"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn same_host_materialization_keeps_the_accepted_selection_without_allowing_another_host()
-> TestResult {
    let s = Fixture::same_host().await?;
    let owner = s.job.row().await?;
    let actual: Recipe = serde_json::from_value(owner.plan.clone())?;
    let mut conn = s.world.pool.acquire().await?;
    let old = envs::runtime(&mut conn, s.old)
        .await?
        .ok_or("same-host old runtime absent")?;
    let candidate = envs::runtime(&mut conn, s.candidate)
        .await?
        .ok_or("same-host candidate absent")?;
    assert_eq!(
        actual.host,
        Some(old.host_id),
        "normal replace materializes its resolved host in the existing plan"
    );
    assert_eq!(candidate.host_id, old.host_id);
    assert_eq!(
        candidate.volumes, old.volumes,
        "same-host replacement keeps existing persistent volumes"
    );
    let other: Id = sqlx::query_scalar("SELECT id FROM hosts WHERE machine_id=$1")
        .bind(s.target_host.id)
        .fetch_one(&mut *conn)
        .await?;
    drop(conn);
    assert!(s.current().await?);
    let command = s.image()?;
    assert_eq!(command.machine, s.old_host.id);
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    let mut tx = s.world.pool.begin().await?;
    sqlx::query("UPDATE runtimes SET host_id=$2 WHERE id=$1")
        .bind(s.candidate)
        .bind(other)
        .execute(&mut *tx)
        .await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"host":other}),
        s.world.clock.now(),
    )
    .await?;
    let changed = jobs::job(&mut tx, s.job.id())
        .await?
        .ok_or("same-host job absent")?;
    assert!(
        !current(&mut tx, &changed, Time::new(s.world.clock.now())).await?,
        "normalizing the materialized host must not authorize a different selected destination"
    );
    assert!(
        !command_authorized(
            &mut tx,
            command.id,
            command.machine,
            command.key.wire().as_deref(),
            command.kind.as_str(),
            &command.body,
            Time::new(s.world.clock.now())
        )
        .await?
    );
    tx.rollback().await?;
    assert!(s.current().await?);
    assert!(s.allows(&command).await?);
    assert_eq!(s.job.row().await?.plan, before);
    s.engine.stop();
    Ok(())
}
