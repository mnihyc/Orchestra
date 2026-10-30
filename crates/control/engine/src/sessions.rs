//! Open, resume, fork for a side thread, segment and lost-session recovery.
//!
//! A thread's conversation with its harness is a sequence of segments, each
//! a stretch on one agent profile, provider and environment. The first is
//! made with the thread; a segment ends when the unit of its environment is
//! made anew or the thread is put on something else, and the next begins.
//! A segment has one session in the harness, opened by `session.open`: in
//! mode `new` the first time, and in mode `resume` whenever the harness has
//! lost it (it was restarted, or the thread was brought back from the
//! archive). A harness that answers `session_not_found` to a resume has
//! lost the conversation itself: the segment ends as `error` and the next
//! begins.
//!
//! **Assumed unbuilt.** The history of a lost conversation is carried to the
//! new segment by an import of the neutral history, which needs the written
//! form of a conversation and the blob store; until those exist the new
//! segment begins empty and says so in a line of the timeline. Forking a
//! session for a side thread, and the history imports, are likewise later.

use crate::binding::{self, Facts};
use crate::engine::Engine;
use crate::error::EngineError;
use crate::harnesses::{self, HarnessKey};
use crate::instructions::{self, Place};
use crate::resolved::Resolved;
use orch_agent_contract::arrangement::Arrangement;
use orch_agent_contract::credentials::CredentialPath;
use orch_agent_contract::descriptor::{
    AgentDescriptor, ApprovalMode, ArrangementType, OrchToolsSupport, SandboxMode, sandbox_for_turn,
};
use orch_agent_contract::parameters::ModelParameters;
use orch_agent_contract::session::{MemoryPlan, NoteKeeper};
use orch_db::Conn;
use orch_db::queries::environments::thread_engine::{self as environments, EnvironmentRow};
use orch_db::queries::resources::thread_engine::{
    self as resources, AgentRow, MachinePlace, ModelRow, ProviderRow,
};
use orch_db::queries::threads::{
    self, InputRow, LeaseRow, NewSegment, SegmentRow, ThreadRow, TurnRow,
};
use orch_link_contract::commands as link;
use orch_link_contract::tunnels::{Service, TunnelKind};
use orch_types::{Id, Json, Seq, new_id};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

/// The machine's spelling of this generation's workspace, without changing its registration.
/// Facts from another generation cannot identify the current workspace.
#[must_use]
pub(crate) fn workspace_root<'a>(registered: &'a str, facts: &'a Json, generation: i32) -> &'a str {
    if facts.get("generation").and_then(Json::as_i64) != Some(i64::from(generation)) {
        return registered;
    }
    facts
        .get("workspace_root")
        .and_then(Json::as_str)
        .filter(|root| {
            let bytes = root.as_bytes();
            root.starts_with('/')
                || root.starts_with("\\\\")
                || (bytes.len() >= 3
                    && bytes[0].is_ascii_alphabetic()
                    && bytes[1] == b':'
                    && matches!(bytes[2], b'/' | b'\\'))
        })
        .unwrap_or(registered)
}

/// The absolute path of a thread's directory: the workspace root, then the
/// directory relative to it.
#[must_use]
pub fn absolute_directory(workspace_root: &str, directory: &str) -> String {
    let root = workspace_root.trim_end_matches('/');
    if directory.is_empty() {
        if root.is_empty() {
            "/".to_owned()
        } else {
            root.to_owned()
        }
    } else {
        format!("{root}/{}", directory.trim_matches('/'))
    }
}

/// The path from the workspace root where the agent starts: its directory, then `cwd` (relative to it).
/// Empty when that is the root itself.
#[must_use]
pub fn workspace_path(directory: &str, cwd: &str) -> String {
    let (directory, cwd) = (directory.trim_matches('/'), cwd.trim_matches('/'));
    match (directory.is_empty(), cwd.is_empty()) {
        (true, _) => cwd.to_owned(),
        (false, true) => directory.to_owned(),
        (false, false) => format!("{directory}/{cwd}"),
    }
}

/// The absolute path where the agent starts: its directory, then `cwd`
/// (relative to it).
#[must_use]
pub fn absolute_cwd(workspace_root: &str, directory: &str, cwd: &str) -> String {
    let base = absolute_directory(workspace_root, directory);
    let cwd = cwd.trim_matches('/');
    if cwd.is_empty() {
        base
    } else {
        format!("{}/{cwd}", base.trim_end_matches('/'))
    }
}

/// Writes one contract enum as the other contract has it: both write the
/// same words.
pub(crate) fn convert<T: Serialize, U: DeserializeOwned>(value: &T) -> Result<U, EngineError> {
    Ok(serde_json::from_value(serde_json::to_value(value)?)?)
}

fn sandbox_word(value: Option<&Json>) -> Option<SandboxMode> {
    value
        .and_then(Json::as_str)
        .and_then(|word| serde_json::from_value(Json::String(word.into())).ok())
}

fn sandbox_ceiling(
    environment: &orch_db::queries::environment_facts::EnvironmentMachine,
    template: Option<&Json>,
) -> (
    SandboxMode,
    orch_api_contract::environment::SandboxLimitSource,
    SandboxMode,
    Option<SandboxMode>,
) {
    use orch_api_contract::environment::SandboxLimitSource as Source;
    let fallback = sandbox_word(Some(&json!(
        orch_settings::supplied::agent_defaults(environment.confinement()).1
    )))
    .unwrap_or(SandboxMode::Workspace);
    let explicit_machine = sandbox_word(
        environment
            .policy
            .as_ref()
            .and_then(|policy| policy.get("agent.max_sandbox")),
    );
    let machine = explicit_machine.unwrap_or(fallback);
    let source = if explicit_machine.is_some() {
        Source::MachinePolicy
    } else {
        Source::Default
    };
    let Some(template) = template else {
        return (machine, source, machine, None);
    };
    let explicit_template = sandbox_word(template.pointer("/sandbox/max"));
    let ceiling = explicit_template.unwrap_or(fallback);
    if ceiling < machine {
        (
            ceiling,
            if explicit_template.is_some() {
                Source::Template
            } else {
                Source::Default
            },
            machine,
            Some(ceiling),
        )
    } else {
        (machine, source, machine, Some(ceiling))
    }
}

/// A missing template contributes no limit. An actual template retains its conservative default.
pub fn widest_sandbox(
    environment: &orch_db::queries::environment_facts::EnvironmentMachine,
    template: Option<&Json>,
) -> SandboxMode {
    sandbox_ceiling(environment, template).0
}

/// One truthful capability/ceiling comparison for turn admission and resource readiness.
pub fn sandbox_readiness(
    requested: SandboxMode,
    agent_modes: Option<&[SandboxMode]>,
    facts: &Json,
    machine: &orch_db::queries::environment_facts::EnvironmentMachine,
    template: Option<&Json>,
    executor: Option<&str>,
) -> orch_api_contract::environment::SandboxReadiness {
    let (widest, source, machine_limit, template_limit) = sandbox_ceiling(machine, template);
    let (enforceable, reason) = if let Some(kind) = executor {
        let reported = facts
            .get("executors")
            .and_then(Json::as_array)
            .and_then(|items| {
                items
                    .iter()
                    .find(|item| item.get("kind").and_then(Json::as_str) == Some(kind))
            });
        match reported {
            Some(fact) if fact.get("available").and_then(Json::as_bool) != Some(false) => {
                let modes = fact
                    .get("sandbox_modes")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<Vec<SandboxMode>>(value).ok())
                    .unwrap_or_default();
                (
                    modes,
                    fact.get("reason").and_then(Json::as_str).map(str::to_owned),
                )
            }
            Some(fact) => (
                Vec::new(),
                Some(
                    fact.get("reason")
                        .and_then(Json::as_str)
                        .unwrap_or("This executor is unavailable.")
                        .into(),
                ),
            ),
            None => (
                Vec::new(),
                Some("This executor has not reported which sandbox it can enforce.".into()),
            ),
        }
    } else {
        (agent_modes.unwrap_or_default().to_vec(), None)
    };
    let usable: Vec<_> = enforceable
        .iter()
        .copied()
        .filter(|mode| agent_modes.is_none_or(|agent| agent.contains(mode)))
        .collect();
    orch_api_contract::environment::SandboxReadiness {
        ready: sandbox_for_turn(requested, widest, &usable).is_some(),
        requested,
        widest,
        source,
        machine_limit,
        template_limit,
        executor: executor.map(str::to_owned),
        executor_ref: crate::programs::executor_ref(
            orch_agent_programs::Programs::builtin(),
            executor,
            None,
        ),
        enforceable,
        agent_modes: agent_modes.unwrap_or_default().to_vec(),
        reason,
    }
}

fn sandbox_name(mode: SandboxMode) -> &'static str {
    match mode {
        SandboxMode::ReadOnly => "Read only",
        SandboxMode::Workspace => "Workspace",
        SandboxMode::Open => "Full access",
    }
}

/// A refusal identifies the actual requested mode, limit source and reported executor capability.
pub fn sandbox_explanation(
    name: &str,
    report: &orch_api_contract::environment::SandboxReadiness,
) -> String {
    use orch_api_contract::environment::SandboxLimitSource as Source;
    let source = match report.source {
        Source::MachinePolicy => "machine policy",
        Source::Template => "template limit",
        Source::Default => "cautious default",
    };
    let modes = report
        .enforceable
        .iter()
        .map(|mode| sandbox_name(*mode))
        .collect::<Vec<_>>()
        .join(", ");
    // An executor is called by the title its manifest gives; one nobody described is called by its kind.
    let executor = match report.executor.as_deref() {
        Some(kind) => orch_agent_programs::Programs::builtin()
            .executor(kind)
            .map_or(kind, |executor| executor.title.en()),
        None => "this agent's own tools",
    };
    let capability = if modes.is_empty() {
        format!("{executor} has reported no usable sandbox")
    } else if report.enforceable.len() == 1
        && report.enforceable.first() == Some(&SandboxMode::Open)
    {
        format!(
            "{executor} cannot confine commands; it supports Full access within the environment"
        )
    } else {
        format!("{executor} can enforce {modes}")
    };
    let usable: Vec<_> = report
        .enforceable
        .iter()
        .copied()
        .filter(|mode| report.agent_modes.is_empty() || report.agent_modes.contains(mode))
        .collect();
    let mut acts = Vec::new();
    if report.enforceable.is_empty() {
        acts.push("Start or probe the environment so it reports its tools, or choose another environment.".to_owned());
    } else if let Some(target) = usable.iter().min().copied() {
        if report.requested < target {
            acts.push(format!(
                "Allow {} in this place's Sandbox setting.",
                sandbox_name(target)
            ));
        }
        if report.machine_limit < target {
            acts.push(format!(
                "Allow {} in this machine's policy.",
                sandbox_name(target)
            ));
        }
        if report.template_limit.is_some_and(|limit| limit < target) {
            acts.push(format!(
                "Allow {} in the environment template's widest sandbox.",
                sandbox_name(target)
            ));
        }
        acts.push(format!(
            "Or choose an environment and tools that can enforce {}.",
            sandbox_name(report.requested)
        ));
    } else {
        acts.push("Choose an agent and executor with a sandbox mode in common.".to_owned());
    }
    let reason = report.reason.as_deref().unwrap_or("");
    format!(
        "Sandbox: {name}: {capability}. This place asks for {}; the environment allows at most {} ({source}). {reason} {}",
        sandbox_name(report.requested),
        sandbox_name(report.widest),
        acts.join(" ")
    )
}

/// Distinguish this refusal from other binding problems before adding sandbox-specific recovery acts.
#[must_use]
pub fn is_sandbox_refusal(detail: &str) -> bool {
    detail.starts_with("Sandbox: ")
        || detail == SANDBOX_UNSUPPORTED
        || detail.starts_with("no sandbox mode at or below ")
}

/// Fixed product explanations, mirrored by the catalogues; never inferred from a vendor's words.
pub const APPROVAL_NOT_ALLOWED: &str = "This approval mode is not allowed here. Choose a mode permitted by approval.allowed_modes, or ask a manager to change that setting.";
/// The selected adapter cannot use the requested approval mode.
pub const APPROVAL_UNSUPPORTED: &str = "This agent does not support the selected approval.mode. Choose another approval mode or agent.";
/// No supported sandbox fits all the current ceilings.
pub const SANDBOX_UNSUPPORTED: &str = "This agent and environment cannot provide the requested tool access. Choose another agent or environment, or ask a manager to change sandbox.mode and the sandbox.max or agent.max_sandbox limits.";

/// How the harness is confined for a turn: the approval mode and the sandbox
/// mode, or what has to give.
pub fn confinement(
    settings: &Resolved,
    descriptor: &AgentDescriptor,
    environment: &EnvironmentRow,
    template: Option<&Json>,
    machine: &orch_db::queries::environment_facts::EnvironmentMachine,
    executor: Option<&str>,
) -> Result<(ApprovalMode, SandboxMode), String> {
    let mode = settings.approval_mode;
    if !settings.allowed_modes.contains(&mode) {
        return Err(APPROVAL_NOT_ALLOWED.into());
    }
    if !descriptor.approval_modes.contains(&mode) {
        return Err(APPROVAL_UNSUPPORTED.into());
    }
    let report = sandbox_readiness(
        settings.sandbox_mode,
        Some(&descriptor.sandbox_modes),
        &environment.facts,
        machine,
        template,
        executor,
    );
    // Pre-flight must let an unmade/detached environment start and probe its tools. This
    // provisional comparison does not start a turn: session admission repeats it with the
    // running environment's report. An explicit empty or unavailable report still refuses.
    let awaiting_probe = environment.status != "running"
        && executor.is_some_and(|kind| {
            !environment
                .facts
                .get("executors")
                .and_then(Json::as_array)
                .is_some_and(|items| {
                    items
                        .iter()
                        .any(|item| item.get("kind").and_then(Json::as_str) == Some(kind))
                })
        });
    let usable: Vec<_> = if awaiting_probe {
        descriptor.sandbox_modes.clone()
    } else {
        report
            .enforceable
            .iter()
            .copied()
            .filter(|mode| descriptor.sandbox_modes.contains(mode))
            .collect()
    };
    sandbox_for_turn(settings.sandbox_mode, report.widest, &usable)
        .map(|sandbox| (mode, sandbox))
        .ok_or_else(|| sandbox_explanation(&environment.name, &report))
}

/// What a turn is set up from.
#[derive(Debug, Clone)]
pub struct StartSnapshot {
    /// Current topic-document policy and first-reading material paths.
    pub material: crate::material::Material,
    /// The thread.
    pub thread: ThreadRow,
    /// Exact positions prepared for this turn, bounded by the notice delivery page.
    pub pending_notice_positions: Vec<i64>,
    /// The turn that is starting.
    pub turn: TurnRow,
    /// The input it answers.
    pub input: InputRow,
    /// The segment it is in.
    pub segment: SegmentRow,
    /// The provider.
    pub provider: ProviderRow,
    /// The agent profile.
    pub agent: AgentRow,
    /// The environment.
    pub environment: EnvironmentRow,
    /// What the profile's adapter declares.
    pub descriptor: AgentDescriptor,
    /// The settings of the thread's place.
    pub settings: Resolved,
    /// The harness process.
    pub key: HarnessKey,
    /// The machine it runs on.
    pub machine: MachinePlace,
    /// The model's entry on the provider.
    pub model: Option<ModelRow>,
    /// How the harness reaches the model.
    pub path: CredentialPath,
    /// The lease of the turn, where it has one.
    pub lease: Option<LeaseRow>,
    /// The arrangement as it is chosen now.
    pub arrangement: Arrangement,
    /// The approval mode.
    pub approval: ApprovalMode,
    /// Exact models this input's author may use; supplied to a native reviewer in Auto.
    pub available_models: Vec<String>,
    /// The sandbox mode, lowered to what can be enforced.
    pub sandbox: SandboxMode,
    /// The name of the topic.
    pub topic_name: String,
    /// The name of the thread.
    pub thread_name: String,
    /// The standing instructions of the thread's places, each with the kind and the name of its place.
    pub standing: Vec<instructions::Standing>,
    /// The other threads of the topic in the environment.
    pub neighbours: Vec<String>,
    /// Whether threads of other topics run in the environment.
    pub other_work: bool,
    /// The name of the author of the input.
    pub author_name: Option<String>,
    /// The number of distinct people whose inputs were delivered.
    pub authors: i64,
    /// The tools of the `orch` server the session is opened with.
    pub tools: Vec<orch_agent_contract::events::OrchTool>,
    /// What the environment has and can be given, where capabilities apply to it.
    pub capabilities: Option<instructions::CapabilityLine>,
    /// What it runs on and under, with the limits that touch what it has or can be given.
    pub isolation: Option<orch_driver_contract::isolation::RuntimeIsolation>,
    /// What was found to run a browser and where its profiles are kept.
    pub browser_line: String,
    /// What the people who manage the environment answered about a configuration of the harness's own in its
    /// workspace, and for which files; none while nobody was asked.
    pub workspace_configuration: Option<link::WorkspaceConfiguration>,
    /// The cut of the turn before this one that no input was told of yet (T8): that turn, its recorded cause, and
    /// the name of whoever stopped it.
    pub cut: Option<(Id, String, Option<String>)>,
    /// Who sent the words that named nobody, where the person's input delivered last did not name its sender: told
    /// once, when names begin.
    pub unnamed_before: Vec<String>,
    /// What the segment's agent was given of where it is (`segments.context_sent`), against which a change is told.
    pub context_sent: Json,
}

impl StartSnapshot {
    /// Whether the input's words go to the agent headed by its author's name: a person's, where the place attributes
    /// messages always, or by now that two people or more had their words delivered.
    #[must_use]
    pub fn names_its_author(&self) -> bool {
        self.author_name.is_some() && self.settings.attribution.applies(self.authors)
    }

    /// The directory of the thread in the environment.
    #[must_use]
    pub fn directory(&self) -> String {
        absolute_directory(&self.environment.workspace_root, &self.thread.workspace_dir)
    }

    /// Where the agent starts.
    #[must_use]
    pub fn cwd(&self) -> String {
        absolute_cwd(
            &self.environment.workspace_root,
            &self.thread.workspace_dir,
            &self.settings.cwd,
        )
    }

    /// What the model's entry says of it.
    pub fn model_parameters(&self) -> Result<ModelParameters, EngineError> {
        self.model
            .as_ref()
            .map(binding::model_parameters)
            .transpose()
            .map(Option::unwrap_or_default)
    }

    /// The parameters of the adapter that are shown and sent: those the turn was made with, which pre-flight
    /// resolved from the thread's choices and what the model's entry starts with, and recorded on the turn.
    /// What a session and a turn are started with is what the turn records, so that the two cannot differ.
    pub fn adapter_params(&self) -> Result<Json, EngineError> {
        Ok(match &self.turn.params {
            recorded @ Json::Object(_) => recorded.clone(),
            _ => Json::Object(serde_json::Map::new()),
        })
    }

    /// Who keeps the agent's notes.
    #[must_use]
    pub fn memory_plan(&self) -> MemoryPlan {
        MemoryPlan::decide(
            self.settings.memory,
            self.descriptor.capabilities.memory,
            self.thread.arrangement.as_deref() != Some("separated"),
        )
    }

    /// Resolve once through the same supplier used by SessionOpen and its accepted-value comparison.
    #[must_use]
    pub fn session_configuration(&self) -> harnesses::SessionConfiguration {
        harnesses::SessionConfiguration {
            cwd: self.cwd(),
            memory: self
                .memory_plan()
                .session_memory(&self.directory())
                .map(|memory| link::SessionOpenMemory {
                    directory: memory.directory,
                    kept_by: keeper(memory.kept_by),
                }),
        }
    }

    /// What the session's texts say of the thread's place.
    fn place<'a>(&'a self, directory: &'a str, cwd: &'a str) -> Place<'a> {
        Place {
            documents_policy: self.material.policy,
            read_first: &self.material.paths,
            directory,
            cwd,
            topic: &self.topic_name,
            thread: &self.thread_name,
            conversation_path: &self.material.conversation_path,
            neighbours: &self.neighbours,
            other_work: self.other_work,
            notes: self
                .memory_plan()
                .session_memory(directory)
                .map(|memory| memory.kept_by),
            capabilities: self.capabilities.as_ref(),
            isolation: self.isolation.as_ref(),
            tools: &self.tools,
            browsers: &self.browser_line,
            rates: instructions::ToolRates {
                calls_per_minute: self.settings.tool_calls_per_minute,
                sends_per_hour: self.settings.sends_per_hour,
            },
        }
    }

    /// What a session is opened with, from the preamble down.
    #[must_use]
    pub fn instructions(&self) -> instructions::Text {
        let directory = self.directory();
        let cwd = self.cwd();
        let place = self.place(&directory, &cwd);
        instructions::session_instructions(&place, &self.standing, &self.settings.messages)
    }

    /// What the session is opened with, source by source in the order the agent reads them, as the segment keeps it
    /// (`segments.instruction_sources`) and `GET /threads/{id}/instructions` shows it: Orch's preamble, the guide to
    /// its tools where it goes with the instructions, each place's standing instructions outermost first (with a hash
    /// of their words, by which a later change is known), then the files the harness reads as its adapter reported
    /// them, in the environment the thread works in.
    #[must_use]
    pub fn instruction_sources(
        &self,
        reported: &[orch_agent_contract::session::InstructionFile],
    ) -> Json {
        use orch_settings_contract::provenance::SourceScope;
        let measure = |text: &str| {
            let lines = text.lines().count();
            (text.len(), lines)
        };
        let mut sources: Vec<Json> = Vec::new();
        for block in self.instruction_blocks() {
            let (kind, label) = match block.key.as_str() {
                instructions::PREAMBLE_KEY => ("preamble", "Orch's introduction"),
                instructions::TOOLS_GUIDE_KEY => ("guide", "The guide to Orch's tools"),
                // The standing instructions are given place by place below.
                _ => continue,
            };
            let (bytes, lines) = measure(&block.text);
            sources.push(json!({
                "kind": kind, "label": label, "scope": null, "environment": null, "path": null, "home": false,
                "evidence": "orch", "bytes": bytes, "loaded_bytes": bytes, "cut_bytes": 0, "lines": lines,
                "loaded_lines": lines, "reason": null,
            }));
        }
        for part in &self.settings.standing {
            let label = match part.scope {
                SourceScope::Org => "Standing instructions of the org",
                SourceScope::Project => "Standing instructions of the project",
                SourceScope::Topic => "Standing instructions of the topic",
                SourceScope::Thread => "Standing instructions of this thread",
                _ => continue,
            };
            let (bytes, lines) = measure(&part.text);
            sources.push(json!({
                "kind": "standing", "label": label, "scope": part.node, "environment": null, "path": null,
                "home": false, "evidence": "orch", "bytes": bytes, "loaded_bytes": bytes, "cut_bytes": 0,
                "lines": lines, "loaded_lines": lines, "reason": null,
                "hash": orch_types::Hash::of(part.text.as_bytes()).to_string(),
            }));
        }
        for file in reported {
            let source = crate::instruction_sources::file_json(
                file,
                self.environment.id,
                &self.environment.workspace_root,
            )
            .unwrap_or(Json::Null);
            sources.push(source);
        }
        Json::Array(sources)
    }

    /// What a session is opened with, block by block, as the record of what the model was given keeps it.
    #[must_use]
    pub fn instruction_blocks(&self) -> Vec<crate::model_inputs::ModelBlock> {
        let directory = self.directory();
        let cwd = self.cwd();
        let place = self.place(&directory, &cwd);
        instructions::session_blocks(&place, &self.standing, &self.settings.messages)
            .into_iter()
            .map(|(key, text)| {
                let source = if key == instructions::STANDING_HEADING_KEY {
                    "standing"
                } else {
                    "orch"
                };
                crate::model_inputs::ModelBlock::new(key, source, text)
            })
            .collect()
    }

    /// The actual Orch tool catalogue this session is given, including the place's descriptions and schemas.
    #[must_use]
    pub fn tool_blocks(&self) -> Vec<crate::model_inputs::ModelBlock> {
        self.tools
            .iter()
            .map(|tool| {
                let definition = crate::tools::definitions::definition_at(
                    *tool,
                    &self.settings.messages,
                    &self.settings.broadcast_agent_scope,
                    &self.settings.broadcast_agent_options,
                );
                crate::model_inputs::ModelBlock::new(
                    tool.name(),
                    "tool",
                    definition.to_mcp().to_string(),
                )
            })
            .collect()
    }

    /// A stable fingerprint includes offered tools, their text and the current policy-narrowed schemas.
    pub fn tool_catalogue(&self) -> Json {
        Json::Array(
            self.tool_blocks()
                .into_iter()
                .map(|block| Json::String(block.text))
                .collect(),
        )
    }

    /// The own-tool revision is independent of external servers refreshed in an open session.
    #[must_use]
    pub fn orch_tools_revision(&self) -> crate::harnesses::OrchToolsRevision {
        crate::harnesses::OrchToolsRevision::of(&self.tool_catalogue())
    }

    /// Schemas travel to the connector beside the tool list and are exactly those recorded above.
    pub fn tool_schemas(&self) -> std::collections::BTreeMap<String, Json> {
        self.tools
            .iter()
            .filter(|tool| **tool == orch_agent_contract::events::OrchTool::Announce)
            .map(|tool| {
                let definition = crate::tools::definitions::definition_at(
                    *tool,
                    &self.settings.messages,
                    &self.settings.broadcast_agent_scope,
                    &self.settings.broadcast_agent_options,
                );
                (tool.name().to_owned(), definition.input_schema)
            })
            .collect()
    }

    /// Where the agent is, line by line, as it stands now.
    #[must_use]
    pub fn context_lines(&self) -> Vec<(&'static str, String)> {
        let directory = self.directory();
        let cwd = self.cwd();
        let place = self.place(&directory, &cwd);
        instructions::context_lines(&place)
    }
}

/// The standing instructions of a thread's places with the kind and the name of each, as T1b heads them.
pub async fn standing_of(
    conn: &mut Conn,
    parts: &[crate::resolved::StandingPart],
) -> Result<Vec<instructions::Standing>, EngineError> {
    use orch_settings_contract::provenance::SourceScope;
    let mut standing = Vec::with_capacity(parts.len());
    for part in parts {
        let kind = match part.scope {
            SourceScope::Org => Some("org"),
            SourceScope::Project => Some("project"),
            SourceScope::Topic => Some("topic"),
            SourceScope::Thread => None,
            // Standing instructions are set at an org, a project, a topic or a thread, and nowhere else.
            SourceScope::Default
            | SourceScope::Environment
            | SourceScope::System
            | SourceScope::EnvironmentOwn
            | SourceScope::ProviderOwn
            | SourceScope::HostOwn => continue,
        };
        let name = match (kind, part.node) {
            (Some(_), Some(node)) => threads::node_name(conn, node).await?.unwrap_or_default(),
            _ => String::new(),
        };
        standing.push(instructions::Standing {
            kind,
            name,
            text: part.text.clone(),
        });
    }
    Ok(standing)
}

/// The most characters the standing instructions of a thread may come to with their headings: about the vendor's
/// ceiling of ten thousand estimated tokens, counted as characters of Orch's configuration, not as tokens.
pub const STANDING_LIMIT: usize = 40_000;

/// How many characters the standing instructions of a place come to with their headings, as a thread there is told
/// them (T1b).
pub async fn standing_characters(conn: &mut Conn, place: Id) -> Result<usize, EngineError> {
    let parts = crate::resolved::standing_parts(conn, place).await?;
    let standing = standing_of(conn, &parts).await?;
    Ok(instructions::standing_block(&standing).map_or(0, |block| block.chars().count()))
}

/// Whether a change at `node` leaves some thread there, or one made there, with standing instructions longer than
/// [`STANDING_LIMIT`]: the most characters found where it does, and none where it does not. Which place it is is not
/// said, since the one who changed it may not see it.
pub async fn standing_past_limit(conn: &mut Conn, node: Id) -> Result<Option<usize>, EngineError> {
    let mut most: Option<usize> = None;
    for place in threads::standing_places_under(conn, node).await? {
        let characters = standing_characters(conn, place).await?;
        if characters > STANDING_LIMIT && most.is_none_or(|before| characters > before) {
            most = Some(characters);
        }
    }
    Ok(most)
}

/// Whether the notes are kept by the harness, as a session says it.
fn keeper(kept_by: NoteKeeper) -> link::MemoryKeeper {
    match kept_by {
        NoteKeeper::Harness => link::MemoryKeeper::Harness,
        NoteKeeper::Agent => link::MemoryKeeper::Agent,
    }
}

/// A request for a ticket in a command that is stored: a fresh one is signed
/// each time the command is put on the wire.
fn ticket(
    kind: TunnelKind,
    service: &Service,
    dst: Id,
    thread: Id,
    tool_set: Option<&str>,
) -> link::TicketSlot {
    let mut more = serde_json::Map::new();
    more.insert("service".to_owned(), Json::String(service.to_string()));
    more.insert("dst".to_owned(), Json::String(dst.to_string()));
    more.insert("thread_id".to_owned(), Json::String(thread.to_string()));
    if let Some(set) = tool_set {
        more.insert("tool_set".to_owned(), Json::String(set.to_owned()));
    }
    link::TicketSlot::Asked(link::TicketAsk {
        ticket: link::TicketWanted { kind, more },
    })
}

/// What a session that begins with a history is given of it: the part of the fork and switch that carries a
/// conversation to another agent.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionImport {
    /// The harness's name for the conversation to fork from; the segment's own when none.
    pub native_session_ref: Option<String>,
    /// The native history to import.
    pub archive: Option<link::SessionOpenArchive>,
    /// The neutral history to import.
    pub history: Option<link::SessionOpenHistory>,
    /// For a fork and for an import: only through this turn.
    pub upto_turn_ordinal: Option<u32>,
}

/// The command that opens the session of a segment.
pub fn session_open_body(
    snapshot: &StartSnapshot,
    mode: link::SessionMode,
) -> Result<link::SessionOpen, EngineError> {
    session_open_body_importing(snapshot, mode, &SessionImport::default())
}

/// The command that opens the session of a segment, with the history it begins with.
pub fn session_open_body_importing(
    snapshot: &StartSnapshot,
    mode: link::SessionMode,
    import: &SessionImport,
) -> Result<link::SessionOpen, EngineError> {
    let thread = snapshot.thread.node_id;
    let configuration = snapshot.session_configuration();
    let provider = snapshot.provider.id;
    let provider_side = match snapshot.path {
        CredentialPath::Native => link::SessionOpenProvider::Native {
            provider_id: provider,
        },
        CredentialPath::Forwarded => {
            let lease = snapshot
                .lease
                .as_ref()
                .ok_or_else(|| EngineError::unreadable("a forwarded turn has no lease"))?;
            let dst = crate::leases::machine_of(&snapshot.provider).ok_or_else(|| {
                EngineError::unreadable("the provider has no machine to forward through")
            })?;
            link::SessionOpenProvider::Forwarded {
                provider_id: provider,
                protocol: snapshot.arrangement.protocol.clone().unwrap_or_default(),
                ticket: ticket(
                    TunnelKind::Llm,
                    &Service::Llm { lease_id: lease.id },
                    dst,
                    thread,
                    None,
                ),
            }
        }
        CredentialPath::Shared | CredentialPath::Handover => {
            let form = snapshot
                .lease
                .as_ref()
                .and_then(|lease| lease.form.clone())
                .or_else(|| {
                    snapshot
                        .arrangement
                        .form
                        .as_ref()
                        .map(|form| form.form.clone())
                })
                .ok_or_else(|| {
                    EngineError::unreadable("a turn that is given a credential names no form of it")
                })?;
            if snapshot.path == CredentialPath::Shared {
                link::SessionOpenProvider::Shared {
                    provider_id: provider,
                    form,
                }
            } else {
                link::SessionOpenProvider::Handover {
                    provider_id: provider,
                    form,
                }
            }
        }
    };
    let environment_machine = snapshot.environment.machine_id;
    let executor = snapshot.thread.executor.clone();
    let tool_set = snapshot.thread.tool_set.clone();
    let exec_ticket = match (executor.as_deref(), environment_machine) {
        (None, _) => None,
        (Some(_), None) => {
            return Err(EngineError::unreadable(
                "the environment has no machine to carry out tool calls",
            ));
        }
        (Some("orch-exec"), Some(machine)) => Some(ticket(
            TunnelKind::Mcp,
            &Service::Mcp {
                server: "exec".to_owned(),
            },
            machine,
            thread,
            tool_set.as_deref(),
        )),
        (Some(other), Some(machine)) => Some(ticket(
            TunnelKind::Exec,
            &Service::Exec {
                executor: other.to_owned(),
            },
            machine,
            thread,
            None,
        )),
    };
    let parameters = snapshot.adapter_params()?;
    let params = match parameters {
        Json::Object(members) => members,
        _ => serde_json::Map::new(),
    };
    Ok(link::SessionOpen {
        agent_id: snapshot.agent.id,
        thread_id: thread,
        segment_id: snapshot.segment.id,
        mode,
        native_session_ref: import.native_session_ref.clone().or_else(|| {
            if matches!(mode, link::SessionMode::New) {
                None
            } else {
                snapshot.segment.native_session_ref.clone()
            }
        }),
        archive: import.archive.clone(),
        history: import.history,
        upto_turn_ordinal: import.upto_turn_ordinal,
        workspace_root: snapshot.environment.workspace_root.clone(),
        instruction_root_marker: false,
        directory: snapshot.directory(),
        cwd: configuration.cwd,
        mounts: Vec::new(),
        model: snapshot.turn.model.clone(),
        effort: snapshot.turn.effort.clone(),
        params,
        approval_mode: convert(&snapshot.approval)?,
        available_models: snapshot.available_models.clone(),
        subagent_limits: snapshot.settings.subagent_limits,
        sandbox_mode: convert(&snapshot.sandbox)?,
        instructions: snapshot.instructions().text,
        memory: configuration.memory,
        arrangement: convert(&snapshot.arrangement.kind)?,
        provider: provider_side,
        exec: link::SessionOpenExec {
            executor,
            tool_set,
            environment_id: snapshot.environment.id,
            generation: i64::from(snapshot.environment.generation),
            ticket: exec_ticket,
        },
        mcp: Vec::new(),
        skills: Vec::new(),
        orch_tools: convert(&snapshot.tools)?,
        // Diagnostics are kept for as long as `diagnostics.retention_days` says (`raw_events`).
        raw_events: true,
        raw_after: 0,
        // A side thread reads by the mechanism chosen when it was made; the adapter enforces it at every open
        // and resume, or refuses the session.
        side_reading: snapshot
            .thread
            .side_reading
            .as_deref()
            .map(|word| convert(&Json::String(word.to_owned())))
            .transpose()?,
        // The environment's answer about the workspace's own harness configuration: without one, an adapter that
        // finds such a configuration refuses, and the people who manage the environment are asked.
        workspace_configuration: snapshot.workspace_configuration.clone(),
        // The descriptions the place set for the tools it offers, where they differ from the shipped ones: the
        // harness's own list of the tools says them.
        orch_tool_descriptions: snapshot
            .tools
            .iter()
            .filter_map(|tool| {
                let described =
                    crate::tools::definitions::definition(*tool, &snapshot.settings.messages)
                        .description;
                (described != crate::tools::definitions::shipped_description(*tool))
                    .then(|| (tool.name().to_owned(), described))
            })
            .collect(),
        orch_tool_schemas: snapshot.tool_schemas(),
    })
}

/// What a segment is made to run on.
#[derive(Debug, Clone, Copy)]
pub struct SegmentPlace<'a> {
    /// The thread.
    pub thread: &'a ThreadRow,
    /// The agent profile.
    pub agent: &'a AgentRow,
    /// The provider.
    pub provider: &'a ProviderRow,
    /// The environment.
    pub environment: &'a EnvironmentRow,
    /// The way the provider's credential reaches the harness.
    pub path: CredentialPath,
    /// The person whose input the segment is made for.
    pub started_by: Option<Id>,
}

/// Makes sure the thread has a segment fit for the turn that is about to
/// start: its current one if it lies on what the thread runs on now, and
/// otherwise the next, the old one ended for the reason that tells why.
pub async fn ensure_segment(
    unit: &mut crate::unit::Unit<'_>,
    place: &SegmentPlace<'_>,
    seq: Seq,
) -> Result<SegmentRow, EngineError> {
    let SegmentPlace {
        thread,
        agent,
        provider,
        environment,
        path,
        started_by,
    } = *place;
    let now = unit.now();
    let current = match thread.current_segment_id {
        Some(id) => threads::segment(&mut *unit.conn, id).await?,
        None => None,
    };
    let arrangement = thread
        .arrangement
        .clone()
        .ok_or_else(|| EngineError::unreadable("a bound thread has an arrangement"))?;
    let mut carried: Option<String> = None;
    if let Some(segment) = current {
        let same_binding = segment.agent_id == agent.id
            && segment.provider_id == provider.id
            && segment.environment_id == environment.id;
        let same_place = segment.ended_at.is_none() && same_binding;
        if same_place && segment.env_generation == environment.generation {
            return Ok(segment);
        }
        if segment.ended_at.is_none() {
            let reason = if same_place {
                "environment_recreated"
            } else {
                "switched"
            };
            threads::end_segment(&mut *unit.conn, segment.id, reason, now, seq).await?;
        }
        // A unit made anew keeps the conversation: the harness resumes it with
        // the new parameters of the tools. A thread put on something else does
        // not, until a switch carries the history over.
        if same_place || (same_binding && segment.end_reason.as_deref() == Some("archived")) {
            carried = segment.native_session_ref.clone();
        }
    }
    let row = threads::insert_segment(
        &mut *unit.conn,
        &NewSegment {
            id: new_id(),
            thread_id: thread.node_id,
            agent_id: agent.id,
            provider_id: provider.id,
            environment_id: environment.id,
            env_generation: environment.generation,
            arrangement: &arrangement,
            executor: thread.executor.as_deref(),
            tool_set: thread.tool_set.as_deref(),
            credential_path: binding::credential_path_word(path),
            native_session_ref: carried.as_deref(),
            started_by,
            started_at: now,
        },
        seq,
    )
    .await?;
    threads::set_current(
        &mut *unit.conn,
        thread.node_id,
        Some(row.id),
        thread.current_turn_id,
        seq,
    )
    .await?;
    Ok(row)
}

/// What a look at a thread that may have a turn to start found.
pub(crate) enum StartLoad {
    /// No turn is starting.
    NoTurn,
    /// A turn is starting that cannot be set up from what the database holds
    /// now.
    Unstartable(Id),
    /// A turn is starting, and this is what it is set up from.
    Ready(Box<StartSnapshot>),
}

impl Engine {
    /// Reads what a turn that is starting is set up from: the snapshot, or
    /// that no turn is starting, or that the one that is cannot be set up
    /// from what the database holds now (something it needs is gone).
    pub(crate) async fn load_start_snapshot(
        &self,
        conn: &mut Conn,
        thread_id: Id,
    ) -> Result<StartLoad, EngineError> {
        self.load_turn_snapshot(conn, thread_id, threads::turn_state::STARTING)
            .await
    }

    /// As [`Engine::load_start_snapshot`], for the thread's active turn in `state`: a running turn's, for what a
    /// steer into it is told of where the agent is.
    pub(crate) async fn load_turn_snapshot(
        &self,
        conn: &mut Conn,
        thread_id: Id,
        state: &str,
    ) -> Result<StartLoad, EngineError> {
        let Some(thread) = threads::thread(conn, thread_id).await? else {
            return Ok(StartLoad::NoTurn);
        };
        let Some(turn) = threads::active_turn(conn, thread_id).await? else {
            return Ok(StartLoad::NoTurn);
        };
        if turn.state != state {
            return Ok(StartLoad::NoTurn);
        }
        self.snapshot_for_turn(conn, thread, turn).await
    }

    /// Prepares configuration for an idle continuation without minting a turn or a cap reservation.
    pub(crate) async fn continuation_snapshot(
        &self,
        conn: &mut Conn,
        thread_id: Id,
        turn_id: Id,
        segment: threads::SegmentRow,
    ) -> Result<StartSnapshot, EngineError> {
        let thread = threads::thread(conn, thread_id)
            .await?
            .ok_or_else(|| EngineError::unreadable("The thread is gone."))?;
        let turn = threads::turn(conn, turn_id)
            .await?
            .filter(|turn| turn.thread_id == thread_id)
            .ok_or_else(|| EngineError::unreadable("The retained native turn is gone."))?;
        let StartLoad::Ready(mut snapshot) = self.snapshot_for_turn(conn, thread, turn).await?
        else {
            return Err(EngineError::unreadable(
                "The current binding cannot prepare the retained native conversation.",
            ));
        };
        if snapshot.agent.id != segment.agent_id
            || snapshot.provider.id != segment.provider_id
            || snapshot.environment.id != segment.environment_id
            || snapshot.environment.generation != segment.env_generation
        {
            return Err(EngineError::unreadable(
                "The retained conversation belongs to another binding or environment generation.",
            ));
        }
        snapshot.turn.model = snapshot
            .settings
            .model
            .clone()
            .ok_or_else(|| EngineError::unreadable("Choose a model for this thread."))?;
        snapshot.turn.effort = snapshot.settings.effort.clone();
        snapshot.turn.segment_id = segment.id;
        snapshot.segment = segment;
        Ok(*snapshot)
    }

    async fn snapshot_for_turn(
        &self,
        conn: &mut Conn,
        thread: ThreadRow,
        turn: TurnRow,
    ) -> Result<StartLoad, EngineError> {
        let thread_id = thread.node_id;
        let (Some(agent_id), Some(provider_id), Some(environment_id)) =
            (thread.agent_id, thread.provider_id, thread.environment_id)
        else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let (Some(input), Some(segment), Some(agent), Some(provider), Some(environment)) = (
            threads::input(conn, turn.input_id).await?,
            threads::segment(conn, turn.segment_id).await?,
            resources::agent(conn, agent_id).await?,
            resources::provider(conn, provider_id).await?,
            environments::environment(conn, environment_id).await?,
        ) else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let Some(descriptor) = harnesses::descriptor_of(&agent)? else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let settings = crate::resolved::load(conn, thread_id).await?;
        let Some(machine_id) = harnesses::machine_of(
            thread.arrangement.as_deref(),
            &provider,
            environment.machine_id,
        ) else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let Some(machine) = resources::machine_place(conn, machine_id).await? else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let offers = binding::offers_of(conn, environment.id).await?;
        let colocated = binding::colocated_of(conn, &environment).await?;
        let facts = Facts {
            descriptor: &descriptor,
            provider: &provider,
            environment: &environment,
            offers,
            settings: &settings,
            entry: None,
            colocated,
            runtime: binding::runtime_facts(conn, &agent, &provider, &environment).await?,
        };
        let requested: Option<ArrangementType> = thread
            .arrangement
            .as_deref()
            .and_then(|word| serde_json::from_value(Json::String(word.to_owned())).ok());
        let Ok(arrangement) = binding::choose(&facts, requested) else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        let template = environments::template_spec(conn, environment.id).await?;
        let environment_machine = orch_db::queries::environment_facts::of(conn, environment.id)
            .await?
            .ok_or_else(|| EngineError::unreadable("the environment is gone"))?;
        let Ok((approval, sandbox)) = confinement(
            &settings,
            &descriptor,
            &environment,
            template.as_ref(),
            &environment_machine,
            arrangement.executor.as_deref(),
        ) else {
            return Ok(StartLoad::Unstartable(turn.id));
        };
        if self
            .bypass_environment(conn, environment.id, approval)
            .await?
            .is_some()
        {
            return Ok(StartLoad::Unstartable(turn.id));
        }
        let model = resources::model(conn, provider.id, &turn.model).await?;
        let available_models = if approval == ApprovalMode::Auto {
            self.review_models(conn, &thread, provider.id, input.author_id, &settings)
                .await?
        } else {
            vec![turn.model.clone()]
        };
        if crate::approval_review::reviewer_allowed(
            approval,
            harnesses::descriptor_of(&agent)?.as_ref(),
            &available_models,
        )
        .is_err()
        {
            return Ok(StartLoad::Unstartable(turn.id));
        }
        let path = binding::selected_path(&arrangement, &provider, &agent);
        let mut lease = match turn.lease_id {
            Some(id) => threads::lease(conn, id).await?,
            None => None,
        };
        if approval == ApprovalMode::Auto
            && let Some(lease) = &mut lease
        {
            // The forwarder bounds both session and reviewer requests by this same current policy.
            sqlx::query("UPDATE leases SET allowed_models=$2 WHERE id=$1 AND closed_at IS NULL")
                .bind(lease.id)
                .bind(&available_models)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            lease.allowed_models.clone_from(&available_models);
        }
        let topic_name = threads::node_name(conn, thread.topic_id)
            .await?
            .unwrap_or_default();
        let thread_name = threads::node_name(conn, thread_id)
            .await?
            .unwrap_or_default();
        let standing = standing_of(conn, &settings.standing).await?;
        let neighbours =
            threads::topic_threads_in_environment(conn, thread.topic_id, environment.id, thread_id)
                .await?;
        let other_work =
            threads::other_topic_threads_in_environment(conn, thread.topic_id, environment.id)
                .await?
                > 0;
        // A goal's input is Orch's own words in the name of whoever set the goal: it is not headed by their name.
        let author_name = match input.author_id.filter(|_| input.source != "goal") {
            Some(author) => threads::people(conn, &[author])
                .await?
                .into_iter()
                .next()
                .map(|person| {
                    orch_chat::written::person_name(&person.display_name, &person.username)
                }),
            None => None,
        };
        let authors = threads::count_delivering_authors(conn, thread_id).await?;
        let key = HarnessKey {
            provider: provider.id,
            agent: agent.id,
            machine: machine_id,
        };
        let tools = crate::tools::offered_for_session(
            conn,
            &thread,
            descriptor.capabilities.orch_tools == OrchToolsSupport::Mcp,
            self.inner.clock.time(),
        )
        .await?;
        let view = self.environment_capabilities(conn, environment.id).await?;
        let isolation = view
            .as_ref()
            .and_then(|view| view.isolation.clone())
            .filter(|isolation| isolation.runtime.is_some());
        let capabilities = match view {
            Some(view) if view.applies => Some(instructions::CapabilityLine {
                held: view.held.iter().map(|held| held.key.clone()).collect(),
                offered: tools
                    .contains(&orch_agent_contract::events::OrchTool::RequestCapability)
                    .then(|| {
                        view.offered
                            .iter()
                            .map(|offered| offered.key.clone())
                            .collect()
                    }),
            }),
            _ => None,
        };
        let browser_line = crate::tools::browser::preamble(conn, environment.id).await?;
        let workspace_configuration =
            crate::trust::answer_for(conn, environment.id, &agent.harness_kind).await?;
        let cut = threads::untold_cut(conn, thread_id, turn.ordinal).await?;
        // Only words that go to the agent headed by their author's name can be the first to name anybody.
        let unnamed_before = if author_name.is_some() && settings.attribution.applies(authors) {
            threads::unnamed_authors_before(conn, thread_id, input.id).await?
        } else {
            Vec::new()
        };
        let context_sent = threads::context_sent(conn, segment.id).await?;
        let material = self.topic_material(conn, thread_id).await?;
        Ok(StartLoad::Ready(Box::new(StartSnapshot {
            material,
            thread,
            pending_notice_positions: Vec::new(),
            turn,
            input,
            segment,
            provider,
            agent,
            environment,
            descriptor,
            settings,
            key,
            machine,
            model,
            path,
            lease,
            arrangement,
            approval,
            available_models,
            sandbox,
            topic_name,
            thread_name,
            standing,
            neighbours,
            other_work,
            author_name,
            authors,
            tools,
            capabilities,
            isolation,
            browser_line,
            workspace_configuration,
            cut,
            unnamed_before,
            context_sent,
        })))
    }
}

#[cfg(test)]
#[path = "sessions/byte_stability_tests.rs"]
mod byte_stability_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_actual_unconfined_template_without_a_ceiling_defaults_to_workspace() {
        let host = orch_db::queries::environment_facts::EnvironmentMachine {
            machine_id: Some(new_id()),
            descriptor: Some(json!({"confinement":"none","lifecycle":"unit"})),
            policy: Some(json!({"agent.max_sandbox":"open"})),
        };
        assert_eq!(
            widest_sandbox(&host, Some(&json!({}))),
            SandboxMode::Workspace
        );
        let mut directory = host.clone();
        directory.descriptor = Some(json!({"confinement":"none","lifecycle":"attach"}));
        assert_eq!(
            widest_sandbox(&directory, None),
            SandboxMode::Open,
            "an absent template imposes no ceiling"
        );
        assert_eq!(
            widest_sandbox(&host, None),
            widest_sandbox(&directory, None)
        );
        directory.policy = None;
        assert_eq!(
            widest_sandbox(&directory, None),
            SandboxMode::Workspace,
            "only an unset machine ceiling uses the cautious default"
        );
        assert_eq!(
            widest_sandbox(&host, Some(&json!({"sandbox":{"max":"read_only"}}))),
            SandboxMode::ReadOnly
        );
        let mut isolated = host;
        isolated.descriptor = Some(json!({"confinement":"isolated"}));
        assert_eq!(widest_sandbox(&isolated, None), SandboxMode::Open);
        isolated.policy = Some(json!({"agent.max_sandbox":"workspace"}));
        assert_eq!(
            widest_sandbox(&isolated, Some(&json!({"sandbox":{"max":"open"}}))),
            SandboxMode::Workspace
        );
    }

    #[test]
    fn an_open_only_executor_runs_without_a_template_when_the_machine_and_place_allow_full_access()
    {
        use orch_api_contract::environment::SandboxLimitSource as Source;
        let mut machine = orch_db::queries::environment_facts::EnvironmentMachine {
            machine_id: Some(new_id()),
            descriptor: Some(json!({"confinement":"none","lifecycle":"attach"})),
            policy: Some(json!({"agent.max_sandbox":"open"})),
        };
        let facts =
            json!({"executors":[{"kind":"orch-exec","available":true,"sandbox_modes":["open"]}]});
        let modes = [
            SandboxMode::ReadOnly,
            SandboxMode::Workspace,
            SandboxMode::Open,
        ];
        let ready = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &facts,
            &machine,
            None,
            Some("orch-exec"),
        );
        assert!(ready.ready);
        assert_eq!(ready.widest, SandboxMode::Open);
        assert_eq!(ready.source, Source::MachinePolicy);
        assert!(ready.template_limit.is_none());
        machine.policy = Some(json!({"agent.max_sandbox":"workspace"}));
        let denied = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &facts,
            &machine,
            None,
            Some("orch-exec"),
        );
        let words = sandbox_explanation("world-box", &denied);
        assert!(!denied.ready);
        assert!(
            words.contains("world-box")
                && words.contains("cannot confine commands")
                && words.contains("Workspace (machine policy)")
        );
        assert!(words.contains("machine's policy"));
        machine.policy = Some(json!({"agent.max_sandbox":"open"}));
        let template = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &facts,
            &machine,
            Some(&json!({"sandbox":{"max":"read_only"}})),
            Some("orch-exec"),
        );
        assert!(!template.ready);
        assert_eq!(template.source, Source::Template);
        assert!(sandbox_explanation("world-box", &template).contains("environment template"));
        let cautious = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &facts,
            &machine,
            Some(&json!({})),
            Some("orch-exec"),
        );
        assert!(!cautious.ready);
        assert_eq!(cautious.source, Source::Default);
        let requested = sandbox_readiness(
            SandboxMode::Workspace,
            Some(&modes),
            &facts,
            &machine,
            None,
            Some("orch-exec"),
        );
        assert!(!requested.ready);
        assert!(sandbox_explanation("world-box", &requested).contains("place's Sandbox setting"));
        let unknown = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &json!({}),
            &machine,
            None,
            Some("orch-exec"),
        );
        assert!(!unknown.ready);
        assert!(unknown.enforceable.is_empty());
        assert!(sandbox_explanation("world-box", &unknown).contains("probe the environment"));
        machine.policy = None;
        let default = sandbox_readiness(
            SandboxMode::Open,
            Some(&modes),
            &facts,
            &machine,
            None,
            Some("orch-exec"),
        );
        assert!(!default.ready);
        assert_eq!(default.source, Source::Default);
        assert_eq!(default.widest, SandboxMode::Workspace);
        assert!(default.template_limit.is_none());
    }

    #[test]
    fn a_threads_directory_and_the_place_its_agent_starts_are_absolute() {
        assert_eq!(absolute_directory("/workspace", ""), "/workspace");
        assert_eq!(absolute_directory("/workspace/", "web"), "/workspace/web");
        assert_eq!(absolute_cwd("/workspace", "web", ""), "/workspace/web");
        assert_eq!(
            absolute_cwd("/workspace", "web", "api/v1"),
            "/workspace/web/api/v1"
        );
        assert_eq!(absolute_cwd("/workspace", "", "api"), "/workspace/api");
    }

    #[test]
    fn a_contract_enum_is_written_as_the_other_contract_has_it() {
        let link: link::ApprovalMode = convert(&ApprovalMode::OnRequest).unwrap();
        assert_eq!(link, link::ApprovalMode::OnRequest);
        let arrangement: link::ArrangementType = convert(&ArrangementType::KeySeparated).unwrap();
        assert_eq!(arrangement, link::ArrangementType::KeySeparated);
    }
}
