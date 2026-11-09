//! Real native events enter the existing scripted-command Engine fixture, as in native_executor_loss.
//! This proves admission and accounting consumers, not full connector/container orchestration.
use super::{claude_native, codex_native, common};
use futures::{StreamExt as _, future::LocalBoxFuture};
use orch_access_contract::permissions::Permission;
use orch_agent_contract::adapter::{AdapterError, AdapterEvent, AgentAdapter, EventStream};
use orch_agent_contract::background::{BackgroundTask, BackgroundTaskState};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{ItemContent, SessionEvent};
use orch_agent_contract::requests::UsageRequest;
use orch_agent_contract::session::{SessionMode, TurnStart};
use orch_api_contract::approvals::{
    ApprovalDecision, HarnessOptionDecision, HarnessRequest, OptionScope, OptionStyle,
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, SessionOpenResult,
};
use orch_link_contract::events::Event;
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::{BTreeMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MODEL: &str = "mock-large";
const WITHIN: Duration = Duration::from_secs(90);
const APPROVAL: ApprovalMode = ApprovalMode::OnRequest;
const SANDBOX: SandboxMode = SandboxMode::Open;

fn bounded<'a, T: 'a>(
    phase: &'static str,
    work: impl Future<Output = T> + 'a,
) -> LocalBoxFuture<'a, T> {
    let work = Box::pin(work);
    Box::pin(async move {
        tokio::time::timeout(WITHIN, work)
            .await
            .unwrap_or_else(|_| {
                panic!("native background proof timed out during {phase} after {WITHIN:?}")
            })
    })
}

/// Setup/foreground, completion, accounting and teardown each have the existing phase budget.
pub fn run_case<'a>(
    name: &'static str,
    proof: impl Future<Output = ()> + 'a,
) -> LocalBoxFuture<'a, ()> {
    let proof = Box::pin(proof);
    Box::pin(async move {
        tokio::time::timeout(WITHIN * 4, proof)
            .await
            .unwrap_or_else(|_| {
                panic!("native background proof {name} exceeded its four bounded phases")
            });
    })
}

#[derive(Clone, Copy)]
pub enum Kind {
    Claude,
    Codex,
}

pub enum Harness {
    Claude(Box<claude_native::Rig>),
    Codex(Box<codex_native::Rig>),
}
impl Harness {
    async fn new(kind: Kind) -> Self {
        match kind {
            Kind::Claude => {
                let mut rig = claude_native::Rig::new()
                    .await
                    .expect("the pinned native image is required");
                let started = rig.start_quick().await;
                assert_eq!(started.harness_version, "2.1.319");
                Self::Claude(Box::new(rig))
            }
            Kind::Codex => {
                let mut rig = codex_native::Rig::start("background-wake-engine")
                    .await
                    .expect("the requested native comparison image is required");
                let started = rig.boot().await;
                let expected = std::env::var("ORCH_BACKGROUND_CODEX_VERSION")
                    .unwrap_or_else(|_| "0.167.0".into());
                assert!(
                    matches!(expected.as_str(), "0.165.0" | "0.166.0" | "0.167.0"),
                    "use an explicit approved comparison pin"
                );
                assert_eq!(
                    started.harness_version, expected,
                    "a native case cannot silently qualify another version"
                );
                Self::Codex(Box::new(rig))
            }
        }
    }

    fn parts(&mut self) -> (&mut dyn AgentAdapter, &mut EventStream) {
        match self {
            Self::Claude(rig) => (&mut rig.adapter, &mut rig.events),
            Self::Codex(rig) => (&mut rig.adapter, &mut rig.events),
        }
    }
    fn events(&mut self) -> &mut EventStream {
        match self {
            Self::Claude(rig) => &mut rig.events,
            Self::Codex(rig) => &mut rig.events,
        }
    }
    pub fn workspace(&self) -> &Path {
        match self {
            Self::Claude(rig) => rig.work.path(),
            Self::Codex(rig) => rig.workspace_path(),
        }
    }
    pub fn requests(&self) -> Vec<orch_testkit::doubles::model::RecordedRequest> {
        match self {
            Self::Claude(rig) => rig.model.requests(),
            Self::Codex(rig) => rig.model_requests(),
        }
    }
    fn config_off_at_launch(&self) -> bool {
        let Self::Codex(rig) = self else {
            return false;
        };
        let launches = rig.launcher.launched.lock().unwrap();
        let actual = launches
            .iter()
            .find(|request| request.args.iter().any(|arg| arg == "app-server"))
            .expect("the native app-server was actually launched");
        let home = Path::new(
            actual
                .env
                .get(orch_adapter_codex::process::HOME_VARIABLE)
                .expect("the actual launch uses an isolated home"),
        );
        assert!(home.starts_with(rig.home_path()));
        let config = std::fs::read_to_string(home.join(orch_adapter_codex::process::CONFIG_FILE))
            .expect("the actual launched configuration is readable");
        let mut features = false;
        for line in config.lines().map(str::trim) {
            if line.starts_with('[') {
                features = line == "[features]";
            } else if features && line == "wake_on_agent_messages = false" {
                return true;
            }
        }
        false
    }
    pub fn kill_owned(&self) {
        match self {
            Self::Claude(rig) => rig.launcher.kill_all(),
            Self::Codex(rig) => {
                let names = rig.launcher.containers.lock().unwrap().clone();
                codex_native::remove_containers(&names);
            }
        }
    }
    async fn open(&mut self, thread: Id, segment: Id, wake: bool) -> SessionOpenResult {
        let opened = match self {
            Self::Claude(rig) => {
                rig.thread = thread;
                rig.segment = segment;
                let mut spec = rig.spec(SessionMode::New, None);
                spec.settings.model = MODEL.into();
                spec.settings.available_models = vec![MODEL.into()];
                spec.settings.background_wake = wake;
                spec.settings.approval_mode = APPROVAL;
                spec.settings.sandbox_mode = SANDBOX;
                spec.settings.raw_events = true;
                rig.open_with(spec).await
            }
            Self::Codex(rig) => {
                let mut spec = rig.spec(thread, SessionMode::New, None);
                spec.settings.segment_id = segment;
                spec.settings.model = MODEL.into();
                spec.settings.available_models = vec![MODEL.into()];
                spec.settings.background_wake = wake;
                spec.settings.approval_mode = APPROVAL;
                spec.settings.sandbox_mode = SANDBOX;
                spec.settings.raw_events = true;
                rig.adapter.open_session(spec).await.unwrap()
            }
        };
        serde_json::from_value(serde_json::to_value(opened).unwrap()).unwrap()
    }
}

#[derive(Default)]
pub struct State {
    pub foreground_done: bool,
    pub running: HashSet<String>,
    pub finished: Vec<BackgroundTask>,
    pub completed: HashSet<Id>,
    pub native_wake: bool,
    pub quiet: bool,
    pub lost: bool,
    pub interrupted: bool,
    pub config_off: bool,
    pub checked_markers: HashSet<Id>,
    pub requests: Vec<(Id, UsageRequest)>,
    pub native_inputs: Vec<String>,
}

pub struct Bridge {
    pub world: common::World,
    pub native: Harness,
    pub thread: Id,
    pub segment: Id,
    pub author: Id,
    pub author_role: Id,
    pub input: Id,
    pub origin: Id,
    pub release: PathBuf,
    pub state: State,
    pub wake: Option<Id>,
    dispatched: HashSet<Id>,
    interrupted: HashSet<Id>,
    approved: HashSet<Id>,
    resolved: HashSet<String>,
    published: i64,
}

enum NativeControl {
    Start(Box<TurnStart>),
    Approve(Box<orch_link_contract::commands::ApprovalResolve>),
    Interrupt { thread: Id, turn: Id },
    Stop,
}

impl NativeControl {
    async fn run(self, adapter: &mut dyn AgentAdapter) -> Result<(), AdapterError> {
        match self {
            Self::Start(turn) => adapter.start_turn(*turn).await,
            Self::Approve(body) => {
                let orch_link_contract::commands::ApprovalResolve {
                    thread_id,
                    request_ref,
                    decision,
                } = *body;
                adapter
                    .resolve_approval(thread_id, &request_ref, decision)
                    .await
            }
            Self::Interrupt { thread, turn } => adapter.interrupt(thread, turn).await,
            Self::Stop => adapter.stop().await,
        }
    }
}

struct Observation<'a> {
    world: &'a common::World,
    state: &'a mut State,
    published: &'a mut i64,
    thread: Id,
    segment: Id,
    origin: Id,
    wake: Option<Id>,
}

fn quote(path: &Path) -> String {
    format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
}

impl Bridge {
    pub async fn start(kind: Kind, child: bool, wake: bool) -> Self {
        bounded(
            "native harness and Engine setup plus initial foreground",
            Self::start_inner(kind, child, wake),
        )
        .await
    }

    async fn start_inner(kind: Kind, child: bool, wake: bool) -> Self {
        eprintln!("native background proof phase=harness-start");
        let mut native = Harness::new(kind).await;
        eprintln!("native background proof phase=engine-fixture");
        let world = common::World::observed().await;
        let model = sqlx::query("INSERT INTO provider_models(id,provider_id,model_name,display_name,parameters,groups,enabled,source,created_by)
            SELECT $4,provider_id,$2,$2,parameters,groups,true,source,created_by FROM provider_models WHERE provider_id=$1 AND model_name=$3
            ON CONFLICT(provider_id,model_name) DO NOTHING")
            .bind(world.provider).bind(MODEL).bind(common::MODEL).bind(orch_types::new_id()).execute(world.pool.pool()).await.unwrap();
        assert_eq!(model.rows_affected(), 1);
        world.set(world.topic, "agent.model", json!(MODEL)).await;
        // This bridge attaches the owned native workspace as a directory. It has no
        // isolation declaration: use On request and the operator's explicit Open ceiling.
        world
            .set(world.topic, "approval.mode", json!(APPROVAL))
            .await;
        world.set(world.topic, "sandbox.mode", json!(SANDBOX)).await;
        sqlx::query("UPDATE machines SET facts=jsonb_set(facts,'{policy,agent.max_sandbox}',$2) WHERE id=$1")
            .bind(world.machine_id).bind(json!(SANDBOX)).execute(world.pool.pool()).await.unwrap();
        let workspace = native.workspace().to_string_lossy().into_owned();
        let attached_root = workspace.clone();
        world
            .machine
            .handler
            .on(CommandKind::EnvAttach, move |command| {
                let Command::EnvAttach(attach) = command else {
                    return Err(CommandError::new(
                        ErrorCode::Internal,
                        "unexpected directory attachment command",
                    ));
                };
                serde_json::from_value(json!({
                "os":"linux","arch":std::env::consts::ARCH,
                "shell":{"name":"sh","path":"/bin/sh"},"user":"agent","home":"/home/agent",
                "workspace_root":attach.workspace_root.as_deref().unwrap_or(&attached_root),
                "path_style":"posix","case_sensitive":true,
                "executors":[{"kind":"orch-exec","version":"0.1.0","available":true,"reason":null,
                    "sandbox_modes":["read_only","workspace","open"],"tool_sets":["plain"]}],
                "mcp_found":[],"index":"full","mounts":[],"private_views":false,
                "suspend":false,"git":true,"generation":attach.data_generation.unwrap_or(1)
            })).map(CommandResult::EnvAttach)
                .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))
            });
        sqlx::query("UPDATE environments SET placement='connected',template_id=NULL,workspace_root=$2 WHERE id=$1")
            .bind(world.environment).bind(&workspace).execute(world.pool.pool()).await.unwrap();
        world
            .set(
                world.topic,
                "threads.background_wake",
                json!(if wake { "on" } else { "off" }),
            )
            .await;
        let author = world.person("native-worker").await;
        let author_role = world.give(author, "interactor").await;
        let thread = world.thread("Native background continuation").await;
        {
            let mut conn = world.pool.acquire().await.unwrap();
            let principal = Principal::load(
                &mut conn,
                Caller::session(author),
                Time::new(world.clock.now()),
            )
            .await
            .unwrap();
            assert!(
                !orch_authz::nodes::check(
                    &mut conn,
                    &principal,
                    thread,
                    Permission::ThreadInteract,
                    Act::Use,
                    Policy::default(),
                )
                .await
                .unwrap()
                .is_allowed(),
                "a topic role alone must not grant access to a Connected thread"
            );
            // This owned Connected thread explicitly shares its driving row with the
            // topic's role. Removing that role still removes the author's Interact.
            // Live entries apply to this object; profile columns belong to defaults.
            access_testing::add_entry(
                &mut conn,
                &PageKey::new("node", thread, "live"),
                "any",
                Permission::ThreadInteract.as_str(),
                ("role", None, Some("interactor")),
                None,
                world.clock.now(),
            )
            .await
            .unwrap();
            let principal = Principal::load(
                &mut conn,
                Caller::session(author),
                Time::new(world.clock.now()),
            )
            .await
            .unwrap();
            assert!(
                orch_authz::nodes::check(
                    &mut conn,
                    &principal,
                    thread,
                    Permission::ThreadInteract,
                    Act::Use,
                    Policy::default(),
                )
                .await
                .unwrap()
                .is_allowed(),
                "the actual native input author must hold Interact before the first turn"
            );
        }
        world.set(thread, "agent.model", json!(MODEL)).await;
        {
            let mut conn = world.pool.acquire().await.unwrap();
            let row = orch_db::queries::threads::thread(&mut conn, thread)
                .await
                .unwrap()
                .unwrap();
            let environment = orch_db::queries::environments::thread_engine::environment(
                &mut conn,
                world.environment,
            )
            .await
            .unwrap()
            .unwrap();
            let machine = orch_db::queries::environment_facts::of(&mut conn, world.environment)
                .await
                .unwrap()
                .unwrap();
            let template = orch_db::queries::environments::thread_engine::template_spec(
                &mut conn,
                world.environment,
            )
            .await
            .unwrap();
            let settings = orch_engine::resolved::load(&mut conn, thread)
                .await
                .unwrap();
            let descriptor = serde_json::from_value(common::descriptor()).unwrap();
            assert_ne!(
                machine.confinement(),
                "isolated",
                "this bridge must not invent confinement"
            );
            assert_eq!(
                orch_engine::sessions::confinement(
                    &settings,
                    &descriptor,
                    &environment,
                    template.as_ref(),
                    &machine,
                    row.executor.as_deref(),
                )
                .expect("the real consumer's native confinement prerequisite must be positive"),
                (APPROVAL, SANDBOX)
            );
        }
        let segment = world.segment_of(thread).await;
        eprintln!("native background proof phase=native-session-open");
        let opened = native.open(thread, segment, wake).await;
        let reference = opened.native_session_ref.clone();
        for kind in [CommandKind::SessionOpen, CommandKind::SessionOpenFenced] {
            let receipt = opened.clone();
            world
                .machine
                .handler
                .on(kind, move |command| match command {
                    Command::SessionOpen(open)
                        if open.thread_id == thread && open.segment_id == segment =>
                    {
                        Ok(CommandResult::SessionOpen(receipt.clone()))
                    }
                    Command::SessionOpenFenced(open)
                        if open.session.thread_id == thread
                            && open.session.segment_id == segment =>
                    {
                        Ok(CommandResult::SessionOpenFenced(receipt.clone()))
                    }
                    _ => panic!("this fixture owns only its exact source session"),
                });
        }
        world.open_session_for_reports(thread, segment).await;
        let stored: String = world
            .scalar(
                "SELECT native_session_ref FROM segments WHERE id=$1",
                segment,
            )
            .await;
        assert_eq!(
            stored, reference,
            "Engine retains the actual native session receipt"
        );
        let release = native.workspace().join("release-native-task");
        let command = if wake || matches!(kind, Kind::Codex) {
            format!(
                "while [ ! -e {} ]; do sleep 0.1; done; result=$(cat {}); printf '%s' \"$result\"; if [ \"$result\" = fail ]; then exit 7; fi",
                quote(&release),
                quote(&release)
            )
        } else {
            "printf foreground-only".into()
        };
        let script = match (kind, child, wake) {
            (Kind::Claude, false, _) => format!(
                "#!mock\ntool Bash {}\nthen\nsay Waiting for Build.\n",
                if wake {
                    json!({"command":command,"description":"Build","run_in_background":true})
                } else {
                    json!({"command":command,"description":"Foreground"})
                }
            ),
            (Kind::Claude, true, _) => {
                let child = format!(
                    "#!mock\nusage 70 10\ntool Bash {}\nthen\nusage 90 10\nsay Review complete.\n",
                    json!({"command":command})
                );
                format!(
                    "#!mock\ntool Agent {}\nthen\nsay Waiting for Review.\n",
                    json!({"subagent_type":"general-purpose","description":"Review","run_in_background":true,"prompt":child})
                )
            }
            (Kind::Codex, false, _) => format!(
                "#!mock\ntool exec_command {}\nthen\nsay Waiting for Build.\n",
                json!({"cmd":command,"yield_time_ms":1000})
            ),
            (Kind::Codex, true, _) => {
                // The child stays in its foreground command while the parent's first result arrives.
                let child = format!(
                    "#!mock\ntool exec_command {}\nthen\nsay Review complete.\n",
                    json!({"cmd":command,"yield_time_ms":30000})
                );
                format!(
                    "#!mock\ntool spawn_agent {}\nthen\nsay Waiting for Review.\n",
                    json!({"agent_type":"default","message":child})
                )
            }
        };
        let submitted = world
            .engine()
            .submit_input(&Caller::session(author), thread, common::queued(&script))
            .await
            .unwrap();
        let first = world.turn_start(thread, 0).await;
        let origin = first.turn_id;
        let turn: TurnStart = serde_json::from_value(serde_json::to_value(first).unwrap()).unwrap();
        let mut result = Self {
            world,
            native,
            thread,
            segment,
            author,
            author_role,
            input: submitted.input.id,
            origin,
            release,
            state: State::default(),
            wake: None,
            dispatched: HashSet::from([origin]),
            interrupted: HashSet::new(),
            approved: HashSet::new(),
            resolved: HashSet::new(),
            published: 0,
        };
        eprintln!("native background proof phase=initial-native-turn-start");
        result
            .native_control(
                "initial native turn/start reply",
                NativeControl::Start(Box::new(turn)),
            )
            .await;
        result.state.config_off = result.native.config_off_at_launch();
        result
            .pump_until(|state| state.foreground_done && (!wake || !state.running.is_empty()))
            .await;
        result.world.wait_turn(origin, "completed").await;
        result
    }

    fn observation(&mut self) -> Observation<'_> {
        Observation {
            world: &self.world,
            state: &mut self.state,
            published: &mut self.published,
            thread: self.thread,
            segment: self.segment,
            origin: self.origin,
            wake: self.wake,
        }
    }

    async fn accept(&mut self, event: AdapterEvent) {
        self.observation().accept(event).await;
    }

    async fn publish(&mut self, event: Event) {
        self.observation().publish(event).await;
    }

    async fn native_control(&mut self, phase: &'static str, control: NativeControl) {
        let stopping = matches!(&control, NativeControl::Stop);
        let mut observation = Observation {
            world: &self.world,
            state: &mut self.state,
            published: &mut self.published,
            thread: self.thread,
            segment: self.segment,
            origin: self.origin,
            wake: self.wake,
        };
        let (adapter, events) = self.native.parts();
        let mut reply = Box::pin(control.run(adapter));
        // A control handler can emit more than the bounded sink holds before replying.
        // Keep the production-shaped event consumer alive while awaiting that reply.
        bounded(phase, async {
            let mut stream_open = true;
            loop {
                tokio::select! {
                    biased;
                    result = &mut reply => return result,
                    event = events.next(), if stream_open => {
                        match event {
                            Some(event) => observation.accept(event).await,
                            None if stopping => stream_open = false,
                            None => panic!("native stream ended while waiting for {phase}"),
                        }
                    }
                }
            }
        })
        .await
        .unwrap_or_else(|error| panic!("native control failed during {phase}: {error}"));
    }

    async fn dispatch(&mut self) {
        // Only this fixture owner's initial-turn tool questions are answered, once,
        // through the same Engine decision and connector-shaped response as a person.
        let approvals = {
            let mut conn = self.world.pool.acquire().await.unwrap();
            orch_db::queries::approvals::pending_of_thread(&mut conn, self.thread)
                .await
                .unwrap()
        };
        for approval in approvals {
            if approval.kind != "harness"
                || approval.turn_id != Some(self.origin)
                || !self.approved.insert(approval.id)
            {
                continue;
            }
            let request: HarnessRequest = serde_json::from_value(approval.request).unwrap();
            let choice = request
                .options
                .iter()
                .find(|option| {
                    option.style == OptionStyle::Allow && option.scope == Some(OptionScope::Once)
                })
                .expect("the native tool question offers an explicit one-request approval");
            self.world
                .engine()
                .decide_approval(
                    &Caller::session(self.world.ada),
                    approval.id,
                    &ApprovalDecision::Option(HarnessOptionDecision {
                        option_id: choice.id.clone(),
                        note: None,
                    }),
                )
                .await
                .unwrap();
        }
        for command in self.world.seen.of_kind(CommandKind::ApprovalResolve) {
            let Command::ApprovalResolve(body) = command else {
                continue;
            };
            if body.thread_id == self.thread && self.resolved.insert(body.request_ref.clone()) {
                eprintln!("native background proof phase=native-approval-reply");
                self.native_control(
                    "native approval control reply",
                    NativeControl::Approve(Box::new(body)),
                )
                .await;
            }
        }
        for command in self.world.seen.of_kind(CommandKind::TurnInterrupt) {
            let Command::TurnInterrupt(body) = command else {
                continue;
            };
            if body.thread_id == self.thread && self.interrupted.insert(body.turn_id) {
                eprintln!("native background proof phase=native-interrupt-reply");
                self.native_control(
                    "native interrupt control reply",
                    NativeControl::Interrupt {
                        thread: self.thread,
                        turn: body.turn_id,
                    },
                )
                .await;
                self.state.interrupted = true;
            }
        }
        for start in self.world.turn_starts(self.thread) {
            if !self.dispatched.insert(start.turn_id) {
                continue;
            }
            let marker:Option<(bool,Id,Option<Id>)>=sqlx::query_as(
                "SELECT (i.payload->'data'->>'admitted')::boolean,(i.payload->'data'->>'origin_input')::uuid,q.author_id
                 FROM items i JOIN turns t ON t.id=i.turn_id JOIN thread_inputs q ON q.id=t.input_id
                 WHERE i.thread_id=$1 AND i.turn_id=$2 AND i.payload->>'marker'='background_wake'")
                .bind(self.thread).bind(start.turn_id).fetch_optional(self.world.pool.pool()).await.unwrap();
            if let Some((admitted, input, author)) = marker {
                assert!(admitted);
                assert_eq!(input, self.input);
                assert_eq!(author, Some(self.author));
                self.state.checked_markers.insert(start.turn_id);
                self.wake = Some(start.turn_id);
            }
            let turn = serde_json::from_value(serde_json::to_value(start).unwrap()).unwrap();
            eprintln!("native background proof phase=continuation-native-turn-start");
            self.native_control(
                "continuation native turn/start reply",
                NativeControl::Start(Box::new(turn)),
            )
            .await;
        }
    }

    pub async fn pump_until(&mut self, done: impl Fn(&State) -> bool) {
        let mut phase = "command dispatch";
        let result = tokio::time::timeout(WITHIN, async {
            let mut poll = tokio::time::interval(Duration::from_millis(20));
            loop {
                if done(&self.state) {
                    return;
                }
                phase = "command dispatch";
                self.dispatch().await;
                phase = "native event receive";
                tokio::select! {
                    event=self.native.events().next()=>{
                        phase = "native event publication";
                        self.accept(event.expect("the native stream remains open")).await;
                    }
                    _=poll.tick()=>{}
                }
            }
        })
        .await;
        assert!(
            result.is_ok(),
            "native background pump timed out during {phase}; foreground_done={}, running={}, finished={}, completed={}, checked_markers={}",
            self.state.foreground_done,
            self.state.running.len(),
            self.state.finished.len(),
            self.state.completed.len(),
            self.state.checked_markers.len()
        );
    }

    pub async fn finish(&mut self) {
        bounded(
            "task completion, wake admission and completed continuation",
            self.finish_inner(),
        )
        .await;
    }

    async fn finish_inner(&mut self) {
        tokio::fs::write(&self.release, b"finish").await.unwrap();
        self.pump_until(|state| state.completed.len() > 1).await;
        self.flush().await;
        let wake = self
            .wake
            .expect("a completed task starts an admitted continuation");
        self.world.wait_turn(wake, "completed").await;
    }

    pub async fn flush(&self) {
        let stream = Stream::Session {
            thread_id: self.thread,
            segment_id: self.segment,
        };
        bounded(
            "all published native observations to commit",
            common::eventually("all actual native observations to commit", || async {
                self.world
                    .commands
                    .acknowledged(self.world.machine_id, &stream)
                    >= self.published
            }),
        )
        .await;
    }

    pub async fn prove_replay_accounting(&mut self) {
        bounded(
            "continuation accounting and replay idempotence",
            self.prove_replay_accounting_inner(),
        )
        .await;
    }

    async fn prove_replay_accounting_inner(&mut self) {
        let wake = self.wake.expect("an admitted continuation");
        self.flush().await;
        eprintln!("native background proof phase=accounting-follow-up");
        bounded(
            "the ended turn's accounting follow-up",
            self.world.wait_followed_up(self.thread),
        )
        .await;
        let before = self.world.ledger_of(wake).await;
        let total: i64 = before
            .iter()
            .filter(|row| row.counted)
            .map(|row| row.total_tokens)
            .sum();
        assert!(
            total > 0,
            "the scripted native model reports actual continuation usage"
        );
        assert!(
            before.iter().all(|row| row.user_id == Some(self.author)),
            "usage belongs to the original input author, not the thread owner"
        );
        let mut terminal = BTreeMap::new();
        for (turn, request) in &self.state.requests {
            if *turn == wake && request.outcome.is_some() {
                terminal.insert(request.id.clone(), request.clone());
            }
        }
        assert!(
            !terminal.is_empty(),
            "actual native request receipts must reach the engine"
        );
        for request in terminal.into_values() {
            self.publish(Event::ModelRequest(
                orch_link_contract::events::ModelRequest {
                    turn_id: wake,
                    request,
                },
            ))
            .await;
        }
        self.flush().await;
        let after = self.world.ledger_of(wake).await;
        assert_eq!(
            after
                .iter()
                .filter(|row| row.counted)
                .map(|row| row.total_tokens)
                .sum::<i64>(),
            total
        );
        assert_eq!(
            after.len(),
            before.len(),
            "replayed native requests do not create another charge"
        );
    }

    pub async fn close(&mut self) {
        bounded("native and Engine proof teardown", self.close_inner()).await;
    }

    async fn close_inner(&mut self) {
        eprintln!("native background proof phase=native-stop");
        self.native_control("native adapter stop", NativeControl::Stop)
            .await;
        eprintln!("native background proof phase=engine-shutdown");
        bounded(
            "Engine fixture shutdown and pool closure",
            self.world.shutdown(),
        )
        .await;
    }
}

impl Observation<'_> {
    async fn accept(&mut self, event: AdapterEvent) {
        let linked = match event {
            AdapterEvent::Session {
                thread_id,
                segment_id,
                event,
            } => {
                assert_eq!((thread_id, segment_id), (self.thread, self.segment));
                match &event {
                    SessionEvent::TurnCompleted(done) => {
                        self.state.completed.insert(done.turn_id);
                        self.state.foreground_done |= done.turn_id == self.origin;
                    }
                    SessionEvent::BackgroundTasks(report) => {
                        self.state.native_wake |= report.native_wake.is_some();
                        self.state.quiet |= report.native_idle;
                        for task in &report.tasks {
                            let key =
                                json!([task.native_item_ref, task.id, task.run_id]).to_string();
                            if task.state == BackgroundTaskState::Running {
                                self.state.running.insert(key);
                            } else {
                                self.state.running.remove(&key);
                                if task.state.wakes() {
                                    self.state.finished.push(task.clone());
                                }
                            }
                        }
                    }
                    SessionEvent::SessionLost(_) => {
                        self.state.lost = true;
                    }
                    SessionEvent::RawFrame(raw) => {
                        if raw.payload["method"] == "turn/start"
                            && let Some(input) = raw.payload["params"]["input"].as_array()
                        {
                            for part in input {
                                if let Some(text) = part["text"].as_str() {
                                    self.state.native_inputs.push(text.to_owned());
                                }
                            }
                        }
                    }
                    SessionEvent::ItemStarted(item) if self.wake == Some(item.turn_id) => {
                        if matches!(&item.content, ItemContent::AgentMessage(_)) {
                            assert!(
                                self.state.checked_markers.contains(&item.turn_id),
                                "a wake cause is committed before native output is released"
                            );
                        }
                    }
                    _ => {}
                }
                let value = serde_json::to_value(&event).unwrap();
                Event::decode(
                    event.kind(),
                    value.get("body").cloned().unwrap_or(Json::Null),
                )
                .unwrap()
            }
            AdapterEvent::ModelRequest(report)
                if report.thread_id == self.thread && report.segment_id == self.segment =>
            {
                self.state
                    .requests
                    .push((report.turn_id, report.request.clone()));
                Event::ModelRequest(orch_link_contract::events::ModelRequest {
                    turn_id: report.turn_id,
                    request: report.request,
                })
            }
            _ => return,
        };
        self.publish(linked).await;
    }

    async fn publish(&mut self, event: Event) {
        bounded(
            "publishing an actual native event to the Engine",
            self.world.machine.publish(
                &Stream::Session {
                    thread_id: self.thread,
                    segment_id: self.segment,
                },
                &event,
            ),
        )
        .await;
        *self.published += 1;
    }
}
