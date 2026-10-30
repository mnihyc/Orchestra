//! Per-session flags and settings: the MCP set and nothing else, `AGENTS.md` read beside the harness's own file, its memory kept in the thread's directory or switched off, resume, fork, and the restart a change of effort takes.
//!
//! A session's process is started with everything it needs on its command line and in its environment, and
//! nothing is written to a settings file: the flags carry the model, the mode, the tools and the servers,
//! `--settings` carries what has no flag (the memory, the instruction files, the shell sandbox), and the
//! environment carries the credential and the switches. [`launch`] works all of it out from the session and
//! the turn without touching anything, so that every rule is tested on its result.
//!
//! Two things are held on purpose. The set of MCP servers is exactly the one Orch built (`--strict-mcp-config`):
//! the harness loads no server from its user's configuration or from a file in the directory. And the harness
//! is always told where its memory is, or that it has none: left alone it keeps notes under its home, which
//! one provider's every thread shares.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use orch_agent_contract::adapter::AdapterError;
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::loopback::{EXEC_SERVER, Loopback, ORCH_SERVER};
use orch_agent_contract::session::{NoteKeeper, ProviderAccess, SessionSpec};
use orch_types::Json;
use serde_json::json;

use crate::approvals::{BYPASS_ENV, PLAN_MODE, permission_mode, sandbox_settings};

/// Where the harness keeps the plans it writes, relative to the thread's directory.
pub const PLANS_DIRECTORY: &str = ".orch/plans";
use crate::config::{MANAGED_PROJECT_DIR, MANAGED_XDG_DIR, MANAGED_XDG_PLACES, ProfileConfig};
use crate::parameters::{
    FAST_TIER, Preflight, Values, assumed_window, auto_compact_window, effort_for_harness,
    model_argument, model_environment,
};
use crate::process::{LaunchSpec, base_environment};
use crate::tools::{tool_arguments, tool_set};

/// The value of the harness's instruction-file option that reads `AGENTS.md` beside its own instruction
/// file. Left to its default the harness reads `AGENTS.md` only where no `CLAUDE.md` lies in or above the
/// directory, and the file of a thread's directory would go unread beside a repository that has one.
pub const INSTRUCTION_FILES_SETTING: &str = "claude-md-and-agents-md";

/// The plugin of the harness that reads `AGENTS.md`.
const AGENTS_MD_PLUGIN: &str = "cc-plugin-agents-md@builtin";

/// The name under which the connector's bearer token reaches the harness for its MCP servers; the servers'
/// headers name this variable, so that the token is never in an argument.
pub const MCP_TOKEN_VARIABLE: &str = "ORCH_MCP_TOKEN";

/// The connector's ordinary deadline leaves ten seconds for its answer to arrive here.
pub const ORCH_TOOL_TIMEOUT_MILLIS: u32 = 60_000;

/// While `open_port` is offered: it waits sixty seconds for a person's decision and the connector seventy, so the
/// server's deadline is eighty, ten seconds after the connector's.
pub const ORCH_TOOL_TIMEOUT_OPEN_PORT_MILLIS: u32 = 80_000;

/// How long the harness waits for a call of the executor's tools: the longest foreground command the executor
/// runs (ten minutes, the bound of its `Bash`, which is the harness's own shell bound) and half a minute for the
/// answer to come back. Left to the harness's default, a command the executor still allows would be cut short.
pub const EXEC_TOOL_TIMEOUT_MILLIS: u32 = 630_000;

/// What fixes every session of one started harness.
#[derive(Debug, Clone)]
pub struct HarnessSetup {
    /// The program to run.
    pub program: String,
    /// The home of the harness, when one is given.
    pub home: Option<PathBuf>,
    /// Whether the home is one Orch made for the harness: its sessions then keep their transcripts in one
    /// directory of it, whatever their working directory.
    pub managed_home: bool,
    /// Where the connector's listener is and the token it wants.
    pub loopback: Loopback,
    /// The variables the profile adds.
    pub profile_env: BTreeMap<String, String>,
    /// The profile's own options.
    pub config: ProfileConfig,
    /// What this process was started with, of which the harness inherits a few.
    pub inherited: BTreeMap<String, String>,
    /// Whether this adapter runs as the administrator, where the harness refuses to bypass permissions.
    pub as_root: bool,
}

/// What a turn states every time.
#[derive(Debug, Clone, PartialEq)]
pub struct TurnSettings {
    /// The routing name of the model.
    pub model: String,
    /// The reasoning effort, if the model takes one.
    pub effort: Option<String>,
    /// How the harness treats actions it does not already trust.
    pub approval_mode: ApprovalMode,
    /// What its shell sandbox lets commands do.
    pub sandbox_mode: SandboxMode,
    /// The values of the adapter's parameters.
    pub params: Json,
    /// Whether the turn plans: the harness's own plan mode, whatever the approval mode is.
    pub plan: bool,
}

/// Where a session's conversation comes from when its process starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A conversation that begins now, under the id the adapter chose.
    New,
    /// A conversation the harness holds, continued.
    Resume,
    /// A copy of another session, which the new id names.
    Fork {
        /// The session it is a copy of.
        from: String,
    },
    /// A native fork ending at one verified transcript checkpoint, inclusive.
    ForkPrefix {
        /// The retained source session.
        from: String,
        /// The exact parent of the user input excluded from the new session.
        through: String,
    },
}

/// One process of a session, before it is started.
#[derive(Debug, Clone, Copy)]
pub struct Launch<'a> {
    /// What fixes the harness.
    pub setup: &'a HarnessSetup,
    /// The session.
    pub spec: &'a SessionSpec,
    /// The harness's id of the session.
    pub session_id: &'a str,
    /// Where its conversation comes from.
    pub origin: &'a Origin,
    /// What the turn states.
    pub turn: &'a TurnSettings,
    /// The long-lived token the harness runs on, when it has one.
    pub oauth_token: Option<&'a str>,
}

/// Whether the harness works through Orch's executor: it is with its provider, and the tools that act on a
/// machine belong to the environment.
#[must_use]
pub fn through_executor(spec: &SessionSpec) -> bool {
    spec.exec.executor.is_some()
}

/// What makes a thread's skills directory a plugin of the harness's: its name, and that it is Orch's.
const SKILLS_PLUGIN_MANIFEST: &str = r#"{
  "name": "orch-skills",
  "version": "1.0.0",
  "description": "The skills Orch gives this thread."
}
"#;

/// Makes the directory of a thread's skills a plugin of this harness, as a session is opened: the connector lays
/// out `skills/<name>/SKILL.md` and nothing else, and what the harness loads as a plugin is this adapter's to
/// make. Nothing is done where the session's tools act elsewhere, since the skills' files would not be there.
pub fn skills_plugin(spec: &SessionSpec) -> Result<(), AdapterError> {
    let Some(skills) = spec.skills.as_ref().filter(|_| !through_executor(spec)) else {
        return Ok(());
    };
    let plugin = std::path::Path::new(&skills.dir).join(".claude-plugin");
    std::fs::create_dir_all(&plugin).map_err(|error| {
        AdapterError::Internal(format!("making the plugin of a thread's skills: {error}"))
    })?;
    let manifest = plugin.join("plugin.json");
    if std::fs::read_to_string(&manifest).ok().as_deref() != Some(SKILLS_PLUGIN_MANIFEST) {
        std::fs::write(&manifest, SKILLS_PLUGIN_MANIFEST).map_err(|error| {
            AdapterError::Internal(format!("writing the plugin of a thread's skills: {error}"))
        })?;
    }
    Ok(())
}

/// Where the process of a session works: the thread's `cwd` when the harness is in the environment, and
/// an empty directory of its own when it is with its provider and has no machine to work on.
#[must_use]
pub fn working_directory(setup: &HarnessSetup, spec: &SessionSpec) -> PathBuf {
    if through_executor(spec) {
        let base = setup.home.clone().unwrap_or_else(std::env::temp_dir);
        base.join("sessions").join(&spec.session_key).join("work")
    } else {
        PathBuf::from(&spec.settings.cwd)
    }
}

/// The private file supplied with the instruction flag, outside the workspace's own files.
#[must_use]
pub fn instructions_path(setup: &HarnessSetup, spec: &SessionSpec) -> PathBuf {
    let base = setup.home.clone().unwrap_or_else(std::env::temp_dir);
    base.join("sessions")
        .join(&spec.session_key)
        .join("instructions.txt")
}

/// The settings files of a project the harness reads, where a workspace may hold them.
const PROJECT_SETTINGS: [&str; 2] = [".claude/settings.json", ".claude/settings.local.json"];

/// The settings of the harness's own that the workspace holds on the way from its root to where the session
/// works, relative to the root: each of `PROJECT_SETTINGS` that is there. A `.claude` the harness made for
/// itself without them is not a configuration.
pub fn configuration_found(
    spec: &SessionSpec,
) -> Result<Vec<String>, orch_agent_contract::adapter::AdapterError> {
    use orch_agent_contract::adapter::AdapterError;
    let root = std::path::Path::new(&spec.settings.workspace_root);
    let cwd = std::path::Path::new(&spec.settings.cwd);
    if !root.is_absolute() || !cwd.starts_with(root) {
        return Err(AdapterError::Unsupported("the instruction chain needs an absolute workspace root containing the working directory".to_owned()));
    }
    let mut found = Vec::new();
    for directory in cwd
        .ancestors()
        .take_while(|directory| directory.starts_with(root))
    {
        for name in PROJECT_SETTINGS {
            let source = directory.join(name);
            match std::fs::symlink_metadata(&source) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                _ => found.push(source.strip_prefix(root).map_or_else(
                    |_| source.display().to_string(),
                    |relative| relative.display().to_string(),
                )),
            }
        }
    }
    Ok(found)
}

/// Checks the workspace's own settings before the project loader can activate them: a workspace that holds some
/// is refused while nobody who manages the environment has answered for them
/// ([`orch_agent_contract::adapter::AdapterError::workspace_configuration`]). Trusted, the harness reads them as
/// the project's; kept out, the session reads no project source at all ([`setting_sources`]), since this build
/// reads a project's instruction files only together with its settings.
pub fn validate_instruction_settings(
    spec: &SessionSpec,
) -> Result<(), orch_agent_contract::adapter::AdapterError> {
    if through_executor(spec) {
        return Ok(());
    }
    let found = configuration_found(spec)?;
    if found.is_empty() {
        return Ok(());
    }
    // An answer holds for the files it was given for: when they change, the question is asked again.
    let files = orch_agent_contract::session::configuration_files(
        std::path::Path::new(&spec.settings.workspace_root),
        &found,
    );
    if spec
        .settings
        .workspace_configuration
        .as_ref()
        .is_some_and(|answered| answered.files == files)
    {
        return Ok(());
    }
    Err(
        orch_agent_contract::adapter::AdapterError::workspace_configuration(
            "Claude Code",
            &files,
            &found,
        ),
    )
}

/// The setting sources a session's process is started with: the profile's, and for a workspace whose own
/// configuration its environment's managers kept out, those of the profile that are no project's (`user`), or
/// `user` alone where the profile names nothing else.
#[must_use]
pub fn setting_sources(configured: &str, spec: &SessionSpec) -> String {
    if spec
        .settings
        .workspace_configuration
        .as_ref()
        .map(|answered| answered.answer)
        != Some(orch_agent_contract::session::ConfigTrust::KeepOut)
    {
        return configured.to_owned();
    }
    let kept: Vec<&str> = configured
        .split(',')
        .map(str::trim)
        .filter(|source| !matches!(*source, "project" | "local" | ""))
        .collect();
    if kept.is_empty() {
        "user".to_owned()
    } else {
        kept.join(",")
    }
}

/// The MCP servers of a session: Orch's tools when it has any, the executor's tools when the harness works
/// through a tool set that has a server, and the environment's servers by name.
#[must_use]
pub fn mcp_config(setup: &HarnessSetup, spec: &SessionSpec) -> Json {
    let entry = |url: String| {
        json!({
            "type": "http",
            "url": url,
            "headers": { "Authorization": format!("Bearer ${{{MCP_TOKEN_VARIABLE}}}") },
        })
    };
    let key = &spec.session_key;
    let mut servers = serde_json::Map::new();
    if !spec.settings.orch_tools.is_empty() {
        let mut ordinary = entry(setup.loopback.orch(key));
        ordinary["timeout"] = json!(if spec
            .settings
            .orch_tools
            .contains(&orch_agent_contract::events::OrchTool::OpenPort)
        {
            ORCH_TOOL_TIMEOUT_OPEN_PORT_MILLIS
        } else {
            ORCH_TOOL_TIMEOUT_MILLIS
        });
        servers.insert(ORCH_SERVER.to_owned(), ordinary);
    }
    let has_exec_server =
        spec.exec.executor.is_some() && spec.exec.tool_set.as_deref() != Some(tool_set::CUSTOM);
    if has_exec_server {
        let mut exec = entry(setup.loopback.mcp(key, EXEC_SERVER));
        exec["timeout"] = json!(EXEC_TOOL_TIMEOUT_MILLIS);
        servers.insert(EXEC_SERVER.to_owned(), exec);
    }
    for name in &spec.mcp {
        servers.insert(name.clone(), entry(setup.loopback.mcp(key, name)));
    }
    json!({ "mcpServers": servers })
}

/// The files of each directory from the workspace root to where the session works that this build reads, by its
/// documented loading rule with the settings a session is given: its own instruction files, `AGENTS.md` beside them
/// (the both-files setting), and the rules of `.claude/rules`.
const DIRECTORY_INSTRUCTION_FILES: [&str; 5] = [
    "CLAUDE.md",
    ".claude/CLAUDE.md",
    "CLAUDE.local.md",
    "AGENTS.md",
    ".claude/AGENTS.md",
];

/// How much of the notes' index this build reads when a session starts, as its documentation says: the first 200
/// lines, and no more than 25 KB of them.
const MEMORY_INDEX_LINES: usize = 200;
/// See [`MEMORY_INDEX_LINES`].
const MEMORY_INDEX_BYTES: usize = 25 * 1024;

/// The instruction files a session reads, by this build's documented loading rule, since the harness reports none: the
/// home's own `CLAUDE.md` (named, not located and not read), then those of each directory from the workspace root to
/// where the session works, outermost first, and the notes' index where the harness keeps the notes. Only files that
/// are there are listed; one a session loads later, on demand, is not. Where the session's tools run in an
/// environment elsewhere, its files are not looked at here and nothing of them is claimed.
#[must_use]
pub fn documented_instruction_sources(
    setup: &HarnessSetup,
    spec: &SessionSpec,
) -> Vec<orch_agent_contract::session::InstructionFile> {
    use orch_agent_contract::session::{
        InstructionEvidence, InstructionFile, InstructionFileKind, directory_chain,
    };
    let mut files = Vec::new();
    if let Some(home) = &setup.home
        && home.join("CLAUDE.md").is_file()
    {
        files.push(
            InstructionFile::unmeasured(
                "CLAUDE.md of the harness's home",
                None,
                InstructionEvidence::DocumentedRule,
                "a file of the harness's home: it stays there, unread by Orch",
            )
            .of_home(),
        );
    }
    if through_executor(spec) {
        return files;
    }
    let root = spec.settings.workspace_root.as_str();
    let label = |path: &Path| {
        path.strip_prefix(root).map_or_else(
            |_| path.display().to_string(),
            |relative| relative.display().to_string(),
        )
    };
    for directory in directory_chain(root, &spec.settings.cwd).unwrap_or_default() {
        for name in DIRECTORY_INSTRUCTION_FILES {
            let path = directory.join(name);
            if path.is_file() {
                files.push(InstructionFile::measured(
                    label(&path),
                    &path,
                    InstructionEvidence::DocumentedRule,
                    None,
                ));
            }
        }
        let mut rules: Vec<PathBuf> = std::fs::read_dir(directory.join(".claude/rules"))
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "md") && path.is_file()
            })
            .collect();
        rules.sort();
        for path in rules {
            files.push(InstructionFile::measured(
                label(&path),
                &path,
                InstructionEvidence::DocumentedRule,
                None,
            ));
        }
    }
    if let Some(memory) = spec
        .settings
        .memory
        .as_ref()
        .filter(|memory| memory.kept_by == NoteKeeper::Harness)
    {
        let path =
            Path::new(&memory.directory).join(orch_agent_contract::session::MEMORY_INDEX_FILE);
        if let Ok(body) = std::fs::read(&path) {
            let mut index = InstructionFile::measured(
                label(&path),
                &path,
                InstructionEvidence::DocumentedRule,
                None,
            );
            index.kind = InstructionFileKind::Memory;
            index.line_limit = Some(u64::try_from(MEMORY_INDEX_LINES).unwrap_or(u64::MAX));
            index.byte_limit = Some(u64::try_from(MEMORY_INDEX_BYTES).unwrap_or(u64::MAX));
            // What the index's two bounds leave of it: its first lines, then no more bytes than the second allows.
            let by_lines = body
                .split_inclusive(|byte| *byte == b'\n')
                .take(MEMORY_INDEX_LINES)
                .map(<[u8]>::len)
                .sum::<usize>();
            let loaded = by_lines.min(MEMORY_INDEX_BYTES);
            let loaded_lines = body[..loaded]
                .split_inclusive(|byte| *byte == b'\n')
                .count();
            index.loaded_bytes = Some(u64::try_from(loaded).unwrap_or(u64::MAX));
            index.cut_bytes = Some(u64::try_from(body.len() - loaded).unwrap_or(u64::MAX));
            index.loaded_lines = Some(u64::try_from(loaded_lines).unwrap_or(u64::MAX));
            index.reason = Some(format!(
                "the notes' index is read up to its first {MEMORY_INDEX_LINES} lines and {} KB, as documented",
                MEMORY_INDEX_BYTES / 1024
            ));
            files.push(index);
        }
    }
    files
}

/// The settings that have no flag.
#[must_use]
pub fn settings_json(setup: &HarnessSetup, spec: &SessionSpec, turn: &TurnSettings) -> Json {
    let mut settings = serde_json::Map::new();
    if setup.managed_home {
        settings.extend(managed_settings(keeps_memory(spec)));
    }
    let own_tools = !through_executor(spec);
    if own_tools {
        settings.insert(
            "pluginConfigs".to_owned(),
            json!({ AGENTS_MD_PLUGIN: { "options": { "instructionFiles": INSTRUCTION_FILES_SETTING } } }),
        );
        let mut exclusions = Vec::new();
        for ancestor in std::path::Path::new(&spec.settings.workspace_root)
            .ancestors()
            .skip(1)
        {
            for name in [
                "CLAUDE.md",
                "CLAUDE.local.md",
                "AGENTS.md",
                ".claude/CLAUDE.md",
                ".claude/AGENTS.md",
                ".claude/rules/**",
            ] {
                let source = ancestor.join(name);
                // Native home instructions remain under the owner's control.
                if setup
                    .home
                    .as_ref()
                    .is_none_or(|home| !source.starts_with(home))
                {
                    exclusions.push(source.display().to_string());
                }
            }
        }
        settings.insert("claudeMdExcludes".to_owned(), json!(exclusions));
        if let Some(memory) = spec
            .settings
            .memory
            .as_ref()
            .filter(|memory| memory.kept_by == NoteKeeper::Harness)
        {
            settings.insert("autoMemoryEnabled".to_owned(), json!(true));
            settings.insert("autoMemoryDirectory".to_owned(), json!(memory.directory));
        }
        if let Some(Json::Object(sandbox)) = sandbox_settings(turn.approval_mode, turn.sandbox_mode)
        {
            settings.extend(sandbox);
        }
    }
    // The fast mode is stated every time, off as well as on, so that a preference kept in the home never
    // switches it on.
    settings.insert(
        "fastMode".to_owned(),
        json!(Values::from_json(&turn.params).service_tier.as_deref() == Some(FAST_TIER)),
    );
    if turn.approval_mode == ApprovalMode::Auto {
        settings.insert(
            "availableModels".to_owned(),
            json!(spec.settings.available_models),
        );
    }
    let skip_preflight = match Values::from_json(&turn.params).webfetch_preflight {
        Preflight::On => false,
        Preflight::Off => true,
        Preflight::Auto => matches!(spec.provider, ProviderAccess::Forwarded { .. }),
    };
    if own_tools && skip_preflight {
        settings.insert("skipWebFetchPreflight".to_owned(), json!(true));
    }
    // A plan the harness writes goes into the thread's directory, never into the provider's home that other
    // threads share.
    settings.insert("plansDirectory".to_owned(), json!(PLANS_DIRECTORY));
    Json::Object(settings)
}

/// The same isolation applies to conversations and temporary provider probes.
pub(crate) fn managed_settings(keep_memory: bool) -> serde_json::Map<String, Json> {
    let mut settings = serde_json::Map::new();
    // The vendor documents ten years for managed retention; Orch applies each thread's own idle rule.
    const MANAGED_CLEANUP_DAYS: i64 = 3650;
    settings.insert("cleanupPeriodDays".to_owned(), json!(MANAGED_CLEANUP_DAYS));
    // Only Orch-delivered integrations and explicit turns enter a managed session.
    settings.insert("disableClaudeAiConnectors".to_owned(), json!(true));
    settings.insert("syncClaudeAiSkills".to_owned(), json!(false));
    settings.insert("syncClaudeAiPlugins".to_owned(), json!(false));
    settings.insert("remoteControlAtStartup".to_owned(), json!(false));
    settings.insert("disableRemoteControl".to_owned(), json!(true));
    settings.insert("crossSessionInbound".to_owned(), json!("refuse"));
    settings.insert("autoContinueAtUsageLimit".to_owned(), json!(false));
    settings.insert("idleCompaction".to_owned(), json!(false));
    settings.insert("agentPushNotifEnabled".to_owned(), json!(false));
    settings.insert("disableAgentView".to_owned(), json!(true));
    settings.insert("disableAllHooks".to_owned(), json!(true));
    settings.insert("awaySummaryEnabled".to_owned(), json!(false));
    settings.insert("disableWorkflows".to_owned(), json!(true));
    settings.insert("enableWorkflows".to_owned(), json!(false));
    settings.insert("workflowKeywordTriggerEnabled".to_owned(), json!(false));
    settings.insert("promptSuggestionEnabled".to_owned(), json!(false));
    settings.insert("enableArtifact".to_owned(), json!(false));
    settings.insert("env".to_owned(), json!(managed_environment(keep_memory)));
    settings
}

/// Whether the harness keeps its memory, in the thread's directory: the thread says so, and the harness is
/// in the environment where that directory is.
fn keeps_memory(spec: &SessionSpec) -> bool {
    !through_executor(spec)
        && spec
            .settings
            .memory
            .as_ref()
            .is_some_and(|memory| memory.kept_by == NoteKeeper::Harness)
}

/// Managed sessions perform only issued work; a profile cannot enable unattended activity.
pub(crate) fn managed_environment(keep_memory: bool) -> BTreeMap<String, String> {
    let mut env: BTreeMap<String, String> = crate::process::QUIET_SWITCHES
        .into_iter()
        .map(|(key, value)| (key.to_owned(), value.to_owned()))
        .collect();
    for (key, value) in [
        ("DISABLE_AUTOUPDATER", "1"),
        ("CLAUDE_CODE_DISABLE_TERMINAL_TITLE", "1"),
        ("CLAUDE_CODE_ENABLE_AWAY_SUMMARY", "0"),
        ("CLAUDE_CODE_DISABLE_WORKFLOWS", "1"),
        ("MCP_DISCOVERY_CACHE", "0"),
        ("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS", "1"),
        ("CLAUDE_AUTO_BACKGROUND_TASKS", "0"),
        ("CLAUDE_CODE_DISABLE_MCP_TASK_BACKGROUND", "1"),
        ("CLAUDE_CODE_MCP_AUTO_BACKGROUND_MS", "0"),
        ("CLAUDE_CODE_DISABLE_AGENT_VIEW", "1"),
        ("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "0"),
        ("CLAUDE_CODE_AUTO_CONNECT_IDE", "false"),
        ("CLAUDE_CODE_IDE_SKIP_AUTO_INSTALL", "1"),
        ("CLAUDE_CODE_ENABLE_PROMPT_SUGGESTION", "false"),
        ("ENABLE_CLAUDEAI_MCP_SERVERS", "false"),
        ("CLAUDE_CODE_DISABLE_ARTIFACT", "1"),
        ("CLAUDE_CODE_ARTIFACT_AUTO_OPEN", "0"),
        ("CLAUDE_CODE_ARTIFACT_COMMENTS", "0"),
        ("CLAUDE_CODE_ARTIFACT_COMMENTS_AUTOREACT", "0"),
        ("FORCE_AUTOUPDATE_PLUGINS", "0"),
        ("CLAUDE_CODE_ENABLE_BACKGROUND_PLUGIN_REFRESH", "0"),
        ("CLAUDE_CODE_PACKAGE_MANAGER_AUTO_UPDATE", "0"),
        ("CLAUDE_CODE_ENABLE_TELEMETRY", "0"),
    ] {
        env.insert(key.to_owned(), value.to_owned());
    }
    // The explicit native-note setting enables its own thread directory; absence is enforced by the env switch.
    if !keep_memory {
        env.insert("CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_owned(), "1".to_owned());
    }
    env
}

/// Says where the harness keeps its configuration, and, in a home of Orch's own, that everything else it keeps
/// is in that home too: the transcripts of every session in one directory, and what its libraries keep by the
/// freedesktop rules below it, so that nothing is left in the account's home beside the people's own files.
pub(crate) fn place_home(env: &mut BTreeMap<String, String>, setup: &HarnessSetup) {
    let Some(home) = &setup.home else {
        return;
    };
    env.insert("CLAUDE_CONFIG_DIR".to_owned(), home.display().to_string());
    if setup.managed_home {
        env.insert(
            "CLAUDE_CODE_PROJECT_DIR_NAME".to_owned(),
            MANAGED_PROJECT_DIR.to_owned(),
        );
        for (variable, place) in MANAGED_XDG_PLACES {
            env.insert(
                variable.to_owned(),
                home.join(MANAGED_XDG_DIR).join(place).display().to_string(),
            );
        }
    }
}

/// The environment of a session's process.
#[must_use]
pub fn environment(launch: &Launch<'_>) -> BTreeMap<String, String> {
    let Launch {
        setup, spec, turn, ..
    } = *launch;
    let mut env = base_environment(setup.inherited.clone());
    place_home(&mut env, setup);
    match &spec.provider {
        ProviderAccess::Forwarded { .. } => {
            env.insert(
                "ANTHROPIC_BASE_URL".to_owned(),
                setup.loopback.llm(&spec.session_key),
            );
            env.insert(
                "ANTHROPIC_AUTH_TOKEN".to_owned(),
                setup.loopback.token.expose().to_owned(),
            );
        }
        ProviderAccess::Native { .. }
        | ProviderAccess::Shared { .. }
        | ProviderAccess::Handover { .. } => {
            if let Some(token) = launch.oauth_token {
                env.insert("CLAUDE_CODE_OAUTH_TOKEN".to_owned(), token.to_owned());
            }
        }
    }
    let values = Values::from_json(&turn.params);
    for (name, value) in model_environment(&turn.model, values.small_model.as_deref()) {
        env.insert(name.to_owned(), value);
    }
    if let Some(effort) = effort_for_harness(turn.effort.as_deref()) {
        env.insert("CLAUDE_CODE_EFFORT_LEVEL".to_owned(), effort.to_owned());
    }
    // The harness is told the context, and nothing else: it keeps its own room for the answer. A model whose
    // name it may not know (a gateway's own) is told its window, or the context where that is larger.
    if let Some(context) = auto_compact_window(values.context) {
        env.insert(
            "CLAUDE_CODE_AUTO_COMPACT_WINDOW".to_owned(),
            context.to_string(),
        );
    }
    if let Some(window) = assumed_window(values.window, values.context) {
        env.insert(
            "CLAUDE_CODE_MAX_CONTEXT_TOKENS".to_owned(),
            window.to_string(),
        );
    }
    let tool_search = values
        .tool_search
        .unwrap_or(!matches!(spec.provider, ProviderAccess::Forwarded { .. }));
    env.insert("ENABLE_TOOL_SEARCH".to_owned(), tool_search.to_string());
    if !setup.config.background_tasks {
        env.insert(
            "CLAUDE_CODE_DISABLE_BACKGROUND_TASKS".to_owned(),
            "1".to_owned(),
        );
    }
    if setup.config.todo_tools {
        env.insert("CLAUDE_CODE_ENABLE_TODO_TOOLS".to_owned(), "1".to_owned());
    }
    if !keeps_memory(spec) {
        env.insert("CLAUDE_CODE_DISABLE_AUTO_MEMORY".to_owned(), "1".to_owned());
    }
    env.insert(
        MCP_TOKEN_VARIABLE.to_owned(),
        setup.loopback.token.expose().to_owned(),
    );
    // Personal sessions retain the profile's choices; managed sessions stay supervised.
    env.extend(setup.profile_env.clone());
    if setup.managed_home {
        env.extend(managed_environment(keeps_memory(spec)));
    }
    // This native switch is read at process start and must follow the turn's approval mode.
    if turn.approval_mode == ApprovalMode::Bypass {
        env.insert(BYPASS_ENV.to_owned(), "1".to_owned());
    } else {
        env.remove(BYPASS_ENV);
    }
    // The selected credential home and resolved limits remain authoritative over environment variables.
    place_home(&mut env, setup);
    // A profile cannot raise a place's resolved limit through a process variable.
    env.insert(
        "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS".to_owned(),
        spec.settings.subagent_limits.at_once.to_string(),
    );
    env.insert(
        "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH".to_owned(),
        spec.settings.subagent_limits.depth.to_string(),
    );
    env
}

/// The arguments of a session's process.
#[must_use]
pub fn arguments(launch: &Launch<'_>) -> Vec<String> {
    let Launch {
        setup,
        spec,
        session_id,
        origin,
        turn,
        ..
    } = *launch;
    let mut args: Vec<String> = [
        "-p",
        "--input-format",
        "stream-json",
        "--output-format",
        "stream-json",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
        "--forward-subagent-text",
        "--permission-prompt-tool",
        "stdio",
    ]
    .iter()
    .map(|arg| (*arg).to_owned())
    .collect();
    args.push("--permission-mode".to_owned());
    args.push(
        if turn.plan {
            PLAN_MODE
        } else {
            permission_mode(turn.approval_mode)
        }
        .to_owned(),
    );
    if !setup.as_root {
        args.push("--allow-dangerously-skip-permissions".to_owned());
    }
    args.push("--model".to_owned());
    args.push(model_argument(
        &turn.model,
        Values::from_json(&turn.params).context,
    ));
    match origin {
        Origin::New => {
            args.push("--session-id".to_owned());
            args.push(session_id.to_owned());
        }
        Origin::Resume => {
            args.push("--resume".to_owned());
            args.push(session_id.to_owned());
        }
        Origin::Fork { from } | Origin::ForkPrefix { from, .. } => {
            args.push("--resume".to_owned());
            args.push(from.clone());
            args.push("--fork-session".to_owned());
            args.push("--session-id".to_owned());
            args.push(session_id.to_owned());
            if let Origin::ForkPrefix { through, .. } = origin {
                args.push(format!("--resume-session-at={through}"));
            }
        }
    }
    args.push("--setting-sources".to_owned());
    args.push(setting_sources(&setup.config.setting_sources, spec));
    args.push("--settings".to_owned());
    args.push(settings_json(setup, spec, turn).to_string());
    args.push("--strict-mcp-config".to_owned());
    args.push("--mcp-config".to_owned());
    args.push(mcp_config(setup, spec).to_string());
    // The thread's skills, as a plugin of this session only, from the thread's own directory (which `skills_plugin`
    // has made one); not where its commands run elsewhere, since the skills' files would not be there.
    if let Some(skills) = spec.skills.as_ref().filter(|_| !through_executor(spec)) {
        args.push("--plugin-dir".to_owned());
        args.push(skills.dir.clone());
    }
    if !spec.settings.orch_tools.is_empty() {
        args.push("--allowedTools".to_owned());
        args.push("mcp__orch__*".to_owned());
    }
    if !spec.settings.instructions.trim().is_empty() {
        args.push("--append-system-prompt-file".to_owned());
        args.push(instructions_path(setup, spec).display().to_string());
    }
    args.push("--system-prompt-snapshot".to_owned());
    args.push("off".to_owned());
    args.extend(tool_arguments(through_executor(spec)));
    if !through_executor(spec) {
        let directory = spec.settings.directory.trim_end_matches('/');
        if spec.settings.cwd.trim_end_matches('/') != directory {
            args.push("--add-dir".to_owned());
            args.push(spec.settings.directory.clone());
        }
        for mount in &spec.settings.mounts {
            args.push("--add-dir".to_owned());
            args.push(mount.path.clone());
        }
    }
    args
}

/// Everything a session's process is started with.
#[must_use]
pub fn launch(launch: &Launch<'_>) -> LaunchSpec {
    LaunchSpec {
        program: launch.setup.program.clone(),
        args: arguments(launch),
        env: environment(launch),
        cwd: working_directory(launch.setup, launch.spec),
        label: format!(
            "session-{}",
            &launch.spec.session_key[..8.min(launch.spec.session_key.len())]
        ),
    }
}

/// Where the harness keeps the transcript of a session in a home of Orch's, which is the same place for
/// every session.
#[must_use]
pub fn managed_transcript(home: &Path, session_id: &str) -> PathBuf {
    home.join("projects")
        .join(MANAGED_PROJECT_DIR)
        .join(format!("{session_id}.jsonl"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_agent_contract::session::{
        ExecAccess, MountGrant, SessionMemory, SessionMode, SessionSettings,
    };
    use orch_plugin_contract::Secret;
    use orch_types::new_id;

    const KEY: &str = "0123456789abcdef0123456789abcdef";

    fn setup() -> HarnessSetup {
        HarnessSetup {
            program: "claude".into(),
            home: Some(PathBuf::from("/data/harness/p/a")),
            managed_home: true,
            loopback: Loopback::new(41231, Secret::new("t".repeat(64))),
            profile_env: BTreeMap::new(),
            config: ProfileConfig::default(),
            inherited: BTreeMap::from([("PATH".to_owned(), "/usr/bin".to_owned())]),
            as_root: false,
        }
    }

    fn spec() -> SessionSpec {
        SessionSpec {
            settings: SessionSettings {
                agent_id: new_id(),
                thread_id: new_id(),
                segment_id: new_id(),
                mode: SessionMode::New,
                native_session_ref: None,
                upto_turn_ordinal: None,
                workspace_root: "/workspace/web".into(),
                instruction_root_marker: false,
                directory: "/workspace/web".into(),
                cwd: "/workspace/web/api".into(),
                mounts: vec![MountGrant {
                    name: "src".into(),
                    path: "/data/src".into(),
                    read_only: true,
                }],
                model: "gpt-x".into(),
                effort: Some("high".into()),
                params: json!({}),
                approval_mode: ApprovalMode::Ask,
                sandbox_mode: SandboxMode::Open,
                instructions: "You are working inside Orch.".into(),
                available_models: vec!["gpt-x".to_owned()],
                subagent_limits: Default::default(),
                memory: Some(SessionMemory {
                    directory: "/workspace/web/.orch/memory".into(),
                    kept_by: NoteKeeper::Harness,
                }),
                arrangement: orch_agent_contract::descriptor::ArrangementType::KeySeparated,
                orch_tools: vec![orch_agent_contract::events::OrchTool::Announce],
                orch_tool_schemas: Default::default(),
                raw_events: false,
                side_reading: None,
                workspace_configuration: None,
            },
            session_key: KEY.into(),
            archive: None,
            history: None,
            provider: ProviderAccess::Forwarded {
                provider_id: new_id(),
                protocol: "anthropic-messages".into(),
            },
            exec: ExecAccess {
                executor: None,
                tool_set: None,
                environment_id: new_id(),
                generation: 1,
            },
            mcp: vec!["tmux".into()],
            skills: None,
        }
    }

    #[test]
    fn the_documented_rule_lists_the_home_file_by_name_then_the_chain_outermost_first_and_the_notes_index_with_its_bounds()
     {
        use orch_agent_contract::session::{InstructionEvidence, InstructionFileKind};
        let workspace = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        let root = workspace.path().to_string_lossy().into_owned();
        std::fs::create_dir_all(workspace.path().join("api/.claude/rules")).unwrap();
        std::fs::create_dir_all(workspace.path().join(".orch/memory")).unwrap();
        std::fs::write(home.path().join("CLAUDE.md"), "mine").unwrap();
        std::fs::write(workspace.path().join("AGENTS.md"), "root").unwrap();
        std::fs::write(workspace.path().join("api/CLAUDE.md"), "api").unwrap();
        std::fs::write(workspace.path().join("api/.claude/rules/style.md"), "style").unwrap();
        let index: String = (0..250).map(|n| format!("- note {n}\n")).collect();
        std::fs::write(workspace.path().join(".orch/memory/MEMORY.md"), &index).unwrap();
        let mut setup = setup();
        setup.home = Some(home.path().to_path_buf());
        let mut spec = spec();
        spec.settings.workspace_root = root.clone();
        spec.settings.directory = root.clone();
        spec.settings.cwd = format!("{root}/api");
        spec.settings.memory = Some(SessionMemory {
            directory: format!("{root}/.orch/memory"),
            kept_by: NoteKeeper::Harness,
        });
        let sources = documented_instruction_sources(&setup, &spec);
        let labels: Vec<&str> = sources.iter().map(|source| source.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "CLAUDE.md of the harness's home",
                "AGENTS.md",
                "api/CLAUDE.md",
                "api/.claude/rules/style.md",
                ".orch/memory/MEMORY.md",
            ]
        );
        assert!(
            sources[0].home && sources[0].path.is_none(),
            "a home file has no path"
        );
        assert!(
            sources
                .iter()
                .all(|source| source.evidence == InstructionEvidence::DocumentedRule)
        );
        let notes = sources.last().unwrap();
        assert_eq!(notes.kind, InstructionFileKind::Memory);
        assert_eq!((notes.lines, notes.loaded_lines), (Some(250), Some(200)));
        // Where the tools run in an environment elsewhere, nothing of it is claimed.
        spec.exec.executor = Some("orch-exec".into());
        assert_eq!(documented_instruction_sources(&setup, &spec).len(), 1);
    }

    fn turn() -> TurnSettings {
        TurnSettings {
            model: "gpt-x".into(),
            effort: Some("high".into()),
            approval_mode: ApprovalMode::OnRequest,
            sandbox_mode: SandboxMode::Workspace,
            params: json!({}),
            plan: false,
        }
    }

    #[test]
    fn a_workspaces_own_settings_wait_for_an_answer_and_kept_out_the_session_reads_no_project_source()
     {
        use orch_agent_contract::session::{ConfigTrust, WorkspaceConfiguration};
        let workspace = tempfile::tempdir().unwrap();
        let root = workspace.path().to_string_lossy().into_owned();
        let mut spec = spec();
        spec.settings.workspace_root = root.clone();
        spec.settings.directory = root.clone();
        spec.settings.cwd = root;
        // A `.claude` the harness made for itself, without settings, is no configuration.
        std::fs::create_dir_all(workspace.path().join(".claude")).unwrap();
        validate_instruction_settings(&spec).unwrap();
        std::fs::write(workspace.path().join(".claude/settings.json"), "{}").unwrap();
        let error = validate_instruction_settings(&spec).unwrap_err();
        assert_eq!(error.code(), "workspace_configuration");
        assert_eq!(
            orch_agent_contract::adapter::AdapterError::configuration_found(&error.detail()),
            vec![".claude/settings.json".to_owned()]
        );
        let files =
            orch_agent_contract::adapter::AdapterError::configuration_files(&error.detail());
        spec.settings.workspace_configuration = Some(WorkspaceConfiguration {
            answer: ConfigTrust::KeepOut,
            files: files.clone(),
        });
        validate_instruction_settings(&spec).unwrap();
        assert_eq!(setting_sources("project", &spec), "user");
        assert_eq!(setting_sources("user,project,local", &spec), "user");
        spec.settings.workspace_configuration = Some(WorkspaceConfiguration {
            answer: ConfigTrust::Trust,
            files,
        });
        validate_instruction_settings(&spec).unwrap();
        assert_eq!(setting_sources("project", &spec), "project");
    }

    fn built(
        setup: &HarnessSetup,
        spec: &SessionSpec,
        turn: &TurnSettings,
        origin: &Origin,
    ) -> LaunchSpec {
        launch(&Launch {
            setup,
            spec,
            session_id: "11111111-1111-4111-8111-111111111111",
            origin,
            turn,
            oauth_token: None,
        })
    }

    fn after(args: &[String], flag: &str) -> Option<String> {
        args.iter()
            .position(|arg| arg == flag)
            .and_then(|at| args.get(at + 1))
            .cloned()
    }

    #[test]
    fn a_thread_with_skills_is_given_their_directory_as_a_plugin_of_the_session_where_its_commands_run()
     {
        let (setup, mut spec, turn) = (setup(), spec(), turn());
        assert_eq!(
            after(
                &built(&setup, &spec, &turn, &Origin::New).args,
                "--plugin-dir"
            ),
            None
        );
        spec.skills = Some(orch_agent_contract::session::SessionSkills {
            dir: "/data/skills/threads/t1".into(),
            names: vec!["greet".into()],
        });
        for origin in [Origin::New, Origin::Resume] {
            assert_eq!(
                after(&built(&setup, &spec, &turn, &origin).args, "--plugin-dir").as_deref(),
                Some("/data/skills/threads/t1")
            );
        }
        spec.exec.executor = Some("orch".into());
        assert_eq!(
            after(
                &built(&setup, &spec, &turn, &Origin::New).args,
                "--plugin-dir"
            ),
            None,
            "where commands run elsewhere, the skills' files would not be there"
        );
    }

    #[test]
    fn a_new_session_names_its_id_a_resumed_one_asks_for_it_and_a_fork_names_both() {
        let (setup, spec, turn) = (setup(), spec(), turn());
        let new = built(&setup, &spec, &turn, &Origin::New);
        assert_eq!(
            after(&new.args, "--session-id").as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        assert!(!new.args.contains(&"--resume".to_owned()));
        let resumed = built(&setup, &spec, &turn, &Origin::Resume);
        assert_eq!(
            after(&resumed.args, "--resume").as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
        let fork = built(
            &setup,
            &spec,
            &turn,
            &Origin::Fork {
                from: "22222222-2222-4222-8222-222222222222".into(),
            },
        );
        assert_eq!(
            after(&fork.args, "--resume").as_deref(),
            Some("22222222-2222-4222-8222-222222222222")
        );
        assert!(fork.args.contains(&"--fork-session".to_owned()));
        assert_eq!(
            after(&fork.args, "--session-id").as_deref(),
            Some("11111111-1111-4111-8111-111111111111")
        );
    }

    /// A turn that carries these parameters.
    fn turn_with(params: Json) -> TurnSettings {
        TurnSettings { params, ..turn() }
    }

    fn env_of<'a>(launch: &'a LaunchSpec, name: &str) -> Option<&'a str> {
        launch.env.get(name).map(String::as_str)
    }

    fn fast_mode_of(launch: &LaunchSpec) -> Json {
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        settings["fastMode"].clone()
    }

    #[test]
    fn the_harness_is_told_the_context_and_nothing_else_and_the_suffix_only_above_200000() {
        let (setup, spec) = (setup(), spec());
        let large = built(
            &setup,
            &spec,
            &turn_with(json!({ "context": 1_000_000, "context_window": 200_000 })),
            &Origin::New,
        );
        assert_eq!(
            env_of(&large, "CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
            Some("1000000")
        );
        assert_eq!(
            after(&large.args, "--model").as_deref(),
            Some("gpt-x[1m]"),
            "the larger window is asked for by the name"
        );
        // The auxiliary models are the thread's model, without the suffix: it is the main model that asks.
        assert_eq!(env_of(&large, "ANTHROPIC_MODEL"), Some("gpt-x"));
        // At 200 000 and below, the name is as it is.
        for context in [200_000, 150_000] {
            let small = built(
                &setup,
                &spec,
                &turn_with(json!({ "context": context, "context_window": 200_000 })),
                &Origin::New,
            );
            assert_eq!(
                after(&small.args, "--model").as_deref(),
                Some("gpt-x"),
                "{context}"
            );
            assert_eq!(
                env_of(&small, "CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
                Some(context.to_string().as_str())
            );
        }
        // Nothing chosen and nothing larger open: nothing is told of the conversation's size.
        let plain = built(&setup, &spec, &turn_with(json!({})), &Origin::New);
        assert_eq!(env_of(&plain, "CLAUDE_CODE_AUTO_COMPACT_WINDOW"), None);
        assert_eq!(after(&plain.args, "--model").as_deref(), Some("gpt-x"));
    }

    #[test]
    fn a_model_the_harness_may_not_know_is_told_its_window_or_the_context_where_that_is_larger() {
        let (setup, spec) = (setup(), spec());
        let window_only = built(
            &setup,
            &spec,
            &turn_with(json!({ "context_window": 128_000 })),
            &Origin::New,
        );
        assert_eq!(
            env_of(&window_only, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("128000")
        );
        assert_eq!(
            env_of(&window_only, "CLAUDE_CODE_AUTO_COMPACT_WINDOW"),
            None
        );
        let larger = built(
            &setup,
            &spec,
            &turn_with(json!({ "context": 400_000, "context_window": 128_000 })),
            &Origin::New,
        );
        assert_eq!(
            env_of(&larger, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("400000")
        );
        let smaller = built(
            &setup,
            &spec,
            &turn_with(json!({ "context": 100_000, "context_window": 128_000 })),
            &Origin::New,
        );
        assert_eq!(
            env_of(&smaller, "CLAUDE_CODE_MAX_CONTEXT_TOKENS"),
            Some("128000")
        );
        assert_eq!(
            env_of(
                &built(&setup, &spec, &turn_with(json!({})), &Origin::New),
                "CLAUDE_CODE_MAX_CONTEXT_TOKENS"
            ),
            None
        );
    }

    #[test]
    fn the_fast_mode_is_stated_every_time_on_for_the_speed_fast_and_off_otherwise() {
        let (setup, spec) = (setup(), spec());
        let fast = built(
            &setup,
            &spec,
            &turn_with(json!({ "service_tier": "fast" })),
            &Origin::New,
        );
        assert_eq!(fast_mode_of(&fast), json!(true));
        let ordinary = built(&setup, &spec, &turn_with(json!({})), &Origin::New);
        assert_eq!(
            fast_mode_of(&ordinary),
            json!(false),
            "a preference kept in the home never switches it on"
        );
        let other = built(
            &setup,
            &spec,
            &turn_with(json!({ "service_tier": "flex" })),
            &Origin::New,
        );
        assert_eq!(fast_mode_of(&other), json!(false));
    }

    #[test]
    fn a_plan_turn_starts_in_plan_mode_and_plans_are_kept_in_the_threads_directory() {
        let planning = TurnSettings {
            plan: true,
            ..turn()
        };
        let launch = built(&setup(), &spec(), &planning, &Origin::New);
        assert_eq!(
            after(&launch.args, "--permission-mode").as_deref(),
            Some("plan")
        );
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        assert_eq!(settings["plansDirectory"], PLANS_DIRECTORY);
        // The approval mode is not changed by planning: the next turn is in it again.
        let working = built(&setup(), &spec(), &turn(), &Origin::New);
        assert_eq!(
            after(&working.args, "--permission-mode").as_deref(),
            Some("acceptEdits")
        );
    }

    #[test]
    fn the_model_the_mode_and_the_effort_are_stated_every_time() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        assert_eq!(after(&launch.args, "--model").as_deref(), Some("gpt-x"));
        assert_eq!(
            after(&launch.args, "--permission-mode").as_deref(),
            Some("acceptEdits")
        );
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_EFFORT_LEVEL")
                .map(String::as_str),
            Some("high")
        );
        assert_eq!(
            launch.env.get("ANTHROPIC_MODEL").map(String::as_str),
            Some("gpt-x")
        );
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_SUBAGENT_MODEL_FORCE")
                .map(String::as_str),
            Some("1")
        );
        assert_eq!(
            after(&launch.args, "--permission-prompt-tool").as_deref(),
            Some("stdio")
        );
    }

    #[test]
    fn behind_a_forwarder_the_harness_calls_the_connector_with_its_token_and_never_holds_a_key() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        assert_eq!(
            launch.env.get("ANTHROPIC_BASE_URL").map(String::as_str),
            Some(format!("http://127.0.0.1:41231/llm/{KEY}").as_str())
        );
        assert_eq!(
            launch.env.get("ANTHROPIC_AUTH_TOKEN"),
            Some(&"t".repeat(64))
        );
        assert!(!launch.env.contains_key("ANTHROPIC_API_KEY"));
        assert!(!launch.env.contains_key("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(
            !launch.args.iter().any(|arg| arg.contains(&"t".repeat(64))),
            "the token is never an argument"
        );
    }

    #[test]
    fn on_a_token_of_the_subscription_the_harness_runs_on_it_and_calls_no_forwarder() {
        let (setup, mut spec, turn) = (setup(), spec(), turn());
        spec.provider = ProviderAccess::Shared {
            provider_id: new_id(),
            form: "oauth_token".into(),
        };
        let launch = launch(&Launch {
            setup: &setup,
            spec: &spec,
            session_id: "s",
            origin: &Origin::New,
            turn: &turn,
            oauth_token: Some("sk-ant-oat01-abc"),
        });
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_OAUTH_TOKEN")
                .map(String::as_str),
            Some("sk-ant-oat01-abc")
        );
        assert!(!launch.env.contains_key("ANTHROPIC_BASE_URL"));
        assert!(!launch.args.iter().any(|arg| arg.contains("sk-ant")));
    }

    #[test]
    fn the_harness_is_held_to_the_servers_it_is_given() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        assert!(launch.args.contains(&"--strict-mcp-config".to_owned()));
        let config: Json =
            serde_json::from_str(&after(&launch.args, "--mcp-config").unwrap()).unwrap();
        let servers = config["mcpServers"].as_object().unwrap();
        assert_eq!(
            servers.keys().cloned().collect::<Vec<_>>(),
            ["orch", "tmux"]
        );
        assert_eq!(
            servers["orch"]["url"],
            format!("http://127.0.0.1:41231/mcp/{KEY}/orch")
        );
        assert_eq!(servers["tmux"]["type"], "http");
        assert_eq!(
            servers["orch"]["headers"]["Authorization"],
            "Bearer ${ORCH_MCP_TOKEN}"
        );
        assert_eq!(launch.env.get("ORCH_MCP_TOKEN"), Some(&"t".repeat(64)));
    }

    #[test]
    fn no_orch_server_is_given_when_the_thread_has_no_orch_tools() {
        let (setup, mut spec) = (setup(), spec());
        spec.settings.orch_tools.clear();
        spec.mcp.clear();
        let config = mcp_config(&setup, &spec);
        assert!(config["mcpServers"].as_object().unwrap().is_empty());
    }

    #[test]
    fn the_memory_of_the_harness_is_pointed_at_the_thread_and_agents_md_is_read_beside_claude_md() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        assert_eq!(settings["autoMemoryEnabled"], true);
        assert_eq!(
            settings["autoMemoryDirectory"],
            "/workspace/web/.orch/memory"
        );
        assert_eq!(
            settings["pluginConfigs"]["cc-plugin-agents-md@builtin"]["options"]["instructionFiles"],
            "claude-md-and-agents-md"
        );
        assert!(!launch.env.contains_key("CLAUDE_CODE_DISABLE_AUTO_MEMORY"));
        assert_eq!(
            after(&launch.args, "--setting-sources").as_deref(),
            Some("project")
        );
    }

    #[test]
    fn without_a_memory_to_keep_the_harness_is_told_it_has_none() {
        let (setup, mut spec, turn) = (setup(), spec(), turn());
        spec.settings.memory = None;
        let off = built(&setup, &spec, &turn, &Origin::New);
        assert_eq!(
            off.env
                .get("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
                .map(String::as_str),
            Some("1")
        );
        let settings: Json =
            serde_json::from_str(&after(&off.args, "--settings").unwrap()).unwrap();
        assert!(settings.get("autoMemoryEnabled").is_none());
        spec.settings.memory = Some(SessionMemory {
            directory: "/w/.orch/memory".into(),
            kept_by: NoteKeeper::Agent,
        });
        let by_agent = built(&setup, &spec, &turn, &Origin::New);
        assert_eq!(
            by_agent
                .env
                .get("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn the_shell_sandbox_is_asked_for_when_the_mode_says_workspace() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        assert_eq!(settings["sandbox"]["enabled"], true);
        assert_eq!(settings["sandbox"]["autoAllowBashIfSandboxed"], true);
    }

    #[test]
    fn the_directories_the_harness_may_reach_are_the_thread_and_its_mounts() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        let dirs: Vec<_> = launch
            .args
            .windows(2)
            .filter(|pair| pair[0] == "--add-dir")
            .map(|pair| pair[1].clone())
            .collect();
        assert_eq!(dirs, ["/workspace/web", "/data/src"]);
        assert_eq!(launch.cwd, PathBuf::from("/workspace/web/api"));
    }

    #[test]
    fn with_its_provider_the_harness_has_only_the_tools_that_touch_no_machine_and_the_executors_server()
     {
        let (setup, mut spec, turn) = (setup(), spec(), turn());
        spec.exec.executor = Some("orch-exec".into());
        spec.exec.tool_set = Some("claude-code".into());
        spec.settings.memory = None;
        let launch = built(&setup, &spec, &turn, &Origin::New);
        assert_eq!(
            after(&launch.args, "--tools")
                .as_deref()
                .map(|tools| tools.contains("Bash")),
            Some(false)
        );
        let config: Json =
            serde_json::from_str(&after(&launch.args, "--mcp-config").unwrap()).unwrap();
        assert_eq!(
            config["mcpServers"]["exec"]["url"],
            format!("http://127.0.0.1:41231/mcp/{KEY}/exec")
        );
        assert_eq!(
            config["mcpServers"]["exec"]["timeout"],
            json!(EXEC_TOOL_TIMEOUT_MILLIS),
            "the executor's longest command is not cut short by the harness's default"
        );
        assert!(
            launch.cwd.starts_with("/data/harness/p/a/sessions"),
            "an empty directory of its own: {:?}",
            launch.cwd
        );
        assert!(!launch.args.contains(&"--add-dir".to_owned()));
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        assert!(
            settings.get("pluginConfigs").is_none(),
            "it is not in the directory to read a file there"
        );
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_DISABLE_AUTO_MEMORY")
                .map(String::as_str),
            Some("1")
        );
    }

    #[test]
    fn the_set_custom_has_no_server_of_the_executors_and_the_environments_servers_alone() {
        let (setup, mut spec) = (setup(), spec());
        spec.exec.executor = Some("orch-exec".into());
        spec.exec.tool_set = Some("custom".into());
        let config = mcp_config(&setup, &spec);
        assert!(config["mcpServers"].get("exec").is_none());
        assert!(config["mcpServers"].get("tmux").is_some());
    }

    #[test]
    fn nothing_that_starts_a_turn_by_itself_is_left_on() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS")
                .map(String::as_str),
            Some("1")
        );
        assert!(launch.args.contains(&"--disallowed-tools".to_owned()));
        let mut on = setup();
        on.config.background_tasks = true;
        assert_eq!(
            built(&on, &spec(), &turn(), &Origin::New).env["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"],
            "1"
        );
        // A person's own profile keeps its previous explicit opt-in.
        on.managed_home = false;
        assert!(
            !built(&on, &spec(), &turn(), &Origin::New)
                .env
                .contains_key("CLAUDE_CODE_DISABLE_BACKGROUND_TASKS")
        );
    }

    #[test]
    fn the_profile_has_the_last_word_on_the_environment() {
        let mut setup = setup();
        setup.managed_home = false;
        setup
            .profile_env
            .insert("DISABLE_TELEMETRY".into(), "0".into());
        setup
            .profile_env
            .insert("CLAUDE_CODE_MAX_RETRIES".into(), "2".into());
        let launch = built(&setup, &spec(), &turn(), &Origin::New);
        assert_eq!(
            launch.env.get("DISABLE_TELEMETRY").map(String::as_str),
            Some("0")
        );
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_MAX_RETRIES")
                .map(String::as_str),
            Some("2")
        );
    }

    #[test]
    fn managed_sessions_keep_updates_account_sync_and_unattended_work_off_despite_profile_overrides()
     {
        let mut setup = setup();
        setup
            .profile_env
            .insert("DISABLE_UPDATES".into(), "0".into());
        setup
            .profile_env
            .insert("DISABLE_TELEMETRY".into(), "0".into());
        setup
            .profile_env
            .insert("CLAUDE_AUTO_BACKGROUND_TASKS".into(), "1".into());
        setup
            .profile_env
            .insert("CLAUDE_CODE_DISABLE_AUTO_MEMORY".into(), "0".into());
        let mut no_notes = spec();
        no_notes.settings.memory = None;
        let launch = built(&setup, &no_notes, &turn(), &Origin::New);
        assert_eq!(launch.env["DISABLE_UPDATES"], "1");
        assert_eq!(launch.env["DISABLE_TELEMETRY"], "1");
        assert_eq!(launch.env["CLAUDE_AUTO_BACKGROUND_TASKS"], "0");
        assert_eq!(launch.env["CLAUDE_CODE_DISABLE_TERMINAL_TITLE"], "1");
        assert_eq!(launch.env["MCP_DISCOVERY_CACHE"], "0");
        assert_eq!(launch.env["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1");
        let settings: Json =
            serde_json::from_str(&after(&launch.args, "--settings").unwrap()).unwrap();
        assert_eq!(settings["disableClaudeAiConnectors"], true);
        assert_eq!(settings["syncClaudeAiSkills"], false);
        assert_eq!(settings["syncClaudeAiPlugins"], false);
        assert_eq!(settings["remoteControlAtStartup"], false);
        assert_eq!(settings["crossSessionInbound"], "refuse");
        assert_eq!(settings["cleanupPeriodDays"], 3650);
        assert!(settings.get("autoMemoryEnabled").is_none());
        assert_eq!(settings["env"]["CLAUDE_CODE_DISABLE_AUTO_MEMORY"], "1");
        assert_eq!(settings["env"]["DISABLE_UPDATES"], "1");
        setup.managed_home = false;
        let personal = settings_json(&setup, &spec(), &turn());
        assert!(personal.get("disableClaudeAiConnectors").is_none());
        assert!(personal.get("syncClaudeAiSkills").is_none());
        assert!(personal.get("syncClaudeAiPlugins").is_none());
        assert!(personal.get("remoteControlAtStartup").is_none());
        assert!(personal.get("crossSessionInbound").is_none());
        assert!(personal.get("env").is_none());
    }

    #[test]
    fn subagent_limits_are_always_sent_and_a_profile_cannot_raise_the_places_limit() {
        let mut harness = setup();
        let mut session = spec();
        harness.profile_env.insert(
            "CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS".to_owned(),
            "64".to_owned(),
        );
        harness.profile_env.insert(
            "CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH".to_owned(),
            "5".to_owned(),
        );
        for (at_once, depth) in [(8, 1), (3, 2), (64, 5)] {
            session.settings.subagent_limits =
                orch_agent_contract::session::SubagentLimits { at_once, depth };
            let process = built(&harness, &session, &turn(), &Origin::New);
            assert_eq!(
                process.env.get("CLAUDE_CODE_MAX_CONCURRENT_SUBAGENTS"),
                Some(&at_once.to_string())
            );
            assert_eq!(
                process.env.get("CLAUDE_CODE_MAX_SUBAGENT_SPAWN_DEPTH"),
                Some(&depth.to_string())
            );
        }
    }

    #[test]
    fn the_selected_harness_home_is_authoritative_over_profile_or_runtime_variables() {
        let mut harness = setup();
        harness.home = Some("/resolved-credential-home".into());
        harness.profile_env.insert(
            "CLAUDE_CONFIG_DIR".to_owned(),
            "/someone-elses-home".to_owned(),
        );
        let launch = built(&harness, &spec(), &turn(), &Origin::New);
        assert_eq!(
            launch.env["CLAUDE_CONFIG_DIR"],
            harness.home.as_ref().unwrap().display().to_string()
        );
    }

    #[test]
    fn as_the_administrator_the_harness_is_not_allowed_to_bypass_permissions() {
        let mut setup = setup();
        setup.as_root = true;
        let launch = built(&setup, &spec(), &turn(), &Origin::New);
        assert!(
            !launch
                .args
                .contains(&"--allow-dangerously-skip-permissions".to_owned())
        );
    }

    #[test]
    fn a_home_of_orchs_holds_what_the_harness_keeps_and_a_persons_own_is_left_as_it_is() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        for (variable, place) in [
            ("XDG_CACHE_HOME", "cache"),
            ("XDG_CONFIG_HOME", "config"),
            ("XDG_DATA_HOME", "data"),
            ("XDG_STATE_HOME", "state"),
        ] {
            assert_eq!(
                launch.env.get(variable).map(String::as_str),
                Some(format!("/data/harness/p/a/xdg/{place}").as_str()),
                "{variable}: a cache or a log of the libraries the harness is made of is in its home"
            );
        }
        let mut person = setup();
        person.home = None;
        person.managed_home = false;
        let launch = built(&person, &spec(), &turn(), &Origin::New);
        assert!(
            !launch.env.keys().any(|name| name.starts_with("XDG_")),
            "the harness of a person keeps its caches where it always has"
        );
        // A home the person named for the profile is theirs: the harness is told where its configuration
        // is and nothing more.
        let mut named = setup();
        named.managed_home = false;
        let launch = built(&named, &spec(), &turn(), &Origin::New);
        assert!(launch.env.contains_key("CLAUDE_CONFIG_DIR"));
        assert!(!launch.env.keys().any(|name| name.starts_with("XDG_")));
    }

    #[test]
    fn only_orch_managed_homes_defer_the_vendors_session_sweep() {
        let mut setup = setup();
        assert_eq!(
            settings_json(&setup, &spec(), &turn())["cleanupPeriodDays"],
            3650
        );
        setup.managed_home = false;
        assert!(
            settings_json(&setup, &spec(), &turn())
                .get("cleanupPeriodDays")
                .is_none()
        );
    }

    #[test]
    fn a_home_of_orchs_keeps_every_transcript_in_one_directory() {
        let launch = built(&setup(), &spec(), &turn(), &Origin::New);
        assert_eq!(
            launch.env.get("CLAUDE_CONFIG_DIR").map(String::as_str),
            Some("/data/harness/p/a")
        );
        assert_eq!(
            launch
                .env
                .get("CLAUDE_CODE_PROJECT_DIR_NAME")
                .map(String::as_str),
            Some("orch")
        );
        assert_eq!(
            managed_transcript(Path::new("/data/harness/p/a"), "abc"),
            PathBuf::from("/data/harness/p/a/projects/orch/abc.jsonl")
        );
        let mut person = setup();
        person.home = None;
        person.managed_home = false;
        let launch = built(&person, &spec(), &turn(), &Origin::New);
        assert!(
            !launch.env.contains_key("CLAUDE_CONFIG_DIR")
                && !launch.env.contains_key("CLAUDE_CODE_PROJECT_DIR_NAME")
        );
    }

    #[test]
    fn the_page_check_is_left_on_for_the_vendors_own_upstream_and_off_behind_another_endpoint() {
        let (setup, spec, mut turn) = (setup(), spec(), turn());
        let settings = |turn: &TurnSettings, spec: &SessionSpec| settings_json(&setup, spec, turn);
        assert_eq!(
            settings(&turn, &spec)["skipWebFetchPreflight"],
            true,
            "forwarded: another vendor's upstream"
        );
        turn.params = json!({ "webfetch_preflight": "on" });
        assert!(
            settings(&turn, &spec)
                .get("skipWebFetchPreflight")
                .is_none()
        );
        let mut own = self::spec();
        own.provider = ProviderAccess::Native {
            provider_id: new_id(),
        };
        turn.params = json!({});
        assert!(settings(&turn, &own).get("skipWebFetchPreflight").is_none());
    }
}
