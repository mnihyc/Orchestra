//! Real claim takeovers while source undo is held before its metadata publication.
//! These tests reconstruct a failed-phase checkpoint; they do not inject a process crash.

use super::*;
use crate::EngineParts;
use crate::test_common as common;
use orch_api_contract::source::{WorktreeChoice, WorktreeFrom};
use orch_db::queries::jobs::state;
use orch_link_contract::commands::{CommandKind, CommandResult};
use orch_link_contract::source::{GitPrepareCleanupResult, GitWorktreeCleanupResult};
use orch_types::{Clock, ManualClock};
use std::sync::Arc;

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

struct Fixture {
    world: common::World,
    clock: Arc<ManualClock>,
    engine: Engine,
    host: Id,
    thread: Id,
    input: threads::InputRow,
}

impl Fixture {
    async fn new() -> TestResult<Self> {
        let clock = Arc::new(orch_testkit::clock::manual());
        let world = common::World::with_clock(clock.clone()).await;
        let thread = world.thread("Source attempt ownership").await;
        let original = world.engine();
        original.stop();
        common::eventually("the fixture thread's previous dispatch drains", || {
            let drained = !original
                .inner
                .flights
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains_key(&thread);
            async move { drained }
        })
        .await;
        // No supervisor runs these manually claimed jobs. The real link still persists and
        // replies to cleanup, so its hold is the exact gap before source failure publication.
        let engine = Engine::new(EngineParts {
            db: world.pool.clone(),
            clock: world.clock.clone(),
            config: common::config(),
            commands: world.commands.as_engine(),
            access: world.access.clone(),
            live: world.live.clone(),
        });
        let mut tx = world.pool.begin().await?;
        let host = new_id();
        sqlx::query(
            "INSERT INTO hosts (id,machine_id,name) VALUES ($1,$2,'Synthetic source host')",
        )
        .bind(host)
        .bind(world.machine_id)
        .execute(&mut *tx)
        .await?;
        // A unit's policy comes from its physical host, even when the fixture skips provisioning.
        sqlx::query("UPDATE environments SET remote_write=true, config=config || jsonb_build_object('env.host_id',$2::text) WHERE id=$1")
            .bind(world.environment).bind(host).execute(&mut *tx).await?;
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,environment.remote_write}','true') WHERE id=$1")
            .bind(world.machine_id).execute(&mut *tx).await?;
        threads::lock(&mut tx, thread)
            .await?
            .ok_or("thread absent")?;
        let mut unit = Unit::new(&mut tx, clock.now());
        let seq = unit.stamp(thread).await?;
        let input = threads::insert_input(
            &mut *unit.conn,
            &threads::NewInput {
                plan: false,
                id: new_id(),
                thread_id: thread,
                author_id: Some(world.ada),
                submission_authority: Some(
                    &serde_json::json!({"credentials":[{"type":"session","as_admin":false}]}),
                ),
                run_as: None,
                run_as_authority: None,
                source: "user",
                content: &json!([{ "type": "text", "text": "Keep the original pending input" }]),
                delivery: "queue",
                state: "pending",
                turn_id: None,
                client_id: None,
                limit_bypass: None,
                place: threads::Place::End,
                at: clock.now(),
            },
            seq,
        )
        .await?
        .ok_or("input absent")?;
        unit.finish().await?;
        tx.commit().await?;
        Ok(Self {
            world,
            clock,
            engine,
            host,
            thread,
            input,
        })
    }

    async fn plan(&self, worktree: bool) -> TestResult<Plan> {
        let mut conn = self.world.pool.acquire().await?;
        let env = environments::workspace::environment(&mut conn, self.world.environment)
            .await?
            .ok_or("environment absent")?;
        let row = threads::thread(&mut conn, self.thread)
            .await?
            .ok_or("thread absent")?;
        let frozen = RepositorySource {
            repository_url: Some("https://example.test/team/repo.git".into()),
            branch: Some("main".into()),
            secret_name: None,
        };
        let action = if worktree {
            Action::Worktree {
                body: GitWorktree {
                    environment_id: self.world.environment,
                    job_id: Id::nil(),
                    snapshot_id: None,
                    choice: WorktreeChoice {
                        repository_dir: "repo".into(),
                        branch: "work/attempt".into(),
                        from: WorktreeFrom::Commit {
                            commit: "synthetic-base".into(),
                        },
                        directory: row.workspace_dir,
                    },
                },
                source: frozen,
                record_days: 7,
                max_bytes: 1024,
            }
        } else {
            Action::Prepare {
                directory: row.workspace_dir,
                source: frozen,
                credential: None,
                destination: SourceDestination::MissingDirectory,
                effect_id: None,
            }
        };
        Ok(Plan {
            environment: env.id,
            generation: env.generation,
            machine: env.machine_id.ok_or("machine absent")?,
            thread: Some(self.thread),
            caller: self.world.ada,
            token: None,
            action,
        })
    }

    async fn begin(&self, plan: Plan) -> TestResult<(Job, Preparing, Id)> {
        let mut tx = self.world.pool.begin().await?;
        let row = begin(&mut tx, plan, self.clock.now()).await?;
        let runner = new_id();
        // A short, controlled lease makes expiry deterministic without expiring a native command.
        assert!(
            rows::claim(
                &mut tx,
                row.id,
                runner,
                self.clock.now() + chrono::Duration::seconds(1),
                self.clock.now()
            )
            .await?
        );
        let row = rows::job(&mut tx, row.id)
            .await?
            .ok_or("claimed job absent")?;
        tx.commit().await?;
        Ok((
            Job::claimed(self.engine.clone(), row.id, runner),
            Preparing::of(self.engine.clone(), &row)?,
            runner,
        ))
    }

    async fn take_over(&self, id: Id) -> TestResult<Job> {
        let mut tx = self.world.pool.begin().await?;
        let before = rows::lock(&mut tx, id).await?.ok_or("job absent")?;
        let until: chrono::DateTime<chrono::Utc> =
            serde_json::from_value(before.plan["runner"]["until"].clone())?;
        // Advance only the domain clock while holding the row. There is no timer, native call
        // or second connection inside this transaction, and no sweep can claim between these steps.
        let runner = new_id();
        assert!(
            !rows::claim(
                &mut tx,
                id,
                runner,
                until + chrono::Duration::minutes(1),
                self.clock.now()
            )
            .await?,
            "the second runner cannot take an unexpired claim"
        );
        self.clock.set(until + chrono::Duration::seconds(1));
        assert!(
            rows::claim(
                &mut tx,
                id,
                runner,
                self.clock.now() + chrono::Duration::minutes(1),
                self.clock.now()
            )
            .await?
        );
        let after = rows::job(&mut tx, id)
            .await?
            .ok_or("taken-over job absent")?;
        assert_ne!(before.plan["runner"]["id"], after.plan["runner"]["id"]);
        assert_eq!(after.state, state::RUNNING);
        tx.commit().await?;
        Ok(Job::claimed(self.engine.clone(), id, runner))
    }

    async fn source(&self) -> TestResult<Json> {
        let mut conn = self.world.pool.acquire().await?;
        Ok(threads::thread(&mut conn, self.thread)
            .await?
            .ok_or("thread absent")?
            .source
            .ok_or("source absent")?)
    }

    async fn assert_no_work(&self, jobs: i64) -> TestResult {
        for kind in [
            CommandKind::GitPrepare,
            CommandKind::GitWorktree,
            CommandKind::FsRecord,
            CommandKind::SessionOpen,
            CommandKind::LeaseOpen,
            CommandKind::TurnStart,
        ] {
            assert_eq!(
                self.world.began(kind),
                0,
                "stale undo must not start {kind}"
            );
        }
        let mut conn = self.world.pool.acquire().await?;
        assert_eq!(
            threads::input(&mut conn, self.input.id).await?.as_ref(),
            Some(&self.input)
        );
        assert_eq!(
            threads::pending_inputs(&mut conn, self.thread).await?,
            vec![self.input.clone()]
        );
        let work: (i64, i64, i64) = sqlx::query_as("SELECT (SELECT count(*) FROM jobs WHERE environment_id=$1), (SELECT count(*) FROM turns WHERE thread_id=$2), (SELECT count(*) FROM leases WHERE thread_id=$2)")
            .bind(self.world.environment).bind(self.thread).fetch_one(&mut *conn).await?;
        assert_eq!(work, (jobs, 0, 0));
        Ok(())
    }

    fn hold_cleanup(&self, worktree: bool, removed: bool) -> CommandKind {
        let kind = if worktree {
            CommandKind::GitWorktreeCleanup
        } else {
            CommandKind::GitPrepareCleanup
        };
        self.world
            .machine
            .handler
            .on(CommandKind::GitPrepareCleanup, move |_| {
                Ok(CommandResult::GitPrepareCleanup(GitPrepareCleanupResult {
                    removed,
                    reason: (!removed).then(|| "the synthetic checkout is kept".into()),
                }))
            });
        self.world
            .machine
            .handler
            .on(CommandKind::GitWorktreeCleanup, move |_| {
                Ok(CommandResult::GitWorktreeCleanup(
                    GitWorktreeCleanupResult {
                        removed,
                        reason: (!removed).then(|| "the synthetic worktree is kept".into()),
                    },
                ))
            });
        self.world.machine.handler.hold(kind);
        kind
    }
}

fn failure() -> Stop {
    Stop::Failed(Failure::new(
        "workspace.source_failed",
        "the reconstructed native phase failed",
    ))
}

fn ready(preparing: &Preparing) -> SourceInfo {
    let (kind, directory, source) = match &preparing.plan.action {
        Action::Prepare {
            directory, source, ..
        } => (SourceKind::Repository, directory, source),
        Action::Worktree { body, source, .. } => {
            (SourceKind::Worktree, &body.choice.directory, source)
        }
        _ => unreachable!("the fixture only creates source attempts"),
    };
    SourceInfo {
        kind,
        directory: directory.clone(),
        repository_url: source.repository_url.clone(),
        branch: Some("main".into()),
        head: Some("synthetic-current-head".into()),
        state: SourceState::Ready,
        job: None,
        reason: None,
    }
}

#[tokio::test]
async fn a_stale_undo_cannot_fail_or_amend_the_runner_that_actually_took_over() -> TestResult {
    let fixture = Fixture::new().await?;
    let (old, preparing, runner) = fixture.begin(fixture.plan(false).await?).await?;
    let old_preparing = preparing.clone();
    old.remember(&json!({ "file_result": ready(&preparing) }))
        .await?;
    let cleanup = fixture.hold_cleanup(false, false);
    let engine = fixture.engine.clone();
    let ending = old.clone();
    // This invokes the real terminal path at a reconstructed failed-phase checkpoint.
    let stale =
        tokio::spawn(async move { engine.end_job(&ending, Some(&preparing), failure()).await });
    fixture.world.wait_began(cleanup, 1).await;
    let current = fixture.take_over(old.id()).await?;
    let expected_job = current.row().await?;
    let expected_source = fixture.source().await?;
    fixture.world.machine.handler.release();
    tokio::time::timeout(common::PATIENCE, stale).await???;
    assert_eq!(
        current.row().await?,
        expected_job,
        "the old cleanup's remember/progress/failure writes are fenced"
    );
    assert_eq!(fixture.source().await?, expected_source);
    // Fresh handles with the old token have no cached lost flag: the stored claim is decisive.
    let stale_handle = || Job::claimed(fixture.engine.clone(), old.id(), runner);
    stale_handle()
        .remember(&json!({ "stale_write": true }))
        .await?;
    stale_handle()
        .amend(|progress, _| progress.message = Some("stale progress".into()))
        .await?;
    assert_eq!(stale_handle().pass_commit().await, Err(Stop::Lost));
    assert_eq!(
        old_preparing.publish(&stale_handle()).await,
        Err(Stop::Lost)
    );
    fixture
        .engine
        .finish_job(&stale_handle(), Some(&json!({ "stale_success": true })))
        .await?;
    fixture
        .engine
        .end_job(&stale_handle(), None, failure())
        .await?;
    assert_eq!(current.row().await?, expected_job);
    assert_eq!(fixture.world.began(cleanup), 1);
    fixture.assert_no_work(1).await
}

async fn same_choice_retry_survives(ready_result: bool) -> TestResult {
    for worktree in [false, true] {
        let fixture = Fixture::new().await?;
        let (old, preparing, _) = fixture.begin(fixture.plan(worktree).await?).await?;
        let recipe = preparing.plan.clone();
        let old_id = old.id();
        // A removed reply would clear the old retry's effect/capture. That reply must never
        // reset the newer job's recipe, whether the newer result is Preparing or Ready.
        let cleanup = fixture.hold_cleanup(worktree, true);
        let engine = fixture.engine.clone();
        let stale =
            tokio::spawn(async move { engine.end_job(&old, Some(&preparing), failure()).await });
        fixture.world.wait_began(cleanup, 1).await;
        let recovered = fixture.take_over(old_id).await?;
        // The recovery owner closes the old job; only then does begin admit the explicit retry.
        fixture.engine.end_job(&recovered, None, failure()).await?;
        let (current, preparing, _) = fixture.begin(recipe.clone()).await?;
        assert_ne!(current.id(), old_id);
        assert_eq!(
            json!(preparing.plan),
            json!(recipe),
            "the new job deliberately reuses the entire same-choice native recipe/effect"
        );
        let answer = json!(ready(&preparing));
        // Reconstruct the durable native-answer checkpoint without issuing new native work.
        current
            .remember(&json!({ "file_result": answer, "file_source_attempt": preparing.plan }))
            .await?;
        if ready_result {
            assert_eq!(preparing.publish(&current).await, Ok(Done::Finished));
            fixture.engine.finish_job(&current, Some(&answer)).await?;
        }
        let expected_source = fixture.source().await?;
        let expected_job = current.row().await?;
        let expected_old = recovered.row().await?;
        assert_eq!(
            expected_source["info"]["state"],
            if ready_result { "ready" } else { "preparing" }
        );
        assert_eq!(expected_source["info"]["job"]["id"], json!(current.id()));
        assert_eq!(expected_source["native_plan"], json!(recipe));
        assert_eq!(expected_job.plan["file_result"], answer);
        fixture.world.machine.handler.release();
        tokio::time::timeout(common::PATIENCE, stale).await???;
        assert_eq!(
            fixture.source().await?,
            expected_source,
            "including the exact presence/absence of native_result"
        );
        assert_eq!(
            current.row().await?,
            expected_job,
            "the retry's runner, state, native answer and result survive exactly"
        );
        assert_eq!(
            recovered.row().await?,
            expected_old,
            "the old terminal row also stays immutable"
        );
        assert_eq!(fixture.world.began(cleanup), 1);
        fixture.assert_no_work(2).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_same_choice_retry_stays_preparing_after_the_old_undo_is_released() -> TestResult {
    same_choice_retry_survives(false).await
}

#[tokio::test]
async fn a_same_choice_ready_result_survives_the_old_undo_with_its_exact_native_answer()
-> TestResult {
    same_choice_retry_survives(true).await
}

#[tokio::test]
async fn a_takeover_finishes_the_original_abort_instead_of_publishing_retained_success()
-> TestResult {
    let fixture = Fixture::new().await?;
    let (old, preparing, _) = fixture.begin(fixture.plan(false).await?).await?;
    let recovered_preparing = preparing.clone();
    let id = old.id();
    let cleanup = fixture.hold_cleanup(false, false);
    let engine = fixture.engine.clone();
    let stale =
        tokio::spawn(async move { engine.end_job(&old, Some(&preparing), failure()).await });
    fixture.world.wait_began(cleanup, 1).await;
    let current = fixture.take_over(id).await?;
    let answer = json!(ready(&recovered_preparing));
    current
        .remember(
            &json!({ "file_result": answer, "file_source_attempt": recovered_preparing.plan }),
        )
        .await?;
    assert_eq!(current.pass_commit().await, Err(failure()));
    assert_eq!(recovered_preparing.publish(&current).await, Err(failure()));
    fixture.engine.finish_job(&current, Some(&answer)).await?;
    assert_eq!(current.row().await?.state, state::RUNNING);
    assert_eq!(fixture.source().await?["info"]["state"], "preparing");
    let intent = current.row().await?.plan["abort"].clone();
    fixture.world.machine.handler.release();
    tokio::time::timeout(common::PATIENCE, stale).await???;
    tokio::time::timeout(common::PATIENCE, fixture.engine.drive_job(&current)).await??;
    let row = current.row().await?;
    assert_eq!(row.state, state::FAILED);
    assert_eq!(row.plan["abort"], intent);
    assert!(row.result.is_none());
    assert_eq!(
        row.error.as_ref().and_then(|error| error.get("code")),
        Some(&json!("workspace.source_failed"))
    );
    let source = fixture.source().await?;
    assert_eq!(source["info"]["state"], "failed");
    assert_eq!(
        source["native_result"], answer,
        "a kept checkout retains its exact answer for explicit retry"
    );
    assert_eq!(fixture.world.began(cleanup), 1);
    fixture.assert_no_work(1).await
}

#[tokio::test]
async fn a_ready_source_is_never_failed_even_by_its_still_claimed_runner() -> TestResult {
    let fixture = Fixture::new().await?;
    let (job, preparing, _) = fixture.begin(fixture.plan(false).await?).await?;
    job.remember(&json!({ "file_result": ready(&preparing) }))
        .await?;
    assert_eq!(preparing.publish(&job).await, Ok(Done::Finished));
    let source = fixture.source().await?;
    let row = job.row().await?;
    assert!(matches!(
        preparing.undo(&job, &failure()).await,
        Undone::Partly(_)
    ));
    assert_eq!(fixture.source().await?, source);
    assert_eq!(job.row().await?, row);
    assert_eq!(fixture.world.began(CommandKind::GitPrepareCleanup), 0);
    fixture.assert_no_work(1).await
}

#[tokio::test]
async fn an_expired_claim_cannot_publish_success_or_failure_even_without_takeover() -> TestResult {
    let fixture = Fixture::new().await?;
    let (job, preparing, runner) = fixture.begin(fixture.plan(false).await?).await?;
    job.remember(&json!({ "file_result": ready(&preparing) }))
        .await?;
    let expected = job.row().await?;
    let source = fixture.source().await?;
    let until: chrono::DateTime<chrono::Utc> =
        serde_json::from_value(expected.plan["runner"]["until"].clone())?;
    fixture.clock.set(until);
    assert_eq!(preparing.publish(&job).await, Err(Stop::Lost));
    let fresh = Job::claimed(fixture.engine.clone(), job.id(), runner);
    fixture
        .engine
        .finish_job(&fresh, Some(&json!({ "late": true })))
        .await?;
    fixture
        .engine
        .end_job(&fresh, Some(&preparing), failure())
        .await?;
    assert_eq!(job.row().await?, expected);
    assert_eq!(fixture.source().await?, source);
    assert_eq!(fixture.world.began(CommandKind::GitPrepareCleanup), 0);
    fixture.assert_no_work(1).await
}

#[tokio::test]
async fn the_owned_cancel_still_undoes_publishes_failure_and_closes_cancelled() -> TestResult {
    let fixture = Fixture::new().await?;
    let (job, preparing, _) = fixture.begin(fixture.plan(false).await?).await?;
    fixture.hold_cleanup(false, false);
    fixture.world.machine.handler.release();
    let mut conn = fixture.world.pool.acquire().await?;
    assert!(
        rows::request_cancel(&mut conn, job.id(), fixture.world.ada, fixture.clock.now()).await?
    );
    drop(conn);
    fixture
        .engine
        .end_job(&job, Some(&preparing), Stop::Cancelled)
        .await?;
    let cancelled = job.row().await?;
    assert_eq!(cancelled.state, state::CANCELLED);
    assert_eq!(cancelled.plan["abort"]["kind"], "cancelled");
    assert_eq!(
        cancelled.error.as_ref().and_then(|error| error.get("code")),
        Some(&json!("cancelled"))
    );
    assert_eq!(fixture.source().await?["info"]["state"], "failed");
    assert_eq!(
        fixture.source().await?["info"]["job"]["id"],
        json!(job.id())
    );
    assert_eq!(fixture.world.began(CommandKind::GitPrepareCleanup), 1);
    fixture.assert_no_work(1).await
}

// The runtime/recipe rows below are an explicitly reconstructed enrolled-files checkpoint.
// Only unrelated provisioning cleanup is replaced: the real runner terminal path still loads
// the parent, proves its claim, fails its inline source, and writes the terminal job row.
struct AtFilesCheckpoint;

#[async_trait]
impl Phased for AtFilesCheckpoint {
    async fn run(&self, _job: &Job, _phase: &str) -> Result<Done, Stop> {
        Ok(Done::Finished)
    }
    async fn undo(&self, _job: &Job, _why: &Stop) -> Undone {
        Undone::Everything
    }
}

async fn inline_parent(fixture: &Fixture, keep_result: bool) -> TestResult<(Job, Preparing, Json)> {
    let mut plan = fixture.plan(false).await?;
    let parent = new_id();
    let runtime = new_id();
    let effect = crate::ids::derived(
        "source.provision.files",
        &[parent.as_bytes(), fixture.thread.as_bytes()],
    );
    if let Action::Prepare {
        source, effect_id, ..
    } = &mut plan.action
    {
        source.repository_url = None;
        *effect_id = Some(effect);
    }
    let mut tx = fixture.world.pool.begin().await?;
    let template: Id = sqlx::query_scalar("SELECT template_id FROM environments WHERE id=$1")
        .bind(fixture.world.environment)
        .fetch_one(&mut *tx)
        .await?;
    environments::hosting::insert_runtime(
        &mut tx,
        &environments::hosting::NewRuntime {
            id: runtime,
            host_id: fixture.host,
            unit_name: &format!("source-attempt-{runtime}"),
            environment_id: plan.environment,
            driver: "test",
            template_id: template,
            spec_hash: &[0; 32],
            generation: plan.generation,
            volumes: &json!({}),
        },
        fixture.clock.now(),
    )
    .await?;
    sqlx::query("UPDATE runtimes SET machine_id=$2 WHERE id=$1")
        .bind(runtime)
        .bind(plan.machine)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE environments SET generation=$2, machine_id=NULL, status='provisioning' WHERE id=$1",
    )
    .bind(plan.environment)
    .bind(plan.generation - 1)
    .execute(&mut *tx)
    .await?;
    let preparing = Preparing {
        engine: fixture.engine.clone(),
        plan,
        provisioning_runtime: Some(runtime),
    };
    let answer = json!(SourceInfo {
        kind: SourceKind::Directory,
        repository_url: None,
        ..ready(&preparing)
    });
    let attempt = if keep_result {
        json!(preparing.plan)
    } else {
        json!({ "other_thread": new_id() })
    };
    let row = rows::insert(&mut tx, &NewJob {
        id: parent, kind: kind::ENVIRONMENT_PROVISION, node_id: None, environment_id: Some(preparing.plan.environment),
        provider_id: None, plan: &json!({ "mode": "make", "runtime": runtime, "generation": preparing.plan.generation - 1,
            "file_source_attempt": attempt, "file_result": answer }),
        progress: &json!(initial_progress(&PHASES)), created_by: Some(fixture.world.ada),
    }, fixture.clock.now()).await?;
    let runner = new_id();
    assert!(
        rows::claim(
            &mut tx,
            row.id,
            runner,
            fixture.clock.now() + chrono::Duration::minutes(1),
            fixture.clock.now()
        )
        .await?
    );
    threads::lock(&mut tx, fixture.thread)
        .await?
        .ok_or("thread absent")?;
    let mut unit = Unit::new(&mut tx, fixture.clock.now());
    let seq = unit.stamp(fixture.thread).await?;
    let info = SourceInfo {
        kind: SourceKind::Directory,
        state: SourceState::Pending,
        head: None,
        repository_url: None,
        job: None,
        ..ready(&preparing)
    };
    let Action::Prepare { source: frozen, .. } = &preparing.plan.action else {
        return Err("prepare action absent".into());
    };
    source::set_source(&mut *unit.conn, fixture.thread, &json!({ "info": info, "source": frozen,
        "prepare": true, "caller": preparing.plan.caller, "token": null, "native_plan": preparing.plan }), seq).await?;
    unit.finish().await?;
    tx.commit().await?;
    Ok((
        Job::claimed(fixture.engine.clone(), row.id, runner),
        preparing,
        answer,
    ))
}

#[tokio::test]
async fn provisioning_failure_without_info_job_uses_the_current_parent_effect_and_its_own_native_answer()
-> TestResult {
    for keep_result in [false, true] {
        let fixture = Fixture::new().await?;
        let (parent, preparing, answer) = inline_parent(&fixture, keep_result).await?;
        assert!(
            fixture.source().await?["info"]["job"].is_null(),
            "inline sources have no standalone job reference"
        );
        fixture
            .engine
            .end_job(&parent, Some(&AtFilesCheckpoint), failure())
            .await?;
        let failed = fixture.source().await?;
        assert_eq!(failed["info"]["state"], "failed");
        assert_eq!(failed["info"]["job"]["id"], json!(parent.id()));
        assert_eq!(failed["native_plan"], json!(preparing.plan));
        assert_eq!(
            failed["native_result"],
            if keep_result { answer } else { Json::Null },
            "a preceding thread's file_result is never saved under this recipe"
        );
        assert_eq!(parent.row().await?.state, state::FAILED);
        fixture.assert_no_work(1).await?;
    }
    Ok(())
}

#[tokio::test]
async fn a_taken_over_provisioning_parent_cannot_fail_its_inline_source_but_the_current_owner_can()
-> TestResult {
    let fixture = Fixture::new().await?;
    let (old, preparing, _) = inline_parent(&fixture, true).await?;
    let current = fixture.take_over(old.id()).await?;
    let expected_job = current.row().await?;
    let expected_source = fixture.source().await?;
    fixture
        .engine
        .end_job(&old, Some(&AtFilesCheckpoint), failure())
        .await?;
    preparing.undo(&old, &failure()).await;
    assert_eq!(current.row().await?, expected_job);
    assert_eq!(fixture.source().await?, expected_source);
    fixture
        .engine
        .end_job(&current, Some(&AtFilesCheckpoint), Stop::Cancelled)
        .await?;
    assert_eq!(current.row().await?.state, state::CANCELLED);
    assert_eq!(fixture.source().await?["info"]["state"], "failed");
    fixture.assert_no_work(1).await
}

#[cfg(unix)]
#[path = "attempt_native_tests.rs"]
mod native;
