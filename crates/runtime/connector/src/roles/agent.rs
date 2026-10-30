//! Adapter hosting and harness lifecycle.
//!
//! The agent role answers the commands that concern harnesses: starting and stopping a harness process
//! for a provider and an agent profile, opening, closing, exporting and reporting again the sessions in
//! it, starting, steering and interrupting turns, answering what a harness asked and what Orch's tools
//! returned, and the sign-in and the credentials of a provider whose login the harness holds. Each is
//! turned from the link's body into the call of the adapter, and what the adapter answers into the
//! link's result. What an adapter reports on its own goes to the spool in the order it was said
//! ([`crate::harnesses`]).
//!
//! Adapters are registered by kind ([`crate::harnesses::AdapterRegistry`]): the scripted one is
//! registered by the program that composes the connector, and so are the adapters of Codex and Claude
//! Code when they exist. A command for a harness kind with no adapter is answered
//! `harness_unavailable`; a command for an operation the adapter does not support, `unsupported`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use orch_agent_contract::credentials::{GivenCredential, UpstreamTarget};
use orch_agent_contract::discovery::NativeSession;
use orch_agent_contract::history::{
    ExportKind, NativeInputPrefix, NativePrefixOpened, NativePrefixSource, SessionArchive,
};
use orch_agent_contract::login::{LoginRequest, LoginValues};
use orch_agent_contract::session::{
    ApprovalDecision, ExecAccess, ImageInput, LocalFile, ProviderAccess, SessionSettings,
    SessionSpec, ThreadId, ToolError, TurnStart as AdapterTurn, TurnSteer as AdapterSteer,
};
use orch_link_contract::commands::{
    AgentCredentialReturn, AgentCredentialSet, AgentDiscoverSessionsResult, AgentReadSessionResult,
    AgentStartResult, BlobRef, Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode,
    ProviderDiscoverModels, ProviderRefresh, RecordRef, Sealed, SealedCredential,
    Secret as LinkSecret, SecretOrText, SessionExportResult, SessionOpen, SessionOpenProvider,
    SessionOpenResult, SessionResyncResult, TicketSlot, TurnImage,
};
use orch_link_contract::session::Role;
use orch_plugin_contract::Secret;
use orch_types::{Field, FieldType, Id, Json};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::credentials::{Direction, exchange_info};
use crate::facts::FactsDraft;
use crate::harnesses::{Hosting, Process, command_error};
use crate::loopback::Tickets;
use crate::roles::{Call, RoleHandler};

pub use crate::harnesses::{AdapterRegistration, AdapterRegistry, HomeRule};

/// The kinds of command the agent role takes.
const TAKES: &[CommandKind] = &[
    CommandKind::UnitAfterUp,
    CommandKind::AgentStart,
    CommandKind::AgentStop,
    CommandKind::AgentDiscoverSessions,
    CommandKind::AgentReadSession,
    CommandKind::SessionOpen,
    CommandKind::SessionOpenFenced,
    CommandKind::SessionPrefixInspect,
    CommandKind::SessionPrefixOpen,
    CommandKind::SessionContinuationResume,
    CommandKind::SessionContinuationClose,
    CommandKind::SessionClose,
    CommandKind::AgentHistoryManage,
    CommandKind::AgentHistoryHomeCleanup,
    CommandKind::SessionCloseFenced,
    CommandKind::SessionMcpReload,
    CommandKind::SessionBackground,
    CommandKind::SessionNativeActions,
    CommandKind::SessionBackgroundEnd,
    CommandKind::SessionContext,
    CommandKind::SessionExport,
    CommandKind::SessionResync,
    CommandKind::TurnStart,
    CommandKind::TurnStartFenced,
    CommandKind::TurnSteer,
    CommandKind::TurnInterrupt,
    CommandKind::ApprovalResolve,
    CommandKind::OrchToolResult,
    CommandKind::ProviderLoginStart,
    CommandKind::ProviderLoginInput,
    CommandKind::ProviderLoginCancel,
    CommandKind::ProviderLogout,
    CommandKind::ProviderRefresh,
    CommandKind::ProviderResetConsume,
    CommandKind::ProviderDiscoverModels,
    CommandKind::AgentCredentialReturn,
    CommandKind::AgentCredentialSet,
];

/// The agent role.
pub struct AgentRole {
    after_up: crate::after_up::Runner,
    hosting: Arc<Hosting>,
    background: crate::background::Reads,
}

enum OpenIntent {
    Ordinary,
    Guarded(Option<NativePrefixSource>),
    Prefix(NativeInputPrefix),
    Resume {
        source: NativePrefixSource,
        replace_segment: Option<Id>,
        retained: Option<NativePrefixSource>,
    },
}

struct Opened {
    session: orch_agent_contract::session::SessionOpened,
    prefix: Option<NativeInputPrefix>,
}

impl Opened {
    fn ordinary(self) -> SessionOpenResult {
        SessionOpenResult {
            native_session_ref: self.session.native_session_ref,
            resumed: self.session.resumed,
            import_report: self.session.import_report,
            instruction_sources: self.session.instruction_sources,
            approval_reviewer: self.session.approval_reviewer,
        }
    }
}

impl std::fmt::Debug for AgentRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentRole").finish_non_exhaustive()
    }
}

/// Turns a body of the link into the type of the adapter's contract that has the same shape.
fn recast<A: Serialize, B: DeserializeOwned>(value: &A) -> Result<B, CommandError> {
    serde_json::to_value(value)
        .and_then(serde_json::from_value)
        .map_err(|e| {
            CommandError::new(
                ErrorCode::Internal,
                format!("a command's body does not fit what the adapter takes: {e}"),
            )
        })
}

fn native_selection(
    segment: Option<Id>,
    selection: Option<&orch_agent_contract::native::NativeInvocation>,
) -> Result<Option<orch_agent_contract::native::NativeInvocation>, CommandError> {
    let Some(selection) = selection else {
        return Ok(None);
    };
    let raw = segment.and_then(|segment| {
        selection
            .revision
            .strip_prefix(&format!("{segment}:"))
            .map(str::to_owned)
    });
    let Some(revision) = raw.filter(|revision| !revision.is_empty()) else {
        let mut error = CommandError::new(
            ErrorCode::Unsupported,
            "The session's command list changed; refresh the selection",
        );
        error.code = "native_action.stale".to_owned();
        return Err(error);
    };
    let mut selected = selection.clone();
    selected.revision = revision;
    Ok(Some(selected))
}

fn provider_of(provider: &SessionOpenProvider) -> Id {
    match provider {
        SessionOpenProvider::Native { provider_id }
        | SessionOpenProvider::Forwarded { provider_id, .. }
        | SessionOpenProvider::Shared { provider_id, .. }
        | SessionOpenProvider::Handover { provider_id, .. } => *provider_id,
    }
}

fn signed(slot: &TicketSlot) -> Option<String> {
    match slot {
        TicketSlot::Signed(ticket) => Some(ticket.clone()),
        TicketSlot::Asked(_) => None,
    }
}

impl AgentRole {
    /// The role over the hosting of this connector's harness processes.
    #[must_use]
    pub fn new(hosting: Arc<Hosting>) -> Self {
        Self {
            after_up: crate::after_up::Runner::new(hosting.services()),
            hosting,
            background: crate::background::Reads::default(),
        }
    }

    /// The hosting of the processes.
    #[must_use]
    pub fn hosting(&self) -> &Arc<Hosting> {
        &self.hosting
    }

    fn process_of_thread(&self, thread: ThreadId) -> Result<Arc<Process>, CommandError> {
        self.hosting.process_of(thread)
    }

    /// Puts up, as the content of the command, a file the adapter made for it, and removes the file
    /// whichever way that ends: it is done, it failed, the system asked for the command not to be carried
    /// out, or this call was dropped (the dispatcher drops a command that has taken longer than it is
    /// allowed). When the system asks, the upload ends at its next piece and the answer says so; the part
    /// that arrived stays with the system, which continues an upload of the same bytes from there.
    async fn upload_made(
        &self,
        call: &Call,
        path: PathBuf,
        kind: CommandKind,
    ) -> Result<BlobRef, CommandError> {
        let file = Removed(path);
        let stop = &call.context.cancel;
        let put = self
            .hosting
            .services()
            .content
            .put(call.context.id, &file.0);
        let uploaded = crate::content::stoppable(stop, u64::from(kind.deadline_secs()), put).await;
        drop(file);
        uploaded.map_err(|error| {
            if stop.is_cancelled() {
                cancelled("the upload of the file")
            } else {
                error
            }
        })
    }

    /// Fetches content named by a command into the command's own directory, ending at the next piece
    /// when the system asks for the command not to be carried out.
    async fn fetch(
        &self,
        call: &Call,
        blob: orch_types::Hash,
        size: i64,
        name: &str,
    ) -> Result<PathBuf, CommandError> {
        let stop = &call.context.cancel;
        let fetch = self
            .hosting
            .services()
            .content
            .fetch(call.context.id, blob, size, name);
        // The allowance of the ordinary opening of a session: what the dispatcher gives the command.
        let allowance = u64::from(CommandKind::SessionOpen.deadline_secs());
        crate::content::stoppable(stop, allowance, fetch)
            .await
            .map_err(|error| {
                if stop.is_cancelled() {
                    cancelled("the fetch of the content")
                } else {
                    error
                }
            })
    }

    /// The processes that can hold the login of a provider: those of its processes whose adapter has a
    /// sign-in of its own, in a fixed order. Which of them is signed in only the adapter knows, so what needs
    /// the login asks them in turn and passes over one that says it has none.
    fn processes_with_login(&self, provider_id: Id) -> Result<Vec<Arc<Process>>, CommandError> {
        let processes = self.hosting.processes_with_login(provider_id);
        if processes.is_empty() {
            return Err(CommandError::new(
                ErrorCode::NotFound,
                "no harness process of this provider holds a sign-in",
            ));
        }
        Ok(processes)
    }

    async fn open(
        &self,
        call: &Call,
        body: SessionOpen,
        mut intent: OpenIntent,
    ) -> Result<Opened, CommandError> {
        self.hosting
            .services()
            .publisher
            .raw_after(
                orch_link_contract::streams::Stream::Session {
                    thread_id: body.thread_id,
                    segment_id: body.segment_id,
                },
                body.raw_after,
            )
            .await
            .map_err(|error| CommandError::new(ErrorCode::Internal, error.to_string()))?;
        let provider_id = provider_of(&body.provider);
        let process = self.hosting.process(provider_id, body.agent_id)?;
        process.check_program_policy()?;
        match &mut intent {
            OpenIntent::Prefix(prefix) => process.check_idle_source(&prefix.source, None)?,
            OpenIntent::Resume {
                source,
                replace_segment,
                retained,
            } => {
                *replace_segment =
                    process.idle_resume_replacement(source, *replace_segment, retained.as_ref())?;
            }
            OpenIntent::Ordinary => {}
            OpenIntent::Guarded(Some(source)) => process.check_idle_source(source, None)?,
            OpenIntent::Guarded(None) if process.session_key(body.thread_id).is_some() => {
                return Err(internal(
                    "The replacement session requires an empty native lane.",
                ));
            }
            OpenIntent::Guarded(None) => {}
        }
        if matches!(&intent, OpenIntent::Ordinary) {
            process.check_refresh(
                body.thread_id,
                body.segment_id,
                body.native_session_ref.as_deref(),
            )?;
        }
        let content = &self.hosting.services().content;
        let command = call.context.id;
        let fenced =
            !matches!(&intent, OpenIntent::Ordinary) || process.continuation_fenced(body.thread_id);
        let outcome = async {
            let archive = match &body.archive {
                Some(archive) => {
                    let path = self
                        .fetch(call, archive.blob, archive.size, "archive")
                        .await?;
                    Some(SessionArchive {
                        format: archive.format.clone(),
                        path: path.display().to_string(),
                        size: archive.size,
                    })
                }
                None => None,
            };
            let history = match &body.history {
                Some(history) => {
                    let path = self
                        .fetch(call, history.blob, history.size, "history.jsonl")
                        .await?;
                    Some(LocalFile {
                        path: path.display().to_string(),
                        size: history.size,
                    })
                }
                None => None,
            };
            // The thread's skills, laid out in its own directory whether it is given any or not, so that a
            // program started now finds the directory its later skills are put into.
            let cache = crate::skills::SkillCache::new(&self.hosting.services().data_dir);
            let mut given = Vec::with_capacity(body.skills.len());
            for skill in &body.skills {
                let version = cache.version_dir(&skill.blob);
                let version = if version.join("SKILL.md").is_file() {
                    version
                } else {
                    let archive = self
                        .fetch(call, skill.blob, skill.size, "skill.tar.gz")
                        .await?;
                    cache.unpack(&skill.blob, &archive).map_err(|error| {
                        internal(format!(
                            "the skill {} could not be laid out: {error}",
                            skill.name
                        ))
                    })?
                };
                given.push((skill.name.clone(), version));
            }
            let skills = Some(cache.lay_out(body.thread_id, &given).map_err(|error| {
                internal(format!(
                    "the thread's skills could not be laid out: {error}"
                ))
            })?);
            let settings: SessionSettings = recast(&body)?;
            let provider = match &body.provider {
                SessionOpenProvider::Native { provider_id } => ProviderAccess::Native {
                    provider_id: *provider_id,
                },
                SessionOpenProvider::Forwarded {
                    provider_id,
                    protocol,
                    ..
                } => ProviderAccess::Forwarded {
                    provider_id: *provider_id,
                    protocol: protocol.clone(),
                },
                SessionOpenProvider::Shared { provider_id, form } => ProviderAccess::Shared {
                    provider_id: *provider_id,
                    form: form.clone(),
                },
                SessionOpenProvider::Handover { provider_id, form } => ProviderAccess::Handover {
                    provider_id: *provider_id,
                    form: form.clone(),
                },
            };
            let tickets = Tickets {
                llm: match &body.provider {
                    SessionOpenProvider::Forwarded { ticket, .. } => signed(ticket),
                    _ => None,
                },
                exec: body.exec.ticket.as_ref().and_then(signed),
                mcp: body
                    .mcp
                    .iter()
                    .filter_map(|server| signed(&server.ticket).map(|t| (server.name.clone(), t)))
                    .collect(),
            };
            if process.session_key(body.thread_id).is_some() {
                process.session_closed(body.thread_id);
            }
            let binding = self
                .hosting
                .bind(&process, body.thread_id, body.segment_id, tickets);
            self.hosting
                .services()
                .loopback
                .set_reach(&binding.session_key, crate::loopback::Reach::of(&body));
            let spec = SessionSpec {
                settings,
                session_key: binding.session_key.clone(),
                archive,
                history,
                provider,
                exec: ExecAccess {
                    executor: body.exec.executor.clone(),
                    tool_set: body.exec.tool_set.clone(),
                    environment_id: body.exec.environment_id,
                    generation: body.exec.generation,
                },
                mcp: body.mcp.iter().map(|server| server.name.clone()).collect(),
                skills,
            };
            let opened = process
                .call(move |adapter| {
                    Box::pin(async move {
                        match intent {
                            OpenIntent::Ordinary | OpenIntent::Guarded(_) => {
                                adapter.open_session(spec).await.map(|session| Opened {
                                    session,
                                    prefix: None,
                                })
                            }
                            OpenIntent::Prefix(prefix) => adapter
                                .open_input_prefix(spec, prefix)
                                .await
                                .map(|opened| Opened {
                                    session: opened.session,
                                    prefix: Some(opened.prefix),
                                }),
                            OpenIntent::Resume {
                                source,
                                replace_segment,
                                ..
                            } => adapter
                                .resume_continuation(spec, source, replace_segment)
                                .await
                                .map(|session| Opened {
                                    session,
                                    prefix: None,
                                }),
                        }
                    })
                })
                .await;
            match opened {
                Ok(opened) => {
                    process.session_opened(&binding, &opened.session.native_session_ref);
                    if fenced {
                        process.fence_continuation(body.thread_id);
                    }
                    self.hosting
                        .report_clamps(body.thread_id, body.segment_id, &call.clamps)
                        .await;
                    Ok(opened)
                }
                Err(error) => {
                    self.hosting
                        .services()
                        .loopback
                        .unbind(&binding.session_key);
                    Err(error)
                }
            }
        }
        .await;
        content.finish(command).await;
        outcome
    }

    async fn start(
        &self,
        call: &Call,
        body: orch_link_contract::commands::TurnStart,
        expected: Option<Id>,
    ) -> Result<Empty, CommandError> {
        let process = self.process_of_thread(body.thread_id)?;
        process.check_program_policy()?;
        process.check_segment(body.thread_id, expected)?;
        let selection = native_selection(
            self.hosting.segment_of(body.thread_id),
            body.native_action.as_ref(),
        )?;
        self.hosting
            .services()
            .loopback
            .note_turn(body.thread_id, body.sandbox_mode);
        let images = self.images(call, &body.images).await?;
        if !images.is_empty() {
            process.keep_for_turn(body.turn_id, call.context.id);
        }
        let turn = AdapterTurn {
            thread_id: body.thread_id,
            turn_id: body.turn_id,
            text: body.text,
            images,
            model: body.model,
            effort: body.effort,
            params: Json::Object(body.params),
            approval_mode: recast(&body.approval_mode)?,
            sandbox_mode: recast(&body.sandbox_mode)?,
            plan: body.plan,
        };
        process.reserve_turn(body.thread_id, Some(body.turn_id));
        if let Err(error) = process
            .call(move |adapter| {
                Box::pin(async move {
                    match selection {
                        Some(selection) => adapter.start_native_turn(turn, selection).await,
                        None => adapter.start_turn(turn).await,
                    }
                })
            })
            .await
        {
            process.reserve_turn(body.thread_id, None);
            return Err(error);
        }
        if let Some(segment) = self.hosting.segment_of(body.thread_id) {
            self.hosting
                .report_clamps(body.thread_id, segment, &call.clamps)
                .await;
        }
        Ok(Empty {})
    }

    async fn images(
        &self,
        call: &Call,
        images: &[TurnImage],
    ) -> Result<Vec<ImageInput>, CommandError> {
        let content = &self.hosting.services().content;
        let mut fetched = Vec::with_capacity(images.len());
        for image in images {
            let size = content.size_of(call.context.id, image.blob).await?;
            let path = content
                .fetch(call.context.id, image.blob, size, &image.name)
                .await?;
            fetched.push(ImageInput {
                blob: image.blob,
                media_type: image.media_type.clone(),
                name: image.name.clone(),
                path: Some(path.display().to_string()),
            });
        }
        Ok(fetched)
    }

    /// The values of a sign-in as the adapter takes them: a text as it is, a boolean from `true` or
    /// `false`, and a map from a JSON object of texts, each by the type of the field it was given for. A
    /// command carries every value as text; a value with no field is left a text, and the adapter says
    /// whether it fits.
    fn unseal_values(
        &self,
        command: Id,
        values: &BTreeMap<String, SecretOrText>,
        fields: &[Field],
    ) -> Result<LoginValues, CommandError> {
        let keys = &self.hosting.services().keys;
        let mut opened = BTreeMap::new();
        for (name, value) in values {
            let text = match value {
                SecretOrText::Text(text) => text.clone(),
                SecretOrText::Secret(LinkSecret::Sealed(sealed)) => keys.open(command, sealed)?,
                SecretOrText::Secret(LinkSecret::Ref(_)) => {
                    return Err(CommandError::new(
                        ErrorCode::Internal,
                        format!("{name} is a reference to a secret that was not replaced"),
                    ));
                }
            };
            let kind = fields
                .iter()
                .find(|field| &field.name == name)
                .map(|field| field.kind);
            opened.insert(name.clone(), typed(name, kind, text)?);
        }
        Ok(LoginValues(opened))
    }
}

/// A value of a sign-in in the type of its field. What does not read as the type is refused, naming the
/// field, and never showing the value: it may be a secret.
fn typed(name: &str, kind: Option<FieldType>, text: String) -> Result<Json, CommandError> {
    let unfit = |what: &str| {
        CommandError::new(
            ErrorCode::Internal,
            format!("{name} is not {what}, as the field says it is"),
        )
    };
    match kind {
        Some(FieldType::Boolean) => match text.as_str() {
            "true" => Ok(Json::Bool(true)),
            "false" => Ok(Json::Bool(false)),
            _ => Err(unfit("true or false")),
        },
        Some(FieldType::Map | FieldType::SecretMap) => match serde_json::from_str::<Json>(&text) {
            Ok(object @ Json::Object(_)) => Ok(object),
            _ => Err(unfit("names with values, written as a JSON object")),
        },
        _ => Ok(Json::String(text)),
    }
}

#[async_trait]
impl RoleHandler for AgentRole {
    fn role(&self) -> Role {
        Role::Agent
    }

    fn takes(&self) -> &'static [CommandKind] {
        TAKES
    }

    /// The models and the state of a provider are asked of the machine that holds its sign-in or of the one
    /// that holds its key: this role answers for a provider whose sign-in one of its harness processes holds.
    fn claims(&self, command: &Command) -> bool {
        match command {
            Command::UnitAfterUp(body) => {
                body.environment_id.is_none()
                    && body.provider_id.is_some()
                    && body.agent_id.is_some()
            }
            Command::ProviderRefresh(ProviderRefresh { provider_id, .. })
            | Command::ProviderDiscoverModels(ProviderDiscoverModels { provider_id, .. }) => {
                self.processes_with_login(*provider_id).is_ok()
            }
            Command::ProviderResetConsume(body) => {
                self.processes_with_login(body.provider_id).is_ok()
            }
            _ => true,
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one arm for each command of the role is the clearest form of the table"
    )]
    async fn handle(&self, call: Call, command: Command) -> Result<CommandResult, CommandError> {
        Ok(match command {
            Command::UnitAfterUp(body) => {
                let process = self.hosting.process(
                    body.provider_id
                        .ok_or_else(|| internal("a provider is required"))?,
                    body.agent_id
                        .ok_or_else(|| internal("an agent profile is required"))?,
                )?;
                let (home, variables) = process.after_up_context()?;
                CommandResult::UnitAfterUp(
                    self.after_up
                        .run_in(&body, &home, &variables, &process.resource_launcher())
                        .await?,
                )
            }
            Command::AgentStart(body) => {
                let (harness_version, descriptor, reused) =
                    self.hosting.start(Some(call.context.id), &body).await?;
                CommandResult::AgentStart(AgentStartResult {
                    reused,
                    harness_version,
                    descriptor,
                })
            }
            Command::AgentStop(body) => {
                let logout = if body.logout == Some(true) {
                    match self.hosting.process(body.provider_id, body.agent_id) {
                        Ok(process) => match process.sign_in_allowed() {
                            Ok(()) => process.call(|adapter| adapter.logout()).await,
                            Err(error) => Err(error),
                        },
                        Err(error) => Err(error),
                    }
                } else {
                    Ok(())
                };
                let stopped = self.hosting.stop(body.provider_id, body.agent_id).await;
                if self
                    .hosting
                    .processes_with_login(body.provider_id)
                    .is_empty()
                {
                    self.hosting
                        .services()
                        .source_runtime
                        .forget(body.provider_id)
                        .await;
                }
                stopped?;
                logout?;
                CommandResult::AgentStop(Empty {})
            }
            Command::AgentDiscoverSessions(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                let sessions: Vec<NativeSession> =
                    process.call(|adapter| adapter.discover_sessions()).await?;
                CommandResult::AgentDiscoverSessions(AgentDiscoverSessionsResult { sessions })
            }
            Command::AgentReadSession(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                let reference = body.native_session_ref.clone();
                let write = body.write;
                let record = process
                    .call(move |adapter| {
                        Box::pin(async move { adapter.read_session(&reference, write).await })
                    })
                    .await?;
                // The record goes up as content of this command, and its file does not stay behind.
                let uploaded = match &record.file {
                    Some(file) => {
                        let blob = self
                            .upload_made(
                                &call,
                                PathBuf::from(&file.path),
                                CommandKind::AgentReadSession,
                            )
                            .await?;
                        Some(RecordRef {
                            blob: blob.hash,
                            size: blob.size,
                        })
                    }
                    None => None,
                };
                CommandResult::AgentReadSession(AgentReadSessionResult {
                    model: record.model,
                    effort: record.effort,
                    turns: record.turns,
                    entries: record.entries,
                    record: uploaded,
                    report: record.report,
                })
            }
            Command::SessionOpen(body) => CommandResult::SessionOpen(
                self.open(&call, body, OpenIntent::Ordinary)
                    .await?
                    .ordinary(),
            ),
            Command::SessionOpenFenced(body) => {
                if body.thread_id != body.session.thread_id
                    || body
                        .predecessor
                        .as_ref()
                        .is_some_and(|source| source.thread_id != body.thread_id)
                {
                    return Err(internal("The replacement session names another thread."));
                }
                CommandResult::SessionOpenFenced(
                    self.open(&call, *body.session, OpenIntent::Guarded(body.predecessor))
                        .await?
                        .ordinary(),
                )
            }
            Command::SessionPrefixInspect(body) => {
                if body.thread_id != body.source.thread_id {
                    return Err(internal("The prefix source names another thread."));
                }
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.check_idle_source(&body.source, None)?;
                CommandResult::SessionPrefixInspect(
                    process
                        .call(move |adapter| {
                            Box::pin(async move {
                                adapter
                                    .inspect_input_prefix(body.source, &body.before_input_ref)
                                    .await
                            })
                        })
                        .await?,
                )
            }
            Command::SessionPrefixOpen(body) => {
                if body.thread_id != body.session.thread_id
                    || body.thread_id != body.prefix.source.thread_id
                {
                    return Err(internal("The prefix session names another thread."));
                }
                let opened = self
                    .open(&call, *body.session, OpenIntent::Prefix(body.prefix))
                    .await?;
                CommandResult::SessionPrefixOpen(NativePrefixOpened {
                    session: opened.session,
                    prefix: opened.prefix.ok_or_else(|| {
                        internal("The native program returned no applied prefix.")
                    })?,
                })
            }
            Command::SessionContinuationResume(body) => {
                if body.thread_id != body.session.thread_id
                    || body.thread_id != body.source.thread_id
                {
                    return Err(internal("The retained session names another thread."));
                }
                CommandResult::SessionContinuationResume(
                    self.open(
                        &call,
                        *body.session,
                        OpenIntent::Resume {
                            source: body.source,
                            replace_segment: body.replace_segment_id,
                            retained: body.replace_retained_source,
                        },
                    )
                    .await?
                    .ordinary(),
                )
            }
            Command::SessionContinuationClose(body) => {
                if body.thread_id != body.source.thread_id {
                    return Err(internal("The retained session names another thread."));
                }
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.check_idle_source(&body.source, None)?;
                process
                    .call(|adapter| adapter.close_session(body.thread_id))
                    .await?;
                process.session_closed(body.thread_id);
                CommandResult::SessionContinuationClose(Empty {})
            }
            Command::AgentHistoryHomeCleanup(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                let items = process
                    .call(|adapter| adapter.cleanup_history_home(body.before))
                    .await?;
                CommandResult::AgentHistoryHomeCleanup(
                    orch_link_contract::commands::AgentHistoryHomeCleanupResult { items },
                )
            }
            Command::AgentHistoryManage(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                let requested: Vec<_> = body
                    .sources
                    .iter()
                    .map(|source| source.source.clone())
                    .collect();
                let facts = process
                    .call(|adapter| adapter.manage_history(body.sources, body.remove))
                    .await?;
                if facts.len() != requested.len()
                    || facts
                        .iter()
                        .zip(&requested)
                        .any(|(fact, source)| &fact.source != source)
                {
                    return Err(internal(
                        "The native history results do not match the requested sessions.",
                    ));
                }
                if body.remove {
                    for fact in &facts {
                        if fact.removed && fact.error.is_none() {
                            process.history_removed(&fact.source);
                        }
                    }
                }
                CommandResult::AgentHistoryManage(
                    orch_link_contract::commands::AgentHistoryManageResult { facts },
                )
            }
            Command::SessionClose(body) => {
                if let Ok(process) = self.process_of_thread(body.thread_id) {
                    process.check_segment(body.thread_id, None)?;
                    process
                        .call(|adapter| adapter.close_session(body.thread_id))
                        .await?;
                    process.session_closed(body.thread_id);
                }
                CommandResult::SessionClose(Empty {})
            }
            Command::SessionCloseFenced(body) => {
                if let Ok(process) = self.process_of_thread(body.thread_id) {
                    process.check_segment(body.thread_id, Some(body.segment_id))?;
                    process
                        .call(|adapter| adapter.close_session(body.thread_id))
                        .await?;
                    process.session_closed(body.thread_id);
                }
                CommandResult::SessionCloseFenced(Empty {})
            }
            Command::SessionMcpReload(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                if !process.has_segment(body.thread_id, body.segment_id) {
                    return Err(internal("the MCP reload names another segment"));
                }
                if !process
                    .started()
                    .1
                    .is_some_and(|descriptor| descriptor.capabilities.mcp.reload)
                {
                    return Err(CommandError::new(
                        ErrorCode::Unsupported,
                        "the adapter takes MCP changes at its next turn",
                    ));
                }
                let key = process
                    .session_key(body.thread_id)
                    .ok_or_else(|| internal("the session is not open"))?;
                let tickets = body
                    .mcp
                    .iter()
                    .filter_map(|server| {
                        signed(&server.ticket).map(|ticket| (server.name.clone(), ticket))
                    })
                    .collect();
                let names = body.mcp.iter().map(|server| server.name.clone()).collect();
                let loopback = &self.hosting.services().loopback;
                let previous = loopback
                    .replace_mcp(&key, tickets)
                    .ok_or_else(|| internal("the session's tool routes are gone"))?;
                if let Err(error) = process
                    .call(|adapter| adapter.reload_mcp(body.thread_id, names))
                    .await
                {
                    loopback.replace_mcp(&key, previous);
                    return Err(error);
                }
                CommandResult::SessionMcpReload(Empty {})
            }
            Command::SessionExport(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                let kind: ExportKind = recast(&body.kind)?;
                // The program writes the archive. When the system asks for the command not to be carried
                // out the asking ends: a program that writes its archive a page at a time stops at the
                // next page and removes what it wrote, as the contract of `export_session` says.
                let archive = tokio::select! {
                    biased;
                    () = call.context.cancel.cancelled() => {
                        return Err(cancelled("the export of the session"));
                    }
                    archive = process.call(|adapter| {
                        adapter.export_session(body.thread_id, kind, body.upto_turn_ordinal)
                    }) => archive?,
                };
                let blob = self
                    .upload_made(
                        &call,
                        PathBuf::from(&archive.path),
                        CommandKind::SessionExport,
                    )
                    .await?;
                CommandResult::SessionExport(SessionExportResult {
                    format: archive.format,
                    blob: blob.hash,
                    size: blob.size,
                })
            }
            Command::SessionContext(body) => {
                // Only the session of that segment, where it is open: a count never opens one.
                let process = self
                    .hosting
                    .process_of_segment(body.thread_id, body.segment_id)?;
                let fill = if process.has_segment(body.thread_id, body.segment_id) {
                    process
                        .call(|adapter| adapter.count_context(body.thread_id))
                        .await?
                } else {
                    None
                };
                CommandResult::SessionContext(orch_link_contract::commands::SessionContextResult {
                    fill,
                })
            }
            Command::SessionBackground(body) => {
                let process = self
                    .hosting
                    .process_of_segment(body.thread_id, body.segment_id)?;
                if !process.has_segment(body.thread_id, body.segment_id) {
                    return Err(CommandError::new(
                        ErrorCode::SessionNotFound,
                        "that native session generation is no longer open",
                    ));
                }
                let initial = if body.cursor.is_none() {
                    Some(
                        process
                            .call(|adapter| adapter.background_commands(body.thread_id))
                            .await?,
                    )
                } else {
                    None
                };
                CommandResult::SessionBackground(self.background.page(
                    body.thread_id,
                    body.segment_id,
                    body.cursor.as_deref(),
                    initial,
                )?)
            }
            Command::SessionNativeActions(body) => {
                let process = self
                    .hosting
                    .process_of_segment(body.thread_id, body.segment_id)?;
                if !process.has_segment(body.thread_id, body.segment_id) {
                    return Err(CommandError::new(
                        ErrorCode::SessionNotFound,
                        "that native session generation is no longer open",
                    ));
                }
                let mut catalog = process
                    .call(|adapter| adapter.native_actions(body.thread_id, body.refresh))
                    .await?;
                if catalog.state == orch_agent_contract::native::NativeCatalogState::Ready {
                    catalog.revision = format!("{}:{}", body.segment_id, catalog.revision);
                }
                CommandResult::SessionNativeActions(catalog)
            }
            Command::SessionBackgroundEnd(body) => {
                let process = self
                    .hosting
                    .process_of_segment(body.thread_id, body.segment_id)?;
                if !process.has_segment(body.thread_id, body.segment_id) {
                    return Err(CommandError::new(
                        ErrorCode::SessionNotFound,
                        "that native session generation is no longer open",
                    ));
                }
                CommandResult::SessionBackgroundEnd(
                    process
                        .call(move |adapter| {
                            Box::pin(async move {
                                adapter
                                    .end_background_command(body.thread_id, &body.command_id)
                                    .await
                            })
                        })
                        .await?,
                )
            }
            Command::SessionResync(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                let items = process
                    .call(|adapter| adapter.resync_session(body.thread_id))
                    .await?;
                CommandResult::SessionResync(SessionResyncResult {
                    items: i64::from(items),
                })
            }
            Command::TurnStart(body) => {
                CommandResult::TurnStart(self.start(&call, body, None).await?)
            }
            Command::TurnStartFenced(body) => {
                if body.thread_id != body.turn.thread_id {
                    return Err(internal("The turn names another thread."));
                }
                CommandResult::TurnStartFenced(
                    self.start(&call, *body.turn, Some(body.segment_id)).await?,
                )
            }
            Command::TurnSteer(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                process.check_program_policy()?;
                let selection = native_selection(
                    self.hosting.segment_of(body.thread_id),
                    body.native_action.as_ref(),
                )?;
                let images = self.images(&call, &body.images).await?;
                if !images.is_empty() {
                    process.keep_for_turn(body.turn_id, call.context.id);
                }
                let steer = AdapterSteer {
                    thread_id: body.thread_id,
                    turn_id: body.turn_id,
                    text: body.text.clone(),
                    images,
                };
                process
                    .call(move |adapter| {
                        Box::pin(async move {
                            match selection {
                                Some(selection) => adapter.steer_native(steer, selection).await,
                                None => adapter.steer(steer).await,
                            }
                        })
                    })
                    .await?;
                CommandResult::TurnSteer(Empty {})
            }
            Command::TurnInterrupt(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                process
                    .call(|adapter| adapter.interrupt(body.thread_id, body.turn_id))
                    .await?;
                CommandResult::TurnInterrupt(Empty {})
            }
            Command::ApprovalResolve(body) => {
                let process = self.process_of_thread(body.thread_id)?;
                let decision: ApprovalDecision = body.decision.clone();
                let (thread, request_ref) = (body.thread_id, body.request_ref.clone());
                process
                    .call(move |adapter| {
                        Box::pin(async move {
                            adapter
                                .resolve_approval(thread, &request_ref, decision)
                                .await
                        })
                    })
                    .await?;
                CommandResult::ApprovalResolve(Empty {})
            }
            Command::OrchToolResult(body) => {
                // The answer to a call that a harness made through the listener is the listener's; an adapter
                // that makes its calls itself is handed its own.
                if self.hosting.orch_calls().answer(
                    body.thread_id,
                    &body.call_ref,
                    match (&body.error, &body.result) {
                        (Some(error), _) => Err(orch_link_contract::commands::ToolError {
                            code: error.code.clone(),
                            message: error.message.clone(),
                        }),
                        (None, result) => Ok(result.clone().unwrap_or(Json::Null)),
                    },
                ) {
                    return Ok(CommandResult::OrchToolResult(Empty {}));
                }
                let process = self.process_of_thread(body.thread_id)?;
                let result: Result<Json, ToolError> = match body.error {
                    Some(error) => Err(ToolError {
                        code: error.code,
                        message: error.message,
                    }),
                    None => Ok(body.result.unwrap_or(Json::Null)),
                };
                let (thread, call_ref) = (body.thread_id, body.call_ref.clone());
                process
                    .call(move |adapter| {
                        Box::pin(
                            async move { adapter.tool_result(thread, &call_ref, result).await },
                        )
                    })
                    .await?;
                CommandResult::OrchToolResult(Empty {})
            }
            Command::ProviderLoginStart(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.sign_in_allowed()?;
                let fields = process.login_fields(&body.method);
                let values = match &body.values {
                    Some(values) => self.unseal_values(call.context.id, values, &fields)?,
                    None => LoginValues::default(),
                };
                let request = LoginRequest {
                    method: body.method.clone(),
                    values,
                    config: body.config.clone(),
                };
                process.call(|adapter| adapter.login(request)).await?;
                CommandResult::ProviderLoginStart(Empty {})
            }
            Command::ProviderLoginInput(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.sign_in_allowed()?;
                let values =
                    self.unseal_values(call.context.id, &body.values, &process.login_asks())?;
                process.call(|adapter| adapter.login_input(values)).await?;
                CommandResult::ProviderLoginInput(Empty {})
            }
            Command::ProviderLoginCancel(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.call(|adapter| adapter.cancel_login()).await?;
                CommandResult::ProviderLoginCancel(Empty {})
            }
            Command::ProviderLogout(body) => {
                let process = self.hosting.process(body.provider_id, body.agent_id)?;
                process.sign_in_allowed()?;
                process.call(|adapter| adapter.logout()).await?;
                CommandResult::ProviderLogout(Empty {})
            }
            Command::ProviderRefresh(body) => {
                let source = crate::sources::dispatch::Context::native(
                    Arc::clone(self.hosting.services()),
                    call.context.id,
                    body.provider_id,
                    self.processes_with_login(body.provider_id)?,
                    body.renew.unwrap_or(false),
                );
                CommandResult::ProviderRefresh(source.refresh(body.facts.as_ref()).await)
            }
            Command::ProviderResetConsume(body) => {
                let source = crate::sources::dispatch::Context::native(
                    Arc::clone(self.hosting.services()),
                    call.context.id,
                    body.provider_id,
                    self.processes_with_login(body.provider_id)?,
                    false,
                );
                let result = source
                    .reset(
                        body.facts.as_ref(),
                        crate::sources::dispatch::reset_request(
                            body.provider_id,
                            body.request_id,
                            body.reset_id,
                        ),
                    )
                    .await
                    .unwrap_or_else(crate::sources::dispatch::failed_reset);
                CommandResult::ProviderResetConsume(result)
            }
            Command::ProviderDiscoverModels(body) => {
                let source = crate::sources::dispatch::Context::native(
                    Arc::clone(self.hosting.services()),
                    call.context.id,
                    body.provider_id,
                    self.processes_with_login(body.provider_id)?,
                    false,
                );
                CommandResult::ProviderDiscoverModels(source.discover(body.models.as_ref()).await)
            }
            Command::AgentCredentialReturn(AgentCredentialReturn {
                provider_id,
                agent_id,
                custodian,
            }) => {
                let process = self.hosting.process(provider_id, agent_id)?;
                let given = process.call(|adapter| adapter.return_credential()).await?;
                let sealed = self.hosting.services().keys.seal_with(
                    &custodian.public_key,
                    &exchange_info(Direction::Returned, provider_id, &given.form),
                    given.credential.expose(),
                )?;
                CommandResult::AgentCredentialReturn(SealedCredential {
                    form: given.form,
                    credential: Sealed { value: sealed },
                    expires_at: given.expires_at,
                })
            }
            Command::AgentCredentialSet(AgentCredentialSet {
                provider_id,
                agent_id,
                form,
                credential,
                expires_at,
                upstream,
                ..
            }) => {
                let process = self.hosting.process(provider_id, agent_id)?;
                if upstream.is_none() {
                    process.sign_in_allowed()?;
                }
                let opened = self.hosting.services().keys.open_with(
                    &exchange_info(Direction::Issued, provider_id, &form),
                    &credential.value,
                )?;
                let given = GivenCredential {
                    form,
                    credential: Secret::new(opened),
                    expires_at,
                    upstream: upstream.map(|target| UpstreamTarget {
                        protocol: target.protocol,
                        base_url: target.base_url,
                    }),
                };
                process
                    .call(|adapter| adapter.set_credential(given))
                    .await?;
                CommandResult::AgentCredentialSet(Empty {})
            }
            other => {
                return Err(CommandError::new(
                    ErrorCode::Unsupported,
                    format!("the agent role does not take {}", other.kind()),
                ));
            }
        })
    }

    fn contribute(&self, facts: &mut FactsDraft) {
        self.hosting.contribute(facts);
        facts.provider_home_storage =
            Some(orch_driver_contract::descriptor::EnvironmentDataStorage {
                made_by_orch: Some(orch_driver_contract::descriptor::MadeDataKind::Directory),
                existing_directory: cfg!(target_os = "linux"),
                reason: (!cfg!(target_os = "linux")).then(|| {
                    "this platform has not verified roots-bounded existing harness homes".into()
                }),
            });
    }

    async fn recover(&self) {
        crate::reconciliation::reconcile(&self.hosting).await;
    }

    async fn stop(&self) {
        self.hosting.stop_all().await;
    }
}

/// What an adapter's error is as a command's, for a caller that holds the adapter itself.
#[must_use]
pub fn refusal_of(error: &orch_agent_contract::adapter::AdapterError) -> CommandError {
    command_error(error)
}

/// A fault of this side: what the server is told when the connector itself cannot do what was asked.
fn internal(detail: impl Into<String>) -> CommandError {
    CommandError::new(ErrorCode::Internal, detail)
}

/// What a command that the system asked not to be carried out answers: it stopped, and says what.
fn cancelled(what: &str) -> CommandError {
    CommandError::new(
        ErrorCode::Conflict,
        format!(
            "{what} was stopped because the system asked for the command not to be carried out"
        ),
    )
}

/// A file made for one command that nothing else keeps: removed when this is dropped, whatever the
/// way the command ends, so that an archive of gigabytes is not left behind by a command that was
/// given up, cancelled or timed out. A file that is gone already is not a fault.
struct Removed(PathBuf);

impl Drop for Removed {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_file(&self.0)
            && error.kind() != std::io::ErrorKind::NotFound
        {
            tracing::warn!(%error, "a file made for a command could not be removed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Removed, cancelled};
    use orch_link_contract::commands::ErrorCode;
    use std::time::Duration;

    #[test]
    fn a_made_file_is_removed_with_its_holder_and_a_file_already_gone_is_no_fault() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("archive");
        std::fs::write(&path, b"bytes").unwrap();
        drop(Removed(path.clone()));
        assert!(!path.exists());
        drop(Removed(path));
    }

    #[tokio::test]
    async fn a_made_file_is_removed_when_the_command_is_dropped_part_way() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("archive");
        std::fs::write(&path, b"bytes").unwrap();
        let held = path.clone();
        // The dispatcher drops a command that has taken longer than it is allowed.
        let command = async move {
            let _file = Removed(held);
            std::future::pending::<()>().await;
        };
        assert!(
            tokio::time::timeout(Duration::from_millis(50), command)
                .await
                .is_err()
        );
        assert!(!path.exists());
    }

    #[test]
    fn a_command_given_up_answers_with_a_conflict_that_says_what_stopped() {
        let answer = cancelled("the export of the session");
        assert_eq!(answer.known(), Some(ErrorCode::Conflict));
        assert!(
            answer
                .detail
                .starts_with("the export of the session was stopped")
        );
    }
}
