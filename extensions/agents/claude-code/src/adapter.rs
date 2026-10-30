//! The adapter itself: start, stop, sessions, turns, sign-in, discovery.
//!
//! [`ClaudeCodeAdapter`] is what the connector hosts for the harness kind `claude-code`. It keeps no process
//! of its own: each open session is a conversation task ([`crate::conversation`]) that starts the harness
//! when a turn needs it. What the adapter does itself is what is not about one session: finding the program
//! and its version, declaring what it can do, opening and closing sessions, and the things that are about the
//! home as a whole (the sign-in, the sessions and servers it already has).
//!
//! The adapter never reads a clock or starts a program on its own account: time comes from the clock it is
//! given, and every process is started through the [`Launcher`] it is given, so that the same code runs the
//! program on this machine or, in a test, somewhere else.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

mod input_prefix;

use async_trait::async_trait;
use base64::Engine as _;
use orch_agent_contract::adapter::{
    AdapterError, AdapterEvent, AgentAdapter, AgentStart, AgentStarted, EventSink,
};
use orch_agent_contract::credentials::{GivenCredential, IssuedCredential};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_agent_contract::discovery::{
    McpDefinition, McpFound, McpImportRequest, McpSearch, ModelSuggestion, NativeSession,
};
use orch_agent_contract::events::{SessionEvent, SessionOpenedEvent};
use orch_agent_contract::facts::{HarnessState, HarnessStatus, ProviderFacts};
use orch_agent_contract::history::{ExportKind, SessionArchive};
use orch_agent_contract::login::{LoginRequest, LoginState, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, ImageInput, SessionMode, SessionOpened, SessionSpec, ThreadId, TurnId,
    TurnStart, TurnSteer,
};
use orch_types::{Clock, Id, Json};

use crate::config::{KIND, PROBE_TIMEOUT, ProfileConfig};
use crate::conversation::{self, Command, ConversationHandle, Params, SharedToken, message_blocks};
use crate::descriptor::{METHOD_BROWSER_CODE, METHOD_SETUP_TOKEN, descriptor, support_of};
use crate::events::MapperSettings;
use crate::process::{DirectLauncher, LaunchSpec, Launcher, base_environment, run_to_end};
use crate::provider::{self, BrowserLogin, Installed};
use crate::sessions::{self, HarnessSetup, Origin, TurnSettings};
use crate::stream::{image_block, text_block};
use crate::{discovery, history};

/// What a started harness is: fixed by `agent.start` until the next one.
#[derive(Debug, Clone)]
struct Started {
    setup: Arc<HarnessSetup>,
    sink: EventSink,
    version: String,
}

struct Session {
    handle: ConversationHandle,
    native_ref: String,
    spec: SessionSpec,
}

/// The adapter for Claude Code.
pub struct ClaudeCodeAdapter {
    clock: Arc<dyn Clock>,
    launcher: Arc<dyn Launcher>,
    started: Option<Started>,
    sessions: HashMap<ThreadId, Session>,
    /// Exact lanes observed idle when their native input was closed.
    prefix_sources: std::collections::HashSet<orch_agent_contract::history::NativePrefixSource>,
    history_inventory: Option<(Id, Arc<crate::native_history::Inventory>)>,
    token: SharedToken,
    browser: Option<BrowserLogin>,
}

impl std::fmt::Debug for ClaudeCodeAdapter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeCodeAdapter")
            .field("started", &self.started.is_some())
            .field("sessions", &self.sessions.len())
            .finish_non_exhaustive()
    }
}

/// Whether this process runs as the administrator, where the harness refuses to bypass permissions.
fn running_as_root() -> bool {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status
                .lines()
                .find_map(|line| line.strip_prefix("Uid:").map(str::to_owned))
        })
        .and_then(|ids| ids.split_whitespace().nth(1).map(|uid| uid == "0"))
        .unwrap_or(false)
}

/// The version in what `claude --version` prints (`2.1.296 (Claude Code)`).
fn parse_version(printed: &str) -> Option<String> {
    let first = printed.split_whitespace().next()?;
    semver::Version::parse(first.trim_start_matches('v'))
        .ok()
        .map(|version| version.to_string())
}

impl ClaudeCodeAdapter {
    /// An adapter that runs the harness as a program of this machine.
    #[must_use]
    pub fn new(clock: Arc<dyn Clock>) -> Self {
        Self::with_launcher(clock, Arc::new(DirectLauncher))
    }

    /// An adapter that starts the harness the way `launcher` says.
    #[must_use]
    pub fn with_launcher(clock: Arc<dyn Clock>, launcher: Arc<dyn Launcher>) -> Self {
        Self {
            clock,
            launcher,
            started: None,
            sessions: HashMap::new(),
            prefix_sources: Default::default(),
            history_inventory: None,
            token: Arc::new(Mutex::new(None)),
            browser: None,
        }
    }

    fn started(&self) -> Result<&Started, AdapterError> {
        self.started.as_ref().ok_or_else(|| {
            AdapterError::HarnessUnavailable("the harness has not been started".to_owned())
        })
    }

    fn session(&self, thread: ThreadId) -> Result<&Session, AdapterError> {
        self.sessions
            .get(&thread)
            .ok_or_else(|| AdapterError::SessionNotFound(thread.to_string()))
    }

    /// The version of the program, asked of the program.
    async fn version_of(
        launcher: &dyn Launcher,
        program: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<String, AdapterError> {
        let spec = LaunchSpec {
            program: program.to_owned(),
            args: vec!["--version".to_owned()],
            env: environment.clone(),
            cwd: std::env::temp_dir(),
            label: "version".to_owned(),
        };
        let finished = run_to_end(launcher, &spec, PROBE_TIMEOUT)
            .await
            .map_err(|error| AdapterError::HarnessUnavailable(error.to_string()))?;
        finished
            .lines
            .iter()
            .find_map(|line| parse_version(line))
            .ok_or_else(|| {
                AdapterError::HarnessUnavailable(format!(
                    "{program} did not say which version it is ({})",
                    finished.exit.describe()
                ))
            })
    }

    async fn status_event(&self, state: HarnessState, detail: Option<String>) {
        if let Some(started) = &self.started {
            let _ = started
                .sink
                .emit(AdapterEvent::Status(HarnessStatus {
                    status: state,
                    detail,
                    harness_version: Some(started.version.clone()),
                    sessions_open: u32::try_from(self.sessions.len()).unwrap_or(u32::MAX),
                }))
                .await;
        }
    }

    /// Sessions that ended (their task is gone) are forgotten.
    fn forget_ended(&mut self) {
        self.sessions.retain(|_, session| session.handle.is_open());
    }

    /// The home whose files are read: the one given, or the harness's own default of whoever runs it.
    fn files_home(setup: &HarnessSetup) -> Option<PathBuf> {
        setup.home.clone().or_else(|| {
            setup
                .inherited
                .get("HOME")
                .map(|person| PathBuf::from(person).join(".claude"))
        })
    }

    fn current_token(&self) -> Option<String> {
        self.token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    fn set_token(&self, token: Option<String>) {
        *self
            .token
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = token;
    }

    /// Every open session starts its next process with what the harness now runs on.
    async fn credential_changed(&self) {
        for session in self.sessions.values() {
            let _ = session
                .handle
                .ask(|reply| Command::CredentialChanged { reply })
                .await;
        }
    }

    fn turn_settings(spec: &SessionSpec) -> TurnSettings {
        TurnSettings {
            model: spec.settings.model.clone(),
            effort: spec.settings.effort.clone(),
            approval_mode: spec.settings.approval_mode,
            sandbox_mode: spec.settings.sandbox_mode,
            params: spec.settings.params.clone(),
            plan: false,
        }
    }
}

/// The blocks of an image: read from the file the connector put it in, encoded as the harness takes them.
async fn image_blocks(images: &[ImageInput]) -> Vec<Json> {
    let mut blocks = Vec::new();
    for image in images {
        let encoded = match &image.path {
            Some(path) => tokio::fs::read(path)
                .await
                .ok()
                .map(|bytes| base64::engine::general_purpose::STANDARD.encode(bytes)),
            None => None,
        };
        blocks.push(match encoded {
            Some(data) => image_block(&image.media_type, &data),
            None => text_block(&format!("[image omitted: {}]", image.name)),
        });
    }
    blocks
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn kind(&self) -> &'static str {
        KIND
    }

    fn native_check(&self) -> orch_agent_contract::conformance::NativeCheck {
        use orch_agent_contract::conformance::{ModelPaths, NativeCheck};
        NativeCheck {
            approval_mode: orch_agent_contract::descriptor::ApprovalMode::Ask,
            // A failing case ends soon: the harness retries its model request once.
            profile_env: std::collections::BTreeMap::from([(
                "CLAUDE_CODE_MAX_RETRIES".to_owned(),
                "1".to_owned(),
            )]),
            model_paths: ModelPaths {
                reached: vec!["/v1/messages".to_owned()],
                allowed: vec![
                    "/v1/messages".to_owned(),
                    "/v1/messages/count_tokens".to_owned(),
                ],
                // The usage read of a subscription sign-in.
                exact: vec!["/api/oauth/usage".to_owned()],
            },
        }
    }

    fn verify_quiet_start(
        &self,
        evidence: &orch_agent_contract::conformance::QuietEvidence<'_>,
    ) -> Result<(), String> {
        if evidence.launches.is_empty() {
            return Err("no native launch evidence".to_owned());
        }
        for launch in evidence.launches {
            for key in [
                "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
                "DISABLE_TELEMETRY",
                "DISABLE_ERROR_REPORTING",
                "DISABLE_UPDATES",
            ] {
                if launch.get(key).map(String::as_str) != Some("1") {
                    return Err(format!("{key} was not disabled at startup"));
                }
            }
        }
        Ok(())
    }

    async fn start(
        &mut self,
        start: AgentStart,
        sink: EventSink,
    ) -> Result<AgentStarted, AdapterError> {
        let attach = start.profile.attach.clone();
        let program = start
            .binary
            .clone()
            .or_else(|| attach.as_ref().and_then(|attach| attach.binary.clone()))
            .unwrap_or_else(|| crate::config::PROGRAM.to_owned());
        let managed_home = start.home.is_some()
            && !attach
                .as_ref()
                .and_then(|attach| attach.home.as_ref())
                .is_some_and(|home| home.directory().is_some());
        let home = start
            .home
            .clone()
            .or_else(|| {
                attach
                    .as_ref()
                    .and_then(|attach| attach.home.as_ref())
                    .and_then(|home| home.directory().map(str::to_owned))
            })
            .map(PathBuf::from);
        if let Some(home) = &home {
            tokio::fs::create_dir_all(home).await.map_err(|error| {
                AdapterError::HarnessUnavailable(format!(
                    "the home {} could not be made: {error}",
                    home.display()
                ))
            })?;
        }
        let inherited: BTreeMap<String, String> = std::env::vars().collect();
        let as_root = running_as_root();
        let setup = Arc::new(HarnessSetup {
            program,
            home,
            managed_home,
            loopback: start.loopback.clone(),
            profile_env: start.profile.env.clone(),
            config: ProfileConfig::from_json(&start.profile.config),
            inherited,
            as_root,
        });
        let mut environment = base_environment(setup.inherited.clone());
        environment.extend(setup.profile_env.clone());
        sessions::place_home(&mut environment, &setup);
        if setup.managed_home {
            environment.extend(sessions::managed_environment(false));
        }
        let version =
            Self::version_of(self.launcher.as_ref(), &setup.program, &environment).await?;
        if support_of(&version) == orch_agent_contract::descriptor::HarnessSupport::Unsupported {
            return Err(AdapterError::HarnessUnavailable(format!(
                "Claude Code {version} is older than the versions this adapter drives ({})",
                crate::config::SUPPORTED_RANGE
            )));
        }
        let described = descriptor(&version, as_root);
        if let Some(home) = &setup.home
            && let Some(stored) = provider::stored_token(home).await
        {
            *self
                .token
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(stored.token);
        }
        self.started = Some(Started {
            setup,
            sink,
            version: version.clone(),
        });
        self.status_event(HarnessState::Online, None).await;
        Ok(AgentStarted {
            harness_version: version,
            descriptor: described,
        })
    }

    async fn stop(&mut self) -> Result<(), AdapterError> {
        self.prefix_sources.clear();
        self.history_inventory = None;
        let sessions: Vec<_> = self.sessions.drain().collect();
        for (_, session) in sessions {
            let _ = session.handle.ask(|reply| Command::Close { reply }).await;
        }
        self.status_event(HarnessState::Offline, None).await;
        self.started = None;
        Ok(())
    }

    async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
        let started = self.started()?.clone();
        let mut environment = base_environment(started.setup.inherited.clone());
        environment.extend(started.setup.profile_env.clone());
        sessions::place_home(&mut environment, &started.setup);
        if started.setup.managed_home {
            environment.extend(sessions::managed_environment(false));
        }
        let version =
            Self::version_of(self.launcher.as_ref(), &started.setup.program, &environment).await?;
        Ok(descriptor(&version, started.setup.as_root))
    }

    async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
        let approval_reviewer = self.validate_open(&spec)?;
        self.forget_ended();
        let started = self.started()?.clone();
        let thread = spec.settings.thread_id;
        let existing_ref = self
            .sessions
            .get(&thread)
            .map(|session| session.native_ref.clone());
        let given = spec.settings.native_session_ref.clone();
        let (session_id, origin, resumed) = match spec.settings.mode {
            SessionMode::New => (orch_types::new_id().to_string(), Origin::New, false),
            SessionMode::Resume | SessionMode::Adopt => {
                let id = given.ok_or_else(|| {
                    AdapterError::NotFound("a session to resume names its id".to_owned())
                })?;
                if existing_ref.as_deref() == Some(id.as_str()) {
                    // The same conversation, opened again: what changed takes effect with the next turn.
                    let mut sources =
                        crate::sessions::documented_instruction_sources(&started.setup, &spec);
                    if let Some(session) = self.sessions.get_mut(&thread) {
                        session
                            .handle
                            .ask(|reply| Command::Reconfigure {
                                spec: Box::new(spec.clone()),
                                reply,
                            })
                            .await?;
                        if let Ok(reported) = session
                            .handle
                            .ask(|reply| Command::InstructionSources { reply })
                            .await
                        {
                            sources = crate::instruction_sources::merge(sources, reported);
                        }
                        session.spec = spec;
                    }
                    return Ok(SessionOpened {
                        native_session_ref: id,
                        resumed: true,
                        import_report: None,
                        instruction_sources: sources,
                        approval_reviewer,
                    });
                }
                if let Some(home) = &started.setup.home
                    && history::find_transcript(home, &id).is_none()
                {
                    return Err(AdapterError::SessionNotFound(id));
                }
                (id, Origin::Resume, true)
            }
            SessionMode::Fork => {
                let parent = given.ok_or_else(|| {
                    AdapterError::NotFound("a fork names the session it copies".to_owned())
                })?;
                if spec.settings.upto_turn_ordinal.is_some() {
                    return Err(AdapterError::Unsupported(
                        "fork_native at an earlier turn".to_owned(),
                    ));
                }
                if let Some(home) = &started.setup.home
                    && history::find_transcript(home, &parent).is_none()
                {
                    return Err(AdapterError::SessionNotFound(parent));
                }
                (
                    orch_types::new_id().to_string(),
                    Origin::Fork { from: parent },
                    false,
                )
            }
            SessionMode::ImportNative => {
                let archive = spec.archive.clone().ok_or_else(|| {
                    AdapterError::ImportNativeFailed("no archive was given".to_owned())
                })?;
                let home = started.setup.home.clone().ok_or_else(|| {
                    AdapterError::ImportNativeFailed(
                        "the harness has no home of its own to import into".to_owned(),
                    )
                })?;
                let id = history::import_transcript(
                    &home,
                    started.setup.managed_home,
                    &archive,
                    &started.version,
                )
                .await
                .map_err(|error| AdapterError::ImportNativeFailed(error.to_string()))?;
                (id, Origin::Resume, true)
            }
            SessionMode::ImportNeutral => {
                return Err(AdapterError::Unsupported("import_neutral".to_owned()));
            }
        };
        self.install_session(spec, session_id, origin, resumed, approval_reviewer)
            .await
    }

    async fn inspect_input_prefix(
        &mut self,
        source: orch_agent_contract::history::NativePrefixSource,
        before_input_ref: &str,
    ) -> Result<orch_agent_contract::history::NativeInputPrefix, AdapterError> {
        self.inspect_prefix(source, before_input_ref).await
    }

    async fn open_input_prefix(
        &mut self,
        spec: SessionSpec,
        prefix: orch_agent_contract::history::NativeInputPrefix,
    ) -> Result<orch_agent_contract::history::NativePrefixOpened, AdapterError> {
        self.open_prefix(spec, prefix).await
    }

    async fn resume_continuation(
        &mut self,
        spec: SessionSpec,
        source: orch_agent_contract::history::NativePrefixSource,
        replace_segment_id: Option<orch_types::Id>,
    ) -> Result<SessionOpened, AdapterError> {
        if self.started()?.version != "2.1.296" {
            return Err(AdapterError::Unsupported(
                "Exact continuation resume requires Claude Code 2.1.296.".into(),
            ));
        }
        if spec.settings.mode != SessionMode::Resume
            || spec.settings.thread_id != source.thread_id
            || spec.settings.upto_turn_ordinal.is_some()
            || spec.settings.native_session_ref.as_deref()
                != Some(source.native_session_ref.as_str())
        {
            return Err(AdapterError::Other {code:"invalid_input".into(),retryable:false,
                message:"A continuation resume names its exact retained native lane without an ordinal cut.".into()});
        }
        if let Some(current) = self.sessions.get(&source.thread_id) {
            let exact = current.spec.settings.segment_id == source.segment_id
                && current.native_ref == source.native_session_ref;
            if !(exact || replace_segment_id == Some(current.spec.settings.segment_id))
                || !current
                    .handle
                    .ask(|reply| Command::Quiescent { reply })
                    .await?
            {
                return Err(AdapterError::Other {code:"conflict".into(),retryable:false,
                    message:"Another or still-running native generation occupies this continuation lane.".into()});
            }
        }
        // A plain resumed conversation is installed cold: it starts no CLI/model
        // turn. Prefix inspection reads the retained transcript independently.
        let opened = self.open_session(spec).await?;
        if opened.native_session_ref != source.native_session_ref {
            return Err(AdapterError::Other {
                code: "conflict".into(),
                retryable: false,
                message: "The native program resumed another continuation identity.".into(),
            });
        }
        Ok(opened)
    }

    async fn manage_history(
        &mut self,
        sources: Vec<orch_agent_contract::history::NativeHistorySource>,
        remove: bool,
    ) -> Result<Vec<orch_agent_contract::history::NativeHistoryFact>, AdapterError> {
        if sources.is_empty() {
            self.history_inventory = None;
            return Ok(Vec::new());
        }
        let setup = self.started()?.setup.clone();
        let home = setup
            .home
            .as_ref()
            .filter(|_| setup.managed_home)
            .ok_or_else(|| AdapterError::Unsupported("history.manage in a personal home".into()))?
            .clone();
        let mut facts = Vec::with_capacity(sources.len());
        let pass = sources.first().and_then(|source| source.inventory_pass);
        if sources.iter().any(|source| source.inventory_pass != pass) {
            return Err(AdapterError::Other {
                code: "invalid_input".into(),
                message: "One native history batch belongs to one inventory pass.".into(),
                retryable: false,
            });
        }
        let cached = self
            .history_inventory
            .as_ref()
            .filter(|(id, _)| Some(*id) == pass)
            .map(|(_, inventory)| Arc::clone(inventory));
        let inventory = if let Some(inventory) = cached {
            inventory
        } else {
            let inventory_home = home.clone();
            Arc::new(
                tokio::task::spawn_blocking(move || {
                    crate::native_history::inventory(&inventory_home)
                })
                .await
                .map_err(|error| AdapterError::Internal(error.to_string()))??,
            )
        };
        if let Some(pass) = pass {
            self.history_inventory = Some((pass, Arc::clone(&inventory)));
        }
        for request in sources {
            let started_by_orch = request.started_by_orch;
            let source = request.source;
            let registered = self
                .sessions
                .get(&source.thread_id)
                .is_some_and(|session| session.native_ref == source.native_session_ref);
            let result = async {
                if !started_by_orch {
                    return Err(AdapterError::Other {
                        code: "history.not_owned".into(),
                        message: "Orch did not start this native history.".into(),
                        retryable: false,
                    });
                }
                if remove
                    && let Some(session) = self.sessions.get(&source.thread_id)
                    && session.native_ref == source.native_session_ref
                {
                    if !session
                        .handle
                        .ask(|reply| Command::Quiescent { reply })
                        .await?
                    {
                        return Err(AdapterError::Other {
                            code: "history.in_use".into(),
                            message: "The native session still has running work or an approval."
                                .into(),
                            retryable: false,
                        });
                    }
                    self.close_session(source.thread_id).await?;
                }
                let home = home.clone();
                let inventory = inventory.clone();
                let requested = source.clone();
                tokio::task::spawn_blocking(move || {
                    crate::native_history::manage(&home, &inventory, requested, remove)
                })
                .await
                .map_err(|error| AdapterError::Internal(error.to_string()))?
                .map(|mut fact| {
                    if !remove && registered {
                        fact.removed = false;
                    }
                    fact
                })
            }
            .await;
            facts.push(result.unwrap_or_else(|error| {
                orch_agent_contract::history::NativeHistoryFact {
                    source,
                    bytes: None,
                    removed: false,
                    error: Some(error.code().into()),
                }
            }));
        }
        Ok(facts)
    }

    async fn cleanup_history_home(
        &mut self,
        before: orch_types::Time,
    ) -> Result<Vec<orch_agent_contract::history::NativeHomeCleanupItem>, AdapterError> {
        self.forget_ended();
        if !self.sessions.is_empty() {
            return Err(AdapterError::Other {
                code: "history.in_use".into(),
                message: "Native sessions still hold this home's working files.".into(),
                retryable: false,
            });
        }
        let setup = self.started()?.setup.clone();
        let home = setup
            .home
            .as_ref()
            .filter(|_| setup.managed_home)
            .ok_or_else(|| {
                AdapterError::Unsupported("history.home_cleanup in a personal home".into())
            })?
            .clone();
        tokio::task::spawn_blocking(move || crate::native_history::cleanup_home(&home, before))
            .await
            .map_err(|error| AdapterError::Internal(error.to_string()))
    }

    async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
        let idle = if let Some(session) = self.sessions.get(&thread) {
            session
                .handle
                .ask(|reply| Command::Quiescent { reply })
                .await
                .unwrap_or(false)
        } else {
            false
        };
        if let Some(session) = self.sessions.remove(&thread) {
            let source = orch_agent_contract::history::NativePrefixSource {
                thread_id: thread,
                segment_id: session.spec.settings.segment_id,
                native_session_ref: session.native_ref,
            };
            let closed = session.handle.ask(|reply| Command::Close { reply }).await;
            if idle && closed.is_ok() {
                self.prefix_sources.insert(source);
            }
            self.status_event(HarnessState::Online, None).await;
        }
        Ok(())
    }

    async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
        let session = self.session(turn.thread_id)?;
        crate::sessions::validate_instruction_settings(&session.spec)?;
        let mut images = image_blocks(&turn.images).await;
        let mut blocks = message_blocks(&turn.text, Vec::new());
        blocks.append(&mut images);
        let handle = session.handle.clone();
        handle
            .ask(|reply| Command::StartTurn {
                turn: Box::new(turn),
                blocks,
                reply,
            })
            .await?
    }

    async fn steer(&mut self, steer: TurnSteer) -> Result<(), AdapterError> {
        let session = self.session(steer.thread_id)?;
        let mut blocks = message_blocks(&steer.text, Vec::new());
        blocks.append(&mut image_blocks(&steer.images).await);
        let handle = session.handle.clone();
        handle
            .ask(|reply| Command::Steer {
                steer,
                blocks,
                reply,
            })
            .await?
    }

    async fn interrupt(&mut self, thread: ThreadId, turn: TurnId) -> Result<(), AdapterError> {
        let handle = self.session(thread)?.handle.clone();
        handle
            .ask(|reply| Command::Interrupt {
                turn_id: turn,
                reply,
            })
            .await?
    }

    async fn count_context(
        &mut self,
        thread: ThreadId,
    ) -> Result<Option<orch_agent_contract::events::ContextFill>, AdapterError> {
        // A session that is not open here is not opened for a count.
        let Some(session) = self.sessions.get(&thread) else {
            return Ok(None);
        };
        let handle = session.handle.clone();
        let asked = handle.ask(|reply| Command::CountContext { reply });
        // The vendor's counter is asked once for each part; an answer that does not come in time is none.
        match tokio::time::timeout(crate::config::CONTEXT_COUNT_WAIT, asked).await {
            Ok(answer) => answer,
            Err(_) => Ok(None),
        }
    }

    async fn resolve_approval(
        &mut self,
        thread: ThreadId,
        request_ref: &str,
        decision: ApprovalDecision,
    ) -> Result<(), AdapterError> {
        let handle = self.session(thread)?.handle.clone();
        let request_ref = request_ref.to_owned();
        handle
            .ask(|reply| Command::Resolve {
                request_ref,
                decision,
                reply,
            })
            .await?
    }

    async fn export_session(
        &mut self,
        thread: ThreadId,
        kind: ExportKind,
        upto_turn_ordinal: Option<u32>,
    ) -> Result<SessionArchive, AdapterError> {
        if kind == ExportKind::Neutral {
            return Err(AdapterError::Unsupported("export_neutral".to_owned()));
        }
        if upto_turn_ordinal.is_some() {
            return Err(AdapterError::Unsupported(
                "export_native up to an earlier turn".to_owned(),
            ));
        }
        let handle = self.session(thread)?.handle.clone();
        let quiescent = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            handle.ask(|reply| Command::Quiescent { reply }),
        )
        .await
        .map_err(|_| {
            AdapterError::HarnessError(
                "the session did not answer the native-export readiness check within 30 seconds"
                    .to_owned(),
            )
        })??;
        if !quiescent {
            return Err(AdapterError::Unsupported(
                "native export requires an idle session with no pending steer".to_owned(),
            ));
        }
        let setup = Arc::clone(&self.started()?.setup);
        let native = self.session(thread)?.native_ref.clone();
        let home = Self::files_home(&setup)
            .ok_or_else(|| AdapterError::NotFound("the harness has no home".to_owned()))?;
        let into =
            std::env::temp_dir().join(format!("orch-claude-export-{}", orch_types::new_id()));
        history::export_transcript(&home, &native, &into)
            .await
            .map_err(|error| AdapterError::NotFound(error.to_string()))
    }

    async fn resync_session(&mut self, thread: ThreadId) -> Result<u32, AdapterError> {
        let started = self.started()?.clone();
        let (native, spec) = {
            let session = self.session(thread)?;
            (session.native_ref.clone(), session.spec.clone())
        };
        let home = Self::files_home(&started.setup)
            .ok_or_else(|| AdapterError::NotFound("the harness has no home".to_owned()))?;
        let transcript = history::find_transcript(&home, &native)
            .ok_or_else(|| AdapterError::SessionNotFound(native.clone()))?;
        history::replay_into(
            &transcript,
            MapperSettings {
                directory: spec.settings.directory.clone(),
                cwd: spec.settings.cwd.clone(),
                raw_events: false,
            },
            Arc::clone(&self.clock),
            started.sink.clone(),
            thread,
            spec.settings.segment_id,
        )
        .await
        .map_err(|error| AdapterError::Internal(error.to_string()))
    }

    async fn discover_sessions(&mut self) -> Result<Vec<NativeSession>, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let home = Self::files_home(&setup)
            .ok_or_else(|| AdapterError::NotFound("the harness has no home".to_owned()))?;
        // The API says which of them a thread has adopted; the adapter lists what the harness has.
        tokio::task::spawn_blocking(move || discovery::sessions_of(&home))
            .await
            .map_err(|error| AdapterError::Internal(error.to_string()))
    }

    async fn read_session(
        &mut self,
        native_session_ref: &str,
        write: bool,
    ) -> Result<orch_agent_contract::discovery::SessionRecord, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let home = Self::files_home(&setup)
            .ok_or_else(|| AdapterError::NotFound("the harness has no home".to_owned()))?;
        let transcript = history::find_transcript(&home, native_session_ref)
            .ok_or_else(|| AdapterError::SessionNotFound(native_session_ref.to_owned()))?;
        let clock = Arc::clone(&self.clock);
        let session = native_session_ref.to_owned();
        // Where the connector takes the record from; it removes the file once it is up.
        let into =
            std::env::temp_dir().join(format!("orch-claude-record-{}", orch_types::new_id()));
        history::read_record_async(&transcript, &session, write, &into, clock)
            .await
            .map_err(|error| match error {
                history::HistoryError::OtherVersion(_) => {
                    AdapterError::Unsupported(error.to_string())
                }
                other => AdapterError::HarnessError(other.to_string()),
            })
    }

    async fn discover_mcp(&mut self, search: McpSearch) -> Result<Vec<McpFound>, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let file = discovery::user_configuration(setup.home.as_deref(), &setup.inherited);
        Ok(discovery::found_servers(file.as_deref(), &search)
            .into_iter()
            .map(|(found, _)| found)
            .collect())
    }

    async fn read_mcp(&mut self, request: McpImportRequest) -> Result<McpDefinition, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let file = discovery::user_configuration(setup.home.as_deref(), &setup.inherited);
        discovery::read_definition(file.as_deref(), &request)
    }

    async fn login(&mut self, request: LoginRequest) -> Result<(), AdapterError> {
        let started = self.started()?.clone();
        if let Some(running) = self.browser.take() {
            running.cancel();
        }
        match request.method.as_str() {
            METHOD_SETUP_TOKEN => {
                let token = request.values.text("token").unwrap_or_default();
                if !provider::looks_like_a_token(token) {
                    let state = LoginState::failed(
                        METHOD_SETUP_TOKEN,
                        "That is not a token: it is one word.",
                    );
                    let _ = started.sink.emit(AdapterEvent::Login(state)).await;
                    return Err(AdapterError::CredentialUnusable(
                        "the token is not one word".to_owned(),
                    ));
                }
                let home = started.setup.home.clone().ok_or_else(|| {
                    AdapterError::HarnessUnavailable(
                        "the harness has no home of its own to keep the token in".to_owned(),
                    )
                })?;
                let _ = started
                    .sink
                    .emit(AdapterEvent::Login(LoginState::starting(
                        METHOD_SETUP_TOKEN,
                    )))
                    .await;
                let stored =
                    provider::store_token(&home, token, METHOD_SETUP_TOKEN, self.clock.time())
                        .await?;
                self.set_token(Some(stored.token));
                self.credential_changed().await;
                let _ = started
                    .sink
                    .emit(AdapterEvent::Login(LoginState::completed(
                        METHOD_SETUP_TOKEN,
                    )))
                    .await;
                Ok(())
            }
            METHOD_BROWSER_CODE => {
                let sink = started.sink.clone();
                let running = provider::start_browser_login(
                    Arc::clone(&self.launcher),
                    &started.setup,
                    Arc::clone(&self.clock),
                    move |state| {
                        let sink = sink.clone();
                        async move { sink.emit(AdapterEvent::Login(state)).await.is_ok() }
                    },
                )
                .await?;
                self.browser = Some(running);
                Ok(())
            }
            other => Err(AdapterError::NotFound(format!(
                "there is no sign-in method {other:?}"
            ))),
        }
    }

    async fn login_input(&mut self, values: LoginValues) -> Result<(), AdapterError> {
        let Some(running) = &self.browser else {
            return Err(AdapterError::NotFound(
                "no sign-in is waiting for an answer".to_owned(),
            ));
        };
        let code = values
            .text("code")
            .filter(|code| !code.trim().is_empty())
            .ok_or_else(|| AdapterError::NotFound("the code is missing".to_owned()))?;
        running
            .input
            .send(code.to_owned())
            .await
            .map_err(|_| AdapterError::NotFound("the sign-in has ended".to_owned()))
    }

    async fn cancel_login(&mut self) -> Result<(), AdapterError> {
        if let Some(running) = self.browser.take() {
            running.cancel();
        }
        Ok(())
    }

    async fn logout(&mut self) -> Result<(), AdapterError> {
        let started = self.started()?.clone();
        if let Some(running) = self.browser.take() {
            running.cancel();
        }
        provider::logout(self.launcher.as_ref(), &started.setup).await;
        self.set_token(None);
        self.credential_changed().await;
        let _ = started
            .sink
            .emit(AdapterEvent::Login(LoginState::idle()))
            .await;
        Ok(())
    }

    async fn provider_facts(&mut self) -> Result<ProviderFacts, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let stored = match &setup.home {
            Some(home) => provider::stored_token(home).await,
            None => None,
        };
        provider::facts(
            self.launcher.as_ref(),
            &setup,
            stored.as_ref(),
            self.clock.time(),
        )
        .await
    }

    async fn renew_provider_credential(&mut self) -> Result<ProviderFacts, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let stored = match &setup.home {
            Some(home) => provider::stored_token(home).await,
            None => None,
        };
        provider::renew(
            self.launcher.as_ref(),
            &setup,
            stored.as_ref(),
            self.clock.time(),
        )
        .await
    }

    async fn discover_models(&mut self) -> Result<Vec<ModelSuggestion>, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let token = self.current_token();
        let mut found = provider::models(self.launcher.as_ref(), &setup, token.as_deref()).await?;
        provider::add_context_windows(&mut found, setup.home.as_deref());
        Ok(found)
    }

    async fn issue_credential(&mut self, form: &str) -> Result<IssuedCredential, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        provider::issue(self.launcher.as_ref(), &setup, self.clock.as_ref(), form).await
    }

    async fn set_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        match provider::install(&setup, &credential).await? {
            Installed::Token(token) => self.set_token(Some(token)),
            Installed::Login => self.set_token(None),
        }
        self.credential_changed().await;
        Ok(())
    }

    async fn return_credential(&mut self) -> Result<GivenCredential, AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        let credential = provider::take_back(&setup).await?;
        self.credential_changed().await;
        Ok(credential)
    }

    async fn accept_credential(&mut self, credential: GivenCredential) -> Result<(), AdapterError> {
        let setup = Arc::clone(&self.started()?.setup);
        provider::accept_back(&setup, &credential).await?;
        self.credential_changed().await;
        Ok(())
    }
}
