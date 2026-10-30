//! A thread made into another: a switch in place to another provider, profile or environment, and a fork to a thread of its own. The plan says how the history will travel and what blocks it; the job holds the thread, opens the new session with the history, commits in one transaction, and puts everything back when it fails or is cancelled before that.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

#[path = "forks/approval_mode.rs"]
mod approval_mode;
#[path = "forks/call_recovery.rs"]
mod call_recovery;
mod common;
#[path = "forks/confirmed_source.rs"]
mod confirmed_source;
#[path = "forks/diagnostics.rs"]
mod diagnostics;
#[path = "forks/history_size.rs"]
mod history_size;
mod hosting;
#[path = "forks/native_transfer.rs"]
mod native_transfer;
#[path = "forks/snapshot.rs"]
mod snapshot;

use common::{MODEL, World, eventually, meanwhile, queued};
use orch_api_contract::forks::{
    ActiveTurn, ChangeOptions, ForkCreateBody, ForkHistory, ForkPlan, ForkRequest, ForkResult,
    ForkTo, HistoryMethod, HistoryMode,
};
use orch_authz::Caller;
use orch_blob_contract::store::BlobStore;
use orch_blob_local::LocalBlobStore;
use orch_db::queries::jobs::{self as jobs, JobRow};
use orch_engine::fork_plan::PlanError;
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
    ExportKind, SessionExportResult, SessionMode, SessionOpenResult,
};
use orch_link_contract::events::TurnOutcome;
use orch_link_contract::streams::Stream;
use orch_types::{Hash, Id, Json, new_id};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// What a harness of the tests can do with a history.
#[derive(Clone, Copy, Default)]
struct Can {
    export_native: bool,
    import_native: bool,
    import_neutral: bool,
    fork_native: bool,
}

fn descriptor_with(kind: &str, can: Can) -> Json {
    let mut descriptor = common::descriptor();
    descriptor["kind"] = json!(kind);
    if kind == "dsh" {
        descriptor["approval_modes"] = json!(["ask", "bypass"]);
    }
    descriptor["native_provider"]["harness_kind"] = json!(kind);
    descriptor["capabilities"]["export_native"] = json!(can.export_native);
    descriptor["capabilities"]["import_native"] = json!(can.import_native);
    descriptor["capabilities"]["import_neutral"] = json!(can.import_neutral);
    descriptor["capabilities"]["fork_native"] = json!(can.fork_native);
    descriptor
}

/// A world with a second agent profile on a second provider, both on the world's machine, and a store of
/// content for what travels.
struct Rig {
    world: World,
    store: Arc<LocalBlobStore>,
    other_agent: Id,
    other_provider: Id,
    exports: Arc<Mutex<Vec<Command>>>,
    /// The file commands the machine was given (`fs.snapshot` and `fs.materialize`), in order.
    files: Arc<Mutex<Vec<Command>>>,
    native: Arc<Mutex<confirmed_source::NativePeer>>,
    open_refusals: Arc<Mutex<std::collections::VecDeque<CommandError>>>,
}

impl Rig {
    /// The first harness can `source` and the second `target`.
    async fn new(source: Can, target_kind: &str, target: Can) -> Self {
        Self::with_source_root(source, target_kind, target, None).await
    }

    async fn with_source_root(
        source: Can,
        target_kind: &str,
        target: Can,
        physical_root: Option<&'static str>,
    ) -> Self {
        Self::with_kinds(source, "fake", target_kind, target, physical_root).await
    }

    async fn with_kinds(
        source: Can,
        source_kind: &str,
        target_kind: &str,
        target: Can,
        physical_root: Option<&'static str>,
    ) -> Self {
        diagnostics::install().expect("the narrow fork database diagnostic sink must be installed");
        let world = World::new().await;
        for kind in [source_kind, target_kind]
            .into_iter()
            .filter(|kind| *kind != "fake")
        {
            // This machine's scripted driver also starts these harnesses below.
            sqlx::query("UPDATE machines SET facts = jsonb_set(jsonb_set(jsonb_set(facts,
                '{adapters}', (facts -> 'adapters') || $2),
                '{harnesses}', (facts -> 'harnesses') || $3),
                '{policy,agent.harnesses}', (facts #> '{policy,agent.harnesses}') || $4) WHERE id = $1")
                .bind(world.machine_id)
                .bind(json!([{"kind": kind, "adapter_version": 1, "builtin": true}]))
                .bind(json!([{"kind": kind, "path": "/test/harness", "version": "1.0.0", "daemon": false}]))
                .bind(json!([{"kind": kind, "path": "/test/harness"}]))
                .execute(world.pool.pool()).await.unwrap();
        }
        let root = std::env::temp_dir().join(format!("orch-fork-test-{}", new_id()));
        let store = Arc::new(LocalBlobStore::open(root).await.unwrap());
        world.engine().attach_blobs(store.clone());
        sqlx::query("UPDATE agents SET descriptor = $2, harness_kind = $3 WHERE id = $1")
            .bind(world.agent)
            .bind(descriptor_with(source_kind, source))
            .bind(source_kind)
            .execute(world.pool.pool())
            .await
            .unwrap();
        sqlx::query("UPDATE providers SET harness_kind = $2 WHERE id = $1")
            .bind(world.provider)
            .bind(source_kind)
            .execute(world.pool.pool())
            .await
            .unwrap();
        let (other_agent, other_provider) = (new_id(), new_id());
        sqlx::query(
            "INSERT INTO agents (id, name, owner_id, org_id, home_id, harness_kind, profile, descriptor)
             VALUES ($1, 'Other agent', $2, $3, $3, $4, '{}', $5)",
        )
        .bind(other_agent)
        .bind(world.ada)
        .bind(world.root)
        .bind(target_kind)
        .bind(descriptor_with(target_kind, target))
        .execute(world.pool.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO providers (id, name, owner_id, org_id, home_id, kind, harness_kind, agent_id,
                                    machine_id, arrangements, status)
             VALUES ($1, 'Other subscription', $2, $3, $3, 'subscription', $4, $5, $6,
                     ARRAY['separated'], 'ready')",
        )
        .bind(other_provider)
        .bind(world.ada)
        .bind(world.root)
        .bind(target_kind)
        .bind(other_agent)
        .bind(world.machine_id)
        .execute(world.pool.pool())
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO provider_models (id, provider_id, model_name, display_name, parameters, enabled, source, created_by)
             VALUES ($1, $2, $3, 'Fake One', $4, true, 'manual', $5)",
        )
        .bind(new_id())
        .bind(other_provider)
        .bind(MODEL)
        .bind(json!({ "efforts": ["low", "high"], "default_effort": "low" }))
        .bind(world.ada)
        .execute(world.pool.pool())
        .await
        .unwrap();
        // Each harness reports, when it is started, what its profile declares of it: the world's own answer is
        // the same for every profile and would put back the abilities that were taken away above.
        let reports = [
            (world.agent, descriptor_with(source_kind, source)),
            (other_agent, descriptor_with(target_kind, target)),
        ];
        world
            .machine
            .handler
            .on(CommandKind::AgentStart, move |command| {
                let Command::AgentStart(start) = command else {
                    return Err(CommandError::new(ErrorCode::Internal, "not an agent.start"));
                };
                let descriptor = reports
                    .iter()
                    .find(|(agent, _)| *agent == start.agent_id)
                    .map_or_else(common::descriptor, |(_, descriptor)| descriptor.clone());
                Ok(CommandResult::AgentStart(AgentStartResult {
                    reused: false,
                    harness_version: "1.0.0".to_owned(),
                    descriptor: serde_json::from_value(descriptor).unwrap(),
                }))
            });
        // The harness writes its archive when asked.
        let exports = Arc::new(Mutex::new(Vec::new()));
        // Snapshot uploads real fixture bytes/a valid manifest. Keep the
        // original declared materialization controls and their assertions.
        let files = Arc::new(Mutex::new(Vec::new()));
        let source_environment = world.environment;
        for kind in [
            CommandKind::FsManifest,
            CommandKind::FsSnapshot,
            CommandKind::FsMaterialize,
        ] {
            let kept = Arc::clone(&files);
            let (store, pool, clock) = (store.clone(), world.pool.clone(), world.clock.clone());
            world.machine.handler.on(kind, move |command| {
                kept.lock().unwrap().push(command.clone());
                let (environment, selection) = match command {
                    Command::FsManifest(body) => (body.environment_id, &body.selection),
                    Command::FsSnapshot(body) => (body.environment_id, &body.selection),
                    _ => {
                        return Ok(CommandResult::FsMaterialize(
                            orch_link_contract::commands::FsMaterializeResult {
                                files: 3,
                                bytes: 30,
                                skipped: 0,
                            },
                        ));
                    }
                };
                let taken = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(snapshot::uploaded(
                        environment,
                        selection,
                        physical_root.filter(|_| environment == source_environment),
                        store.as_ref(),
                        &pool,
                        clock.as_ref(),
                    ))
                })?;
                Ok(if matches!(command, Command::FsManifest(_)) {
                    CommandResult::FsManifest(orch_link_contract::commands::FsManifestResult {
                        manifest_blob: taken.manifest_blob,
                        file_count: taken.file_count,
                        total_bytes: taken.total_bytes,
                    })
                } else {
                    CommandResult::FsSnapshot(taken)
                })
            });
        }
        let native = Arc::new(Mutex::new(confirmed_source::NativePeer::default()));
        let rig = Self {
            world,
            store,
            other_agent,
            other_provider,
            exports,
            files,
            native,
            open_refusals: Arc::new(Mutex::new(std::collections::VecDeque::new())),
        };
        rig.install_peer();
        rig
    }

    fn caller(&self) -> Caller {
        Caller::session(self.world.ada)
    }

    /// A thread that has said something and was answered.
    async fn thread_with_history(&self, name: &str) -> Id {
        let thread = self.world.thread(name).await;
        self.say(thread, "Plan the work").await;
        self.world.run_turn(thread, 0, "The plan is ready").await;
        self.world.wait_state(thread, "idle").await;
        self.record_native(thread).await;
        thread
    }

    async fn say(&self, thread: Id, words: &str) {
        self.world
            .engine()
            .submit_input(&self.caller(), thread, queued(words))
            .await
            .unwrap();
    }

    /// A switch to the other provider and profile.
    fn to_other(&self) -> ForkRequest {
        ForkRequest {
            result: ForkResult::Replace,
            topic_id: None,
            name: None,
            access: None,
            history: None,
            to: Some(ForkTo {
                provider_id: Some(self.other_provider),
                agent_id: Some(self.other_agent),
                ..ForkTo::default()
            }),
            environment: None,
            worktree: None,
            files_at: None,
            memory: None,
            directory: None,
            cwd: None,
            options: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: None,
        }
    }

    async fn plan(&self, thread: Id, request: &ForkRequest) -> ForkPlan {
        let draft = self
            .world
            .engine()
            .draft_fork(&self.caller(), thread, request)
            .await
            .unwrap();
        self.world
            .engine()
            .keep_fork_plan(&self.caller(), draft)
            .await
            .unwrap()
    }

    /// Carries out the plan and answers the job that was made.
    async fn confirm(&self, thread: Id, plan: &ForkPlan) -> JobRow {
        self.record_native(thread).await;
        self.world
            .engine()
            .confirm_fork(
                &self.caller(),
                thread,
                &ForkCreateBody {
                    notify: None,
                    preflight_id: plan.preflight_id,
                    options: None,
                },
            )
            .await
            .unwrap()
    }

    async fn job(&self, job: Id) -> JobRow {
        let mut conn = self.world.pool.acquire().await.unwrap();
        jobs::job(&mut conn, job).await.unwrap().unwrap()
    }

    async fn wait_job(&self, job: Id, state: &str) -> JobRow {
        let mut last = String::new();
        // Two minutes: the machines the tests run on are often busy with other builds.
        for _ in 0..2400 {
            let row = self.job(job).await;
            if row.state == state {
                return row;
            }
            last = row.state.clone();
            if !row.is_open() {
                panic!("the job ended {} ({:?}), not {state}", row.state, row.error);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("the job did not become {state}: it is {last}");
    }

    /// Waits until the job is over, in whatever state.
    async fn wait_job_over(&self, job: Id, more: impl Fn() -> String) -> JobRow {
        for _ in 0..3600 {
            let row = self.job(job).await;
            if !row.is_open() {
                return row;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let row = self.job(job).await;
        let environments: Vec<(String, String, Option<String>)> = sqlx::query_as(
            "SELECT name, status, status_detail FROM environments ORDER BY created_at",
        )
        .fetch_all(self.world.pool.pool())
        .await
        .unwrap();
        let jobs: Vec<(String, String, Option<String>, Option<Json>)> = sqlx::query_as(
            "SELECT kind, state, progress ->> 'phase', error FROM jobs ORDER BY created_at",
        )
        .fetch_all(self.world.pool.pool())
        .await
        .unwrap();
        panic!(
            "the job did not end: {} {:?}\nenvironments: {environments:?}\njobs: {jobs:?}\n{}",
            row.state,
            row.progress,
            more()
        );
    }

    async fn blob_text(&self, hash: &Hash) -> String {
        let read = self.store.get(hash, None).await.unwrap();
        let bytes = orch_blob_contract::store::read_all(read.body)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn opens(&self) -> Vec<orch_link_contract::commands::SessionOpen> {
        self.world
            .ordered
            .lock()
            .unwrap()
            .iter()
            .filter_map(|(_, command)| match command {
                Command::SessionOpen(open) => Some(open.clone()),
                Command::SessionOpenFenced(open) => Some(*open.session.clone()),
                _ => None,
            })
            .collect()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_checked_native_import_failure_is_named_and_carry_uses_neutral_without_an_export_attempt()
{
    let can = Can {
        export_native: true,
        import_native: true,
        import_neutral: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.thread_with_history("Checked carry").await;
    let before = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(before.history.method, HistoryMethod::NativeTransplant);
    let build = json!({"kind":"fake","version":"1.0.0","os":"linux","arch":"x86_64",
        "url":"https://build.test/program","sha256":"a".repeat(64),"is_default":false,
        "source":"admin","status":"failed","check":{"at":rig.world.clock.now(),"machine":"Check host",
        "missing":[],"off":["import_native"],"error":null,"stage":"complete",
        "cases":[{"name":"native_import","passed":false,"feature":"import_native","error":"unsupported paginated archive","elapsed_ms":12}]}});
    let mut conn = rig.world.pool.acquire().await.unwrap();
    orch_db::queries::resources::management::put_harness_build(
        &mut conn,
        "harness_build:fake:1.0.0:linux:x86_64",
        &build,
        rig.world.ada,
        rig.world.clock.now(),
    )
    .await
    .unwrap();
    // The selected unit still reports this build even after another unit updates the profile's last report.
    orch_db::queries::resources::thread_engine::set_harness_status(
        &mut conn,
        rig.other_provider,
        rig.other_agent,
        rig.world.machine_id,
        &orch_db::queries::resources::thread_engine::HarnessReport {
            status: "online",
            detail: None,
            version: Some("1.0.0"),
            sessions_open: 0,
        },
        rig.world.clock.now(),
    )
    .await
    .unwrap();
    orch_db::queries::resources::thread_engine::set_harness_descriptor(
        &mut conn,
        rig.other_provider,
        rig.other_agent,
        rig.world.machine_id,
        &descriptor_with("fake", can),
        rig.world.clock.now(),
    )
    .await
    .unwrap();
    sqlx::query("UPDATE agents SET descriptor=jsonb_set(descriptor,'{harness,version}',to_jsonb('2.0.0'::text)), descriptor_overrides=$2 WHERE id=$1")
        .bind(rig.other_agent).bind(json!({"capabilities":{"import_native":true},"harness":{"version":"9.0.0"}})).execute(&mut *conn).await.unwrap();
    drop(conn);
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    let unavailable = plan
        .history
        .native_import_unavailable
        .as_ref()
        .expect("the checked build is named");
    assert_eq!(
        (&*unavailable.kind, &*unavailable.version),
        ("fake", "1.0.0")
    );
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    assert!(
        rig.opens()
            .iter()
            .any(|open| open.mode == SessionMode::ImportNeutral)
    );
    assert!(
        !rig.opens()
            .iter()
            .any(|open| open.mode == SessionMode::ImportNative)
    );
    assert!(rig.exports.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_between_two_harnesses_of_one_kind_moves_the_whole_session_and_the_thread_goes_on_there()
 {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.thread_with_history("Switched").await;
    let old_segment = rig.world.segment_of(thread).await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::NativeTransplant);
    assert_eq!(plan.history.turns, 1);
    assert_eq!(
        plan.owner.before.as_ref().map(|user| user.id),
        Some(rig.world.ada),
        "a switch keeps the first owner it had"
    );
    assert_eq!(plan.owner.before, Some(plan.owner.after.clone()));
    assert_eq!(
        plan.locks.len(),
        1,
        "the thread stands still from confirmation"
    );
    assert!(plan.leaves.as_ref().is_some_and(|leaves| leaves.session));
    // The plan says what the people on the thread's page need on the provider it goes to to go on.
    assert!(
        plan.depends.iter().any(|dependency| {
            dependency.object.id == rig.other_provider && dependency.permission == "provider.use"
        }),
        "{:?}",
        plan.depends
    );

    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;

    // The thread is on the other side, in a segment of its own that holds how the history arrived.
    let new_segment = rig.world.segment_of(thread).await;
    assert_ne!(new_segment, old_segment);
    let (provider, agent): (Id, Id) =
        sqlx::query_as("SELECT provider_id, agent_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert_eq!((provider, agent), (rig.other_provider, rig.other_agent));
    let reason: Option<String> = rig
        .world
        .scalar("SELECT end_reason FROM segments WHERE id = $1", old_segment)
        .await;
    assert_eq!(reason.as_deref(), Some("switched"));
    let fidelity: Option<String> = rig
        .world
        .scalar(
            "SELECT import_fidelity FROM segments WHERE id = $1",
            new_segment,
        )
        .await;
    assert_eq!(fidelity.as_deref(), Some("lossless"));
    let session: Option<String> = rig
        .world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id = $1",
            new_segment,
        )
        .await;
    assert_eq!(session, Some(format!("native-{new_segment}")));
    // What was asked of the harnesses: the old one wrote its archive, the new one read it.
    assert_eq!(rig.exports.lock().unwrap().len(), 1);
    let Command::SessionExport(export) = rig.exports.lock().unwrap()[0].clone() else {
        panic!("a session was exported");
    };
    assert_eq!(
        (export.thread_id, export.kind),
        (thread, ExportKind::Native)
    );
    let import = rig
        .opens()
        .into_iter()
        .find(|open| open.mode == SessionMode::ImportNative)
        .expect("the new harness was given the archive");
    assert_eq!(import.segment_id, new_segment);
    assert_eq!(
        import
            .archive
            .as_ref()
            .map(|archive| archive.format.as_str()),
        Some("fake-v1")
    );
    assert_eq!(import.upto_turn_ordinal, Some(1));
    // One line in the thread's history says so.
    let markers = rig
        .world
        .count(
            "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload ->> 'marker' = 'switched'",
            thread,
        )
        .await;
    assert_eq!(markers, 1);
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM audit_log WHERE node_id = $1 AND action = 'thread.switched'",
                thread
            )
            .await,
        1
    );
    // The thread is free again and answers on the other side.
    rig.world.wait_state(thread, "idle").await;
    rig.say(thread, "Go on").await;
    let start = rig.world.turn_start(thread, 1).await;
    let opens = rig.opens();
    assert_eq!(
        opens.last().map(|open| open.mode),
        Some(SessionMode::Resume),
        "the first turn on the new side takes up the session that was opened with the history"
    );
    assert_eq!(start.thread_id, thread);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_plan_refuses_a_destination_provider_outside_the_places_address_policy() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let thread = rig.world.thread("Switch destination policy").await;
    let address = {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        orch_authz::resources::addresses(&mut conn, &[rig.world.provider])
            .await
            .unwrap()[0]
            .1
            .clone()
    };
    rig.world
        .set(
            rig.world.topic,
            orch_settings_contract::registry::keys::PROVIDERS_ALLOWED,
            json!([address]),
        )
        .await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(
        plan.blockers
            .iter()
            .any(|blocker| blocker.code == "provider_not_allowed")
    );
    assert!(!plan.blockers.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_to_another_kind_of_harness_hands_the_history_over_as_items() {
    let source = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(
        source,
        "fake2",
        Can {
            import_neutral: true,
            ..Can::default()
        },
    )
    .await;
    let thread = rig.thread_with_history("Neutral").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.history.method, HistoryMethod::NeutralImport);
    let report = &plan.history.report;
    assert_eq!(report.by_kind["user_message"].kept, 1);
    assert_eq!(report.by_kind["agent_message"].kept, 1);
    assert!(plan.history.archive_bytes.is_some_and(|bytes| bytes > 0));
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let import = rig
        .opens()
        .into_iter()
        .find(|open| open.mode == SessionMode::ImportNeutral)
        .expect("the new harness was given the history as items");
    let history = import.history.expect("a history blob");
    let text = rig.blob_text(&history.blob).await;
    let kinds: Vec<String> = text
        .lines()
        .map(|line| {
            serde_json::from_str::<Json>(line).unwrap()["kind"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["user_message", "agent_message"]);
    assert!(text.contains("The plan is ready"));
    let new_segment = rig.world.segment_of(thread).await;
    let fidelity: Option<String> = rig
        .world
        .scalar(
            "SELECT import_fidelity FROM segments WHERE id = $1",
            new_segment,
        )
        .await;
    assert_eq!(fidelity.as_deref(), Some("structured"));
    assert_eq!(
        rig.exports.lock().unwrap().len(),
        0,
        "nothing of the harness's own is asked for where only the items are read"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_to_a_harness_that_reads_no_history_leaves_a_transcript_and_tells_the_agent_where_it_is()
 {
    let rig = Rig::new(Can::default(), "fake2", Can::default()).await;
    let thread = rig.thread_with_history("Summed").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.history.method, HistoryMethod::FileHandoff);
    assert_eq!(
        plan.history.report.fidelity,
        orch_api_contract::forks::Fidelity::Structured
    );
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    // The transcript was written under `.orch` of the thread's directory.
    let written: Vec<Command> = rig
        .files
        .lock()
        .unwrap()
        .iter()
        .filter(|command| matches!(command, Command::FsMaterialize(_)))
        .cloned()
        .collect();
    let Some(Command::FsMaterialize(write)) = written.last().cloned() else {
        panic!("the transcript was written: {written:?}");
    };
    assert!(
        write.target_root.ends_with("/web/.orch"),
        "{}",
        write.target_root
    );
    let manifest = rig
        .world
        .engine()
        .read_manifest(&write.manifest_blob)
        .await
        .unwrap();
    let entry = manifest
        .get(&format!("handoff/{thread}.md"))
        .expect("the transcript is in the list");
    let text = rig.blob_text(&entry.hash.unwrap()).await;
    assert!(
        text.contains("Plan the work") && text.contains("The plan is ready"),
        "{text}"
    );
    // The new session begins fresh; a handoff does not create a turn or bypass a held queue.
    let segment = rig.world.segment_of(thread).await;
    assert!(
        rig.opens()
            .iter()
            .any(|open| open.mode == SessionMode::New && open.segment_id == segment)
    );
    assert_eq!(rig.world.turn_starts(thread).len(), 1);
    rig.world
        .engine()
        .submit_input(
            &Caller::session(rig.world.ada),
            thread,
            queued("Check the regression test"),
        )
        .await
        .unwrap();
    let start = rig.world.turn_start(thread, 1).await;
    assert!(
        start.text.contains(&format!(".orch/handoff/{thread}.md")),
        "{}",
        start.text
    );
    // Where the new agent imports nothing, the recent turns go into its context beside the file.
    assert!(
        start.text.contains("Plan the work") && start.text.contains("The plan is ready"),
        "{}",
        start.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_asked_for_a_summary_where_no_summarizer_is_chosen_carries_the_last_turns_briefly()
{
    let rig = Rig::new(Can::default(), "fake2", Can::default()).await;
    let thread = rig.thread_with_history("Briefed").await;
    let summary = ForkRequest {
        history: Some(ForkHistory {
            mode: HistoryMode::Summary,
            from_turn: None,
            to_turn: None,
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(thread, &summary).await;
    // No summarizer is no blocker: the last turns stand in for the summary.
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.mode, HistoryMode::Summary);
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    rig.world
        .engine()
        .submit_input(&Caller::session(rig.world.ada), thread, queued("Go on"))
        .await
        .unwrap();
    let start = rig.world.turn_start(thread, 1).await;
    assert!(
        start
            .text
            .contains("only turns 1 to 1 of 1 are in your context")
            && start.text.contains("Plan the work")
            && start.text.contains(&format!(".orch/handoff/{thread}.md")),
        "{}",
        start.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_that_names_no_history_carries_what_the_place_carries_as_its_plan_showed() {
    let rig = Rig::new(Can::default(), "fake2", Can::default()).await;
    let thread = rig.thread_with_history("Placed").await;
    assert_eq!(
        rig.plan(thread, &rig.to_other()).await.history.mode,
        HistoryMode::All
    );
    rig.world
        .set(rig.world.topic, "thread.switch_history", json!("none"))
        .await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert_eq!(plan.history.mode, HistoryMode::Nothing);
    assert_eq!(plan.history.method, HistoryMethod::Nothing);
    // The plan that was shown is the one carried out, though the place changes its default before the confirmation.
    rig.world
        .set(rig.world.topic, "thread.switch_history", json!("carry"))
        .await;
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    rig.world
        .engine()
        .submit_input(
            &Caller::session(rig.world.ada),
            thread,
            queued("Start over"),
        )
        .await
        .unwrap();
    let start = rig.world.turn_start(thread, 1).await;
    assert!(
        !start.text.contains(".orch/handoff/") && !start.text.contains("Plan the work"),
        "{}",
        start.text
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_plan_that_changes_nothing_or_has_blockers_is_not_carried_out_and_one_that_is_old_is_refused()
 {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let thread = rig.thread_with_history("Refused").await;
    // Nothing would change: a model or an effort alone is a setting.
    let same = ForkRequest {
        to: Some(ForkTo {
            model: Some(MODEL.to_owned()),
            effort: Some("high".to_owned()),
            ..ForkTo::default()
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(thread, &same).await;
    assert_eq!(
        plan.blockers
            .iter()
            .map(|blocker| blocker.code.as_str())
            .collect::<Vec<_>>(),
        ["nothing_to_change"]
    );
    let blocked = rig
        .world
        .engine()
        .confirm_fork(
            &rig.caller(),
            thread,
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await;
    assert!(matches!(blocked, Err(PlanError::Blocked(_))), "{blocked:?}");
    assert!(rig.world.engine().db().acquire().await.is_ok());
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM jobs WHERE node_id = $1", thread)
            .await,
        0,
        "a plan with blockers makes no job and holds nothing"
    );
    // A plan is good for fifteen minutes.
    let plan = rig.plan(thread, &rig.to_other()).await;
    sqlx::query("UPDATE preflights SET expires_at = now() - interval '1 minute' WHERE id = $1")
        .bind(plan.preflight_id)
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    let old = rig
        .world
        .engine()
        .confirm_fork(
            &rig.caller(),
            thread,
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await;
    assert!(matches!(old, Err(PlanError::Expired)), "{old:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_that_is_switched_starts_no_turn_and_keeps_what_is_sent_until_the_switch_is_done()
{
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.thread_with_history("Held").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    // From the moment it is accepted, the thread is held.
    let (state, reason, _) = rig.world.state(thread).await;
    assert_eq!(
        (state.as_str(), reason.as_deref()),
        ("blocked", Some("switching"))
    );
    let locked = orch_engine::locks::lock_of(&mut rig.world.pool.acquire().await.unwrap(), thread)
        .await
        .unwrap()
        .expect("a job holds the thread");
    assert_eq!(locked.job.id, job.id);
    // Something is sent to it meanwhile: it is taken, queued, and starts nothing.
    rig.say(thread, "While you were away").await;
    meanwhile(Duration::from_millis(400)).await;
    assert_eq!(
        rig.world.turn_starts(thread).len(),
        1,
        "no turn starts under a lock"
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM thread_inputs WHERE thread_id = $1 AND state = 'pending'",
                thread
            )
            .await,
        1
    );
    // A second change of the thread is refused while the first is under way.
    let again = rig
        .world
        .engine()
        .draft_fork(&rig.caller(), thread, &rig.to_other())
        .await
        .unwrap();
    assert!(
        again
            .plan
            .blockers
            .iter()
            .any(|blocker| blocker.code == "already_running"),
        "{:?}",
        again.plan.blockers
    );
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
    // The queue runs on the new side.
    let start = rig.world.turn_start(thread, 1).await;
    assert!(start.text.contains("While you were away"));
    let provider: Id = rig
        .world
        .scalar("SELECT provider_id FROM threads WHERE node_id = $1", thread)
        .await;
    assert_eq!(provider, rig.other_provider);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_that_fails_leaves_the_thread_exactly_as_it_was_and_what_was_queued_runs_on_the_old_side()
 {
    // The new harness reads only its own archive, and refuses it: nothing else can be done for it.
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.thread_with_history("Failing").await;
    let old_segment = rig.world.segment_of(thread).await;
    let (agent, provider): (Id, Id) =
        sqlx::query_as("SELECT agent_id, provider_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let job = rig.confirm(thread, &plan).await;
    rig.say(thread, "Wait for me").await;
    rig.refuse_preparations(vec![
        CommandError::new(ErrorCode::Internal, "the harness refuses the archive"),
        CommandError::new(ErrorCode::Internal, "and the transcript"),
    ]);
    rig.world.machine.handler.release();
    let row = rig.wait_job(job.id, "failed").await;
    assert_eq!(row.state, "failed", "{:?}", row.error);
    let error = row.error.expect("the job says why");
    assert_eq!(error["undone"], json!(true));
    assert_eq!(error["phase"], json!("history"));
    // The thread is as it was: its binding, its segment, no lock.
    let (now_agent, now_provider): (Id, Id) =
        sqlx::query_as("SELECT agent_id, provider_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert_eq!((now_agent, now_provider), (agent, provider));
    assert_eq!(rig.world.segment_of(thread).await, old_segment);
    let ended: Option<String> = rig
        .world
        .scalar("SELECT end_reason FROM segments WHERE id = $1", old_segment)
        .await;
    assert_eq!(ended, None, "the old segment goes on");
    assert!(
        orch_engine::locks::lock_of(&mut rig.world.pool.acquire().await.unwrap(), thread)
            .await
            .unwrap()
            .is_none()
    );
    // What was queued while it was held runs, on the side it is still on.
    let start = rig.world.turn_start(thread, 1).await;
    assert!(start.text.contains("Wait for me"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_cancelled_before_its_commit_puts_everything_back() {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.thread_with_history("Cancelled").await;
    let old_segment = rig.world.segment_of(thread).await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let job = rig.confirm(thread, &plan).await;
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, 1)
        .await;
    assert!(
        rig.world
            .engine()
            .cancel_job(job.id, rig.world.ada)
            .await
            .unwrap()
    );
    rig.world.machine.handler.release();
    let row = loop {
        let row = rig.job(job.id).await;
        if !row.is_open() {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(row.state, "cancelled");
    assert_eq!(row.error.as_ref().unwrap()["undone"], json!(true));
    assert_eq!(rig.world.segment_of(thread).await, old_segment);
    rig.world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_is_waited_for_and_the_switch_goes_on_when_it_ends() {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let thread = rig.world.thread("Busy").await;
    let (start, _) = rig.world.running_turn(thread, "Work on it").await;
    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.turn_running);
    let job = rig.confirm(thread, &plan).await;
    eventually(
        "the switch job is admitted before sampling its held turn",
        || async {
            let row = rig.job(job.id).await;
            assert!(
                row.is_open(),
                "the waiting job ended {}: {:?}",
                row.state,
                row.error
            );
            row.state == "running"
        },
    )
    .await;
    meanwhile(Duration::from_millis(500)).await;
    assert_eq!(
        rig.job(job.id).await.state,
        "running",
        "it waits for the turn"
    );
    assert!(
        rig.opens().len() == 1,
        "nothing is changed while a turn runs: {:?}",
        rig.opens().len()
    );
    rig.world
        .say(
            thread,
            common::turn_ended(
                start.turn_id,
                orch_link_contract::events::TurnOutcome::Completed,
            ),
        )
        .await;
    rig.wait_job(job.id, "succeeded").await;
    let provider: Id = rig
        .world
        .scalar("SELECT provider_id FROM threads WHERE node_id = $1", thread)
        .await;
    assert_eq!(provider, rig.other_provider);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_running_turn_is_interrupted_at_once_when_the_options_say_so_and_a_wait_that_runs_out_changes_nothing()
 {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    // A wait of no minutes gives up at once.
    let thread = rig.world.thread("Impatient").await;
    let (_, _) = rig.world.running_turn(thread, "Work on it").await;
    let request = ForkRequest {
        options: Some(ChangeOptions {
            active_turn: Some(ActiveTurn::Wait),
            wait_minutes: Some(0),
            ..ChangeOptions::default()
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(thread, &request).await;
    let job = rig.confirm(thread, &plan).await;
    let row = loop {
        let row = rig.job(job.id).await;
        if !row.is_open() {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    assert_eq!(row.state, "failed");
    assert_eq!(row.error.as_ref().unwrap()["code"], json!("wait_timeout"));
    assert_eq!(row.error.as_ref().unwrap()["undone"], json!(true));
    let provider: Id = rig
        .world
        .scalar("SELECT provider_id FROM threads WHERE node_id = $1", thread)
        .await;
    assert_eq!(provider, rig.world.provider, "nothing was changed");
    // The turn goes on after it: the thread was never stopped.
    assert_eq!(rig.world.state(thread).await.0, "running");
    // Told to interrupt, it does not wait.
    let request = ForkRequest {
        options: Some(ChangeOptions {
            active_turn: Some(ActiveTurn::Interrupt),
            ..ChangeOptions::default()
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(thread, &request).await;
    let job = rig.confirm(thread, &plan).await;
    rig.wait_began_interrupt().await;
    rig.wait_job(job.id, "succeeded").await;
}

impl Rig {
    async fn wait_began_interrupt(&self) {
        eventually("the turn was asked to stop", || {
            let asked = self.world.ran(CommandKind::TurnInterrupt) > 0
                || self.world.commands.issued_of("turn.interrupt") > 0;
            async move { asked }
        })
        .await;
    }
}

// ---------------------------------------------------------------- a fork

impl Rig {
    /// A fork with everything left as the source has it.
    fn fork(&self) -> ForkRequest {
        ForkRequest {
            result: ForkResult::New,
            topic_id: None,
            name: Some("Trial".to_owned()),
            access: None,
            history: None,
            to: None,
            environment: None,
            worktree: None,
            files_at: None,
            memory: None,
            directory: None,
            cwd: None,
            options: None,
            check_fingerprints: None,
            fingerprint_request_id: None,
            mounts: None,
        }
    }

    async fn plan_for(&self, person: Id, thread: Id, request: &ForkRequest) -> ForkPlan {
        let caller = Caller::session(person);
        let draft = self
            .world
            .engine()
            .draft_fork(&caller, thread, request)
            .await
            .unwrap();
        self.world
            .engine()
            .keep_fork_plan(&caller, draft)
            .await
            .unwrap()
    }

    async fn confirm_for(&self, person: Id, thread: Id, plan: &ForkPlan) -> JobRow {
        self.record_native(thread).await;
        self.world
            .engine()
            .confirm_fork(
                &Caller::session(person),
                thread,
                &ForkCreateBody {
                    notify: None,
                    preflight_id: plan.preflight_id,
                    options: None,
                },
            )
            .await
            .unwrap()
    }

    fn new_thread_of(&self, job: &JobRow) -> Id {
        job.plan["new_thread"].as_str().unwrap().parse().unwrap()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_is_its_makers_and_takes_the_history_and_the_source_goes_on_untouched() {
    let can = Can {
        export_native: true,
        import_native: true,
        fork_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let source = rig.thread_with_history("Source").await;
    let source_segment = rig.world.segment_of(source).await;
    // R1's real source had both model inputs and two sub-agent streams. Keep that shape in the gate.
    let source_records: Vec<(Id, Option<Id>, Json)> = sqlx::query_as(
        "SELECT id,segment_id,blocks FROM model_inputs WHERE node_id=$1 ORDER BY id",
    )
    .bind(source)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert!(
        !source_records.is_empty(),
        "the source has delivered model inputs"
    );
    // Attachment ownership is independent of a file-store read during commit.
    let attachment = b"a retained conversation attachment";
    let history_blob = Hash::of(attachment);
    let source_input: Id = rig
        .world
        .scalar(
            "SELECT input_id FROM turns WHERE thread_id=$1 ORDER BY ordinal LIMIT 1",
            source,
        )
        .await;
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        orch_db::queries::storage::register_blob(
            &mut conn,
            &history_blob,
            i64::try_from(attachment.len()).unwrap(),
            rig.world.clock.now(),
        )
        .await
        .unwrap();
        orch_db::queries::storage::input_pins(
            &mut conn,
            source_input,
            &[history_blob],
            rig.world.clock.now(),
        )
        .await
        .unwrap();
    }
    {
        use orch_api_contract::threads::{
            AgentMessagePayload, OutputRef, RawCall, SubagentState, ToolCallPayload, ToolPayload,
        };
        use orch_engine::{
            ingest::{ItemSpec, add_item},
            unit::Unit,
        };
        let turn: Id = sqlx::query_scalar("SELECT id FROM turns WHERE thread_id=$1 AND state='completed' ORDER BY ordinal DESC LIMIT 1")
            .bind(source).fetch_one(rig.world.pool.pool()).await.unwrap();
        let mut tx = rig.world.pool.begin().await.unwrap();
        let mut unit = Unit::new(&mut tx, rig.world.clock.now());
        for name in ["reviewer", "tester"] {
            let stream = format!("sub:{name}");
            let payload = serde_json::to_value(ToolCallPayload {
                tool: ToolPayload::Subagent {
                    child_stream: stream.clone(),
                    name: Some(name.to_owned()),
                    role: Some(name.to_owned()),
                    prompt: Some("Check the redirect".to_owned()),
                    state: SubagentState::Completed,
                    model: None,
                },
                title: format!("Sub-agent {name}"),
                started_at: orch_types::Time::new(rig.world.clock.now()),
                duration_ms: Some(10),
                raw: RawCall {
                    name: "Task".to_owned(),
                    input: OutputRef::of("Check the redirect"),
                    output: Some(OutputRef::of("Checked")),
                    is_error: false,
                },
            })
            .unwrap();
            let parent_ref = format!("native-{source_segment}-task-{name}");
            let parent = add_item(
                &mut unit,
                &ItemSpec {
                    thread: source,
                    segment: source_segment,
                    turn: Some(turn),
                    stream: "main",
                    parent: None,
                    kind: "tool_call",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: Some(&parent_ref),
                },
            )
            .await
            .unwrap()
            .unwrap();
            let words = format!("The {name} checked the redirect");
            let payload = serde_json::to_value(AgentMessagePayload {
                text_len: i64::try_from(words.chars().count()).unwrap(),
                text: words,
                phase: None,
            })
            .unwrap();
            let child_ref = format!("native-{source_segment}-answer-{name}");
            add_item(
                &mut unit,
                &ItemSpec {
                    thread: source,
                    segment: source_segment,
                    turn: Some(turn),
                    stream: &stream,
                    parent: Some(parent.id),
                    kind: "agent_message",
                    status: "completed",
                    author: None,
                    payload: &payload,
                    native_ref: Some(&child_ref),
                },
            )
            .await
            .unwrap();
        }
        unit.finish().await.unwrap();
        tx.commit().await.unwrap();
    }
    let children: Vec<(Id, String, Option<Id>, Json)> = sqlx::query_as("SELECT id,stream,parent_item_id,payload FROM items WHERE thread_id=$1 AND stream LIKE 'sub:%' ORDER BY id")
        .bind(source).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert_eq!(children.len(), 2);
    let bo = rig.world.person("bob").await;
    rig.world.give(bo, "developer").await;
    let plan = rig.plan_for(bo, source, &rig.fork()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::NativeFork);
    assert_eq!(plan.owner.before, None, "a fork has no owner before it");
    assert_eq!(plan.owner.after.id, bo, "the fork is its maker's");
    assert!(plan.locks.is_empty(), "a fork locks nothing");
    assert!(plan.leaves.is_none());
    let job = rig.confirm_for(bo, source, &plan).await;
    let thread = rig.new_thread_of(&job);
    // The source is never held.
    assert!(
        orch_engine::locks::lock_of(&mut rig.world.pool.acquire().await.unwrap(), source)
            .await
            .unwrap()
            .is_none()
    );
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(
        rig.job(job.id).await.result,
        Some(json!({ "thread_id": thread })),
        "the job's result is the thread it made"
    );
    // The new thread is its maker's alone, begun with the history through the last completed turn.
    let (owner, name): (Id, String) =
        sqlx::query_as("SELECT n.owner_id, n.name FROM nodes n WHERE n.id = $1")
            .bind(thread)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert_eq!((owner, name.as_str()), (bo, "Trial"));
    let segment = rig.world.segment_of(thread).await;
    let fidelity: Option<String> = rig
        .world
        .scalar(
            "SELECT import_fidelity FROM segments WHERE id = $1",
            segment,
        )
        .await;
    assert_eq!(fidelity.as_deref(), Some("lossless"));
    let fork_open = rig
        .opens()
        .into_iter()
        .find(|open| open.mode == SessionMode::Fork)
        .expect("the harness forked the session");
    assert_eq!(fork_open.thread_id, thread);
    assert_eq!(
        fork_open.segment_id, segment,
        "the harness and model-input records must name the created fork segment"
    );
    assert_eq!(
        fork_open.native_session_ref,
        Some(format!("native-{source_segment}"))
    );
    assert_eq!(fork_open.upto_turn_ordinal, Some(1));
    let records: Vec<(String, Json, Option<Id>, Option<Id>)> = sqlx::query_as("SELECT purpose,blocks,segment_id,item_id FROM model_inputs WHERE node_id=$1 ORDER BY seq_created")
        .bind(thread).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert_eq!(
        records
            .iter()
            .map(|(purpose, _, _, _)| purpose.as_str())
            .collect::<Vec<_>>(),
        ["session", "tool"]
    );
    assert_eq!(
        records[0]
            .1
            .as_array()
            .unwrap()
            .iter()
            .map(|block| block["text"].as_str().unwrap())
            .collect::<Vec<_>>()
            .join("\n\n"),
        fork_open.instructions
    );
    assert!(
        records
            .iter()
            .all(|(_, _, kept_segment, _)| *kept_segment == Some(segment))
    );
    assert!(
        records[0].3.is_some(),
        "a fork's actual opening has a readable context marker too"
    );
    // This is a read of the ready fork before its first send, not a later native replay.
    assert!(rig.world.turn_starts(thread).is_empty());
    let visible = rig.world.sync(thread).await;
    assert!(visible.items.iter().any(|item| matches!(&item.payload,
        orch_api_contract::threads::ItemPayload::UserMessage(message) if message.parts.iter().any(|part|
            matches!(part, orch_api_contract::threads::ContentPart::Text { text } if text == "Plan the work")))));
    assert!(visible.items.iter().any(|item| matches!(&item.payload,
        orch_api_contract::threads::ItemPayload::AgentMessage(message) if message.text == "The plan is ready")));
    assert_eq!(visible.turns.len(), 1);
    let pins: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(history_blob.as_bytes().as_slice())
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        pins, 2,
        "the source and fork independently retain the attachment"
    );
    assert!(
        visible.turns[0].before_snapshot_id.is_none(),
        "a conversation copy cannot restore the source's files"
    );
    assert!(visible.inputs.iter().all(|input| !matches!(
        input.state,
        orch_api_contract::threads::InputState::Pending
            | orch_api_contract::threads::InputState::Delivering
    )));
    let fork_children: Vec<(Id, Id, String, Json)> = sqlx::query_as(
        "SELECT child.id,parent.id,child.stream,child.payload FROM items child
          JOIN items parent ON parent.id=child.parent_item_id AND parent.thread_id=child.thread_id
          WHERE child.thread_id=$1 AND child.stream LIKE 'sub:%' ORDER BY child.stream",
    )
    .bind(thread)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        fork_children.len(),
        2,
        "both independent child lanes are already readable"
    );
    for (child, parent, stream, payload) in &fork_children {
        assert!(
            !children
                .iter()
                .any(|(original, _, original_parent, _)| original == child
                    || original_parent.as_ref() == Some(parent))
        );
        assert!(
            children
                .iter()
                .any(
                    |(_, original_stream, _, original_payload)| original_stream == stream
                        && original_payload == payload
                )
        );
    }
    let before: i64 = rig
        .world
        .count("SELECT count(*) FROM items WHERE thread_id=$1", thread)
        .await;
    let mut tx = rig.world.pool.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, rig.world.clock.now());
    orch_engine::fork_history::copy_conversation(
        &mut unit,
        source,
        thread,
        segment,
        Some((1, 1)),
        Some(source_segment),
    )
    .await
    .unwrap();
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM items WHERE thread_id=$1", thread)
            .await,
        before,
        "repeating the commit copy adds nothing"
    );
    let pins_after: i32 = sqlx::query_scalar("SELECT pin_count FROM blobs WHERE hash=$1")
        .bind(history_blob.as_bytes().as_slice())
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        pins_after, pins,
        "a repeated copy adds no extra attachment owner"
    );
    let native_source: (Id, String) = sqlx::query_as("SELECT id,native_ref FROM items WHERE thread_id=$1 AND kind='agent_message' AND native_ref IS NOT NULL ORDER BY seq_created LIMIT 1")
        .bind(source).fetch_one(rig.world.pool.pool()).await.unwrap();
    let copied = orch_db::queries::threads::item_by_native_ref(
        &mut rig.world.pool.acquire().await.unwrap(),
        segment,
        &native_source.1,
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(copied.id, native_source.0);
    assert_eq!(copied.thread_id, thread);
    assert_ne!(
        copied.segment_id, segment,
        "the copied history keeps its own ended segment"
    );
    let mut tx = rig.world.pool.begin().await.unwrap();
    let mut unit = orch_engine::unit::Unit::new(&mut tx, rig.world.clock.now());
    let replay = orch_engine::ingest::add_item(
        &mut unit,
        &orch_engine::ingest::ItemSpec {
            thread,
            segment,
            turn: None,
            stream: "main",
            parent: None,
            kind: &copied.kind,
            status: &copied.status,
            author: copied.author_id,
            payload: &copied.payload,
            native_ref: Some(&native_source.1),
        },
    )
    .await
    .unwrap();
    assert!(
        replay.is_none(),
        "a session replay finds the copy instead of writing a duplicate"
    );
    unit.finish().await.unwrap();
    tx.commit().await.unwrap();
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM items WHERE thread_id=$1", thread)
            .await,
        before
    );
    // Both threads say so in their histories.
    for (who, marker) in [(thread, "forked_from"), (source, "forked")] {
        assert_eq!(
            rig.world
                .count(
                    &format!(
                        "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload ->> 'marker' = '{marker}'"
                    ),
                    who
                )
                .await,
            1,
            "{marker}"
        );
    }
    // The source did not change, and the new thread is ready to be used.
    assert_eq!(rig.world.segment_of(source).await, source_segment);
    let after_records: Vec<(Id, Option<Id>, Json)> = sqlx::query_as(
        "SELECT id,segment_id,blocks FROM model_inputs WHERE node_id=$1 ORDER BY id",
    )
    .bind(source)
    .fetch_all(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        after_records, source_records,
        "the fork does not rebind its source's actual inputs"
    );
    let after_children: Vec<(Id, String, Option<Id>, Json)> = sqlx::query_as("SELECT id,stream,parent_item_id,payload FROM items WHERE thread_id=$1 AND stream LIKE 'sub:%' ORDER BY id")
        .bind(source).fetch_all(rig.world.pool.pool()).await.unwrap();
    assert_eq!(
        after_children, children,
        "sub-agent history and parent links stay intact"
    );
    rig.world.wait_state(source, "idle").await;
    rig.world.wait_state(thread, "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_ready_fork_shows_the_boundary_actually_carried_while_its_source_goes_on_and_its_next_answer_is_new()
 {
    let can = Can {
        export_native: true,
        import_native: true,
        fork_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let source = rig.thread_with_history("Source continues").await;
    rig.say(source, "A later source input").await;
    let running = rig.world.turn_start(source, 1).await;
    rig.world
        .say(source, common::turn_began(running.turn_id))
        .await;
    rig.world.wait_state(source, "running").await;
    let plan = rig.plan(source, &rig.fork()).await;
    let already_opened = rig.world.began(CommandKind::SessionOpenFenced);
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let job = rig.confirm(source, &plan).await;
    let fork = rig.new_thread_of(&job);
    rig.world
        .wait_began(CommandKind::SessionOpenFenced, already_opened + 1)
        .await;
    // Its session was already open before the held fork: finishing it needs no second open.
    rig.world
        .answer(
            source,
            &running,
            "later-source-answer",
            "Only the source knows this later answer",
        )
        .await;
    rig.world
        .say(
            source,
            common::turn_ended(running.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig.world.wait_turn(running.turn_id, "completed").await;
    rig.world.machine.handler.release();
    rig.wait_job(job.id, "succeeded").await;
    let carried = rig.world.sync(fork).await;
    let words = common::said(&carried).join("\n");
    assert!(
        words.contains("Plan the work") && words.contains("The plan is ready"),
        "{words}"
    );
    assert!(
        !words.contains("Only the source knows"),
        "the source's later turn was not in the opened session: {words}"
    );
    assert_eq!(carried.turns.len(), 1);
    assert!(
        rig.world.turn_starts(fork).is_empty(),
        "no send is needed to see the carried conversation"
    );
    let final_job = rig.job(job.id).await;
    assert_eq!(final_job.plan["carried_history_range"], json!([1, 1]));
    rig.say(fork, "Try the new branch").await;
    let started = rig.world.turn_start(fork, 0).await;
    rig.world
        .answer(fork, &started, "new-in-the-fork", "The new branch works")
        .await;
    rig.world
        .say(
            fork,
            common::turn_ended(started.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig.world.wait_turn(started.turn_id, "completed").await;
    let after = rig.world.sync(fork).await;
    let words = common::said(&after).join("\n");
    assert!(
        words.contains("The plan is ready") && words.contains("The new branch works"),
        "a new answer cannot replace the copied one: {words}"
    );
    assert_eq!(
        after
            .turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert!(
        !common::said(&rig.world.sync(source).await)
            .join("\n")
            .contains("The new branch works")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_starts_with_the_rows_that_were_changed_on_the_page_of_its_source_unless_it_asks_for_the_places_defaults()
 {
    use orch_db::queries::access::{NewEntry, PageKey, RowWrite, insert_entry, write_row};
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let source = rig.world.thread("Shared page").await;
    let reader = rig.world.person("reader").await;
    // The source's page names a reader for the agent's work, and its owner is its maker.
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let page = PageKey::new("node", source, "live");
        write_row(
            &mut conn,
            &RowWrite {
                page: &page,
                profile: "any",
                permission: "thread.interact",
                inherit: false,
                locked: false,
            },
            Some(rig.world.ada),
            chrono::Utc::now(),
        )
        .await
        .unwrap();
        insert_entry(
            &mut conn,
            &NewEntry {
                id: new_id(),
                page: &page,
                profile: "any",
                permission: "thread.interact",
                holder_kind: "user",
                holder_id: Some(reader),
                holder_role: None,
                options: &json!({}),
                expires_at: None,
                note: "to talk to the agent",
                source: "page",
                created_by: Some(rig.world.ada),
                at: chrono::Utc::now(),
            },
        )
        .await
        .unwrap();
    }
    let entries = |thread: Id, permission: &'static str| {
        let (pool, permission) = (rig.world.pool.pool().clone(), permission);
        async move {
            sqlx::query_scalar::<_, i64>(
                "SELECT count(*) FROM permission_entries
                  WHERE object_kind = 'node' AND object_id = $1 AND page = 'live' AND permission = $2
                    AND removed_at IS NULL",
            )
            .bind(thread)
            .bind(permission)
            .fetch_one(&pool)
            .await
            .unwrap()
        }
    };
    // A request that names no page gets the page of the source: its changed rows, and its maker alone as owner.
    let plan = rig.plan(source, &rig.fork()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let job = rig.confirm(source, &plan).await;
    let carried = rig.new_thread_of(&job);
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(
        entries(carried, "thread.interact").await,
        1,
        "the reader came along"
    );
    assert_eq!(
        entries(carried, "access.own").await,
        1,
        "and the owners did not"
    );
    // One that asks for the defaults of the place gets no row of its own.
    let request = ForkRequest {
        access: Some(orch_api_contract::access::ThreadAccess::Preset(
            orch_api_contract::access::AccessPreset::Defaults,
        )),
        ..rig.fork()
    };
    let plan = rig.plan(source, &request).await;
    let job = rig.confirm(source, &plan).await;
    let plain = rig.new_thread_of(&job);
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(entries(plain, "thread.interact").await, 0);
    assert_eq!(entries(plain, "access.own").await, 1);
    // The source is as it was.
    assert_eq!(entries(source, "thread.interact").await, 1);
}

/// A fork whose result goes to an environment that is made for it, with these files chosen. The unit the
/// environment is made on answers a write of files as `answer` says; the commands it was given are kept.
async fn fork_into_a_new_environment(
    rig: &Rig,
    host: &hosting::ScriptedHost,
    template: Id,
    source: Id,
    name: &str,
    answer: Result<(), CommandError>,
) -> (JobRow, Arc<Mutex<Vec<Command>>>, ForkPlan, hosting::Unit) {
    use orch_api_contract::forks::{FileSelection, ForkEnvironment, ForkNewEnvironment};
    let request = ForkRequest {
        to: Some(ForkTo {
            provider_id: Some(rig.other_provider),
            agent_id: Some(rig.other_agent),
            ..ForkTo::default()
        }),
        environment: Some(ForkEnvironment::New {
            new: ForkNewEnvironment {
                name: Some(name.to_owned()),
                template_id: Some(template),
                files: Some(FileSelection::Selection {
                    include: vec!["src/**".to_owned()],
                    exclude: None,
                }),
                ..ForkNewEnvironment::default()
            },
        }),
        ..rig.fork()
    };
    let plan = rig.plan(source, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let began = host.count(CommandKind::NodeRuntimeStart);
    let job = rig.confirm(source, &plan).await;
    host.wait_for(CommandKind::NodeRuntimeStart, began + 1)
        .await;
    let runtime = host.started().unwrap();
    // The connector of the unit comes up able to write files, and is told what became of it.
    let unit = hosting::connector(&rig.world, "unit", &["environment"]).await;
    let seen: Arc<Mutex<Vec<Command>>> = Arc::new(Mutex::new(Vec::new()));
    for kind in [CommandKind::FsMaterialize, CommandKind::FsRemove] {
        let (kept, answer) = (Arc::clone(&seen), answer.clone());
        unit.added.machine.handler.on(kind, move |command| {
            kept.lock().unwrap().push(command.clone());
            match command {
                Command::FsMaterialize(_) => answer.clone().map(|()| {
                    CommandResult::FsMaterialize(
                        orch_link_contract::commands::FsMaterializeResult {
                            files: 3,
                            bytes: 30,
                            skipped: 0,
                        },
                    )
                }),
                _ => Ok(CommandResult::FsRemove(Empty {})),
            }
        });
    }
    let mut conn = rig.world.pool.acquire().await.unwrap();
    orch_db::queries::resources::hosting::adopt_machine(
        &mut conn,
        unit.id,
        runtime,
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    drop(conn);
    (job, seen, plan, unit)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_into_a_new_environment_makes_it_for_its_maker_and_writes_the_files_chosen_there_and_a_failed_one_takes_it_away()
 {
    selected_files_into_a_new_environment(false, false).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_registered_root_alias_forks_its_selected_files_from_cached_and_live_listings() {
    for live in [false, true] {
        selected_files_into_a_new_environment(true, live).await;
    }
}

async fn selected_files_into_a_new_environment(alias: bool, live: bool) {
    // The scripted machine's physical root is independent of registration and
    // of the controller's root-selection helper, as the real connector reports.
    let rig = Rig::with_source_root(
        Can::default(),
        "fake",
        Can::default(),
        alias.then_some("/workspace"),
    )
    .await;
    if alias {
        sqlx::query(
            "UPDATE environments SET workspace_root='/registered-link',
                    facts=facts || jsonb_build_object('workspace_root','/workspace',
                        'generation',generation,'index',$2::text) WHERE id=$1",
        )
        .bind(rig.world.environment)
        .bind(if live { "live" } else { "full" })
        .execute(rig.world.pool.pool())
        .await
        .unwrap();
    }
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "host-1", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let source = rig.thread_with_history("Moved").await;

    // The environment is made, the files chosen are listed on the source and written on the unit.
    let (job, unit_saw, plan, unit) =
        fork_into_a_new_environment(&rig, &host, template, source, "Fresh", Ok(())).await;
    let about_the_unit = || {
        format!(
            "the unit was given {:?} and began {} attaches",
            unit.kinds(),
            unit.began(CommandKind::EnvAttach)
        )
    };
    let files = plan
        .files
        .as_ref()
        .expect("files travel to an environment of their own");
    assert_eq!(files.path_map.to, "/workspace/web");
    let thread = rig.new_thread_of(&job);
    let done = rig.wait_job_over(job.id, about_the_unit).await;
    assert_eq!(done.state, "succeeded", "{:?}", done.error);
    assert_eq!(files.path_map.from, "/workspace/web");
    if live {
        assert_eq!((files.file_count, files.total_bytes), (3, 30));
        assert!(rig.files.lock().unwrap().iter().any(|command| {
            matches!(command, Command::FsManifest(body)
                if body.selection.include == ["web/src/**"])
        }));
    }
    if alias {
        let registered: String = rig
            .world
            .scalar(
                "SELECT workspace_root FROM environments WHERE id=$1",
                rig.world.environment,
            )
            .await;
        assert_eq!(registered, "/registered-link");
    }
    let (environment, owner, placement): (Id, Id, String) = sqlx::query_as(
        "SELECT e.id, e.owner_id, e.placement FROM threads t JOIN environments e ON e.id = t.environment_id
          WHERE t.node_id = $1",
    )
    .bind(thread)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_ne!(environment, rig.world.environment);
    assert_eq!(
        (owner, placement.as_str()),
        (rig.world.ada, "managed"),
        "the environment is its maker's"
    );
    let snapshots: Vec<_> = rig
        .files
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            Command::FsSnapshot(snapshot) => Some(snapshot.selection.include.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        snapshots[0],
        ["web/src/**"],
        "only what was chosen was listed"
    );
    let written: Vec<_> = unit_saw
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            Command::FsMaterialize(write) => {
                Some((write.environment_id, write.target_root.clone()))
            }
            _ => None,
        })
        .collect();
    // The files are written to the directory of the fork, and the hand-over of the history, which these
    // harnesses cannot read otherwise, below it: everything goes to the environment that was made.
    assert!(
        written.iter().any(|(_, root)| root == "/workspace/web"),
        "{written:?}"
    );
    assert!(
        written
            .iter()
            .all(|(on, root)| *on == environment && root.starts_with("/workspace/web")),
        "{written:?}"
    );
    let copied: Vec<_> = unit_saw
        .lock()
        .unwrap()
        .iter()
        .filter_map(|command| match command {
            Command::FsMaterialize(write) if write.target_root == "/workspace/web" => {
                Some(write.manifest_blob)
            }
            _ => None,
        })
        .collect();
    assert!(!copied.is_empty());
    for hash in copied {
        let manifest = rig.world.engine().read_manifest(&hash).await.unwrap();
        assert_eq!(manifest.header().root, "/workspace/web");
        assert_eq!(
            manifest
                .entries()
                .iter()
                .map(|entry| (entry.path.as_str(), entry.size, entry.hash))
                .collect::<Vec<_>>(),
            [
                ("src/a.txt", 10, Some(Hash::of(b"source-a0\n"))),
                ("src/b.txt", 10, Some(Hash::of(b"source-b0\n"))),
                ("src/c.txt", 10, Some(Hash::of(b"source-c0\n"))),
            ],
            "the selected bytes retain their paths below the thread directory"
        );
    }
    let source_environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id = $1",
            source,
        )
        .await;
    assert_eq!(
        source_environment, rig.world.environment,
        "the source did not move"
    );

    // A write that is refused fails the fork; the thread, the files and the environment it made are taken away.
    let (job, unit_saw, _, doomed) = fork_into_a_new_environment(
        &rig,
        &host,
        template,
        source,
        "Doomed",
        Err(CommandError::new(ErrorCode::Internal, "the disk is full")),
    )
    .await;
    let thread = rig.new_thread_of(&job);
    let failed = rig
        .wait_job_over(job.id, || {
            format!("the unit was given {:?}", doomed.kinds())
        })
        .await;
    assert_eq!(failed.state, "failed", "{:?}", failed.error);
    assert_eq!(failed.error.as_ref().unwrap()["phase"], json!("copy"));
    assert_eq!(failed.error.as_ref().unwrap()["undone"], json!(true));
    let deleted: Option<chrono::DateTime<chrono::Utc>> = rig
        .world
        .scalar("SELECT deleted_at FROM nodes WHERE id = $1", thread)
        .await;
    assert!(
        deleted.is_some(),
        "the thread that was made is deleted again"
    );
    assert!(
        unit_saw
            .lock()
            .unwrap()
            .iter()
            .any(|command| matches!(command, Command::FsRemove(_))),
        "the directory that was written is removed from the unit"
    );
    let scheduled: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT remove_at FROM environments WHERE name = 'Doomed'")
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert!(
        scheduled.is_some(),
        "the environment it made is scheduled for removal"
    );
}

impl Rig {
    /// A second environment on the world's machine, running.
    async fn second_environment(&self) -> Id {
        let second = new_id();
        sqlx::query(
            "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                       template_id, generation, status)
             SELECT $1, 'Second', owner_id, org_id, 'managed', home_node_id, '/second', machine_id, template_id, 1, 'running'
               FROM environments WHERE id = $2",
        )
        .bind(second)
        .bind(self.world.environment)
        .execute(self.world.pool.pool())
        .await
        .unwrap();
        second
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_switch_to_another_environment_keeps_the_session_moves_the_files_and_schedules_the_empty_one_it_left_for_removal()
 {
    use orch_api_contract::forks::{ForkEnvironment, LeftAction};
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    // World has a raw Running environment, but its placeholder template has no
    // offering host. Declare this case's actual unit recipe/host before opening
    // its conversation; placement or an unused template ID cannot imply one.
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        assert!(
            !orch_db::queries::environment_facts::has_unit(&mut conn, rig.world.environment)
                .await
                .unwrap()
        );
    }
    let host = hosting::ScriptedHost::on(&rig.world, "Source unit host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    {
        let mut conn = rig.world.pool.acquire().await.unwrap();
        let changed = sqlx::query(
            "UPDATE environments SET template_id = $2,
                    config = config || jsonb_build_object('env.host_id', $3::text)
              WHERE id = $1 AND deleted_at IS NULL",
        )
        .bind(rig.world.environment)
        .bind(template)
        .bind(host.id.to_string())
        .execute(&mut *conn)
        .await
        .unwrap();
        assert_eq!(changed.rows_affected(), 1);
        let services = orch_db::queries::environment_facts::of(&mut conn, rig.world.environment)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(services.machine_id, Some(host.machine));
        assert!(services.makes_units());
        assert!(
            orch_db::queries::environment_facts::has_unit(&mut conn, rig.world.environment)
                .await
                .unwrap()
        );
    }
    // This repairs declared fixture topology, not a claim that a provisioning
    // job ran. The source connector, files, generation and session stay owned.
    let thread = rig.thread_with_history("Moving").await;
    let old_segment = rig.world.segment_of(thread).await;
    let second = rig.second_environment().await;
    let request = ForkRequest {
        to: None,
        environment: Some(ForkEnvironment::Existing {
            environment_id: second,
            files: None,
        }),
        ..rig.to_other()
    };
    let plan = rig.plan(thread, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(
        plan.history.method,
        HistoryMethod::SameSession,
        "the harness stays where it is: only the environment changes"
    );
    let left = plan
        .leaves
        .as_ref()
        .and_then(|leaves| leaves.environment.as_ref());
    assert_eq!(
        left.map(|left| (left.environment.id, left.action)),
        Some((rig.world.environment, LeftAction::Remove)),
        "no other thread is in the environment it leaves"
    );
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(environment, second);
    let writes = rig
        .files
        .lock()
        .unwrap()
        .iter()
        .filter(|command| {
            matches!(command, Command::FsMaterialize(write)
                if write.purpose == orch_link_contract::commands::MaterializePurpose::Move
                    && write.target_root == "/second/web")
        })
        .count();
    assert_eq!(
        writes, 2,
        "the files were written once, and once more for what changed meanwhile"
    );
    // The old environment is out of use and is scheduled for removal; the new segment goes on with the
    // session of the old.
    let removal: Option<chrono::DateTime<chrono::Utc>> = rig
        .world
        .scalar(
            "SELECT remove_at FROM environments WHERE id = $1",
            rig.world.environment,
        )
        .await;
    assert!(removal.is_some());
    let new_segment = rig.world.segment_of(thread).await;
    assert_ne!(new_segment, old_segment);
    let old_ref: Option<String> = rig
        .world
        .scalar(
            "SELECT native_session_ref FROM segments WHERE id = $1",
            old_segment,
        )
        .await;
    let resumed = rig
        .opens()
        .into_iter()
        .find(|open| open.segment_id == new_segment)
        .expect("the new side opened the session");
    assert_eq!(
        (resumed.mode, resumed.native_session_ref),
        (SessionMode::Resume, old_ref),
        "the harness is asked to go on with the session it has"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_that_takes_no_history_begins_fresh_and_one_that_is_not_allowed_says_what_is_lacking()
 {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let source = rig.thread_with_history("Fresh").await;
    let request = ForkRequest {
        history: Some(orch_api_contract::forks::ForkHistory {
            mode: orch_api_contract::forks::HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        }),
        ..rig.fork()
    };
    let plan = rig.plan(source, &request).await;
    assert_eq!(plan.history.method, HistoryMethod::Nothing);
    assert_eq!(plan.history.turns, 0);
    let job = rig.confirm(source, &plan).await;
    let thread = rig.new_thread_of(&job);
    rig.wait_job(job.id, "succeeded").await;
    let segment = rig.world.segment_of(thread).await;
    let fidelity: Option<String> = rig
        .world
        .scalar(
            "SELECT import_fidelity FROM segments WHERE id = $1",
            segment,
        )
        .await;
    assert_eq!(fidelity, None, "a fresh session imported nothing");
    assert!(
        rig.opens().iter().all(|open| open.thread_id != thread),
        "no session is opened until the first turn"
    );
    assert!(
        rig.world.sync(thread).await.turns.is_empty(),
        "Nothing copies no conversation turns"
    );
    // Somebody who may only look cannot start a thread in the topic: the plan says what is lacking.
    let viewer = rig.world.person("viewer").await;
    rig.world.give(viewer, "viewer").await;
    let caller = Caller::session(viewer);
    let draft = rig
        .world
        .engine()
        .draft_fork(&caller, source, &rig.fork())
        .await
        .unwrap();
    assert!(!draft.is_clear());
    assert!(
        draft
            .plan
            .blockers
            .iter()
            .any(|blocker| blocker.code == "access"),
        "{:?}",
        draft.plan.blockers
    );
    assert!(!draft.lacks.is_empty(), "what is lacking is named");
    // Somebody who cannot see the thread is told there is none.
    let stranger = rig.world.person("stranger").await;
    let none = rig
        .world
        .engine()
        .draft_fork(&Caller::session(stranger), source, &rig.fork())
        .await;
    assert!(matches!(none, Err(PlanError::NotFound)), "{none:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_of_a_range_of_turns_travels_as_items_and_says_which_turns_were_left_out() {
    let can = Can {
        import_neutral: true,
        fork_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let source = rig.thread_with_history("Ranged").await;
    rig.say(source, "And then?").await;
    rig.world.run_turn(source, 1, "Then this").await;
    rig.world.wait_state(source, "idle").await;
    let request = ForkRequest {
        history: Some(orch_api_contract::forks::ForkHistory {
            mode: orch_api_contract::forks::HistoryMode::Range,
            from_turn: Some(2),
            to_turn: Some(2),
        }),
        ..rig.fork()
    };
    let plan = rig.plan(source, &request).await;
    assert_eq!(plan.history.turns, 1);
    assert_eq!(
        plan.history.method,
        HistoryMethod::NeutralImport,
        "a history that does not begin at the first turn cannot be forked by the harness"
    );
    assert!(
        plan.history
            .report
            .warnings
            .iter()
            .any(|warning| warning.contains("left out"))
    );
    let job = rig.confirm(source, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let import = rig
        .opens()
        .into_iter()
        .find(|open| open.mode == SessionMode::ImportNeutral)
        .expect("the history was imported as items");
    let text = rig.blob_text(&import.history.unwrap().blob).await;
    assert!(
        text.lines().next().unwrap().contains("begins at turn 2"),
        "{text}"
    );
    assert!(
        text.contains("Then this") && !text.contains("The plan is ready"),
        "{text}"
    );
    let fork = rig.world.sync(rig.new_thread_of(&job)).await;
    let words = common::said(&fork).join("\n");
    assert!(
        words.contains("Then this") && !words.contains("The plan is ready"),
        "the ready fork shows exactly its selected conversation: {words}"
    );
    assert_eq!(
        fork.turns
            .iter()
            .map(|turn| turn.ordinal)
            .collect::<Vec<_>>(),
        [2]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_fork_into_another_environment_copies_the_files_twice_and_a_failed_one_deletes_what_it_made()
 {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let source = rig.thread_with_history("Copied").await;
    // A second environment on the same machine.
    let second = new_id();
    sqlx::query(
        "INSERT INTO environments (id, name, owner_id, org_id, placement, home_node_id, workspace_root, machine_id,
                                   template_id, generation, status)
         SELECT $1, 'Second', owner_id, org_id, 'managed', home_node_id, '/second', machine_id, template_id, 1, 'running'
           FROM environments WHERE id = $2",
    )
    .bind(second)
    .bind(rig.world.environment)
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    let request = ForkRequest {
        environment: Some(orch_api_contract::forks::ForkEnvironment::Existing {
            environment_id: second,
            files: None,
        }),
        ..rig.fork()
    };
    let plan = rig.plan(source, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert!(plan.files.is_some(), "files go to another environment");
    let job = rig.confirm(source, &plan).await;
    let thread = rig.new_thread_of(&job);
    rig.wait_job(job.id, "succeeded").await;
    let seen = rig.files.lock().unwrap().clone();
    let snapshots: Vec<_> = seen
        .iter()
        .filter_map(|command| match command {
            Command::FsSnapshot(snapshot) => Some(snapshot.clone()),
            _ => None,
        })
        .collect();
    let writes: Vec<_> = seen
        .iter()
        .filter_map(|command| match command {
            Command::FsMaterialize(write)
                if write.purpose == orch_link_contract::commands::MaterializePurpose::Move =>
            {
                Some(write.clone())
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        (snapshots.len(), writes.len()),
        (2, 2),
        "once, and once more for what changed meanwhile"
    );
    assert_eq!(snapshots[0].selection.include, ["web"]);
    assert_eq!(snapshots[0].environment_id, rig.world.environment);
    assert_eq!(writes[0].target_root, "/second/web");
    assert_eq!(writes[0].environment_id, second);
    assert_eq!(
        writes[0].purpose,
        orch_link_contract::commands::MaterializePurpose::Move
    );
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id = $1",
            thread,
        )
        .await;
    assert_eq!(
        environment, second,
        "the new thread works in the other environment"
    );
    let source_environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id = $1",
            source,
        )
        .await;
    assert_eq!(source_environment, rig.world.environment);

    // A fork whose harness refuses the history leaves no thread behind and nothing in the source.
    let plan = rig.plan(source, &request).await;
    let history_refused = vec![
        CommandError::new(ErrorCode::Internal, "no"),
        CommandError::new(ErrorCode::Internal, "no again"),
    ];
    rig.refuse_preparations(history_refused);
    let job = rig.confirm(source, &plan).await;
    let thread = rig.new_thread_of(&job);
    let row = loop {
        let row = rig.job(job.id).await;
        if !row.is_open() {
            break row;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    };
    // The harness in this world cannot fork, so the history is handed over as a transcript, which the
    // refusals then make impossible.
    assert_eq!(row.state, "failed", "{:?}", row.error);
    assert_eq!(row.error.as_ref().unwrap()["undone"], json!(true));
    let deleted: Option<chrono::DateTime<chrono::Utc>> = rig
        .world
        .scalar("SELECT deleted_at FROM nodes WHERE id = $1", thread)
        .await;
    assert!(
        deleted.is_some(),
        "the thread that was made is deleted again"
    );
    assert!(
        writes
            .iter()
            .all(|write| write.target_root == "/second/web"),
        "only the thread's directory was written"
    );
}

// ---------------------------------------------------------------- side threads

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_side_thread_runs_on_what_its_parent_runs_on_and_starts_from_its_history() {
    let can = Can {
        fork_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    let parent = rig.thread_with_history("Parent").await;
    let parent_segment = rig.world.segment_of(parent).await;
    let started = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "A quick question".to_owned(),
            }],
            "aside-1".to_owned(),
            None,
        )
        .await
        .unwrap();
    let side = started.thread;
    let (side_of, from_turn, name, node_parent): (Option<Id>, Option<i32>, String, Option<Id>) =
        sqlx::query_as(
            "SELECT t.side_of, t.side_from_turn, n.name, n.parent_id FROM threads t JOIN nodes n ON n.id = t.node_id
              WHERE t.node_id = $1",
        )
        .bind(side)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!((side_of, from_turn), (Some(parent), Some(1)));
    assert_eq!(name, "Parent (side)");
    assert_eq!(
        node_parent,
        Some(parent),
        "it lies in the thread it was taken from"
    );
    let (agent, provider, environment): (Id, Id, Id) = sqlx::query_as(
        "SELECT agent_id, provider_id, environment_id FROM threads WHERE node_id = $1",
    )
    .bind(side)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(
        (agent, provider, environment),
        (rig.world.agent, rig.world.provider, rig.world.environment)
    );
    assert_eq!(
        rig.world
            .count("SELECT count(*) FROM permission_entries WHERE object_kind = 'node' AND object_id = $1", side)
            .await,
        0,
        "a side thread has no page of its own"
    );
    // Its first turn forks the session of its parent through the turn it was taken at.
    let start = rig.world.run_turn(side, 0, "The aside is answered").await;
    assert!(start.text.contains("A quick question"));
    assert!(
        common::words_after_where(&start.text)
            .starts_with("<orch>\nThis is a side conversation, taken from the main one"),
        "an aside's first input says what it is and that it only reads: {}",
        start.text
    );
    // What was said in it is not indexed for a search, as what was said in its parent is.
    assert!(
        rig.world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind IN ('user_message', 'agent_message')",
                side
            )
            .await
            >= 2,
        "the aside was said and answered"
    );
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND search_text IS NOT NULL",
                side
            )
            .await,
        0
    );
    assert!(
        rig.world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND search_text IS NOT NULL",
                parent
            )
            .await
            > 0
    );
    let open = rig
        .opens()
        .into_iter()
        .find(|open| open.thread_id == side)
        .expect("a session was opened for the side thread");
    assert_eq!(open.mode, SessionMode::Fork);
    assert_eq!(
        open.native_session_ref,
        Some(format!("native-{parent_segment}"))
    );
    assert_eq!(open.upto_turn_ordinal, Some(1));
    // The parent has one line that says where it was started, and nothing of what was said.
    assert_eq!(
        rig.world
            .count(
                "SELECT count(*) FROM items WHERE thread_id = $1 AND kind = 'marker' AND payload ->> 'marker' = 'side_started'",
                parent
            )
            .await,
        1
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn only_as_many_side_threads_are_open_as_the_setting_allows_and_none_where_they_are_off() {
    // The parent's harness takes in a history: an aside about a conversation begins with it.
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Can::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Limited").await;
    let say = |text: &str, client: &str| {
        (
            vec![orch_api_contract::threads::ContentPart::Text {
                text: text.to_owned(),
            }],
            client.to_owned(),
        )
    };
    rig.world
        .set(rig.world.topic, "thread.side_max", json!(1))
        .await;
    let (content, client) = say("One", "a");
    let first = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content, client, None)
        .await
        .unwrap();
    let (content, client) = say("Two", "b");
    let second = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content, client, None)
        .await;
    match second {
        Err(orch_engine::side_threads::SideError::Limit { open, max, allowed }) => {
            assert_eq!((open, max, allowed), (vec![first.thread], 1, true));
        }
        other => panic!("the limit is reached: {other:?}"),
    }
    // Enqueueing a side input does not synchronously admit a turn. Observe the
    // real first start and its native Started event before closing/counting it.
    let start = rig.world.turn_start(first.thread, 0).await;
    rig.world
        .say(first.thread, common::turn_began(start.turn_id))
        .await;
    rig.world.wait_turn(start.turn_id, "running").await;
    rig.world
        .wait_input(first.submitted.input.id, "delivered")
        .await;
    let owned: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM threads h
            JOIN turns t ON t.thread_id = h.node_id AND t.id = $2
            JOIN thread_inputs i ON i.thread_id = h.node_id AND i.id = t.input_id
            JOIN segments s ON s.thread_id = h.node_id AND s.id = t.segment_id
          WHERE h.node_id = $1 AND i.id = $3 AND i.state = 'delivered' AND i.turn_id = t.id
            AND h.current_turn_id = t.id AND h.current_segment_id = s.id
            AND t.continuation_id = h.active_continuation_id
            AND s.continuation_id = t.continuation_id AND i.continuation_id = t.continuation_id)",
    )
    .bind(first.thread)
    .bind(start.turn_id)
    .bind(first.submitted.input.id)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert!(
        owned,
        "the counted side turn belongs to its actual first delivered input and selected segment"
    );
    assert_eq!(rig.world.turn_starts(first.thread).len(), 1);
    // Closed by hand, it makes room, and its parent has a line that says how many turns it had.
    assert!(
        rig.world
            .engine()
            .close_side_thread(first.thread, "closed", Some(rig.world.ada), None)
            .await
            .unwrap()
    );
    assert!(
        !rig.world
            .engine()
            .close_side_thread(first.thread, "closed", Some(rig.world.ada), None)
            .await
            .unwrap(),
        "a side thread is closed once"
    );
    assert_eq!(
        rig.world.engine().side_threads(parent, true).await.unwrap(),
        [first.thread]
    );
    assert!(
        rig.world
            .engine()
            .side_threads(parent, false)
            .await
            .unwrap()
            .is_empty()
    );
    let closed_line: Json = sqlx::query_scalar(
        "SELECT payload -> 'data' FROM items WHERE thread_id = $1 AND payload ->> 'marker' = 'side_closed'",
    )
    .bind(parent)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(closed_line["why"], json!("closed"));
    assert_eq!(
        closed_line["turns"],
        json!(1),
        "its parent's history says how many turns it had: the one its message began, and not what was said"
    );
    let (content, client) = say("Three", "c");
    rig.world
        .engine()
        .start_side_thread(&rig.caller(), parent, content, client, None)
        .await
        .unwrap();
    rig.world
        .set(rig.world.topic, "thread.side_threads", json!(false))
        .await;
    let (content, client) = say("Four", "d");
    let off = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content, client, None)
        .await;
    assert!(
        matches!(
            off,
            Err(orch_engine::side_threads::SideError::Limit { allowed: false, .. })
        ),
        "{off:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_side_thread_that_stood_idle_for_its_hours_is_closed_and_one_in_use_is_not() {
    // The parent's harness takes in a history: an aside about a conversation begins with it.
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Can::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Idle").await;
    let content = || {
        vec![orch_api_contract::threads::ContentPart::Text {
            text: "Hm".to_owned(),
        }]
    };
    let used = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content(), "u".to_owned(), None)
        .await
        .unwrap()
        .thread;
    rig.world.run_turn(used, 0, "Answer").await;
    rig.world.wait_state(used, "idle").await;
    let forgotten = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content(), "f".to_owned(), None)
        .await
        .unwrap()
        .thread;
    // The second answered, and a day and a bit has gone by since: nothing is under way in it or waits for it.
    rig.world.run_turn(forgotten, 0, "Answer").await;
    rig.world.wait_state(forgotten, "idle").await;
    sqlx::query(
        "UPDATE turns SET completed_at = completed_at - interval '25 hours' WHERE thread_id = $1",
    )
    .bind(forgotten)
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    let closed = rig.world.engine().close_idle_side_threads().await.unwrap();
    assert_eq!(closed, 1);
    assert_eq!(
        rig.world.engine().side_threads(parent, true).await.unwrap(),
        [forgotten]
    );
    assert_eq!(
        rig.world
            .engine()
            .side_threads(parent, false)
            .await
            .unwrap(),
        [used]
    );
    let line: Json = sqlx::query_scalar(
        "SELECT payload -> 'data' FROM items WHERE thread_id = $1 AND payload ->> 'marker' = 'side_closed'",
    )
    .bind(parent)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(line["why"], json!("idle"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_side_thread_is_not_switched_has_no_side_threads_and_is_asked_of_its_parents_page() {
    // The parent's harness takes in a history: an aside about a conversation begins with it.
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Can::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Aside").await;
    let content = || {
        vec![orch_api_contract::threads::ContentPart::Text {
            text: "Hm".to_owned(),
        }]
    };
    let side = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), parent, content(), "s".to_owned(), None)
        .await
        .unwrap()
        .thread;
    let switch = rig
        .world
        .engine()
        .draft_fork(&rig.caller(), side, &rig.to_other())
        .await;
    assert!(matches!(switch, Err(PlanError::Side)), "{switch:?}");
    let again = rig
        .world
        .engine()
        .start_side_thread(&rig.caller(), side, content(), "t".to_owned(), None)
        .await;
    assert!(
        matches!(again, Err(orch_engine::side_threads::SideError::Side)),
        "{again:?}"
    );
    // Only somebody who may speak to the parent may start one; somebody who may not see it is told there is none.
    let viewer = rig.world.person("looker").await;
    rig.world.give(viewer, "viewer").await;
    let refused = rig
        .world
        .engine()
        .start_side_thread(
            &Caller::session(viewer),
            parent,
            content(),
            "v".to_owned(),
            None,
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(orch_engine::side_threads::SideError::Denied(_))
        ),
        "{refused:?}"
    );
    let stranger = rig.world.person("nobody").await;
    let none = rig
        .world
        .engine()
        .start_side_thread(
            &Caller::session(stranger),
            parent,
            content(),
            "w".to_owned(),
            None,
        )
        .await;
    assert!(
        matches!(none, Err(orch_engine::side_threads::SideError::NotFound)),
        "{none:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_side_thread_kept_by_a_fork_is_closed_with_that_reason() {
    // The parent's harness takes in a history: an aside about a conversation begins with it.
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Can::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("Worth keeping").await;
    let side = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Try this".to_owned(),
            }],
            "k".to_owned(),
            None,
        )
        .await
        .unwrap()
        .thread;
    rig.world.run_turn(side, 0, "Done aside").await;
    rig.world.wait_state(side, "idle").await;
    let request = ForkRequest {
        history: Some(orch_api_contract::forks::ForkHistory {
            mode: orch_api_contract::forks::HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        }),
        ..rig.fork()
    };
    let plan = rig.plan(side, &request).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    let job = rig.confirm(side, &plan).await;
    let kept = rig.new_thread_of(&job);
    rig.wait_job(job.id, "succeeded").await;
    assert_eq!(
        rig.world.engine().side_threads(parent, true).await.unwrap(),
        [side]
    );
    let line: Json = sqlx::query_scalar(
        "SELECT payload -> 'data' FROM items WHERE thread_id = $1 AND payload ->> 'marker' = 'side_closed'",
    )
    .bind(parent)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert_eq!(line["why"], json!("kept"));
    assert_eq!(line["into"]["id"], json!(kept));
    let topic: Id = rig
        .world
        .scalar("SELECT parent_id FROM nodes WHERE id = $1", kept)
        .await;
    assert_eq!(
        topic, rig.world.topic,
        "the thread that keeps it lies in the topic"
    );
}

// ---------------------------------------------------------------- several at once

fn shared_batch_request(
    sources: Vec<Id>,
    template: Id,
    name: &str,
) -> orch_api_contract::forks::ForkBatchPreflightBody {
    use orch_api_contract::forks::{
        FileSelection, ForkBatchPreflightBody, ForkEnvironment, ForkNewEnvironment,
    };
    ForkBatchPreflightBody {
        thread_ids: Some(sources),
        environment: Some(ForkEnvironment::New {
            new: ForkNewEnvironment {
                name: Some(name.to_owned()),
                template_id: Some(template),
                files: Some(FileSelection::Selection {
                    include: vec!["src/**".to_owned()],
                    exclude: None,
                }),
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

async fn confirm_shared_new_batch(
    rig: &Rig,
    body: &orch_api_contract::forks::ForkBatchPreflightBody,
) -> (Id, Vec<JobRow>) {
    let draft = rig
        .world
        .engine()
        .draft_batch(&rig.caller(), body)
        .await
        .unwrap();
    assert_eq!(draft.plan.executable, 3, "{:?}", draft.plan.plans);
    assert!(draft.plan.plans.iter().all(|plan| plan.blockers.is_empty()));
    let plan = rig
        .world
        .engine()
        .keep_batch_plan(&rig.caller(), draft)
        .await
        .unwrap();
    rig.world
        .engine()
        .confirm_batch(
            &rig.caller(),
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap()
}

async fn shared_batch_unit(rig: &Rig, host: &hosting::ScriptedHost) -> hosting::Unit {
    let unit = hosting::connector(&rig.world, "shared unit", &["environment"]).await;
    unit.added
        .machine
        .handler
        .on(CommandKind::FsMaterialize, |_| {
            Ok(CommandResult::FsMaterialize(
                orch_link_contract::commands::FsMaterializeResult {
                    files: 3,
                    bytes: 30,
                    skipped: 0,
                },
            ))
        });
    unit.added.machine.handler.on(CommandKind::FsRemove, |_| {
        Ok(CommandResult::FsRemove(Empty {}))
    });
    orch_db::queries::resources::hosting::adopt_machine(
        &mut rig.world.pool.acquire().await.unwrap(),
        unit.id,
        host.started().unwrap(),
        chrono::Utc::now(),
    )
    .await
    .unwrap();
    unit
}

async fn shared_new_batch(
    rig: &Rig,
    host: &hosting::ScriptedHost,
    template: Id,
    sources: Vec<Id>,
    name: &str,
) -> (Id, Vec<JobRow>, hosting::Unit) {
    let mut body = shared_batch_request(sources, template, name);
    body.to = Some(ForkTo {
        provider_id: Some(rig.other_provider),
        agent_id: Some(rig.other_agent),
        ..Default::default()
    });
    let (batch, jobs) = confirm_shared_new_batch(rig, &body).await;
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    // Followers remain queued until the one environment is up, even when the batch permits four runners.
    let states = rig.the_states_of(batch).await;
    assert_eq!(
        states
            .iter()
            .filter(|state| state.as_str() == "running")
            .count(),
        1
    );
    assert_eq!(
        states
            .iter()
            .filter(|state| state.as_str() == "queued")
            .count(),
        2
    );
    let unit = shared_batch_unit(rig, host).await;
    (batch, jobs, unit)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_threads_share_one_new_environment_and_one_provisioning_and_bind_once_each() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "batch host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let sources = vec![
        rig.thread_with_history("One").await,
        rig.thread_with_history("Two").await,
        rig.thread_with_history("Three").await,
    ];
    let (batch, jobs, _unit) =
        shared_new_batch(&rig, &host, template, sources.clone(), "Shared batch").await;
    for job in &jobs {
        rig.wait_job(job.id, "succeeded").await;
    }
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id=$1",
            sources[0],
        )
        .await;
    for source in sources {
        assert_eq!(
            rig.world
                .scalar::<Id>(
                    "SELECT environment_id FROM threads WHERE node_id=$1",
                    source
                )
                .await,
            environment
        );
        let markers: i64 = rig
            .world
            .scalar(
                "SELECT count(*) FROM items WHERE thread_id=$1 AND payload->>'marker'='switched'",
                source,
            )
            .await;
        assert_eq!(markers, 1, "one accepted binding per member");
    }
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 1);
    let provisions: i64 = rig
        .world
        .scalar(
            "SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_provision'",
            environment,
        )
        .await;
    assert_eq!(provisions, 1);
    assert_eq!(
        rig.the_states_of(batch).await,
        ["succeeded", "succeeded", "succeeded"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_environment_move_keeps_each_threads_provider_profile_and_conversation() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "shared move host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let sources = vec![
        rig.thread_with_history("One").await,
        rig.thread_with_history("Two").await,
        rig.thread_with_history("Three").await,
    ];
    let body = shared_batch_request(sources.clone(), template, "One shared workspace");
    let draft = rig
        .world
        .engine()
        .draft_batch(&rig.caller(), &body)
        .await
        .unwrap();
    assert_eq!(draft.plan.executable, 3);
    assert!(
        draft
            .plan
            .plans
            .iter()
            .all(|plan| plan.blockers.is_empty()
                && plan.history.method == HistoryMethod::SameSession)
    );
    let plan = rig
        .world
        .engine()
        .keep_batch_plan(&rig.caller(), draft)
        .await
        .unwrap();
    let (_, jobs) = rig
        .world
        .engine()
        .confirm_batch(
            &rig.caller(),
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _unit = shared_batch_unit(&rig, &host).await;
    for job in &jobs {
        rig.wait_job(job.id, "succeeded").await;
    }
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id=$1",
            sources[0],
        )
        .await;
    assert_ne!(environment, rig.world.environment);
    for source in sources {
        let binding: (Id, Id, Id) = sqlx::query_as(
            "SELECT agent_id,provider_id,environment_id FROM threads WHERE node_id=$1",
        )
        .bind(source)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
        assert_eq!(binding, (rig.world.agent, rig.world.provider, environment));
        let messages: i64 = sqlx::query_scalar("SELECT count(*) FROM items WHERE thread_id=$1 AND ((kind='user_message' AND payload->'parts' @> $2) OR (kind='agent_message' AND payload->>'text'='The plan is ready'))")
            .bind(source).bind(json!([{"type":"text","text":"Plan the work"}])).fetch_one(rig.world.pool.pool()).await.unwrap();
        assert_eq!(
            messages, 2,
            "the original request and answer remain on each moved thread"
        );
    }
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 1);
    assert_eq!(rig.world.scalar::<i64>("SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_provision'", environment).await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_that_times_out_before_provisioning_does_not_prevent_the_remaining_moves() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "replacement maker host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let mut sources = vec![
        rig.thread_with_history("One").await,
        rig.thread_with_history("Two").await,
        rig.thread_with_history("Three").await,
    ];
    sources.sort();
    let (running, _) = rig.world.running_turn(sources[0], "Keep working").await;
    let mut body = shared_batch_request(sources.clone(), template, "A surviving batch");
    body.to = Some(ForkTo {
        provider_id: Some(rig.other_provider),
        agent_id: Some(rig.other_agent),
        ..Default::default()
    });
    body.options = Some(ChangeOptions {
        active_turn: Some(ActiveTurn::Wait),
        wait_minutes: Some(0),
        ..Default::default()
    });
    let (batch, jobs) = confirm_shared_new_batch(&rig, &body).await;
    assert_eq!(jobs[0].node_id, Some(sources[0]));
    let failed = rig.wait_job(jobs[0].id, "failed").await;
    assert_eq!(failed.error.as_ref().unwrap()["code"], "wait_timeout");
    assert!(
        failed.plan.get("new_environment").is_none(),
        "the original maker failed before creating anything"
    );
    host.wait_for(CommandKind::NodeRuntimeStart, 1).await;
    let _unit = shared_batch_unit(&rig, &host).await;
    for job in &jobs[1..] {
        rig.wait_job(job.id, "succeeded").await;
    }
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id=$1",
            sources[1],
        )
        .await;
    assert_eq!(
        rig.world
            .scalar::<Id>(
                "SELECT environment_id FROM threads WHERE node_id=$1",
                sources[2]
            )
            .await,
        environment
    );
    assert_eq!(
        rig.world
            .scalar::<Id>(
                "SELECT environment_id FROM threads WHERE node_id=$1",
                sources[0]
            )
            .await,
        rig.world.environment
    );
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 1);
    assert_eq!(rig.world.scalar::<i64>("SELECT count(*) FROM jobs WHERE environment_id=$1 AND kind='environment_provision'", environment).await, 1);
    assert_eq!(
        rig.the_states_of(batch).await,
        ["failed", "succeeded", "succeeded"]
    );
    rig.world
        .say(
            sources[0],
            common::turn_ended(running.turn_id, TurnOutcome::Completed),
        )
        .await;
    rig.world.wait_state(sources[0], "idle").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn one_failed_member_cannot_remove_the_shared_environment_of_the_successful_members() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "partial batch host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let sources = vec![
        rig.thread_with_history("One").await,
        rig.thread_with_history("Two").await,
        rig.thread_with_history("Three").await,
    ];
    let refused = sources[1];
    let original_environment = rig.world.environment;
    let original_segment = rig.world.segment_of(refused).await;
    let original_native = orch_db::queries::threads::segment(
        &mut rig.world.pool.acquire().await.unwrap(),
        original_segment,
    )
    .await
    .unwrap()
    .unwrap()
    .native_session_ref
    .unwrap();
    rig.on_open(move |open| {
        // Refuse this member's new destination, not its retained original
        // session when the real undo path resumes it after that refusal.
        if open.thread_id == refused && open.exec.environment_id != original_environment {
            return Err(CommandError::new(
                ErrorCode::Internal,
                "this destination session was refused",
            ));
        }
        Ok(SessionOpenResult {
            native_session_ref: if open.mode == SessionMode::Resume {
                open.native_session_ref.clone().unwrap()
            } else {
                format!("batch-{}", open.thread_id)
            },
            resumed: open.mode == SessionMode::Resume,
            import_report: None,
            instruction_sources: Vec::new(),
            approval_reviewer: None,
        })
    });
    let (batch, jobs, _unit) =
        shared_new_batch(&rig, &host, template, sources.clone(), "Partly shared").await;
    for job in &jobs {
        rig.wait_job_over(job.id, || format!("batch {batch}")).await;
    }
    assert_eq!(
        rig.the_states_of(batch).await,
        ["failed", "succeeded", "succeeded"]
    );
    let environment: Id = rig
        .world
        .scalar(
            "SELECT environment_id FROM threads WHERE node_id=$1",
            sources[0],
        )
        .await;
    assert_eq!(
        rig.world
            .scalar::<Id>(
                "SELECT environment_id FROM threads WHERE node_id=$1",
                sources[2]
            )
            .await,
        environment
    );
    assert_eq!(
        rig.world
            .scalar::<Id>(
                "SELECT environment_id FROM threads WHERE node_id=$1",
                refused
            )
            .await,
        rig.world.environment
    );
    assert!(
        rig.opens()
            .iter()
            .any(|open| open.thread_id == refused
                && open.exec.environment_id != original_environment),
        "the requested destination was actually attempted before refusal"
    );
    assert!(
        rig.opens().iter().any(|open| open.thread_id == refused
            && open.exec.environment_id == original_environment
            && open.segment_id == original_segment
            && open.mode == SessionMode::Resume
            && open.native_session_ref.as_deref() == Some(original_native.as_str())),
        "the failed member recovered its exact original native session"
    );
    let restored: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE batch_id=$1 AND node_id=$2
        AND plan#>>'{history_original_restored,segment_id}'=$3::uuid::text
        AND plan#>>'{history_original_restored,native_session_ref}'=$4)",
    )
    .bind(batch)
    .bind(refused)
    .bind(original_segment)
    .bind(&original_native)
    .fetch_one(rig.world.pool.pool())
    .await
    .unwrap();
    assert!(
        restored,
        "the original recovery receipt was retained before completing the failed member"
    );
    let remove: Option<chrono::DateTime<chrono::Utc>> = rig
        .world
        .scalar(
            "SELECT remove_at FROM environments WHERE id=$1",
            environment,
        )
        .await;
    assert_eq!(remove, None);
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_every_member_removes_the_one_unpublished_environment_and_keeps_all_sources() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    hosting::serve(&rig.world);
    let host = hosting::ScriptedHost::on(&rig.world, "cancelled batch host", 16_384).await;
    let template = hosting::template(&rig.world).await;
    let sources = vec![
        rig.thread_with_history("One").await,
        rig.thread_with_history("Two").await,
        rig.thread_with_history("Three").await,
    ];
    rig.world
        .machine
        .handler
        .hold(CommandKind::SessionOpenFenced);
    let (batch, jobs, _unit) =
        shared_new_batch(&rig, &host, template, sources.clone(), "Abandoned batch").await;
    // Cancellation reaches the running maker and all still-queued followers under their accepted locks.
    rig.world
        .engine()
        .cancel_batch(rig.world.ada, batch)
        .await
        .unwrap();
    rig.world.machine.handler.release();
    for job in &jobs {
        rig.wait_job_over(job.id, || format!("batch {batch}")).await;
    }
    let rows = rig.world.engine().batch_jobs(batch).await.unwrap();
    let environment = rows
        .iter()
        .find_map(|job| {
            job.plan
                .get("new_environment")
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
        })
        .unwrap();
    for source in sources {
        assert_eq!(
            rig.world
                .scalar::<Id>(
                    "SELECT environment_id FROM threads WHERE node_id=$1",
                    source
                )
                .await,
            rig.world.environment
        );
    }
    eventually(
        "the shared unpublished environment is scheduled for scratch cleanup",
        || async {
            let row: (
                Option<chrono::DateTime<chrono::Utc>>,
                Option<chrono::DateTime<chrono::Utc>>,
            ) = sqlx::query_as("SELECT remove_at,deleted_at FROM environments WHERE id=$1")
                .bind(environment)
                .fetch_one(rig.world.pool.pool())
                .await
                .unwrap();
            row.0.is_some() || row.1.is_some()
        },
    )
    .await;
    assert_eq!(host.count(CommandKind::NodeRuntimeStart), 1);
}

impl Rig {
    async fn batch_plan(&self, threads: Vec<Id>) -> orch_api_contract::forks::ForkBatchPlan {
        let body = orch_api_contract::forks::ForkBatchPreflightBody {
            thread_ids: Some(threads),
            to: Some(ForkTo {
                provider_id: Some(self.other_provider),
                agent_id: Some(self.other_agent),
                ..ForkTo::default()
            }),
            ..orch_api_contract::forks::ForkBatchPreflightBody::default()
        };
        let draft = self
            .world
            .engine()
            .draft_batch(&self.caller(), &body)
            .await
            .unwrap();
        self.world
            .engine()
            .keep_batch_plan(&self.caller(), draft)
            .await
            .unwrap()
    }

    async fn the_states_of(&self, batch: Id) -> Vec<String> {
        let mut states: Vec<String> = self
            .world
            .engine()
            .batch_jobs(batch)
            .await
            .unwrap()
            .into_iter()
            .map(|row| row.state)
            .collect();
        states.sort();
        states
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_batch_makes_a_job_for_each_plan_without_blockers_locks_all_at_once_and_runs_a_few_at_a_time()
 {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES (NULL, 'switch.batch_parallel', '1')
         ON CONFLICT (node_id, key) DO UPDATE SET value = '1'",
    )
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    let (a, b, c) = (
        rig.thread_with_history("A").await,
        rig.thread_with_history("B").await,
        rig.thread_with_history("C").await,
    );
    // A thread that runs on the target already has nothing to change.
    let there = rig
        .world
        .engine()
        .create_thread(
            &rig.caller(),
            orch_engine::binding::NewThread {
                topic: rig.world.topic,
                name: "There already".to_owned(),
                agent: rig.other_agent,
                provider: rig.other_provider,
                environment: rig.world.environment,
                directory: "web".to_owned(),
                arrangement: None,
                model: Some(MODEL.to_owned()),
                effort: None,
            },
        )
        .await
        .unwrap()
        .thread;
    let plan = rig.batch_plan(vec![a, b, c, there]).await;
    assert_eq!(plan.plans.len(), 4);
    assert_eq!(plan.executable, 3);
    let blocked: Vec<_> = plan
        .plans
        .iter()
        .filter(|plan| !plan.blockers.is_empty())
        .collect();
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0].blockers[0].code, "nothing_to_change");
    assert!(plan.estimate.seconds_high >= plan.plans[0].estimate.seconds_high);

    rig.world.machine.handler.hold(CommandKind::SessionExport);
    let (batch, jobs) = rig
        .world
        .engine()
        .confirm_batch(
            &rig.caller(),
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(jobs.len(), 3, "one job for each plan without blockers");
    assert!(jobs.iter().all(|job| job.batch_id == Some(batch)));
    // All three threads are held at once, those whose job waits its turn too; the fourth is left alone.
    let mut conn = rig.world.pool.acquire().await.unwrap();
    for thread in [a, b, c] {
        assert!(
            orch_engine::locks::lock_of(&mut conn, thread)
                .await
                .unwrap()
                .is_some(),
            "{thread} is locked"
        );
    }
    assert!(
        orch_engine::locks::lock_of(&mut conn, there)
            .await
            .unwrap()
            .is_none()
    );
    drop(conn);
    // At most as many run as the setting allows; the others wait queued.
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    meanwhile(Duration::from_millis(500)).await;
    assert_eq!(
        rig.the_states_of(batch).await,
        ["queued", "queued", "running"],
        "one at a time"
    );
    rig.world.machine.handler.release();
    for _ in 0..1600 {
        if rig.the_states_of(batch).await == ["succeeded", "succeeded", "succeeded"] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert_eq!(
        rig.the_states_of(batch).await,
        ["succeeded", "succeeded", "succeeded"],
        "each job succeeds on its own"
    );
    for thread in [a, b, c] {
        let provider: Id = rig
            .world
            .scalar("SELECT provider_id FROM threads WHERE node_id = $1", thread)
            .await;
        assert_eq!(provider, rig.other_provider);
    }
    let rows = rig.world.engine().batch_jobs(batch).await.unwrap();
    assert_eq!(orch_engine::jobs::batch::percent_of_batch(&rows), 100);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_a_batch_cancels_the_queued_jobs_at_once_and_the_others_before_their_commit() {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let rig = Rig::new(can, "fake", can).await;
    sqlx::query(
        "INSERT INTO settings (node_id, key, value) VALUES (NULL, 'switch.batch_parallel', '1')
         ON CONFLICT (node_id, key) DO UPDATE SET value = '1'",
    )
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    let (a, b) = (
        rig.thread_with_history("A").await,
        rig.thread_with_history("B").await,
    );
    let plan = rig.batch_plan(vec![a, b]).await;
    rig.world.machine.handler.hold(CommandKind::SessionExport);
    rig.world.machine.handler.hold(CommandKind::MachineProbe);
    let (batch, _) = rig
        .world
        .engine()
        .confirm_batch(
            &rig.caller(),
            &ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            },
        )
        .await
        .unwrap();
    rig.world.wait_began(CommandKind::SessionExport, 1).await;
    let export = rig
        .world
        .machine
        .handler
        .executions()
        .into_iter()
        .find(|execution| execution.kind == CommandKind::SessionExport)
        .unwrap()
        .id;
    rig.world
        .engine()
        .cancel_batch(rig.world.ada, batch)
        .await
        .unwrap();
    let mut barrier_released = false;
    for _ in 0..1600 {
        if !barrier_released && rig.world.began(CommandKind::MachineProbe) > 0 {
            assert!(
                rig.the_states_of(batch)
                    .await
                    .iter()
                    .any(|state| state == "running"),
                "cancellation cannot finish before its native barrier replies"
            );
            rig.world.machine.handler.release();
            barrier_released = true;
        }
        if rig.the_states_of(batch).await == ["cancelled", "cancelled"] {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let rows = rig.world.engine().batch_jobs(batch).await.unwrap();
    let diagnostics: Vec<_> = rows
        .iter()
        .map(|row| {
            let (progress, _) = orch_engine::jobs::runner::read_progress(row);
            json!({
                "state": row.state,
                "phase": progress.phase,
                "message": progress.message,
                "cancel_requested": row.cancel_requested(),
                "plan_keys": row.plan.as_object().map(|plan| plan.keys().collect::<Vec<_>>()),
            })
        })
        .collect();
    assert_eq!(
        rig.the_states_of(batch).await,
        ["cancelled", "cancelled"],
        "{diagnostics:?}"
    );
    assert!(
        barrier_released,
        "cancellation must win over the held export"
    );
    let fence: Id =
        sqlx::query_scalar("SELECT fence_id FROM thread_history_cut_commands WHERE command_id=$1")
            .bind(export)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    let mut conn = rig.world.pool.acquire().await.unwrap();
    let original = orch_db::queries::link::command(&mut conn, export)
        .await
        .unwrap()
        .unwrap();
    let barrier = orch_db::queries::link::command(&mut conn, fence)
        .await
        .unwrap()
        .unwrap();
    drop(conn);
    assert_eq!(barrier.kind, CommandKind::MachineProbe.as_str());
    assert_eq!(
        (barrier.machine_id, barrier.serial_key()),
        (original.machine_id, original.serial_key()),
        "the barrier must drain the original native queue"
    );
    assert!(matches!(
        rig.world
            .commands
            .as_engine()
            .status(fence)
            .await
            .unwrap()
            .and_then(|info| info.outcome),
        Some(orch_engine::commands::Outcome::Succeeded(_))
    ));
    for thread in [a, b] {
        let provider: Id = rig
            .world
            .scalar("SELECT provider_id FROM threads WHERE node_id = $1", thread)
            .await;
        assert_eq!(provider, rig.world.provider, "nothing was changed");
        rig.world.wait_state(thread, "idle").await;
    }
}

// ---------------------------------------------------------------- through the whole stack

/// A scripted harness that keeps what each of its sessions was told, writes a session out and reads one in,
/// and answers with all it knows: so that a thread moved from one provider to another shows by what it
/// answers there what came along.
#[derive(Default)]
struct Remembering {
    /// What each session was told and has said, in order, by the name of the session.
    sessions: Mutex<HashMap<String, Vec<String>>>,
    /// The segment and the session each thread is in now.
    in_thread: Mutex<HashMap<Id, (Id, String)>>,
    /// What it exported, by the hash that names it.
    archives: Mutex<HashMap<Hash, Vec<u8>>>,
    /// How many sessions it has made.
    made: AtomicU64,
}

fn refusal(why: &str) -> CommandError {
    CommandError::new(ErrorCode::Internal, why)
}

impl Remembering {
    /// Teaches the machine of the rig to be this harness.
    fn on(rig: &Rig) -> Arc<Self> {
        let harness = Arc::new(Self::default());
        let handler = &rig.world.machine.handler;
        rig.on_open({
            let (harness, store) = (Arc::clone(&harness), Arc::clone(&rig.store));
            move |open| {
                let (name, resumed, said) = match open.mode {
                    SessionMode::Resume | SessionMode::Adopt => {
                        let name = open
                            .native_session_ref
                            .clone()
                            .ok_or_else(|| refusal("a session is resumed by its name"))?;
                        let said = lock(&harness.sessions)
                            .get(&name)
                            .cloned()
                            .unwrap_or_default();
                        (name, true, said)
                    }
                    SessionMode::ImportNative => {
                        let archive = open.archive.as_ref().ok_or_else(|| refusal("no archive"))?;
                        let bytes = lock(&harness.archives)
                            .get(&archive.blob)
                            .cloned()
                            .ok_or_else(|| refusal("this harness never wrote that archive"))?;
                        let said: Vec<String> = serde_json::from_slice(&bytes)
                            .map_err(|error| refusal(&error.to_string()))?;
                        (harness.name(), false, said)
                    }
                    SessionMode::ImportNeutral => {
                        let history = open.history.as_ref().ok_or_else(|| refusal("no history"))?;
                        let text = tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                let read = store.get(&history.blob, None).await.ok()?;
                                let bytes =
                                    orch_blob_contract::store::read_all(read.body).await.ok()?;
                                String::from_utf8(bytes.to_vec()).ok()
                            })
                        })
                        .ok_or_else(|| refusal("the history cannot be read"))?;
                        let said = text
                            .lines()
                            .filter_map(|line| serde_json::from_str::<Json>(line).ok())
                            .filter_map(|item| match item["kind"].as_str() {
                                Some("user_message") => item["payload"]["parts"][0]["text"]
                                    .as_str()
                                    .map(str::to_owned),
                                Some("agent_message") => {
                                    item["payload"]["text"].as_str().map(str::to_owned)
                                }
                                _ => None,
                            })
                            .collect();
                        (harness.name(), false, said)
                    }
                    SessionMode::New | SessionMode::Fork => (harness.name(), false, Vec::new()),
                };
                lock(&harness.sessions).insert(name.clone(), said);
                lock(&harness.in_thread).insert(open.thread_id, (open.segment_id, name.clone()));
                Ok(SessionOpenResult {
                    native_session_ref: name,
                    resumed,
                    import_report: None,
                    instruction_sources: Vec::new(),
                    approval_reviewer: None,
                })
            }
        });
        rig.on_export({
            let (harness, store, pool, clock) = (
                harness.clone(),
                rig.store.clone(),
                rig.world.pool.clone(),
                rig.world.clock.clone(),
            );
            move |export| {
                let (_, name) = lock(&harness.in_thread)
                    .get(&export.thread_id)
                    .cloned()
                    .ok_or_else(|| refusal("the thread has no session here"))?;
                let said = lock(&harness.sessions)
                    .get(&name)
                    .cloned()
                    .unwrap_or_default();
                let bytes =
                    serde_json::to_vec(&said).map_err(|error| refusal(&error.to_string()))?;
                let hash = Hash::of(&bytes);
                let size = i64::try_from(bytes.len()).unwrap_or(i64::MAX);
                lock(&harness.archives).insert(hash, bytes.clone());
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        store
                            .put(
                                &hash,
                                Some(size),
                                orch_blob_contract::store::stream_of(vec![bytes.into()]),
                            )
                            .await
                            .map_err(|_| refusal("archive upload failed"))?;
                        orch_db::queries::storage::register_blob(
                            &mut *pool
                                .acquire()
                                .await
                                .map_err(|_| refusal("archive metadata unavailable"))?,
                            &hash,
                            size,
                            clock.now(),
                        )
                        .await
                        .map_err(|_| refusal("archive metadata failed"))?;
                        Ok::<_, CommandError>(())
                    })
                })?;
                Ok(SessionExportResult {
                    format: "remembering-v1".to_owned(),
                    blob: hash,
                    size,
                })
            }
        });
        for kind in [CommandKind::TurnStart, CommandKind::TurnStartFenced] {
            handler.on(kind, {
                let (harness, machine) = (Arc::clone(&harness), Arc::clone(&rig.world.machine));
                move |command| {
                    let start = match command {
                        Command::TurnStart(start) => start,
                        Command::TurnStartFenced(start) => &start.turn,
                        _ => return Err(refusal("not a real turn start")),
                    };
                    let (segment, name) =
                        lock(&harness.in_thread)
                            .get(&start.thread_id)
                            .cloned()
                            .ok_or_else(|| refusal("the thread has no session here"))?;
                    let reply = {
                        let mut sessions = lock(&harness.sessions);
                        let said = sessions.entry(name).or_default();
                        said.push(common::words_after_where(&start.text).to_owned());
                        let reply = format!("I know: {}", said.join(" / "));
                        said.push(reply.clone());
                        reply
                    };
                    let (thread, turn, machine) =
                        (start.thread_id, start.turn_id, Arc::clone(&machine));
                    tokio::spawn(async move {
                        let stream = Stream::Session {
                            thread_id: thread,
                            segment_id: segment,
                        };
                        machine.publish(&stream, &common::turn_began(turn)).await;
                        machine
                            .publish(&stream, &common::message_began(turn, "m"))
                            .await;
                        machine
                            .publish(&stream, &common::message_grew("m", 0, &reply))
                            .await;
                        machine
                            .publish(&stream, &common::message_done("m", &reply))
                            .await;
                        machine
                            .publish(&stream, &common::turn_ended(turn, TurnOutcome::Completed))
                            .await;
                    });
                    Ok(if kind == CommandKind::TurnStart {
                        CommandResult::TurnStart(Empty {})
                    } else {
                        CommandResult::TurnStartFenced(Empty {})
                    })
                }
            });
        }
        harness
    }

    fn name(&self) -> String {
        format!(
            "remembered-{}",
            self.made.fetch_add(1, Ordering::SeqCst) + 1
        )
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Rig {
    /// What the agent of a thread has said, oldest first.
    async fn agent_said(&self, thread: Id) -> Vec<String> {
        sqlx::query_scalar(
            "SELECT payload ->> 'text' FROM items
              WHERE thread_id = $1 AND kind = 'agent_message' AND status = 'completed'
              ORDER BY seq_created",
        )
        .bind(thread)
        .fetch_all(self.world.pool.pool())
        .await
        .unwrap()
    }

    /// Says something to a thread and waits for the answer that makes `answers` of them.
    async fn asked(&self, thread: Id, words: &str, answers: usize) -> String {
        self.say(thread, words).await;
        let mut found = Vec::new();
        for _ in 0..3200 {
            found = self.agent_said(thread).await;
            if found.len() >= answers {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        assert_eq!(found.len(), answers, "the agent answered: {found:?}");
        self.world.wait_state(thread, "idle").await;
        found.pop().unwrap()
    }
}

/// A thread answers on one provider, is switched to the other, and answers there knowing what was said.
async fn answers_after_a_switch(
    source: Can,
    target_kind: &str,
    target: Can,
    method: HistoryMethod,
) {
    let rig = Rig::new(source, target_kind, target).await;
    let harness = Remembering::on(&rig);
    let thread = rig.world.thread("Whole stack").await;
    let first = rig.asked(thread, "My name is Ada", 1).await;
    assert_eq!(
        first, "I know: My name is Ada",
        "on the first provider it knows what it was told"
    );

    let plan = rig.plan(thread, &rig.to_other()).await;
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, method);
    let job = rig.confirm(thread, &plan).await;
    rig.wait_job(job.id, "succeeded").await;
    let (provider, agent): (Id, Id) =
        sqlx::query_as("SELECT provider_id, agent_id FROM threads WHERE node_id = $1")
            .bind(thread)
            .fetch_one(rig.world.pool.pool())
            .await
            .unwrap();
    assert_eq!((provider, agent), (rig.other_provider, rig.other_agent));

    // On the other provider the agent knows what was said before: the first message and its own answer.
    let second = rig.asked(thread, "Who am I?", 2).await;
    assert_eq!(
        second, "I know: My name is Ada / I know: My name is Ada / Who am I?",
        "the session it answers from came with everything the other one had"
    );
    // It is another session of the harness, and the harness holds the first one still.
    let sessions = lock(&harness.sessions);
    assert_eq!(sessions.len(), 2, "{sessions:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_that_answered_on_one_provider_is_switched_and_answers_on_another_of_its_kind_knowing_what_was_said()
 {
    let can = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    answers_after_a_switch(can, "fake", can, HistoryMethod::NativeTransplant).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_thread_that_answered_on_one_provider_is_switched_and_answers_on_a_harness_of_another_kind_knowing_what_was_said()
 {
    let source = Can {
        export_native: true,
        import_native: true,
        ..Can::default()
    };
    let target = Can {
        import_neutral: true,
        ..Can::default()
    };
    answers_after_a_switch(source, "fake2", target, HistoryMethod::NeutralImport).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_aside_whose_harness_verified_no_way_to_read_without_acting_is_refused_and_nothing_is_made()
 {
    let rig = Rig::new(
        Can {
            import_neutral: true,
            ..Can::default()
        },
        "fake",
        Can::default(),
    )
    .await;
    let parent = rig.thread_with_history("No boundary").await;
    sqlx::query(
        "UPDATE agents SET descriptor = jsonb_set(descriptor, '{side_thread_modes}', '[]'::jsonb) WHERE id = $1",
    )
    .bind(rig.world.agent)
    .execute(rig.world.pool.pool())
    .await
    .unwrap();
    let refused = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "Only look".to_owned(),
            }],
            "nb".to_owned(),
            None,
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(orch_engine::side_threads::SideError::NoReading(_))
        ),
        "{refused:?}"
    );
    let sides: i64 = sqlx::query_scalar("SELECT count(*) FROM threads WHERE side_of = $1")
        .bind(parent)
        .fetch_one(rig.world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        sides, 0,
        "no aside opened with the tools of an ordinary thread"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_aside_of_a_conversation_its_harness_can_neither_fork_nor_take_in_is_refused() {
    let rig = Rig::new(Can::default(), "fake", Can::default()).await;
    let parent = rig.thread_with_history("No history").await;
    let refused = rig
        .world
        .engine()
        .start_side_thread(
            &rig.caller(),
            parent,
            vec![orch_api_contract::threads::ContentPart::Text {
                text: "What did we decide?".to_owned(),
            }],
            "nh".to_owned(),
            None,
        )
        .await;
    assert!(
        matches!(
            refused,
            Err(orch_engine::side_threads::SideError::NoHistory)
        ),
        "{refused:?}"
    );
}
