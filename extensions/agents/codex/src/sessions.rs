//! Per-session configuration, remote environment binding, the project root that puts `AGENTS.md` of the thread's directory on the harness's path, the servers of the home's own configuration switched off, its memories kept off, and a fork for a side thread.
//!
//! A session is a thread of the harness, and everything that belongs to it is stated when it is opened and
//! again whenever it is opened anew, because the harness keeps nothing of it that Orch relies on. The home's
//! `config.toml` holds nothing that belongs to a session; the session's own configuration is passed as the
//! `config` of `thread/start`, `thread/resume` and `thread/fork`:
//!
//! * the **model provider** that reaches the connector's forwarder under the session's key
//!   (`orch_<session key>`, the Responses format, the token found in the variable `ORCH_LLM_TOKEN`), or none
//!   where the harness uses its own sign-in;
//! * the **MCP servers** it is given, each at `/mcp/<session key>/<name>` with the token found in
//!   `ORCH_MCP_TOKEN`, and the servers of the home's own configuration switched off, so that none is used
//!   that nobody imported;
//! * instruction files use native discovery until a verified workspace root can be supplied;
//! * the **review model**, which is the session's model, so that the harness's own review steps stay on it;
//! * the **trust** of each directory from the workspace root to where the session works, where the workspace
//!   holds a configuration of Codex's own (a `.codex` with anything in it but empty directories): `trusted`
//!   when the people who manage the environment answered Trust, `untrusted` when they answered Keep it out.
//!   This Codex build reads a project's configuration at `thread/start` whatever the trust of its home says,
//!   and an untrusted project gives it neither its configuration nor its instruction files, so nothing short
//!   of the answer decides: a workspace that holds one and has no answer for the files it holds now is refused
//!   ([`AdapterError::workspace_configuration`]), and the system asks. A `.codex` the harness made for
//!   itself and left empty is not a configuration, and nobody is asked about it.
//!
//! Where the tools are carried out in an environment of their own, the session is also given an
//! **environment** of the harness's naming: the connection to the environment's exec server at
//! `/exec/<session key>`, named for the environment, its generation and the session, and used for every turn.

use std::collections::BTreeMap;

use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{
    ConfigTrust, ProviderAccess, SessionMode, SessionSpec, WorkspaceConfiguration,
};
use serde_json::{Map, json};

use orch_agent_contract::adapter::AdapterError;
use orch_types::Json;

use crate::descriptor::{EXECUTOR, FORWARDED_PROTOCOL};
use crate::parameters::{self, TurnSettings};
use crate::process::{LLM_TOKEN_VARIABLE, MCP_TOKEN_VARIABLE};

/// The prefix of the name of the model provider a session is given.
pub const PROVIDER_PREFIX: &str = "orch_";

/// The name of a harness's own provider, the vendor's, when the configuration names none.
pub const VENDOR_PROVIDER: &str = "openai";

/// The workspace-only marker, offered only after its configuration boundary has been proved.
pub const PROJECT_ROOT_MARKER: &str = orch_agent_contract::session::INSTRUCTION_ROOT_MARKER;

/// The directories from the workspace root to where the session works, both ends included.
pub fn instruction_chain(root: &str, cwd: &str) -> Result<Vec<std::path::PathBuf>, AdapterError> {
    use std::path::Path;
    let root = Path::new(root);
    let cwd = Path::new(cwd);
    if !root.is_absolute() || !cwd.is_absolute() {
        return Err(AdapterError::Unsupported(
            "the instruction chain needs absolute workspace and working directories".to_owned(),
        ));
    }
    let relative = cwd.strip_prefix(root).map_err(|_| {
        AdapterError::Unsupported(
            "the working directory is outside the supplied workspace root".to_owned(),
        )
    })?;
    let mut directory = root.to_path_buf();
    let mut chain = vec![directory.clone()];
    for part in relative.components() {
        if !matches!(part, std::path::Component::Normal(_)) {
            return Err(AdapterError::Unsupported(
                "the instruction chain needs an absolute, normalized directory".to_owned(),
            ));
        }
        directory.push(part);
        chain.push(directory.clone());
    }
    Ok(chain)
}

/// What this Codex build reads of a project's instruction files together, at most, where its configuration sets no
/// other bound (`project_doc_max_bytes`, 32 KiB): what lies beyond is cut, and the sizes the panel shows of it are
/// derived from this bound.
pub const PROJECT_DOC_BUDGET: u64 = 32 * 1024;

/// The instruction files a session reads, as the harness reported them in the answer to `thread/start`,
/// `thread/resume` or `thread/fork` (`instructionSources`), in its order. A file of the harness's home is named, not
/// located and not read. The project's are measured here where the session works on this machine, each with what
/// the project budget leaves of it; where its tools run in an environment elsewhere they are not read here.
#[must_use]
pub fn reported_instruction_sources(
    opened: &Json,
    home: Option<&std::path::Path>,
    workspace_root: &str,
    local: bool,
) -> Vec<orch_agent_contract::session::InstructionFile> {
    use orch_agent_contract::session::{InstructionEvidence, InstructionFile};
    let mut left = PROJECT_DOC_BUDGET;
    opened
        .get("instructionSources")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|source| {
            source
                .as_str()
                .or_else(|| source.get("path").and_then(Json::as_str))
        })
        .map(|reported| {
            let path = std::path::Path::new(reported);
            let name = path.file_name().map_or_else(
                || "an instruction file".to_owned(),
                |name| name.to_string_lossy().into_owned(),
            );
            if home.is_some_and(|home| path.starts_with(home)) {
                return InstructionFile::unmeasured(
                    format!("{name} of the harness's home"),
                    None,
                    InstructionEvidence::Reported,
                    "a file of the harness's home: it stays there, unread by Orch",
                )
                .of_home();
            }
            let label = path
                .strip_prefix(workspace_root)
                .ok()
                .filter(|relative| !relative.as_os_str().is_empty())
                .map_or(name, |relative| relative.display().to_string());
            if !local {
                return InstructionFile::unmeasured(
                    label,
                    Some(path),
                    InstructionEvidence::Reported,
                    "in the environment the tools run in, which the adapter does not read",
                );
            }
            let file =
                InstructionFile::measured(label, path, InstructionEvidence::Reported, Some(left));
            if let Some(loaded) = file.loaded_bytes {
                left = left.saturating_sub(loaded);
            }
            file
        })
        .collect()
}

/// How deep a `.codex` is looked through for anything in it. A harness that makes the directory for itself
/// leaves at most a few empty levels; anything deeper counts as something, and is asked about rather than walked.
const LOOK_DEPTH: usize = 8;

/// How many entries of a `.codex` are looked at, for the same reason.
const LOOK_ENTRIES: usize = 512;

/// Whether `place` holds anything of a configuration: anything there but directories that hold nothing. A link, a
/// file in the place of the directory, and what cannot be read all count as something.
fn holds_something(place: &std::path::Path) -> bool {
    match std::fs::symlink_metadata(place) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
        Ok(meta) if meta.is_dir() => {
            let mut seen = 0_usize;
            !only_empty_directories(place, 0, &mut seen)
        }
        _ => true,
    }
}

fn only_empty_directories(directory: &std::path::Path, depth: usize, seen: &mut usize) -> bool {
    if depth > LOOK_DEPTH {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return false;
    };
    for entry in entries {
        *seen += 1;
        if *seen > LOOK_ENTRIES {
            return false;
        }
        let Ok(entry) = entry else {
            return false;
        };
        // The type of the entry itself: a link to a directory is a link, and counts.
        let Ok(kind) = entry.file_type() else {
            return false;
        };
        if !kind.is_dir() || !only_empty_directories(&entry.path(), depth + 1, seen) {
            return false;
        }
    }
    true
}

/// What a workspace holds of Codex's own configuration on `chain`: each `.codex` with anything in it, relative
/// to the first directory of the chain, which is the workspace root.
#[must_use]
pub fn configuration_found(chain: &[std::path::PathBuf]) -> Vec<String> {
    let Some(root) = chain.first() else {
        return Vec::new();
    };
    chain
        .iter()
        .map(|directory| directory.join(crate::discovery::PROJECT_CONFIG_DIRECTORY))
        .filter(|place| holds_something(place))
        .map(|place| {
            place.strip_prefix(root).map_or_else(
                |_| place.display().to_string(),
                |relative| relative.display().to_string(),
            )
        })
        .collect()
}

/// What a session's chain holds of Codex's own configuration, and what the session says of it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WorkspaceLook {
    /// Each `.codex` with something in it, relative to the workspace root.
    pub found: Vec<String>,
    /// Whether the session uses them, by the answer of the people who manage the environment.
    pub trusted: bool,
    /// The trust of each directory of the chain, as the `projects` of the session's configuration: said only
    /// where something was found, by the answer for it ([`trust_of_chain`]).
    pub projects: Option<Json>,
}

/// The trust a session gives each directory of its chain where the workspace holds a configuration of Codex's
/// own: `trusted` for Trust, `untrusted` for Keep it out. Where nothing was found nothing is said, and the
/// harness finds the instruction files as it always does.
#[must_use]
pub fn trust_of_chain(
    chain: &[std::path::PathBuf],
    found: bool,
    answer: Option<ConfigTrust>,
) -> Option<Json> {
    let level = match answer? {
        _ if !found => return None,
        ConfigTrust::Trust => "trusted",
        ConfigTrust::KeepOut => "untrusted",
    };
    Some(Json::Object(
        chain
            .iter()
            .map(|directory| {
                (
                    directory.display().to_string(),
                    json!({ "trust_level": level }),
                )
            })
            .collect(),
    ))
}

/// Checks the instruction chain of a session before the native loader can read anything of it, and looks for a
/// configuration of Codex's own on it. A workspace that holds one is refused while nobody has answered for the
/// files it holds now ([`AdapterError::workspace_configuration`]); with an answer for them, the session says it
/// ([`WorkspaceLook::projects`]).
pub fn validate_instruction_files(
    root: &str,
    cwd: &str,
    marker: bool,
    answer: Option<&WorkspaceConfiguration>,
) -> Result<WorkspaceLook, AdapterError> {
    let chain = instruction_chain(root, cwd)?;
    let found = configuration_found(&chain);
    let answer = if found.is_empty() {
        None
    } else {
        let files =
            orch_agent_contract::session::configuration_files(std::path::Path::new(root), &found);
        match answer.filter(|answer| answer.files == files) {
            Some(answer) => Some(answer.answer),
            None => {
                return Err(AdapterError::workspace_configuration(
                    "Codex", &files, &found,
                ));
            }
        }
    };
    if marker {
        for (index, directory) in chain.iter().enumerate() {
            let found = directory.join(PROJECT_ROOT_MARKER);
            match std::fs::symlink_metadata(&found) {
                Ok(meta) if index == 0 && meta.is_dir() && !meta.file_type().is_symlink() => {
                    let mut entries = std::fs::read_dir(&found).map_err(|error| AdapterError::HarnessError(format!("the instruction-root marker could not be verified: {error}")))?;
                    if entries.next().is_some() {
                        return Err(AdapterError::Unsupported("the instruction-root marker is not empty; existing content was left untouched".to_owned()));
                    }
                }
                Err(error) if index > 0 && error.kind() == std::io::ErrorKind::NotFound => {}
                _ => return Err(AdapterError::Unsupported("the instruction-root marker is missing, conflicting, or duplicated on the directory chain; existing content was left untouched".to_owned())),
            }
            let actual = std::fs::canonicalize(directory).map_err(|error| {
                AdapterError::HarnessError(format!(
                    "the instruction directory could not be verified: {error}"
                ))
            })?;
            if actual != *directory {
                return Err(AdapterError::Unsupported("native instruction-root discovery through a directory link has not been verified".to_owned()));
            }
        }
    }
    let projects = trust_of_chain(&chain, !found.is_empty(), answer);
    Ok(WorkspaceLook {
        found,
        trusted: answer == Some(ConfigTrust::Trust),
        projects,
    })
}

/// Ordinary Orch calls finish at the connector before this native server deadline.
pub const ORCH_TOOL_TIMEOUT_SECONDS: u32 = 60;
/// Difference between the place's depth and this harness's depth index. The real pinned-harness
/// trial fixes this offset by checking that depth 1 admits children and refuses grandchildren.
pub const SUBAGENT_DEPTH_OFFSET: i64 = 0;

/// While `open_port` is offered: it waits sixty seconds for a person's decision and the connector seventy, so the
/// server's deadline is eighty, ten seconds after the connector's.
pub const ORCH_TOOL_TIMEOUT_OPEN_PORT_SECONDS: u32 = 80;

/// A byte-bounded answer must reach the model without the native default shortening it.
pub const ORCH_TOOL_OUTPUT_TOKENS: u32 = 40_000;

/// What the harness calls the model provider of a session.
#[must_use]
pub fn provider_name(session_key: &str) -> String {
    format!("{PROVIDER_PREFIX}{session_key}")
}

/// What the harness calls the environment of a session's tools.
#[must_use]
pub fn environment_name(spec: &SessionSpec) -> String {
    format!(
        "orch-{}-g{}-{}",
        spec.exec.environment_id, spec.exec.generation, spec.session_key
    )
}

/// How a session reaches the tools of its environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEnvironment {
    /// The harness's name for it.
    pub id: String,
    /// The address of the connection, at the connector's listener.
    pub url: String,
    /// The bearer token that connection requires.
    pub token: String,
    /// Where the agent starts, in the environment.
    pub cwd: String,
    /// The places it works in, in the environment.
    pub roots: Vec<String>,
}

impl RemoteEnvironment {
    /// The request that makes the harness know the environment.
    #[must_use]
    pub fn add_params(&self) -> Json {
        json!({
            "environmentId": self.id,
            "execServerUrl": self.url,
            "authBearerToken": self.token,
        })
    }

    /// Resolves the executor's shell before the harness chooses a session's native tools.
    pub async fn ensure_ready(&self, rpc: &crate::protocol::Rpc) -> Result<(), AdapterError> {
        let unavailable = || {
            AdapterError::Other {
            code: "environment_unreachable".to_owned(),
            message: "Codex could not read the environment executor's shell. Check that its connector is online and retry.".to_owned(),
            retryable: true,
        }
        };
        // environment/add only registers a connection. Its empty reply does not mean that the
        // exec server is connected; starting now can permanently omit shell tools from the turn.
        // Unlike the cached environment/status, info resolves the native connection and its shell.
        // The ordinary RPC deadline bounds this handshake; no local shell is substituted.
        let info = rpc
            .call("environment/info", json!({ "environmentId": self.id }))
            .await
            .map_err(|_| unavailable())?;
        if info
            .pointer("/shell/path")
            .and_then(Json::as_str)
            .is_some_and(|path| !path.trim().is_empty())
        {
            Ok(())
        } else {
            Err(unavailable())
        }
    }

    /// The environment as a turn (and a new thread) names it.
    #[must_use]
    pub fn selection(&self) -> Json {
        json!({ "environmentId": self.id, "cwd": self.cwd, "runtimeWorkspaceRoots": self.roots })
    }
}

/// The environment a session's tools are carried out in, if it is one of the harness's own executor.
pub fn remote_environment(
    spec: &SessionSpec,
    loopback: &Loopback,
) -> Result<Option<RemoteEnvironment>, AdapterError> {
    match spec.exec.executor.as_deref() {
        None => Ok(None),
        Some(EXECUTOR) => Ok(Some(RemoteEnvironment {
            id: environment_name(spec),
            url: loopback.exec(&spec.session_key),
            token: loopback.token.expose().to_owned(),
            cwd: spec.settings.cwd.clone(),
            roots: parameters::workspace_roots(&spec.settings.directory, &spec.settings.mounts),
        })),
        Some(other) => Err(AdapterError::Unsupported(format!(
            "the executor {other}: this harness reaches its tools through {EXECUTOR} or carries them out itself"
        ))),
    }
}

/// Everything that belongs to one session, as the configuration of `thread/start` carries it.
pub fn thread_config(
    spec: &SessionSpec,
    loopback: &Loopback,
    switched_off: &BTreeMap<String, Json>,
) -> Result<Json, AdapterError> {
    let mut config = Map::new();
    config.insert(
        "agents.max_concurrent_threads_per_session".to_owned(),
        json!(spec.settings.subagent_limits.at_once),
    );
    config.insert(
        "agents.max_depth".to_owned(),
        json!(spec.settings.subagent_limits.depth + SUBAGENT_DEPTH_OFFSET),
    );
    // The selected backend honours agents.max_depth; the other backend documents that it ignores it.
    config.insert("features.multi_agent_v2".to_owned(), json!(false));
    // Goal mode is Orch's own: the harness's would start turns that Orch did not check.
    config.insert("features.goals".to_owned(), json!(false));
    // Codex's own skills that write skills into its home are off for the thread: the home is shared by the
    // threads of its provider, and the skills a thread is given are Orch's to govern. Verified on 0.162.1 with
    // the model's request recorded: the thread's request no longer lists them, another thread's still does.
    config.insert(
        "skills".to_owned(),
        json!({ "config": HOME_WRITING_SKILLS.iter().map(|name| json!({ "name": name, "enabled": false })).collect::<Vec<_>>() }),
    );
    // A person question is available during ordinary work as well as planning.
    // The supported native build otherwise advertises the tool but refuses its call in default mode.
    config.insert(
        "features.default_mode_request_user_input".to_owned(),
        json!(true),
    );
    if spec.settings.instruction_root_marker {
        if spec.exec.executor.is_some() {
            return Err(AdapterError::Unsupported("native instruction discovery on a remote executor has not been verified; no workspace-marker override was applied".to_owned()));
        }
        config.insert(
            "project_root_markers".to_owned(),
            json!([PROJECT_ROOT_MARKER]),
        );
    }
    match &spec.provider {
        ProviderAccess::Forwarded { protocol, .. } => {
            if protocol != FORWARDED_PROTOCOL {
                return Err(AdapterError::Unsupported(format!(
                    "the protocol {protocol}: this harness speaks {FORWARDED_PROTOCOL} to the forwarder"
                )));
            }
            config.insert(
                "model_providers".to_owned(),
                json!({
                    provider_name(&spec.session_key): {
                        "name": "Orch",
                        "base_url": format!("{}/v1", loopback.llm(&spec.session_key)),
                        "wire_api": "responses",
                        "env_key": LLM_TOKEN_VARIABLE,
                        "requires_openai_auth": false,
                    }
                }),
            );
        }
        ProviderAccess::Native { .. } => {}
        ProviderAccess::Shared { .. } | ProviderAccess::Handover { .. } => {
            return Err(AdapterError::Unsupported("credentials".to_owned()));
        }
    }
    let mut servers: BTreeMap<String, Json> = switched_off.clone();
    for name in &spec.mcp {
        servers.insert(
            name.clone(),
            server_entry(&loopback.mcp(&spec.session_key, name), None),
        );
    }
    if !spec.settings.orch_tools.is_empty() {
        let tools: Vec<&str> = spec
            .settings
            .orch_tools
            .iter()
            .map(|tool| tool.name())
            .collect();
        let all = orch_agent_contract::events::OrchTool::ALL.len();
        let enabled = (tools.len() < all).then_some(tools.clone());
        let mut entry = server_entry(&loopback.orch(&spec.session_key), enabled);
        entry["tool_timeout_sec"] = json!(if spec
            .settings
            .orch_tools
            .contains(&orch_agent_contract::events::OrchTool::OpenPort)
        {
            ORCH_TOOL_TIMEOUT_OPEN_PORT_SECONDS
        } else {
            ORCH_TOOL_TIMEOUT_SECONDS
        });
        entry["default_tools_approval_mode"] = json!("approve");
        entry["required"] = json!(true);
        entry["tools"] = Json::Object(
            tools
                .into_iter()
                .map(|name| {
                    (
                        name.to_owned(),
                        json!({ "output_token_limit": ORCH_TOOL_OUTPUT_TOKENS }),
                    )
                })
                .collect(),
        );
        servers.insert(orch_agent_contract::loopback::ORCH_SERVER.to_owned(), entry);
    }
    if !servers.is_empty() {
        config.insert("mcp_servers".to_owned(), json!(servers));
    }
    config.insert("review_model".to_owned(), json!(spec.settings.model));
    // How large the model's context is and where the conversation is shortened, which this harness holds apart.
    config.extend(parameters::context_members(&spec.settings.params));
    Ok(Json::Object(config))
}

fn server_entry(url: &str, enabled_tools: Option<Vec<&str>>) -> Json {
    let mut entry = json!({ "url": url, "bearer_token_env_var": MCP_TOKEN_VARIABLE });
    if let Some(tools) = enabled_tools {
        entry["enabled_tools"] = json!(tools);
    }
    entry
}

/// What opening a session asks of the harness.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenPlan {
    /// The method: `thread/start`, `thread/resume` or `thread/fork`.
    pub method: &'static str,
    /// Its parameters.
    pub params: Json,
    /// The environment to make known first, when the tools are carried out elsewhere.
    pub remote: Option<RemoteEnvironment>,
}

/// What opening a session needs to know beyond what the session's own description gives.
#[derive(Debug, Clone, Default)]
pub struct OpenContext {
    /// The servers of the configuration the harness reads that the session switches off, each as the entry that
    /// does so ([`crate::discovery::switch_off_entries`]).
    pub switched_off: BTreeMap<String, Json>,
    /// The provider the harness uses when the session names none: the vendor's, or the one the person's own
    /// configuration says.
    pub default_provider: Option<String>,
    /// For a fork: the turn to fork through, inclusive.
    pub fork_through: Option<String>,
    /// The trust of the directories of the session's chain, where its workspace holds a configuration of Codex's
    /// own and the environment has an answer for it ([`WorkspaceLook::projects`]).
    pub projects: Option<Json>,
}

/// The skills Codex brings that write skills into its home, switched off in every thread Orch runs.
pub const HOME_WRITING_SKILLS: [&str; 2] = ["skill-installer", "skill-creator"];

/// The id under which a thread's skills are its capability root.
pub const SKILLS_ROOT_ID: &str = "orch-skills";
/// The environment Codex runs a thread's commands in when no other is named.
pub const LOCAL_ENVIRONMENT: &str = "local";

/// The parameters that open a session by `mode`: the thread, where it works, how it is confined, how it
/// reaches the model and the tools, and what it is told.
pub fn open_plan(
    spec: &SessionSpec,
    loopback: &Loopback,
    context: &OpenContext,
) -> Result<OpenPlan, AdapterError> {
    let settings = &spec.settings;
    let remote = remote_environment(spec, loopback)?;
    let mut params = Map::new();
    let method = match settings.mode {
        SessionMode::New | SessionMode::ImportNeutral => "thread/start",
        SessionMode::Resume | SessionMode::Adopt => "thread/resume",
        SessionMode::Fork => "thread/fork",
        // A thread's own history is not carried as a file: there is no request that opens one.
        SessionMode::ImportNative => return Err(crate::native_archive_refused()),
    };
    if matches!(method, "thread/start" | "thread/fork") {
        // Root sessions can ask the person for a data form even when commands need no approval.
        params.insert("threadSource".to_owned(), json!("user"));
    }
    if method != "thread/start" {
        let native = settings.native_session_ref.as_deref().ok_or_else(|| {
            AdapterError::NotFound(format!(
                "a session opened by {:?} names the session",
                settings.mode
            ))
        })?;
        params.insert("threadId".to_owned(), json!(native));
    }
    if method != "thread/start" {
        // The answer to a resume or a fork is the thread's record without its turns: with them the harness reads
        // every page of a paginated history into the answer (a deprecated route it keeps for old clients), so that
        // the cost of opening a thread would grow with its history. The turns are read, a page at a time, only by
        // whoever needs them.
        params.insert("excludeTurns".to_owned(), json!(true));
    }
    if let Some(turn) = &context.fork_through {
        params.insert("lastTurnId".to_owned(), json!(turn));
    }
    params.insert("model".to_owned(), json!(settings.model));
    params.insert(
        "modelProvider".to_owned(),
        match &spec.provider {
            ProviderAccess::Forwarded { .. } => json!(provider_name(&spec.session_key)),
            _ => json!(
                context
                    .default_provider
                    .as_deref()
                    .unwrap_or(VENDOR_PROVIDER)
            ),
        },
    );
    params.insert("cwd".to_owned(), json!(settings.cwd));
    params.insert(
        "approvalPolicy".to_owned(),
        json!(parameters::approval_policy(settings.approval_mode)),
    );
    params.insert(
        "approvalsReviewer".to_owned(),
        json!(parameters::approval_reviewer(settings.approval_mode)),
    );
    params.insert(
        "sandbox".to_owned(),
        json!(parameters::sandbox_word(settings.sandbox_mode)),
    );
    let mut config = thread_config(spec, loopback, &context.switched_off)?;
    if let (Some(projects), Json::Object(members)) = (&context.projects, &mut config) {
        members.insert("projects".to_owned(), projects.clone());
    }
    params.insert("config".to_owned(), config);
    if !settings.instructions.is_empty() {
        params.insert(
            "developerInstructions".to_owned(),
            json!(settings.instructions),
        );
    }
    // The thread's skills are a capability root of its own, given when the thread starts and kept by Codex for it:
    // what the connector puts into the directory later reaches the thread's next session. Where its
    // commands run in another environment the root is not given: the files would not be where they run.
    if let (Some(skills), None, "thread/start") = (&spec.skills, &remote, method) {
        params.insert(
            "selectedCapabilityRoots".to_owned(),
            json!([{
                "id": SKILLS_ROOT_ID,
                "location": {
                    "type": "environment",
                    "environmentId": LOCAL_ENVIRONMENT,
                    "path": format!("{}/skills", skills.dir),
                },
            }]),
        );
    }
    match &remote {
        Some(environment) if method == "thread/start" => {
            params.insert("environments".to_owned(), json!([environment.selection()]));
        }
        Some(_) => {}
        None => {
            params.insert(
                "runtimeWorkspaceRoots".to_owned(),
                json!(parameters::workspace_roots(
                    &settings.directory,
                    &settings.mounts
                )),
            );
        }
    }
    if method == "thread/start"
        && TurnSettings::from_params(&settings.params)
            .cyber_access_program
            .as_deref()
            .is_some_and(parameters::is_daybreak)
    {
        params.insert("daybreakEnabled".to_owned(), json!(true));
    }
    Ok(OpenPlan {
        method,
        params: Json::Object(params),
        remote,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};

    #[test]
    fn reported_sources_name_a_home_file_and_measure_the_projects_against_their_budget() {
        let root =
            std::env::temp_dir().join(format!("orch-codex-sources-{}", orch_types::new_id()));
        std::fs::create_dir_all(root.join("sub")).unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(root.join("AGENTS.md"), "a".repeat(30 * 1024))
            .unwrap_or_else(|error| panic!("{error}"));
        std::fs::write(root.join("sub/AGENTS.md"), "b".repeat(4 * 1024))
            .unwrap_or_else(|error| panic!("{error}"));
        let home = std::path::Path::new("/harness-home/.codex");
        let opened = json!({ "instructionSources": [
            "/harness-home/.codex/AGENTS.md",
            root.join("AGENTS.md").display().to_string(),
            root.join("sub/AGENTS.md").display().to_string(),
        ] });
        let workspace = root.display().to_string();
        let sources = reported_instruction_sources(&opened, Some(home), &workspace, true);
        assert_eq!(sources.len(), 3);
        assert!(
            sources[0].home && sources[0].path.is_none(),
            "a home file has no path"
        );
        assert_eq!(sources[1].label, "AGENTS.md");
        assert_eq!(sources[1].loaded_bytes, Some(30 * 1024));
        assert_eq!(sources[2].label, "sub/AGENTS.md");
        assert_eq!(
            (sources[2].loaded_bytes, sources[2].cut_bytes),
            (Some(2 * 1024), Some(2 * 1024)),
            "the project budget is shared by the project's files"
        );
        let elsewhere = reported_instruction_sources(&opened, Some(home), &workspace, false);
        assert_eq!(
            elsewhere[1].bytes, None,
            "a file of an environment elsewhere is not read here"
        );
        assert!(elsewhere[1].reason.is_some());
        let _ = std::fs::remove_dir_all(&root);
    }

    use orch_agent_contract::events::OrchTool;
    use orch_agent_contract::session::{ExecAccess, MountGrant, SessionSettings};
    use orch_plugin_contract::Secret;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn loopback() -> Loopback {
        Loopback::new(41231, Secret::new("t".repeat(64)))
    }

    fn id(n: u8) -> orch_types::Id {
        format!("0199abcd-1234-7000-8000-0000000000{n:02}")
            .parse()
            .unwrap()
    }

    fn spec(mode: SessionMode) -> SessionSpec {
        SessionSpec {
            settings: SessionSettings {
                agent_id: id(1),
                thread_id: id(2),
                segment_id: id(3),
                mode,
                native_session_ref: (mode != SessionMode::New).then(|| "thr_1".to_owned()),
                upto_turn_ordinal: None,
                workspace_root: "/workspace/web".into(),
                instruction_root_marker: false,
                directory: "/workspace/web".into(),
                cwd: "/workspace/web/api".into(),
                mounts: vec![MountGrant {
                    name: "out".into(),
                    path: "/data/out".into(),
                    read_only: false,
                }],
                model: "gpt-6-sol".into(),
                effort: Some("high".into()),
                params: json!({}),
                approval_mode: ApprovalMode::OnRequest,
                sandbox_mode: SandboxMode::Workspace,
                instructions: "You are working inside Orch.".into(),
                available_models: vec!["gpt-6-sol".to_owned()],
                subagent_limits: Default::default(),
                memory: None,
                arrangement: ArrangementType::KeySeparated,
                orch_tools: OrchTool::ALL.to_vec(),
                orch_tool_schemas: Default::default(),
                raw_events: false,
                side_reading: None,
                workspace_configuration: None,
            },
            session_key: KEY.into(),
            archive: None,
            history: None,
            provider: ProviderAccess::Forwarded {
                provider_id: id(4),
                protocol: FORWARDED_PROTOCOL.into(),
            },
            exec: ExecAccess {
                executor: None,
                tool_set: None,
                environment_id: id(5),
                generation: 3,
            },
            mcp: vec!["tmux".into()],
            skills: None,
        }
    }

    #[test]
    fn the_skills_that_write_into_the_shared_home_are_off_in_every_thread() {
        let plan = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        assert_eq!(
            plan.params["config"]["skills"]["config"],
            json!([
                { "name": "skill-installer", "enabled": false },
                { "name": "skill-creator", "enabled": false },
            ])
        );
    }

    #[test]
    fn a_new_thread_is_given_its_skills_directory_as_its_own_capability_root_and_a_resumed_one_keeps_it()
     {
        let mut started = spec(SessionMode::New);
        started.skills = Some(orch_agent_contract::session::SessionSkills {
            dir: "/data/skills/threads/t1".to_owned(),
            names: vec!["greet".to_owned()],
        });
        let plan = open_plan(&started, &loopback(), &OpenContext::default()).unwrap();
        assert_eq!(
            plan.params["selectedCapabilityRoots"],
            json!([{
                "id": SKILLS_ROOT_ID,
                "location": { "type": "environment", "environmentId": "local", "path": "/data/skills/threads/t1/skills" },
            }])
        );
        // Codex keeps a thread's roots: a resume names none, and only `thread/start` takes them.
        let mut resumed = started.clone();
        resumed.settings.mode = SessionMode::Resume;
        resumed.settings.native_session_ref = Some("native-1".to_owned());
        let plan = open_plan(&resumed, &loopback(), &OpenContext::default()).unwrap();
        assert!(plan.params.get("selectedCapabilityRoots").is_none());
        let plain = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        assert!(plain.params.get("selectedCapabilityRoots").is_none());
    }

    #[test]
    fn a_forwarded_session_gets_a_provider_of_its_own_at_the_connectors_listener() {
        let plan = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        assert_eq!(plan.method, "thread/start");
        assert_eq!(plan.params["modelProvider"], format!("orch_{KEY}"));
        let provider = &plan.params["config"]["model_providers"][format!("orch_{KEY}")];
        assert_eq!(
            provider["base_url"],
            format!("http://127.0.0.1:41231/llm/{KEY}/v1")
        );
        assert_eq!(provider["wire_api"], "responses");
        assert_eq!(provider["env_key"], "ORCH_LLM_TOKEN");
        assert!(
            !plan.params.to_string().contains(&"t".repeat(64)),
            "the token is found in the environment of the process and is in no parameter"
        );
    }

    #[test]
    fn a_chat_completions_provider_is_refused_before_starting_the_pinned_harness() {
        let mut requested = spec(SessionMode::New);
        requested.provider = ProviderAccess::Forwarded {
            provider_id: id(4),
            protocol: "openai-chat".into(),
        };
        assert!(open_plan(&requested, &loopback(), &OpenContext::default()).is_err());
    }

    #[test]
    fn a_session_given_a_larger_context_tells_the_harness_the_window_and_where_it_shortens_the_conversation()
     {
        let mut large = spec(SessionMode::New);
        large.settings.params = json!({ "context": 1_000_000, "context_window": 272_000 });
        for mode in [SessionMode::New, SessionMode::Resume] {
            large.settings.mode = mode;
            large.settings.native_session_ref = Some("thr_1".into());
            let plan = open_plan(&large, &loopback(), &OpenContext::default()).unwrap();
            let config = &plan.params["config"];
            assert_eq!(config["model_context_window"], 1_000_000, "{mode:?}");
            assert_eq!(
                config["model_auto_compact_token_limit"], 900_000,
                "{mode:?}"
            );
        }
        // Left to the model's own window, only the window is told, and a session with no window tells nothing.
        let mut own = spec(SessionMode::New);
        own.settings.params = json!({ "context": 272_000, "context_window": 272_000 });
        let config = open_plan(&own, &loopback(), &OpenContext::default())
            .unwrap()
            .params["config"]
            .clone();
        assert_eq!(config["model_context_window"], 272_000);
        assert!(config.get("model_auto_compact_token_limit").is_none());
        let plain = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap()
        .params["config"]
            .clone();
        assert!(plain.get("model_context_window").is_none());
        assert!(plain.get("model_auto_compact_token_limit").is_none());
    }

    #[test]
    fn the_session_is_given_the_servers_it_is_given_and_orchs_own_tools_beside_them() {
        let plan = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        let servers = &plan.params["config"]["mcp_servers"];
        assert_eq!(
            servers["tmux"]["url"],
            format!("http://127.0.0.1:41231/mcp/{KEY}/tmux")
        );
        assert_eq!(
            servers["orch"]["url"],
            format!("http://127.0.0.1:41231/mcp/{KEY}/orch")
        );
        assert_eq!(servers["orch"]["bearer_token_env_var"], "ORCH_MCP_TOKEN");
        assert!(
            servers["orch"].get("enabled_tools").is_none(),
            "every registered tool is on"
        );
        let mut few = spec(SessionMode::New);
        few.settings.orch_tools = vec![OrchTool::ListShared, OrchTool::ReadShared];
        let plan = open_plan(&few, &loopback(), &OpenContext::default()).unwrap();
        assert_eq!(
            plan.params["config"]["mcp_servers"]["orch"]["enabled_tools"],
            json!(["list_shared", "read_shared"])
        );
        let mut none = spec(SessionMode::New);
        none.settings.orch_tools.clear();
        none.mcp.clear();
        let plan = open_plan(&none, &loopback(), &OpenContext::default()).unwrap();
        assert!(
            plan.params["config"].get("mcp_servers").is_none(),
            "no orch server at all"
        );
    }

    #[test]
    fn the_servers_of_the_configuration_the_harness_reads_are_switched_off_for_the_session() {
        let context = OpenContext {
            switched_off: BTreeMap::from([
                (
                    "github".to_owned(),
                    json!({ "command": "npx", "enabled": false }),
                ),
                ("tmux".to_owned(), json!({ "enabled": false })),
            ]),
            ..OpenContext::default()
        };
        let plan = open_plan(&spec(SessionMode::New), &loopback(), &context).unwrap();
        let servers = &plan.params["config"]["mcp_servers"];
        assert_eq!(
            servers["github"],
            json!({ "command": "npx", "enabled": false })
        );
        assert_eq!(
            servers["tmux"]["url"],
            format!("http://127.0.0.1:41231/mcp/{KEY}/tmux"),
            "a server Orch gives wins over one of the same name that is switched off"
        );
    }

    #[test]
    fn subagents_always_receive_the_places_limits_in_the_agents_table_without_the_legacy_alias() {
        let mut session = spec(SessionMode::New);
        for (at_once, depth) in [(8, 1), (3, 2), (64, 5)] {
            session.settings.subagent_limits =
                orch_agent_contract::session::SubagentLimits { at_once, depth };
            let config = thread_config(&session, &loopback(), &BTreeMap::new()).unwrap();
            assert_eq!(config["agents.max_concurrent_threads_per_session"], at_once);
            assert_eq!(config["agents.max_depth"], depth + SUBAGENT_DEPTH_OFFSET);
            assert!(config.get("agents.max_threads").is_none());
            assert!(config.get("agents.default_subagent_model").is_none());
            assert!(
                config
                    .get("agents.default_subagent_reasoning_effort")
                    .is_none()
            );
            assert!(
                config.get("agents").is_none(),
                "leaf overrides preserve the person's other agents configuration"
            );
            assert!(config.get("max_depth").is_none());
        }
    }

    #[test]
    fn native_instruction_discovery_keeps_the_working_directory_and_review_model() {
        let plan = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        assert!(plan.params["config"].get("project_root_markers").is_none());
        assert_eq!(plan.params["config"]["review_model"], "gpt-6-sol");
        assert_eq!(plan.params["cwd"], "/workspace/web/api");
        assert_eq!(plan.params["approvalPolicy"], "on-request");
        assert_eq!(plan.params["sandbox"], "workspace-write");
        assert_eq!(
            plan.params["developerInstructions"],
            "You are working inside Orch."
        );
        assert_eq!(
            plan.params["runtimeWorkspaceRoots"],
            json!(["/workspace/web", "/data/out"])
        );
    }

    #[test]
    fn a_session_whose_tools_are_carried_out_elsewhere_is_given_an_environment_of_its_own() {
        let mut with = spec(SessionMode::New);
        with.exec.executor = Some(EXECUTOR.into());
        let plan = open_plan(&with, &loopback(), &OpenContext::default()).unwrap();
        let remote = plan.remote.clone().unwrap();
        assert_eq!(remote.id, format!("orch-{}-g3-{KEY}", id(5)));
        assert_eq!(remote.url, format!("ws://127.0.0.1:41231/exec/{KEY}"));
        assert_eq!(
            plan.params["environments"],
            json!([{ "environmentId": remote.id, "cwd": "/workspace/web/api",
                     "runtimeWorkspaceRoots": ["/workspace/web", "/data/out"] }])
        );
        assert!(plan.params.get("runtimeWorkspaceRoots").is_none());
        assert_eq!(remote.add_params()["authBearerToken"], "t".repeat(64));
        // The environments of a resumed thread are named by each turn, not by the request.
        let mut resumed = with.clone();
        resumed.settings.mode = SessionMode::Resume;
        resumed.settings.native_session_ref = Some("thr_1".into());
        let plan = open_plan(&resumed, &loopback(), &OpenContext::default()).unwrap();
        assert_eq!(plan.method, "thread/resume");
        assert!(plan.params.get("environments").is_none());
        // An executor of another harness is not one this harness can work through.
        let mut other = spec(SessionMode::New);
        other.exec.executor = Some("orch-exec".into());
        assert!(matches!(
            open_plan(&other, &loopback(), &OpenContext::default()),
            Err(AdapterError::Unsupported(_))
        ));
    }

    #[test]
    fn each_way_to_open_a_session_is_a_request_of_the_harness() {
        let method = |mode| {
            open_plan(&spec(mode), &loopback(), &OpenContext::default())
                .unwrap()
                .method
        };
        assert_eq!(method(SessionMode::New), "thread/start");
        assert_eq!(method(SessionMode::ImportNeutral), "thread/start");
        assert_eq!(method(SessionMode::Resume), "thread/resume");
        assert_eq!(method(SessionMode::Adopt), "thread/resume");
        assert!(matches!(
            open_plan(
                &spec(SessionMode::ImportNative),
                &loopback(),
                &OpenContext::default()
            ),
            Err(AdapterError::Unsupported(_))
        ));
        assert_eq!(method(SessionMode::Fork), "thread/fork");
        let fork = open_plan(
            &spec(SessionMode::Fork),
            &loopback(),
            &OpenContext {
                fork_through: Some("turn_2".into()),
                ..OpenContext::default()
            },
        )
        .unwrap();
        assert_eq!(fork.params["threadId"], "thr_1");
        assert_eq!(fork.params["lastTurnId"], "turn_2");
        // Opening a thread never asks for its turns, whatever their number; a new thread has none to leave out.
        for mode in [SessionMode::Resume, SessionMode::Adopt, SessionMode::Fork] {
            let plan = open_plan(&spec(mode), &loopback(), &OpenContext::default()).unwrap();
            assert_eq!(plan.params["excludeTurns"], true, "{mode:?}");
        }
        for mode in [SessionMode::New, SessionMode::ImportNeutral] {
            let plan = open_plan(&spec(mode), &loopback(), &OpenContext::default()).unwrap();
            assert!(plan.params.get("excludeTurns").is_none(), "{mode:?}");
        }
        let mut lost = spec(SessionMode::Resume);
        lost.settings.native_session_ref = None;
        assert!(matches!(
            open_plan(&lost, &loopback(), &OpenContext::default()),
            Err(AdapterError::NotFound(_))
        ));
    }

    #[test]
    fn the_harnesss_own_sign_in_is_used_when_the_session_names_no_provider_of_its_own() {
        let mut native = spec(SessionMode::Resume);
        native.provider = ProviderAccess::Native { provider_id: id(4) };
        let plan = open_plan(&native, &loopback(), &OpenContext::default()).unwrap();
        assert_eq!(plan.params["modelProvider"], "openai");
        assert!(plan.params["config"].get("model_providers").is_none());
        let plan = open_plan(
            &native,
            &loopback(),
            &OpenContext {
                default_provider: Some("ollama".into()),
                ..OpenContext::default()
            },
        )
        .unwrap();
        assert_eq!(plan.params["modelProvider"], "ollama");
        native.provider = ProviderAccess::Shared {
            provider_id: id(4),
            form: "access_token".into(),
        };
        assert!(matches!(
            open_plan(&native, &loopback(), &OpenContext::default()),
            Err(AdapterError::Unsupported(_))
        ));
    }

    #[test]
    fn a_daybreak_program_enables_daybreak_for_a_new_thread() {
        let mut daybreak = spec(SessionMode::New);
        daybreak.settings.params = json!({ "cyber_access_program": "daybreakBlue" });
        let plan = open_plan(&daybreak, &loopback(), &OpenContext::default()).unwrap();
        assert_eq!(plan.params["daybreakEnabled"], true);
        let plan = open_plan(
            &spec(SessionMode::New),
            &loopback(),
            &OpenContext::default(),
        )
        .unwrap();
        assert!(plan.params.get("daybreakEnabled").is_none());
    }
}
