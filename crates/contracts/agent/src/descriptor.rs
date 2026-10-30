//! Capabilities, the arrangement types a harness can run in with their executors and tool sets, effort and quiet-start support, what the harness keeps as memory and where, whether a session is held to the MCP servers it is given and whether the harness's own can be listed for an import, the template its provider runtime is made from, and support facts.
//!
//! An adapter says what it is in one [`AgentDescriptor`], and everything Orch offers for a harness, from the
//! options of the "new thread" dialog to whether "steer" is shown, is read from it. Nothing above the
//! boundary names a harness: what an agent, its subscription, its plans and its ways of signing in are
//! called is in the descriptor too.
//!
//! A descriptor is data. A member that is absent means the adapter does not claim it, so the least claim is
//! what a missing [`Capabilities`] member comes to; a member the reader does not know is ignored, and one
//! whose name starts with `x_` is kept and never interpreted. A system may override any member of a stored
//! descriptor ([`AgentDescriptor::with_overrides`]).
//!
//! The rules here that need nothing but the declaration: how a harness's version stands against the range
//! its adapter supports ([`classify_version`]), whether an executor's build fits a harness
//! ([`ExecutorBuild::fits`]), and the sandbox mode a turn can run in ([`sandbox_for_turn`]). Which
//! arrangement a thread gets is in [`crate::arrangement`].

use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

use orch_plugin_contract::Extensions;
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::credentials::{CredentialForm, CredentialSupport, FormFault};
use crate::events::HarnessRequestKind;
use crate::login::{LoginMethodDef, MethodFault, check_methods};
use crate::parameters::{ParameterDef, ParameterFault, check_parameters};

/// Whether a provider is the sign-in of a harness or an API upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// The subscription login of one harness.
    Subscription,
    /// An API upstream of one or more endpoints, opened by a key.
    Api,
}

/// Where a machine or a runtime comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Orch runs it, from a template.
    Managed,
    /// A person's machine is it.
    Connected,
}

/// The five ways to place a harness, its tools and its credential for a thread. This is the only list of
/// them: an adapter declares the types its harness can run in, a provider's owner says in which the provider
/// may be used, a policy narrows and orders them, and a thread shows the one it has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ArrangementType {
    /// The harness runs in the provider's runtime; its tools are carried out by an executor in the
    /// environment; the credential stays where it is kept.
    Separated,
    /// The harness runs in the environment and calls Orch's forwarder, which adds the key in the provider's
    /// runtime.
    KeySeparated,
    /// The harness runs in the environment on a copy of the credential that the provider's runtime issues
    /// and renews.
    CredentialShared,
    /// The harness runs in the environment on the credential itself, moved there until it is returned.
    HandedOver,
    /// One machine is both places: a person's own, with harness, login and tools. Never chosen: it is what
    /// any arrangement amounts to when the provider's runtime and the environment are one machine.
    Together,
}

impl ArrangementType {
    /// Every type, in their fixed order.
    pub const ALL: [Self; 5] = [
        Self::Separated,
        Self::KeySeparated,
        Self::CredentialShared,
        Self::HandedOver,
        Self::Together,
    ];

    /// The type as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Separated => "separated",
            Self::KeySeparated => "key_separated",
            Self::CredentialShared => "credential_shared",
            Self::HandedOver => "handed_over",
            Self::Together => "together",
        }
    }

    /// Whether a provider's owner has to consent to it: every type but `together`, since nothing leaves a
    /// machine that already has everything.
    #[must_use]
    pub const fn needs_consent(self) -> bool {
        !matches!(self, Self::Together)
    }
}

impl fmt::Display for ArrangementType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Which build of its executor a `separated` entry will do with.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ExecutorBuild {
    /// The build of the harness it serves; what a harness that ships its own executor declares until a
    /// wider range is shown to work.
    Same,
    /// Any build, as for Orch's own executor.
    Any,
    /// A semver range, such as `>=0.160.0, <0.163.0`.
    Range(String),
}

impl ExecutorBuild {
    /// Whether an executor of version `executor` is a fit for a harness of version `harness`.
    #[must_use]
    pub fn fits(&self, executor: &str, harness: Option<&str>) -> bool {
        match self {
            Self::Any => true,
            Self::Same => harness.is_some_and(|harness| harness == executor),
            Self::Range(range) => version_in_range(executor, range),
        }
    }
}

impl fmt::Display for ExecutorBuild {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Same => f.write_str("same"),
            Self::Any => f.write_str("*"),
            Self::Range(range) => f.write_str(range),
        }
    }
}

impl Serialize for ExecutorBuild {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for ExecutorBuild {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Ok(match text.as_str() {
            "same" => Self::Same,
            "*" => Self::Any,
            _ => Self::Range(text),
        })
    }
}

impl JsonSchema for ExecutorBuild {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "ExecutorBuild".into()
    }

    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "string",
            "description": "\"same\" (the build of the harness it serves), \"*\" (any build) or a semver range"
        })
    }
}

/// One way the harness can be placed, as its adapter declares it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ArrangementEntry {
    /// The type.
    #[serde(rename = "type")]
    pub kind: ArrangementType,
    /// For `separated`: the executor the harness works through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor: Option<String>,
    /// For `separated`: the build that executor has to be.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_version: Option<ExecutorBuild>,
    /// For `separated` through an executor with several tool sets: those the harness can work with, in
    /// the adapter's order of preference. Which of them a thread gets is its environment's to say.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_sets: Option<Vec<String>>,
}

/// Whether a harness version is supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessSupport {
    /// The version is inside the supported range.
    Supported,
    /// The version is newer than the range. It works best-effort, and the interface says so.
    Untested,
    /// The version is older than the range, or lacks a required method. It cannot be bound.
    Unsupported,
}

/// The harness the adapter drives.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct HarnessInfo {
    /// What the agent is called wherever it is shown.
    pub name: String,
    /// The version of the harness that was found.
    pub version: String,
    /// The versions the adapter supports, as a semver range.
    pub supported_range: String,
    /// Where the version stands against the range.
    pub status: HarnessSupport,
}

/// Parses a harness version, tolerating a leading `v`.
fn parse_version(text: &str) -> Option<semver::Version> {
    semver::Version::parse(text.trim().trim_start_matches('v')).ok()
}

/// Whether `version` is in the semver range `range`. A version or a range that cannot be read is in no
/// range.
#[must_use]
pub fn version_in_range(version: &str, range: &str) -> bool {
    match (parse_version(version), semver::VersionReq::parse(range)) {
        (Some(version), Ok(range)) => range.matches(&version),
        _ => false,
    }
}

/// Where a harness of `version` stands against the range its adapter supports: `supported` inside it,
/// `untested` when it is newer than anything the range names a lower bound for (it is not older than the
/// range, and it is not inside), `unsupported` when it is older than the range or `has_required_methods`
/// is false. A version or a range that cannot be read is `untested`: nothing says it is wrong.
#[must_use]
pub fn classify_version(version: &str, range: &str, has_required_methods: bool) -> HarnessSupport {
    if !has_required_methods {
        return HarnessSupport::Unsupported;
    }
    let (Some(version), Ok(req)) = (parse_version(version), semver::VersionReq::parse(range))
    else {
        return HarnessSupport::Untested;
    };
    if req.matches(&version) {
        return HarnessSupport::Supported;
    }
    let below_a_lower_bound = req.comparators.iter().any(|comparator| {
        let lower = semver::Version::new(
            comparator.major,
            comparator.minor.unwrap_or(0),
            comparator.patch.unwrap_or(0),
        );
        match comparator.op {
            semver::Op::Greater => version.cmp(&lower) != Ordering::Greater,
            semver::Op::GreaterEq
            | semver::Op::Exact
            | semver::Op::Tilde
            | semver::Op::Caret
            | semver::Op::Wildcard => version.cmp(&lower) == Ordering::Less,
            _ => false,
        }
    });
    if below_a_lower_bound {
        HarnessSupport::Unsupported
    } else {
        HarnessSupport::Untested
    }
}

/// What a subscription of the harness is, when the harness has a sign-in of its own.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NativeProvider {
    /// The harness kind that holds the login.
    pub harness_kind: String,
    /// Which of the program's sign-ins this is, when it has several (`deepseek`, `openrouter`); absent for a
    /// program with one, which is named by its kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// What such a provider is called where one is added or listed.
    pub title: String,
    /// The plans the harness reports (`ProviderFacts.plan`), by the id reported, with the name to show; a
    /// plan that is not listed is shown as reported.
    #[serde(default)]
    pub plans: BTreeMap<String, String>,
    /// Every way a provider of this harness can be signed in.
    #[serde(default)]
    pub login_methods: Vec<LoginMethodDef>,
    /// JSON Schema of what such a provider may configure; `{}` is nothing.
    #[serde(default)]
    pub options_schema: Json,
    /// The types a new subscription of this harness allows as it is created.
    #[serde(default)]
    pub arrangements: Vec<ArrangementType>,
}

impl NativeProvider {
    /// The name this sign-in is told apart by: its own id, or the program's kind when it is the only one.
    #[must_use]
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.harness_kind)
    }
}

/// A template of the provider pool that an adapter brings, for a harness the system's own cannot run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeTemplate {
    /// Its name.
    pub name: String,
    /// The driver it is for.
    pub driver: String,
    /// The driver's own spec.
    pub spec: Json,
    /// The labels a host must carry.
    #[serde(default)]
    pub host_labels: Vec<String>,
}

/// How a harness is asked to treat actions it does not already trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalMode {
    /// It asks before actions it does not already trust.
    Ask,
    /// It asks when it judges it necessary or when its sandbox blocks an action.
    OnRequest,
    /// The harness's own reviewer decides eligible actions; anything it still asks is a card.
    Auto,
    /// It allows tool actions without asking; questions and proposed plans still wait for a person.
    Bypass,
}

/// How a side conversation reads and never acts: the mechanisms an adapter has verified on its exact build
/// and loaded configuration, tried in a fixed order; a mode's name or an API accepting it proves nothing.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SideReading {
    /// The harness's own enforced read-only mode: escalation denied, mutating tools removed or denied.
    ReadOnly,
    /// Every permission request declined before any effect, with only verified reading admitted.
    ApprovalGated,
    /// No model-callable tool at all: only the supplied history and message.
    NoTools,
}

impl SideReading {
    /// The order the mechanisms are tried in: the first an adapter has verified is the one a side
    /// conversation uses.
    pub const ORDER: [Self; 3] = [Self::ReadOnly, Self::ApprovalGated, Self::NoTools];

    /// The first mechanism of `verified`, in [`SideReading::ORDER`]; none when it has none.
    #[must_use]
    pub fn first_of(verified: &[Self]) -> Option<Self> {
        Self::ORDER.into_iter().find(|mode| verified.contains(mode))
    }
}

/// The native sandbox levels, shared with their public readiness report.
pub use orch_api_contract::environment::SandboxMode;

/// The mode a turn runs in: the requested one, lowered to the widest the environment allows when it
/// exceeds that, and then to the widest mode at or below that which `enforceable` lists. `None` when no
/// mode at or below can be enforced: the thread is then blocked, and what has to give is the setting, the
/// environment's limit, or the environment itself.
#[must_use]
pub fn sandbox_for_turn(
    requested: SandboxMode,
    widest_allowed: SandboxMode,
    enforceable: &[SandboxMode],
) -> Option<SandboxMode> {
    let lowered = requested.min(widest_allowed);
    enforceable
        .iter()
        .copied()
        .filter(|mode| *mode <= lowered)
        .max()
}

/// How a harness takes the reasoning effort.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EffortSupport {
    /// With each turn.
    Turn,
    /// Once, when a session opens.
    Session,
    /// It has no such control; the control is shown disabled with that reason.
    #[default]
    None,
}

/// Whether the harness can be given Orch's own tools.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OrchToolsSupport {
    /// As an MCP server named `orch`.
    Mcp,
    /// It cannot take an MCP server at all.
    #[default]
    None,
}

/// How firmly the models of a harness's sub-agents and helper steps are held to the allowlist.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryModels {
    /// They are kept on the thread's model, or on the allowed one a parameter names.
    Pinned,
    /// The harness chooses, within the allowlist it is given with the session.
    Allowlisted,
    /// The harness offers no such control; the binding dialog says so beside the model.
    #[default]
    Uncontrolled,
}

/// What the harness keeps for itself from one session to the next, and whether the adapter can put it into
/// a directory it is given.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MemoryKind {
    /// The harness has a memory of its own, and can be told for one session where to keep it.
    Directory,
    /// The harness has one memory for its whole home. It is always switched off: one home serves every
    /// thread of a provider.
    Home,
    /// The harness has no memory.
    #[default]
    None,
}

/// Whether the harness has a terminal of its own that people can reach.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AgentTerminal {
    /// The harness is driven by a protocol. There is no agent terminal.
    #[default]
    None,
    /// The harness's screen can be watched.
    View,
    /// It can also be typed into.
    Interactive,
}

/// The kinds of fact a harness reports of its provider: written once, with the provider's facts.
pub use orch_provider_contract::facts::ProviderFactKind;

/// When a harness takes up the files of a thread's directory that changed since its session was opened.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionReload {
    /// Only when the session is opened again: the harness keeps those files while a session is open, so Orch
    /// opens it again before each turn that works in the environment itself.
    Turn,
    /// By itself, as they change.
    #[default]
    OnChange,
}

/// Whether, and how, a harness takes the skills of a thread. Whichever it is, the connector writes only
/// `skills/<name>/SKILL.md` into the session's skills root, and the adapter maps that root into its harness.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SkillsSupport {
    /// It takes no skills; the thread's skills are not written and the screens say so.
    #[default]
    None,
    /// It reads skills from a directory it is told, and the adapter hands it the root.
    Directory,
    /// It loads skills as a plugin, which the adapter builds for each session from the root.
    Plugin,
}

/// The reviewer a harness has for its own permission decisions.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ApprovalReviewerSupport {
    /// The model it reviews with unless told another.
    pub model: String,
    /// Whether a person can choose another model for it. When they cannot, Auto is offered only to people who
    /// may use that model.
    #[serde(default)]
    pub settable: bool,
}

/// How plan mode works in one harness: how the plan is put to the person and what Proceed does.
pub use orch_api_contract::binding::{PlanMechanics, PlanProceed, PlanProposal};

/// What a harness reports of how full the context is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UsageContext {
    /// Nothing: the fill is not known.
    #[default]
    None,
    /// How large the last prompt was against the window, and no more; Orch splits it into the parts it can see.
    SizeOnly,
    /// The size, and what fills it by part.
    Breakdown,
}

impl<'de> Deserialize<'de> for UsageContext {
    /// Descriptors written before this had a boolean: `true` said that the prompt's size is reported.
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(rename_all = "snake_case")]
        enum Word {
            None,
            SizeOnly,
            Breakdown,
        }
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Written {
            Legacy(bool),
            Word(Word),
        }
        Ok(match Written::deserialize(deserializer)? {
            Written::Legacy(true) | Written::Word(Word::SizeOnly) => Self::SizeOnly,
            Written::Legacy(false) | Written::Word(Word::None) => Self::None,
            Written::Word(Word::Breakdown) => Self::Breakdown,
        })
    }
}

impl UsageContext {
    /// Whether the size of the prompt is reported.
    #[must_use]
    pub const fn reported(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// What the harness can do with MCP servers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpSupport {
    /// It takes servers that it starts as programs.
    pub stdio: bool,
    /// It takes servers it reaches by address.
    pub http: bool,
    /// A changed set of servers reaches a session at its next turn.
    pub reload: bool,
    /// A session can be held to the servers Orch gives it: the harness then loads none from its own
    /// configuration. One that cannot says so, and its threads show that the harness may use servers of
    /// its own.
    pub strict: bool,
}

/// A native control over the harness's own sub-agents.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SubagentControl {
    /// How many spawned agents can be open at once.
    AtOnce,
    /// The deepest layer of spawned agents.
    Depth,
}

/// What an adapter claims for its harness. A member that is absent is not claimed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NativeInputPrefixSupport {
    /// Actual retained first user input of a native turn; mid-turn inputs are not included.
    TurnStart,
}

/// What an adapter claims for its harness. A member that is absent is not claimed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct Capabilities {
    /// A message can be added to a running turn.
    pub steer: bool,
    /// A running turn can be stopped.
    pub interrupt: bool,
    /// Live native session commands can be listed and individually ended without model input.
    #[serde(default)]
    pub background_commands: bool,
    /// The kinds of approval it asks for.
    pub approvals: Vec<HarnessRequestKind>,
    /// It runs sub-agents of its own.
    pub subagents: bool,
    /// The sub-agent limits this harness can enforce; empty means neither is claimed.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub subagent_limits: Vec<SubagentControl>,
    /// It keeps a plan.
    pub plan: bool,
    /// It can work a turn in plan mode: it reads, proposes a plan and changes nothing until a person
    /// proceeds, without any change of the approval mode.
    #[serde(default)]
    pub plan_mode: bool,
    /// How plan mode works where `plan_mode` is claimed: how the plan is put, and what Proceed does. Absent, a
    /// plan is an item and Proceed sends a message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_mechanics: Option<PlanMechanics>,
    /// It reports a summary of its reasoning.
    pub reasoning_summary: bool,
    /// It takes images.
    pub input_images: bool,
    /// How it takes the reasoning effort.
    pub effort: EffortSupport,
    /// It can list the MCP servers its harness's own configuration names, and read the definition of one
    /// out for an import.
    pub mcp_discovery: bool,
    /// What it can do with MCP servers.
    pub mcp: McpSupport,
    /// Whether it can be given Orch's own tools.
    pub orch_tools: OrchToolsSupport,
    /// A session can be exported as the harness keeps it.
    pub export_native: bool,
    /// A session exported by this harness kind can be imported.
    pub import_native: bool,
    /// A neutral history can be imported.
    pub import_neutral: bool,
    /// A session can be forked inside the harness.
    pub fork_native: bool,
    /// Exact native boundary support, separate from inclusive whole-turn/whole-session forks.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub native_input_prefix: Option<NativeInputPrefixSupport>,
    /// A session can be resumed by its id.
    pub resume: bool,
    /// The sessions the harness already has can be listed.
    pub discover_sessions: bool,
    /// It reports token totals while it works.
    pub usage_tokens: bool,
    /// What it reports of how full the context is.
    pub usage_context: UsageContext,
    /// It says when a running turn starts to shorten its own conversation, and when it goes on.
    pub phases: bool,
    /// The facts it reports of its provider.
    pub provider_facts: Vec<ProviderFactKind>,
    /// It can list the models of its provider.
    pub model_discovery: bool,
    /// It reports a title for a session.
    pub title: bool,
    /// How firmly its sub-agents and helper steps are held to the allowlist.
    pub auxiliary_models: AuxiliaryModels,
    /// It can be started with every call that is not the work switched off.
    pub telemetry_off: bool,
    /// What the harness keeps for itself from one session to the next.
    pub memory: MemoryKind,
    /// Its own terminal, if people can reach one.
    pub agent_terminal: AgentTerminal,
    /// When it takes up changed files of a thread's directory.
    #[serde(default)]
    pub session_reload: SessionReload,
    /// Whether it walks up the directory tree for its instruction files, so that a marker at the workspace root
    /// must stop it there.
    #[serde(default)]
    pub instruction_root_marker: bool,
    /// Whether, and how, it takes the thread's skills.
    #[serde(default)]
    pub skills: SkillsSupport,
    /// The reviewer of its own permission decisions, when it has one; Auto is offered only for a program that
    /// declares it and whose build passed the conformance case that proves it, which no override can supply.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub approval_reviewer: Option<ApprovalReviewerSupport>,
}

impl Capabilities {
    /// How plan mode works: what the adapter declared, or an item that Proceed answers with a message, which is
    /// what an adapter that says nothing is read as. Absent where plan mode is not claimed.
    #[must_use]
    pub fn plan_style(&self) -> Option<PlanMechanics> {
        self.plan_mode.then(|| {
            self.plan_mechanics.unwrap_or(PlanMechanics {
                proposal: PlanProposal::Item,
                proceed: PlanProceed::Message,
            })
        })
    }
}

/// Everything an adapter declares about its harness.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct AgentDescriptor {
    /// The harness kind, such as `codex`.
    pub kind: String,
    /// The version of the adapter's own declarations.
    pub adapter_version: u32,
    /// The harness it drives.
    pub harness: HarnessInfo,
    /// The ways this harness can be placed, in the adapter's own order of preference.
    pub arrangements: Vec<ArrangementEntry>,
    /// The harness's own sign-in, or `null` when it has none. Read for descriptors written before a harness could
    /// have several; use [`AgentDescriptor::native_providers`].
    #[serde(default)]
    pub native_provider: Option<NativeProvider>,
    /// The harness's own sign-ins: one for each vendor it signs in to. A descriptor that has only
    /// `native_provider` has this one.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_providers: Vec<NativeProvider>,
    /// The protocols it can speak to Orch's forwarder (it accepts another endpoint).
    #[serde(default)]
    pub forwarded_protocols: Vec<String>,
    /// The protocols it speaks to its upstream itself, on a key it is given.
    #[serde(default)]
    pub direct_protocols: Vec<String>,
    /// The credential forms it can issue from its login and the names of those it can run on.
    #[serde(default)]
    pub credentials: CredentialSupport,
    /// A template of the provider pool that the adapter brings, for a harness the system's cannot run;
    /// `null` for the system's.
    #[serde(default)]
    pub runtime_template: Option<RuntimeTemplate>,
    /// The approval modes it has, with its default first. A new thread keeps the place's mode where
    /// offered and otherwise takes the first mode other than Auto, which is never chosen automatically.
    /// Policy and confinement still apply.
    #[serde(default)]
    pub approval_modes: Vec<ApprovalMode>,
    /// What its own sandbox can enforce; `["open"]` is none.
    #[serde(default)]
    pub sandbox_modes: Vec<SandboxMode>,
    /// How a side conversation can read without acting, as verified on this build and configuration; empty
    /// where nothing is verified, and a side conversation is then refused with why.
    #[serde(default)]
    pub side_thread_modes: Vec<SideReading>,
    /// What it claims to do.
    #[serde(default)]
    pub capabilities: Capabilities,
    /// The parameters its harness accepts.
    #[serde(default)]
    pub parameters: Vec<ParameterDef>,
    /// The words of the descriptor in the other languages: language tag, then the place of a word and its
    /// translation. The English a descriptor carries in place is the fallback. The places are `harness.name`,
    /// `native_provider.title` (`native_providers.<id>.title` for one of several), `…plans.<plan>`,
    /// `…login_methods.<method>.title`, `.summary` and `.requires`, `…login_methods.<method>.fields.<name>.label`
    /// and `.help`, `parameters.<parameter>.title` and `.description`, `parameters.<parameter>.options.<value>.label` and `.help`, and `windows.<window>` for the label of a plan
    /// window the harness reports. A place that is not listed is not translated.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub text: BTreeMap<String, BTreeMap<String, String>>,
    /// Members that belong to the implementation, kept and never interpreted.
    #[serde(flatten)]
    pub extensions: Extensions,
}

/// What is wrong with a descriptor.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DescriptorFault {
    /// It names no kind.
    #[error("a descriptor names its kind")]
    NoKind,
    /// It declares no way to place the harness.
    #[error("a descriptor declares at least one arrangement")]
    NoArrangement,
    /// A type is declared twice, other than `separated` through different executors.
    #[error("arrangement {0} is declared twice")]
    DuplicateArrangement(ArrangementType),
    /// A `separated` entry names no executor.
    #[error("a separated arrangement names the executor it works through")]
    SeparatedWithoutExecutor,
    /// An entry other than `separated` names an executor or tool sets.
    #[error("arrangement {0} names an executor, and only separated does")]
    ExecutorOutsideSeparated(ArrangementType),
    /// Two `separated` entries name one executor.
    #[error("executor {0:?} is named by two separated entries")]
    DuplicateExecutor(String),
    /// Tool sets are listed without being asked of an executor, or are an empty list.
    #[error("the tool sets of executor {0:?} are a list that names at least one")]
    EmptyToolSets(String),
    /// `native_provider` proposes a type that the adapter does not declare, or `together`.
    #[error(
        "a new subscription proposes {0}, which the harness does not declare or which needs no consent"
    )]
    ProposalNotDeclared(ArrangementType),
    /// `key_separated` is declared and no protocol can reach the forwarder.
    #[error("key_separated is declared and the harness speaks no protocol to a forwarder")]
    KeySeparatedWithoutProtocol,
    /// A credential-carrying arrangement is declared and no form is accepted.
    #[error("{0} is declared and the harness accepts no credential form")]
    SharingWithoutForms(ArrangementType),
    /// It lists no approval mode.
    #[error("a descriptor lists the approval modes it has")]
    NoApprovalMode,
    /// It lists no sandbox mode; a harness without a sandbox declares `open` alone.
    #[error(
        "a descriptor lists the sandbox modes it can enforce; a harness without a sandbox lists open"
    )]
    NoSandboxMode,
    /// A credential form is declared wrongly.
    #[error(transparent)]
    Form(#[from] FormFault),
    /// The sign-in methods are declared wrongly.
    #[error(transparent)]
    Method(#[from] MethodFault),
    /// The parameters are declared wrongly.
    #[error(transparent)]
    Parameter(#[from] ParameterFault),
}

impl AgentDescriptor {
    /// Whether the harness can run in an arrangement of `kind`.
    #[must_use]
    pub fn supports(&self, kind: ArrangementType) -> bool {
        self.arrangements.iter().any(|entry| entry.kind == kind)
    }

    /// The `separated` entries, in the adapter's order.
    pub fn separated(&self) -> impl Iterator<Item = &ArrangementEntry> {
        self.arrangements
            .iter()
            .filter(|entry| entry.kind == ArrangementType::Separated)
    }

    /// Every sign-in of its own the harness has: those the descriptor lists, and the single one an older
    /// descriptor carries, first.
    #[must_use]
    pub fn native_providers(&self) -> Vec<&NativeProvider> {
        let mut all: Vec<&NativeProvider> = Vec::new();
        for provider in self.native_provider.iter().chain(&self.native_providers) {
            if !all.iter().any(|seen| seen.id() == provider.id()) {
                all.push(provider);
            }
        }
        all
    }

    /// The descriptor in the form every reader of the API sees: the list of sign-ins complete, and the single one
    /// the first of them.
    #[must_use]
    pub fn normalized(mut self) -> Self {
        let all: Vec<NativeProvider> = self.native_providers().into_iter().cloned().collect();
        self.native_provider = all.first().cloned();
        self.native_providers = all;
        self
    }

    /// The words at `place` in `language`, when the descriptor has a translation there.
    #[must_use]
    pub fn text_in(&self, language: &str, place: &str) -> Option<&str> {
        self.text
            .get(language)
            .and_then(|words| words.get(place))
            .map(String::as_str)
            .filter(|words| !words.trim().is_empty())
    }

    /// The words at `place` in every language that has some, by language tag.
    #[must_use]
    pub fn words_at(&self, place: &str) -> BTreeMap<String, String> {
        self.text
            .iter()
            .filter_map(|(language, words)| {
                words
                    .get(place)
                    .filter(|words| !words.trim().is_empty())
                    .map(|words| (language.clone(), words.clone()))
            })
            .collect()
    }

    /// What the agent is called in `language`: the translation, or the name it declares.
    #[must_use]
    pub fn name_in(&self, language: &str) -> &str {
        self.text_in(language, "harness.name")
            .unwrap_or(&self.harness.name)
    }

    /// The forms this adapter issues, in the order it declares them.
    #[must_use]
    pub fn issues(&self) -> &[CredentialForm] {
        &self.credentials.issues
    }

    /// Checks that the descriptor is consistent with itself: what it declares can hold together. Every
    /// fault is returned. This is not a check that the declaration is true of the harness; the contract
    /// suite does that.
    pub fn check(&self) -> Result<(), Vec<DescriptorFault>> {
        let mut faults = Vec::new();
        if self.kind.is_empty() {
            faults.push(DescriptorFault::NoKind);
        }
        if self.arrangements.is_empty() {
            faults.push(DescriptorFault::NoArrangement);
        }
        let mut executors: Vec<&str> = Vec::new();
        for (position, entry) in self.arrangements.iter().enumerate() {
            match (entry.kind, entry.executor.as_deref()) {
                (ArrangementType::Separated, None) => {
                    faults.push(DescriptorFault::SeparatedWithoutExecutor)
                }
                (ArrangementType::Separated, Some(executor)) => {
                    if executors.contains(&executor) {
                        faults.push(DescriptorFault::DuplicateExecutor(executor.to_owned()));
                    } else {
                        executors.push(executor);
                    }
                    if entry.tool_sets.as_ref().is_some_and(Vec::is_empty) {
                        faults.push(DescriptorFault::EmptyToolSets(executor.to_owned()));
                    }
                }
                (kind, _) => {
                    if entry.executor.is_some()
                        || entry.tool_sets.is_some()
                        || entry.executor_version.is_some()
                    {
                        faults.push(DescriptorFault::ExecutorOutsideSeparated(kind));
                    }
                    if self.arrangements[..position]
                        .iter()
                        .any(|before| before.kind == kind)
                    {
                        faults.push(DescriptorFault::DuplicateArrangement(kind));
                    }
                }
            }
        }
        if self.supports(ArrangementType::KeySeparated) && self.forwarded_protocols.is_empty() {
            faults.push(DescriptorFault::KeySeparatedWithoutProtocol);
        }
        for sharing in [
            ArrangementType::CredentialShared,
            ArrangementType::HandedOver,
        ] {
            if self.supports(sharing) && self.credentials.accepts.is_empty() {
                faults.push(DescriptorFault::SharingWithoutForms(sharing));
            }
        }
        if self.approval_modes.is_empty() {
            faults.push(DescriptorFault::NoApprovalMode);
        }
        if self.sandbox_modes.is_empty() {
            faults.push(DescriptorFault::NoSandboxMode);
        }
        faults.extend(
            self.credentials
                .issues
                .iter()
                .filter_map(|form| form.check().err())
                .map(DescriptorFault::Form),
        );
        for native in self.native_providers() {
            for proposed in &native.arrangements {
                if !proposed.needs_consent() || !self.supports(*proposed) {
                    faults.push(DescriptorFault::ProposalNotDeclared(*proposed));
                }
            }
            if let Err(found) = check_methods(&native.login_methods) {
                faults.extend(found.into_iter().map(DescriptorFault::Method));
            }
        }
        if let Err(found) = check_parameters(&self.parameters) {
            faults.extend(found.into_iter().map(DescriptorFault::Parameter));
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }

    /// The descriptor with a system's overrides merged over it member by member: an object in the
    /// overrides is merged into the object it meets, anything else replaces what it meets, and `null`
    /// removes the member (so that its default applies).
    pub fn with_overrides(&self, overrides: &Json) -> Result<Self, serde_json::Error> {
        let mut merged = serde_json::to_value(self)?;
        merge_patch(&mut merged, overrides);
        if let Some(modes) = merged
            .get_mut("approval_modes")
            .and_then(Json::as_array_mut)
        {
            modes.retain(|mode| {
                serde_json::from_value::<ApprovalMode>(mode.clone())
                    .ok()
                    .is_some_and(|mode| self.approval_modes.contains(&mode))
            });
        }
        // A manager may turn proven native control off, but cannot manufacture its conformance proof.
        if !self.capabilities.background_commands
            && let Some(capabilities) = merged.get_mut("capabilities").and_then(Json::as_object_mut)
        {
            capabilities.insert("background_commands".to_owned(), Json::Bool(false));
        }
        if !self.capabilities.import_native
            && let Some(capabilities) = merged.get_mut("capabilities").and_then(Json::as_object_mut)
        {
            capabilities.insert("import_native".to_owned(), Json::Bool(false));
        }
        if self.capabilities.native_input_prefix.is_none()
            && let Some(capabilities) = merged.get_mut("capabilities").and_then(Json::as_object_mut)
        {
            capabilities.remove("native_input_prefix");
        }
        // Auto is a mode the adapter claims on its own proof; a reviewer an override names for an adapter
        // that has none would open it.
        if self.capabilities.approval_reviewer.is_none()
            && let Some(capabilities) = merged.get_mut("capabilities").and_then(Json::as_object_mut)
        {
            capabilities.remove("approval_reviewer");
        }
        if let Some(controls) = merged
            .pointer_mut("/capabilities/subagent_limits")
            .and_then(Json::as_array_mut)
        {
            controls.retain(|control| {
                serde_json::from_value::<SubagentControl>(control.clone())
                    .ok()
                    .is_some_and(|control| self.capabilities.subagent_limits.contains(&control))
            });
        }
        serde_json::from_value(merged)
    }
}

/// Merges `patch` into `target` as a JSON merge patch does.
fn merge_patch(target: &mut Json, patch: &Json) {
    match patch {
        Json::Object(members) => {
            if !target.is_object() {
                *target = Json::Object(serde_json::Map::new());
            }
            if let Json::Object(into) = target {
                for (name, value) in members {
                    if value.is_null() {
                        into.remove(name);
                    } else {
                        merge_patch(into.entry(name.clone()).or_insert(Json::Null), value);
                    }
                }
            }
        }
        other => *target = other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// A descriptor of the first built-in adapter, as it is published.
    fn codex() -> Json {
        let capabilities = json!({
          "steer": true, "interrupt": true, "background_commands": false,
          "approvals": ["command", "file_change", "permissions", "user_input", "mcp_elicitation"],
          "subagents": true, "plan": true, "plan_mode": true,
          "plan_mechanics": { "proposal": "item", "proceed": "message" },
          "reasoning_summary": true, "input_images": true,
          "effort": "turn",
          "mcp_discovery": true,
          "mcp": { "stdio": true, "http": true, "reload": true, "strict": true },
          "orch_tools": "mcp",
          "export_native": true, "import_native": true, "import_neutral": true, "fork_native": true,
          "resume": true, "discover_sessions": true,
          "usage_tokens": true, "usage_context": "size_only",
          "phases": true,
          "provider_facts": ["plan", "windows", "plan_credits"],
          "model_discovery": true, "title": true,
          "auxiliary_models": "pinned",
          "telemetry_off": true,
          "memory": "home",
          "agent_terminal": "none",
          "session_reload": "turn",
          "instruction_root_marker": true,
          "skills": "directory",
          "approval_reviewer": { "model": "codex-auto-review", "settable": false }
        });
        json!({
          "kind": "codex",
          "adapter_version": 1,
          "harness": { "name": "Codex", "version": "0.160.1", "supported_range": ">=0.160.0, <0.163.0", "status": "supported" },
          "arrangements": [
            { "type": "separated", "executor": "codex-exec-server", "executor_version": "same" },
            { "type": "key_separated" },
            { "type": "together" } ],
          "native_provider": {
            "harness_kind": "codex", "title": "Codex subscription",
            "plans": { "plus": "Plus", "pro": "Pro", "team": "Team" },
            "login_methods": [],
            "options_schema": {},
            "arrangements": ["separated"] },
          "forwarded_protocols": ["openai-responses"],
          "direct_protocols": [],
          "credentials": { "issues": [], "accepts": [] },
          "runtime_template": null,
          "approval_modes": ["ask", "on_request", "auto"],
          "sandbox_modes": ["read_only", "workspace", "open"],
          "side_thread_modes": ["read_only"],
          "capabilities": capabilities,
          "parameters": []
        })
    }

    #[test]
    fn the_descriptor_of_the_first_adapter_reads_and_writes_as_the_spec_shows_it() {
        let read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        assert_eq!(read.kind, "codex");
        assert_eq!(read.harness.status, HarnessSupport::Supported);
        assert_eq!(
            read.arrangements[0].executor_version,
            Some(ExecutorBuild::Same)
        );
        assert_eq!(read.capabilities.memory, MemoryKind::Home);
        assert_eq!(read.capabilities.effort, EffortSupport::Turn);
        assert!(read.capabilities.mcp.strict);
        assert_eq!(read.check(), Ok(()));
        assert_eq!(serde_json::to_value(&read).unwrap(), codex());
    }

    #[test]
    fn a_codex_profile_cannot_invent_native_import_and_may_narrow_a_proved_build() {
        let mut declared: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        for proved in [false, true] {
            declared.capabilities.import_native = proved;
            assert_eq!(
                declared
                    .with_overrides(&json!({"capabilities":{"import_native":true}}))
                    .unwrap()
                    .capabilities
                    .import_native,
                proved
            );
            assert!(
                !declared
                    .with_overrides(&json!({"capabilities":{"import_native":false}}))
                    .unwrap()
                    .capabilities
                    .import_native
            );
        }
    }

    #[test]
    fn a_descriptor_that_claims_the_minimum_is_a_descriptor() {
        let read: AgentDescriptor = serde_json::from_value(json!({
            "kind": "tiny", "adapter_version": 1,
            "harness": { "name": "Tiny", "version": "1.0.0", "supported_range": ">=1.0.0", "status": "supported" },
            "arrangements": [ { "type": "together" } ],
            "approval_modes": ["auto"], "sandbox_modes": ["open"]
        }))
        .unwrap();
        assert!(!read.capabilities.steer && !read.capabilities.resume);
        assert_eq!(read.capabilities.memory, MemoryKind::None);
        assert_eq!(
            read.capabilities.auxiliary_models,
            AuxiliaryModels::Uncontrolled
        );
        assert!(read.native_provider.is_none() && read.runtime_template.is_none());
        assert_eq!(read.check(), Ok(()));
    }

    #[test]
    fn members_of_the_implementations_own_are_kept_and_unknown_ones_are_ignored() {
        let mut with = codex();
        with["x_build"] = json!({ "commit": "abc123" });
        with["added_by_a_later_version"] = json!(true);
        let read: AgentDescriptor = serde_json::from_value(with).unwrap();
        assert_eq!(
            read.extensions.get("x_build"),
            Some(&json!({ "commit": "abc123" }))
        );
        let written = serde_json::to_value(&read).unwrap();
        assert_eq!(written["x_build"]["commit"], "abc123");
        assert!(written.get("added_by_a_later_version").is_none());
    }

    #[test]
    fn a_profile_cannot_restore_native_import_after_the_build_switched_it_off()
    -> Result<(), serde_json::Error> {
        let mut read: AgentDescriptor = serde_json::from_value(codex())?;
        read.capabilities.import_native = false;
        let changed = read.with_overrides(&json!({"capabilities":{"import_native":true}}))?;
        assert!(!changed.capabilities.import_native);
        assert!(changed.capabilities.import_neutral);
        Ok(())
    }

    #[test]
    fn a_systems_overrides_are_merged_over_the_descriptor_member_by_member() {
        let read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        let merged = read
            .with_overrides(&json!({
                "harness": { "name": "Our Codex" },
                "capabilities": { "steer": false },
                "native_provider": { "title": "Team Codex" },
                "forwarded_protocols": null
            }))
            .unwrap();
        assert_eq!(merged.harness.name, "Our Codex");
        assert_eq!(
            merged.harness.version, "0.160.1",
            "what the override does not name stays"
        );
        assert!(!merged.capabilities.steer && merged.capabilities.interrupt);
        assert_eq!(merged.native_provider.as_ref().unwrap().title, "Team Codex");
        assert_eq!(merged.native_provider.as_ref().unwrap().plans["pro"], "Pro");
        assert!(
            merged.forwarded_protocols.is_empty(),
            "null removes the member, so its default applies"
        );
    }

    #[test]
    fn an_override_can_narrow_but_cannot_manufacture_native_prefix_support()
    -> Result<(), serde_json::Error> {
        let mut read: AgentDescriptor = serde_json::from_value(codex())?;
        assert!(
            read.with_overrides(&json!({"capabilities":{"native_input_prefix":"turn_start"}}))?
                .capabilities
                .native_input_prefix
                .is_none()
        );
        read.capabilities.native_input_prefix = Some(NativeInputPrefixSupport::TurnStart);
        assert_eq!(
            read.with_overrides(&json!({}))?
                .capabilities
                .native_input_prefix,
            Some(NativeInputPrefixSupport::TurnStart)
        );
        assert!(
            read.with_overrides(&json!({"capabilities":{"native_input_prefix":null}}))?
                .capabilities
                .native_input_prefix
                .is_none()
        );
        Ok(())
    }

    #[test]
    fn a_descriptor_that_cannot_hold_together_is_refused_for_each_reason() {
        let mut broken: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        broken.arrangements = vec![
            ArrangementEntry {
                kind: ArrangementType::Separated,
                executor: None,
                executor_version: None,
                tool_sets: None,
            },
            ArrangementEntry {
                kind: ArrangementType::KeySeparated,
                executor: Some("x".into()),
                executor_version: None,
                tool_sets: None,
            },
            ArrangementEntry {
                kind: ArrangementType::CredentialShared,
                executor: None,
                executor_version: None,
                tool_sets: None,
            },
            ArrangementEntry {
                kind: ArrangementType::CredentialShared,
                executor: None,
                executor_version: None,
                tool_sets: None,
            },
        ];
        broken.forwarded_protocols.clear();
        broken.approval_modes.clear();
        broken.sandbox_modes.clear();
        broken.native_provider.as_mut().unwrap().arrangements =
            vec![ArrangementType::Together, ArrangementType::HandedOver];
        let faults = broken.check().unwrap_err();
        for expected in [
            DescriptorFault::SeparatedWithoutExecutor,
            DescriptorFault::ExecutorOutsideSeparated(ArrangementType::KeySeparated),
            DescriptorFault::DuplicateArrangement(ArrangementType::CredentialShared),
            DescriptorFault::KeySeparatedWithoutProtocol,
            DescriptorFault::SharingWithoutForms(ArrangementType::CredentialShared),
            DescriptorFault::NoApprovalMode,
            DescriptorFault::NoSandboxMode,
            DescriptorFault::ProposalNotDeclared(ArrangementType::Together),
            DescriptorFault::ProposalNotDeclared(ArrangementType::HandedOver),
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
    }

    #[test]
    fn a_harness_version_is_supported_inside_the_range_untested_above_it_and_unsupported_below_it()
    {
        let range = ">=0.160.0, <0.163.0";
        assert_eq!(
            classify_version("0.160.0", range, true),
            HarnessSupport::Supported
        );
        assert_eq!(
            classify_version("0.162.9", range, true),
            HarnessSupport::Supported
        );
        assert_eq!(
            classify_version("0.163.0", range, true),
            HarnessSupport::Untested
        );
        assert_eq!(
            classify_version("1.0.0", range, true),
            HarnessSupport::Untested
        );
        assert_eq!(
            classify_version("0.159.9", range, true),
            HarnessSupport::Unsupported
        );
        assert_eq!(
            classify_version("0.161.0", range, false),
            HarnessSupport::Unsupported
        );
        assert_eq!(
            classify_version("2.1.285", ">=2.1.285, <2.2.0", true),
            HarnessSupport::Supported
        );
        assert_eq!(
            classify_version("v2.2.1", ">=2.1.285, <2.2.0", true),
            HarnessSupport::Untested
        );
        assert_eq!(
            classify_version("soon", range, true),
            HarnessSupport::Untested
        );
        assert_eq!(
            classify_version("0.161.0", "<1.0.0", true),
            HarnessSupport::Supported
        );
        assert_eq!(
            classify_version("1.0.0", "<1.0.0", true),
            HarnessSupport::Untested
        );
    }

    #[test]
    fn an_executor_build_fits_a_harness_as_the_entry_says() {
        assert!(ExecutorBuild::Any.fits("9.9.9", None));
        assert!(ExecutorBuild::Same.fits("0.160.1", Some("0.160.1")));
        assert!(!ExecutorBuild::Same.fits("0.160.2", Some("0.160.1")));
        assert!(!ExecutorBuild::Same.fits("0.160.1", None));
        let range = ExecutorBuild::Range(">=0.160.0, <0.162.0".into());
        assert!(range.fits("0.161.5", Some("0.162.0")));
        assert!(!range.fits("0.162.0", Some("0.162.0")));
        assert_eq!(
            serde_json::to_value(&range).unwrap(),
            json!(">=0.160.0, <0.162.0")
        );
        assert_eq!(
            serde_json::from_value::<ExecutorBuild>(json!("*")).unwrap(),
            ExecutorBuild::Any
        );
    }

    #[test]
    fn the_sandbox_a_turn_runs_in_is_lowered_to_the_environment_and_then_to_what_can_be_enforced() {
        use SandboxMode::*;
        let all = [ReadOnly, Workspace, Open];
        assert_eq!(sandbox_for_turn(Open, Open, &all), Some(Open));
        assert_eq!(sandbox_for_turn(Open, Workspace, &all), Some(Workspace));
        assert_eq!(
            sandbox_for_turn(Workspace, Open, &[Open]),
            None,
            "none at or below workspace can be enforced"
        );
        assert_eq!(sandbox_for_turn(Open, Open, &[ReadOnly, Open]), Some(Open));
        assert_eq!(
            sandbox_for_turn(Workspace, Open, &[ReadOnly, Open]),
            Some(ReadOnly)
        );
        assert_eq!(sandbox_for_turn(Open, Open, &[]), None);
        assert!(ReadOnly < Workspace && Workspace < Open);
    }

    #[test]
    fn the_arrangement_types_are_five_and_written_in_snake_case() {
        let written: Vec<_> = ArrangementType::ALL
            .iter()
            .map(|kind| kind.to_string())
            .collect();
        assert_eq!(
            written,
            [
                "separated",
                "key_separated",
                "credential_shared",
                "handed_over",
                "together"
            ]
        );
        assert!(!ArrangementType::Together.needs_consent());
        assert_eq!(
            serde_json::to_value(ArrangementType::HandedOver).unwrap(),
            json!("handed_over")
        );
    }

    #[test]
    fn a_descriptor_written_before_the_context_report_had_words_reads_its_boolean() {
        let mut older = codex();
        older["capabilities"]["usage_context"] = json!(true);
        let read: AgentDescriptor = serde_json::from_value(older.clone()).unwrap();
        assert_eq!(read.capabilities.usage_context, UsageContext::SizeOnly);
        older["capabilities"]["usage_context"] = json!(false);
        let read: AgentDescriptor = serde_json::from_value(older.clone()).unwrap();
        assert_eq!(read.capabilities.usage_context, UsageContext::None);
        older["capabilities"]["usage_context"] = json!("breakdown");
        let read: AgentDescriptor = serde_json::from_value(older).unwrap();
        assert!(read.capabilities.usage_context.reported());
        assert_eq!(
            serde_json::to_value(read.capabilities.usage_context).unwrap(),
            json!("breakdown")
        );
    }

    #[test]
    fn a_harness_that_says_nothing_of_plan_mode_is_read_as_an_item_that_proceeds_with_a_message() {
        let mut read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        read.capabilities.plan_mechanics = None;
        assert_eq!(
            read.capabilities.plan_style(),
            Some(PlanMechanics {
                proposal: PlanProposal::Item,
                proceed: PlanProceed::Message
            })
        );
        read.capabilities.plan_mode = false;
        assert_eq!(read.capabilities.plan_style(), None);
    }

    #[test]
    fn the_sign_ins_of_a_harness_are_its_list_and_the_single_one_an_older_descriptor_has() {
        let read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        assert_eq!(read.native_providers().len(), 1);
        let mut several = read.clone();
        let mut second = several.native_provider.clone().unwrap();
        second.id = Some("other".to_owned());
        second.title = "Other subscription".to_owned();
        several.native_providers = vec![second.clone(), several.native_provider.clone().unwrap()];
        let ids: Vec<_> = several
            .native_providers()
            .iter()
            .map(|each| each.id())
            .collect();
        assert_eq!(
            ids,
            ["codex", "other"],
            "the single one first, nothing twice"
        );
        let normal = several.normalized();
        assert_eq!(normal.native_providers.len(), 2);
        assert_eq!(
            normal.native_provider.as_ref().map(NativeProvider::id),
            Some("codex")
        );
        assert_eq!(normal.check(), Ok(()));
    }

    #[test]
    fn words_in_another_language_are_found_where_they_are_listed_and_fall_back_to_the_declared_ones()
     {
        let mut with = codex();
        with["text"] =
            json!({ "zh-CN": { "harness.name": "代码助手", "native_provider.title": "" } });
        let read: AgentDescriptor = serde_json::from_value(with).unwrap();
        assert_eq!(read.name_in("zh-CN"), "代码助手");
        assert_eq!(read.name_in("en"), "Codex");
        assert_eq!(read.text_in("zh-CN", "native_provider.title"), None);
        assert_eq!(read.text_in("fr", "harness.name"), None);
    }

    #[test]
    fn an_override_cannot_give_an_adapter_without_a_reviewer_one() {
        let mut read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        read.capabilities.approval_reviewer = None;
        let changed = read
            .with_overrides(
                &json!({"capabilities":{"approval_reviewer":{"model":"x","settable":true}}}),
            )
            .unwrap();
        assert!(changed.capabilities.approval_reviewer.is_none());
    }

    #[test]
    fn an_override_cannot_add_auto_to_the_modes_an_adapter_claims() {
        let read: AgentDescriptor = serde_json::from_value(codex()).unwrap();
        let changed = read
            .with_overrides(&json!({"approval_modes":["ask","on_request","auto","bypass"]}))
            .unwrap();
        assert_eq!(
            changed.approval_modes,
            vec![
                ApprovalMode::Ask,
                ApprovalMode::OnRequest,
                ApprovalMode::Auto
            ]
        );
        let mut claimed = read.clone();
        claimed.approval_modes = vec![ApprovalMode::Ask];
        let changed = claimed
            .with_overrides(&json!({"approval_modes":["ask","auto"]}))
            .unwrap();
        assert_eq!(changed.approval_modes, vec![ApprovalMode::Ask]);
    }

    #[test]
    fn a_descriptor_says_in_its_schema_that_it_may_carry_members_of_its_own() {
        let schema = serde_json::to_value(schemars::schema_for!(AgentDescriptor)).unwrap();
        assert!(schema["properties"]["arrangements"].is_object());
        assert!(
            schema.to_string().contains("^x_"),
            "the schema names the x_ members it allows: {schema}"
        );
    }
}
