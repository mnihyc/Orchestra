//! What a program's manifest tells the generic adapter: how to launch it, how its words map to Orch's, what it declares.
//!
//! An agent program that speaks the Agent Client Protocol needs no Rust code of its own: everything the adapter
//! cannot learn from the protocol is data in an [`AcpProgram`], carried in the program's manifest. The data is
//! read once when the adapter is made. It says how the program is started (its arguments, its variables, the files
//! Orch writes into the program's home before it starts, and in which directory it starts), which approval modes
//! it has and what each comes to in the program's own words, how its tools are named, and which companion plugin,
//! if any, runs inside it and what that plugin may announce.
//!
//! **Templates.** Text in the launch data (an argument, a variable, a file) may hold `{name}` or `{name:json}`;
//! the second writes the value as a JSON string, which is also a YAML string, so that a value with a colon or a
//! quote cannot change the meaning of a file. The names are the closed list [`Variables::NAMES`], with `{file:<name>}`
//! (the path of a file written for the session) and `{approval.<name>}` (a word of the session's approval mode); a name that is
//! not on it, or a value that is missing for the session, is an error and the program is not started: a launch
//! that silently drops a setting could drop a privacy switch.
//!
//! **Nothing leaves by accident.** Every variable the program finds is either set explicitly here, or it is not
//! passed at all unless the manifest names it in `inherit`: the adapter starts the program with an empty
//! environment beside what the manifest lists, so that a person's shell, a repository's configuration or the
//! connector's own variables cannot switch a setting back.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use orch_agent_contract::descriptor::{ApprovalMode, ArrangementType, SandboxMode};
use serde::{Deserialize, Serialize};
use serde_json::Value as Json;

/// Whether one process of the program serves one session, or many.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProcessModel {
    /// A process for each session of Orch, started when the session is opened, with a home of its own. The
    /// program is then told, by its variables, which session's model address and credential it uses.
    #[default]
    PerSession,
    /// One process for every session of the agent profile: the program multiplexes sessions itself.
    Shared,
}

/// Where the program's home lies, which is also where its own record of a session is kept.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeScope {
    /// One home for each thread segment: a session resumed later finds what the program recorded of it.
    #[default]
    Segment,
    /// One home for the whole agent profile.
    Profile,
}

/// A file Orch writes before the program starts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchFile {
    /// Its name, which templates cite as `{file:<name>}` for the absolute path; also its path below the program's
    /// home. It never leaves the home: a path that goes up or is absolute is refused.
    pub path: String,
    /// What it holds, as a template.
    pub template: String,
    /// Whether it is written again at every launch (the default), or only when it is not there.
    #[serde(default = "yes")]
    pub rewrite: bool,
    /// The name of a variable that has to have a value for the file to be written (`plugin`: the file of a
    /// companion plugin that is there); the file is left out otherwise.
    #[serde(default)]
    pub requires: Option<String>,
}

fn yes() -> bool {
    true
}

/// What an approval mode comes to in the program's words.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApprovalWords {
    /// The value `{permission_mode}` takes for it.
    #[serde(default)]
    pub permission_mode: String,
    /// Further words for templates, cited as `{approval.<name>}`.
    #[serde(default)]
    pub values: BTreeMap<String, String>,
    /// Variables added for it, as templates, over the common ones.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}

/// How the program's own words for modes map to Orch's plan mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanModes {
    /// The id of the session mode in which the program plans and changes nothing.
    pub plan: String,
    /// The id of the mode a session returns to after a plan; the mode it opened in when left out.
    #[serde(default)]
    pub default: Option<String>,
}

/// How a tool of the program is understood: by its name, or by the kind the protocol says.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolRule {
    /// The program's names for the tool, compared without case and ignoring the prefix an MCP server's name gives.
    pub names: Vec<String>,
    /// What it is.
    pub family: ToolFamily,
    /// The member of its input that holds the command, the path, the pattern, the query or the address.
    #[serde(default)]
    pub argument: Option<String>,
}

/// The kinds of thing an agent's tool call is understood as.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolFamily {
    /// A command of a shell.
    Command,
    /// A read of a file.
    Read,
    /// A new file, whole.
    Write,
    /// A change of part of a file.
    Edit,
    /// A search in the files' content.
    Grep,
    /// A search of the files' names.
    Glob,
    /// A search of the web.
    WebSearch,
    /// A fetch of a page.
    WebFetch,
    /// Work handed to a sub-agent.
    Subagent,
    /// A list of things to do, or a note: nothing to run.
    Todo,
}

/// What a companion plugin of the program may announce, and how long the adapter waits to hear it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Companion {
    /// The name the plugin's file is given in a session's home. The adapter carries the file for the kinds it
    /// ships a plugin for and writes it at every start; a program the adapter carries no plugin for has none.
    #[serde(default)]
    pub file: Option<String>,
    /// Arguments added to the program's own, as templates, only where the plugin's file is there; `{plugin}` is
    /// its absolute path.
    #[serde(default)]
    pub args: Vec<String>,
    /// Arguments added after those, as templates, only where the plugin's file is there and the session's arrangement
    /// is `separated` (`{file:<name>}` of a file whose `requires` is `separated`).
    #[serde(default)]
    pub separated_args: Vec<String>,
    /// How long the adapter waits for the plugin's hello after the program answered `initialize`, in
    /// milliseconds. A plugin that stays silent past it is reported missing and its extras stay off.
    #[serde(default = "default_hello_wait_ms")]
    pub hello_wait_ms: u64,
    /// The largest neutral history, in bytes, the plugin is asked to write into a session in one go. A longer one is
    /// refused, with the way out named, so that the plugin never reads more than this into memory (a default).
    #[serde(default = "default_max_import_bytes")]
    pub max_import_bytes: u64,
    /// The extras of [`crate::extensions::Extra`], by name, the adapter accepts from it. An extra not listed is
    /// left off even when the plugin says it works.
    #[serde(default)]
    pub extras: Vec<String>,
}

fn default_hello_wait_ms() -> u64 {
    5_000
}

fn default_max_import_bytes() -> u64 {
    64 * 1024 * 1024
}

/// How the program is told which model, and which API, its session uses: the words of the program for the wire
/// protocols Orch's forwarder speaks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtocolWords {
    /// For each protocol name of Orch (`openai-chat`, `openai-responses`, `anthropic-messages`), the program's name
    /// for it, which is what `{api}` takes. A protocol that is not listed is one the program does not speak to the
    /// forwarder.
    #[serde(default)]
    pub api: BTreeMap<String, String>,
    /// For each protocol, the path the base address ends in that the program is given (`/v1` for the Responses
    /// format, none for the Chat format), which is what `{llm_url}` ends with.
    #[serde(default)]
    pub base_suffix: BTreeMap<String, String>,
    /// The name the program's own configuration gives the one provider Orch's forwarder is to it (`orch` when none
    /// is given); a plugin that writes history for a model names it.
    #[serde(default)]
    pub provider_id: String,
    /// The protocols in the order the program is best driven by them; those not listed follow, by name.
    #[serde(default)]
    pub order: Vec<String>,
}

/// The launch data of one agent program that speaks ACP.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcpProgram {
    /// The program's kind, which the descriptor and every event carry.
    pub kind: String,
    /// What the program is called wherever it is shown.
    pub title: String,
    /// The versions of the program the adapter has been shown to drive, as a semver range.
    pub supported_range: String,
    /// The names the program's own file goes by, in the order they are looked for; the kind when none is given.
    #[serde(default)]
    pub programs: Vec<String>,
    /// One process for each session, or one for all.
    #[serde(default)]
    pub process: ProcessModel,
    /// How far a home reaches.
    #[serde(default)]
    pub home_scope: HomeScope,
    /// The arguments after the program, as templates.
    #[serde(default)]
    pub args: Vec<String>,
    /// The variables the program finds, as templates; nothing else is passed except what `inherit` names.
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    /// The names of variables copied from the connector's own environment: `PATH` and `LANG` and the like.
    #[serde(default = "default_inherit")]
    pub inherit: Vec<String>,
    /// The variable that names the program's home; the home is also `{home}`.
    #[serde(default)]
    pub home_variable: Option<String>,
    /// The arguments that make the program print its version (`--version`), and nothing more.
    #[serde(default = "default_version_args")]
    pub version_args: Vec<String>,
    /// A patch merged over the capabilities the adapter derives, for what only the program's authors know:
    /// `{ "telemetry_off": true, "mcp": { "strict": true } }`. `null` removes a member.
    #[serde(default)]
    pub capabilities: Option<Json>,
    /// Whether the program uses the client's own file and terminal methods, which the adapter then carries out in
    /// the environment through Orch's executor: the arrangement `separated` is real for it.
    #[serde(default)]
    pub client_methods: bool,
    /// Whether the program asks questions by `elicitation/create`.
    #[serde(default)]
    pub elicitation: bool,
    /// The text before the number in the line some programs end a command's output with (`[exit code:`), where
    /// the protocol carries no exit code.
    #[serde(default)]
    pub command_exit_marker: Option<String>,
    /// Files written before the program starts.
    #[serde(default)]
    pub files: Vec<LaunchFile>,
    /// Directories made before the program starts, as templates (a cache the program writes to), each readable by
    /// this user alone.
    #[serde(default)]
    pub directories: Vec<String>,
    /// What each approval mode the program has comes to in its words.
    #[serde(default)]
    pub approvals: BTreeMap<String, ApprovalWords>,
    /// The sandbox modes the program's own sandbox can enforce; `["open"]` where it has none to rely on.
    #[serde(default = "default_sandbox_modes")]
    pub sandbox_modes: Vec<SandboxMode>,
    /// The arrangements the program can be placed in, in its order of preference: `key_separated`, `together`,
    /// and `separated` only where the program honours the client's file and terminal methods or its companion
    /// plugin's backends.
    #[serde(default)]
    pub arrangements: Vec<ArrangementType>,
    /// The protocols of Orch the program speaks to the forwarder.
    #[serde(default)]
    pub protocols: ProtocolWords,
    /// The id of the session config option that holds the model, where the program does not mark it by the
    /// category `model`.
    #[serde(default = "default_model_option")]
    pub model_option: String,
    /// The id of the session config option that holds the reasoning effort, where the program does not mark it by
    /// the category `thought_level`.
    #[serde(default = "default_effort_option")]
    pub effort_option: String,
    /// How the program's modes map to Orch's plan mode, where it has a plan mode of its own.
    #[serde(default)]
    pub plan_modes: Option<PlanModes>,
    /// How its tools are understood.
    #[serde(default)]
    pub tools: Vec<ToolRule>,
    /// The instruction files the program reads in a directory, in its order.
    #[serde(default)]
    pub instruction_files: Vec<String>,
    /// How skills reach it: the directory of a session's skills is passed as `{skills_dir}`.
    #[serde(default)]
    pub skills: SkillsMode,
    /// Whether the program tells, with `usage_update`, how full its context is, without a plugin. A program that
    /// does not say so is declared to report nothing of its context until its plugin announces `usage`: a
    /// claim is made only of what has been seen.
    #[serde(default)]
    pub reports_context: bool,
    /// What the program is called in the languages other than English, in the shape a descriptor's `text` has it
    /// (language, then place, then words). It comes from the program's manifest.
    #[serde(default)]
    pub text: std::collections::BTreeMap<String, std::collections::BTreeMap<String, String>>,
    /// The companion plugin, where there is one.
    #[serde(default)]
    pub companion: Option<Companion>,
    /// What the program is stopped with: the longest it is given to answer a cancellation, in milliseconds,
    /// before its process is ended. Every number in the adapter is a default; this one is the program's.
    #[serde(default = "default_cancel_grace_ms")]
    pub cancel_grace_ms: u64,
    /// Whether the program takes images in a prompt without saying so in `initialize`, which is how a program
    /// that decides per session announces nothing until its session exists.
    #[serde(default)]
    pub images: Option<bool>,
    /// What the program adds to its model requests that must not leave the machine.
    #[serde(default)]
    pub privacy: RequestPrivacy,
}

/// What the model forwarder holds a program to ([`RequestPrivacy`] of its manifest): the headers it drops and the
/// members of a body it refuses, whichever program sends them.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPrivacy {
    /// Names of request headers (or prefixes ending in `*`) that name the client or the installation.
    #[serde(default)]
    pub strip_headers: Vec<String>,
    /// Members of a request body's top level that carry a record of the session to the vendor.
    #[serde(default)]
    pub refuse_members: Vec<String>,
}

/// How a program is given the skills of a session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillsMode {
    /// The program has no skills, or this adapter cannot pass them.
    #[default]
    None,
    /// The program reads `skills/<name>/SKILL.md` below a directory it is told of (`{skills_dir}`).
    Directory,
}

fn default_version_args() -> Vec<String> {
    vec!["--version".to_owned()]
}

fn default_inherit() -> Vec<String> {
    [
        "PATH",
        "LANG",
        "LC_ALL",
        "TZ",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ]
    .iter()
    .map(|name| (*name).to_owned())
    .collect()
}

fn default_sandbox_modes() -> Vec<SandboxMode> {
    vec![SandboxMode::Open]
}

fn default_model_option() -> String {
    "model".to_owned()
}

fn default_effort_option() -> String {
    "reasoning_effort".to_owned()
}

fn default_cancel_grace_ms() -> u64 {
    10_000
}

/// What is wrong with a program's launch data, or with the values given to it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProgramError {
    /// The manifest's data is not of the shape the adapter reads.
    #[error("the launch data of the program is not readable: {0}")]
    Unreadable(String),
    /// A template names a variable the adapter does not know.
    #[error("the template {0:?} names the unknown variable {1:?}")]
    UnknownVariable(String, String),
    /// A template needs a value that this session has none of.
    #[error("the template {0:?} needs {1:?}, which this session has no value for")]
    MissingValue(String, String),
    /// A template is not closed.
    #[error("the template {0:?} has a brace that is never closed")]
    Unclosed(String),
    /// A file of the home would be written outside it.
    #[error("the file {0:?} is outside the program's home")]
    OutsideHome(String),
    /// The program does not speak the protocol the session's model is reached by.
    #[error("the program does not speak {0} to the forwarder")]
    ProtocolNotSpoken(String),
}

impl AcpProgram {
    /// Reads launch data as the manifest carries it.
    pub fn from_json(data: &Json) -> Result<Self, ProgramError> {
        serde_json::from_value(data.clone())
            .map_err(|error| ProgramError::Unreadable(error.to_string()))
    }

    /// The words of an approval mode, or the empty ones where the program has none for it.
    #[must_use]
    pub fn approval(&self, mode: ApprovalMode) -> ApprovalWords {
        let key = match mode {
            ApprovalMode::Ask => "ask",
            ApprovalMode::OnRequest => "on_request",
            ApprovalMode::Auto => "auto",
            ApprovalMode::Bypass => "bypass",
        };
        self.approvals.get(key).cloned().unwrap_or_default()
    }

    /// The approval modes the program has, in the order Orch lists them.
    #[must_use]
    pub fn approval_modes(&self) -> Vec<ApprovalMode> {
        [
            ApprovalMode::Ask,
            ApprovalMode::OnRequest,
            ApprovalMode::Auto,
            ApprovalMode::Bypass,
        ]
        .into_iter()
        .filter(|mode| {
            let key = match mode {
                ApprovalMode::Ask => "ask",
                ApprovalMode::OnRequest => "on_request",
                ApprovalMode::Auto => "auto",
                ApprovalMode::Bypass => "bypass",
            };
            self.approvals.contains_key(key)
        })
        .collect()
    }

    /// The protocols of the forwarder the program speaks, in the order it is best driven by them.
    #[must_use]
    pub fn protocol_order(&self) -> Vec<String> {
        let mut order: Vec<String> = self
            .protocols
            .order
            .iter()
            .filter(|protocol| self.protocols.api.contains_key(protocol.as_str()))
            .cloned()
            .collect();
        for protocol in self.protocols.api.keys() {
            if !order.contains(protocol) {
                order.push(protocol.clone());
            }
        }
        order
    }

    /// The name of the one provider the program's configuration holds for Orch's forwarder.
    #[must_use]
    pub fn provider_id(&self) -> &str {
        if self.protocols.provider_id.is_empty() {
            "orch"
        } else {
            &self.protocols.provider_id
        }
    }

    /// The program's name for a protocol of Orch, if it speaks it to the forwarder.
    #[must_use]
    pub fn api_of(&self, protocol: &str) -> Option<&str> {
        self.protocols.api.get(protocol).map(String::as_str)
    }

    /// Everything that starts the program for one session: arguments, variables, files, the directories.
    pub fn plan(&self, variables: &Variables) -> Result<LaunchPlan, ProgramError> {
        let mut variables = variables.clone();
        let written: Vec<&LaunchFile> = self
            .files
            .iter()
            .filter(|file| {
                file.requires
                    .as_deref()
                    .is_none_or(|name| variables.values.contains_key(name))
            })
            .collect();
        for file in &written {
            let path = variables.home().join(safe_relative(&file.path)?);
            variables.file(&file.path, &path);
        }
        let variables = &variables;
        let mut files = Vec::new();
        for file in written {
            let relative = safe_relative(&file.path)?;
            files.push(PlannedFile {
                path: variables.home().join(relative),
                content: render(&file.template, variables)?.into_bytes(),
                rewrite: file.rewrite,
            });
        }
        let mut args = Vec::new();
        for arg in &self.args {
            args.push(render(arg, variables)?);
        }
        if let Some(companion) = &self.companion
            && variables.values.contains_key("plugin")
        {
            for arg in &companion.args {
                args.push(render(arg, variables)?);
            }
            if variables.values.contains_key("separated") {
                for arg in &companion.separated_args {
                    args.push(render(arg, variables)?);
                }
            }
        }
        let mut env = BTreeMap::new();
        for name in &self.inherit {
            if let Some(value) = variables.inherited(name) {
                env.insert(name.clone(), value);
            }
        }
        if let Some(name) = &self.home_variable {
            env.insert(
                name.clone(),
                variables.home().to_string_lossy().into_owned(),
            );
        }
        for (name, template) in &self.env {
            env.insert(name.clone(), render(template, variables)?);
        }
        for (name, template) in &variables.approval.env {
            env.insert(name.clone(), render(template, variables)?);
        }
        let mut directories = Vec::new();
        for directory in &self.directories {
            directories.push(PathBuf::from(render(directory, variables)?));
        }
        Ok(LaunchPlan {
            args,
            env,
            files,
            directories,
        })
    }
}

/// A file the plan writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedFile {
    /// Where, absolute.
    pub path: PathBuf,
    /// What.
    pub content: Vec<u8>,
    /// Whether an existing file is replaced.
    pub rewrite: bool,
}

/// What one launch is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchPlan {
    /// The arguments after the program.
    pub args: Vec<String>,
    /// The variables, all of them.
    pub env: BTreeMap<String, String>,
    /// The files to write first.
    pub files: Vec<PlannedFile>,
    /// The directories to make first.
    pub directories: Vec<PathBuf>,
}

/// A relative path that stays inside the directory it is joined to.
fn safe_relative(path: &str) -> Result<PathBuf, ProgramError> {
    let candidate = Path::new(path);
    let clean = !path.is_empty()
        && candidate
            .components()
            .all(|part| matches!(part, Component::Normal(_)));
    if clean {
        Ok(candidate.to_path_buf())
    } else {
        Err(ProgramError::OutsideHome(path.to_owned()))
    }
}

/// The values a template may cite, for one session of one program.
#[derive(Debug, Clone, Default)]
pub struct Variables {
    values: BTreeMap<&'static str, String>,
    files: BTreeMap<String, String>,
    inherited: BTreeMap<String, String>,
    /// The models the session may use, for `{#each models}`.
    pub models: Vec<String>,
    home: PathBuf,
    /// What the session's approval mode comes to.
    pub approval: ApprovalWords,
}

impl Variables {
    /// The names a template may use, and what each is.
    pub const NAMES: [(&'static str, &'static str); 26] = [
        ("home", "the program's home directory, absolute"),
        (
            "profile_home",
            "the directory every session of the agent profile shares",
        ),
        (
            "agents_home",
            "a directory of the home for the program's shared agent files",
        ),
        ("neutral_dir", "an empty directory the program starts in"),
        ("cwd", "where the session works"),
        ("workspace", "the environment's workspace root"),
        (
            "skills_dir",
            "the directory of the session's skills, or empty",
        ),
        (
            "llm_url",
            "the forwarder's address for the session, with the protocol's own prefix",
        ),
        (
            "llm_token",
            "the token of the connector's listener, for the forwarder",
        ),
        ("model", "the model's routing name"),
        (
            "models_json",
            "the models the session may use, as a JSON list of names",
        ),
        ("effort", "the reasoning effort, or empty"),
        (
            "api",
            "the program's name for the protocol the model is reached by",
        ),
        ("protocol", "Orch's name for that protocol"),
        ("context_window", "the model's window in tokens, or empty"),
        (
            "image_input",
            "`true` when the model takes images, else `false`",
        ),
        ("approval_mode", "the session's approval mode"),
        ("permission_mode", "the program's word for it"),
        ("thread_id", "the thread's id"),
        ("segment_id", "the segment's id"),
        (
            "session_key",
            "the session's key in the connector's listener",
        ),
        ("orch_url", "the address of Orch's tools for the session"),
        ("plugin", "the absolute path of the companion plugin"),
        (
            "separated",
            "`true` when the session's arrangement puts the program's tools in the environment",
        ),
        (
            "instructions_json",
            "the session's instructions as a JSON string",
        ),
        (
            "item",
            "the model of the round of an `{#each models}` block",
        ),
    ];

    /// Variables for a program whose home is `home`.
    #[must_use]
    pub fn new(home: PathBuf) -> Self {
        let mut variables = Self {
            home: home.clone(),
            ..Self::default()
        };
        variables.set("home", home.to_string_lossy());
        variables.set("agents_home", home.join("agents").to_string_lossy());
        variables.set("neutral_dir", home.join("neutral").to_string_lossy());
        variables
    }

    /// The home.
    #[must_use]
    pub fn home(&self) -> &Path {
        &self.home
    }

    /// Gives a variable its value. A name that is not on [`Variables::NAMES`] is never read.
    pub fn set(&mut self, name: &'static str, value: impl Into<String>) {
        self.values.insert(name, value.into());
    }

    /// Records a file's absolute path, for `{file:<name>}`.
    pub fn file(&mut self, name: &str, path: &Path) {
        self.files
            .insert(name.to_owned(), path.to_string_lossy().into_owned());
    }

    /// Records the value of a variable the connector has, for those the manifest lists in `inherit`.
    pub fn inherit(&mut self, name: &str, value: &str) {
        self.inherited.insert(name.to_owned(), value.to_owned());
    }

    fn inherited(&self, name: &str) -> Option<String> {
        self.inherited.get(name).cloned()
    }

    fn lookup(&self, template: &str, name: &str) -> Result<String, ProgramError> {
        if let Some(word) = name.strip_prefix("approval.") {
            return self
                .approval
                .values
                .get(word)
                .cloned()
                .ok_or_else(|| ProgramError::MissingValue(template.to_owned(), name.to_owned()));
        }
        if let Some(file) = name.strip_prefix("file:") {
            return self
                .files
                .get(file)
                .cloned()
                .ok_or_else(|| ProgramError::MissingValue(template.to_owned(), name.to_owned()));
        }
        let known = Self::NAMES.iter().find(|(known, _)| *known == name);
        let Some((known, _)) = known else {
            return Err(ProgramError::UnknownVariable(
                template.to_owned(),
                name.to_owned(),
            ));
        };
        self.values
            .get(known)
            .cloned()
            .ok_or_else(|| ProgramError::MissingValue(template.to_owned(), name.to_owned()))
    }
}

/// Writes a template out. `{name}` is the value as it is, `{name:json}` the value as a JSON string, and `{{` and
/// `}}` are the braces themselves. What lies between `{#each models}` and `{/each}` is written once for each model
/// the session may use, with `{item}` the model. `{#if name=value}…{/if}` writes its body only when that
/// variable has exactly that value; protocol-specific options can then stay with the protocol that accepts them.
pub fn render(template: &str, variables: &Variables) -> Result<String, ProgramError> {
    const OPEN: &str = "{#each models}";
    const CLOSE: &str = "{/each}";
    const IF: &str = "{#if ";
    const END_IF: &str = "{/if}";
    let each = template.find(OPEN);
    if let Some(start) = template.find(IF)
        && each.is_none_or(|each| start < each)
    {
        let condition_start = start + IF.len();
        let condition_end = template[condition_start..]
            .find('}')
            .map(|at| condition_start + at)
            .ok_or_else(|| ProgramError::Unclosed(template.to_owned()))?;
        let (name, expected) = template[condition_start..condition_end]
            .split_once('=')
            .ok_or_else(|| {
                ProgramError::Unreadable("a template condition needs name=value".to_owned())
            })?;
        let after = condition_end + 1;
        let end = block_end(template, after, IF, END_IF)?;
        let mut out = render(&template[..start], variables)?;
        if variables.lookup(template, name)? == expected {
            out.push_str(&render(&template[after..end], variables)?);
        }
        out.push_str(&render(&template[end + END_IF.len()..], variables)?);
        return Ok(out);
    }
    if let Some(start) = each {
        let after = start + OPEN.len();
        let end = block_end(template, after, OPEN, CLOSE)?;
        let mut out = render(&template[..start], variables)?;
        for model in &variables.models {
            let mut round = variables.clone();
            round.set("item", model.clone());
            out.push_str(&render(&template[after..end], &round)?);
        }
        out.push_str(&render(&template[end + CLOSE.len()..], variables)?);
        return Ok(out);
    }
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let mut name = String::new();
                let mut closed = false;
                for next in chars.by_ref() {
                    if next == '}' {
                        closed = true;
                        break;
                    }
                    name.push(next);
                }
                if !closed {
                    return Err(ProgramError::Unclosed(template.to_owned()));
                }
                let (name, json) = match name.strip_suffix(":json") {
                    Some(plain) => (plain, true),
                    None => (name.as_str(), false),
                };
                let value = variables.lookup(template, name)?;
                if json {
                    out.push_str(&Json::String(value).to_string());
                } else {
                    out.push_str(&value);
                }
            }
            other => out.push(other),
        }
    }
    Ok(out)
}

/// Finds a block's own closing marker, allowing blocks of the same kind inside it.
fn block_end(
    template: &str,
    mut after: usize,
    open: &str,
    close: &str,
) -> Result<usize, ProgramError> {
    let mut depth = 1_usize;
    loop {
        let end = template[after..]
            .find(close)
            .map(|at| after + at)
            .ok_or_else(|| ProgramError::Unclosed(template.to_owned()))?;
        if let Some(start) = template[after..].find(open).map(|at| after + at)
            && start < end
        {
            depth += 1;
            after = start + open.len();
        } else {
            depth -= 1;
            if depth == 0 {
                return Ok(end);
            }
            after = end + close.len();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn variables() -> Variables {
        let mut variables = Variables::new(PathBuf::from("/home/h"));
        variables.set("model", "deepseek/flash: v4");
        variables.set("llm_url", "http://127.0.0.1:41000/llm/abc");
        variables
    }

    #[test]
    fn a_template_writes_values_raw_or_as_json_strings_and_keeps_doubled_braces() {
        let variables = variables();
        assert_eq!(
            render("--model {model} {{literal}}", &variables).unwrap(),
            "--model deepseek/flash: v4 {literal}"
        );
        assert_eq!(
            render("model: {model:json}", &variables).unwrap(),
            "model: \"deepseek/flash: v4\""
        );
    }

    #[test]
    fn conditions_keep_protocol_options_together_and_work_inside_model_blocks() {
        let mut variables = variables();
        variables.set("api", "openai-responses");
        variables.models = vec!["a".into(), "b".into()];
        assert_eq!(
            render(
                "before{#if api=openai-completions} chat{/if} after",
                &variables
            )
            .unwrap(),
            "before after"
        );
        variables.set("api", "openai-completions");
        assert_eq!(
            render(
                "{#if api=openai-completions}{#each models}{#if item=a}{item}{/if}{/each}{/if}",
                &variables
            )
            .unwrap(),
            "a"
        );
        assert_eq!(
            render(
                "{#if api=openai-completions}a{#if model=other}b{/if}c{/if}",
                &variables
            )
            .unwrap(),
            "ac"
        );
        assert!(matches!(
            render("{#if api=openai-completions}x", &variables),
            Err(ProgramError::Unclosed(_))
        ));
        assert!(matches!(
            render("{#if unknown=x}x{/if}", &variables),
            Err(ProgramError::UnknownVariable(_, _))
        ));
        assert!(matches!(
            render("{#if effort=x}x{/if}", &variables),
            Err(ProgramError::MissingValue(_, _))
        ));
    }

    #[test]
    fn a_file_that_needs_a_variable_is_left_out_without_it() {
        let mut program = program();
        program.files.push(LaunchFile {
            path: "bridge.yml".into(),
            template: "name: {plugin:json}\n".into(),
            rewrite: true,
            requires: Some("plugin".into()),
        });
        let mut variables = variables();
        variables.set("llm_token", "t");
        variables.set("permission_mode", "ask");
        assert_eq!(program.plan(&variables).unwrap().files.len(), 1);
        variables.set("plugin", "/p/bridge.js");
        let plan = program.plan(&variables).unwrap();
        assert_eq!(plan.files.len(), 2);
        assert_eq!(plan.files[1].content, b"name: \"/p/bridge.js\"\n");
    }

    #[test]
    fn a_block_is_written_once_for_each_model_with_the_model_as_the_item() {
        let mut variables = variables();
        variables.models = vec!["a/b".into(), "c".into()];
        assert_eq!(
            render(
                "m:\n{#each models}- {item:json} on {model:json}\n{/each}end",
                &variables
            )
            .unwrap(),
            "m:\n- \"a/b\" on \"deepseek/flash: v4\"\n- \"c\" on \"deepseek/flash: v4\"\nend"
        );
        assert!(matches!(
            render("{#each models}x", &variables),
            Err(ProgramError::Unclosed(_))
        ));
    }

    #[test]
    fn a_name_that_is_unknown_or_has_no_value_stops_the_launch_instead_of_dropping_the_setting() {
        let variables = variables();
        assert!(matches!(
            render("{nonsense}", &variables),
            Err(ProgramError::UnknownVariable(_, name)) if name == "nonsense"
        ));
        assert!(matches!(
            render("{effort}", &variables),
            Err(ProgramError::MissingValue(_, name)) if name == "effort"
        ));
        assert!(matches!(
            render("{model", &variables),
            Err(ProgramError::Unclosed(_))
        ));
        assert!(matches!(
            render("{file:patch}", &variables),
            Err(ProgramError::MissingValue(_, _))
        ));
    }

    fn program() -> AcpProgram {
        AcpProgram::from_json(&json!({
            "kind": "demo", "title": "Demo", "supported_range": ">=0.1.0",
            "args": ["--profile", "acp", "--patch", "{file:privacy.yml}"],
            "env": { "DEMO_HOME": "{home}", "DEMO_KEY": "{llm_token}" },
            "home_variable": "DEMO_HOME_DIR",
            "files": [ { "path": "privacy.yml", "template": "model: {model:json}\n" } ],
            "approvals": { "ask": { "permission_mode": "ask", "env": { "DEMO_MODE": "{permission_mode}" } } }
        }))
        .unwrap()
    }

    #[test]
    fn a_plan_has_the_arguments_the_files_and_only_the_variables_the_manifest_names() {
        let program = program();
        let mut variables = variables();
        variables.set("llm_token", "t0ken");
        variables.set("permission_mode", "ask");
        variables.approval = program.approval(ApprovalMode::Ask);
        variables.file("privacy.yml", Path::new("/home/h/privacy.yml"));
        variables.inherit("PATH", "/usr/bin");
        variables.inherit("HOME", "/root");
        let plan = program.plan(&variables).unwrap();
        assert_eq!(
            plan.args,
            ["--profile", "acp", "--patch", "/home/h/privacy.yml"]
        );
        assert_eq!(plan.files.len(), 1);
        assert_eq!(plan.files[0].path, Path::new("/home/h/privacy.yml"));
        assert_eq!(
            String::from_utf8(plan.files[0].content.clone()).unwrap(),
            "model: \"deepseek/flash: v4\"\n"
        );
        assert_eq!(
            plan.env.keys().map(String::as_str).collect::<Vec<_>>(),
            [
                "DEMO_HOME",
                "DEMO_HOME_DIR",
                "DEMO_KEY",
                "DEMO_MODE",
                "PATH"
            ],
            "a variable of the connector that the manifest does not name is not passed"
        );
        assert_eq!(plan.env["DEMO_MODE"], "ask");
    }

    #[test]
    fn a_file_that_would_leave_the_home_is_refused() {
        for path in ["../x", "/etc/x", "a/../../x", ""] {
            let mut program = program();
            program.files[0].path = path.to_owned();
            let mut variables = variables();
            variables.set("llm_token", "t");
            variables.set("permission_mode", "ask");
            variables.file("privacy.yml", Path::new("/home/h/privacy.yml"));
            assert!(
                matches!(program.plan(&variables), Err(ProgramError::OutsideHome(_))),
                "{path}"
            );
        }
    }

    #[test]
    fn launch_data_with_a_member_of_the_wrong_kind_is_unreadable_not_ignored() {
        assert!(matches!(
            AcpProgram::from_json(
                &json!({ "kind": "x", "title": "X", "supported_range": "*", "args": "no" })
            ),
            Err(ProgramError::Unreadable(_))
        ));
    }
}
