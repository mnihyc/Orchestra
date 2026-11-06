//! A pending provider birth keeps its original key and its exact native command identities.
use super::*;
use crate::commands::Outgoing;
use crate::jobs::provider_provision::{Mode, ProviderProvision};
use crate::jobs::runner::{Failure, Job, Stop, Undone};
use crate::operation_scope::targets;
use crate::{
    Engine, EngineParts,
    test_common::{Added, World},
};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{
    KeyAction, KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_authz::ownership::{self, NewAccess};
use orch_authz::{Caller, CapturedOperation, TokenGrant};
use orch_db::queries::resources::provider_runtime as provider_rows;
use orch_db::queries::{identity, jobs};
use orch_link_contract::commands::{
    CommandKind, CommandResult, Empty, NodeImageEnsure, NodeImageEnsureResult, SerialKey,
};
use orch_types::{Hash, Id, Json, Time, new_id};
use serde_json::json;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    token: Id,
    origin: CapturedOperation,
    policy: KeyScope,
    runtime: Id,
    host_machine: Added,
    placement_home: Id,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        Self::with_legacy(false).await
    }

    async fn legacy_unplaced() -> TestResult<Self> {
        Self::with_legacy(true).await
    }

    async fn with_legacy(legacy: bool) -> TestResult<Self> {
        let world = World::on_a_key("synthetic-provider-creator-fixture").await;
        let host_machine = world
            .add_machine("Provider creator physical host", &["node"])
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
        let (token, host, template, runtime) = (new_id(), new_id(), new_id(), new_id());
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Org),
                    id: Some(world.root),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![KeyTargetKind::Object(ObjectKind::Provider)]),
                allow: vec![KeyOperation {
                    operation: "ProviderCreate".into(),
                    actions: vec![KeyAction::Manage],
                }],
            }],
            ..KeyScope::default()
        };
        let scopes = if legacy {
            vec!["provider.manage".to_owned()]
        } else {
            vec![]
        };
        let subtree = legacy.then_some(world.root);
        let operation_scope = (!legacy).then(|| policy.clone());
        let stored_policy = operation_scope.as_ref().map(|scope| json!(scope));
        let mut tx = world.pool.begin().await?;
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Pending provider birth",
                token_hash: Hash::of(token.as_bytes()).as_bytes(),
                scopes: &scopes,
                node_id: subtree,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            stored_policy.as_ref(),
        )
        .await?;
        let target = targets::node(&mut tx, world.root).await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(
                &scopes,
                subtree,
                operation_scope,
                Some(Time::new(expires)),
            )?
            .for_token(token),
        )
        .admit_facets(&[KeyFacet {
            operation: "ProviderCreate".into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::Provider),
            target,
        }])?
        .into_caller();
        let origin = CapturedOperation::capture(&caller)?;
        let placement_home = if legacy {
            let outside = new_id();
            orch_db::queries::hierarchy::testing::insert_node(
                &mut tx,
                outside,
                "org",
                None,
                "provider-placement-realm",
                world.ada,
            )
            .await?;
            orch_db::queries::access::testing::insert_member(&mut tx, outside, world.ada, outside)
                .await?;
            orch_db::queries::access::testing::name_owner(
                &mut tx,
                outside,
                ("user", world.ada),
                world.clock.now(),
            )
            .await?;
            outside
        } else {
            world.root
        };
        sqlx::query("INSERT INTO hosts(id,machine_id,name,owner_id,org_id,home_id,drivers,capacity) VALUES($1,$2,'Creator provider host',$3,$4,$4,$5,$6)")
            .bind(host).bind(host_machine.id).bind(world.ada).bind(placement_home)
            .bind(json!({"test":{"spec_schemas":{"provider":{}},"sources":["image"]}}))
            .bind(json!({"memory_mb":4096})).execute(&mut *tx).await?;
        sqlx::query("INSERT INTO env_templates(id,name,owner_id,org_id,home_id,kind,driver,spec) VALUES($1,'Creator provider template',$2,$3,$3,'provider','test',$4)")
            .bind(template).bind(world.ada).bind(placement_home)
            .bind(json!({"image":"synthetic-provider-image"})).execute(&mut *tx).await?;
        for (kind, id) in [
            (ResourceKind::Host, host),
            (ResourceKind::Template, template),
        ] {
            ownership::start_resource(
                &mut tx,
                ObjectRef {
                    kind: kind.into(),
                    id,
                },
                world.ada,
                NewAccess::OnlyMe,
                Time::new(world.clock.now()),
            )
            .await?;
        }
        sqlx::query("UPDATE providers SET machine_id=NULL,key_machine_id=NULL,status='needs_setup',runtime_config=$2 WHERE id=$1")
            .bind(world.provider)
            .bind(json!({"runtime.template_id":template,"runtime.host_id":host}))
            .execute(&mut *tx).await?;
        let row = match super::super::queue(
            &mut tx,
            world.clock.now(),
            world.provider,
            Mode::Make,
            Some(world.ada),
        )
        .await?
        {
            jobs::provider_jobs::Queued::Made(row) => row,
            _ => return Err("fresh provider Make was not queued".into()),
        };
        retain(
            &mut tx,
            &origin,
            world.provider,
            world.root,
            row.id,
            Time::new(world.clock.now()),
        )
        .await?;
        // The retained birth comes first. Its exact ordinary scheduler candidate is then recorded;
        // no old or unrelated runtime is attached to establish the accepted creator origin.
        // The legacy case leaves that step to the actual schedule method under test.
        if !legacy {
            provider_rows::insert_runtime(
                &mut tx,
                &provider_rows::NewRuntime {
                    id: runtime,
                    host_id: host,
                    unit_name: "provider-creator-candidate",
                    provider_id: world.provider,
                    driver: "test",
                    template_id: template,
                    spec_hash: &[0; 32],
                    generation: 1,
                    volumes: &json!({}),
                },
                world.clock.now(),
            )
            .await?;
            assert!(
                orch_db::queries::resources::hosting::adopt_machine(
                    &mut tx,
                    world.machine_id,
                    runtime,
                    world.clock.now()
                )
                .await?
            );
            jobs::merge_plan(
                &mut tx,
                row.id,
                &json!({"runtime":runtime,"host":host,"fresh":true}),
                world.clock.now(),
            )
            .await?;
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
        Ok(Self {
            world,
            engine,
            job,
            token,
            origin,
            policy,
            runtime,
            host_machine,
            placement_home,
        })
    }

    async fn schedule(&mut self) -> TestResult {
        let provision = ProviderProvision::of(self.engine.clone(), &self.job.row().await?);
        provision
            .schedule(&self.job)
            .await
            .map_err(|error| format!("{error:?}"))?;
        let mut conn = self.world.pool.acquire().await?;
        self.runtime = provider_rows::live_runtime(&mut conn, self.world.provider)
            .await?
            .ok_or("scheduled provider candidate absent")?
            .id;
        Ok(())
    }

    fn image(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            new_id(),
            self.host_machine.id,
            SerialKey::Image("synthetic-provider-image".into()),
            &NodeImageEnsure {
                driver: "test".into(),
                spec: json!({"image":"synthetic-provider-image"}),
                job_id: Some(self.job.id()),
            },
        )?)
    }

    async fn reserve(&self, command: &Outgoing) -> TestResult {
        self.job.remember(&json!({"active_command":{
            "id":command.id,"machine":command.machine,"kind":command.kind.as_str(),"phase":"image"
        }})).await?;
        remember_command(&self.job, command)
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    }

    async fn allows(&self, command: &Outgoing) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        // Production first-send reads the retained command through JSONB, including key order.
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
async fn queued_provider_birth_intersects_its_snapshot_with_current_scope_and_revocation()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(KeyScope::default()))
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "the captured creator cannot override narrower current scope"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(s.policy))
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.allows(&command).await?,
        "queued work cannot replace the original key with the owner's session"
    );
    assert_eq!(
        s.job.row().await?.plan,
        before,
        "refusal does not rewrite retained origin or recipe"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn provider_journal_keeps_old_ids_and_refuses_changed_or_aborted_forward_frames() -> TestResult
{
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    for mismatch in ["body", "machine", "key", "kind"] {
        let mut changed = command.clone();
        match mismatch {
            "body" => changed.body["spec"]["image"] = json!("different-provider-image"),
            "machine" => changed.machine = new_id(),
            "key" => changed.key = SerialKey::Image("another-image".into()),
            "kind" => changed.kind = CommandKind::NodeRuntimeStart,
            _ => unreachable!(),
        }
        assert!(!s.allows(&changed).await?, "{mismatch}");
    }
    let mut rewritten = command.clone();
    rewritten.body["spec"]["image"] = json!("rewritten");
    assert!(remember_command(&s.job, &rewritten).await.is_err());
    assert!(
        s.allows(&command).await?,
        "failed same-ID rewrite preserves original evidence"
    );
    let mut tx = s.world.pool.begin().await?;
    sqlx::query("UPDATE jobs SET plan=plan-'provider_creator' WHERE id=$1")
        .bind(s.job.id())
        .execute(&mut *tx)
        .await?;
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
        .await?,
        "retained command evidence without its creator origin must not become ordinary work"
    );
    tx.rollback().await?;
    assert!(s.allows(&command).await?);
    let next = s.image()?;
    s.reserve(&next).await?;
    let mut tx = s.world.pool.begin().await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"cancel":true}),
        s.world.clock.now(),
    )
    .await?;
    assert!(
        !command_authorized(
            &mut tx,
            next.id,
            next.machine,
            next.key.wire().as_deref(),
            next.kind.as_str(),
            &next.body,
            Time::new(s.world.clock.now())
        )
        .await?,
        "an owned cancellation independently refuses the next queued frame"
    );
    tx.rollback().await?;
    assert!(
        s.allows(&next).await?,
        "rollback leaves the same unmodified accepted command permitted"
    );
    assert!(
        !s.allows(&command).await?,
        "an older queued ID is found and fails its active fence"
    );
    assert!(s.allows(&next).await?);
    s.job.remember(&json!({"active_command":null})).await?;
    assert!(
        !s.allows(&next).await?,
        "clearing the pointer does not launder the retained ID"
    );
    s.reserve(&next).await?;
    let why = Stop::Failed(Failure::new(
        "creator_authority",
        "The pending provider birth was stopped.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    assert!(
        !s.allows(&next).await?,
        "failed undo also stops old forward frames without requiring cancel"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn provider_birth_marker_cannot_be_moved_to_another_job_or_existing_runtime() -> TestResult {
    let s = Fixture::new().await?;
    let mut tx = s.world.pool.begin().await?;
    assert!(
        retain(
            &mut tx,
            &s.origin,
            new_id(),
            s.world.root,
            s.job.id(),
            Time::new(s.world.clock.now())
        )
        .await
        .is_err()
    );
    assert!(
        retain(
            &mut tx,
            &s.origin,
            s.world.provider,
            s.world.root,
            new_id(),
            Time::new(s.world.clock.now())
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    sqlx::query("UPDATE jobs SET state='failed',finished_at=$2 WHERE id=$1")
        .bind(s.job.id())
        .bind(s.world.clock.now())
        .execute(s.world.pool.pool())
        .await?;
    let mut tx = s.world.pool.begin().await?;
    let next = match super::super::queue(
        &mut tx,
        s.world.clock.now(),
        s.world.provider,
        Mode::Make,
        Some(s.world.ada),
    )
    .await?
    {
        jobs::provider_jobs::Queued::Made(row) => row,
        _ => return Err("replacement test job not queued".into()),
    };
    assert!(
        retain(
            &mut tx,
            &s.origin,
            s.world.provider,
            s.world.root,
            next.id,
            Time::new(s.world.clock.now())
        )
        .await
        .is_err(),
        "a new Make for an already existing unit is not this creator's first birth"
    );
    tx.rollback().await?;
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn revoked_provider_creator_refuses_new_work_while_owned_make_undo_removes_its_unit()
-> TestResult {
    let s = Fixture::new().await?;
    let forward = s.image()?;
    s.reserve(&forward).await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&forward).await?);
    assert!(s.job.command(s.image()?).await.is_err());
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    let why = Stop::Failed(Failure::new(
        "creator_authority",
        "The creator credential was revoked.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim absent")?;
    s.host_machine
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Ok(CommandResult::NodeRuntimeRemove(Empty {}))
        });
    let provision = ProviderProvision::of(s.engine.clone(), &s.job.row().await?);
    assert!(matches!(
        provision.undo_make(&s.job, &why).await,
        Undone::Everything
    ));
    let issued = s.world.commands.issued_commands("node.runtime.remove");
    assert_eq!(issued.len(), 1);
    let (machine, actual) = &issued[0];
    assert_eq!(
        actual.id,
        crate::ids::derived("node.runtime.remove", &[s.job.id().as_bytes(), b"undo"])
    );
    assert_eq!(actual.body["runtime_id"], json!(s.runtime));
    let mut tx = s.world.pool.begin().await?;
    assert!(
        command_authorized(
            &mut tx,
            actual.id,
            *machine,
            actual.key.as_deref(),
            &actual.kind,
            &actual.body,
            Time::new(s.world.clock.now())
        )
        .await?,
        "existing owned compensation has a disjoint command identity"
    );
    tx.rollback().await?;
    assert!(
        !s.allows(&forward).await?,
        "cleanup does not reauthorize the old forward operation"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn accepted_and_terminal_provider_commands_keep_their_result_after_creator_revocation()
-> TestResult {
    let s = Fixture::new().await?;
    s.host_machine
        .machine
        .handler
        .hold(CommandKind::NodeImageEnsure);
    s.host_machine
        .machine
        .handler
        .on(CommandKind::NodeImageEnsure, |_| {
            Ok(CommandResult::NodeImageEnsure(NodeImageEnsureResult {
                digest: Some("synthetic-retained-digest".into()),
                built: None,
                programs: std::collections::BTreeMap::new(),
            }))
        });
    let command = s.image()?;
    let replay = command.clone();
    let id = command.id;
    let job = s.job.clone();
    let running = tokio::spawn(async move { job.command(command).await });
    s.host_machine.machine.handler.started(id).await;
    assert!(
        s.allows(&replay).await?,
        "the runner retained its actual outgoing native identity"
    );
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(
        !s.allows(&replay).await?,
        "future first-send work would now be refused"
    );
    // Its real native journal already accepted this identity. Reconcile that receipt, then the
    // same terminal receipt on phase replay; revocation must not fabricate a different outcome.
    s.host_machine.machine.handler.release();
    let result = running.await?.map_err(|error| format!("{error:?}"))?;
    assert_eq!(result["digest"], json!("synthetic-retained-digest"));
    assert_eq!(
        s.job
            .command(replay)
            .await
            .map_err(|error| format!("{error:?}"))?,
        result
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    assert_eq!(s.host_machine.machine.handler.runs_of(id), 1);
    assert!(
        s.job.command(s.image()?).await.is_err(),
        "a new native identity still needs current creator authority"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 1);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn unchanged_provider_command_needs_its_current_first_runtime_and_physical_host_machine()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    sqlx::query("UPDATE runtimes SET generation=2 WHERE id=$1")
        .bind(s.runtime)
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "the same journal cannot authorize a later provider runtime generation"
    );
    sqlx::query("UPDATE runtimes SET generation=1 WHERE id=$1")
        .bind(s.runtime)
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("provider candidate absent")?;
    drop(conn);
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(runtime.host_id)
        .bind(s.world.machine_id)
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "a current host remap cannot send the original queued command to its old machine"
    );
    sqlx::query("UPDATE hosts SET machine_id=$2 WHERE id=$1")
        .bind(runtime.host_id)
        .bind(s.host_machine.id)
        .execute(s.world.pool.pool())
        .await?;
    assert!(s.allows(&command).await?);
    assert_eq!(
        s.job.row().await?.plan,
        before,
        "current-binding refusals preserve the accepted origin and immutable native journal"
    );
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn revoked_provider_creator_cannot_publish_or_pass_commit() -> TestResult {
    let s = Fixture::new().await?;
    let before = s.job.row().await?;
    assert!(before.plan.get("committed").is_none());
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    let provision = ProviderProvision::of(s.engine.clone(), &before);
    let result = provision.commit(&s.job).await;
    assert!(
        matches!(&result,Err(Stop::Failed(failure)) if failure.code=="access.denied"),
        "the original creator must be checked within publication before pass_commit: {result:?}"
    );
    let after = s.job.row().await?;
    assert_eq!(after.plan, before.plan);
    assert!(after.plan.get("committed").is_none());
    assert!(after.plan.get("provider_creator_committed").is_none());
    let mut conn = s.world.pool.acquire().await?;
    let provider = provider_rows::state(&mut conn, s.world.provider)
        .await?
        .ok_or("provider absent")?;
    assert_eq!(provider.machine_id, None);
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("provider candidate absent")?;
    assert_eq!(runtime.id, s.runtime);
    assert_eq!(runtime.state, "creating");
    assert_eq!(s.world.commands.issued_of("node.runtime.start"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn inline_creator_login_keeps_both_credential_boundaries_and_its_original_attempt()
-> TestResult {
    use crate::provider_setup::{LoginAuthority, Pending};
    let s = Fixture::new().await?;
    let second_token = new_id();
    let expires = s.world.clock.now() + chrono::Duration::hours(1);
    let now = Time::new(s.world.clock.now());
    let mut tx = s.world.pool.begin().await?;
    identity::create_api_token_scoped(
        &mut tx,
        &identity::NewApiToken {
            id: second_token,
            user_id: s.world.ada,
            name: "Second inline creator boundary",
            token_hash: Hash::of(second_token.as_bytes()).as_bytes(),
            scopes: &[],
            node_id: None,
            at: s.world.clock.now(),
            expires_at: Some(expires),
        },
        Some(&json!(s.policy)),
    )
    .await?;
    let original = s
        .origin
        .restore(&mut tx, now)
        .await?
        .ok_or("original creator credential absent")?;
    let second = Caller::token(
        s.world.ada,
        TokenGrant::from_stored_scope(&[], None, Some(s.policy.clone()), Some(Time::new(expires)))?
            .for_token(second_token),
    );
    let target = targets::node(&mut tx, s.world.root).await?;
    let admitted = original
        .intersect_restrictions(&second)?
        .admit_facets(&[KeyFacet {
            operation: "ProviderCreate".into(),
            action: KeyAction::Manage,
            resource_kind: KeyTargetKind::Object(ObjectKind::Provider),
            target,
        }])?
        .into_caller();
    // Exercise the pending authority carrier; ordinary subscription/method validation is
    // performed by the creator producer before this continuation is retained.
    let pending = Pending::new(
        "synthetic-inline-method",
        &std::collections::BTreeMap::new(),
        s.world.ada,
    )
    .with_authority(&admitted)?;
    assert_eq!(
        pending.authority.as_ref().map(CapturedOperation::operation),
        Some("ProviderCreate")
    );
    let bytes = pending.to_bytes()?;
    let authority = pending.authority_state()?;
    provider_rows::keep_pending(&mut tx, s.world.provider, &bytes, s.world.clock.now()).await?;
    provider_rows::keep_login_authority(&mut tx, s.world.provider, authority.as_ref()).await?;
    tx.commit().await?;
    let mut conn = s.world.pool.acquire().await?;
    let active = LoginAuthority::load(&mut conn, s.world.provider)
        .await?
        .ok_or("active inline creator authority absent")?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    assert!(
        identity::revoke_api_token(&mut conn, second_token, s.world.ada, s.world.clock.now())
            .await?
    );
    assert!(s.origin.restore(&mut conn, now).await?.is_some());
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "the still-live first token cannot replace its revoked second boundary"
    );
    sqlx::query("UPDATE api_tokens SET revoked_at=NULL WHERE id=$1")
        .bind(second_token)
        .execute(&mut *conn)
        .await?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(KeyScope::default()))
        .execute(&mut *conn)
        .await?;
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "the second live token cannot replace narrower current scope on the first boundary"
    );
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(s.policy))
        .execute(&mut *conn)
        .await?;
    assert!(active.authorized(&mut conn, s.world.provider, now).await?);
    drop(conn);
    let replacement = Pending::new(
        "synthetic-inline-method",
        &std::collections::BTreeMap::new(),
        s.world.ada,
    )
    .with_authority(&admitted)?;
    assert_ne!(pending.attempt, replacement.attempt);
    assert_eq!(
        replacement
            .authority
            .as_ref()
            .map(CapturedOperation::operation),
        Some("ProviderCreate")
    );
    let bytes = replacement.to_bytes()?;
    let authority = replacement.authority_state()?;
    let mut tx = s.world.pool.begin().await?;
    provider_rows::keep_pending(&mut tx, s.world.provider, &bytes, s.world.clock.now()).await?;
    provider_rows::keep_login_authority(&mut tx, s.world.provider, authority.as_ref()).await?;
    tx.commit().await?;
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        !active.authorized(&mut conn, s.world.provider, now).await?,
        "an old attempt cannot borrow the replacement's otherwise valid creator origin"
    );
    let current = LoginAuthority::load(&mut conn, s.world.provider)
        .await?
        .ok_or("replacement inline creator authority absent")?;
    assert!(current.authorized(&mut conn, s.world.provider, now).await?);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn legacy_provider_manage_keeps_its_home_boundary_while_scheduling_account_placement()
-> TestResult {
    let mut s = Fixture::legacy_unplaced().await?;
    let mut conn = s.world.pool.acquire().await?;
    let restored = s
        .origin
        .restore(&mut conn, Time::new(s.world.clock.now()))
        .await?
        .ok_or("legacy creator credential absent")?;
    assert!(restored.permits(Permission::ProviderManage));
    assert!(!restored.permits(Permission::HostUse));
    assert!(!restored.permits(Permission::TemplateUse));
    assert!(restored.reaches(Some(&[s.world.root])));
    assert!(!restored.reaches(Some(&[s.placement_home])));
    assert!(
        provider_rows::live_runtime(&mut conn, s.world.provider)
            .await?
            .is_none()
    );
    drop(conn);
    // Run the real initial scheduler: internal host/template use is the account's placement
    // decision, while the original provider.manage credential remains scoped to this home.
    s.schedule().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let before = s.job.row().await?.plan;
    let mut tx = s.world.pool.begin().await?;
    let owner = jobs::job(&mut tx, s.job.id())
        .await?
        .ok_or("legacy provider job absent")?;
    let current = principal(&mut tx, &owner, Time::new(s.world.clock.now()))
        .await?
        .ok_or("legacy provider principal absent")?;
    assert!(
        current.caller().grant().is_some(),
        "placement must not reconstruct a browser session"
    );
    assert!(!current.caller().permits(Permission::HostUse));
    assert!(!current.caller().permits(Permission::TemplateUse));
    tx.rollback().await?;
    for boundary in ["permission", "subtree", "exclusion"] {
        let mut tx = s.world.pool.begin().await?;
        match boundary {
            "permission" => {
                sqlx::query("UPDATE api_tokens SET scopes=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(vec!["provider.view".to_owned()])
                    .execute(&mut *tx)
                    .await?;
            }
            "subtree" => {
                sqlx::query("UPDATE api_tokens SET node_id=$2 WHERE id=$1")
                    .bind(s.token)
                    .bind(s.placement_home)
                    .execute(&mut *tx)
                    .await?;
            }
            "exclusion" => {
                assert!(
                    orch_db::queries::tokens::refuse(
                        &mut tx,
                        s.token,
                        s.world.root,
                        s.world.ada,
                        s.world.clock.now()
                    )
                    .await?
                );
            }
            _ => unreachable!(),
        }
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
            .await?,
            "{boundary}"
        );
        tx.rollback().await?;
        assert!(
            s.allows(&command).await?,
            "restored {boundary} keeps the original unchanged command usable"
        );
    }
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&command).await?);
    assert_eq!(s.job.row().await?.plan, before);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn legacy_provider_placement_still_requires_current_account_provider_host_and_template_rights()
-> TestResult {
    let mut s = Fixture::legacy_unplaced().await?;
    s.schedule().await?;
    let command = s.image()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let mut conn = s.world.pool.acquire().await?;
    let runtime = provider_rows::live_runtime(&mut conn, s.world.provider)
        .await?
        .ok_or("legacy provider candidate absent")?;
    drop(conn);
    let before = s.job.row().await?.plan;
    for (kind, id, permission) in [
        (
            ResourceKind::Provider,
            s.world.provider,
            Permission::ProviderManage,
        ),
        (ResourceKind::Host, runtime.host_id, Permission::HostUse),
        (
            ResourceKind::Template,
            runtime.template_id,
            Permission::TemplateUse,
        ),
    ] {
        let mut tx = s.world.pool.begin().await?;
        let page = orch_db::queries::access::PageKey::new(kind.as_str(), id, "live");
        let entries = orch_db::queries::access::entries_of(&mut tx, &page).await?;
        assert!(
            !entries.is_empty(),
            "the fixture grants ordinary account rights on {}",
            kind.as_str()
        );
        for entry in entries {
            assert!(
                orch_db::queries::access::remove_entry(
                    &mut tx,
                    entry.id,
                    s.world.ada,
                    s.world.clock.now()
                )
                .await?
            );
        }
        orch_db::queries::access::testing::set_row(
            &mut tx,
            &page,
            "any",
            permission.as_str(),
            false,
            false,
        )
        .await?;
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
            .await?,
            "the legacy provider.manage boundary must not replace withdrawn account {}",
            permission.as_str()
        );
        tx.rollback().await?;
        assert!(s.allows(&command).await?);
    }
    assert_eq!(s.job.row().await?.plan, before);
    assert_eq!(s.world.commands.issued_of("node.image.ensure"), 0);
    s.engine.stop();
    Ok(())
}
