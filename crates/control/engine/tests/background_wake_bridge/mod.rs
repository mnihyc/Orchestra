//! Real native events enter the existing scripted-command Engine fixture, as in native_executor_loss.
//! This proves admission and accounting consumers, not full connector/container orchestration.
use super::{claude_native, codex_native, common};
use futures::{FutureExt as _, StreamExt as _, future::LocalBoxFuture};
use orch_access_contract::permissions::Permission;
use orch_agent_contract::adapter::{AdapterError, AdapterEvent, AgentAdapter, EventStream};
use orch_agent_contract::background::{BackgroundTask, BackgroundTaskState};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{ItemContent, SessionEvent};
use orch_agent_contract::requests::{RequestOutcome, UsageRequest};
use orch_agent_contract::session::{SessionMode, TurnStart};
use orch_api_contract::approvals::{
    ApprovalDecision, HarnessOptionDecision, HarnessRequest, OptionScope, OptionStyle,
};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::queries::access::{PageKey, testing as access_testing};
use orch_engine::approvals::ApprovalError;
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
use std::sync::OnceLock;
use std::time::{Duration, Instant};

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

/// Seconds since the first line of this test program, so that a failure shows where the time went.
fn clock() -> String {
    static BEGAN: OnceLock<Instant> = OnceLock::new();
    format!(
        "{:.1}",
        BEGAN.get_or_init(Instant::now).elapsed().as_secs_f64()
    )
}

fn phase(name: &str) {
    eprintln!("native background proof phase={name} t={}s", clock());
}

/// One line for a frame of the native program that bears on the case: a child's messages and a task's
/// notices for Claude Code, the items and turns for Codex. Streamed text and output deltas are left out.
fn frame_line(frame: &orch_agent_contract::events::RawFrame) -> Option<String> {
    let payload = &frame.payload;
    let text = |value: &Json| value.as_str().unwrap_or("-").to_owned();
    let cut = |value: &Json| value.to_string().chars().take(160).collect::<String>();
    if let Some(method) = payload["method"].as_str() {
        if method.ends_with("Delta") || method.ends_with("/delta") || method.contains("/delta/") {
            return None;
        }
        let params = &payload["params"];
        let item = &params["item"];
        return Some(format!(
            "{} {method} turn={} item={}:{}:{} status={} exit={} output={}",
            frame.direction,
            text(&params["turn"]["id"])
                .chars()
                .take(8)
                .collect::<String>(),
            text(&item["type"]),
            text(&item["tool"]),
            text(&item["id"]),
            text(&item["status"]),
            item["exitCode"],
            cut(&item["aggregatedOutput"]),
        ));
    }
    let kind = payload["type"].as_str()?;
    let parent = payload["parent_tool_use_id"].as_str();
    let task = kind == "result"
        || (kind == "system"
            && payload["subtype"]
                .as_str()
                .is_some_and(|name| name.starts_with("task_")));
    if parent.is_none() && !task {
        return None;
    }
    let event = &payload["event"];
    if matches!(
        event["type"].as_str(),
        Some("content_block_delta" | "ping" | "content_block_start" | "content_block_stop")
    ) {
        return None;
    }
    let usage = [
        &payload["message"]["usage"],
        &event["message"]["usage"],
        &event["usage"],
        &payload["usage"],
    ]
    .into_iter()
    .find(|usage| !usage.is_null())
    .map_or_else(|| "-".to_owned(), ToString::to_string);
    let whole = if payload["modelUsage"].is_null() {
        String::new()
    } else {
        format!(" model_usage={}", payload["modelUsage"])
    };
    Some(format!(
        "{kind}/{} parent={} message={} usage={usage}{whole}",
        payload["subtype"]
            .as_str()
            .or(event["type"].as_str())
            .unwrap_or("-"),
        parent.unwrap_or("-"),
        text(&payload["message"]["id"]),
    ))
}

/// The usage the native program wrote into the transcripts of its sub-agents, one line each: what it settled
/// on once a message was complete, to set beside what its frames said on the way.
fn transcript_usage(home: &Path) -> Vec<String> {
    let mut lines = Vec::new();
    let mut directories = vec![home.to_path_buf()];
    while let Some(directory) = directories.pop() {
        let Ok(entries) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                directories.push(path);
                continue;
            }
            if path
                .extension()
                .is_none_or(|extension| extension != "jsonl")
            {
                continue;
            }
            let name = path.display().to_string();
            for line in std::fs::read_to_string(&path).unwrap_or_default().lines() {
                let Ok(value) = serde_json::from_str::<Json>(line) else {
                    continue;
                };
                let usage = &value["message"]["usage"];
                if usage.is_null() || !(name.contains("subagent") || value["isSidechain"] == true) {
                    continue;
                }
                lines.push(format!(
                    "transcript {} type={} message={} usage={usage}",
                    path.file_name().map_or_else(
                        || "-".to_owned(),
                        |file| file.to_string_lossy().into_owned()
                    ),
                    value["type"].as_str().unwrap_or("-"),
                    value["message"]["id"].as_str().unwrap_or("-"),
                ));
            }
        }
    }
    lines.sort();
    lines
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
                assert_eq!(started.harness_version, "2.1.322");
                Self::Claude(Box::new(rig))
            }
            Kind::Codex => {
                let mut rig = codex_native::Rig::start("background-wake-engine")
                    .await
                    .expect("the requested native comparison image is required");
                let started = rig.boot().await;
                let expected = std::env::var("ORCH_BACKGROUND_CODEX_VERSION")
                    .unwrap_or_else(|_| "0.167.2".into());
                assert!(
                    matches!(
                        expected.as_str(),
                        "0.165.0" | "0.166.0" | "0.167.0" | "0.167.2"
                    ),
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
    /// Releases a hold of the model server's script (`hold <name>`).
    fn release(&self, name: &str) {
        match self {
            Self::Claude(rig) => rig.model.release(name),
            Self::Codex(_) => panic!("only the Claude flow holds a message"),
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
    /// The frames of the native program that bear on a case, in order (see `frame_line`).
    pub trace: Vec<String>,
    /// The questions the native program took back, by their references: a Claude turn that ends lets go of what is
    /// still open, and tells the engine so with the event that comes after the refusal.
    pub taken_back: HashSet<String>,
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
    /// The file the sub-agent's command creates when it begins, where the case has one.
    child_begun: Option<PathBuf>,
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

    /// A sub-agent whose first message is still being written when its parent's turn ends. Both orders are the case's
    /// own: the model server holds the parent's last message before it says a word until the case has seen the first
    /// block of the sub-agent's message (the start does that), so the parent's result cannot come first; and it holds the
    /// sub-agent's message after that block until the case releases it ([`Bridge::release_the_child`]), which the case
    /// does after the parent's turn is over. The sub-agent then reads a file of the workspace (which the program allows
    /// by itself), so no question is put to a person.
    pub async fn start_streaming_child() -> Self {
        bounded(
            "native harness and Engine setup plus initial foreground",
            Self::start_flow(Kind::Claude, true, true, true),
        )
        .await
    }

    async fn start_inner(kind: Kind, child: bool, wake: bool) -> Self {
        Self::start_flow(kind, child, wake, false).await
    }

    async fn start_flow(kind: Kind, child: bool, wake: bool, streaming: bool) -> Self {
        phase("harness-start");
        let mut native = Harness::new(kind).await;
        phase("engine-fixture");
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
        phase("native-session-open");
        let opened = native.open(thread, segment, wake).await;
        phase("native-session-opened");
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
        let mut child_begun: Option<PathBuf> = None;
        let script = match (kind, child, wake) {
            (Kind::Claude, false, _) => format!(
                "#!mock\ntool Bash {}\nthen\nsay Waiting for Build.\n",
                if wake {
                    json!({"command":command,"description":"Build","run_in_background":true})
                } else {
                    json!({"command":command,"description":"Foreground"})
                }
            ),
            (Kind::Claude, true, _) if streaming => {
                let note = native.workspace().join("note.txt");
                std::fs::write(&note, "a note the sub-agent reads").unwrap();
                let child = format!(
                    "#!mock\nusage 70 10 cached=20 cache_write=5\nsay Looking at it.\nhold child-message\ntool Read {}\nthen\nusage 90 10 cached=30 cache_write=10\nsay Review complete.\n",
                    json!({"file_path": note.to_string_lossy()})
                );
                format!(
                    "#!mock\ntool Agent {}\nthen\nhold parent-message\nsay Waiting for Review.\n",
                    json!({"subagent_type":"general-purpose","description":"Review","run_in_background":true,"prompt":child}),
                )
            }
            (Kind::Claude, true, _) => {
                // The sub-agent's command needs a person's word, and a question of a sub-agent does not outlive the
                // turn of its parent (the engine cancels what is pending when a turn ends, and the harness is told
                // so): the parent's turn does not end until the command has begun, which it does only once the
                // person has allowed it. The case then does not depend on which of the answer and the end of the
                // turn comes first.
                let begun = native.workspace().join("child-command-begun");
                let child = format!(
                    "#!mock\nusage 70 10 cached=20 cache_write=5\ntool Bash {}\nthen\nusage 90 10 cached=30 cache_write=10\nsay Review complete.\n",
                    json!({"command":format!("touch {}; {command}", quote(&begun))})
                );
                let wait = format!("while [ ! -e {} ]; do sleep 0.1; done", quote(&begun));
                child_begun = Some(begun);
                format!(
                    "#!mock\ntool Agent {}\nthen\ntool Bash {}\nthen\nsay Waiting for Review.\n",
                    json!({"subagent_type":"general-purpose","description":"Review","run_in_background":true,"prompt":child}),
                    json!({"command":wait,"description":"Wait for the review to begin"})
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
                // The pinned Codex registers its sub-agent tools in the namespace of their multi-agent
                // backend, as the program's manifest names the role `spawn_agent`; the bare name is refused
                // as an unsupported call and no child is ever started.
                format!(
                    "#!mock\ntool multi_agent_v1.spawn_agent {}\nthen\nsay Waiting for Review.\n",
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
            child_begun,
        };
        phase("initial-native-turn-start");
        result
            .native_control(
                "initial native turn/start reply",
                NativeControl::Start(Box::new(turn)),
            )
            .await;
        phase("initial-native-turn-started");
        result.state.config_off = result.native.config_off_at_launch();
        if streaming {
            // The model server holds the parent's last message before it says a word, so that the parent's turn cannot
            // end before the case has seen the first block of the sub-agent's message: the order is the case's, not a
            // matter of time.
            result
                .pump_until(|state| {
                    state
                        .trace
                        .iter()
                        .any(|line| line.starts_with("assistant/") && !line.contains("parent=-"))
                })
                .await;
            result.native.release("parent-message");
        }
        result
            .pump_until(|state| state.foreground_done && (!wake || !state.running.is_empty()))
            .await;
        result.world.wait_turn(origin, "completed").await;
        result
    }

    /// Lets the sub-agent's first message go on from where the model server holds it (the streaming flow of
    /// [`Bridge::start_streaming_child`]): the case does it once it has seen the parent's result, so that the message is
    /// unended at that result because of the order of events and not because of how long anything takes.
    pub fn release_the_child(&self) {
        self.native.release("child-message");
    }

    /// Whether the sub-agent's command was allowed and has begun (the case has none without a sub-agent).
    pub fn child_command_begun(&self) -> bool {
        self.child_begun.as_ref().is_some_and(|file| file.exists())
    }

    /// The commands a person has allowed: the questions of the program that were put to a person and answered yes.
    pub async fn allowed_commands(&self) -> Vec<String> {
        let requests: Vec<orch_types::Json> = sqlx::query_scalar(
            "SELECT request FROM approvals WHERE thread_id=$1 AND state='approved' ORDER BY id",
        )
        .bind(self.thread)
        .fetch_all(self.world.pool.pool())
        .await
        .unwrap();
        requests
            .into_iter()
            .filter_map(|request| serde_json::from_value::<HarnessRequest>(request).ok())
            .filter_map(|request| match request.detail {
                orch_api_contract::approvals::HarnessRequestDetail::Command(command) => {
                    Some(command.command)
                }
                _ => None,
            })
            .collect()
    }

    /// How many questions of the program were put to a person, whatever became of them.
    pub async fn questions_asked(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM approvals WHERE thread_id=$1")
            .bind(self.thread)
            .fetch_one(self.world.pool.pool())
            .await
            .unwrap()
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
        let answering = match &control {
            NativeControl::Approve(body) => Some(body.request_ref.clone()),
            _ => None,
        };
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
        let result = bounded(phase, async {
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
        .await;
        if let (Err(AdapterError::NotFound(_)), Some(request)) = (&result, &answering) {
            // A person's answer can come after the program took the question back (the turn it was asked in
            // ended): the engine hears of that from the event that follows the refusal, and an answer it had
            // already handed on is an undelivered decision, which it cancels with a word to the person. The event
            // is in the stream by now (the adapter sends it before it takes up the answer): a question that was
            // never taken back is still a failure.
            while let Some(Some(event)) = events.next().now_or_never() {
                observation.accept(event).await;
            }
            if observation.state.taken_back.contains(request) {
                self::phase("native-approval-late");
                return;
            }
        }
        result.unwrap_or_else(|error| panic!("native control failed during {phase}: {error}"));
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
            let reference = approval.native_ref.clone();
            let request: HarnessRequest = serde_json::from_value(approval.request).unwrap();
            let choice = request
                .options
                .iter()
                .find(|option| {
                    option.style == OptionStyle::Allow && option.scope == Some(OptionScope::Once)
                })
                .expect("the native tool question offers an explicit one-request approval");
            let decided = self
                .world
                .engine()
                .decide_approval(
                    &Caller::session(self.world.ada),
                    approval.id,
                    &ApprovalDecision::Option(HarnessOptionDecision {
                        option_id: choice.id.clone(),
                        note: None,
                    }),
                )
                .await;
            match decided {
                Ok(_) => {}
                // The program took the question back, and the engine heard of it, between the look and the decision:
                // the event that takes it back is seen here before the engine hears of it.
                Err(ApprovalError::AlreadyDecided)
                    if reference
                        .as_ref()
                        .is_some_and(|reference| self.state.taken_back.contains(reference)) =>
                {
                    phase("native-approval-taken-back");
                }
                Err(error) => panic!("the person's decision was refused: {error}"),
            }
        }
        for command in self.world.seen.of_kind(CommandKind::ApprovalResolve) {
            let Command::ApprovalResolve(body) = command else {
                continue;
            };
            if body.thread_id == self.thread && self.resolved.insert(body.request_ref.clone()) {
                phase("native-approval-reply");
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
                phase("native-interrupt-reply");
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
            phase("continuation-native-turn-start");
            self.native_control(
                "continuation native turn/start reply",
                NativeControl::Start(Box::new(turn)),
            )
            .await;
        }
    }

    /// The holds of the model server's scripts that expired for want of a release, by the server's record, in words: a
    /// stream that waited at one ended with an error naming it, and the program may have retried into the same hold, so
    /// that the case would otherwise end at its outer bound with nothing to say why.
    fn expired_holds(&self) -> String {
        self.native
            .requests()
            .iter()
            .filter_map(|request| {
                request.hold_expired.as_ref().map(|name| {
                    format!(
                        "; THE HOLD {name:?} EXPIRED in request {} (nobody released it within the model server's limit)",
                        request.seq
                    )
                })
            })
            .collect()
    }

    fn counters(&self) -> String {
        format!(
            "foreground_done={}, running={}, finished={}, completed={}, checked_markers={}{}",
            self.state.foreground_done,
            self.state.running.len(),
            self.state.finished.len(),
            self.state.completed.len(),
            self.state.checked_markers.len(),
            self.expired_holds()
        )
    }

    pub async fn pump_until(&mut self, done: impl Fn(&State) -> bool) {
        let mut phase = "command dispatch";
        let result = tokio::time::timeout(WITHIN, async {
            let mut poll = tokio::time::interval(Duration::from_millis(20));
            let mut reported = Instant::now();
            loop {
                if done(&self.state) {
                    return;
                }
                if reported.elapsed() >= Duration::from_secs(15) {
                    reported = Instant::now();
                    eprintln!(
                        "native background proof still waiting t={}s: {}\n  {}",
                        clock(),
                        self.counters(),
                        self.request_digest()
                    );
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
            "native background pump timed out during {phase}; {}",
            self.counters()
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

    /// What the accounting holds for the turns of the case, what the native program reported, and the frames
    /// that bear on it: the facts a failed comparison of counts needs.
    pub async fn accounting_facts(&self) -> String {
        let mut lines = Vec::new();
        for (name, turn) in [("origin", Some(self.origin)), ("wake", self.wake)] {
            let Some(turn) = turn else { continue };
            for row in self.world.ledger_of(turn).await {
                lines.push(format!(
                    "ledger {name} request={:?} counted={} subagent={} input={} cached={} output={} total={} outcome={:?}",
                    row.request_ref,
                    row.counted,
                    row.timing().subagent,
                    row.input_tokens,
                    row.cached_input_tokens,
                    row.output_tokens,
                    row.total_tokens,
                    row.outcome
                ));
            }
        }
        for (turn, request) in &self.state.requests {
            let name = if *turn == self.origin {
                "origin"
            } else if Some(*turn) == self.wake {
                "wake"
            } else {
                "other"
            };
            lines.push(format!(
                "reported {name} id={} subagent={} input={:?} output={:?} total={:?} outcome={:?}",
                request.id,
                request.subagent,
                request.tokens.input_tokens,
                request.tokens.output_tokens,
                request.tokens.total_tokens,
                request.outcome.as_ref().map(RequestOutcome::code)
            ));
        }
        lines.extend(self.state.trace.iter().map(|line| format!("frame {line}")));
        if let Harness::Claude(rig) = &self.native {
            lines.extend(transcript_usage(rig.home.path()));
        }
        lines.join("\n  ")
    }

    /// The model requests the native program made, in order: which script answered each, and the tail of
    /// what the last ones carried.
    pub fn request_digest(&self) -> String {
        fn words(item: &Json) -> String {
            let text = match &item["content"] {
                Json::String(text) => text.clone(),
                Json::Array(parts) => parts
                    .iter()
                    .map(|part| {
                        part["text"]
                            .as_str()
                            .or(part["content"].as_str())
                            .map_or_else(|| part["type"].to_string(), str::to_owned)
                    })
                    .collect::<Vec<_>>()
                    .join(" "),
                _ => item["output"]
                    .as_str()
                    .or(item["arguments"].as_str())
                    .unwrap_or_default()
                    .to_owned(),
            };
            format!(
                "{}/{}: {}",
                item["type"].as_str().unwrap_or("-"),
                item["role"].as_str().unwrap_or("-"),
                text.chars().take(100).collect::<String>()
            )
        }
        let requests = self.native.requests();
        let last = requests.len().saturating_sub(3);
        let mut lines = Vec::new();
        for (at, request) in requests.iter().enumerate() {
            let items = request.body["input"]
                .as_array()
                .or(request.body["messages"].as_array());
            let tools: Vec<&str> = request.body["tools"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|tool| tool["name"].as_str().or(tool["function"]["name"].as_str()))
                .collect();
            lines.push(format!(
                "request {} {} round={} script={:?} status={:?} items={} tools={}",
                request.seq,
                request.path,
                request.round,
                request.script,
                request.status,
                items.map_or(0, Vec::len),
                if at == 0 {
                    tools.join(",")
                } else {
                    tools.len().to_string()
                }
            ));
            if at >= last {
                let items: Vec<&Json> = items.into_iter().flatten().collect();
                for item in items.iter().skip(items.len().saturating_sub(6)) {
                    lines.push(format!("    {}", words(item)));
                }
            }
        }
        lines.extend(
            self.expired_holds()
                .split("; ")
                .filter(|each| !each.is_empty())
                .map(str::to_owned),
        );
        lines.join("\n  ")
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
        phase("accounting-follow-up");
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
        phase("native-stop");
        self.native_control("native adapter stop", NativeControl::Stop)
            .await;
        phase("engine-shutdown");
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
                    SessionEvent::ApprovalClosed(closed) => {
                        self.state.taken_back.insert(closed.request_ref.clone());
                    }
                    SessionEvent::RawFrame(raw) => {
                        if let Some(line) = frame_line(raw)
                            && self.state.trace.len() < 600
                        {
                            eprintln!("native background proof frame t={}s {line}", clock());
                            self.state.trace.push(line);
                        }
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
