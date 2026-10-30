//! Providers with their setup (sign-in methods, options, endpoints, presets), the arrangement types their owner allows and their runtime with its pending updates; models with the name that is sent and the name that is shown, and the speeds and access programs each offers and has enabled; profiles; hosts with what they do when full; machines with their policy and who changes each key; templates of both kinds with their sources, their imports and how an exec node is launched; and MCP servers: presets, and those imported from a harness's own configuration.
//!
//! A resource is owned, listed, shared and removed by the same means
//! whatever it is: it has a first owner, a home (an org at any level, or none
//! for the system's), a page of permissions, and a revision that a change
//! names as `expected_revision`. A [`Provider`] is what a thread's model calls
//! go through, a subscription that a harness signs in to or an API reached with
//! a key; a [`ProviderSetup`] gathers what the page that adds one needs. An
//! [`Agent`] is a profile of a harness, a [`Host`] a machine that makes
//! environments from a [`Template`], a [`Machine`] one that a person
//! registered, and an [`McpServer`] a tool server that environments can be
//! given. A request that is the same for all of them answers with a
//! [`Resource`].

use std::collections::BTreeMap;

use orch_types::{Field, Id, Json, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{NewResourceAccess, ResourceAccess};
use crate::approvals::{ApproverScope, Risk};
use crate::binding::ArrangementType;
use crate::common::{PageQuery, absent_or_null};
use crate::documents::Document;
use crate::environment::{
    Amounts, Environment, EnvironmentRef, HostStorage, McpScope, PendingUpdate, Placement,
    RecycleBody, RecycleReply, TemplateRef, UpdatePreflightBody, WhenFull,
};
use crate::forks::{
    AppliedChangeOptions, Estimate, ForkCreateBody, ProviderUpdatePlan, UpdateConfirmQuery,
};
pub use crate::machine_programs::{
    MachineProgram, MachineProgramKey, MachineProgramKind, MachineProgramRegistration,
    MachinePrograms, MachineProgramsCommand,
};
use crate::mounts::Mount;
use crate::nodes::{JobRef, NodeRef};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;
use crate::workspace::Job;

/// The operating systems a machine runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachineOs {
    /// Linux.
    Linux,
    /// macOS.
    Macos,
    /// Windows.
    Windows,
}

/// A machine, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineRef {
    /// The machine's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Whether it is connected now.
    pub online: bool,
    /// Its operating system.
    pub platform_os: MachineOs,
    /// Whether it is a person's own machine or one the system manages.
    pub placement: Placement,
}

/// A host, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostRef {
    /// The host's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Where it comes from, as short names.
    pub address: String,
}

/// Whether a provider is paid for by subscription or by use of an API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// A subscription the harness signs in to.
    Subscription,
    /// An API reached with a key.
    Api,
}

/// Where a provider stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// It has to be set up.
    NeedsSetup,
    /// Ready.
    Ready,
    /// It has no room left for now.
    Exhausted,
    /// Something is wrong.
    Error,
    /// Its machine is not reachable.
    Offline,
    /// Switched off.
    Disabled,
}

/// A provider, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRef {
    /// The provider's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// A subscription or an API.
    pub kind: ProviderKind,
    /// The harness a subscription signs in to.
    pub harness_kind: Option<String>,
    /// The same harness, named.
    pub harness: Option<HarnessKindName>,
    /// Where it comes from, as the short names of its home and its owner.
    pub address: String,
    /// Whether it is made by the system or on a person's machine.
    pub placement: Placement,
    /// Where it stands.
    pub status: ProviderStatus,
}

/// An agent profile, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentRef {
    /// The profile's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Where it comes from, as the short names of its home and its owner.
    pub address: String,
    /// The harness it runs.
    pub harness_kind: String,
    /// The same harness, named.
    pub harness: HarnessKindName,
    /// The machine its harness runs on, if it runs on one.
    pub machine: Option<MachineRef>,
    /// Whether it is switched off.
    pub disabled: bool,
}

// The members of a provider's facts and of its sources' descriptors and choices that the API shows as the
// provider's contract writes them, once, for the API, the engine and the sources alike. A balance, a row of
// spending and a descriptor keep the source's own `x_` members, as the agent contract does.
pub use orch_provider_contract::address::{AddressAuth, SourceAddress};
pub use orch_provider_contract::choice::{SourceBy, SourceChoiceLastError};
pub use orch_provider_contract::facts::{
    AvailableResets, Balance, BalancePart, ExtraUsage, FactMember, MemberState, ResetCredit, Spend,
    SpendPeriod, UsageWindow,
};
/// What a model can be run with at a provider, and a model as a list gives it, written once with the
/// provider's contract.
pub use orch_provider_contract::models::{
    ModelOffered, ModelParameters, ModelSuggestion, Rate, RateAbove, ServiceTier,
};
pub use orch_provider_contract::source::{
    SourceCredential, SourceDescriptor, SourceDescriptorFacts, SourceDescriptorModels,
    SourceDescriptorPresents, SourceErrorCode,
};

/// Which pool a template belongs to, and so what it makes: environments, or the runtimes of
/// providers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitKind {
    /// Environments.
    Environment,
    /// The runtimes of providers.
    Provider,
}

/// Whether a machine may still be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachineStatus {
    /// It is registered.
    Active,
    /// Its registration was withdrawn and its session dropped.
    Revoked,
}

/// A machine that is registered with the system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Machine {
    /// When its identity was permanently declared lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_at: Option<Time>,
    /// The physical program's actual role editing support under its current effective policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role_management: Option<MachineRoleCapability>,
    /// Observed installation and exact lifecycle commands for this instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub installation: Option<crate::machine_programs::MachineInstallation>,
    /// Physical program versions and latest Update now progress.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software_update: Option<ComponentUpdate>,
    /// The one resource page of a physical machine; internal unit connectors have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::access::ResourceAccess>,
    /// The host facade supplied by this same machine's node role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<HostRef>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: MachineRef,
    /// Whose it is.
    pub owner: Option<UserRef>,
    /// Its processor architecture.
    pub platform_arch: String,
    /// The version of the connector on it.
    pub connector_version: String,
    /// The version of the protocol it speaks.
    pub protocol_version: u32,
    /// The roles its connector announces, such as `agent` and `environment`.
    pub roles: Vec<String>,
    /// Whether it may still be used.
    pub status: MachineStatus,
    /// When it was last reachable.
    pub last_seen_at: Option<Time>,
    /// The address it last connected from.
    pub last_ip: Option<String>,
    /// When its certificate ends.
    pub cert_not_after: Time,
    /// What it reported about itself.
    pub facts: Json,
    /// The providers registered from it.
    pub providers: Vec<ProviderRef>,
    /// The agent profiles registered from it.
    pub agents: Vec<AgentRef>,
    /// The environments registered from it.
    pub environments: Vec<EnvironmentRef>,
    /// Sandbox readiness at each registered environment's home, before any thread exists.
    #[serde(default)]
    pub environment_sandboxes: Vec<crate::environment::EnvironmentSandboxReadiness>,
}

/// The running hello version compared with the server's recommendation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ComponentUpdate {
    /// Version reported by the currently executing program.
    pub running_version: String,
    /// Build served by this server and announced in ready.versions.
    pub recommended_version: String,
    /// True only when the recommendation is a newer semantic version.
    pub behind: bool,
    /// current, queued, updating, refused or failed; pending states persist until reconnect.
    pub status: String,
    /// A machine's refusal or verification failure, in its own words.
    pub detail: Option<String>,
    /// The local command, including when local policy refuses the web action.
    pub command: String,
}

/// A limit of the plan that has been reached.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFactsLimitReached {
    /// Which limit, in the vendor's words.
    #[serde(rename = "type")]
    pub kind: String,
    /// When it lifts.
    pub resets_at: Option<Time>,
}

/// What a credential gives a harness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CredentialYields {
    /// A login that the harness keeps.
    Login,
    /// A token.
    Token,
    /// A key.
    Key,
    /// Nothing: the endpoint takes none.
    #[serde(rename = "none")]
    Nothing,
}

/// How a provider was given its credential.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFactsCredential {
    /// The id of the sign-in method that gave it; `null` for a key.
    pub method: Option<String>,
    /// What it is.
    pub yields: CredentialYields,
    /// When a token lapses, where that is known.
    pub expires_at: Option<Time>,
    /// When the credential was issued, where known.
    #[serde(default)]
    pub issued_at: Option<Time>,
}

/// What a provider's sources report about it: its plan and its limits, what is left to spend and what was
/// spent, and who its credential belongs to. Each member is in one of four states, which `facts_state` beside
/// it says: a member that is not told is `null` or `[]` like one told empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFacts {
    /// The plan.
    pub plan: Option<String>,
    /// The windows of the plan and how much of each is used.
    pub windows: Vec<UsageWindow>,
    /// A limit that has been reached.
    pub limit_reached: Option<ProviderFactsLimitReached>,
    /// Who it is signed in as, when the harness reports it; for example an email address.
    pub identity: Option<String>,
    /// How it was given its credential.
    pub credential: Option<ProviderFactsCredential>,
    /// Resets still available, where the vendor tells them.
    #[serde(default)]
    pub available_resets: Option<AvailableResets>,
    /// Spending beyond the included plan.
    #[serde(default)]
    pub extra_usage: Option<ExtraUsage>,
    /// What the account, the plan or the key still holds to be spent, each in its own unit.
    #[serde(default)]
    pub balances: Vec<Balance>,
    /// What has been spent, each row over its own period.
    #[serde(default)]
    pub spend: Vec<Spend>,
    /// The vendor's explicit permission for ordinary included usage; null is no proof of recovery.
    #[serde(default)]
    pub ordinary_usage_allowed: Option<bool>,
    /// True only when the vendor authenticated this facts observation with the provider's credential.
    #[serde(default)]
    pub credential_verified: Option<bool>,
    /// The last successfully discovered model list; suggestions remain separate from the accepted allowlist.
    #[serde(default)]
    pub models: Option<Vec<ModelSuggestion>>,
    /// The last successful observation of each source group; absent groups were never observed.
    #[serde(default)]
    pub field_times: std::collections::BTreeMap<String, Time>,
}

/// Where each member of a provider's facts stands, and the list of its models.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderFactsState {
    /// Who the credential belongs to.
    pub identity: MemberState,
    /// How the credential was given, and when it lapses.
    pub credential: MemberState,
    /// Whether a read authenticated with the credential.
    pub credential_verified: MemberState,
    /// The plan.
    pub plan: MemberState,
    /// The usage windows.
    pub windows: MemberState,
    /// A limit that has been reached.
    pub limit_reached: MemberState,
    /// Whether ordinary use is admitted now.
    pub ordinary_usage_allowed: MemberState,
    /// What is left to be spent.
    pub balances: MemberState,
    /// What has been spent.
    pub spend: MemberState,
    /// Spending beyond the plan.
    pub extra_usage: MemberState,
    /// Resets that can be spent.
    pub available_resets: MemberState,
    /// The models the models source lists: unsupported where it lists none of its own (they are added by
    /// name).
    pub models: MemberState,
}

/// Where one element of a provider comes from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SourceChoice {
    /// What implements it.
    pub by: SourceBy,
    /// The built-in source or the program, by name; for the default, the built-in source it follows
    /// (`null` where that is nothing).
    pub name: Option<String>,
    /// The address, for a source at one.
    pub address: Option<SourceAddress>,
    /// What its managers gave for its fields that are no secrets.
    pub config: BTreeMap<String, Json>,
    /// Its secret fields that have a value, by name; never their values.
    pub secrets: Vec<String>,
    /// What the source says it is: what it reports, its fields, what it implements.
    pub descriptor: Option<SourceDescriptor>,
    /// When it was last asked what it is.
    pub described_at: Option<Time>,
    /// Its last error, until a read succeeds.
    pub last_error: Option<SourceChoiceLastError>,
}

/// Where a provider's details come from: its facts (with the resets they list), its models, and what presents
/// its key.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSources {
    /// The source of its facts and of the resets they list.
    pub facts: SourceChoice,
    /// The source of its models.
    pub models: SourceChoice,
    /// What presents its key; `null` where the key is presented as its endpoints' options say.
    pub presenter: Option<SourceChoice>,
}

/// How a sign-in method goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginMethodKind {
    /// It leads the person through something and reports what to show and what to ask as it goes.
    Interactive,
    /// It takes a credential the person already has, and is done in one request.
    Import,
}

/// What a sign-in method gives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginYields {
    /// A login that the harness keeps.
    Login,
    /// A token.
    Token,
}

/// One way of signing a subscription in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoginMethod {
    /// What a request names it by.
    pub id: String,
    /// What it is called.
    pub title: String,
    /// A sentence on what it does.
    pub summary: String,
    /// Whether it leads the person through steps or takes what they already have.
    pub kind: LoginMethodKind,
    /// What it asks for up front.
    pub fields: Vec<Field>,
    /// What it gives.
    pub yields: LoginYields,
    /// How long what it gives lasts, where that is known.
    pub lifetime_secs: Option<i64>,
    /// What has to be switched on first for the method to work, as a sentence; `null` when
    /// nothing.
    pub requires: Option<String>,
    /// Whether it is the one to suggest first.
    pub recommended: bool,
}

/// Where a sign-in stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginProgress {
    /// None is under way.
    Idle,
    /// One is being started.
    Starting,
    /// It waits for the person.
    AwaitingUser,
    /// It succeeded.
    Completed,
    /// It failed.
    Failed,
    /// It ran out of time.
    Expired,
}

/// What a sign-in shows the person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum LoginShowKind {
    /// An address to open.
    Uri,
    /// A code to enter there.
    Code,
    /// A command to run.
    Command,
    /// A sentence.
    Text,
}

/// One thing a running sign-in shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct LoginShow {
    /// What it is.
    pub kind: LoginShowKind,
    /// What it is called.
    pub label: String,
    /// The address, the code, the command or the sentence.
    pub value: String,
}

/// What a running sign-in shows and asks for.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct LoginState {
    /// Where it stands.
    pub state: LoginProgress,
    /// The method running, or the one last used.
    pub method: Option<String>,
    /// What the person has to see now.
    pub shows: Vec<LoginShow>,
    /// What the person has to give back now, if anything.
    pub asks: Option<Vec<Field>>,
    /// When it runs out.
    pub expires_at: Option<Time>,
    /// What went wrong.
    pub error: Option<String>,
}

/// One endpoint of an API provider's upstream and the protocol it speaks there. One key serves
/// all of an upstream's endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Upstream {
    /// The protocol's name.
    pub protocol: String,
    /// The address: `https`, unless the setting `providers.allow_http_upstreams` allows `http`.
    pub base_url: String,
    /// What the protocol's endpoint options allow: how the key is presented (`auth`: `bearer`,
    /// `x-api-key`, `header` or `none`), `headers`, `query`, and `secret_headers`, which are
    /// given as names and whose values are written with `ProviderSecretSet`.
    pub options: Json,
}

/// A named list of endpoints that fills the form of a new provider in. A provider does not refer
/// to it afterwards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderPreset {
    /// Its name.
    pub name: String,
    /// A note on it.
    pub note: Option<String>,
    /// Where its documentation is.
    pub docs_url: Option<String>,
    /// The endpoints.
    pub upstreams: Vec<Upstream>,
    /// An ordinary template recipe offered with this preset; its home is the chosen provider home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<TemplateCreateBody>,
    /// An ordinary model entry offered with this preset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<ProviderModelCreateBody>,
    /// The suggested vector length for an embeddings model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dimensions: Option<u32>,
}

/// A harness, named by its kind and by what it is called. Every payload that says which harness a thing
/// belongs to carries one of these beside the kind, so that no screen keeps a table of names.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessKindName {
    /// The harness's kind, such as `codex`.
    pub kind: String,
    /// What it is called, in English.
    pub name: String,
    /// What it is called in the other languages, by language tag (`zh-CN`); the English is the fallback.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub names: BTreeMap<String, String>,
}

impl HarnessKindName {
    /// What the harness is called in `language`.
    #[must_use]
    pub fn in_language(&self, language: &str) -> &str {
        self.names
            .get(language)
            .filter(|name| !name.trim().is_empty())
            .map_or(self.name.as_str(), String::as_str)
    }
}

/// What a form shows of the credential that an arrangement would let leave.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArrangementConsentForm {
    /// The adapter's name for what leaves.
    pub name: String,
    /// What it is called for people.
    pub title: String,
    /// How long a copy lasts, where it has a lifetime.
    pub lifetime_secs: Option<i64>,
}

/// One arrangement type, as a provider's owner is asked about it. Of the types, `separated`,
/// `key_separated`, `credential_shared` and `handed_over` are the ones a provider can allow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ArrangementConsent {
    /// Which type.
    #[serde(rename = "type")]
    pub kind: ArrangementType,
    /// Ticked: on a provider, what its owner allows; in a provider's setup, what is proposed.
    pub allowed: bool,
    /// Whether some harness that can run on this provider can be placed this way.
    pub available: bool,
    /// What it puts into an environment, in one sentence.
    pub puts: String,
    /// For `credential_shared` and `handed_over`: what would leave, with the sign-in the provider
    /// has or the method chosen.
    pub form: Option<ArrangementConsentForm>,
}

/// A harness of which a provider may be signed in, and the ways to do it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupSubscription {
    /// The profile whose harness holds the sign-in.
    pub agent: AgentRef,
    /// The harness.
    pub harness: HarnessKindName,
    /// The ways of signing in.
    pub methods: Vec<LoginMethod>,
    /// What the adapter's options may be: a JSON Schema.
    pub options_schema: Json,
    /// Every type its harness can run in, with those ticked that its adapter proposes for a new
    /// one.
    pub arrangements: Vec<ArrangementConsent>,
    /// The template its runtime would be made from.
    pub template: TemplateRef,
}

/// A harness that a policy treats specially, and the scope of that policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupPolicy {
    /// The harness.
    pub harness: HarnessKindName,
    /// The place that set the policy; `null` is the system.
    pub scope: Option<NodeRef>,
}

/// A protocol that an API provider may speak, and who could use it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupProtocol {
    /// Its name.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// What an endpoint of this protocol is asked for.
    pub endpoint_options: Vec<Field>,
    /// The profiles the caller may use whose harness speaks it.
    pub agents: Vec<AgentRef>,
}

/// A template that can make the runtime of a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupRuntimeTemplate {
    /// The template's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// The driver that makes units from it.
    pub driver: String,
    /// Whose it is; `null` is the system's.
    pub owner: Option<UserRef>,
}

/// Everything the page that adds a provider needs, for the caller, in one answer.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetup {
    /// Usable physical machines and the services each actually declares for the two independent locations.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machines: Option<Vec<ProviderSetupMachine>>,
    /// The harnesses a provider can be signed in to.
    pub subscriptions: Vec<ProviderSetupSubscription>,
    /// Usable profiles whose sign-in capabilities have not been reported yet; never guessed by kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions_unreported: Option<Vec<AgentRef>>,
    /// Named programs which cannot be chosen for sign-in yet, with an actual reason and no guessed capabilities.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subscriptions_unavailable: Option<Vec<ProviderSetupUnavailableSubscription>>,
    /// Sign-ins that the policy `providers.subscription_harnesses` leaves out there, and the scope
    /// that says so. Such a harness is still usable on an API key.
    pub subscriptions_withheld: Vec<ProviderSetupPolicy>,
    /// Kinds whose subscription only its owner may use there (`providers.subscription_sharing`),
    /// and the scope that says so: who may use it is then fixed at the owner.
    pub subscriptions_unshared: Vec<ProviderSetupPolicy>,
    /// The protocols an API provider may speak.
    pub protocols: Vec<ProviderSetupProtocol>,
    /// The presets.
    pub presets: Vec<ProviderPreset>,
    /// The same as for a subscription, for an API provider: `separated` and `key_separated`
    /// ticked, in which the key stays home.
    pub api_arrangements: Vec<ArrangementConsent>,
    /// The caller's machines that may keep a key.
    pub key_machines: Vec<MachineRef>,
    /// What `providers.allow_http_upstreams` resolves to for the org that would be its home.
    pub http_allowed: bool,
    /// The templates of kind `provider` that the caller may use for a provider's runtime, beside
    /// the one it would take by itself.
    pub runtime_templates: Vec<ProviderSetupRuntimeTemplate>,
    /// Where a new provider's details may come from: the built-in sources, the programs of the
    /// caller's machines that may keep a key, and the programs of the provider templates the caller
    /// may use.
    #[serde(default)]
    pub sources: Vec<ProviderSetupSource>,
}

/// A named sign-in program retained as an unavailable choice until it can actually be used.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupUnavailableSubscription {
    /// A public system program or a program of a profile the caller may use.
    pub harness: HarnessKindName,
    /// The usable profile, if there is one. A private profile is never named here.
    pub agent: Option<AgentRef>,
    /// Why it cannot yet supply a subscription sign-in.
    pub reason: ProviderSetupSubscriptionUnavailableReason,
}

/// Absence of a report is different from a report which declares no native sign-in support.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSetupSubscriptionUnavailableReason {
    /// Add an agent profile, or obtain Use on one that already exists.
    ProfileNeeded,
    /// The profile has not reported a descriptor; starting it or registering its adapter reads one.
    NotReported,
    /// Its descriptor declares no native subscription provider.
    Unsupported,
}

/// A machine offered by the common provider creation flow; capability comes from its reports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupMachine {
    /// Credential arrangements its physical policy permits; absent means no policy was reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangements: Option<Vec<ArrangementType>>,
    /// Its currently reported allowed model addresses; absent means the policy was not reported.
    /// `*` allows every address; otherwise these are exact endpoint base addresses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstreams: Option<Vec<String>>,
    /// Its readable reference.
    pub machine: MachineRef,
    /// It declares an attach lifecycle for directory runtimes.
    pub directory_runtime: bool,
    /// Roles currently enabled on it.
    pub roles: Vec<String>,
    /// Adapters it reports, for a native credential keeper.
    pub harnesses: Vec<String>,
}

/// A source a new provider's details may come from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSetupSource {
    /// How it is chosen: `builtin` or `program`.
    pub by: SourceBy,
    /// Its name.
    pub name: String,
    /// What it says it is; `null` for a template's program that its runtime has not run yet, whose
    /// command is shown instead.
    pub descriptor: Option<SourceDescriptor>,
    /// For a built-in source, the hosts of the endpoints it is the default for (`openrouter.ai`).
    pub default_for: Vec<String>,
    /// For a program of a person's machine: that machine, which then keeps the key.
    pub machine: Option<MachineRef>,
    /// For a program of a provider template: that template, which choosing the program sets for the
    /// runtime.
    pub template: Option<TemplateRef>,
    /// For a template's program that its runtime has not run yet: the program and its arguments.
    pub command: Option<Vec<String>>,
}

/// Where a model came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ModelSource {
    /// A manager wrote it.
    Manual,
    /// Discovery listed it and a manager accepted it.
    Suggested,
}

/// A model that a provider lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderModel {
    /// The entry's identifier.
    pub id: Id,
    /// Its routing name: the exact name sent upstream, in whatever form that upstream takes
    /// (`gpt-6-sol`, or `openai/gpt-6-sol` through a gateway that names models by vendor). It is
    /// the model's identity.
    pub model_name: String,
    /// Its reported or manager-supplied display name (`GPT-6 Sol (A)`). Empty means no display
    /// name was supplied; a routing identifier is not silently substituted for it.
    pub display_name: String,
    /// What it can be run with here.
    pub parameters: ModelParameters,
    /// What its tokenizer comes to, for the estimates where nobody counted.
    pub token_estimate: crate::programs::TokenEstimate,
    /// The groups its managers put it in (`frontier`, `small`): names of their own, by which a cap,
    /// an allowlist or an entry's option names several models at once as `@<group>`.
    pub groups: Vec<String>,
    /// Whether it may be chosen.
    pub enabled: bool,
    /// Where it came from.
    pub source: ModelSource,
}

/// The most groups one model can be in.
pub const MODEL_GROUPS_MAX: usize = 8;
/// The most characters in the name of a group.
pub const MODEL_GROUP_NAME_MAX: usize = 40;

/// Whether `name` can be the name of a group of models: 1 to [`MODEL_GROUP_NAME_MAX`] of `a-z`, `0-9`,
/// `-` and `_`, beginning with a letter or a digit.
#[must_use]
pub fn is_model_group_name(name: &str) -> bool {
    let mut characters = name.chars();
    let Some(first) = characters.next() else {
        return false;
    };
    name.chars().count() <= MODEL_GROUP_NAME_MAX
        && (first.is_ascii_lowercase() || first.is_ascii_digit())
        && characters.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// A conversation that a harness already holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NativeSession {
    /// The harness's own reference to it.
    pub native_session_ref: String,
    /// Its title.
    pub title: Option<String>,
    /// The directory it ran in.
    pub cwd: Option<String>,
    /// When it last changed.
    pub updated_at: Option<Time>,
    /// The thread that adopted it, if one did.
    pub adopted_thread_id: Option<Id>,
}

/// An environment that holds a provider's credential for a while.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderHeldBy {
    /// The environment, whose reference names its owner.
    pub environment: EnvironmentRef,
    /// Since when.
    pub since: Time,
}

/// What an org still accepts of a subscription, and whether anyone but its owner may use it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSubscription {
    /// Whether the org still accepts its kind (`providers.subscription_harnesses`).
    pub accepted: bool,
    /// Whether anyone but its first owner may use it (`providers.subscription_sharing`).
    pub shareable: bool,
}

/// The template a provider's runtime is made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRuntimeTemplate {
    /// The template's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Whose it is; `null` is the system's.
    pub owner: Option<UserRef>,
}

/// The runtime a managed provider has: the unit in which its harness holds the credential and the
/// conversations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRuntime {
    /// The effective network of this provider's runtime, independently of any environment's traffic.
    pub network: crate::environment::NetworkState,
    /// Where it stands.
    pub state: String,
    /// The host it is on.
    pub host: Option<HostRef>,
    /// The driver that made it.
    pub driver: String,
    /// What it is made from.
    pub template: ProviderRuntimeTemplate,
    /// How many units it has had.
    pub generation: i64,
    /// What it is behind in; empty means up to date. An update applies it; nothing does by itself
    /// unless `runtime.update` is `on_start`.
    pub updates: Vec<PendingUpdate>,
    /// Whether it cannot attach until it is updated.
    pub update_required: bool,
    /// When something last happened in it.
    pub last_active_at: Option<Time>,
    /// When the idle timeout will stop it.
    pub idle_stop_at: Option<Time>,
    /// When native renewal is due from its known lapse, or its last successful renewal when unknown;
    /// `null` when renewal is off or the credential is an imported token.
    pub renew_at: Option<Time>,
    /// A job on it now: its provision or its recreation.
    pub job: Option<JobRef>,
}

/// How a harness process started for a provider stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HarnessStatus {
    /// Its machine is not reachable.
    Offline,
    /// Being started.
    Starting,
    /// Up.
    Online,
    /// It failed.
    Error,
}

/// A harness process that was started for a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessState {
    /// The profile it runs.
    pub agent: AgentRef,
    /// The machine it runs on.
    pub machine: MachineRef,
    /// Where it stands.
    pub status: HarnessStatus,
    /// A sentence on it, where there is one.
    pub status_detail: Option<String>,
    /// Its version.
    pub harness_version: Option<String>,
    /// How many sessions it holds open.
    pub sessions_open: i64,
    /// When something last happened in it.
    pub last_active_at: Option<Time>,
    /// The latest reported program error, retained across a supervised recovery.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_error: Option<HarnessProgramError>,
}

/// A historical program error, distinct from the harness's current status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessProgramError {
    /// When Orch received the error report.
    pub at: Time,
    /// The program's reason, when available.
    pub detail: Option<String>,
}

/// The runtime's actual declaration and one reported machine-policy value, without a placement inference.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRuntimeSupport {
    /// Each resource's actual enforcement support on the runtime's hosting machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_limits: Option<orch_driver_contract::descriptor::ResourceLimitSupport>,
    /// Actual harness-home choices; the directory declaration does not require an environment role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_storage: Option<orch_driver_contract::descriptor::EnvironmentDataStorage>,
    /// The runtime's physical machine, so home choices use that machine's roots and facts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<Id>,
    /// The declared unit driver; absent for directory attachment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
    /// The resolved home choice, visible to the runtime's manager.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<orch_driver_contract::runtime::EnvironmentDataSource>,
    /// Unit start/stop or directory attach/detach; null until declared.
    pub lifecycle: Option<orch_driver_contract::descriptor::Lifecycle>,
    /// Process-group support and its technical reason, as the running implementation declares it.
    pub commands_after_start: Option<orch_driver_contract::descriptor::OperationSupport>,
    /// The physical machine's environment.after_up switch; null when no policy has been reported.
    pub after_up: Option<bool>,
    /// Whether Orch can change the native sign-in in the explicitly selected harness home.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sign_in: Option<ProviderSignInSupport>,
}

/// The home and physical-machine permission behind a provider's native sign-in actions.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderSignInSupport {
    /// The selected harness home was made by Orch, rather than borrowed from the machine's owner.
    pub home_owned: bool,
    /// The credential keeper's provider.signin switch; null until it has reported its policy.
    pub policy_allowed: Option<bool>,
    /// The home is Orch's, or its keeper explicitly permits sign-in changes there.
    pub allowed: bool,
}

/// A provider: a subscription the harness signs in to, or an API reached with a key. Every
/// provider selects its credential keeper independently from its unit or directory runtime.
/// Placement is a derived label for that runtime, rather than a stored capability or policy.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct Provider {
    /// Window projections on the single-provider read; absent on lists and without View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plan_forecasts: Option<BTreeMap<String, orch_provider_contract::forecast::UsageForecast>>,
    /// A lost or explicitly removed runtime awaits its manager's reviewed re-creation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_unavailable: Option<String>,
    /// The caller's applicable provider caps and observed seven-day model performance; absent without View.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub choice_usage: Option<ProviderChoiceUsage>,
    /// Where credential operations run, independently of the runtime machine. Visible to managers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_keeper: Option<MachineRef>,
    /// The runtime services and effective physical-machine switch that apply to this provider.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_support: Option<ProviderRuntimeSupport>,
    /// Its current driver and measured use, for the manager's runtime change form.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_changes: Option<orch_driver_contract::runtime::RuntimeChanges>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: ProviderRef,
    /// Its first owner.
    pub owner: UserRef,
    /// How many people own it.
    pub owners: i64,
    /// The root org of its home.
    pub org_id: Id,
    /// The org it belongs to, at any level: whose defaults its page follows, whose admins
    /// administer it, and which of the subscription policies govern it. It does not limit the
    /// threads that run on it.
    pub home: NodeRef,
    /// For a subscription, the profile whose harness holds the login. It is fixed at creation.
    pub agent: Option<AgentRef>,
    /// The types in which its owner allows it to be used, among those possible for it.
    pub arrangements: Vec<ArrangementConsent>,
    /// The environment its credential is handed over to, while it is.
    pub held_by: Option<ProviderHeldBy>,
    /// For a managed one, its runtime's own values for the keys of scope `provider`: template, host,
    /// resources, idle stop, update, and after how long a stopped runtime is started to renew.
    pub runtime_config: Json,
    /// For an API provider, each endpoint of its upstream. Empty for a subscription.
    pub upstreams: Vec<Upstream>,
    /// Names of endpoint secrets with a value, and runtime:NAME for runtime secrets; never values.
    pub secrets_set: Vec<String>,
    /// For a subscription, what its adapter's `options_schema` allows.
    pub config: Json,
    /// For a subscription, what its org accepts of it; `null` for an API provider.
    pub subscription: Option<ProviderSubscription>,
    /// What it can serve: those protocols, and any that a registered translator adds.
    pub protocols: Vec<String>,
    /// Whether it has a key; false also for an upstream that takes none.
    pub key_set: bool,
    /// For an API provider, the connected machine that keeps the key; `null` means the server does.
    pub key_machine: Option<MachineRef>,
    /// Where its runtime is.
    pub machine: Option<MachineRef>,
    /// A sentence on its status, where there is one.
    pub status_detail: Option<String>,
    /// When this status began; facts reads do not move the time of a refusal.
    pub status_at: Time,
    /// What its harness or endpoint reports, with the overrides applied.
    pub facts: ProviderFacts,
    /// Where each member of `facts` stands: unsupported, not read yet, empty or told.
    #[serde(default)]
    pub facts_state: ProviderFactsState,
    /// Where its details come from: the source of its facts, of its models, and what presents its key.
    #[serde(default)]
    pub sources: ProviderSources,
    /// The names of the members of `facts` that come from overrides.
    pub facts_overridden: Vec<String>,
    /// When the facts were taken.
    pub facts_at: Option<Time>,
    /// When a source read was last attempted, including a refusal or an unknown answer.
    pub facts_requested_at: Option<Time>,
    /// Who last used a reset, while its resulting windows still stand.
    #[serde(default)]
    pub last_reset: Option<ProviderLastReset>,
    /// The models it lists.
    pub models: Vec<ProviderModel>,
    /// How many turns run on it now.
    pub running_threads: i64,
    /// For a managed one, its own runtime, when one exists.
    pub runtime: Option<ProviderRuntime>,
    /// The harness processes started for it.
    pub harnesses: Vec<HarnessState>,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// When it was made.
    pub created_at: Time,
    /// When it was last changed.
    pub updated_at: Time,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// Permitted facts used by provider and model choices without a request for each option.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderChoiceUsage {
    /// Enabled caps defined on this provider that apply to this caller, with per-person precedence resolved.
    pub caps: Vec<crate::usage::CapStatus>,
    /// Observed model, effort, tier and program combinations in the current seven UTC days.
    pub performance: Vec<crate::usage::UsageComparison>,
    /// When these observations were read.
    pub observed_at: Time,
}

/// An agent profile: which harness to run, and how. It has no runtime; the harness process is
/// started inside a provider's runtime, or inside an environment where the thread's arrangement
/// puts it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Agent {
    /// Readable providers that keep this profile or currently use it for a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub providers: Option<Vec<ProviderRef>>,
    /// Models of those readable providers, retaining the provider needed to open each entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<Vec<AgentModel>>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: AgentRef,
    /// Its first owner; `null` for a profile of the system.
    pub owner: Option<UserRef>,
    /// The root org of its home.
    pub org_id: Option<Id>,
    /// The org it belongs to, at any level; `null` for a profile of the system.
    pub home: Option<NodeRef>,
    /// Which build of the harness to run, and how.
    pub profile: Json,
    /// What its adapter declares about the harness as last reported, with overrides applied.
    pub descriptor: Json,
    /// The names of the members of `descriptor` that come from overrides.
    pub descriptor_overridden: Vec<String>,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// When it was made.
    pub created_at: Time,
    /// When it was last changed.
    pub updated_at: Time,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// One model of a provider associated with an agent profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentModel {
    /// The provider whose model entry this is.
    pub provider_id: Id,
    /// The ordinary model entry.
    #[serde(flatten)]
    pub model: ProviderModel,
}

/// What a driver on a host can make there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostDriver {
    /// Its name.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// What it makes: environments, providers' runtimes, or both.
    pub creates: Vec<UnitKind>,
    /// The values a template's `runtime` member may take on this host, such as the container
    /// runtimes it has.
    pub runtime_classes: Vec<String>,
    /// Each of those runtimes as the host declares it: what holds a unit in, what is offered and what is
    /// not under it, and the limits a person or an agent is told. The driver's default one is `default_runtime`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub runtimes: Vec<orch_driver_contract::isolation::RuntimeClass>,
    /// The runtime a unit is made under when nothing names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_runtime: Option<String>,
    /// Optional operations and the actual reason for an unavailable choice.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub availability: BTreeMap<String, orch_driver_contract::descriptor::OperationSupport>,
}

/// What a host does when it is full, and whether that is its own answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct HostWhenFull {
    /// What it does when it is full or declares too little room for a new environment.
    pub value: WhenFull,
    /// Whether that is its own answer; otherwise it is the system's (`hosts.when_full`).
    pub own: bool,
}

/// How many units run on a host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct HostRunning {
    /// The runtimes of providers.
    pub provider: i64,
    /// Environments.
    pub environment: i64,
}

/// A machine that runs `orch-node`, on which environments are made from templates. It is a
/// resource: owned, with a home (an org at any level, or none for the system's), and used by
/// whoever its page names, wherever they work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Host {
    /// When this host was permanently declared lost.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lost_at: Option<Time>,
    /// Whether declarations reach the resolved fullness setting; absent on older responses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full: Option<bool>,
    /// Latest measured memory and CPU; absent before the first load sample.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<HostUsage>,
    /// Existing stopped environment units, distinct from the running count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_environments: Option<i64>,
    /// The recorded offline transition; absent while online or before one has been observed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offline_since: Option<Time>,
    /// This host's node executable, distinct from the connectors of its units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub software_update: Option<ComponentUpdate>,
    /// Retained environment files, visible only to this host's managers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_data: Vec<HostKeptData>,
    /// Owned installed-file images, including archived environments without a unit, for host readers.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub kept_layers: Vec<HostKeptLayers>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: HostRef,
    /// Its first owner; `null` for the system's.
    pub owner: Option<UserRef>,
    /// The root org of its home.
    pub org_id: Option<Id>,
    /// The org it belongs to, at any level; `null` for the system's.
    pub home: Option<NodeRef>,
    /// The machine it is.
    pub machine: MachineRef,
    /// Its labels.
    pub labels: Vec<String>,
    /// The drivers it has.
    pub drivers: Vec<HostDriver>,
    /// Its saved declaration of host ports: the address callers are given, where it listens and the
    /// ranges its openings take numbers from; `null` until a manager saves one.
    pub ports: Option<crate::ports::HostPorts>,
    /// The revision of the declaration its node acknowledged.
    pub ports_revision: Revision,
    /// What its node last proposed for the declaration, under its own ceiling; it opens nothing.
    pub ports_proposal: Option<crate::ports::HostPortsProposal>,
    /// What it has.
    pub capacity: Amounts,
    /// What the units running there declare they need.
    pub declared: Amounts,
    /// Whether it takes new environments.
    pub schedulable: bool,
    /// What it does when it is full.
    pub when_full: HostWhenFull,
    /// Its named storages, as it reports them.
    pub storages: Vec<HostStorage>,
    /// How many units run there.
    pub running: HostRunning,
    /// How many environments of the caller's are there.
    pub mine: i64,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// A host's latest load observation, kept separate from declared reservations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostUsage {
    /// Percent of CPU in thousandths, preserving sub-percent observations in an integer.
    pub cpu_percent_milli: Option<i64>,
    /// Measured memory use in megabytes.
    pub memory_used_mb: Option<i64>,
    /// When the machine reported this sample.
    pub observed_at: Time,
}

/// Safe retained-file rows without physical volume names or an early-purge permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostKeptData {
    /// The environment whose files remain.
    pub environment_id: Id,
    /// Its name only where the reader may inspect that environment.
    pub name: Option<String>,
    /// Its home, redacted under the ordinary node view rules.
    pub home: Option<NodeRef>,
    /// Named storages; null denotes the driver's default storage.
    pub storages: Vec<Option<String>>,
    /// The retained lifecycle phase.
    pub phase: crate::environment::EnvironmentRemovalPhase,
    /// Measured size, or null when unknown.
    pub bytes: Option<i64>,
    /// The frozen actual purge date, if scheduled.
    pub purge_at: Option<Time>,
    /// Current unresolved lifecycle work, with no private plan or log.
    pub job: Option<crate::nodes::JobRef>,
}

/// One environment generation's kept installed files, excluding its shared base image.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostKeptLayers {
    pub environment_id: Id,
    pub environment: Option<crate::environment::EnvironmentRef>,
    pub generation: i64,
    pub bytes: i64,
}

/// A recipe for units: environments, or the runtimes of providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Template {
    /// The template's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// The root org of its home; `null` for the system's.
    pub org_id: Option<Id>,
    /// Its first owner.
    pub owner: Option<UserRef>,
    /// The org it belongs to, at any level; `null` for the system's.
    pub home: Option<NodeRef>,
    /// Where it comes from, as short names.
    pub address: String,
    /// The pool it is for. It does not change.
    pub kind: UnitKind,
    /// The driver that makes units from it. It does not change.
    pub driver: String,
    /// The recipe, in that driver's schema for its kind.
    pub spec: Json,
    /// How many units were made from it.
    pub units: i64,
    /// Whether units may be made from it.
    pub enabled: bool,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// Where an MCP server was found, when it was imported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerImported {
    /// The harness whose configuration it was in.
    pub source: String,
    /// Whether the user's own configuration or a directory of the workspace had it.
    pub scope: McpScope,
    /// For a project, the directory.
    pub directory: Option<String>,
    /// What it was called there.
    pub name: String,
    /// When it was imported.
    pub at: Time,
    /// Who imported it.
    pub by: UserRef,
    /// Whether the harness's own definition has changed since.
    pub stale: bool,
}

/// An MCP server that environments can be given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServer {
    /// Current machine offerings; the resource stays when this list is empty.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub offerings: Vec<McpServerOffering>,
    /// Places and environment configurations that explicitly switch this server on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled_places: Option<i64>,
    /// The server's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// The root org of its home; `null` for the system's.
    pub org_id: Option<Id>,
    /// Its first owner.
    pub owner: Option<UserRef>,
    /// The org it belongs to, at any level; `null` for the system's.
    pub home: Option<NodeRef>,
    /// Where it comes from, as short names.
    pub address: String,
    /// Set for a server that was imported from the harness configuration of that machine: it runs
    /// in that machine's environments alone.
    pub machine: Option<MachineRef>,
    /// Set for a server of one environment's own: written on its page, on there alone, and gone with
    /// it.
    pub environment: Option<EnvironmentRef>,
    /// Where it was imported from, when it was.
    pub imported: Option<McpServerImported>,
    /// How it is started and reached; for an imported one, its transport alone.
    pub spec: Json,
    /// Whether it may be switched on.
    pub enabled: bool,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// A native definition copied or being prepared on one machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpServerOffering {
    /// A different live discovery is a proposal; an existing protected copy stays ready until a manager acts.
    #[serde(default)]
    pub different_definition: bool,
    /// Identity of the reported definition, without any credential or variable values.
    pub definition_fingerprint: Option<orch_types::Hash>,
    /// Whether this reader may explicitly accept this machine's different definition.
    pub can_use_definition: bool,
    /// The supplying machine.
    pub machine: MachineRef,
    /// Its copy's availability.
    pub state: McpOfferingState,
    /// When the verified native copy became available.
    pub offered_at: Option<Time>,
    /// When its origin was last reported.
    pub seen_at: Time,
    /// A safe reason when the copy could not be prepared.
    pub reason: Option<String>,
}

/// Whether a machine's native copy is prepared for use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpOfferingState {
    /// A different or unrecorded definition was found, and nobody has selected it.
    Found,
    /// Copying the discovered definition under the connector's own policy.
    Pending,
    /// The protected definition is available on that machine.
    Ready,
    /// Its local policy or native source refused the copy.
    Failed,
}

/// Selects one machine's reported definition after a manager reviews the offering.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpOfferingUseBody {
    /// The current resource revision.
    pub expected_revision: Revision,
    /// The reported structural identity, when the machine can supply it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_fingerprint: Option<orch_types::Hash>,
}

/// What a capability does when an environment asks for it and nothing else decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityDefaultDecision {
    /// It is granted.
    Allow,
    /// Its approvers are asked.
    Ask,
    /// It is refused.
    Deny,
}

/// What an environment may be given beyond the ordinary, such as a debugger or a GPU.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct CapabilityDef {
    /// Its key.
    pub key: String,
    /// What it is called.
    pub label: String,
    /// What it lets an environment do.
    pub description: String,
    /// How much harm granting it could do.
    pub risk: Risk,
    /// What it comes to for each driver, by the driver's name, in the shape that driver declares
    /// for the key. A driver applies only its own.
    pub driver_patch: BTreeMap<String, Json>,
    /// What happens when one is asked for and the policy `capability.policy` says nothing.
    pub default_decision: CapabilityDefaultDecision,
    /// Whose approvers answer when the decision is to ask.
    pub default_approver_scope: ApproverScope,
    /// Whether it ships with the system.
    pub builtin: bool,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
}

/// A driver that the system's hosts report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriverInfo {
    /// Its name.
    pub name: String,
    /// What it is called.
    pub title: String,
    /// How many hosts have it.
    pub hosts: i64,
    /// What it declares about itself; the form of a template is drawn from its `spec_schemas`, by
    /// the template's kind.
    pub descriptor: Json,
}

/// A build of a harness.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessKindBuild {
    /// Its version.
    pub version: String,
    /// Whether it is the one used unless a profile names another.
    pub is_default: bool,
}

/// The schema of an agent descriptor, which the contract of the adapters owns: the document's component of that
/// name, supplied where the document is made.
pub(crate) fn descriptor_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
    let path = generator.settings().definitions_path.clone();
    let reference = format!("{path}AgentDescriptor");
    schemars::json_schema!({ "anyOf": [ { "$ref": reference }, { "type": "null" } ] })
}

/// An adapter that the system has, built in or external.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HarnessKindInfo {
    /// The harness's kind.
    pub kind: String,
    /// What it is called, in English.
    pub title: String,
    /// The same, named in each language.
    pub harness: HarnessKindName,
    /// The builds there are.
    pub builds: Vec<HarnessKindBuild>,
    /// What the adapter declares about the harness, as an `AgentDescriptor`; `null` while no profile of the
    /// kind has reported one.
    #[schemars(schema_with = "descriptor_schema")]
    pub descriptor: Option<Json>,
    /// What its manifest says, which is known before it runs; `null` for an adapter whose program sent none.
    pub program: Option<crate::programs::ProgramInfo>,
}

/// The one-line commands that enroll a machine, ready to paste into a shell of that system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollmentCommands {
    /// For Linux.
    pub linux: String,
    /// For macOS.
    pub macos: String,
    /// For Windows.
    pub windows: String,
}

/// The single-use token with which a machine enrolls.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollmentToken {
    /// The token.
    pub token: String,
    /// When it stops being good.
    pub expires_at: Time,
    /// Opaque tracking reference; only the issuer can read its progress. It cannot enroll a machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub receipt: Option<String>,
    /// The commands that use it.
    pub commands: EnrollmentCommands,
}

/// The issuer's current native view of the resource that consumed this enrollment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnrollmentProgress {
    /// The tracking reference supplied when the command was made.
    pub receipt: String,
    /// A connected machine, only when enrolled and currently readable by this person.
    pub machine: Option<Box<Machine>>,
    /// A host's node, only when enrolled and currently readable by this person.
    pub host: Option<Box<Host>>,
}

/// Installer readiness can be read without issuing an enrollment secret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineEnrollmentInfo {
    /// The operator setting that must be configured before a command can be offered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configure: Option<String>,
    /// The configured system address that installer commands would use.
    pub public_url: String,
    /// Configured secure addresses permit offering an installer command; examples require configuration.
    pub secure: bool,
}

/// What a key of a machine's local policy limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PolicyBounds {
    /// The environments registered from the machine.
    Environment,
    /// The providers registered from it.
    Provider,
    /// The machine itself.
    Machine,
}

/// One key of a machine's local policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachinePolicySection {
    /// Doors and work inside directories or units.
    Environment,
    /// Making units and exposing resources of the physical machine.
    Node,
    /// Running harness programs.
    Agent,
    /// Keeping credentials and reaching their upstreams.
    Provider,
    /// The machine's own updates and delegation.
    Machine,
}

impl MachinePolicySection {
    /// The stable section names used in delegation selectors and pages.
    pub const ALL: &'static [Self] = &[
        Self::Environment,
        Self::Node,
        Self::Agent,
        Self::Provider,
        Self::Machine,
    ];
    /// Its wire name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Node => "node",
            Self::Agent => "agent",
            Self::Provider => "provider",
            Self::Machine => "machine",
        }
    }
    /// The local command's selector; the machine action hands over every offered section.
    #[must_use]
    pub const fn selector(self) -> &'static str {
        match self {
            Self::Environment => "environment.*",
            Self::Node => "node.*",
            Self::Agent => "agent.*",
            Self::Provider => "provider.*",
            Self::Machine => "*",
        }
    }
}

/// Definition data supplied by the same catalogue that enforces the machine policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePolicyDefinition {
    /// Its single key name.
    pub key: String,
    /// Its short name in words.
    pub title: String,
    /// What it controls.
    pub meaning: String,
    /// The roles whose work it bounds; roots serve both environment and node.
    pub sections: Vec<MachinePolicySection>,
    /// The shipped value when Orch is handed the key.
    pub managed: Json,
    /// The shipped value while the key is kept on the machine.
    pub cautious: Json,
    /// Whether a local operator may hand this key over.
    pub delegable: bool,
}

/// One key of a machine's local policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePolicyKey {
    /// Meaning, role sections and both shipped values from the enforcing catalogue.
    pub definition: MachinePolicyDefinition,
    /// The key.
    pub key: String,
    /// Its value.
    pub value: Json,
    /// What it limits, and so who changes it.
    pub bounds: PolicyBounds,
    /// Whether its owner handed it over: it may then be changed from here.
    pub delegated: bool,
    /// Whether this caller may change it now.
    pub writable: bool,
}

/// A machine's local policy, which comes before every page: what it switched off is off for
/// everybody, the owner and the admins included.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePolicy {
    /// Mutation acknowledgement for the open machine page, including readiness; avoids a second GET.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_view: Option<Machine>,
    /// Exact physical program/instance command prefix ending in policy; never inferred from key sections.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// The machine.
    pub machine: MachineRef,
    /// When the machine last reported it.
    pub reported_at: Option<Time>,
    /// The keys.
    pub keys: Vec<MachinePolicyKey>,
}

pub use crate::admin::{MetricResolution, MetricSeries, Runtime};

/// A provider as it is answered after it was written: with a warning where what was written needs
/// one. A copy of the credential may now leave an environment, a runtime may be placed on a
/// machine whose manager can read the sign-in, or a key may travel in clear.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderWriteReply {
    /// The provider.
    #[serde(flatten)]
    pub provider: Provider,
    /// What may now happen and what that means, in a sentence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warning: Option<String>,
}

/// Any of the resources that have a page of their own, which a request that is the same for all of
/// them answers with.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
#[allow(
    clippy::large_enum_variant,
    reason = "resource sizes vary with contract features; an answer is built once, without a boxed layer in every use"
)]
pub enum Resource {
    /// A provider.
    Provider(Provider),
    /// An agent profile.
    Agent(Agent),
    /// An environment.
    Environment(Environment),
    /// A host.
    Host(Host),
    /// A physical machine with its permission page.
    Machine(Machine),
    /// A template.
    Template(Template),
    /// An MCP server.
    McpServer(McpServer),
    /// A mount.
    Mount(Mount),
    /// A skill.
    Skill(crate::skills::Skill),
    /// A document.
    Document(Document),
    /// Document-only folder metadata, without an editable document payload.
    DocumentContainer(crate::document_tree::DocumentTreeRead),
}

/// Which providers a list holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderListScope {
    /// Those the caller owns.
    Mine,
    /// Those they may run work on.
    Usable,
    /// Those they manage.
    Managed,
    /// Everything the caller may see, including what they own, manage or may use.
    Viewable,
    /// Everything they own or hold a permission on, of whatever org; for an admin of the org
    /// asked for, every provider whose home is in it.
    All,
}

/// The providers of the caller. Each is as far as the caller may see it: its name, address, first
/// owner, kind and status to everybody listed, its runtime, sign-in state and settings to those
/// who manage it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderListQuery {
    /// Only those whose home is that org or an org inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// Which; `all` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scope: Option<ProviderListScope>,
    /// How many candidates the page examines, at most 200; 50 when absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// The `next_cursor` of the page before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// The org a provider would be added in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSetupGetQuery {
    /// The org that would be its home, at any level.
    pub org_id: Id,
    /// For an existing provider's source choices: requires provider.manage, and org_id must be its home.
    /// Offers are restricted to its actual keeping machine and selected runtime template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_id: Option<Id>,
}

/// Starts a sign-in by one of the adapter's methods, which may be a different one from the last.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderLoginStartBody {
    /// The method's id.
    pub method: String,
    /// What its `fields` ask for. They are checked against the method's fields; secret values are
    /// sealed at once and are never logged or returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub values: Option<BTreeMap<String, String>>,
}

/// Makes a provider with independent credential-keeper and runtime locations. An omitted runtime
/// machine requests a unit from its template; an omitted keeper uses a native profile's declared
/// machine, or that runtime when the profile is portable. An API key
/// supplied here stays sealed in Orch's secret store and is delivered only to the selected keeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CreateProvider {
    /// A subscription that a harness signs in to.
    Subscription {
        /// A declared directory machine for the runtime; omitted, a unit is made from its template.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_machine_id: Option<Id>,
        /// The native keeper; omitted, the profile's registered machine or its runtime keeps it.
        #[serde(
            default,
            rename = "keeper_machine_id",
            skip_serializing_if = "Option::is_none"
        )]
        key_machine_id: Option<Id>,
        /// Its name.
        name: String,
        /// Its home: an org at any level.
        home_id: Id,
        /// Who may use it; as the defaults of its home say when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access: Option<NewResourceAccess>,
        /// The types its owner allows; what the profile's adapter proposes when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arrangements: Option<Vec<ArrangementType>>,
        /// Its runtime's own values.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_config: Option<Json>,
        /// The profile whose harness will hold the sign-in.
        agent_id: Id,
        /// What the adapter's `options_schema` allows.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        config: Option<Json>,
        /// A method to start at once.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sign_in: Option<ProviderLoginStartBody>,
        /// Where its details come from; an element left out has its default source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<NewProviderSources>,
    },
    /// An API reached with a key.
    Api {
        /// A declared directory machine for the runtime; omitted, a unit is made from its template.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_machine_id: Option<Id>,
        /// Its name.
        name: String,
        /// Its home: an org at any level.
        home_id: Id,
        /// Who may use it; as the defaults of its home say when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        access: Option<NewResourceAccess>,
        /// The types its owner allows; `separated` and `key_separated` when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        arrangements: Option<Vec<ArrangementType>>,
        /// Its runtime's own values.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        runtime_config: Option<Json>,
        /// Its endpoints: at least one, and at most one for each protocol.
        upstreams: Vec<Upstream>,
        /// A key to store now; omitted, it can be added later on Sign-in and keys.
        /// Omission never means that an endpoint permits unauthenticated requests.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        api_key: Option<String>,
        /// Values for the endpoints' `secret_headers`, by name.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        secrets: Option<BTreeMap<String, String>>,
        /// For a key that stays on a machine of the caller's, which must have every `base_url` in
        /// its local `provider.upstreams`, instead of `api_key` and `secrets`.
        #[serde(
            default,
            rename = "keeper_machine_id",
            alias = "key_machine_id",
            skip_serializing_if = "Option::is_none"
        )]
        key_machine_id: Option<Id>,
        /// Where its details come from; an element left out has its default source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<NewProviderSources>,
    },
}

/// Changes a provider. A change its running runtime cannot take (its template, its host, its
/// resources) is stored and listed in `Provider.runtime.updates`. Changing the arrangement types
/// writes `provider.arrangements_changed`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its endpoints, which replace the old ones.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upstreams: Option<Vec<Upstream>>,
    /// What the adapter's `options_schema` allows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// The types its owner allows; at least one stays.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arrangements: Option<Vec<ArrangementType>>,
    /// Its runtime's own values.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_config: Option<Json>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Brings a handed-over credential home.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderHandoverReturnBody {
    /// Whether to interrupt a turn in the holding environment that runs on it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
    /// Whether to give it up when the environment cannot return it: the holder is cleared and the
    /// provider is left in `needs_setup`. Only whoever holds `provider.manage` may.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub declare_lost: Option<bool>,
}

/// Starts or stops a managed provider's runtime by hand.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderStartStopBody {
    /// Whether to go on although a turn runs on it; a stop is refused otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupt: Option<bool>,
}

/// Asks for a plan to update the runtimes of several providers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRuntimeUpdateBatchPreflightBody {
    /// The providers.
    pub provider_ids: Vec<Id>,
}

/// Why a provider is left out of a batch of updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderUpdateSkipReason {
    /// The caller may not manage it.
    NoPermission,
    /// Its runtime is not behind in anything.
    UpToDate,
    /// It has no runtime of its own to update.
    NotManaged,
}

/// A provider left out of a batch of updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderUpdateSkipped {
    /// The provider.
    pub provider: ProviderRef,
    /// Why.
    pub reason: ProviderUpdateSkipReason,
}

/// Several providers' runtimes updated together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderRuntimeUpdateBatchPreflightReply {
    /// Distinct people the optional notice reaches across this batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<crate::forks::UpdateAudience>,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// One plan for each provider that is updated, each with its own blockers.
    pub plans: Vec<ProviderUpdatePlan>,
    /// The providers left out.
    pub skipped: Vec<ProviderUpdateSkipped>,
    /// How many plans have no blockers.
    pub executable: i64,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
}

/// The job of one provider of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderBatchJob {
    /// The provider.
    pub provider: ProviderRef,
    /// Its job.
    pub job: Job,
}

/// A batch of updates being carried out, one job for each plan without blockers, a few at a time
/// and each rolling back alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderBatch {
    /// The batch.
    pub batch_id: Id,
    /// The jobs.
    pub jobs: Vec<ProviderBatchJob>,
}

/// A thread that is bound to a provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderThread {
    /// The thread.
    pub thread: NodeRef,
    /// Its topic.
    pub topic: NodeRef,
    /// The org it is in, which may not be the provider's home.
    pub org: NodeRef,
    /// What it is doing.
    pub state: String,
    /// Why it cannot go on, when it cannot.
    pub blocked_reason: Option<String>,
    /// Its first owner.
    pub owner: UserRef,
}

/// The threads bound to a provider that the caller may view, of whatever org, each with its owner.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderThreadListReply {
    /// The threads.
    pub items: Vec<ProviderThread>,
    /// How many more there are than the caller may view.
    pub hidden: i64,
    /// What to send as `cursor` for the next page; `null` at the end.
    pub next_cursor: Option<String>,
}

/// Writes an API provider's key without changing its credential keeper.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderKeySetBody {
    /// The key. It is write-only.
    pub api_key: String,
}

/// Writes the value of one secret header of an endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSecretSetBody {
    /// The value. It is write-only.
    pub value: String,
}

/// What the person gave back for the fields a running sign-in asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderLoginInputBody {
    /// The answers, by field name (`LoginState.asks`).
    pub values: BTreeMap<String, String>,
}

/// The reset and count the person confirmed, with one identity for all retries.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderResetConsumeBody {
    /// A fresh logical request identifier, reused when the same request is retried.
    pub request_id: Id,
    /// The exact reset credit displayed in the confirmation.
    pub reset_id: String,
    /// The available count displayed in the confirmation.
    pub available_count: i64,
}

/// The vendor's answer and the provider after a confirmed reset attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderResetConsumeReply {
    /// The vendor's own outcome: reset, nothingToReset, noCredit or alreadyRedeemed.
    pub outcome: String,
    /// The provider with the resulting facts.
    pub provider: Provider,
}

/// Who last used a reset, shown until the reset windows next begin.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderLastReset {
    /// Who confirmed the reset.
    pub user: UserRef,
    /// The vendor's credit identifier.
    pub reset_id: String,
    /// When the vendor accepted the attempt.
    pub at: Time,
}

/// Display metadata for a credential; source lifetime and recovery proof cannot be overridden.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderCredentialDisplay {
    /// The displayed method name.
    pub method: Option<String>,
    /// The displayed credential kind; it never determines provider state or renewal.
    pub yields: CredentialYields,
}

/// An ordinary fact read, or a manager's explicit credential-recovery attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderRefreshBody {
    /// Try the current refused credential immediately; this needs current provider.manage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry: Option<bool>,
}

/// Facts that are written in by hand and so win over what the harness or endpoint reports: any
/// member of `ProviderFacts`, each one left out being as reported.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderFactsOverridesSetBody {
    /// The provider revision the manager reviewed; a stale write is refused before saving facts.
    pub expected_revision: Revision,
    /// The plan.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<String>")]
    pub plan: Option<Option<String>>,
    /// The windows of the plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<UsageWindow>>,
    /// What is left to be spent; a plan's own credits are the balance `plan`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub balances: Option<Vec<Balance>>,
    /// What has been spent, over periods.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spend: Option<Vec<Spend>>,
    /// A limit that has been reached.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<ProviderFactsLimitReached>")]
    pub limit_reached: Option<Option<ProviderFactsLimitReached>>,
    /// Who it is signed in as.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<String>")]
    pub identity: Option<Option<String>>,
    /// How it was given its credential.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<ProviderCredentialDisplay>")]
    pub credential: Option<Option<ProviderCredentialDisplay>>,
}

/// What a provider's managers choose for one element: what implements it, with its configuration, the values
/// of its secret fields and the authorization of its address. Secrets are written here and never shown again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SourceChoiceSet {
    /// What implements it.
    pub by: SourceBy,
    /// For a built-in source or a program, its name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// For a source at an address, the address, how it is authorized, and whether the provider's key is
    /// sent to it (turned on only after the person confirmed it for that host; refused for a sign-in).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub address: Option<SourceAddress>,
    /// Values for the source's fields that are no secrets, by field.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub config: BTreeMap<String, Json>,
    /// Values for its secret fields, by field: a field left out keeps the value it has, `null` removes it.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub secrets: BTreeMap<String, Option<String>>,
    /// The address's authorization, the bearer token or the header's value: left out, it keeps the one it has;
    /// `null` removes it.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<String>")]
    pub authorization: Option<Option<String>>,
}

/// Where a new provider's details come from. An element left out has its default source: what follows from
/// the provider's credential and endpoints.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NewProviderSources {
    /// Where its facts, and the resets they list, come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<SourceChoiceSet>,
    /// Where its models come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<SourceChoiceSet>,
    /// What presents its key, for a provider whose credential is a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presenter: Option<SourceChoiceSet>,
}

/// Chooses where a provider's details come from. An element left out stays as it is; one given takes the place
/// of its choice, and its source is asked what it is and read once at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderSourcesSetBody {
    /// The provider revision the manager reviewed; a stale write is refused.
    pub expected_revision: Revision,
    /// Where its facts, and the resets they list, come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facts: Option<SourceChoiceSet>,
    /// Where its models come from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub models: Option<SourceChoiceSet>,
    /// What presents its key, for a provider whose credential is a key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub presenter: Option<SourceChoiceSet>,
}

/// Adds a model to a provider. A new provider has none, and is `needs_setup` until it has a
/// credential and at least one enabled model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelCreateBody {
    /// Its routing name, taken as it is given.
    pub model_name: String,
    /// Its display name; empty shows the routing name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// What it can be run with here. A default (`default_effort`, `default_service_tier`,
    /// `default_access_program`) has to be among what is enabled; `context_max` may not be below
    /// `context_window`; `default_context` is at most `context_max` and needs one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
    /// The groups to put it in: at most [`MODEL_GROUPS_MAX`], each named as [`is_model_group_name`]
    /// says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
}

/// Changes a model of a provider. A routing name is not changed: another model is added.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelUpdateBody {
    /// A new display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    /// What it can be run with now, as a whole. A manager enables a service tier, an access program
    /// or a larger context by writing it into `service_tiers`, `access_programs` or `context_max`,
    /// and only then does a thread on that model show it. The same refusals as when it is made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parameters: Option<ModelParameters>,
    /// The groups it is in now, as a whole.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub groups: Option<Vec<String>>,
    /// Whether it may be chosen.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// Accepts models that discovery suggested. Each is added with what its suggestion says of it,
/// except that the service tiers and access programs it lists and a context larger than the usual
/// one become `parameters.offered`, and none of them is enabled.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelSuggestionAcceptBody {
    /// The routing names.
    pub model_names: Vec<String>,
}

/// Accepted proposals and the provider's recomputed setup state from the same committed write.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderModelSuggestionAcceptReply {
    /// Only the models newly added by this act, for its count and feedback.
    pub added: Vec<ProviderModel>,
    /// The authorized complete provider, including its model list and new standing.
    pub provider: Provider,
}

/// The agent profiles the caller may view, use or manage, or administers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentListQuery {
    /// Only those whose home is that org or an org inside it, with the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Makes an agent profile in an org.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct OrgAgentCreateBody {
    /// Its name.
    pub name: String,
    /// The harness it runs.
    pub harness_kind: String,
    /// Which build of the harness to run, and how.
    pub profile: Json,
}

/// Changes an agent profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new profile.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Json>,
    /// Members of the descriptor to override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub descriptor_overrides: Option<Json>,
    /// Whether it is switched off.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Registers a profile for a harness that is installed on a machine of the caller's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineAgentCreateBody {
    /// Its home: the caller's domain or an org inside it.
    pub home_id: Id,
    /// The harness it runs.
    pub harness_kind: String,
    /// Its name.
    pub name: String,
    /// Which build of the harness to run, and how.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<Json>,
}

/// The templates the caller may view, use or manage, or administers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateListQuery {
    /// Only those whose home is that org or an org inside it, with the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// Only those of this kind; both when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<UnitKind>,
}

/// Makes a template. Its spec is checked against the driver's schema for its kind, and by the
/// driver itself on a host that has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its home: an org at any level; `null` for the system's.
    pub home_id: Option<Id>,
    /// The pool it is for: environments when left out. It does not change afterwards.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<UnitKind>,
    /// The driver. It does not change afterwards.
    pub driver: String,
    /// The recipe, in that driver's schema for its kind.
    pub spec: Json,
    /// Who may use it; as the defaults of its home say when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewResourceAccess>,
}

/// Checks a template configuration without saving or starting anything.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateCheckBody {
    /// The driver whose reported schema is used on save.
    pub driver: String,
    /// The kind of unit the configuration describes.
    pub kind: UnitKind,
    /// The complete configuration to check.
    pub spec: Json,
}

/// One problem at a value in a template configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateSpecProblem {
    /// An RFC 6901 JSON pointer relative to the configuration; empty means its root.
    pub pointer: String,
    /// The rule that failed, without repeating supplied values.
    pub message: String,
}

/// The same validation used on save, without writing a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateCheckReply {
    /// Whether every save-time configuration check passed.
    pub valid: bool,
    /// Each problem with its position in the configuration.
    pub errors: Vec<TemplateSpecProblem>,
}

/// Asks a driver to turn a file it knows into the spec of a template; nothing is saved.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateImportBody {
    /// The home the template would have; `null` for the system's.
    pub home_id: Option<Id>,
    /// The driver.
    pub driver: String,
    /// The pool the template would be for: environments when left out. `provider` proposes the spec
    /// of a template for providers' runtimes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<UnitKind>,
    /// One of the imports the driver declares (`GET /drivers`).
    pub format: String,
    /// What the import's fields ask for: a file's text as a value, an archive as the id of a
    /// finished upload.
    pub values: BTreeMap<String, String>,
}

/// How much a note of an import matters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemplateImportNoteLevel {
    /// For information.
    Info,
    /// Something to look at.
    Warning,
    /// Something that keeps the spec from working.
    Blocker,
}

/// A note about how an import went.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateImportNote {
    /// How much it matters.
    pub level: TemplateImportNoteLevel,
    /// What it says.
    pub text: String,
}

/// A proposal for the form of a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateImportReply {
    /// The proposed spec.
    pub spec: Json,
    /// What the import noticed.
    pub notes: Vec<TemplateImportNote>,
}

/// Changes a template. A change to its spec does not affect running units until they are updated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TemplateUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A new recipe.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Json>,
    /// Whether units may be made from it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// The MCP servers the caller may view, use or manage, or administers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerListQuery {
    /// Only those whose home is that org or an org inside it, with the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Makes an MCP server. One that is imported from a machine is made by importing it into an
/// environment instead, and one of an environment's own by writing it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its home: an org at any level; `null` for the system's.
    pub home_id: Option<Id>,
    /// How it is started and reached.
    pub spec: Json,
    /// Write-only values of declared secret variables; never returned as part of the definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Who may use it; as the defaults of its home say when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewResourceAccess>,
}

/// Changes an MCP server. Of one that was imported from a machine only `name`, `description` and
/// `enabled` change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct McpServerUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A new definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spec: Option<Json>,
    /// New values for declared secrets. Omitted values keep the saved values; imported servers stay machine-owned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Whether it may be switched on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// A role hosted by the connector; Node is enrolled through the host's separate program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachineEnrollmentRole {
    /// Let people work in folders of this machine.
    Environment,
    /// Run agents here.
    Agent,
    /// Keep provider keys here.
    Provider,
}

/// Asks for a token with which a machine enrolls, including its locally accepted role choices.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineEnrollmentTokenCreateBody {
    /// The machine's home org; omitted uses the caller's personal org. Creation there must be allowed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_id: Option<Id>,
    /// The connector roles to offer; omitted keeps the existing agent/environment default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roles: Option<Vec<MachineEnrollmentRole>>,
    /// Role sections handed over (environment, agent, provider), or ["*"] for all policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate: Option<Vec<String>>,
    /// Whether the local enrollment command declares this machine dedicated to Orch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedicated: Option<bool>,
    /// A name to propose for the machine.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_hint: Option<String>,
}

/// Renames a machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineUpdateBody {
    /// The new name.
    pub name: String,
}

/// Change a physical connector's offered roles; Node remains hosted by orch-node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineRolesSetBody {
    /// Environment, agent and/or provider, without duplicates and with at least one role.
    pub roles: Vec<String>,
}

/// The actual physical program's role editing capability and enforcing policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineRoleCapability {
    /// This program can rebuild its own role composition under its native supervisor.
    pub supported: bool,
    /// machine.roles is handed over and has no locally kept value.
    pub delegated: bool,
    /// The effective machine.roles switch permits role editing.
    pub enabled: bool,
}

/// A persisted native role change; the live hello supplies the active roles after reconnecting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineRolesChange {
    /// Current authorized machine projection, never an invented active-role report.
    pub machine_view: Machine,
    /// The roles the machine accepted and saved.
    pub requested_roles: Vec<String>,
    /// The native supervisor is reconnecting with the saved roles.
    pub reconnecting: bool,
}

/// Registers a provider whose runtime is a machine of the caller's. It is shared from its home
/// with whomever its page names, of whatever org, and needs no second registration for that; one
/// registered again in another org is another provider there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MachineProviderCreateBody {
    /// The login that a harness already has there.
    Subscription {
        /// Its home: the caller's domain or an org inside it.
        home_id: Id,
        /// Its name.
        name: String,
        /// A profile of this machine.
        agent_id: Id,
        /// Where its details come from; an element left out has its default source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<NewProviderSources>,
    },
    /// An API whose key is set on the machine.
    Api {
        /// Its home: the caller's domain or an org inside it.
        home_id: Id,
        /// Its name.
        name: String,
        /// Its endpoints.
        upstreams: Vec<Upstream>,
        /// Where its details come from; an element left out has its default source.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sources: Option<NewProviderSources>,
    },
}

/// Keys of a machine's local policy and the values to give them. A key the machine has not
/// delegated is refused, and nothing is changed.
pub type MachinePolicyUpdateBody = BTreeMap<String, Json>;

/// The hosts the caller may view, use or manage, or administers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostListQuery {
    /// Only those whose home is that org or an org inside it, with the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Asks for a token with which a host enrolls. The host belongs to the caller and has that home;
/// the Use row of one for the system names `everyone`, which its managers may change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostEnrollmentTokenCreateBody {
    /// Bind the node command's local handover flag; omission retains the older host token format.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delegate_policy: Option<bool>,
    /// The home the host will have; `null` for the system's.
    pub home_id: Option<Id>,
    /// A name to propose for the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name_hint: Option<String>,
    /// Who may use it; as the defaults of its home say when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewResourceAccess>,
}

/// Changes a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its labels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub labels: Option<Vec<String>>,
    /// Whether it takes new environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schedulable: Option<bool>,
    /// What it does when it is full; `null` follows the system again.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<WhenFull>")]
    pub when_full: Option<Option<WhenFull>>,
    /// Its declaration of host ports, checked against its node's own ceiling; `null` clears it,
    /// which closes the host's listeners.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "absent_or_null"
    )]
    #[schemars(with = "Option<crate::ports::HostPorts>")]
    pub ports: Option<Option<crate::ports::HostPorts>>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// The runtimes of a host to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRuntimeListQuery {
    /// Only those of this kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<UnitKind>,
}

/// The span of a host's load to read, as for the system's own metrics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostMetricsListQuery {
    /// The start of the span.
    pub from: Time,
    /// The end of the span.
    pub to: Time,
    /// The width of a point, in minutes: 1, 5, 60 or 1 440. By the span when left out, so that a
    /// series has at most 720 points.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<u32>,
}

/// Stops a runtime of a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRuntimeStopBody {
    /// Whether to interrupt what runs in it.
    pub interrupt: bool,
}

/// Which runtime to remove from a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostRuntimeDeleteQuery {
    /// The unit's name, to be sure.
    pub confirm: String,
    /// Whether to remove its volumes there too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volumes: Option<bool>,
}

/// Which host to remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HostDeleteQuery {
    /// The host's name, to be sure.
    pub confirm: String,
}

/// The report span and shared chart bucket for a provider's plan history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProviderPlanGetQuery {
    /// First UTC date, included.
    pub from: String,
    /// Last UTC date, included.
    pub to: String,
    /// The report's chart bucket; day when absent.
    pub bucket: Option<crate::usage::UsageBucket>,
    /// Windows in this page, bounded by the ordinary page limit.
    pub limit: Option<u32>,
    /// The last window of this same observation snapshot.
    pub cursor: Option<String>,
}

/// A plan window's measured current values and bucketed consumption history.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct ProviderPlanSeries {
    /// The window id in the provider's facts.
    pub window_id: String,
    /// The later of the scheduled start and its last early start-over.
    pub starts_at: Option<Time>,
    /// Current-window measurements, with at most six hundred peak-preserving points.
    pub points: Vec<orch_provider_contract::forecast::ForecastPoint>,
    /// Positive rises in each observed report bucket; missing buckets are absent.
    pub history: Vec<PlanUsageBucket>,
    /// One representative early-reset marker per report bucket, with its count.
    pub resets: Vec<PlanWindowReset>,
    /// The latest start-over inside the current cycle.
    pub current_reset: Option<PlanWindowReset>,
    /// Exact means and counts computed by the store for the history caption.
    pub summary: PlanHistorySummary,
}

/// The report's observed mean and limit/reset counts, independent of chart decimation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanHistorySummary {
    /// Mean consumption across observed buckets; null without readings.
    pub mean_used_percent: Option<f64>,
    /// Buckets that included a reading at the limit.
    pub limit_buckets: i64,
    /// All early resets in the report, including those grouped into one marker.
    pub early_resets: i64,
}

/// Measured usage added during one history bucket, rather than the window's fill.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanUsageBucket {
    /// Inclusive bucket start.
    pub at: Time,
    /// Exclusive bucket end.
    pub ends_at: Time,
    /// Sum of nonnegative rises between successive readings.
    pub used_percent: f64,
    /// At least one reading in this bucket reached the limit.
    pub reached_limit: bool,
}

/// Why a window began again before the previously scheduled reset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanResetCause {
    /// A person spent a reset through Orch.
    Orch,
    /// A reset credit was spent elsewhere.
    Outside,
    /// The vendor started it over without a known reset credit.
    Vendor,
}

/// A start-over marker linked to its most recent audit entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlanWindowReset {
    /// The audit entry.
    pub id: Id,
    /// When the new window was observed.
    pub at: Time,
    /// The observed cause.
    pub cause: PlanResetCause,
    /// Resets represented by this marker; one for a current-cycle marker.
    pub count: i64,
}

crate::endpoints! {
    /// Lists the providers the caller owns or holds a permission on, of whatever org, each as far as the caller may see it.
    ProviderList: GET "/providers", User, query ProviderListQuery, none, json crate::common::Page<crate::common::Listed<Provider>>, 200;
    /// Gives everything the page that adds a provider needs, for the caller, in one answer.
    ProviderSetupGet: GET "/provider-setup", Rule("provider_id: provider.manage and org_id is its home; otherwise org_id is the caller's domain or an org inside it, or an org on which they hold settings.manage"), query ProviderSetupGetQuery, none, json ProviderSetup, 200;
    /// Makes a managed provider, the caller's own, with the job that makes its runtime; it may start a sign-in at once.
    ProviderCreate: POST "/providers", Rule("its home is the caller's domain or an org inside it and the members of that home may make providers there; else settings.manage on that org"), none, json CreateProvider, json ProviderWriteReply, 200;
    /// Reads a provider.
    ProviderGet: GET "/providers/{id}", Rule("any permission on the provider"), none, none, json Provider, 200;
    /// Reads the visible plan windows' current measurements, usage history and start-over markers.
    ProviderPlanGet: GET "/providers/{id}/plan", Permission("provider.view"), query ProviderPlanGetQuery, none, json crate::common::Page<ProviderPlanSeries>, 200;
    /// Changes a provider; a change its running runtime cannot take is stored and listed in its runtime's updates.
    ProviderUpdate: PATCH "/providers/{id}", Permission("provider.manage"), none, json ProviderUpdateBody, json ProviderWriteReply, 200;
    /// Brings a handed-over credential home.
    ProviderHandoverReturn: POST "/providers/{id}/handover/return", Rule("provider.manage, or environment.manage on the holding environment"), none, json ProviderHandoverReturnBody, json Provider, 200;
    /// Deletes a provider that no thread is bound to, signing its harness out first and retiring its runtime.
    ProviderDelete: DELETE "/providers/{id}", Permission("provider.manage"), none, none, none, 204;
    /// Switches a provider off.
    ProviderDisable: POST "/providers/{id}/disable", Permission("provider.manage"), query crate::resource_facts::ResourcePauseQuery, none, json Provider, 200;
    /// Switches a provider on again.
    ProviderEnable: POST "/providers/{id}/enable", Permission("provider.manage"), query crate::resource_facts::ResourceResumeQuery, none, json Provider, 200;
    /// Starts a managed provider's runtime by hand.
    ProviderStart: POST "/providers/{id}/start", Permission("provider.manage"), none, json ProviderStartStopBody, json Provider, 200;
    /// Stops a managed provider's runtime by hand; refused while a turn runs on it unless told to interrupt.
    ProviderStop: POST "/providers/{id}/stop", Permission("provider.manage"), none, json ProviderStartStopBody, json Provider, 200;
    /// Plans an update of a provider's runtime, or a plain recreation, before anything is done.
    ProviderRuntimeUpdatePreflight: POST "/providers/{id}/runtime/updates/preflight", Permission("provider.manage"), none, json UpdatePreflightBody, json ProviderUpdatePlan, 200;
    /// Carries out a plan of an update as a job; the runtime and every thread bound to the provider are locked at once.
    ProviderRuntimeUpdateCreate: POST "/providers/{id}/runtime/updates", Permission("provider.manage"), query UpdateConfirmQuery, json ForkCreateBody, json Job, 202;
    /// Plans the update of several providers' runtimes together.
    ProviderRuntimeUpdateBatchPreflight: POST "/providers/runtime/updates/preflight", Rule("provider.manage, on each provider"), none, json ProviderRuntimeUpdateBatchPreflightBody, json ProviderRuntimeUpdateBatchPreflightReply, 200;
    /// Carries out a plan of several updates: one job for each plan without blockers, a few at a time, each rolling back alone.
    ProviderRuntimeUpdateBatchCreate: POST "/providers/runtime/updates", Rule("provider.manage, on each provider"), query UpdateConfirmQuery, json ForkCreateBody, json ProviderBatch, 202;
    /// Lists the threads bound to a provider that the caller may view, of whatever org, and how many more there are.
    ProviderThreadList: GET "/providers/{id}/threads", Rule("any permission on the provider"), query PageQuery, none, json ProviderThreadListReply, 200;
    /// Restarts the harness of one agent profile on a provider; its sessions reopen at their next turn.
    ProviderHarnessRestart: POST "/providers/{id}/harnesses/{agent_id}/restart", Permission("provider.manage"), none, none, json Provider, 200;
    /// Lists the conversations a provider's harness already holds, for adapters that can discover them.
    ProviderHarnessSessionList: GET "/providers/{id}/harnesses/{agent_id}/sessions", Permission("provider.use"), none, none, json Vec<NativeSession>, 200;
    /// Saves an API key, asks the configured facts/model sources, and returns the provider; never the key.
    ProviderKeySet: PUT "/providers/{id}/key", Permission("provider.manage"), none, json ProviderKeySetBody, json Provider, 200;
    /// Writes the value of one secret header of a provider's endpoints; write-only, like the key.
    ProviderSecretSet: PUT "/providers/{id}/secrets/{name}", Permission("provider.manage"), none, json ProviderSecretSetBody, none, 204;
    /// Signs a provider in, or in again, by one of its adapter's methods.
    ProviderLoginStart: POST "/providers/{id}/login", Permission("provider.manage"), none, json ProviderLoginStartBody, json LoginState, 200;
    /// Gives back what a running sign-in asked for.
    ProviderLoginInput: POST "/providers/{id}/login/input", Permission("provider.manage"), none, json ProviderLoginInputBody, json LoginState, 200;
    /// Reads where a provider's sign-in stands.
    ProviderLoginGet: GET "/providers/{id}/login", Permission("provider.manage"), none, none, json LoginState, 200;
    /// Cancels a running sign-in.
    ProviderLoginCancel: POST "/providers/{id}/login/cancel", Permission("provider.manage"), none, none, json LoginState, 200;
    /// Signs a provider's harness out.
    ProviderLogout: POST "/providers/{id}/logout", Permission("provider.manage"), none, none, json Provider, 200;
    /// Asks the provider's connector for its status and usage, and waits up to ten seconds for the answer.
    ProviderRefresh: POST "/providers/{id}/refresh", Permission("provider.view"), none, json ProviderRefreshBody, json Provider, 200;
    /// Uses the exact reset credit a browser user with provider management confirmed.
    ProviderResetConsume: POST "/providers/{id}/resets/consume", Rule("browser session with provider.manage"), none, json ProviderResetConsumeBody, json ProviderResetConsumeReply, 200;
    /// Sets the facts that are written in by hand over what the provider reports.
    ProviderFactsOverridesSet: PUT "/providers/{id}/facts-overrides", Permission("provider.manage"), none, json ProviderFactsOverridesSetBody, json Provider, 200;
    /// Chooses where a provider's facts, models and key presentation come from, and reads the chosen sources once at once.
    ProviderSourcesSet: PUT "/providers/{id}/sources", Permission("provider.manage"), none, json ProviderSourcesSetBody, json Provider, 200;
    /// Lists the models a provider lists.
    ProviderModelList: GET "/providers/{id}/models", Rule("any permission on the provider"), none, none, json Vec<ProviderModel>, 200;
    /// Adds a model to a provider, with the groups it is in and what it can be run with there; `409 conflict.exists` for a routing or display name the provider already has, `400 request.invalid` for a default that is not among what is enabled, a `context_max` below the window, a `default_context` above `context_max` or given without one, and a group that is badly named or more than eight.
    ProviderModelCreate: POST "/providers/{id}/models", Permission("provider.manage"), none, json ProviderModelCreateBody, json ProviderModel, 200;
    /// Changes a model of a provider: its display name, what it can be run with, its groups, whether it may be chosen; the same refusals as when it is added.
    ProviderModelUpdate: PATCH "/providers/{id}/models/{model_id}", Permission("provider.manage"), query crate::resource_facts::ResourcePauseQuery, json ProviderModelUpdateBody, json ProviderModel, 200;
    /// Removes a model from a provider.
    ProviderModelDelete: DELETE "/providers/{id}/models/{model_id}", Permission("provider.manage"), none, none, none, 204;
    /// Lists the models that discovery suggests for a provider.
    ProviderModelSuggestionList: GET "/providers/{id}/models/suggestions", Permission("provider.manage"), none, none, json Vec<ModelSuggestion>, 200;
    /// Adds the suggested models that were chosen; the speeds, the access programs and the larger context they list are put under `offered`, and none of them is enabled.
    ProviderModelSuggestionAccept: POST "/providers/{id}/models/suggestions/accept", Permission("provider.manage"), none, json ProviderModelSuggestionAcceptBody, json ProviderModelSuggestionAcceptReply, 200;
    /// Lists the agent profiles the caller may view, use or manage, or administers, of whatever org.
    AgentList: GET "/agents", User, query AgentListQuery, none, json Vec<crate::common::Listed<Agent>>, 200;
    /// Makes an agent profile in an org.
    OrgAgentCreate: POST "/orgs/{id}/agents", Permission("settings.manage"), none, json OrgAgentCreateBody, json Agent, 200;
    /// Reads an agent profile.
    AgentGet: GET "/agents/{id}", Rule("any permission on the agent profile"), none, none, json Agent, 200;
    /// Changes an agent profile.
    AgentUpdate: PATCH "/agents/{id}", Permission("agent.manage"), query crate::resource_facts::ResourcePauseQuery, json AgentUpdateBody, json Agent, 200;
    /// Deletes an agent profile that no thread is bound to.
    AgentDelete: DELETE "/agents/{id}", Permission("agent.manage"), none, none, none, 204;
    /// Lists the adapters the system has, built in and external.
    HarnessKindList: GET "/harness-kinds", User, none, none, json Vec<HarnessKindInfo>, 200;
    /// Lists the executors the system knows, with the tool sets of each and what an environment's exec node can offer of it.
    HarnessExecutorList: GET "/harness-executors", User, none, none, json Vec<crate::programs::ExecutorInfo>, 200;
    /// Registers a profile for a harness installed on a machine of the caller's.
    MachineAgentCreate: POST "/machines/{id}/agents", Rule("machine.use; the home is the caller's domain or an org inside it"), none, json MachineAgentCreateBody, json Agent, 200;
    /// Lists the templates the caller may view, use or manage, or administers, of whatever org and of both kinds unless one is asked for.
    TemplateList: GET "/templates", User, query TemplateListQuery, none, json Vec<crate::common::Listed<Template>>, 200;
    /// Makes a template, the caller's own.
    TemplateCreate: POST "/templates", Rule("its home is the caller's domain or an org inside it and the members of that home may make templates there; else settings.manage on that org; a system admin when it has no home, when the driver is restricted, or for what only an admin may save"), none, json TemplateCreateBody, json Template, 200;
    /// Proposes the spec of a template from a file a driver can read; nothing is saved.
    TemplateImport: POST "/templates/import", Rule("as making a template for that home and driver"), none, json TemplateImportBody, json TemplateImportReply, 200;
    /// Checks a configuration against the reported driver schema and unit rules without saving it.
    TemplateCheck: POST "/templates/check", User, none, json TemplateCheckBody, json TemplateCheckReply, 200;
    /// Reads a template.
    TemplateGet: GET "/templates/{id}", Rule("any permission on the template"), none, none, json Template, 200;
    /// Changes a template; running units are not affected until they are updated.
    TemplateUpdate: PATCH "/templates/{id}", Permission("template.manage"), none, json TemplateUpdateBody, json Template, 200;
    /// Deletes a template that nothing uses.
    TemplateDelete: DELETE "/templates/{id}", Permission("template.manage"), none, none, none, 204;
    /// Lists the capabilities an environment can be given.
    CapabilityList: GET "/capabilities", User, none, none, json Vec<CapabilityDef>, 200;
    /// Lists the drivers the system's hosts report, with what each declares.
    DriverList: GET "/drivers", User, none, none, json Vec<DriverInfo>, 200;
    /// Lists the MCP servers the caller may view, use or manage, or administers, of whatever org.
    McpServerList: GET "/mcp-servers", User, query McpServerListQuery, none, json Vec<crate::common::Listed<McpServer>>, 200;
    /// Makes an MCP server, the caller's own.
    McpServerCreate: POST "/mcp-servers", Rule("its home is the caller's domain or an org inside it and the members of that home may make MCP servers there; else settings.manage on that org; a system admin when it has no home"), none, json McpServerCreateBody, json McpServer, 200;
    /// Reads an MCP server.
    McpServerGet: GET "/mcp-servers/{id}", Rule("any permission on the MCP server"), none, none, json McpServer, 200;
    /// Changes an MCP server; of one imported from a machine only its name, description and switch change.
    McpServerUpdate: PATCH "/mcp-servers/{id}", Rule("mcp.manage; for a server of one environment's own, environment.manage on that environment"), none, json McpServerUpdateBody, json McpServer, 200;
    /// Deletes an MCP server; one imported from a machine has that machine's connector forget its copy.
    McpServerDelete: DELETE "/mcp-servers/{id}", Rule("mcp.manage; for a server of one environment's own, environment.manage on that environment"), none, none, none, 204;
    /// Takes the definition of an imported MCP server again from where it was found.
    McpServerReimport: POST "/mcp-servers/{id}/reimport", Rule("mcp.manage, and what importing it needs"), none, none, json McpServer, 200;
    /// Uses a manager-selected native definition from a machine that reported it.
    McpOfferingUse: POST "/mcp-servers/{id}/offerings/{machine_id}/use", Rule("mcp.manage and authority to import from the offering machine"), none, json McpOfferingUseBody, json McpServer, 200;
    /// Lists the machines visible to the caller, with their current access facts.
    MachineList: GET "/machines", User, none, none, json Vec<Machine>, 200;
    /// Asks for a token with which a machine enrolls, with the commands that use it.
    MachineEnrollmentTokenCreate: POST "/machines/enrollment-tokens", User, none, json MachineEnrollmentTokenCreateBody, json EnrollmentToken, 200;
    /// Issues a token that enrolls the same machine again, preserving its page and registrations.
    MachineEnrollmentAgainTokenCreate: POST "/machines/{id}/enrollment-tokens", Rule("machine.manage, or host.manage on its node facade"), none, none, json EnrollmentToken, 200;
    /// Reads a machine.
    MachineGet: GET "/machines/{id}", Rule("machine.view, or host.view on its node facade"), none, none, json Machine, 200;
    /// Read installation readiness; creates no token or other resource.
    MachineEnrollmentInfoGet: GET "/machines/enrollment-options", User, none, none, json MachineEnrollmentInfo, 200;
    /// Reads only this person's enrollment progress using the resource's current native permissions.
    MachineEnrollmentStatusGet: GET "/machines/enrollment-status/{receipt}", crate::routes::Access::Session, none, none, json EnrollmentProgress, 200;
    /// Renames a machine.
    MachineUpdate: PATCH "/machines/{id}", Rule("machine.manage, or host.manage on its node facade"), none, json MachineUpdateBody, json Machine, 200;
    /// Saves delegated connector roles and reconnects after its answer is journaled.
    MachineRolesSet: PUT "/machines/{id}/roles", Rule("machine.manage; machine.roles must be handed over and enabled"), none, json MachineRolesSetBody, json MachineRolesChange, 200;
    /// Requests a verified physical connector update, drained under its local update.auto policy.
    MachineUpdateNow: POST "/machines/{id}/update", Rule("machine.manage, or host.manage on its node facade"), none, none, json ComponentUpdate, 200;
    /// Requests the same verified procedure for a host's node, after its units finish their turns.
    HostUpdateNow: POST "/hosts/{id}/update", Rule("host.manage"), none, none, json ComponentUpdate, 200;
    /// Withdraws a machine's registration and drops its session at once.
    MachineRevoke: POST "/machines/{id}/revoke", Rule("machine.manage, or host.manage on its node facade"), none, none, json Machine, 200;
    /// Registers a provider whose runtime is the machine: the login its harness has there, or an API whose key is set there.
    MachineProviderCreate: POST "/machines/{id}/providers", Rule("machine.use; the home is the caller's domain or an org inside it"), none, json MachineProviderCreateBody, json Provider, 200;
    /// Asks the machine for its facts again.
    MachineProbe: POST "/machines/{id}/probe", Rule("machine.manage, or host.manage on its node facade"), none, none, json Machine, 200;
    /// Reads the local policy of a machine as it last reported it.
    MachinePolicyGet: GET "/machines/{id}/policy", Rule("machine.view, or host.view on its host page; managers of hosted environments/providers read their binding sections without machine management"), none, none, json MachinePolicy, 200;
    /// Changes keys of a machine's local policy that its owner handed over, key by key; nothing changes if any is refused.
    MachinePolicyUpdate: PATCH "/machines/{id}/policy", Rule("machine.manage, or host.manage on its host page; only keys handed to Orch, with every explicit local value enforced"), none, json MachinePolicyUpdateBody, json MachinePolicy, 200;
    /// The executors this machine can offer, with their tool sets: those of the connector's release, which carries
    /// the registry's executors, and the executor plugins the machine reported. The policy screen offers these and no
    /// others for `exec.offers`.
    MachineExecutorList: GET "/machines/{id}/executors", Rule("machine.view or host.view"), none, none, json Vec<crate::programs::MachineExecutor>, 200;
    /// The latest program registry reported by this machine; no command or idle polling.
    MachineProgramsGet: GET "/machines/{id}/programs", Rule("machine.view or host.view"), none, none, json MachinePrograms, 200;
    /// Registers, tests, removes or reloads a program through the same native registry.
    MachineProgramsChange: POST "/machines/{id}/programs", Rule("machine.manage or host.manage; remote registration also requires its delegated local policy key"), none, json MachineProgramsCommand, json MachinePrograms, 200;
    /// Lists the hosts the caller may view, use or manage, or administers, of whatever org.
    HostList: GET "/hosts", User, query HostListQuery, none, json Vec<crate::common::Listed<Host>>, 200;
    /// Asks for a token with which a host enrolls.
    HostEnrollmentTokenCreate: POST "/hosts/enrollment-tokens", Rule("its home is the caller's domain or an org inside it and the members of that home may add hosts there; else settings.manage on that org; a system admin when it has no home"), none, json HostEnrollmentTokenCreateBody, json EnrollmentToken, 200;
    /// Reads a host.
    HostGet: GET "/hosts/{id}", Rule("any permission on the host"), none, none, json Host, 200;
    /// Changes a host.
    HostUpdate: PATCH "/hosts/{id}", Permission("host.manage"), none, json HostUpdateBody, json Host, 200;
    /// Lists everything that runs on a host, whoever's it is.
    HostRuntimeList: GET "/hosts/{id}/runtimes", Permission("host.manage"), query HostRuntimeListQuery, none, json Vec<Runtime>, 200;
    /// Reads a host's load over time: CPU, memory used, disk used and free.
    HostMetricsList: GET "/hosts/{id}/metrics", Permission("host.manage"), query HostMetricsListQuery, none, json Vec<MetricSeries>, 200;
    /// Stops a runtime of a host.
    HostRuntimeStop: POST "/hosts/{id}/runtimes/{runtime_id}/stop", Permission("host.manage"), none, json HostRuntimeStopBody, json Runtime, 200;
    /// Schedules removal of the exact environment or provider unit, with its owned volumes if asked.
    /// A provider survives; its next turn creates a new unit. Running work prevents acceptance.
    HostRuntimeDelete: DELETE "/hosts/{id}/runtimes/{runtime_id}", Permission("host.manage"), query HostRuntimeDeleteQuery, none, none, 204;
    /// Stops the idle environments on a host, or only lists them.
    HostRecycle: POST "/hosts/{id}/recycle", Permission("host.manage"), none, json RecycleBody, json RecycleReply, 200;
    /// Removes a host that nothing runs on and no mount has a volume on; its machine is revoked.
    HostDelete: DELETE "/hosts/{id}", Permission("host.manage"), query HostDeleteQuery, none, none, 204;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const AT: &str = "2026-10-08T17:04:05.123Z";

    fn user() -> Value {
        json!({ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "human", "username": "alice", "display_name": "Alice", "avatar": { "text": "A", "color": 4 } })
    }

    fn machine() -> Value {
        json!({ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a10", "name": "build-1", "online": true, "platform_os": "linux", "placement": "managed" })
    }

    fn agent() -> Value {
        json!({ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a11", "name": "Codex", "address": "system.codex", "harness_kind": "codex", "harness": { "kind": "codex", "name": "Codex" }, "machine": null, "disabled": false })
    }

    /// The members of several objects written as one: a literal this large does not fit the
    /// macro's recursion limit in one piece.
    fn merged(parts: &[Value]) -> Value {
        let mut all = serde_json::Map::new();
        for part in parts {
            if let Value::Object(members) = part {
                all.extend(members.clone());
            }
        }
        Value::Object(all)
    }

    fn provider() -> Value {
        merged(&[
            json!({
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a12", "name": "Team Pro", "kind": "subscription",
                "harness_kind": "codex", "harness": { "kind": "codex", "name": "Codex" }, "address": "acme.research.alice.team-pro",
                "placement": "managed", "status": "ready",
                "owner": user(), "owners": 2, "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a13",
                "home": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14", "kind": "org", "name": "Research", "redacted": false },
                "agent": agent(),
                "arrangements": [
                    { "type": "separated", "allowed": true, "available": true, "puts": "Nothing: the harness and its tools stay apart.", "form": null },
                    { "type": "credential_shared", "allowed": false, "available": true, "puts": "A copy of the sign-in.",
                      "form": { "name": "auth.json", "title": "Sign-in file", "lifetime_secs": 3600 } }
                ],
                "held_by": null, "runtime_config": {}, "upstreams": [], "secrets_set": [], "config": {},
                "subscription": { "accepted": true, "shareable": false },
                "protocols": ["openai.responses"], "key_set": false, "key_machine": null,
                "machine": null, "status_detail": null, "status_at": AT
            }),
            json!({
                "facts": {
                    "plan": "Team",
                    "windows": [{ "id": "5h", "label": "5 hours", "duration_minutes": 300, "used_percent": 41.5, "resets_at": AT,
                        "model_family": null, "is_active": null, "reset_not_started": null, "warning_percent": null, "severity": null }],
                    "limit_reached": null, "identity": "alice@example.com",
                    "credential": { "method": "device_code", "yields": "login", "expires_at": null, "issued_at": null },
                    "available_resets": null, "extra_usage": null,
                    "balances": [{ "id": "plan", "label": "Plan credits", "unit": "credits", "remaining": "12.5",
                                   "total": null, "used": null, "unlimited": false, "parts": null, "expires_at": null }],
                    "spend": [],
                    "ordinary_usage_allowed": null, "credential_verified": null, "models": null, "field_times": {}
                },
                "facts_state": {
                    "identity": "told", "credential": "told", "credential_verified": "not_read_yet", "plan": "told",
                    "windows": "told", "limit_reached": "empty", "ordinary_usage_allowed": "not_read_yet",
                    "balances": "told", "spend": "unsupported", "extra_usage": "unsupported",
                    "available_resets": "not_read_yet", "models": "not_read_yet"
                },
                "sources": {
                    "facts": { "by": "default", "name": "adapter", "address": null, "config": {}, "secrets": [],
                               "descriptor": { "name": "adapter", "title": "Codex", "summary": "Codex reports the plan, windows, credits and resets.",
                                               "contract": 1,
                                               "facts": { "reports": ["plan", "windows", "balances", "available_resets", "credential_verified"],
                                                          "spends_resets": true, "credential": "none", "min_interval_secs": null, "plans": {} },
                                               "models": {}, "presents": null, "fields": [], "docs_url": null },
                               "described_at": null, "last_error": null },
                    "models": { "by": "address", "name": null,
                                "address": { "url": "http://models.lab.internal/orch", "auth": "bearer", "send_credential": false },
                                "config": {}, "secrets": [], "descriptor": null, "described_at": AT,
                                "last_error": { "code": "unavailable", "message": "models.lab.internal is unavailable", "at": AT } },
                    "presenter": null
                },
                "facts_overridden": ["plan"], "facts_at": AT, "facts_requested_at": AT, "last_reset": null,
                "models": [{
                    "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a15", "model_name": "gpt-6-sol", "display_name": "GPT-6 Sol (A)",
                    "parameters": {
                        "efforts": ["low", "high"], "default_effort": "low",
                        "service_tiers": [{ "id": "fast", "label": "Fast" }], "default_service_tier": "fast",
                        "context_window": 400000, "context_max": 1000000, "default_context": 400000,
                        "offered": { "service_tiers": [{ "id": "fast", "label": "Fast" }], "access_programs": ["daybreak"],
                                     "context_max": 1000000 },
                        "input_images": true,
                        "rate": {
                            "input_per_mtok": "1.25", "cached_input_per_mtok": "0.125", "output_per_mtok": "10",
                            "tiers": { "fast": "2" },
                            "above": [{ "context_tokens": 200000, "input_per_mtok": "2.5", "output_per_mtok": "15" }]
                        }
                    },
                    "groups": ["frontier"],
                    "token_estimate": { "ascii_prose_x100": 400, "ascii_structured_x100": 300, "other_x100": 100, "image_tokens": 1000, "measured": false },
                    "enabled": true, "source": "suggested"
                }],
                "running_threads": 3
            }),
            json!({
                "runtime": {
                    "network": {
                        "mode": "open", "required_mode": "open", "network_mode": "bridge",
                        "proxy_url": null, "allowed_hosts": [], "sources": [], "revision": "r1",
                        "always": [{ "authority": "gateway.orch.internal", "protocol": "wss", "port": 9443, "purpose": "gateway" }],
                        "always_revision": "a1", "always_source": "orch_configuration",
                        "state": "ready", "reason": null, "last_denied": null, "networks": []
                    },
                    "state": "running", "host": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a16", "name": "build-1", "address": "build-1" },
                    "driver": "docker",
                    "template": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a17", "name": "Provider runtime", "owner": null },
                    "generation": 2,
                    "updates": [{ "what": "image", "name": "orch-provider", "from": "1.2.0", "to": "1.3.0" }],
                    "update_required": false, "last_active_at": AT, "idle_stop_at": AT, "renew_at": null, "job": null
                },
                "harnesses": [{
                    "agent": agent(), "machine": machine(), "status": "online", "status_detail": null,
                    "harness_version": "0.9.1", "sessions_open": 2, "last_active_at": AT
                }],
                "revision": 7, "created_at": AT, "updated_at": AT,
                "access": { "permissions": ["provider.manage"], "administers": false, "through": [{ "kind": "owner" }] }
            }),
        ])
    }

    #[test]
    fn program_errors_remain_historical_while_current_harness_status_is_online() {
        let example = serde_json::json!({
            "agent":{"id":"00000000-0000-4000-8000-000000000001","name":"Codex","address":"team.codex","harness_kind":"codex","harness":{"kind":"codex","name":"Codex"},"disabled":false,"machine":null},
            "machine":{"id":"00000000-0000-4000-8000-000000000002","name":"Build host","online":true,"placement":"managed","platform_os":"linux"},
            "status":"online","status_detail":null,"harness_version":"0.162.1","sessions_open":0,"last_active_at":null,
            "last_error":{"at":"2026-10-20T12:00:00.000Z","detail":"app-server exited"}
        });
        let mut decoded: HarnessState = serde_json::from_value(example.clone()).unwrap();
        assert_eq!(decoded.status, HarnessStatus::Online);
        assert_eq!(serde_json::to_value(&decoded).unwrap(), example);
        decoded.last_error = None;
        let old = serde_json::to_value(decoded).unwrap();
        assert!(old.get("last_error").is_none());
        assert!(
            serde_json::from_value::<HarnessState>(old)
                .unwrap()
                .last_error
                .is_none()
        );
    }

    #[test]
    fn a_provider_shows_its_runtime_its_models_and_what_its_facts_owe_to_overrides() {
        let written = provider();
        let provider: Provider = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(provider.base.status, ProviderStatus::Ready);
        assert_eq!(
            provider.arrangements[1].kind,
            ArrangementType::CredentialShared
        );
        assert!(!provider.arrangements[1].allowed);
        assert_eq!(provider.facts_overridden, ["plan"]);
        assert_eq!(
            provider.facts.balances[0].remaining.as_deref(),
            Some("12.5")
        );
        assert_eq!(provider.facts_state.balances, MemberState::Told);
        assert_eq!(provider.facts_state.spend, MemberState::Unsupported);
        assert_eq!(provider.sources.facts.name.as_deref(), Some("adapter"));
        assert!(
            provider
                .sources
                .facts
                .descriptor
                .as_ref()
                .is_some_and(|d| d.facts.as_ref().is_some_and(|facts| facts.spends_resets)),
            "a reset is offered where the facts source spends them"
        );
        assert_eq!(
            provider
                .sources
                .models
                .last_error
                .as_ref()
                .map(|error| error.code),
            Some(SourceErrorCode::Unavailable)
        );
        let written_facts = serde_json::to_value(&provider.facts).unwrap();
        for old in [
            "plan_credits",
            "key_usage",
            "key_credits",
            "key_usage_supported",
        ] {
            assert!(
                written_facts.get(old).is_none(),
                "{old} is no member of the answer"
            );
        }
        assert_eq!(
            provider
                .facts
                .credential
                .as_ref()
                .map(|credential| credential.yields),
            Some(CredentialYields::Login)
        );
        let model = &provider.models[0];
        assert_eq!(model.source, ModelSource::Suggested);
        assert_eq!(model.groups, ["frontier"]);
        let parameters = &model.parameters;
        assert_eq!(parameters.default_service_tier.as_deref(), Some("fast"));
        assert_eq!(
            (
                parameters.context_window,
                parameters.context_max,
                parameters.default_context
            ),
            (Some(400_000), Some(1_000_000), Some(400_000))
        );
        assert_eq!(
            parameters.offered.as_ref().and_then(|o| o.context_max),
            Some(1_000_000),
            "what is on offer is kept apart from what is open"
        );
        let rate = model.parameters.rate.as_ref().unwrap();
        assert_eq!(rate.above.as_ref().unwrap()[0].context_tokens, 200_000);
        assert!(provider.runtime.as_ref().unwrap().updates.len() == 1);
        assert_eq!(serde_json::to_value(&provider).unwrap(), written);
    }

    #[test]
    fn a_model_is_made_and_changed_with_its_groups_and_what_a_thread_on_it_starts_with() {
        let made: ProviderModelCreateBody = serde_json::from_value(json!({
            "model_name": "gpt-6-sol", "groups": ["frontier", "vision"],
            "parameters": { "service_tiers": [{ "id": "fast", "label": "Fast" }],
                            "default_service_tier": "fast", "context_max": 1000000 }
        }))
        .unwrap();
        assert_eq!(
            made.groups,
            Some(vec!["frontier".to_owned(), "vision".to_owned()])
        );
        assert_eq!(
            made.parameters
                .and_then(|p| p.default_service_tier)
                .as_deref(),
            Some("fast")
        );
        let untouched: ProviderModelUpdateBody =
            serde_json::from_value(json!({ "display_name": "GPT-6 Sol (A)" })).unwrap();
        assert_eq!(
            untouched.groups, None,
            "left out: the groups stay as they are"
        );
        let emptied: ProviderModelUpdateBody =
            serde_json::from_value(json!({ "groups": [] })).unwrap();
        assert_eq!(emptied.groups, Some(Vec::new()));
        assert!(
            serde_json::from_value::<ProviderModelUpdateBody>(json!({ "group": "x" })).is_err(),
            "an unknown member of a request is refused"
        );
    }

    #[test]
    fn a_group_is_named_by_one_to_forty_plain_characters_that_begin_with_a_letter_or_a_digit() {
        for name in ["frontier", "small", "gpt-6_2", "7b", "a"] {
            assert!(is_model_group_name(name), "{name}");
        }
        for name in [
            "",
            "-x",
            "_x",
            "Frontier",
            "a b",
            "a/b",
            "a@b",
            "é",
            &"g".repeat(41),
        ] {
            assert!(!is_model_group_name(name), "{name:?}");
        }
        assert!(is_model_group_name(&"g".repeat(MODEL_GROUP_NAME_MAX)));
    }

    #[test]
    fn a_write_that_needs_a_warning_carries_it_beside_the_provider() {
        let mut written = provider();
        written["warning"] = json!("A copy of the sign-in may now leave for an environment.");
        let reply: ProviderWriteReply = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(
            reply.warning.as_deref(),
            Some("A copy of the sign-in may now leave for an environment.")
        );
        assert_eq!(serde_json::to_value(&reply).unwrap(), written);
        let plain: ProviderWriteReply = serde_json::from_value(provider()).unwrap();
        assert_eq!(plain.warning, None);
        assert_eq!(serde_json::to_value(&plain).unwrap().get("warning"), None);
    }

    #[test]
    fn a_provider_is_made_from_a_subscription_or_from_an_api_whose_key_is_kept_somewhere() {
        let subscription: CreateProvider = serde_json::from_value(json!({
            "kind": "subscription", "name": "Team Pro", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14",
            "agent_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a11", "access": "only_me",
            "sign_in": { "method": "device_code" }
        }))
        .unwrap();
        assert!(matches!(
            subscription,
            CreateProvider::Subscription {
                sign_in: Some(ProviderLoginStartBody { values: None, .. }),
                access: Some(NewResourceAccess::OnlyMe),
                ..
            }
        ));
        let on_a_machine: CreateProvider = serde_json::from_value(json!({
            "kind": "api", "name": "Gateway", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14",
            "upstreams": [{ "protocol": "openai.responses", "base_url": "https://llm.example.com/v1", "options": { "auth": "bearer" } }],
            "key_machine_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a10"
        }))
        .unwrap();
        assert!(matches!(
            on_a_machine,
            CreateProvider::Api {
                key_machine_id: Some(_),
                api_key: None,
                ..
            }
        ));
        assert!(
            serde_json::from_value::<CreateProvider>(json!({
                "kind": "subscription", "name": "x", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14"
            }))
            .is_err(),
            "a subscription names the profile whose harness holds its sign-in"
        );
        assert!(
            serde_json::from_value::<CreateProvider>(json!({
                "kind": "subscription", "name": "x", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14",
                "agent_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a11", "upstreams": []
            }))
            .is_err(),
            "endpoints belong to an API provider alone"
        );
    }

    #[test]
    fn a_sign_in_in_progress_shows_what_to_open_and_asks_for_what_it_needs() {
        let state: LoginState = serde_json::from_value(json!({
            "state": "awaiting_user", "method": "device_code",
            "shows": [
                { "kind": "uri", "label": "Open this address", "value": "https://example.com/device" },
                { "kind": "code", "label": "Enter this code", "value": "ABCD-1234" }
            ],
            "asks": [{ "name": "pin", "label": "PIN", "type": "secret", "required": true, "help": null, "choices": null, "default": null, "max_bytes": null }],
            "expires_at": AT, "error": null
        }))
        .unwrap();
        assert_eq!(state.state, LoginProgress::AwaitingUser);
        assert_eq!(state.shows[1].kind, LoginShowKind::Code);
        assert_eq!(state.asks.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn a_fact_written_by_hand_can_be_given_as_nothing_or_left_as_reported() {
        let body: ProviderFactsOverridesSetBody = serde_json::from_value(
            json!({ "expected_revision": 2, "plan": null, "identity": "bo@example.com" }),
        )
        .unwrap();
        assert_eq!(
            body.plan,
            Some(None),
            "null overrides the plan with nothing"
        );
        assert_eq!(body.identity, Some(Some("bo@example.com".to_owned())));
        assert_eq!(body.credential, None, "a member left out stays as reported");
        assert_eq!(
            serde_json::to_value(&body).unwrap(),
            json!({ "expected_revision": 2, "plan": null, "identity": "bo@example.com" })
        );
        let balance: ProviderFactsOverridesSetBody = serde_json::from_value(json!({
            "expected_revision": 3,
            "balances": [{ "id": "plan", "unit": "credits", "remaining": "40" }],
            "spend": [{ "id": "month", "unit": "USD", "period": "month", "used": "3.20", "limit": "20" }]
        }))
        .unwrap();
        assert_eq!(balance.balances.as_ref().unwrap()[0].id, "plan");
        assert_eq!(
            balance.spend.as_ref().unwrap()[0].period,
            SpendPeriod::Month
        );
        assert!(
            serde_json::from_value::<ProviderFactsOverridesSetBody>(json!({
                "expected_revision": 3, "plan_credits": { "balance": "40", "unit": "credits", "unlimited": false }
            }))
            .is_err(),
            "a plan's credits are written as its balance `plan`"
        );
    }

    #[test]
    fn a_source_is_chosen_with_secrets_written_once_and_an_element_left_out_stays() {
        let body: ProviderSourcesSetBody = serde_json::from_value(json!({
            "expected_revision": 4,
            "facts": {
                "by": "address",
                "address": { "url": "https://plan.acme.example/orch", "auth": "header", "header": "X-Acme-Key",
                             "send_credential": true },
                "secrets": { "token": "t0k3n", "old": null },
                "authorization": "s3cr3t"
            },
            "models": { "by": "none" }
        }))
        .unwrap();
        let facts = body.facts.as_ref().unwrap();
        assert_eq!(facts.by, SourceBy::Address);
        assert_eq!(facts.secrets["old"], None, "null removes a secret");
        assert_eq!(facts.authorization, Some(Some("s3cr3t".to_owned())));
        assert!(facts.address.as_ref().unwrap().send_credential);
        assert_eq!(body.models.as_ref().unwrap().by, SourceBy::None);
        assert_eq!(body.presenter, None, "an element left out stays as it is");
        let kept: SourceChoiceSet =
            serde_json::from_value(json!({ "by": "program", "name": "acme-plan" })).unwrap();
        assert_eq!(
            kept.authorization, None,
            "left out, the authorization is kept"
        );
        let removed: SourceChoiceSet = serde_json::from_value(
            json!({ "by": "address", "address": { "url": "https://plan.acme.example" }, "authorization": null }),
        )
        .unwrap();
        assert_eq!(removed.authorization, Some(None));
        assert!(
            serde_json::from_value::<ProviderSourcesSetBody>(
                json!({ "expected_revision": 4, "facts": { "by": "builtin", "nmae": "openrouter" } })
            )
            .is_err(),
            "an unknown member of a request is refused"
        );
        let code: SourceErrorCode =
            serde_json::from_value(json!("policy.upstream_not_allowed")).unwrap();
        assert_eq!(code, SourceErrorCode::UpstreamNotAllowed);
    }

    #[test]
    fn a_host_follows_the_system_again_when_its_answer_is_null() {
        let follows: HostUpdateBody =
            serde_json::from_value(json!({ "when_full": null, "expected_revision": 2 })).unwrap();
        assert_eq!(follows.when_full, Some(None));
        let own: HostUpdateBody =
            serde_json::from_value(json!({ "when_full": "block", "expected_revision": 2 }))
                .unwrap();
        assert_eq!(own.when_full, Some(Some(WhenFull::Block)));
        let untouched: HostUpdateBody =
            serde_json::from_value(json!({ "labels": ["gpu"], "expected_revision": 2 })).unwrap();
        assert_eq!(untouched.when_full, None);
    }

    #[test]
    fn a_machines_policy_says_which_keys_the_owner_handed_over_and_the_caller_may_change() {
        let policy: MachinePolicy = serde_json::from_value(json!({
            "machine": machine(), "reported_at": AT,
            "keys": [
                { "definition":{"key":"environment.terminal","title":"Shells","meaning":"Allow interactive shells.","sections":["environment"],"managed":true,"cautious":false,"delegable":true}, "key": "environment.terminal", "value": true, "bounds": "environment", "delegated": true, "writable": false },
                { "definition":{"key":"update.auto","title":"Automatic updates","meaning":"Allow updates.","sections":["machine"],"managed":true,"cautious":true,"delegable":true}, "key": "update.auto", "value": false, "bounds": "machine", "delegated": false, "writable": false }
            ]
        }))
        .unwrap();
        assert_eq!(policy.keys[0].bounds, PolicyBounds::Environment);
        assert!(policy.keys[0].delegated && !policy.keys[0].writable);
        let update: MachinePolicyUpdateBody =
            serde_json::from_value(json!({ "environment.terminal": false })).unwrap();
        assert_eq!(update["environment.terminal"], json!(false));
        assert_eq!(
            policy.keys[0].definition.sections,
            [MachinePolicySection::Environment]
        );
    }

    #[test]
    fn what_a_transfer_answers_with_is_the_resource_itself() {
        let template = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a17", "name": "Ubuntu", "description": "",
            "org_id": null, "owner": null, "home": null, "address": "system.ubuntu",
            "kind": "environment", "driver": "docker", "spec": {}, "units": 3, "enabled": true,
            "revision": 1,
            "access": { "permissions": ["template.use"], "administers": false, "through": [] }
        });
        let resource = Resource::Template(serde_json::from_value(template.clone()).unwrap());
        assert_eq!(serde_json::to_value(&resource).unwrap(), template);
    }

    #[test]
    fn a_series_is_written_as_pairs_of_a_moment_and_a_number() {
        let series: MetricSeries = serde_json::from_value(json!({
            "name": "host_cpu_percent", "labels": { "host": "build-1" }, "resolution": "hour",
            "points": [[AT, 12.5]], "peaks": [[AT, 90.0]]
        }))
        .unwrap();
        assert_eq!(series.resolution, MetricResolution::Hour);
        assert_eq!(series.points.len(), 1);
        assert_eq!(series.peaks.as_ref().map(Vec::len), Some(1));
    }

    #[test]
    fn a_provider_registered_from_a_machine_is_a_subscription_of_its_profile_or_an_api() {
        let api: MachineProviderCreateBody = serde_json::from_value(json!({
            "kind": "api", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14", "name": "Local",
            "upstreams": [{ "protocol": "openai.chat", "base_url": "http://127.0.0.1:11434/v1", "options": {} }]
        }))
        .unwrap();
        assert!(matches!(api, MachineProviderCreateBody::Api { .. }));
        assert!(
            serde_json::from_value::<MachineProviderCreateBody>(json!({
                "kind": "api", "home_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a14", "name": "Local",
                "upstreams": [], "agent_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a11"
            }))
            .is_err()
        );
    }
}
