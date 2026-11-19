//! Scripted adapter for product behavior exercises and UI development.
//!
//! The adapter of kind `fake` drives no harness. It answers an input as a script says, with the same
//! neutral events, in the same order and at the same pace as a harness would, so that the product can be
//! exercised end to end, and an interface developed, without a model. It is exact and deterministic:
//! what an input says is what happens, the pauses are fixed, and nothing is random.
//!
//! # An input
//!
//! Without a script the answer is `You said: <text>`, streamed in three pieces of equal size, one every
//! 100 ms, so that it takes 300 ms, and the turn completes.
//!
//! When the first line of the input is `#!fake`, each following line is a directive, run in order. A
//! blank line and a line that begins with `#` are skipped. The script is read whole before anything
//! runs; a mistake stops the turn at once with an `error` item that names its line, and the turn fails
//! with the code `harness_error`. In the free text of a directive `\n` is a line break and `\\` is a
//! backslash (a directive is one line); where `|` or `;` separate, `\|` and `\;` are the characters.
//!
//! Every turn is `turn.started`, the items and events its directives make, and `turn.completed`.
//! An item streams as `item.started` (empty), `item.delta`s and `item.completed`; pieces are 40
//! characters (bytes, for the output of a command) and each is followed by a pause of 100 ms, so a
//! text of *n* pieces takes *n*×100 ms. An interrupt ends the turn at once with the item in the making
//! completed as `cancelled` with what it had, and `turn.completed` with the status `interrupted`.
//!
//! | Directive | What happens |
//! |-----------|--------------|
//! | `say <text>` | An agent message, 40 characters per 100 ms. Its phase is `final` for the last `say` of the script and `commentary` for the others |
//! | `think <text>` | A reasoning item, its summary streamed like a message |
//! | `plan <a> \| <b> \| <c>` | One plan item: the first step `in_progress`, the others `pending`. Each directive that runs after it completes the step in progress and starts the next (`item.updated`); the item completes with the last step, or with the turn |
//! | `run <command> => <exit code> <output>` | A command tool call, simulated: the output streams in pieces of 40 bytes (larger pieces beyond 2 000 bytes, never more than 50). Exit code 0 completes the item, any other fails it. An output over 16 KiB is kept whole in `<home>/outputs/<hash>` and cited as `output_blob`. `=> …` may be left out: exit code 0, no output |
//! | `edit <path> +<added> -<removed>` | A file-change tool call: begun without a diff, completed 100 ms later with a unified diff of that many lines and three lines of context |
//! | `mcp <server> <tool> <json>` | An MCP tool call. It completes with `{"content":[{"type":"text","text":"<server>.<tool> ok"}],"isError":false}` if the session was given a server of that name, and fails with a message if it was not |
//! | `ask command <command>` | A request for approval (`once`, `session`, `deny`); the turn waits for as long as it takes. Allowed, the command runs (exit code 0); `session` is remembered for the rest of the session; denied, its tool call is `cancelled` and has not run |
//! | `ask input <question>` | A request with one free question, `q1`; the turn waits. The answer is then said back: `You answered: <answer>` |
//! | `sub <name>: <directive>; <directive>…` | A sub-agent tool call in the main stream, and the directives run in the stream `sub:<name>` as its items; the call ends `completed`, `failed` or `interrupted` with them. `plan` and `sub` are not for a sub-agent |
//! | `announce <title> \| <body>` | A call of `announce` |
//! | `milestone <title>` | A call of `announce` with the category `milestone` |
//! | `capability <key> <reason>` | A call of `request_capability` |
//! | `shared [<path>]` | A call of `list_shared` |
//! | `read <path> [<from> <to> <detail>]` | A call of `read_shared`; `detail` is `brief`, `normal` or `full` |
//! | `search <query>` | A call of `search_shared` |
//! | `download <path>` | A call of `download_shared` |
//! | `doc <command> <path> \| <markdown>` | A call of `edit_document` with `command`, `path` and `markdown` |
//! | `chat <path> \| <text> [\| <file>…]` | A call of `send_message` (chat target, `text`, `files`) |
//! | `tell <thread> \| <text> [\| <file>…]` | A call of `send_message` (thread target, `text`, `files`) |
//! | `manage <command> <path> [\| <path>…] [=> <to>]` | Calls `manage_shared`: `command` is `create_folder`, `copy`, `move`, `delete` or `restore`; `=> <to>` names an existing copy/move destination. Only delete previews first and returns a token. Other commands execute directly |
//! | `confirm` | Confirms a deletion with the same arguments and the last preview's token (or its refreshed token after `not_confirmed`). A repeated confirm tests the used token. Says so and calls nothing when the preceding act supplied no confirmation token |
//! | `remember <text>` | A note for later, kept as the session's `memory` says (see below) |
//! | `effort` | Says `effort: <the effort the turn was started with, or none>` |
//! | `sleep <milliseconds>` | Waits; an interrupt ends it |
//! | `usage <input> <output>` | `usage.updated`: the turn has used that much (it replaces an earlier `usage` of the turn); the thread's use is the turns before plus this one; the window is the context the turn was started with, else the model's own window it was given, else 200 000 tokens |
//! | `exhaust [<minutes>]` | Fails the turn with `provider_exhausted` (not retryable) and the moment the plan is whole again, that many minutes from now (the plan's window, 300, without a number); the provider's facts say the plan is used up until then, and the host is told so |
//! | `limit <part> [<minutes>]` | Fails the turn with `provider_exhausted` for one part of the plan only, a family of models: retryable, since another model may answer, and with the moment it ends. The plan is not used up and its facts do not change |
//! | `busy [<said>]` | Fails the turn with `provider_busy` (retryable); the text is what the vendor said |
//! | `unreachable [<said>]` | Fails the turn with `provider_unreachable` (retryable); the text is what the vendor said |
//! | `cut <text>` | Streams the first half of the text as an agent message and then the stream breaks: the item ends `cancelled` with what it had, and the turn fails with `provider_unreachable` (retryable), marked as one that may have cut the answer short |
//! | `fail <code> <message> [\| <said>]` | Fails the turn with that code and message (not retryable), and the vendor's words if there are any |
//! | `lose` | Emits `session.lost` and ends the turn without `turn.completed`; the session is no longer open |
//! | `expire` | Treats the credential it was given as rejected: emits `credential.needed` (`rejected`) and waits until another is set |
//!
//! Each of Orch's tools is called by its directive: the call is an `orch.tool_call` event and an
//! item of kind `orch` (the harness's own name for it is `mcp__orch__<tool>`), the turn waits for
//! `orch.tool_result`, and the item completes with it; a refusal completes it as `failed`. With no
//! result within 120 s the item fails with `Orch is unreachable; try again later`.
//!
//! Beyond the table, the fake has a few directives that make what reached it, and what it can do,
//! visible: `model` (says `model: <name>`), `mode` (`approval: <mode>, sandbox: <mode>`), `params`
//! (`params: <json>`: the service tier, the access program and the context the turn was started with,
//! and the model's own window, as far as there are any), `where` (`directory: <d>, cwd: <c>`), `notes` (`notes: <n> loaded: <a>; <b>`, or
//! `notes: none`), `title <text>` (`session.title`), `compact [<ms>]` (`turn.phase` `compacting`, a
//! pause, `working`; a fake that does not report phases sends the `compacted` marker after instead),
//! `compacted` (the `compacted` marker, with no duration), and `crash` (the harness process dies: the
//! adapter reports `harness.status` `error`, every session is gone, and the turn ends without a word).
//!
//! # Sessions
//!
//! A session is a file of the harness's home, `sessions/<native session ref>.json` (`fake-v1`), written
//! as items end. Native refs are `fake_1`, `fake_2`… numbered on from where the home was left. Resuming
//! reads the file; `fork` copies it through `upto_turn_ordinal`; `import_native` reads a `fake-v1`
//! export, `import_neutral` the neutral history (JSON Lines of items, main stream, oldest first);
//! `session.export` writes either to `exports/`. `session.resync` says again `item.completed` for
//! every item of the file and `turn.completed` for every turn. Two sessions of one home are two files
//! and see nothing of each other.
//!
//! # Notes
//!
//! `remember` follows the session's `memory`. `null` (the setting is off): nothing is kept, and the fake
//! says so. Kept by the `harness` and the profile's `memory` is `directory`: a line `- <text>` is
//! appended to `MEMORY.md` of the directory, and the fake says `Noted: <text>`; the next session in that
//! directory reads the lines back when it opens (`notes` shows them). Kept by the `agent`: the same
//! line is written, and the write is a file-change tool call, as the agent's own tool would make it. A
//! profile whose `memory` is `home` (or `none`) never writes: it says why it did not.
//!
//! # The profile
//!
//! The `config` of the agent profile switches parts of what the fake declares and does ([`Profile`]):
//!
//! | Member | Meaning |
//! |--------|---------|
//! | `separable` | `false` removes `separated` and `key_separated` |
//! | `new_provider_allows` | what a new provider starts with (`["separated", "key_separated"]`) |
//! | `memory` | `directory` (default), `home` or `none` |
//! | `capabilities` | a patch merged over the capabilities (`{"steer": false}`; `null` removes a member) |
//! | `separated` | the `separated` entries (`[{"executor", "executor_version", "tool_sets"}]`), instead of `orch-exec` in its three tool sets |
//! | `credentials`, `approval_modes`, `sandbox_modes`, `forwarded_protocols`, `harness_version` | instead of the fake's own |
//! | `code_lifetime_secs` | how long the code of `fake_code` is good for (600) |
//! | `mcp_servers` | the tool servers the harness has configured here (`[{"name", "command" or "url", "env", "secrets"}]`) |
//! | `faults` | failures to inject: `{"start_turn": {"code": "harness_error", "message": "…", "times": 1, "after": 0}}`; the calls are [`FAULT_CALLS`], `times: 0` fails every call |
//!
//! # Credentials and sign-in
//!
//! The forms are `fake_token` (a copy that lapses after 120 s), `api_key` (a copy that lasts) and
//! `fake_login` (the login itself, which moves and is unusable at home until it is returned). The
//! sign-in methods are `fake_code` (an address and a code; it completes when `provider.login.input` is
//! sent with any values, and expires by itself after `code_lifetime_secs`), `fake_ask` (an address,
//! then asks for `answer`) and `fake_import` (a credential's text or its path, and a field of every
//! type; its token ends at `expires_at` or after 600 s). The provider option is `tenant`.
//!
//! # What it does not do yet
//!
//! A turn on the `forwarded` path makes no model call through the connector's listener, and `run` and
//! `mcp` are always simulated: the connector's endpoints for the model, the executor and the tool
//! servers arrive with the provider and environment roles.

#![forbid(unsafe_code)]

pub mod credentials;
pub mod descriptor;
pub mod events;
pub mod script;
pub mod sessions;

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_agent_contract::descriptor::{AgentDescriptor, HarnessSupport};
use orch_agent_contract::discovery::{
    McpDefinition, McpFound, McpImportRequest, McpScope, McpSearch, McpServerSpec, ModelSuggestion,
    NativeSession,
};
use orch_agent_contract::events::{ItemCompleted, SessionEvent, TurnCompleted};
use orch_agent_contract::facts::{HarnessState, HarnessStatus, ProviderFacts};
use orch_agent_contract::history::{ExportKind, SessionArchive};
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, SessionMode, SessionOpened, SessionSpec, ThreadId, ToolError, TurnId,
    TurnStart, TurnSteer,
};
use orch_plugin_contract::Secret;
use orch_types::{Clock, Id, Json};
use tokio::sync::Notify;
use tokio_util::sync::CancellationToken;

pub use descriptor::{KIND, Profile};
pub use events::{FAULT_CALLS, Fault};

use credentials::{Credentials, Signin};
use events::{Faults, lock};
use sessions::{ActiveTurn, Home, LiveSession, read_notes};

/// What the turns of a running fake share with the adapter.
pub(crate) struct Shared {
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) home: Home,
    pub(crate) profile: Profile,
    pub(crate) descriptor: AgentDescriptor,
    pub(crate) harness_version: String,
    pub(crate) sink: EventSink,
    pub(crate) sessions: Mutex<HashMap<ThreadId, Arc<LiveSession>>>,
    pub(crate) credentials: Mutex<Credentials>,
    pub(crate) credentials_changed: Notify,
    pub(crate) signin: Signin,
    pub(crate) faults: Arc<Faults>,
    pub(crate) alive: AtomicBool,
}

impl Shared {
    pub(crate) fn credentials_lock(&self) -> MutexGuard<'_, Credentials> {
        lock(&self.credentials)
    }

    /// Tells the host that what the provider reports changed.
    pub(crate) async fn facts_changed(&self) {
        let facts = self.credentials_lock().facts(self.clock.time());
        if let Ok(facts) = facts {
            let _ = self.sink.emit(AdapterEvent::Facts(facts)).await;
        }
    }

    /// The harness process died: nothing it held is left, and the host is told.
    pub(crate) async fn crash(&self) {
        self.alive.store(false, Ordering::Relaxed);
        let sessions: Vec<Arc<LiveSession>> =
            lock(&self.sessions).drain().map(|(_, s)| s).collect();
        for session in sessions {
            if let Some(turn) = lock(&session.turn).as_ref() {
                turn.cancel.cancel();
            }
        }
        script::report_crash(&self.sink, &self.harness_version).await;
    }

    fn session(&self, thread: ThreadId) -> Result<Arc<LiveSession>, AdapterError> {
        lock(&self.sessions)
            .get(&thread)
            .cloned()
            .ok_or_else(|| AdapterError::SessionNotFound(thread.to_string()))
    }

    fn running(&self) -> Result<(), AdapterError> {
        if self.alive.load(Ordering::Relaxed) {
            Ok(())
        } else {
            Err(AdapterError::HarnessUnavailable(
                "the fake harness is not running".to_owned(),
            ))
        }
    }

    /// Ends the turn of a session, if one runs, and waits until it has ended.
    async fn end_turn(session: &LiveSession) {
        let task = {
            let mut active = lock(&session.turn);
            active.as_mut().and_then(|turn| {
                turn.cancel.cancel();
                turn.task.take()
            })
        };
        if let Some(task) = task {
            let _ = task.await;
        }
    }
}

/// The scripted adapter.
pub struct FakeAdapter {
    program: descriptor::Program,
    clock: Arc<dyn Clock>,
    running: Option<Running>,
    faults: Option<(BTreeMap<String, Fault>, Arc<Faults>)>,
}

struct Running {
    shared: Arc<Shared>,
    /// The profile this harness was started with: a start with another is a start anew.
    config: Json,
    requested_version: Option<String>,
    started: AgentStarted,
}

impl std::fmt::Debug for FakeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeAdapter").finish_non_exhaustive()
    }
}

impl FakeAdapter {
    /// A fake adapter that reads the time from `clock`.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_program(clock, descriptor::Program::shipped())
    }

    /// A fake adapter that is the program `program` names: a program whose manifest says it is driven by the
    /// scripted adapter needs no code of its own.
    #[must_use]
    pub fn with_program(clock: Arc<dyn Clock>, program: descriptor::Program) -> Self {
        Self {
            program,
            clock,
            running: None,
            faults: None,
        }
    }

    fn shared(&self) -> Result<&Arc<Shared>, AdapterError> {
        let running = self.running.as_ref().ok_or_else(|| {
            AdapterError::HarnessUnavailable("the fake harness has not been started".to_owned())
        })?;
        running.shared.running()?;
        Ok(&running.shared)
    }

    fn call(&self, call: &str) -> Result<Arc<Shared>, AdapterError> {
        let shared = Arc::clone(self.shared()?);
        shared.faults.check(call)?;
        Ok(shared)
    }

    async fn teardown(&mut self) {
        if let Some(running) = self.running.take() {
            let sessions: Vec<Arc<LiveSession>> = lock(&running.shared.sessions)
                .drain()
                .map(|(_, session)| session)
                .collect();
            for session in sessions {
                Shared::end_turn(&session).await;
            }
            running.shared.signin.cancel().await;
        }
    }
}

#[async_trait]
impl AgentAdapter for FakeAdapter {
    fn kind(&self) -> &'static str {
        self.program.kind
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        let profile = Profile::from_config(&start.profile.config)?;
        let faults = match &self.faults {
            Some((table, faults)) if *table == profile.faults => Arc::clone(faults),
            _ => {
                let faults = Arc::new(Faults::new(&profile.faults)?);
                self.faults = Some((profile.faults.clone(), Arc::clone(&faults)));
                faults
            }
        };
        faults.check("start")?;
        if let Some(running) = &self.running
            && running.config == start.profile.config
            && running.requested_version == start.profile.harness_version
            && running.shared.alive.load(Ordering::Relaxed)
        {
            return Ok(running.started.clone());
        }
        self.teardown().await;
        let home = match &start.home {
            Some(path) => PathBuf::from(path),
            None => std::env::temp_dir().join("orch-fake-home"),
        };
        let home = Home::open(&home)?;
        let version = start
            .profile
            .harness_version
            .clone()
            .or_else(|| profile.harness_version.clone())
            .unwrap_or_else(|| descriptor::HARNESS_VERSION.to_owned());
        let declared = descriptor::descriptor_of(&self.program, &profile, &version)?;
        if declared.harness.status == HarnessSupport::Unsupported {
            return Err(AdapterError::HarnessUnavailable(format!(
                "the fake harness {version} is older than the range {} that this adapter drives",
                descriptor::SUPPORTED_RANGE
            )));
        }
        let signin = Signin::new(
            sink.clone(),
            Arc::clone(&self.clock),
            profile.code_lifetime_secs,
        );
        let shared = Arc::new(Shared {
            clock: Arc::clone(&self.clock),
            home,
            profile,
            descriptor: declared.clone(),
            harness_version: version.clone(),
            sink,
            sessions: Mutex::new(HashMap::new()),
            credentials: Mutex::new(Credentials::default()),
            credentials_changed: Notify::new(),
            signin,
            faults,
            alive: AtomicBool::new(true),
        });
        let started = AgentStarted {
            harness_version: version,
            descriptor: declared,
        };
        self.running = Some(Running {
            shared,
            config: start.profile.config,
            requested_version: start.profile.harness_version,
            started: started.clone(),
        });
        Ok(started)
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.teardown().await;
        Ok(())
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        Ok(self.shared()?.descriptor.clone())
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        let shared = self.call("open_session")?;
        // An aside is opened only by a reading mechanism its profile declares.
        if let Some(reading) = spec.settings.side_reading
            && !shared
                .profile
                .side_thread_modes
                .as_deref()
                .unwrap_or_default()
                .contains(&reading)
        {
            return Err(AdapterError::Unsupported(format!(
                "this fake declares no side conversation that reads by {reading:?}"
            )));
        }
        let mode = spec.settings.mode;
        if mode == SessionMode::Resume {
            shared.faults.check("resume")?;
        }
        let now = shared.clock.time();
        let through = spec.settings.upto_turn_ordinal;
        let wanted = spec.settings.native_session_ref.clone();
        let need_ref = || {
            wanted.clone().ok_or_else(|| {
                AdapterError::NotFound(format!("a session opened by {mode:?} names the session"))
            })
        };
        let (file, resumed, report) = match mode {
            SessionMode::New => (shared.home.create(&spec, now)?, false, None),
            SessionMode::Resume | SessionMode::Adopt => {
                let mut file = shared.home.load(&need_ref()?)?;
                shared.home.reopened_as(&mut file, &spec);
                (file, true, None)
            }
            SessionMode::Fork => (
                shared.home.fork(&need_ref()?, through, &spec, now)?,
                false,
                None,
            ),
            SessionMode::ImportNative => {
                let archive = spec.archive.as_ref().ok_or_else(|| {
                    AdapterError::ImportNativeFailed("no archive was given".to_owned())
                })?;
                let (file, report) = shared.home.import_native(
                    std::path::Path::new(&archive.path),
                    through,
                    &spec,
                    now,
                )?;
                (file, false, Some(report))
            }
            SessionMode::ImportNeutral => {
                let history = spec
                    .history
                    .as_ref()
                    .ok_or_else(|| AdapterError::HarnessError("no history was given".to_owned()))?;
                let (file, report) = shared.home.import_neutral(
                    std::path::Path::new(&history.path),
                    through,
                    &spec,
                    now,
                )?;
                (file, false, Some(report))
            }
        };
        // One live session for a thread: an earlier one is closed first.
        if let Ok(earlier) = shared.session(spec.settings.thread_id) {
            Shared::end_turn(&earlier).await;
            lock(&shared.sessions).remove(&spec.settings.thread_id);
        }
        let notes = spec
            .settings
            .memory
            .as_ref()
            .filter(|memory| {
                memory.kept_by == orch_agent_contract::session::NoteKeeper::Harness
                    && shared.profile.memory
                        == orch_agent_contract::descriptor::MemoryKind::Directory
            })
            .map(|memory| read_notes(&memory.directory))
            .unwrap_or_default();
        shared.home.save(&file)?;
        let native_session_ref = file.native_session_ref.clone();
        let live = LiveSession::new(shared.home.clone(), file, spec, notes);
        lock(&shared.sessions).insert(live.thread_id, Arc::clone(&live));
        events::announce(
            &shared.sink,
            live.thread_id,
            live.segment_id,
            events::session_opened(&native_session_ref, resumed),
        )
        .await;
        Ok(SessionOpened {
            native_session_ref,
            resumed,
            import_report: report,
            instruction_sources: Vec::new(),
            approval_reviewer: None,
        })
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        let shared = self.call("close_session")?;
        let Ok(session) = shared.session(thread) else {
            return Ok(());
        };
        Shared::end_turn(&session).await;
        lock(&shared.sessions).remove(&thread);
        Ok(())
    }

    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        let shared = self.call("start_turn")?;
        let session = shared.session(turn.thread_id)?;
        let mut active = lock(&session.turn);
        if active.is_some() {
            return Err(AdapterError::Other {
                code: "conflict".to_owned(),
                message: "a turn is already running in this session".to_owned(),
                retryable: false,
            });
        }
        let ordinal = session.turns_so_far() + 1;
        let cancel = CancellationToken::new();
        let turn_id = turn.turn_id;
        let task = tokio::spawn(script::run_turn(
            Arc::clone(&shared),
            Arc::clone(&session),
            turn,
            ordinal,
            cancel.clone(),
        ));
        *active = Some(ActiveTurn {
            turn_id,
            cancel,
            task: Some(task),
        });
        Ok(())
    }

    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        let shared = self.call("steer")?;
        let session = shared.session(steer.thread_id)?;
        let running = lock(&session.turn)
            .as_ref()
            .is_some_and(|turn| turn.turn_id == steer.turn_id);
        if !running {
            return Err(AdapterError::Other {
                code: "conflict".to_owned(),
                message: "that turn is not running".to_owned(),
                retryable: false,
            });
        }
        lock(&session.steers).push_back(steer.text);
        session.steer_notify.notify_one();
        Ok(())
    }

    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        let shared = self.call("interrupt")?;
        let session = shared.session(thread)?;
        if let Some(active) = lock(&session.turn).as_ref()
            && active.turn_id == turn
        {
            active.cancel.cancel();
        }
        let ids: Vec<_> = lock(&session.background)
            .values()
            .filter(|command| command.turn_id == Some(turn))
            .map(|command| command.id.clone())
            .collect();
        let mut cleanup = orch_agent_contract::background::CommandCleanup::default();
        for id in ids {
            if scripted_background_end(&shared, &session, &id).await.state
                == orch_agent_contract::background::BackgroundEndState::Ended
            {
                cleanup.ended += 1;
            } else {
                cleanup.unknown += 1;
            }
        }
        Shared::end_turn(&session).await;
        let _ = shared
            .sink
            .session(
                thread,
                session.segment_id,
                orch_agent_contract::events::SessionEvent::TurnCommandCleanup(
                    orch_agent_contract::events::TurnCommandCleanup {
                        turn_id: turn,
                        result: cleanup,
                    },
                ),
            )
            .await;
        Ok(())
    }

    async fn background_commands(
        &mut self,
        thread: ThreadId,
    ) -> Result<orch_agent_contract::background::NativeBackground, AdapterError> {
        let shared = self.shared()?;
        shared.running()?;
        let session = shared.session(thread)?;
        let mut items: Vec<_> = lock(&session.background).values().cloned().collect();
        items.sort_by(|left, right| left.id.cmp(&right.id));
        let total = i64::try_from(items.len()).unwrap_or(i64::MAX);
        Ok(orch_agent_contract::background::NativeBackground {
            supported: shared.descriptor.capabilities.background_commands,
            as_of: Some(shared.clock.time()),
            state: orch_agent_contract::background::BackgroundState::Live,
            reason: None,
            items,
            next_cursor: None,
            total: Some(total),
        })
    }

    async fn end_background_command(
        &mut self,
        thread: ThreadId,
        command: &str,
    ) -> Result<orch_agent_contract::background::NativeBackgroundEnd, AdapterError> {
        let shared = self.shared()?;
        shared.running()?;
        let session = shared.session(thread)?;
        if !shared.descriptor.capabilities.background_commands {
            return Err(AdapterError::Unsupported("background_commands".to_owned()));
        }
        Ok(scripted_background_end(shared, &session, command).await)
    }

    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let shared = self.call("resolve_approval")?;
        let session = shared.session(thread)?;
        let mut approvals = lock(&session.approvals);
        let pending = approvals
            .get(request_ref)
            .ok_or_else(|| AdapterError::NotFound(format!("no request {request_ref} waits")))?;
        script::decision_fits(&pending.request, &decision).map_err(|why| AdapterError::Other {
            code: "invalid".to_owned(),
            message: why,
            retryable: false,
        })?;
        if let Some(pending) = approvals.remove(request_ref) {
            let _ = pending.reply.send(decision);
        }
        Ok(())
    }

    async fn tool_result(
        &mut self,
        thread: ThreadId,
        call_ref: &str,
        result: Result<Json, ToolError>,
    ) -> Result<(), AdapterError> {
        let shared = self.call("tool_result")?;
        let session = shared.session(thread)?;
        let waiting = lock(&session.tool_calls).remove(call_ref).ok_or_else(|| {
            AdapterError::NotFound(format!("no call {call_ref} waits for a result"))
        })?;
        let _ = waiting.send(result);
        Ok(())
    }

    async fn export_session(
        &mut self,
        thread: ThreadId,
        kind: ExportKind,
        upto_turn_ordinal: Option<u32>,
    ) -> Result<SessionArchive, AdapterError> {
        let shared = self.call("export")?;
        let session = shared.session(thread)?;
        let file = session.snapshot();
        let (path, size, format) = match kind {
            ExportKind::Native => {
                let (path, size) = shared.home.export_native(&file, upto_turn_ordinal)?;
                (path, size, sessions::NATIVE_FORMAT.to_owned())
            }
            ExportKind::Neutral => {
                let (path, size) = shared.home.export_neutral(&file, upto_turn_ordinal)?;
                (
                    path,
                    size,
                    orch_agent_contract::history::NEUTRAL_FORMAT.to_owned(),
                )
            }
        };
        Ok(SessionArchive {
            format,
            path: path.display().to_string(),
            size,
        })
    }

    async fn resync_session(&mut self, thread: ThreadId) -> Result<u32, AdapterError> {
        let shared = self.call("resync")?;
        let session = shared.session(thread)?;
        let file = session.snapshot();
        let mut said = 0u32;
        let mut told: Vec<SessionEvent> = Vec::new();
        let mut tell = |event: SessionEvent| told.push(event);
        let say_items = |ordinal: u32, tell: &mut dyn FnMut(SessionEvent), said: &mut u32| {
            for item in file
                .items
                .iter()
                .filter(|item| item.turn_ordinal == ordinal)
            {
                tell(SessionEvent::ItemCompleted(ItemCompleted {
                    native_ref: item.native_ref.clone(),
                    status: item.status,
                    payload: item.payload.clone(),
                    output_blob: item.output_blob.clone(),
                }));
                *said += 1;
            }
        };
        for turn in &file.turns {
            say_items(turn.ordinal, &mut tell, &mut said);
            tell(SessionEvent::TurnCompleted(TurnCompleted {
                turn_id: turn.turn_id,
                status: turn.status,
                error: turn.error.clone(),
            }));
        }
        let last = file.turns.last().map_or(0, |turn| turn.ordinal);
        let mut running: Vec<u32> = file
            .items
            .iter()
            .map(|item| item.turn_ordinal)
            .filter(|ordinal| *ordinal > last)
            .collect();
        running.sort_unstable();
        running.dedup();
        for ordinal in running {
            say_items(ordinal, &mut tell, &mut said);
        }
        for event in told {
            let _ = shared
                .sink
                .session(session.thread_id, session.segment_id, event)
                .await;
        }
        Ok(said)
    }

    async fn discover_sessions(&mut self) -> Result<Vec<NativeSession>, AdapterError> {
        let shared = self.call("discover_sessions")?;
        shared.home.list()
    }

    async fn read_session(
        &mut self,
        native_session_ref: &str,
        write: bool,
    ) -> Result<orch_agent_contract::discovery::SessionRecord, AdapterError> {
        use orch_agent_contract::discovery::{
            RECORD_FORMAT, RECORD_MAX_BYTES, RecordedEntry, RecordedItem, RecordedWords,
            SessionRecord, record_lines, record_report,
        };
        let shared = self.call("read_session")?;
        let file = shared.home.load(native_session_ref)?;
        // What each turn was told, then what it produced, in the order the session keeps them.
        let turns: Vec<Vec<RecordedEntry>> = file
            .turns
            .iter()
            .map(|turn| {
                let ordinal = i64::from(turn.ordinal);
                let mut entries = vec![RecordedEntry::Said(RecordedWords {
                    native_ref: format!("said:{}", turn.native_turn_ref),
                    turn: ordinal,
                    text: turn.input.clone(),
                    left_out: 0,
                    at: None,
                })];
                entries.extend(
                    file.items
                        .iter()
                        .filter(|item| item.turn_ordinal == turn.ordinal)
                        .filter_map(|item| {
                            let content = serde_json::from_value(serde_json::json!({
                                "kind": item.kind, "payload": item.payload,
                            }))
                            .ok()?;
                            Some(RecordedEntry::Item(RecordedItem {
                                native_ref: item.native_ref.clone(),
                                turn: ordinal,
                                stream: item.stream.clone(),
                                parent_native_ref: item.parent_native_ref.clone(),
                                status: item.status,
                                content,
                                at: None,
                            }))
                        }),
                );
                entries
            })
            .collect();
        let (text, kept, entries, left_out) = record_lines(&turns, RECORD_MAX_BYTES)
            .map_err(|error| AdapterError::Internal(error.to_string()))?;
        let skipped = usize::try_from(left_out).unwrap_or(0).min(turns.len());
        let report = record_report(&turns[skipped..], left_out);
        let written = if write && !text.is_empty() {
            let path = std::env::temp_dir()
                .join(format!("orch-fake-record-{}", orch_types::new_id()))
                .join(format!("record-{native_session_ref}.jsonl"));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| AdapterError::Internal(error.to_string()))?;
            }
            std::fs::write(&path, text.as_bytes())
                .map_err(|error| AdapterError::Internal(error.to_string()))?;
            Some(SessionArchive {
                format: RECORD_FORMAT.to_owned(),
                path: path.to_string_lossy().into_owned(),
                size: i64::try_from(text.len()).unwrap_or(i64::MAX),
            })
        } else {
            None
        };
        Ok(SessionRecord {
            model: Some(file.model.clone()).filter(|model| !model.is_empty()),
            effort: file.effort.clone(),
            turns: kept,
            entries,
            file: written,
            report,
        })
    }

    async fn discover_mcp(&mut self, search: McpSearch) -> Result<Vec<McpFound>, AdapterError> {
        let shared = self.call("discover_mcp")?;
        let _ = search;
        shared
            .profile
            .mcp_servers
            .iter()
            .map(|server| {
                let definition = definition_of(server);
                Ok(McpFound {
                    definition_fingerprint: Some(definition.definition_fingerprint()),
                    name: server.name.clone(),
                    source: KIND.to_owned(),
                    scope: McpScope::User,
                    directory: None,
                    transport: server.transport(),
                    digest: definition.digest().to_string(),
                })
            })
            .collect()
    }

    async fn read_mcp(&mut self, request: McpImportRequest) -> Result<McpDefinition, AdapterError> {
        let shared = self.call("read_mcp")?;
        shared
            .profile
            .mcp_servers
            .iter()
            .find(|server| server.name == request.name)
            .map(definition_of)
            .ok_or_else(|| {
                AdapterError::NotFound(format!(
                    "the fake harness configures no server {:?}",
                    request.name
                ))
            })
    }

    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        let shared = self.call("login")?;
        shared.signin.start(&request, &shared.credentials).await?;
        if shared.credentials_lock().signed_in.is_some() {
            shared.facts_changed().await;
        }
        Ok(())
    }

    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let shared = self.call("login_input")?;
        shared.signin.input(&values, &shared.credentials).await?;
        if shared.credentials_lock().signed_in.is_some() {
            shared.facts_changed().await;
        }
        Ok(())
    }

    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        let shared = self.shared()?;
        shared.signin.cancel().await;
        Ok(())
    }

    async fn logout(&mut self) -> Result<(), AdapterError> {
        let shared = self.call("logout")?;
        shared.signin.sign_out(&shared.credentials).await;
        Ok(())
    }

    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        let shared = self.shared()?;
        shared.credentials_lock().facts(shared.clock.time())
    }

    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        self.shared()?;
        Ok(descriptor::models())
    }

    async fn issue_credential(&mut self, form: &str) -> Result<IssuedCredential, AdapterError> {
        let shared = self.call("issue_credential")?;
        let issued = shared.descriptor.credentials.issues.clone();
        let now = shared.clock.time();
        shared.credentials_lock().issue(form, &issued, now)
    }

    async fn set_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let shared = self.call("set_credential")?;
        let accepted = shared.descriptor.credentials.accepts.clone();
        let now = shared.clock.time();
        shared.credentials_lock().set(credential, &accepted, now)?;
        shared.credentials_changed.notify_waiters();
        Ok(())
    }

    async fn return_credential(&mut self) -> Result<GivenCredential, AdapterError> {
        let shared = self.call("return_credential")?;
        shared.credentials_lock().give_back()
    }

    async fn accept_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let shared = self.call("accept_credential")?;
        shared.credentials_lock().take_back(credential)?;
        shared.credentials_changed.notify_waiters();
        Ok(())
    }
}

async fn scripted_background_end(
    shared: &Shared,
    session: &LiveSession,
    id: &str,
) -> orch_agent_contract::background::NativeBackgroundEnd {
    use orch_agent_contract::background::{BackgroundEndState, NativeBackgroundEnd};
    if lock(&session.background_ended).contains(id) {
        return NativeBackgroundEnd {
            id: id.to_owned(),
            state: BackgroundEndState::AlreadyEnded,
            reason: None,
        };
    }
    let Some(command) = lock(&session.background).remove(id) else {
        return NativeBackgroundEnd {
            id: id.to_owned(),
            state: BackgroundEndState::Unknown,
            reason: Some("this session never observed that scripted command".to_owned()),
        };
    };
    let _=shared.sink.session(session.thread_id,session.segment_id,orch_agent_contract::events::SessionEvent::ItemCompleted(orch_agent_contract::events::ItemCompleted {
        native_ref:command.native_item_ref.unwrap_or_else(||id.to_owned()),status:orch_agent_contract::events::ItemStatus::Failed,
        payload:serde_json::json!({"title":"Scripted command ended","duration_ms":null,
            "tool":{"kind":"command","command":command.command.unwrap_or_default(),"cwd":command.cwd.unwrap_or_default(),"exit_code":-1,"background":true,"process_id":id,"actions":[],"output":{"inline":"","total_len":0,"truncated":false,"blob":null}},
            "raw":{"name":"fake_shell","input":{"inline":"","total_len":0,"truncated":false,"blob":null},"output":null}}),output_blob:None })).await;
    lock(&session.background_ended).insert(id.to_owned());
    NativeBackgroundEnd {
        id: id.to_owned(),
        state: BackgroundEndState::Ended,
        reason: None,
    }
}

fn definition_of(server: &descriptor::ConfiguredServer) -> McpDefinition {
    McpDefinition {
        spec: McpServerSpec {
            transport: server.transport(),
            command: server.command.clone(),
            url: server.url.clone(),
            authentication: None,
            env: server.env.clone(),
            secret_env: server.secrets.keys().cloned().collect(),
            working_dir: orch_agent_contract::discovery::McpWorkingDir::Workspace,
            startup_timeout_secs: orch_agent_contract::discovery::DEFAULT_STARTUP_TIMEOUT_SECS,
            tools_allow: None,
            tools_deny: None,
            requires: orch_agent_contract::discovery::McpRequires::default(),
        },
        secrets: server
            .secrets
            .iter()
            .map(|(name, value)| (name.clone(), Secret::new(value.clone())))
            .collect(),
    }
}

/// The status the fake reports of its harness when it is told it is running.
#[must_use]
pub fn status(harness_version: &str, sessions_open: u32) -> HarnessStatus {
    HarnessStatus {
        status: HarnessState::Online,
        detail: None,
        harness_version: Some(harness_version.to_owned()),
        sessions_open,
    }
}

/// An identifier of a thread, for a test that makes sessions.
#[must_use]
pub fn thread_id() -> Id {
    orch_types::new_id()
}
