//! A thread made into another over HTTP: planned and carried out as a switch or a fork, several at once, and the quick form that is a side thread; each request is served for whoever may make the change it asks for and refused to the rest, and a plan that is old, blocked or somebody else's is never carried out.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod binding_support;
mod places;
mod rig;
mod support;

use binding_support::{a_directory_on_a_machine, another_environment, another_provider};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::{
    ForkBatch, ForkBatchCancel, ForkBatchCreate, ForkBatchGet, ForkBatchPlan, ForkBatchPreflight,
    ForkBatchPreflightBody, ForkCreateBody, ForkEnvironment, ForkHistory, ForkPlan, ForkRequest,
    ForkResult, ForkTo, HistoryMethod, HistoryMode, ThreadForkCreate, ThreadForkPreflight,
};
use orch_api_contract::nodes::{JobState, Node, NodeArchive, NodeGet};
use orch_api_contract::ownership::{NodeDelete, NodeDeleteBody};
use orch_api_contract::threads::{
    BlockedReason, ContentPart, Delivery, SideState, Thread, ThreadSideCreate,
    ThreadSideCreateBody, ThreadSideList, ThreadSideListQuery, ThreadState,
};
use orch_api_contract::workspace::{Job, JobCancel, JobGet};
use orch_api_core::testing::Reply;
use orch_link_contract::commands::{
    AgentStartResult, Command, CommandKind, CommandResult, SessionExportResult,
};
use orch_types::{Id, Json};
use places::{Who, give_role, make_topic};
use rig::{MODEL, Scene, descriptor, eventually};
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switched_off_targets_have_their_blockers_for_provider_profile_model_and_environment() {
    let w = Forks::new().await;
    let thread = w.thread_with_history("Off target").await;
    let request = w.to_second();
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(w.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let plan = w.plan(&w.s.ada, thread, &request).await.ok();
    assert!(
        plan.blockers
            .iter()
            .any(|row| row.code == "provider_disabled"),
        "{:?}",
        plan.blockers
    );
    assert_eq!(
        w.carry_out(&w.s.ada, thread, &plan).await.problem().code,
        ErrorCode::PlanBlocked
    );
    sqlx::query("UPDATE providers SET status='ready' WHERE id=$1")
        .bind(w.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
        .bind(w.agent)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let plan = w.plan(&w.s.ada, thread, &request).await.ok();
    assert!(
        plan.blockers.iter().any(|row| row.code == "unavailable"),
        "{:?}",
        plan.blockers
    );
    assert_eq!(
        w.carry_out(&w.s.ada, thread, &plan).await.problem().code,
        ErrorCode::PlanBlocked
    );
    sqlx::query("UPDATE agents SET disabled=false WHERE id=$1")
        .bind(w.agent)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1")
        .bind(w.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let plan = w.plan(&w.s.ada, thread, &request).await.ok();
    assert!(
        plan.blockers
            .iter()
            .any(|row| row.code == "model_not_allowed"),
        "{:?}",
        plan.blockers
    );
    assert_eq!(
        w.carry_out(&w.s.ada, thread, &plan).await.problem().code,
        ErrorCode::PlanBlocked
    );
    sqlx::query("UPDATE provider_models SET enabled=true WHERE provider_id=$1")
        .bind(w.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let environment = another_environment(&w.s, "Off environment", Some(w.s.rig.machine_id)).await;
    sqlx::query("UPDATE environments SET status='disabled' WHERE id=$1")
        .bind(environment)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let mut request = request;
    request.environment = Some(ForkEnvironment::Existing {
        environment_id: environment,
        files: None,
    });
    let plan = w.plan(&w.s.ada, thread, &request).await.ok();
    assert!(
        plan.blockers
            .iter()
            .any(|row| row.code == "environment_disabled"),
        "{:?}",
        plan.blockers
    );
    assert_eq!(
        w.carry_out(&w.s.ada, thread, &plan).await.problem().code,
        ErrorCode::PlanBlocked
    );
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind='thread_fork'")
            .await,
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn provider_wide_switch_uses_current_bindings_and_the_configured_thread_default() {
    let w = Forks::new().await;
    let a = w.thread_with_history("First bound").await;
    let b = w.thread_with_history("Second bound").await;
    let body = ForkBatchPreflightBody {
        all_of_provider: Some(w.s.on.provider),
        to: w.to_second().to,
        ..Default::default()
    };
    let plan = w.batch_plan(&w.s.ada, &body).await.ok();
    assert_eq!(
        plan.plans
            .iter()
            .map(|row| row.source.id)
            .collect::<std::collections::BTreeSet<_>>(),
        [a, b].into_iter().collect()
    );
    sqlx::query("INSERT INTO settings(node_id,key,value) VALUES(NULL,'switch.batch_threads','1') ON CONFLICT(node_id,key) DO UPDATE SET value='1'")
        .execute(w.s.rig.db.pool()).await.unwrap();
    assert_eq!(
        w.batch_plan(&w.s.ada, &body).await.problem().code,
        ErrorCode::RequestInvalid
    );
    sqlx::query(
        "UPDATE settings SET value='500' WHERE node_id IS NULL AND key='switch.batch_threads'",
    )
    .execute(w.s.rig.db.pool())
    .await
    .unwrap();
    let repeated = w.batch_of(vec![a; 201]);
    let plan = w.batch_plan(&w.s.ada, &repeated).await.ok();
    assert_eq!(
        plan.plans.len(),
        1,
        "a raised setting admits a larger request without duplicating a thread"
    );
}

/// What a harness that keeps its sessions in a form another harness of its kind can read declares.
fn native_descriptor() -> Json {
    let mut declared = descriptor();
    for ability in [
        "export_native",
        "import_native",
        "import_neutral",
        "fork_native",
    ] {
        declared["capabilities"][ability] = json!(true);
    }
    declared
}

/// The scene of the tests, a second agent profile on a second provider of the same harness, and a
/// machine whose harness writes its archive when it is asked to.
struct Forks {
    s: Scene,
    agent: Id,
    provider: Id,
    exports: Arc<Mutex<Vec<Command>>>,
}

impl Forks {
    async fn new() -> Self {
        let s = Scene::new().await;
        let (agent, provider) = another_provider(&s, "Second", &["separated"]).await;
        let declared = native_descriptor();
        sqlx::query("UPDATE agents SET descriptor = $1 WHERE id = $2 OR id = $3")
            .bind(&declared)
            .bind(s.on.agent)
            .bind(agent)
            .execute(s.rig.db.pool())
            .await
            .unwrap();
        // Both profiles report the same abilities when their harness starts.
        let handler = &s.rig.machine.handler;
        let reported = declared.clone();
        handler.on(CommandKind::AgentStart, move |_| {
            Ok(CommandResult::AgentStart(AgentStartResult {
                reused: false,
                harness_version: "1.0.0".to_owned(),
                descriptor: serde_json::from_value(reported.clone()).unwrap(),
            }))
        });
        // Recovery puts this real command behind the cancelled export's serial key.
        // Its handler can answer only after the held operation returned; these are declared fixture facts.
        let probe: orch_link_contract::session::MachineFacts = serde_json::from_value(json!({
            "hostname":"m1", "os_version":"scripted", "cpus":1, "memory_bytes":1_048_576,
            "harnesses":[], "adapters":[], "plugins":[], "policy":null,
            "rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0}, "clock_offset_ms":0
        }))
        .unwrap();
        handler.on(CommandKind::MachineProbe, move |_| {
            Ok(CommandResult::MachineProbe(probe.clone()))
        });
        let exports = Arc::new(Mutex::new(Vec::new()));
        let kept = Arc::clone(&exports);
        let archive = s
            .rig
            .world
            .app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from_static(b"a native archive"))
            .await
            .unwrap();
        orch_db::queries::storage::register_blob(
            &mut s.rig.db.acquire().await.unwrap(),
            &archive.hash,
            archive.size,
            s.rig.world.now(),
        )
        .await
        .unwrap();
        handler.on(CommandKind::SessionExport, move |command| {
            kept.lock().unwrap().push(command.clone());
            Ok(CommandResult::SessionExport(SessionExportResult {
                format: "fake-v1".to_owned(),
                blob: archive.hash,
                size: archive.size,
            }))
        });
        Self {
            s,
            agent,
            provider,
            exports,
        }
    }

    /// A thread of Ada's that has said something and been answered.
    async fn thread_with_history(&self, name: &str) -> Id {
        let thread = self.s.thread_by(&self.s.ada, name).await.base.id;
        self.s
            .exchange(thread, 0, "Plan the work", "The plan is ready")
            .await;
        // A native export needs the complete source record, just as the real harness supplies.
        // Keep the scripted exchange's own item identity and final payload in that record.
        let mut conn = self.s.rig.db.acquire().await.unwrap();
        let (native_ref, payload): (String, Json) = sqlx::query_as(
            "SELECT native_ref,payload FROM items WHERE thread_id=$1 AND kind='agent_message'",
        )
        .bind(thread)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        drop(conn);
        let entries = vec![
            serde_json::from_value::<orch_agent_contract::discovery::RecordedEntry>(json!({
                "entry":"said", "native_ref":"said:exchange-0", "turn":1,
                "text":"Plan the work", "left_out":0, "at":null
            }))
            .unwrap(),
            serde_json::from_value(json!({
                "entry":"item", "native_ref":native_ref, "turn":1, "stream":"main",
                "parent_native_ref":null, "status":"completed", "at":null,
                "content":{"kind":"agent_message", "payload":payload}
            }))
            .unwrap(),
        ];
        let report =
            orch_agent_contract::discovery::record_report(std::slice::from_ref(&entries), 0);
        let text = entries
            .iter()
            .map(|entry| serde_json::to_string(entry).unwrap() + "\n")
            .collect::<String>();
        let size = i64::try_from(text.len()).unwrap();
        let stored = self
            .s
            .rig
            .world
            .app
            .app
            .blobs
            .put_bytes(bytes::Bytes::from(text))
            .await
            .unwrap();
        orch_db::queries::storage::register_blob(
            &mut self.s.rig.db.acquire().await.unwrap(),
            &stored.hash,
            stored.size,
            self.s.rig.world.now(),
        )
        .await
        .unwrap();
        self.s
            .rig
            .machine
            .handler
            .on(CommandKind::AgentReadSession, move |_| {
                Ok(CommandResult::AgentReadSession(
                    orch_link_contract::commands::AgentReadSessionResult {
                        model: Some(MODEL.into()),
                        effort: Some("low".into()),
                        turns: 1,
                        entries: 2,
                        record: Some(orch_link_contract::commands::RecordRef {
                            blob: stored.hash,
                            size,
                        }),
                        report: report.clone(),
                    },
                ))
            });
        thread
    }

    /// A switch to the second provider and profile.
    fn to_second(&self) -> ForkRequest {
        ForkRequest {
            result: ForkResult::Replace,
            topic_id: None,
            name: None,
            access: None,
            history: None,
            to: Some(ForkTo {
                provider_id: Some(self.provider),
                agent_id: Some(self.agent),
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

    /// A fork that changes nothing it need not.
    fn fork(&self) -> ForkRequest {
        ForkRequest {
            result: ForkResult::New,
            name: Some("Trial".to_owned()),
            to: None,
            ..self.to_second()
        }
    }

    async fn plan(&self, by: &Who, thread: Id, request: &ForkRequest) -> Reply<ForkPlan> {
        self.s
            .rig
            .world
            .app
            .call(ThreadForkPreflight, &[&thread])
            .token(&by.token)
            .body(request)
            .send()
            .await
    }

    async fn carry_out(&self, by: &Who, thread: Id, plan: &ForkPlan) -> Reply<Job> {
        self.s
            .rig
            .world
            .app
            .call(ThreadForkCreate, &[&thread])
            .token(&by.token)
            .body(&ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            })
            .send()
            .await
    }

    async fn job(&self, by: &Who, id: Id) -> Reply<Job> {
        self.s
            .rig
            .world
            .app
            .call(JobGet, &[&id])
            .token(&by.token)
            .send()
            .await
    }

    /// Waits for a job to end, in whatever state.
    async fn ended(&self, by: &Who, id: Id) -> Job {
        let started = std::time::Instant::now();
        loop {
            let job = self.job(by, id).await.ok();
            if !matches!(job.state, JobState::Queued | JobState::Running) {
                return job;
            }
            assert!(
                started.elapsed() < rig::PATIENCE,
                "the job did not end: {job:?}"
            );
            tokio::time::sleep(Duration::from_millis(30)).await;
        }
    }

    async fn read(&self, by: &Who, id: Id) -> Thread {
        match self
            .s
            .rig
            .world
            .app
            .call(NodeGet, &[&id])
            .token(&by.token)
            .send()
            .await
            .ok()
        {
            Node::Thread(thread) => thread,
            other => panic!("a thread was read, not {other:?}"),
        }
    }

    /// Waits until the harness has been asked to export a session.
    async fn export_begun(&self) {
        let handler = &self.s.rig.machine.handler;
        eventually("a session is being exported", || async {
            handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::SessionExport)
        })
        .await;
    }

    async fn count(&self, sql: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql.to_owned()))
            .fetch_one(self.s.rig.db.pool())
            .await
            .unwrap()
    }

    /// A thread of Ada's in another topic of the project.
    async fn thread_in(&self, topic: Id, name: &str) -> Id {
        use orch_api_contract::binding::EnvironmentChoice;
        use orch_api_contract::nodes::{CreateThread, CreateThreadBinding, ThreadCreate};
        let made = self
            .s
            .rig
            .world
            .app
            .call(ThreadCreate, &[&topic])
            .token(&self.s.ada.token)
            .body(&CreateThread {
                name: Some(name.to_owned()),
                binding: Some(CreateThreadBinding {
                    agent_id: self.s.on.agent,
                    provider_id: self.s.on.provider,
                    arrangement: None,
                }),
                model: Some(MODEL.to_owned()),
                environment: Some(EnvironmentChoice::Use(self.s.on.environment)),
                directory: Some("web".to_owned()),
                ..CreateThread::default()
            })
            .send()
            .await
            .ok();
        let thread = made.base.id;
        self.s
            .exchange(thread, 0, "Plan the work", "The plan is ready")
            .await;
        thread
    }

    async fn batch_plan(&self, by: &Who, body: &ForkBatchPreflightBody) -> Reply<ForkBatchPlan> {
        self.s
            .rig
            .world
            .app
            .call(ForkBatchPreflight, &[])
            .token(&by.token)
            .body(body)
            .send()
            .await
    }

    async fn batch_create(&self, by: &Who, plan: &ForkBatchPlan) -> Reply<ForkBatch> {
        self.s
            .rig
            .world
            .app
            .call(ForkBatchCreate, &[])
            .token(&by.token)
            .body(&ForkCreateBody {
                notify: None,
                preflight_id: plan.preflight_id,
                options: None,
            })
            .send()
            .await
    }

    fn batch_of(&self, threads: Vec<Id>) -> ForkBatchPreflightBody {
        ForkBatchPreflightBody {
            thread_ids: Some(threads),
            to: self.to_second().to,
            ..ForkBatchPreflightBody::default()
        }
    }

    async fn batch_get(&self, by: &Who, batch: Id) -> Reply<ForkBatch> {
        self.s
            .rig
            .world
            .app
            .call(ForkBatchGet, &[&batch])
            .token(&by.token)
            .send()
            .await
    }
}

/// A reply that had to succeed, with a word for where it was asked in the failure.
fn good<T: std::fmt::Debug>(what: &str, reply: Reply<T>) -> T {
    match reply.body {
        Ok(body) => body,
        Err(problem) => panic!("{what}: {} {}", problem.code, problem.detail),
    }
}

fn blocker_codes(plan: &ForkPlan) -> Vec<&str> {
    plan.blockers
        .iter()
        .map(|blocker| blocker.code.as_str())
        .collect()
}

// ---------------------------------------------------------------- one thread

#[tokio::test]
async fn a_preflight_accepts_an_explicit_content_check_without_changing_a_mountless_plan() {
    let w = Forks::new().await;
    let thread = w.thread_with_history("Content check").await;
    let mut request = w.to_second();
    request.check_fingerprints = Some(true);
    let reply = w.plan(&w.s.ada, thread, &request).await;
    assert_eq!(reply.status.as_u16(), 200);
    let plan = reply.ok();
    assert!(plan.mounts.is_empty());
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
}

#[tokio::test]
async fn a_switch_is_planned_without_a_change_and_carried_out_with_the_thread_held_until_it_ends() {
    let w = Forks::new().await;
    let ada = &w.s.ada;
    let thread = w.thread_with_history("Switched").await;
    let plan = w.plan(ada, thread, &w.to_second()).await;
    assert_eq!(plan.status.as_u16(), 200);
    let plan = plan.ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::NativeTransplant);
    assert_eq!(
        plan.locks.len(),
        1,
        "the thread stands still from the moment it is accepted"
    );
    // Planning changed nothing and holds nothing.
    let before = w.read(ada, thread).await;
    assert!(before.lock.is_none());
    assert_eq!(
        before.binding.as_ref().map(|binding| binding.provider.id),
        Some(w.s.on.provider)
    );
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind = 'thread_fork'")
            .await,
        0
    );

    // Carried out while the harness is slow to export the session.
    w.s.rig.machine.handler.hold(CommandKind::SessionExport);
    let accepted = w.carry_out(ada, thread, &plan).await;
    assert_eq!(accepted.status.as_u16(), 202);
    let job = accepted.ok();
    assert_eq!(job.kind, "thread_fork");
    assert_eq!(job.node_id, Some(thread));
    assert_eq!(
        job.created_by.as_ref().map(|person| person.id),
        Some(ada.id())
    );
    w.export_begun().await;
    let held = w.read(ada, thread).await;
    assert_eq!(held.lock.as_ref().map(|lock| lock.id), Some(job.id));
    assert_eq!(
        held.lock.as_ref().map(|lock| lock.kind.as_str()),
        Some("thread_fork")
    );
    assert_eq!(held.state, ThreadState::Blocked);
    assert_eq!(
        held.blocked.as_ref().map(|blocked| blocked.reason),
        Some(BlockedReason::Switching)
    );
    // Something sent to it meanwhile is taken, queued, and starts nothing.
    let sent =
        w.s.say(
            ada,
            thread,
            "While you were away",
            Delivery::Queue,
            "away-1",
        )
        .await;
    assert!(sent.status.is_success(), "{:?}", sent.body);
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(
        w.s.rig.turn_starts(thread).len(),
        1,
        "no turn starts under a lock"
    );
    assert_eq!(w.read(ada, thread).await.pending_inputs, 1);

    w.s.rig.machine.handler.release();
    let done = w.ended(ada, job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    assert_eq!(done.result, Some(json!({ "thread_id": thread })));
    let after = w.read(ada, thread).await;
    assert!(after.lock.is_none());
    assert_eq!(
        after
            .binding
            .as_ref()
            .map(|binding| (binding.provider.id, binding.agent.id)),
        Some((w.provider, w.agent))
    );
    // What waited runs on the other side.
    let start = w.s.rig.turn_start(thread, 1).await;
    assert!(start.text.contains("While you were away"));
    assert_eq!(
        w.exports.lock().unwrap().len(),
        1,
        "the old harness wrote its session once"
    );
}

#[tokio::test]
async fn a_plan_that_is_blocked_old_or_somebody_elses_is_not_carried_out() {
    let w = Forks::new().await;
    let s = &w.s;
    let thread = w.thread_with_history("Refused").await;

    // Whoever may not see the thread is told there is none.
    assert_eq!(
        w.plan(&s.bea, thread, &w.to_second()).await.problem().code,
        ErrorCode::NotFound
    );
    // A reader may not manage it, and Dan manages it but may not use the provider it would go to: the
    // plan is made, and says what each lacks.
    for (who, wanted) in [(&s.cai, "thread.manage"), (&s.dan, "provider.use")] {
        let plan = w.plan(who, thread, &w.to_second()).await.ok();
        assert!(
            blocker_codes(&plan).contains(&"access"),
            "{:?}",
            plan.blockers
        );
        let named = serde_json::to_string(&plan.blockers).unwrap();
        assert!(named.contains(wanted), "{wanted} is named: {named}");
        let refused = w.carry_out(who, thread, &plan).await.problem();
        assert_eq!(refused.code, ErrorCode::PlanBlocked);
        assert!(refused.extra.contains_key("blockers"));
    }
    // A request that changes nothing is refused: a model or an effort is a setting.
    let nothing = ForkRequest {
        to: Some(ForkTo {
            model: Some(MODEL.to_owned()),
            ..ForkTo::default()
        }),
        ..w.to_second()
    };
    let plan = w.plan(&s.ada, thread, &nothing).await.ok();
    assert_eq!(blocker_codes(&plan), ["nothing_to_change"]);
    assert_eq!(
        w.carry_out(&s.ada, thread, &plan).await.problem().code,
        ErrorCode::PlanBlocked
    );
    // A plan is its maker's.
    let plan = w.plan(&s.ada, thread, &w.to_second()).await.ok();
    assert_eq!(
        w.carry_out(&s.eve, thread, &plan).await.problem().code,
        ErrorCode::NotFound
    );
    // A plan is good for fifteen minutes.
    s.rig.world.app.clock.advance(chrono::Duration::minutes(16));
    assert_eq!(
        w.carry_out(&s.ada, thread, &plan).await.problem().code,
        ErrorCode::PreflightExpired
    );
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind = 'thread_fork'")
            .await,
        0,
        "nothing refused made a job or held a thread"
    );
    assert!(w.read(&s.ada, thread).await.lock.is_none());
}

#[tokio::test]
async fn a_switch_is_cancelled_by_whoever_started_it_before_its_commit_and_the_thread_is_as_it_was()
{
    let w = Forks::new().await;
    let s = &w.s;
    let thread = w.thread_with_history("Cancelled").await;
    let plan = w.plan(&s.ada, thread, &w.to_second()).await.ok();
    s.rig.machine.handler.hold(CommandKind::SessionExport);
    let job = w.carry_out(&s.ada, thread, &plan).await.ok();
    w.export_begun().await;
    // Nobody who could not have started it may stop it.
    let stranger = s
        .rig
        .world
        .app
        .call(JobCancel, &[&job.id])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(stranger.code, ErrorCode::AuthzDenied);
    let stopped = s
        .rig
        .world
        .app
        .call(JobCancel, &[&job.id])
        .token(&s.ada.token)
        .send()
        .await;
    assert!(stopped.status.is_success(), "{:?}", stopped.body);
    s.rig.machine.handler.release();
    let ended = w.ended(&s.ada, job.id).await;
    assert_eq!(ended.state, JobState::Cancelled);
    assert!(ended.error.as_ref().is_some_and(|error| error.undone));
    let thread = w.read(&s.ada, thread).await;
    assert!(thread.lock.is_none());
    assert_eq!(
        thread.binding.as_ref().map(|binding| binding.provider.id),
        Some(s.on.provider),
        "the thread is on the side it was on"
    );
}

#[tokio::test]
async fn a_fork_is_theirs_who_makes_it_and_needs_what_starting_a_thread_there_needs() {
    let w = Forks::new().await;
    let s = &w.s;
    let source = w.thread_with_history("Source").await;
    // A reader holds the right to fork as shipped, and cannot start a thread in the topic.
    let reader = w.plan(&s.cai, source, &w.fork()).await.ok();
    assert!(
        blocker_codes(&reader).contains(&"access"),
        "{:?}",
        reader.blockers
    );
    assert_eq!(
        w.carry_out(&s.cai, source, &reader).await.problem().code,
        ErrorCode::PlanBlocked
    );
    // A developer who may use what the source runs on and manage where it runs may.
    let plan = w.plan(&s.eve, source, &w.fork()).await.ok();
    assert!(plan.blockers.is_empty(), "{:?}", plan.blockers);
    assert_eq!(plan.history.method, HistoryMethod::NativeFork);
    assert_eq!(plan.owner.before, None);
    assert_eq!(plan.owner.after.id, s.eve.id());
    assert!(plan.locks.is_empty(), "a fork holds nothing");
    let job = w.carry_out(&s.eve, source, &plan).await.ok();
    let done = w.ended(&s.eve, job.id).await;
    assert_eq!(done.state, JobState::Succeeded, "{:?}", done.error);
    let made: Id = done.result.as_ref().unwrap()["thread_id"]
        .as_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_ne!(made, source);
    let fork = w.read(&s.eve, made).await;
    assert_eq!(fork.base.owner.username, "eve", "the fork is its maker's");
    assert_eq!(fork.base.name, "Trial");
    assert!(fork.lock.is_none());
    let untouched = w.read(&s.ada, source).await;
    assert!(untouched.lock.is_none());
    assert_eq!(untouched.base.owner.username, "ada");
    // A fork that takes nothing of the history is one the plan says begins fresh.
    let fresh = ForkRequest {
        history: Some(ForkHistory {
            mode: HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        }),
        ..w.fork()
    };
    let plan = w.plan(&s.ada, source, &fresh).await.ok();
    assert_eq!(plan.history.method, HistoryMethod::Nothing);
    assert_eq!(plan.history.turns, 0);
}

// ---------------------------------------------------------------- several at once

#[tokio::test]
async fn a_batch_makes_a_job_for_each_plan_and_is_read_by_those_who_may_and_cancelled_by_its_creator()
 {
    let w = Forks::new().await;
    let s = &w.s;
    let (a, b) = (
        w.thread_with_history("A").await,
        w.thread_with_history("B").await,
    );
    let elsewhere = make_topic(&s.rig.world, &s.ada, s.tree.project.base.id, "Elsewhere")
        .await
        .base
        .id;
    let c = w.thread_in(elsewhere, "C").await;
    // Bea may manage what is in the first topic and does not know the second exists.
    give_role(
        &s.rig.world,
        s.tree.topic.base.id,
        &s.bea,
        "developer",
        &s.ada,
    )
    .await;
    let plan = w.batch_plan(&s.ada, &w.batch_of(vec![a, b, c])).await.ok();
    assert_eq!(plan.plans.len(), 3);
    assert_eq!(plan.executable, 3);
    assert!(plan.plans.iter().all(|each| each.blockers.is_empty()));
    // Nothing is held until it is carried out.
    assert_eq!(
        w.count("SELECT count(*) FROM jobs WHERE kind = 'thread_fork'")
            .await,
        0
    );

    s.rig.machine.handler.hold(CommandKind::SessionExport);
    let created = w.batch_create(&s.ada, &plan).await;
    assert_eq!(created.status.as_u16(), 202);
    let batch = created.ok();
    assert_eq!(
        batch.jobs.len(),
        3,
        "one job for each plan without blockers"
    );
    assert_eq!(batch.hidden, 0);
    assert!(
        batch
            .jobs
            .iter()
            .all(|each| each.job.batch_id == Some(batch.batch_id))
    );
    for thread in [a, b, c] {
        assert!(
            w.read(&s.ada, thread).await.lock.is_some(),
            "all are held at once"
        );
    }
    w.export_begun().await;

    // Its creator reads all of it; a manager of a listed thread reads what they may view and sees
    // how many others there are; everybody else is told there is none.
    let own = w.batch_get(&s.ada, batch.batch_id).await.ok();
    assert_eq!((own.jobs.len(), own.hidden), (3, 0));
    let partly = w.batch_get(&s.bea, batch.batch_id).await.ok();
    assert_eq!((partly.jobs.len(), partly.hidden), (2, 1));
    assert_eq!(
        w.batch_get(&s.cai, batch.batch_id).await.problem().code,
        ErrorCode::NotFound,
        "a reader manages none of them"
    );
    // Only its creator cancels it.
    let refused = s
        .rig
        .world
        .app
        .call(ForkBatchCancel, &[&batch.batch_id])
        .token(&s.bea.token)
        .send()
        .await
        .problem();
    assert_eq!(refused.code, ErrorCode::NotFound);
    let stopped = s
        .rig
        .world
        .app
        .call(ForkBatchCancel, &[&batch.batch_id])
        .token(&s.ada.token)
        .send()
        .await;
    assert!(stopped.status.is_success(), "{:?}", stopped.body);
    s.rig.machine.handler.release();
    let started = std::time::Instant::now();
    let ended = loop {
        let batch = w.batch_get(&s.ada, batch.batch_id).await.ok();
        if batch.counts.queued == 0 && batch.counts.running == 0 {
            break batch;
        }
        assert!(
            started.elapsed() < rig::PATIENCE,
            "the batch did not end: {batch:?}"
        );
        tokio::time::sleep(Duration::from_millis(30)).await;
    };
    assert_eq!(ended.counts.cancelled, 3);
    for thread in [a, b, c] {
        let read = w.read(&s.ada, thread).await;
        assert!(read.lock.is_none(), "a cancelled switch lets go");
        assert_eq!(
            read.binding.as_ref().map(|binding| binding.provider.id),
            Some(s.on.provider)
        );
    }
}

#[tokio::test]
async fn a_batch_names_threads_or_an_environment_and_the_environment_of_a_person_who_does_not_manage_it_is_refused()
 {
    let w = Forks::new().await;
    let s = &w.s;
    let (a, b) = (
        w.thread_with_history("A").await,
        w.thread_with_history("B").await,
    );
    // Exactly one of the two ways to name the threads.
    let neither = w
        .batch_plan(&s.ada, &ForkBatchPreflightBody::default())
        .await
        .problem();
    assert_eq!(neither.code, ErrorCode::RequestInvalid);
    let both = ForkBatchPreflightBody {
        all_of_environment: Some(s.on.environment),
        ..w.batch_of(vec![a])
    };
    assert_eq!(
        w.batch_plan(&s.ada, &both).await.problem().code,
        ErrorCode::RequestInvalid
    );
    let most = usize::try_from(
        orch_settings::get::<i64>(
            &mut w.s.rig.db.acquire().await.unwrap(),
            orch_settings::Place::System,
            orch_settings_contract::registry::keys::SWITCH_BATCH_THREADS,
        )
        .await
        .unwrap()
        .unwrap(),
    )
    .unwrap();
    let too_many = w.batch_of(vec![a; most + 1]);
    assert_eq!(
        w.batch_plan(&s.ada, &too_many).await.problem().code,
        ErrorCode::RequestInvalid
    );
    // Every thread of an environment the caller manages, moved to another environment: the request takes
    // the environment alone.
    let elsewhere = another_environment(s, "Elsewhere", Some(s.rig.machine_id)).await;
    let all = ForkBatchPreflightBody {
        all_of_environment: Some(s.on.environment),
        environment: Some(ForkEnvironment::Existing {
            environment_id: elsewhere,
            files: None,
        }),
        ..ForkBatchPreflightBody::default()
    };
    let with_to = ForkBatchPreflightBody {
        to: w.to_second().to,
        ..all.clone()
    };
    assert_eq!(
        w.batch_plan(&s.ada, &with_to).await.problem().code,
        ErrorCode::RequestInvalid,
        "all the threads of an environment change their environment and nothing else"
    );
    let plan = w.batch_plan(&s.ada, &all).await.ok();
    let planned: std::collections::BTreeSet<Id> =
        plan.plans.iter().map(|each| each.source.id).collect();
    assert_eq!(planned, [a, b].into_iter().collect());
    assert_eq!(plan.executable, 2);
    // A reader manages no environment; Eve manages this one and not the directory of Ada's machine they go
    // to, so every plan she gets says what she lacks, and none of them can be carried out.
    let reader = w.batch_plan(&s.cai, &all).await;
    assert!(!reader.status.is_success(), "{:?}", reader.body);
    let directory = a_directory_on_a_machine(s, "Ada's directory").await;
    let into_directory = ForkBatchPreflightBody {
        environment: Some(ForkEnvironment::Existing {
            environment_id: directory,
            files: None,
        }),
        ..all.clone()
    };
    let blocked = w.batch_plan(&s.eve, &into_directory).await.ok();
    assert_eq!(blocked.plans.len(), 2);
    assert_eq!(blocked.executable, 0);
    assert_eq!(
        w.batch_create(&s.eve, &blocked).await.problem().code,
        ErrorCode::PlanBlocked,
        "a batch is refused only when none of it can be done"
    );
    // An environment that no thread is in has nothing to move.
    let empty = ForkBatchPreflightBody {
        all_of_environment: Some(elsewhere),
        environment: Some(ForkEnvironment::Existing {
            environment_id: s.on.environment,
            files: None,
        }),
        ..ForkBatchPreflightBody::default()
    };
    let plan = w.batch_plan(&s.ada, &empty).await.ok();
    assert!(plan.plans.is_empty());
}

// ---------------------------------------------------------------- side threads

#[tokio::test]
async fn a_side_thread_is_started_by_one_request_listed_and_closed_and_it_refuses_what_it_does_not_take()
 {
    let w = Forks::new().await;
    let s = &w.s;
    let parent = w.thread_with_history("Parent").await;
    let say = |text: &str, client: &str| ThreadSideCreateBody {
        content: vec![ContentPart::Text {
            text: text.to_owned(),
        }],
        client_id: client.to_owned(),
        to_turn: None,
    };
    let start = |who: &Who, on: Id, body: ThreadSideCreateBody| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadSideCreate, &[&on])
            .token(&who.token)
            .body(&body);
        async move { call.send().await }
    };
    // One request makes it, with the message queued in it.
    let made = start(&s.ada, parent, say("A quick question", "q-1")).await;
    assert_eq!(made.status.as_u16(), 201);
    let made = good("the first side thread", made);
    let side = made.thread.base.id;
    assert_eq!(made.thread.side_of.as_ref().map(|of| of.id), Some(parent));
    assert_eq!(made.thread.side_from_turn, Some(1));
    assert_eq!(made.thread.base.name, "Parent (side)");
    assert_eq!(made.input.content.len(), 1);
    // The same request sent again is the same request.
    let again = good(
        "the same request again",
        start(&s.ada, parent, say("A quick question", "q-1")).await,
    );
    assert_eq!(again.thread.base.id, side);
    assert_eq!(again.input.id, made.input.id);
    assert_eq!(
        w.count("SELECT count(*) FROM threads WHERE side_of IS NOT NULL")
            .await,
        1
    );
    // The listing is the parent's.
    let list = |who: &Who, state: Option<SideState>| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadSideList, &[&parent])
            .token(&who.token)
            .query(&ThreadSideListQuery { state });
        async move { call.send().await }
    };
    assert_eq!(good("the open ones", list(&s.ada, None).await).len(), 1);
    assert!(
        good(
            "the closed ones",
            list(&s.ada, Some(SideState::Closed)).await
        )
        .is_empty()
    );
    assert_eq!(
        good("the open ones, read by a reader", list(&s.cai, None).await).len(),
        1,
        "a reader may look"
    );
    assert_eq!(
        list(&s.bea, None).await.problem().code,
        ErrorCode::AuthzDenied
    );
    // Speaking to the parent takes more than looking at it.
    let refused = start(&s.cai, parent, say("Hm", "q-2")).await.problem();
    assert_eq!(refused.code, ErrorCode::AuthzDenied);
    assert_eq!(
        start(&s.bea, parent, say("Hm", "q-3")).await.problem().code,
        ErrorCode::NotFound
    );
    // What it does not take is `thread.side`: a switch, a side thread of its own, an archive, a setting.
    let switch = w.plan(&s.ada, side, &w.to_second()).await.problem();
    assert_eq!(switch.code, ErrorCode::ThreadSide);
    let of_its_own = start(&s.ada, side, say("Hm", "q-4")).await.problem();
    assert_eq!(of_its_own.code, ErrorCode::ThreadSide);
    let archive = s
        .rig
        .world
        .app
        .call(NodeArchive, &[&side])
        .token(&s.ada.token)
        .body(&places::archive_body(&s.rig.world, &s.ada.token, side, true).await)
        .send()
        .await
        .problem();
    assert_eq!(archive.code, ErrorCode::ThreadSide);
    // A fork of it makes an ordinary thread of the topic, and keeps the side thread as closed.
    let kept = good(
        "a fork of a side thread",
        w.plan(&s.ada, side, &w.fork()).await,
    );
    assert!(kept.blockers.is_empty(), "{:?}", kept.blockers);

    // It is closed by its maker, by whoever manages its parent or the environment it runs in; a reader
    // can do none of these.
    let close = |who: &Who| {
        let call = s
            .rig
            .world
            .app
            .call(NodeDelete, &[&side])
            .token(&who.token)
            .body(&NodeDeleteBody::default());
        async move { call.send().await }
    };
    assert_eq!(close(&s.cai).await.problem().code, ErrorCode::AuthzDenied);
    let closed = close(&s.ada).await;
    assert_eq!(closed.status.as_u16(), 200, "{:?}", closed.body);
    assert!(good("the open ones after closing", list(&s.ada, None).await).is_empty());
    let closed_ones = good(
        "the closed ones after closing",
        list(&s.ada, Some(SideState::Closed)).await,
    );
    assert_eq!(
        closed_ones
            .iter()
            .map(|each| each.base.id)
            .collect::<Vec<_>>(),
        [side]
    );
    // Closing it is a line in its parent's history that says how many turns it had and nothing else.
    let line: Json = sqlx::query_scalar(
        "SELECT payload -> 'data' FROM items WHERE thread_id = $1 AND payload ->> 'marker' = 'side_closed'",
    )
    .bind(parent)
    .fetch_one(s.rig.db.pool())
    .await
    .unwrap();
    assert_eq!(line["why"], json!("closed"));
    assert_eq!(line["by"]["username"], json!("ada"));
}

#[tokio::test]
async fn side_threads_are_limited_by_the_setting_of_the_place_and_the_refusal_lists_the_open_ones()
{
    let w = Forks::new().await;
    let s = &w.s;
    let parent = w.thread_with_history("Limited").await;
    sqlx::query("INSERT INTO settings (node_id, key, value) VALUES ($1, 'thread.side_max', '1')")
        .bind(s.tree.topic.base.id)
        .execute(s.rig.db.pool())
        .await
        .unwrap();
    let start = |client: &str| {
        let call = s
            .rig
            .world
            .app
            .call(ThreadSideCreate, &[&parent])
            .token(&s.ada.token)
            .body(&ThreadSideCreateBody {
                content: vec![ContentPart::Text {
                    text: "Hm".to_owned(),
                }],
                client_id: client.to_owned(),
                to_turn: None,
            });
        async move { call.send().await }
    };
    let first = start("one").await.ok().thread.base.id;
    let refused = start("two").await.problem();
    assert_eq!(refused.code, ErrorCode::ThreadSideLimit);
    assert_eq!(refused.extra["open"], json!([first]));
    assert_eq!(refused.extra["max"], json!(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_and_model_thread_selectors_use_current_bindings_while_the_source_is_switched_off()
{
    let w = Forks::new().await;
    let first = w.thread_with_history("First source").await;
    let second = w.thread_with_history("Second source").await;
    sqlx::query("UPDATE agents SET disabled=true WHERE id=$1")
        .bind(w.s.on.agent)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let body = ForkBatchPreflightBody {
        all_of_agent: Some(w.s.on.agent),
        to: w.to_second().to,
        ..Default::default()
    };
    let plan = w.batch_plan(&w.s.ada, &body).await.ok();
    assert_eq!(
        plan.plans
            .iter()
            .map(|row| row.source.id)
            .collect::<std::collections::BTreeSet<_>>(),
        [first, second].into_iter().collect()
    );
    assert!(
        plan.plans.iter().all(|row| row
            .blockers
            .iter()
            .all(|blocker| blocker.code != "agent_disabled")),
        "a switched-off source must not become a target blocker"
    );
    let (model,name):(Id,String)=sqlx::query_as("SELECT id,model_name FROM provider_models WHERE provider_id=$1 ORDER BY model_name LIMIT 1").bind(w.s.on.provider).fetch_one(w.s.rig.db.pool()).await.unwrap();
    for (thread, name) in [
        (first, name.as_str()),
        (second, "a-different-current-selection"),
    ] {
        sqlx::query("INSERT INTO settings(node_id,key,value) VALUES($1,'agent.model',$2) ON CONFLICT(node_id,key) DO UPDATE SET value=excluded.value").bind(thread).bind(json!(name)).execute(w.s.rig.db.pool()).await.unwrap();
    }
    sqlx::query("UPDATE providers SET status='disabled' WHERE id=$1")
        .bind(w.s.on.provider)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    sqlx::query("UPDATE provider_models SET enabled=false WHERE id=$1")
        .bind(model)
        .execute(w.s.rig.db.pool())
        .await
        .unwrap();
    let body = ForkBatchPreflightBody {
        all_of_provider: Some(w.s.on.provider),
        source_model_id: Some(model),
        to: w.to_second().to,
        ..Default::default()
    };
    let plan = w.batch_plan(&w.s.ada, &body).await.ok();
    assert_eq!(
        plan.plans
            .iter()
            .map(|row| row.source.id)
            .collect::<Vec<_>>(),
        vec![first],
        "the exact model membership narrows the shared provider bindings"
    );
    assert!(
        plan.plans[0]
            .blockers
            .iter()
            .all(|blocker| blocker.code != "provider_disabled"
                && blocker.code != "model_not_allowed"),
        "only the new setup is required to be switched on"
    );
    let invalid = ForkBatchPreflightBody {
        all_of_agent: Some(w.s.on.agent),
        source_model_id: Some(model),
        to: w.to_second().to,
        ..Default::default()
    };
    assert_eq!(w.batch_plan(&w.s.ada, &invalid).await.status, 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn switching_away_from_a_disabled_provider_or_environment_keeps_queue_positions_and_checks_each_author()
 {
    use orch_api_contract::environment::EnvironmentDisable;
    use orch_api_contract::resources::ProviderDisable;
    use orch_link_contract::events::TurnOutcome;
    for source in ["provider", "environment"] {
        let w = Forks::new().await;
        let thread = w.thread_with_history("Waiting work").await;
        w.s.rig
            .lets_use("agent", w.agent, "agent.use", &w.s.eve, json!({}))
            .await;
        w.s.say(
            &w.s.ada,
            thread,
            "Active work",
            Delivery::Queue,
            "active-work",
        )
        .await
        .ok();
        let active = w.s.rig.turn_start(thread, 1).await;
        w.s.rig.say(thread, rig::turn_began(active.turn_id)).await;
        w.s.rig.wait_turn(active.turn_id, "running").await;
        let first =
            w.s.say(
                &w.s.eve,
                thread,
                "First, by Eve",
                Delivery::Queue,
                "waiting-eve",
            )
            .await
            .ok()
            .input
            .id;
        let second =
            w.s.say(
                &w.s.ada,
                thread,
                "Second, by Ada",
                Delivery::Queue,
                "waiting-ada",
            )
            .await
            .ok()
            .input
            .id;
        let positions: Vec<(Id, String)> = sqlx::query_as(
            "SELECT id,position::text FROM thread_inputs WHERE id=ANY($1) ORDER BY id",
        )
        .bind([first, second].as_slice())
        .fetch_all(w.s.rig.db.pool())
        .await
        .unwrap();
        if source == "provider" {
            w.s.rig
                .world
                .app
                .call(ProviderDisable, &[&w.s.on.provider])
                .token(&w.s.ada.token)
                .send()
                .await
                .ok();
        } else {
            w.s.rig
                .world
                .app
                .call(EnvironmentDisable, &[&w.s.on.environment])
                .token(&w.s.ada.token)
                .send()
                .await
                .ok();
        }
        w.s.rig
            .say(
                thread,
                rig::turn_ended(active.turn_id, TurnOutcome::Interrupted),
            )
            .await;
        w.s.rig.wait_turn(active.turn_id, "interrupted").await;
        let pending: Vec<(Id, String, bool)> = sqlx::query_as(
            "SELECT id,state,queue_held FROM thread_inputs WHERE id=ANY($1) ORDER BY id",
        )
        .bind([first, second].as_slice())
        .fetch_all(w.s.rig.db.pool())
        .await
        .unwrap();
        assert!(
            pending
                .iter()
                .all(|(_, state, held)| state == "pending" && !held),
            "Disable keeps waiting inputs eligible, {source}: {pending:?}"
        );
        let mut request = w.to_second();
        request.history = Some(ForkHistory {
            mode: HistoryMode::Nothing,
            from_turn: None,
            to_turn: None,
        });
        if source == "environment" {
            let target =
                another_environment(&w.s, "Available environment", Some(w.s.rig.machine_id)).await;
            request.environment = Some(ForkEnvironment::Existing {
                environment_id: target,
                files: None,
            });
        }
        let plan = w.plan(&w.s.ada, thread, &request).await.ok();
        assert!(
            plan.blockers.is_empty(),
            "a switched-off source never traps its thread: {source}: {:?}",
            plan.blockers
        );
        let copied = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        if source == "environment" {
            use orch_engine::manifest::{Entry, EntryKind, Header, build};
            use orch_link_contract::commands::{FsMaterializeResult, FsSnapshotResult};
            let content =
                w.s.rig
                    .world
                    .app
                    .app
                    .blobs
                    .put_bytes(bytes::Bytes::from_static(b"kept file"))
                    .await
                    .unwrap();
            let manifest = build(
                &Header {
                    v: 1,
                    root: plan.files.as_ref().unwrap().path_map.from.clone(),
                    generated_at: orch_types::Time::new(w.s.rig.world.now()),
                    generation: 1,
                    case_sensitive: true,
                },
                vec![Entry {
                    path: "keep.txt".into(),
                    kind: EntryKind::File,
                    size: content.size,
                    mtime_ns: 0,
                    mode: 0o644,
                    hash: Some(content.hash),
                    link: None,
                }],
            )
            .unwrap();
            let archive =
                w.s.rig
                    .world
                    .app
                    .app
                    .blobs
                    .put_bytes(bytes::Bytes::from(manifest.bytes))
                    .await
                    .unwrap();
            for blob in [content, archive] {
                orch_db::queries::storage::register_blob(
                    &mut w.s.rig.db.acquire().await.unwrap(),
                    &blob.hash,
                    blob.size,
                    w.s.rig.world.now(),
                )
                .await
                .unwrap();
            }
            let original = w.s.on.environment;
            w.s.rig
                .machine
                .handler
                .on(CommandKind::FsSnapshot, move |command| {
                    let Command::FsSnapshot(snapshot) = command else {
                        panic!("expected the selected source file read")
                    };
                    assert_eq!(snapshot.environment_id, original);
                    assert!(!snapshot.selection.include.is_empty());
                    Ok(CommandResult::FsSnapshot(FsSnapshotResult {
                        manifest_blob: archive.hash,
                        file_count: 1,
                        total_bytes: 9,
                        uploaded_bytes: 9,
                        missing: 0,
                    }))
                });
            let received = Arc::clone(&copied);
            w.s.rig
                .machine
                .handler
                .on(CommandKind::FsMaterialize, move |command| {
                    let Command::FsMaterialize(write) = command else {
                        panic!("expected the selected destination file write")
                    };
                    assert_ne!(write.environment_id, original);
                    received.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Ok(CommandResult::FsMaterialize(FsMaterializeResult {
                        files: 1,
                        bytes: 9,
                        skipped: 0,
                    }))
                });
        }
        let job = w.carry_out(&w.s.ada, thread, &plan).await.ok();
        let result = w.ended(&w.s.ada, job.id).await;
        assert_eq!(
            result.state,
            JobState::Succeeded,
            "{source}: {:?}",
            result.error
        );
        if source == "environment" {
            assert_eq!(
                copied.load(std::sync::atomic::Ordering::SeqCst),
                2,
                "the selected files and final delta are copied while the source is off"
            );
        }
        let started = w.s.rig.turn_start(thread, 2).await;
        assert!(started.text.contains("Second, by Ada"));
        assert!(
            !started.text.contains("First, by Eve"),
            "Eve has no use of the target provider"
        );
        let attribution:(Id,Id,Id)=sqlx::query_as("SELECT t.input_id,t.audit_for_user_id,s.provider_id FROM turns t JOIN segments s ON s.id=t.segment_id WHERE t.id=$1")
            .bind(started.turn_id).fetch_one(w.s.rig.db.pool()).await.unwrap();
        assert_eq!(attribution, (second, w.s.ada.id(), w.provider));
        let held: (String, bool, Id) =
            sqlx::query_as("SELECT state,queue_held,author_id FROM thread_inputs WHERE id=$1")
                .bind(first)
                .fetch_one(w.s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            held,
            ("pending".into(), false, w.s.eve.id()),
            "model-use approval is not a person's held queue"
        );
        let after: Vec<(Id, String)> = sqlx::query_as(
            "SELECT id,position::text FROM thread_inputs WHERE id=ANY($1) ORDER BY id",
        )
        .bind([first, second].as_slice())
        .fetch_all(w.s.rig.db.pool())
        .await
        .unwrap();
        assert_eq!(
            after, positions,
            "switching never reorders or reauthors the original messages"
        );
        w.s.rig.say(thread, rig::turn_began(started.turn_id)).await;
        w.s.rig
            .say(
                thread,
                rig::turn_ended(started.turn_id, TurnOutcome::Completed),
            )
            .await;
        w.s.rig.wait_turn(started.turn_id, "completed").await;
        assert_eq!(
            w.s.rig.turn_starts(thread).len(),
            3,
            "the disallowed author does not silently run"
        );
        w.s.rig
            .lets_use("provider", w.provider, "provider.use", &w.s.eve, json!({}))
            .await;
        w.s.rig.world.app.app.engine.wake(thread);
        let resumed = w.s.rig.turn_start(thread, 3).await;
        assert!(resumed.text.contains("First, by Eve"));
        let attribution: (Id, Id) =
            sqlx::query_as("SELECT input_id,audit_for_user_id FROM turns WHERE id=$1")
                .bind(resumed.turn_id)
                .fetch_one(w.s.rig.db.pool())
                .await
                .unwrap();
        assert_eq!(
            attribution,
            (first, w.s.eve.id()),
            "the waiting message runs as its original author after permission is granted"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn enable_holds_waiting_messages_by_default_and_runs_them_only_when_selected_for_each_kind() {
    use orch_api_contract::environment::EnvironmentEnable;
    use orch_api_contract::resource_facts::{ResourcePauseQuery, ResourceResumeQuery};
    use orch_api_contract::resources::{
        AgentUpdate, AgentUpdateBody, ProviderEnable, ProviderModelUpdate, ProviderModelUpdateBody,
    };
    use orch_api_contract::threads::{ThreadQueueResume, ThreadQueueResumeBody};
    use orch_link_contract::events::TurnOutcome;
    for kind in ["provider", "model", "agent", "environment"] {
        for run_waiting in [false, true] {
            let w = Forks::new().await;
            let thread = w.thread_with_history("Paused queue").await;
            let model: Id = sqlx::query_scalar(
                "SELECT id FROM provider_models WHERE provider_id=$1 AND model_name=$2",
            )
            .bind(w.s.on.provider)
            .bind(MODEL)
            .fetch_one(w.s.rig.db.pool())
            .await
            .unwrap();
            let (query, object) = match kind {
                "provider" => (
                    "UPDATE providers SET status='disabled' WHERE id=$1",
                    w.s.on.provider,
                ),
                "model" => (
                    "UPDATE provider_models SET enabled=false WHERE id=$1",
                    model,
                ),
                "agent" => ("UPDATE agents SET disabled=true WHERE id=$1", w.s.on.agent),
                _ => (
                    "UPDATE environments SET status='disabled' WHERE id=$1",
                    w.s.on.environment,
                ),
            };
            sqlx::query(sqlx::AssertSqlSafe(query))
                .bind(object)
                .execute(w.s.rig.db.pool())
                .await
                .unwrap();
            let mut inputs = Vec::new();
            for (text, key) in [
                ("First waiting message", "enable-first"),
                ("Second waiting message", "enable-second"),
            ] {
                inputs.push(
                    w.s.say(&w.s.ada, thread, text, Delivery::Queue, key)
                        .await
                        .ok()
                        .input
                        .id,
                );
            }
            w.s.rig.wait_state(thread, "blocked").await;
            let positions: Vec<(Id,String,Id)> = sqlx::query_as("SELECT id,position::text,author_id FROM thread_inputs WHERE id=ANY($1) ORDER BY id")
                .bind(&inputs).fetch_all(w.s.rig.db.pool()).await.unwrap();
            let resume = ResourceResumeQuery {
                run_waiting: run_waiting.then_some(true),
            };
            let patch = ResourcePauseQuery {
                note: None,
                run_waiting: resume.run_waiting,
            };
            match kind {
                "provider" => {
                    w.s.rig
                        .world
                        .app
                        .call(ProviderEnable, &[&object])
                        .query(&resume)
                        .token(&w.s.ada.token)
                        .send()
                        .await
                        .ok();
                }
                "environment" => {
                    w.s.rig
                        .world
                        .app
                        .call(EnvironmentEnable, &[&object])
                        .query(&resume)
                        .token(&w.s.ada.token)
                        .send()
                        .await
                        .ok();
                }
                "agent" => {
                    let revision: i64 =
                        sqlx::query_scalar("SELECT revision FROM agents WHERE id=$1")
                            .bind(object)
                            .fetch_one(w.s.rig.db.pool())
                            .await
                            .unwrap();
                    let body: AgentUpdateBody = serde_json::from_value(
                        json!({"expected_revision":revision,"disabled":false}),
                    )
                    .unwrap();
                    w.s.rig
                        .world
                        .app
                        .call(AgentUpdate, &[&object])
                        .query(&patch)
                        .body(&body)
                        .token(&w.s.ada.token)
                        .send()
                        .await
                        .ok();
                }
                _ => {
                    let body: ProviderModelUpdateBody =
                        serde_json::from_value(json!({"enabled":true})).unwrap();
                    w.s.rig
                        .world
                        .app
                        .call(ProviderModelUpdate, &[&w.s.on.provider, &object])
                        .query(&patch)
                        .body(&body)
                        .token(&w.s.ada.token)
                        .send()
                        .await
                        .ok();
                }
            }
            let after: Vec<(Id,String,Id)> = sqlx::query_as("SELECT id,position::text,author_id FROM thread_inputs WHERE id=ANY($1) ORDER BY id")
                .bind(&inputs).fetch_all(w.s.rig.db.pool()).await.unwrap();
            assert_eq!(
                after, positions,
                "Enable preserves each message's place and author: {kind}"
            );
            if !run_waiting {
                let held: Vec<bool> =
                    sqlx::query_scalar("SELECT queue_held FROM thread_inputs WHERE id=ANY($1)")
                        .bind(&inputs)
                        .fetch_all(w.s.rig.db.pool())
                        .await
                        .unwrap();
                assert_eq!(
                    held,
                    [true, true],
                    "the absent choice holds both messages: {kind}"
                );
                assert_eq!(
                    w.s.rig.turn_starts(thread).len(),
                    1,
                    "Enable alone spends no queued usage: {kind}"
                );
                let revision: i64 =
                    sqlx::query_scalar("SELECT queue_revision FROM threads WHERE node_id=$1")
                        .bind(thread)
                        .fetch_one(w.s.rig.db.pool())
                        .await
                        .unwrap();
                w.s.rig
                    .world
                    .app
                    .call(ThreadQueueResume, &[&thread])
                    .body(&ThreadQueueResumeBody { revision })
                    .token(&w.s.ada.token)
                    .send()
                    .await
                    .ok();
            }
            for (index, input) in inputs.iter().enumerate() {
                let turn = w.s.rig.turn_start(thread, index + 1).await;
                let authored: (Id, Id) =
                    sqlx::query_as("SELECT input_id,audit_for_user_id FROM turns WHERE id=$1")
                        .bind(turn.turn_id)
                        .fetch_one(w.s.rig.db.pool())
                        .await
                        .unwrap();
                assert_eq!(
                    authored,
                    (*input, w.s.ada.id()),
                    "the original input starts as its author: {kind}"
                );
                w.s.rig.say(thread, rig::turn_began(turn.turn_id)).await;
                w.s.rig
                    .say(
                        thread,
                        rig::turn_ended(turn.turn_id, TurnOutcome::Completed),
                    )
                    .await;
                w.s.rig.wait_turn(turn.turn_id, "completed").await;
            }
        }
    }
}
