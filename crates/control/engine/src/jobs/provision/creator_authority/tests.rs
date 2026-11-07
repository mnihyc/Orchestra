//! Current creator authority and canonical command identity across delayed provisioning.
use super::*;
use crate::{EngineParts, test_common::World};
use orch_access_contract::key_scope::{
    KeyFacet, KeyOperation, KeyReach, KeyRule, KeyScope, KeyTarget,
};
use orch_authz::TokenGrant;
use orch_db::queries::identity;
use orch_link_contract::commands::{
    CommandKind, CommandResult, FsSnapshot, FsSnapshotResult, FsStat, Selection,
};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: World,
    engine: Engine,
    job: Job,
    token: Id,
    origin: CapturedOperation,
    thread: Id,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let world = World::observed().await;
        let thread = world.thread("Creator-owned new environment").await;
        world.engine().stop();
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: crate::test_common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let token = new_id();
        let expires = world.clock.now() + chrono::Duration::hours(1);
        let policy = KeyScope {
            rules: vec![KeyRule {
                target: KeyTarget {
                    kind: KeyTargetKind::Object(ObjectKind::Topic),
                    id: Some(world.topic),
                    reach: KeyReach::SelfOnly,
                },
                resources: Some(vec![
                    KeyTargetKind::Object(ObjectKind::Thread),
                    KeyTargetKind::Object(ObjectKind::Environment),
                ]),
                allow: vec![KeyOperation {
                    operation: "ThreadCreate".into(),
                    actions: vec![KeyAction::Write],
                }],
            }],
            ..KeyScope::default()
        };
        let mut tx = world.pool.begin().await?;
        identity::create_api_token_scoped(
            &mut tx,
            &identity::NewApiToken {
                id: token,
                user_id: world.ada,
                name: "Creator provision authority",
                token_hash: Hash::of(token.as_bytes()).as_bytes(),
                scopes: &[],
                node_id: None,
                at: world.clock.now(),
                expires_at: Some(expires),
            },
            Some(&json!(policy)),
        )
        .await?;
        let target = targets::node(&mut tx, world.topic).await?;
        let caller = Caller::token(
            world.ada,
            TokenGrant::from_stored_scope(&[], None, Some(policy), Some(Time::new(expires)))?
                .for_token(token),
        )
        .admit_facets(&[
            KeyFacet {
                operation: "ThreadCreate".into(),
                action: KeyAction::Write,
                resource_kind: KeyTargetKind::Object(ObjectKind::Thread),
                target: target.clone(),
            },
            KeyFacet {
                operation: "ThreadCreate".into(),
                action: KeyAction::Write,
                resource_kind: KeyTargetKind::Object(ObjectKind::Environment),
                target,
            },
        ])?
        .into_caller();
        let origin = CapturedOperation::capture(&caller).map_err(|_| "origin missing")?;
        sqlx::query("UPDATE environments SET generation=0,status='provisioning',machine_id=NULL WHERE id=$1")
            .bind(world.environment).execute(&mut *tx).await?;
        let row = jobs::insert(
            &mut tx,
            &jobs::NewJob {
                id: new_id(),
                kind: kind::ENVIRONMENT_PROVISION,
                node_id: None,
                environment_id: Some(world.environment),
                provider_id: None,
                plan: &json!({"mode":"make","generation":0}),
                progress: &json!(crate::jobs::runner::initial_progress(&phases(
                    Mode::Make,
                    false
                ))),
                created_by: Some(world.ada),
            },
            world.clock.now(),
        )
        .await?;
        retain(
            &mut tx,
            &origin,
            thread,
            world.environment,
            world.topic,
            row.id,
            true,
            Time::new(world.clock.now()),
        )
        .await?;
        let runner = new_id();
        assert!(
            jobs::claim(
                &mut tx,
                row.id,
                runner,
                world.clock.now() + chrono::Duration::hours(1),
                world.clock.now(),
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
            thread,
        })
    }

    async fn reserve(&self, command: &Outgoing) -> TestResult {
        self.job
            .remember(&json!({"active_command":{
                "id":command.id,"machine":command.machine,"kind":command.kind.as_str()
            }}))
            .await?;
        remember_command(&self.job, command)
            .await
            .map_err(|error| format!("{error:?}"))?;
        Ok(())
    }

    async fn allows(&self, command: &Outgoing) -> TestResult<bool> {
        let mut tx = self.world.pool.begin().await?;
        orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
        // Production reloads the outgoing command from JSONB before its wire guard.
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
            self.world.clock.as_ref(),
            self.world.access.as_ref(),
        )
        .await?;
        tx.rollback().await?;
        Ok(allowed)
    }

    fn stat(&self) -> TestResult<Outgoing> {
        Ok(Outgoing::new(
            new_id(),
            self.world.machine_id,
            SerialKey::Environment(self.world.environment),
            &FsStat {
                environment_id: self.world.environment,
                mount: None,
                path: "web".into(),
            },
        )?)
    }
}

#[tokio::test]
async fn the_original_flat_creator_record_stays_readable_but_known_missing_sources_never_become_native()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.stat()?;
    s.reserve(&command).await?;
    sqlx::query("UPDATE jobs SET plan=plan-'creator_provision_version' WHERE id=$1")
        .bind(s.job.id())
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        s.allows(&command).await?,
        "the existing flat ThreadCreate payload needs no new version field"
    );
    sqlx::query("UPDATE jobs SET plan=plan-'creator_provision' WHERE id=$1")
        .bind(s.job.id())
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "its retained native command journal proves this is known creator work"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn actual_outgoing_work_report_is_retained_before_issue_and_old_results_still_complete()
-> TestResult {
    let s = Fixture::new().await?;
    s.world.machine.handler.hold(CommandKind::FsSnapshot);
    s.world.machine.handler.on(CommandKind::FsSnapshot, |_| {
        Ok(CommandResult::FsSnapshot(FsSnapshotResult {
            manifest_blob: Hash::of(b"test manifest"),
            file_count: 0,
            total_bytes: 0,
            uploaded_bytes: 0,
            missing: 0,
        }))
    });
    let command = Outgoing::new(
        new_id(),
        s.world.machine_id,
        SerialKey::Environment(s.world.environment),
        &FsSnapshot {
            work: None,
            environment_id: s.world.environment,
            job_id: s.job.id(),
            selection: Selection {
                include: vec!["web".into()],
                exclude: vec![],
            },
        },
    )?;
    let id = command.id;
    let job = s.job.clone();
    let running = tokio::spawn(async move { job.command(command).await });
    s.world.machine.handler.started(id).await;
    let issued = s.world.commands.issued_commands("fs.snapshot");
    let (_, actual) = issued
        .iter()
        .find(|(_, command)| command.id == id)
        .ok_or("not issued")?;
    assert_eq!(actual.body["work"]["job_id"], json!(s.job.id()));
    assert!(
        actual.body["work"]["timeout_secs"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );
    let canonical = Outgoing {
        id,
        machine: s.world.machine_id,
        kind: CommandKind::FsSnapshot,
        key: SerialKey::Environment(s.world.environment),
        body: actual.body.clone(),
        deadline: Some(actual.deadline),
        authority: None,
    };
    assert!(s.allows(&canonical).await?);
    let mut conn = s.world.pool.acquire().await?;
    assert!(
        identity::revoke_api_token(&mut conn, s.token, s.world.ada, s.world.clock.now()).await?
    );
    drop(conn);
    assert!(!s.allows(&canonical).await?);
    // The native journal already accepted this command: reconciliation must retain its result.
    s.world.machine.handler.release();
    running.await?.map_err(|error| format!("{error:?}"))?;
    assert!(
        !s.allows(&canonical).await?,
        "cleared active pointer cannot orphan retained creator evidence"
    );
    assert_eq!(s.world.commands.issued_of("fs.snapshot"), 1);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn canonical_journal_denies_changed_recipes_and_late_cancelled_or_replaced_commands()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.stat()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let mut changed = command.clone();
    changed.body["path"] = json!("other");
    assert!(!s.allows(&changed).await?);
    assert!(remember_command(&s.job, &changed).await.is_err());
    assert!(
        s.allows(&command).await?,
        "failed rewrite preserves the original recipe"
    );
    let next = s.stat()?;
    s.reserve(&next).await?;
    assert!(
        !s.allows(&command).await?,
        "retained old ID is found and fails its exact active fence"
    );
    assert!(s.allows(&next).await?);
    s.job
        .remember(&json!({"cancel":true,"active_command":null}))
        .await?;
    assert!(
        !s.allows(&next).await?,
        "cancelled late queued frame does not become ordinary"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn current_scope_and_exact_new_prefix_are_required_without_attaching_existing_jobs()
-> TestResult {
    let s = Fixture::new().await?;
    let command = s.stat()?;
    s.reserve(&command).await?;
    assert!(s.allows(&command).await?);
    let mut tx = s.world.pool.begin().await?;
    assert!(
        retain(
            &mut tx,
            &s.origin,
            s.thread,
            s.world.environment,
            s.world.topic,
            s.job.id(),
            false,
            Time::new(s.world.clock.now())
        )
        .await
        .is_err()
    );
    tx.rollback().await?;
    sqlx::query("UPDATE api_tokens SET operation_scope=$2 WHERE id=$1")
        .bind(s.token)
        .bind(json!(KeyScope::default()))
        .execute(s.world.pool.pool())
        .await?;
    assert!(
        !s.allows(&command).await?,
        "accepted snapshot cannot override narrower current key scope"
    );
    assert_eq!(s.world.commands.issued_of("fs.stat"), 0);
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn revoked_creator_refuses_new_forward_work_but_owned_make_compensation_still_removes_its_unit()
-> TestResult {
    use orch_link_contract::commands::Empty;
    let s = Fixture::new().await?;
    // The primary World connector has Agent/Environment/Provider roles only. Actual unit
    // cleanup is a Node command, so use the existing explicit Node fixture and its transport.
    let cleanup_node = s.world.add_machine("owned-cleanup-node", &["node"]).await;
    let forward = s.stat()?;
    s.reserve(&forward).await?;
    let mut tx = s.world.pool.begin().await?;
    let (host, runtime) = (new_id(), new_id());
    sqlx::query("INSERT INTO hosts(id,machine_id,name) VALUES($1,$2,'Owned cleanup host')")
        .bind(host)
        .bind(cleanup_node.id)
        .execute(&mut *tx)
        .await?;
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
        .bind(s.world.environment)
        .fetch_one(&mut *tx)
        .await?;
    env_rows::insert_runtime(
        &mut tx,
        &env_rows::NewRuntime {
            id: runtime,
            host_id: host,
            unit_name: "owned-cleanup-candidate",
            environment_id: s.world.environment,
            driver: "test",
            template_id: template,
            spec_hash: &[0; 32],
            generation: 1,
            volumes: &json!({}),
        },
        s.world.clock.now(),
    )
    .await?;
    jobs::merge_plan(
        &mut tx,
        s.job.id(),
        &json!({"host":host,"runtime":runtime,"fresh":true}),
        s.world.clock.now(),
    )
    .await?;
    assert!(identity::revoke_api_token(&mut tx, s.token, s.world.ada, s.world.clock.now()).await?);
    tx.commit().await?;
    assert!(!s.allows(&forward).await?);
    assert!(s.job.command(s.stat()?).await.is_err());
    assert_eq!(s.world.commands.issued_of("fs.stat"), 0);
    let why = Stop::Failed(Failure::new(
        "creator_authority",
        "The creator credential was revoked.",
    ));
    s.job.begin_undo(&why).await?.ok_or("undo claim missing")?;
    cleanup_node
        .machine
        .handler
        .on(CommandKind::NodeRuntimeRemove, |_| {
            Ok(CommandResult::NodeRuntimeRemove(Empty {}))
        });
    let row = s.job.row().await?;
    let provision = Provision::of(s.engine.clone(), &row);
    let undone = provision.undo_make(&s.job, &why).await;
    assert!(
        matches!(undone, Undone::Everything),
        "owned Make compensation must finish: {undone:?}; runtime removals={}, volume removals={}, refused forward commands={}",
        s.world.commands.issued_of("node.runtime.remove"),
        s.world.commands.issued_of("node.volume.remove"),
        s.world.commands.issued_of("fs.stat")
    );
    let issued = s.world.commands.issued_commands("node.runtime.remove");
    assert_eq!(issued.len(), 1);
    let (machine, actual) = &issued[0];
    assert_eq!(
        *machine, cleanup_node.id,
        "compensation targets the actual Node connector"
    );
    assert_eq!(
        actual.id,
        ids::derived("node.runtime.remove", &[s.job.id().as_bytes(), b"undo"])
    );
    assert_eq!(actual.body["runtime_id"], json!(runtime));
    let mut tx = s.world.pool.begin().await?;
    assert!(
        command_authorized(
            &mut tx,
            actual.id,
            *machine,
            actual.key.as_deref(),
            &actual.kind,
            &actual.body,
            s.world.clock.as_ref(),
            s.world.access.as_ref(),
        )
        .await?,
        "the exact existing compensation command is outside forward creator authority"
    );
    tx.rollback().await?;
    assert!(
        !s.allows(&forward).await?,
        "old queued forward work stays refused throughout cleanup"
    );
    s.engine.stop();
    Ok(())
}

#[tokio::test]
async fn a_real_thread_creator_cannot_be_relabelled_as_standalone_or_gain_that_version()
-> TestResult {
    let fixture = Fixture::new().await?;
    let row = fixture
        .job
        .row()
        .await
        .map_err(|error| format!("{error:?}"))?;
    assert!(read(&row)?.is_some());
    for marker in [Json::Null, json!(false), json!(true)] {
        let mut changed = row.clone();
        changed.plan[FIELD]["standalone"] = marker;
        assert!(
            read(&changed).is_err(),
            "the real Thread parent and standalone parent cannot coexist"
        );
    }
    let mut changed = row.clone();
    changed.plan[VERSION] = json!(3);
    assert!(
        read(&changed).is_err(),
        "the standalone version cannot authorize a Thread parent"
    );
    let original = read(&row)?.ok_or("the original Thread parent disappeared")?;
    assert!(
        serde_json::to_value(original)?.get("standalone").is_none(),
        "old Thread wire data keep the field absent"
    );
    Ok(())
}
