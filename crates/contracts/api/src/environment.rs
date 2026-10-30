//! Environments as resources: owners, home and address, what the caller holds, who reaches into it, configuration that is changed while it runs, what it declares it needs, what its exec node offers, the MCP servers found there and those of its own, the threads in it, its history, pending updates, removal and its reason, places with their warnings, storages, fleet and recycle.
//!
//! An [`Environment`] is where a thread's tools run: a workspace with its
//! files, processes and network, made from a template on a host or a directory
//! on a person's own machine. It belongs to a person, and the threads that
//! somebody who manages it put there share it. Where one could be made is a
//! [`Place`], with the reasons a host cannot be chosen and what it warns about;
//! who can reach into one, through its own page and its four doors (files, a
//! terminal, browsers and displays) or through an agent there, is an
//! [`EnvironmentReach`]; what it was found to have for browsers and displays is
//! its [`EnvironmentSupport`]; and a fleet view lists the environments under a
//! node with their load, to be recycled, moved or updated together.

use std::collections::BTreeMap;

use orch_types::{Id, Json, Revision, Seq, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::ResourceAccess;
use crate::binding::NewEnvironment;
use crate::forks::{
    AppliedChangeOptions, EnvironmentMovePlan, EnvironmentUpdatePlan, Estimate, ForkCreateBody,
    UpdateConfirmQuery,
};
use crate::mounts::{EnvMount, MountRef};
use crate::nodes::{JobRef, NodeRef};
use crate::resources::{HostRef, MachineRef, ProviderRef};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;
use crate::workspace::Job;

/// How something that runs work came to be where it is: made by the system, or
/// found on a person's own machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Placement {
    /// Made from a template on a host.
    Managed,
    /// On a person's machine, connected to the system.
    Connected,
}

/// Where an environment stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentStatus {
    /// A record only, until its first use.
    #[serde(rename = "none")]
    NotMade,
    /// Being made.
    Provisioning,
    /// Up.
    Running,
    /// Stopped.
    Stopped,
    /// Being replaced by a new unit.
    Recreating,
    /// Moving to another host.
    Moving,
    /// Failed.
    Failed,
    /// Its machine is not reachable.
    Offline,
    /// Explicitly declared permanently unreachable.
    Lost,
    /// Switched off.
    Disabled,
    /// Its unit was removed and owned files follow the recorded removal lifecycle.
    Removed,
}

/// An environment, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentRef {
    /// Current counts for compact choices; absent when this reference has no activity observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<EnvironmentActivity>,
    /// A currently effective wider opening, even when its target is stopped.
    pub ports_exposed: bool,
    /// The environment's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Who owns it.
    pub owner: UserRef,
    /// Where it comes from, as the short names of its home and its owner.
    pub address: String,
    /// How it came to exist; nothing more.
    pub placement: Placement,
    /// Where it stands.
    pub status: EnvironmentStatus,
}

/// Current activity counts without the identities of threads or shell owners.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentActivity {
    /// Non-side, undeleted threads whose state is running.
    pub running_threads: i64,
    /// Non-side, undeleted threads whose state is idle.
    pub idle_threads: i64,
    /// Non-side, undeleted threads whose state is blocked.
    pub blocked_threads: i64,
    /// Turns starting or running, including side threads.
    pub active_turns: i64,
    /// Shell sessions that have not closed.
    pub open_shells: i64,
}

/// The native sandbox levels, from the strictest to the widest; shared with the agent contract.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum SandboxMode {
    /// Read; nothing is written and nothing leaves the machine.
    ReadOnly,
    /// Write inside the thread's directory and writable mounts only.
    Workspace,
    /// Use everything the environment permits, without an agent sandbox.
    Open,
}

/// Where the effective widest sandbox comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SandboxLimitSource {
    /// The hosting machine's explicit agent.max_sandbox.
    MachinePolicy,
    /// The environment template's explicit sandbox.max.
    Template,
    /// A missing limit takes the confinement's cautious default.
    Default,
}

/// One comparison of the requested sandbox, the actual ceilings and reported enforcement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SandboxReadiness {
    /// Whether any reported mode can satisfy this request and all ceilings.
    pub ready: bool,
    /// The mode the place requests.
    pub requested: SandboxMode,
    /// The effective widest mode after the actual ceilings intersect.
    pub widest: SandboxMode,
    /// The setting or default that supplies that widest mode.
    pub source: SandboxLimitSource,
    /// The machine limit, explicit or its cautious default.
    pub machine_limit: SandboxMode,
    /// No template means no template limit at all.
    pub template_limit: Option<SandboxMode>,
    /// The executor used, or none for tools run by the harness itself.
    pub executor: Option<String>,
    /// The same executor, named in each language.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executor_ref: Option<crate::programs::ExecutorRef>,
    /// Modes the executor, or a local harness, actually reports.
    pub enforceable: Vec<SandboxMode>,
    /// The profile's supported modes; empty on an environment-only readiness check.
    pub agent_modes: Vec<SandboxMode>,
    /// A reported executor refusal or missing-facts explanation, without guessed capability.
    pub reason: Option<String>,
}

/// Readiness before creating a thread, and the same explanation on a blocked thread.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSandboxReadiness {
    /// The environment already visible to this caller.
    pub environment: EnvironmentRef,
    /// Its physical machine, when selected and reported.
    pub machine: Option<MachineRef>,
    /// Whether this caller may manage that machine and follow its policy act.
    pub policy_manage: bool,
    /// The common capability/limit comparison.
    pub sandbox: SandboxReadiness,
}

/// The thread beside which the caller works. It gives nothing: what a request needs is held on the
/// environment itself, whichever thread is named. It says where a path or a search starts when none
/// is given, and which thread a line of the history names.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ThroughThreadQuery {
    /// The thread; a client that opens the file panel of a thread names it here. It has to run in
    /// the environment and be one the caller may view.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<Id>,
}

/// What a unit is behind in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateWhat {
    /// Its template.
    Template,
    /// Its image.
    Image,
    /// Its settings.
    Settings,
    /// The connector.
    Connector,
    /// The executor.
    Executor,
    /// The harness.
    Harness,
}

/// One thing a unit is behind in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PendingUpdate {
    /// What it is.
    pub what: UpdateWhat,
    /// Its name.
    pub name: String,
    /// The version it has.
    pub from: Option<String>,
    /// The version it would have.
    pub to: Option<String>,
}

/// An amount of what a host has.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Amounts {
    /// Thousandths of a CPU.
    pub cpu_millis: i64,
    /// Megabytes of memory.
    pub memory_mb: i64,
    /// Gigabytes of disk.
    pub disk_gb: i64,
}

/// What is in use on a host, as measured.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceUsed {
    /// Percent of the CPU.
    pub cpu_percent: Option<f64>,
    /// Megabytes of memory.
    pub memory_mb: Option<i64>,
    /// Gigabytes of disk.
    pub disk_gb: Option<i64>,
}

/// How a host behaves once it is full.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WhenFull {
    /// The host can still be chosen; whether the unit is then made is up to the host.
    Warn,
    /// It is not offered.
    Block,
}

/// How many environments are on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceEnvironments {
    /// Running.
    pub running: i64,
    /// Idle.
    pub idle: i64,
}

/// A named storage of a host, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostStorage {
    /// Its name.
    pub name: String,
    /// Whether it is the default.
    pub default: bool,
    /// Its size in gigabytes.
    pub capacity_gb: i64,
    /// What is free, in gigabytes.
    pub free_gb: i64,
    /// Whether it can hold private views of mounts.
    pub private_views: bool,
}

/// A mount of the environment to be, and whether the host has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceMount {
    /// The mount.
    pub mount: MountRef,
    /// Whether it is present there.
    pub present: bool,
}

/// Why a host cannot be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostReason {
    /// Its driver cannot make this template.
    Driver,
    /// The template's source is not reachable from it.
    Source,
    /// Its labels do not match.
    Labels,
    /// A mount is not there.
    Mount,
    /// The storage is not there.
    Storage,
    /// It is full, and blocks when it is.
    Full,
    /// There is no room for what the environment declares it needs.
    NoRoom,
    /// It takes no new environment.
    Unschedulable,
    /// The caller's entry on it does not allow this.
    EntryLimit,
    /// The caller's entry on it does not allow this template.
    TemplateNotAllowed,
}

/// What a host warns about while it can still be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostWarning {
    /// It is full.
    Full,
    /// There is no room for what the environment declares it needs.
    NoRoom,
}

/// An authorized host that blocks creation. Absence of an offline timestamp is never guessed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlanHostBlocker {
    /// The host the caller may use, never a hidden candidate.
    pub host: HostRef,
    /// Its currently reported connection state.
    pub online: bool,
    /// Its first observed offline transition; absent while online or before an observation.
    pub offline_since: Option<Time>,
    /// The actual scheduler refusals.
    pub reasons: Vec<HostReason>,
    /// A fallback explanation for readers that do not know a refusal code.
    pub message: String,
}

/// A host an environment could be placed on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceHost {
    /// The host.
    pub host: HostRef,
    /// Its labels.
    pub labels: Vec<String>,
    /// Whose machine it is; `null` is the system's.
    pub owner: Option<UserRef>,
    /// What it has, as it reports it now.
    pub capacity: Amounts,
    /// What the units running on it say they need. A declaration reserves nothing.
    pub declared: Amounts,
    /// What is left of the capacity after that.
    pub free: Amounts,
    /// What is in use.
    pub used: PlaceUsed,
    /// What it does about `full` and `no_room`.
    pub when_full: WhenFull,
    /// How many environments are there.
    pub environments: PlaceEnvironments,
    /// Its named storages.
    pub storages: Vec<HostStorage>,
    /// Of the mounts the environment would attach, which are there.
    pub mounts: Vec<PlaceMount>,
    /// Whether it can be chosen.
    pub usable: bool,
    /// Why not, when not. `full` and `no_room` appear here only where the host
    /// blocks.
    pub reasons: Vec<HostReason>,
    /// A machine-policy refusal names its key, role and deciding operator/managers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_refusal: Option<String>,
    /// What it warns about; it can be chosen all the same.
    pub warnings: Vec<HostWarning>,
    /// Whether it is where the environment is now.
    pub current: bool,
    /// Whether it is what the scheduler would choose.
    pub automatic: bool,
}

/// What an existing environment runs on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceFacts {
    /// Its operating system.
    pub os: String,
    /// How many CPUs.
    pub cpus: Option<i64>,
    /// How much memory, in megabytes.
    pub memory_mb: Option<i64>,
}

/// Why an existing environment cannot be chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentReason {
    /// Its machine is not reachable.
    Offline,
    /// It is switched off.
    Disabled,
    /// It is being removed.
    Removing,
}

/// An existing environment the caller manages, as a place to run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlaceEnvironment {
    /// The environment.
    pub environment: EnvironmentRef,
    /// The host it is on, if managed.
    pub host: Option<HostRef>,
    /// The machine it is on, if it is a directory on one.
    pub machine: Option<MachineRef>,
    /// What it runs on.
    pub facts: PlaceFacts,
    /// How many threads are bound to it.
    pub threads: i64,
    /// The topics whose threads work there; a thread placed in it shares its workspace with those
    /// topics.
    pub topics: Vec<NodeRef>,
    /// How many more topics there are than the caller may view.
    pub topics_hidden: i64,
    /// Of the mounts the environment would attach, which are there.
    pub mounts: Vec<PlaceMount>,
    /// Whether it can be chosen.
    pub usable: bool,
    /// Why not, when not.
    pub reasons: Vec<EnvironmentReason>,
    /// Whether it is where the thread is now.
    pub current: bool,
}

/// Somewhere a thread's tools could run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Place {
    /// A host that the caller may use for a new environment.
    Host(PlaceHost),
    /// An environment that exists.
    Environment(PlaceEnvironment),
}

/// A thread that works in an environment, as far as the caller may see it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentThread {
    /// The thread.
    pub thread: NodeRef,
    /// The topic it belongs to.
    pub topic: NodeRef,
    /// Its directory in the workspace.
    pub directory: String,
    /// Its first owner.
    pub owner: UserRef,
}

/// One thing an environment's exec node may offer a harness that runs elsewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ExecOffer {
    /// An executor kind, or a tool set of Orch's own.
    pub name: String,
    /// Whether it is there now.
    pub available: bool,
    /// Why not, when not.
    pub reason: Option<String>,
}

/// What an environment's exec node offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentExec {
    /// The offers, in the order in which they are tried.
    pub offers: Vec<ExecOffer>,
    /// Whether its agents get Orch's own tools.
    pub orch_tools: bool,
    /// On a person's machine: what its local policy lets it offer; `null` elsewhere.
    pub machine_offers: Option<Vec<String>>,
}

/// Where an MCP server was found, or was configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpScope {
    /// In the user's own configuration of the harness.
    User,
    /// In a directory of the workspace.
    Project,
}

/// How an MCP server is reached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    /// By a process it starts.
    Stdio,
    /// By an address.
    Http,
}

/// An MCP server found in the configuration of a harness that is installed in an environment.
/// None is used until somebody imports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpFound {
    /// What it is called there.
    pub name: String,
    /// The harness whose configuration it is in, such as `codex` or `claude-code`.
    pub source: String,
    /// Whether the user's own configuration or a directory of the workspace has it.
    pub scope: McpScope,
    /// For a project, the directory.
    pub directory: Option<String>,
    /// How it is reached.
    pub transport: McpTransport,
    /// The server it became, once it was imported.
    pub imported: Option<Id>,
}

/// Whether the tree of an environment is indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentIndex {
    /// There is a manifest of every file.
    Full,
    /// The tree is too large to index, or was set so: there is no manifest.
    Live,
}

/// The template an environment was made from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentTemplate {
    /// The template's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// The driver that makes units from it.
    pub driver: String,
}

/// Why an environment is to be removed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentRemoveReason {
    /// A person asked.
    Request,
    /// A switch left it without a thread.
    Switch,
    /// It was unused for as long as the setting `env.unused_remove_days` allows.
    Unused,
}

/// A program found in an environment that can be started for a browser or a display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSupportProgram {
    /// What it is called.
    pub name: String,
    /// Where it lies in the environment.
    pub path: String,
    /// Its version, where it says one.
    pub version: Option<String>,
}

/// What an environment has to run a browser.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSupportBrowser {
    /// Browsers of the Chromium family found there, the one Orch starts first coming first.
    pub programs: Vec<EnvironmentSupportProgram>,
    /// Whether one can be started with a window, so that a display can be opened for it.
    pub window: bool,
    /// The version of the command line an agent drives a browser with, where it is installed.
    pub playwright_cli: Option<String>,
    /// Whether a browser's own sandbox can be had there; `null` until one was started.
    pub sandbox: Option<bool>,
}

/// What an environment has to serve a display.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSupportVnc {
    /// Programs found there that serve a display Orch can open.
    pub servers: Vec<EnvironmentSupportProgram>,
    /// The window manager that is started on a display Orch opens, where there is one.
    pub window_manager: Option<String>,
}

/// What an environment was found to have for browsers and displays. Nothing is switched off
/// because nothing was found: the settings and the switches say whether they are wanted, and this
/// says whether they can be had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSupport {
    /// When the environment looked; `null` while it has not attached yet.
    pub looked_at: Option<Time>,
    /// What it has for browsers.
    pub browser: EnvironmentSupportBrowser,
    /// What it has for displays.
    pub vnc: EnvironmentSupportVnc,
}

impl EnvironmentSupport {
    /// What an environment has where nothing was found: no program, no window, no command line.
    #[must_use]
    pub fn nothing_found() -> Self {
        Self {
            looked_at: None,
            browser: EnvironmentSupportBrowser {
                programs: Vec::new(),
                window: false,
                playwright_cli: None,
                sandbox: None,
            },
            vnc: EnvironmentSupportVnc {
                servers: Vec::new(),
                window_manager: None,
            },
        }
    }
}

/// How far a unit may reach outward.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EgressMode {
    /// Wherever its network lets it.
    #[default]
    Open,
    /// Orch's own transport and the other names its place allows, through its host's proxy.
    Restricted,
}

/// What network a unit stands on.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitNetworkMode {
    /// The pool's network of its host.
    #[default]
    Bridge,
    /// A network of its own whose only way out is its proxy, to Orch alone.
    None,
    /// A person's machine, whose network is the machine's.
    Connected,
}

/// What a trusted Orch endpoint is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkTransportPurpose {
    /// The gateway a connector keeps its link to.
    Gateway,
    /// A remote model forwarder.
    Model,
    /// The origin blobs are fetched from.
    Blob,
}

/// A trusted Orch endpoint a restricted unit always reaches: an exact authority, protocol and port,
/// with no path, query, token or credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct NetworkTransport {
    /// The host name.
    pub authority: String,
    /// The protocol spoken over it.
    pub protocol: String,
    /// The port.
    pub port: u16,
    /// What it is for.
    pub purpose: NetworkTransportPurpose,
}

/// Where the trusted endpoints come from: Orch's configuration, and nobody's setting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkAlwaysSource {
    /// Orch's own configuration.
    #[default]
    OrchConfiguration,
}

/// Whether a unit's network is as its policy says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkReadiness {
    /// In force, and what it admits flows.
    #[default]
    Ready,
    /// Not in force: outward traffic is closed until it is.
    Unavailable,
}

/// One scope whose list of other host names takes part in a unit's.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkStateSource {
    /// The scope; `null` for the system's own value.
    pub scope: Option<NodeRef>,
    /// The names it lists.
    pub hosts: Vec<String>,
}

/// The last request a unit's proxy refused: its host and why, and nothing of the request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkStateLastDenied {
    /// The host the request named.
    pub host: String,
    /// When.
    pub at: Time,
    /// Why.
    pub reason: String,
}

/// A unit's network: the mode it has and the one asked for, the other names and where they come
/// from, the trusted endpoints, the revisions in force, whether it is ready, and the last refusal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkState {
    /// The mode it has.
    pub mode: EgressMode,
    /// The mode asked of it, which a failed restriction keeps beside the mode it has.
    pub required_mode: EgressMode,
    /// The network it stands on.
    pub network_mode: UnitNetworkMode,
    /// Where its programs find its proxy; `null` for an open unit.
    pub proxy_url: Option<String>,
    /// The other host names it reaches: only the resolved other names.
    pub allowed_hosts: Vec<String>,
    /// The scopes whose lists made them.
    pub sources: Vec<NetworkStateSource>,
    /// The revision of the other names in force.
    pub revision: String,
    /// The trusted Orch endpoints, always reached.
    pub always: Vec<NetworkTransport>,
    /// Their revision.
    pub always_revision: String,
    /// Where they come from.
    pub always_source: NetworkAlwaysSource,
    /// Whether it is in force.
    pub state: NetworkReadiness,
    /// Why not, in a sentence fit to show.
    pub reason: Option<String>,
    /// The last request refused.
    pub last_denied: Option<NetworkStateLastDenied>,
    /// The networks of its own the unit and its proxy stand on on their host, each with its subnet, the host's
    /// address there and theirs; empty where the host has said none.
    #[serde(default)]
    pub networks: Vec<NetworkStateAddress>,
}

/// One network a unit, or its proxy, stands on on its host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct NetworkStateAddress {
    /// The network's name on the host.
    pub name: String,
    /// `unit` for the unit's own network, `egress` for the one a restricted unit's proxy goes out on.
    pub role: NetworkStateAddressRole,
    /// Its subnet, as `address/prefix`.
    pub subnet: String,
    /// The host's address on it; `null` where its bridge has none.
    pub gateway: Option<String>,
    /// The address of the unit, or of its proxy, there; `null` while it is not running.
    pub address: Option<String>,
}

/// What a network is to the unit that stands on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum NetworkStateAddressRole {
    /// The unit's own network.
    Unit,
    /// The network a restricted unit's proxy goes out on.
    Egress,
}

impl NetworkState {
    /// The network of an open unit on `network_mode`: nothing restricted, in force.
    #[must_use]
    pub fn open(network_mode: UnitNetworkMode) -> Self {
        Self {
            mode: EgressMode::Open,
            required_mode: EgressMode::Open,
            network_mode,
            proxy_url: None,
            allowed_hosts: Vec::new(),
            sources: Vec::new(),
            revision: String::new(),
            always: Vec::new(),
            always_revision: String::new(),
            always_source: NetworkAlwaysSource::OrchConfiguration,
            state: NetworkReadiness::Ready,
            reason: None,
            last_denied: None,
            networks: Vec::new(),
        }
    }
}

/// Why a workspace is listed live instead of indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentIndexReason {
    /// Its setting explicitly chose live listing.
    Chosen,
    /// It crossed the entry limit.
    Entries,
    /// The platform could not watch the tree.
    Watches,
    /// One scan took too long.
    ScanTime,
}

/// An environment. It belongs to a person, who may let others manage it; the threads that
/// somebody who manages it put there share its files, processes and network, each from a
/// directory of its own.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Environment {
    /// Whether the current unavailable unit or machine permits an explicit loss plan.
    #[serde(default, skip_serializing_if = "crate::common::is_false")]
    pub can_declare_lost: bool,
    /// Temporary cleanup timing; absent for lasting environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub temporary: Option<EnvironmentTemporary>,
    /// Its connector's hello version compared with the server; it retains the environment's own Update workflow.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connector_update: Option<crate::resources::ComponentUpdate>,
    /// Independent workspace/home source selections; legacy environments may not have a selection yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// The current driver's declaration and measured use, available on the environment's own page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime_changes: Option<orch_driver_contract::runtime::RuntimeChanges>,
    /// What it runs on and under: the driver, the runtime with what holds it in, and the limits there that touch
    /// what it has or can be given. Computed from its host's declaration, so that an environment made before
    /// runtimes were declared shows it too; absent where the host has not reported.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isolation: Option<orch_driver_contract::isolation::RuntimeIsolation>,
    /// What names it wherever it is pointed at.
    #[serde(flatten)]
    pub base: EnvironmentRef,
    /// The root org of its home.
    pub org_id: Id,
    /// The machine it is on, when it is a directory on a person's machine.
    pub machine: Option<MachineRef>,
    /// Its unit's network: the policy in force, not a claim about a separated provider's traffic.
    pub network: NetworkState,
    /// The topic, project or org whose settings it takes, and whose defaults its page follows.
    pub home: NodeRef,
    /// The places that explicitly choose it as their inherited default, that the caller may view.
    pub default_for: Vec<NodeRef>,
    /// The workspace's own configuration of each harness that found one here, and the answer given for it;
    /// `answer` null while the question waits.
    pub config_trust: Vec<crate::trust::ConfigTrust>,
    /// The threads bound to it that the caller may view.
    pub threads: Vec<EnvironmentThread>,
    /// How many more threads are bound to it than the caller may view.
    pub threads_hidden: i64,
    /// How many people reach into it now: through one of its doors, or through an agent there that
    /// they may talk to.
    pub reach: i64,
    /// The absolute path of its workspace: its own, and the boundary of what its Files rows reach.
    pub workspace_root: String,
    /// Opaque equality identity for the current workspace source, also in a limited reader's cut.
    /// Contains no source location or machine identity; it grants no access. Older peers omit it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_incarnation: Option<String>,
    /// For a managed one, the storage of its host that its workspace volume is on.
    pub workspace_storage: Option<String>,
    /// Its own values for the keys of scope `environment`.
    pub config: Json,
    /// For a managed one, the host it is on once it has volumes there.
    pub host: Option<HostRef>,
    /// What it attaches, and what became of each here.
    pub mounts: Vec<EnvMount>,
    /// What its exec node offers.
    pub exec: EnvironmentExec,
    /// The MCP servers found in the configuration of the harnesses there.
    pub mcp_found: Vec<McpFound>,
    /// Whether its tree is indexed.
    pub index: EnvironmentIndex,
    /// Why it is live; null when indexed or when the machine has not reported a reason.
    pub index_reason: Option<EnvironmentIndexReason>,
    /// The template it was made from.
    pub template: Option<EnvironmentTemplate>,
    /// How many units it has had.
    pub generation: i64,
    /// What it is behind in; empty means up to date.
    pub updates: Vec<PendingUpdate>,
    /// Whether it cannot attach until it is updated.
    pub update_required: bool,
    /// Whether files may be written from outside. With its other switches it is the ceiling of a
    /// door: with a switch off that door is shut to everybody its page names, its managers among
    /// them.
    pub remote_write: bool,
    /// Whether shells may be opened in it.
    pub terminal: bool,
    /// Whether browsers may be shown, started and driven in it.
    pub browser: bool,
    /// Whether displays may be shown, started and worked in in it.
    pub vnc: bool,
    /// What it was found to have for browsers and displays. It looks when it attaches, and again
    /// when it is asked; where nothing was found, the members are empty.
    pub support: EnvironmentSupport,
    /// The capabilities granted to it.
    pub granted_capabilities: Vec<String>,
    /// A sentence on its status, where there is one.
    pub status_detail: Option<String>,
    /// What its machine reported about it.
    pub facts: Json,
    /// When its manifest was taken.
    pub manifest_at: Option<Time>,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
    /// When the idle timeout will stop it; `null` for never, or when it is not running.
    pub idle_stop_at: Option<Time>,
    /// Since when no thread, other than archived ones, is bound to it.
    pub unused_since: Option<Time>,
    /// The retained-volume lifecycle and this reader's current actions.
    pub removal: Option<EnvironmentRemoval>,
    /// Read-only inspection of retained deleted metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inspection: Option<crate::nodes::DeletedInspection>,
    /// When its removal is scheduled for.
    pub remove_at: Option<Time>,
    /// Why.
    pub remove_reason: Option<EnvironmentRemoveReason>,
    /// A provision, recreation, update, move or removal under way.
    pub job: Option<JobRef>,
    /// The sequence of its feed.
    pub seq: Seq,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// The owner's opportunity to keep or export a disposable environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentTemporary {
    /// No predicted deadline is shown while a turn is actively using it.
    pub expires_at: Option<Time>,
    /// The place's current idle limit, in hours.
    pub idle_hours: i64,
    /// Its creating thread's archive also starts cleanup.
    pub on_archive: bool,
}

/// The one value of a member that names the template an environment was made from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TemplateMark {
    /// The environment's template.
    Template,
}

/// Which scope switches an MCP server on for an environment.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum McpFrom {
    /// The environment's template.
    Template(TemplateMark),
    /// The place whose `mcp.enabled` does.
    Node(NodeRef),
}

/// How an MCP server stands in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpState {
    /// Its process and protocol are being started.
    Starting,
    /// Running.
    Running,
    /// Stopped.
    Stopped,
    /// It failed.
    Failed,
    /// It cannot be had there.
    Unavailable,
}

/// An MCP server, named where an environment lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMcpServerRef {
    /// The server's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Where it comes from, as short names.
    pub address: String,
    /// For one imported from a machine's harness configuration, that machine.
    pub machine: Option<MachineRef>,
    /// Whether it is this environment's own.
    pub own: bool,
}

/// An MCP server that is switched on for an environment, and what became of it there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMcpServer {
    /// Batched giving and reported calls, where the caller may read this server's resource.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub library: Option<crate::resource_facts::ResourceFacts>,
    /// The server.
    pub server: EnvironmentMcpServerRef,
    /// The scope whose `mcp.enabled` switches it on, or its template; `null` is the
    /// environment's own value.
    pub from: Option<McpFrom>,
    /// How it stands.
    pub state: McpState,
    /// For one that failed, its exit and last stderr lines with known secrets masked; for one that is unavailable, why: a command or a capability it requires is missing, it was
    /// imported from another machine, whoever switched it on may not use it, or two share a
    /// name.
    pub reason: Option<String>,
    /// How many tools it listed when it last started.
    pub tools: Option<i64>,
}

/// When a change to an environment's MCP servers reaches a thread that is at work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpApplies {
    /// At once, where every attached harness declares live MCP reload.
    Immediately,
    /// With its next turn, where its harness takes a new set between turns.
    NextTurn,
    /// With its next session. Nothing here recreates the environment.
    NextSession,
}

/// Why a caller may not import an MCP server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpImportReason {
    /// The machine's local policy lets none be imported.
    Policy,
    /// The caller is not who may.
    NotOwner,
}

/// Whether the caller may import an MCP server found in an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct McpImportRight {
    /// Whether they may.
    pub allowed: bool,
    /// Why not, when not.
    pub reason: Option<McpImportReason>,
}

/// The MCP servers of one environment: those switched on for it and what became of each there,
/// and those found in the harness configuration there that nobody imported yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMcp {
    /// The servers switched on for it.
    pub servers: Vec<EnvironmentMcpServer>,
    /// When a change here reaches a thread that is at work.
    pub applies: McpApplies,
    /// What the harnesses there have configured, imported or not.
    pub found: Vec<McpFound>,
    /// Whether the caller may import one.
    pub import: McpImportRight,
}

/// The permission by which a person reaches into an environment through its own page: managing it,
/// or one of its four doors, of which the highest level they hold is named, once for each door.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ReachPagePermission {
    /// Everything in it, which holds the Manage of all four doors.
    #[serde(rename = "environment.manage")]
    EnvironmentManage,
    /// The files, written or read.
    #[serde(rename = "files.manage")]
    FilesManage,
    /// The files, read.
    #[serde(rename = "files.view")]
    FilesView,
    /// A shell, and with it everything.
    #[serde(rename = "terminal.manage")]
    TerminalManage,
    /// The processes, read.
    #[serde(rename = "terminal.view")]
    TerminalView,
    /// A browser, driven.
    #[serde(rename = "browser.manage")]
    BrowserManage,
    /// The browsers, watched.
    #[serde(rename = "browser.view")]
    BrowserView,
    /// A display, worked in.
    #[serde(rename = "vnc.manage")]
    VncManage,
    /// The displays, watched.
    #[serde(rename = "vnc.view")]
    VncView,
}

/// The permission on a thread by which a person reaches into the environment it runs in: talking to
/// its agent, which does there whatever it is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub enum ReachThreadPermission {
    /// The agent.
    #[serde(rename = "thread.interact")]
    ThreadInteract,
}

/// How a person reaches into an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ReachThrough {
    /// They own it.
    Owner,
    /// They own its physical supplying resource, independently of this page's Owners row.
    SourceOwner,
    /// They are named on its own page: managing it, or in one of its doors.
    Page {
        /// The permission.
        permission: ReachPagePermission,
    },
    /// A thread that runs there lets them talk to its agent.
    Thread {
        /// The thread.
        thread: NodeRef,
        /// The permission.
        permission: ReachThreadPermission,
    },
}

/// A person who reaches into an environment, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ReachPerson {
    /// The person.
    pub user: UserRef,
    /// By what they reach it, each way listed.
    pub through: Vec<ReachThrough>,
}

/// Everybody who can reach into an environment now, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentReach {
    /// The people.
    pub people: Vec<ReachPerson>,
    /// How many people reach it only through threads the caller may not view.
    pub hidden: i64,
}

/// One environment of a fleet view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct FleetEntry {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Its home; `null` when the caller may not view it.
    pub home: Option<NodeRef>,
    /// How many threads are bound to it.
    pub threads: i64,
    /// The name of its host.
    pub host_name: Option<String>,
    /// The template it was made from.
    pub template: Option<TemplateRef>,
    /// How many units it has had.
    pub generation: i64,
    /// How many updates wait.
    pub updates: i64,
    /// When something last happened in it.
    pub last_activity_at: Option<Time>,
    /// For how many minutes nothing has.
    pub idle_minutes: Option<i64>,
    /// When the idle timeout will stop it.
    pub idle_stop_at: Option<Time>,
    /// How many turns run in it.
    pub active_turns: i64,
    /// How many shells are open in it.
    pub open_shells: i64,
    /// How many processes run in it.
    pub processes: Option<i64>,
    /// How much of the CPU it uses, in percent of one CPU.
    pub cpu_percent: Option<f64>,
    /// How much memory it holds.
    pub memory_bytes: Option<i64>,
    /// The most memory it may hold.
    pub memory_limit_bytes: Option<i64>,
    /// How much disk it uses.
    pub disk_used_bytes: Option<i64>,
    /// Whether the caller holds `environment.manage` on it.
    pub manageable: bool,
    /// The providers whose credential is handed over to it; stopping it returns them first.
    pub holds: Vec<ProviderRef>,
}

/// A template, named by what it is called.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct TemplateRef {
    /// The template's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// How loaded a host is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct HostLoad {
    /// Removed environments with data still referenced on this host.
    pub kept_environments: i64,
    /// Measured kept bytes; unknown sizes are counted separately.
    pub kept_bytes: i64,
    /// Kept environments whose size has not been measured.
    pub kept_sizes_unknown: i64,
    /// The host.
    pub host_id: Id,
    /// Its name.
    pub name: String,
    /// The memory the units on it declare they need, in megabytes.
    pub memory_declared_mb: i64,
    /// The memory it has, in megabytes.
    pub memory_capacity_mb: i64,
    /// How many CPUs it has.
    pub cpus: i64,
    /// How many environments run on it.
    pub environments_running: i64,
    /// Whether it takes new environments.
    pub schedulable: bool,
}

/// Why an environment is left out of a batch of moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MoveSkipReason {
    /// The caller may not manage it.
    NoPermission,
    /// It cannot be moved: it is a directory on a person's machine, or was not made from a
    /// template.
    NotMovable,
}

/// An environment left out of a batch of moves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMoveSkipped {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Why.
    pub reason: MoveSkipReason,
}

/// Several environments moved together.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentMoveBatchPlan {
    /// The unique audience for one explicit notification choice.
    #[serde(default)]
    pub notification: crate::loss::OperationNotificationPlan,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// One plan for each environment that is moved, each with its own blockers.
    pub plans: Vec<EnvironmentMovePlan>,
    /// The environments left out.
    pub skipped: Vec<EnvironmentMoveSkipped>,
    /// How many plans have no blockers.
    pub executable: i64,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
}

/// Why an environment is left out of a batch of updates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdateSkipReason {
    /// The caller may not manage it.
    NoPermission,
    /// It is not behind in anything.
    UpToDate,
    /// It cannot be updated.
    NotUpdatable,
}

/// An environment left out of a batch of updates.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentUpdateSkipped {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Why.
    pub reason: UpdateSkipReason,
}

/// Several environments updated together.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentUpdateBatchPlan {
    /// Distinct people the optional notice reaches across this batch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notify: Option<crate::forks::UpdateAudience>,
    /// The plan's identifier, which carries it out.
    pub preflight_id: Id,
    /// When it goes out of date.
    pub expires_at: Time,
    /// One plan for each environment that is updated, each with its own blockers.
    pub plans: Vec<EnvironmentUpdatePlan>,
    /// The environments left out.
    pub skipped: Vec<EnvironmentUpdateSkipped>,
    /// How many plans have no blockers.
    pub executable: i64,
    /// The values that will apply.
    pub options: AppliedChangeOptions,
    /// How long it takes.
    pub estimate: Estimate,
}

/// The job of one environment of a batch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBatchJob {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Its job.
    pub job: Job,
}

/// A batch of moves or of updates being carried out, one job for each plan without blockers, a
/// few at a time and each rolling back alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentBatch {
    /// The batch.
    pub batch_id: Id,
    /// The jobs.
    pub jobs: Vec<EnvironmentBatchJob>,
}

/// Which environments the caller holds a permission on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentHolding {
    /// Any permission that lets them see it.
    View,
    /// `environment.manage`.
    Manage,
}

/// The environments of the caller.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentListQuery {
    /// Only those whose home is that org or an org inside it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
    /// Only those on which the caller holds this much.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub holding: Option<EnvironmentHolding>,
}

/// Where an environment could be made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PlaceListQuery {
    /// The template to be made from; what the node resolves when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Id>,
    /// The environment's own configuration, as JSON text; what the node resolves when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<String>,
}

/// What an environment's feed is read from.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentSyncGetQuery {
    /// Only what changed after this sequence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since: Option<Seq>,
}

/// The state behind an environment's feed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSyncGetReply {
    /// The environment.
    pub environment: Environment,
    /// The sequence of its feed now.
    pub seq: Seq,
    /// The sequence the request started from.
    pub since: Option<Seq>,
    /// False when too much changed for the answer to hold it, and the client reads again from
    /// the start.
    pub complete: bool,
    /// The jobs under way about it.
    pub jobs: Vec<EnvironmentSyncJob>,
}

/// A feed job with its full permitted detail, or the current job's safe progress alone.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum EnvironmentSyncJob {
    /// A job whose detail this caller may read.
    Full(Box<Job>),
    /// Progress without logs, results, people or historical work.
    Reference(JobRef),
}

/// Changes an environment. A change the running unit cannot take is stored and listed in
/// `updates`; what its exec node offers is part of `config`, takes effect for each thread when
/// its next session opens, and on a person's machine is refused beyond what that machine's
/// local policy offers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentUpdateBody {
    /// Independent data root choices. Existing directories are checked by the machine's own policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Its own values for the keys of scope `environment`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config: Option<Json>,
    /// Whether files may be written from outside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_write: Option<bool>,
    /// Whether shells may be opened in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
    /// Whether browsers may be shown, started and driven in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<bool>,
    /// Whether displays may be shown, started and worked in in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vnc: Option<bool>,
    /// Another home: a node of the same root org on which the caller may create environments. It
    /// takes that node's settings and secrets from then on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_node_id: Option<Id>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Imports an MCP server found in the configuration of a harness there: the connector keeps a
/// copy of its definition, which is what runs from then on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMcpImportCreateBody {
    /// The harness whose configuration it is in.
    pub source: String,
    /// The user's own configuration, or a directory of the workspace.
    pub scope: McpScope,
    /// What it is called there.
    pub name: String,
    /// For a project, the directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<String>,
    /// Another name to give it.
    #[serde(default, rename = "as", skip_serializing_if = "Option::is_none")]
    pub as_name: Option<String>,
    /// Whether to switch it on for this environment in the same step.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
}

/// Writes an MCP server of an environment's own: for a tool that is wanted there now and nowhere
/// else. It is on there at once, is listed and switched with the environment's other servers, is
/// changed and removed by whoever manages the environment, and goes when the environment does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMcpServerCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// How it is started and reached, as a preset's is.
    pub spec: Json,
    /// Secrets it needs, by name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secrets: Option<BTreeMap<String, String>>,
    /// Whether to switch it on for the environment; it is unless this is false.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enable: Option<bool>,
}

/// Stops an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentStopBody {
    /// Whether to interrupt the turns that run in it.
    pub interrupt: bool,
}

/// When the same-unit restart may interrupt unfinished turns.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentRestartMode {
    /// Let turns finish during the configured recreate grace, then interrupt any still running.
    #[default]
    Wait,
    /// Interrupt the running turns as soon as the restart begins.
    Interrupt,
}

/// A restart keeps the same unit and every file; it creates no successor.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRestartBody {
    /// Wait under the configured grace or interrupt immediately.
    #[serde(default)]
    pub mode: EnvironmentRestartMode,
}

/// What a plan for new units is asked to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UpdatePreflightReason {
    /// Apply everything the unit is behind in.
    Update,
    /// Make a new unit as it is.
    Recreate,
}

/// Asks for a plan to update, or to recreate, the unit of an environment or of a provider's
/// runtime.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UpdatePreflightBody {
    /// Explicitly copy a provider's existing home into new owned storage on another host.
    /// The original directory is preserved. False unless the reader asks for the copy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_directories: Option<bool>,
    /// What the plan is for; `update` when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<UpdatePreflightReason>,
}

/// Which environment to remove, and when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentDeleteQuery {
    /// The environment's name, to be sure.
    pub confirm: String,
    /// When to remove it; now when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub at: Option<Time>,
}

/// Asks for a plan to move reviewed environment data to another permitted machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentMovePreflightBody {
    /// An explicitly selected machine whose directory services make the destination data.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<Id>,
    /// The template used when a directory moves into a unit; otherwise the current or inherited template.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Id>,
    /// Explicitly copy existing directories into new storage made by Orch on the destination.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_directories: Option<bool>,
    /// The host; wherever there is most room among the hosts the caller may use when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
    /// Whether the home directory goes along.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home: Option<bool>,
    /// Whether what was written in private views goes along.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub private_views: Option<bool>,
    /// The paths to copy; an explicit selection is required for a workspace listed live.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub files: Option<crate::forks::FileSelection>,
    /// What becomes of each mount on the destination; each one left out takes its manager's default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mounts: Option<Vec<crate::forks::MountChoice>>,
    /// Compare attached read-only synced locations with their source now; omitted or false reads no contents.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub check_fingerprints: Option<bool>,
    /// Identifies one explicit content check across its live progress and subsequent replans.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fingerprint_request_id: Option<Id>,
}

/// A machine offering directory lifecycles, with its own policy and declared data-making support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MoveMachine {
    /// The destination machine; its managers choose its policy and storages.
    pub machine: MachineRef,
    /// Its host page, if the same machine also offers units.
    pub host: Option<HostRef>,
    /// Whether its reported facts and current policy allow this choice.
    pub usable: bool,
    /// Why it cannot be chosen; each is a declared fact or policy refusal.
    pub reasons: Vec<String>,
}

/// One move operation chooses either a declared unit lifecycle or a directory lifecycle.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum EnvironmentMovePlace {
    /// A host making a unit from the selected template.
    Host(Box<PlaceHost>),
    /// A machine attaching fresh data made by Orch.
    Machine(MoveMachine),
}

/// The environments under a node to list.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeEnvironmentListQuery {
    /// Only those in this state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<EnvironmentStatus>,
    /// Only those that have been idle at least this many minutes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_minutes: Option<u32>,
    /// Only those with updates waiting.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<bool>,
}

/// The environments that threads under a node run in and those whose home is there, as far as
/// the caller may see them, with the hosts the caller manages.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct NodeEnvironmentListReply {
    /// The environments.
    pub entries: Vec<FleetEntry>,
    /// The hosts the caller manages.
    pub hosts: Vec<HostLoad>,
}

/// Stops the environments that have been idle for long enough. An environment with an active
/// turn or an open shell is never stopped; one with running agent processes only when
/// `include_with_processes` is true. Stopping keeps every volume, and the next turn starts the
/// environment again.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RecycleBody {
    /// How many minutes without activity; any value from zero.
    pub idle_minutes: u32,
    /// Whether to only list what would be stopped.
    pub dry_run: bool,
    /// Whether to stop environments with running agent processes too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_with_processes: Option<bool>,
}

/// What a recycle stopped, or with `dry_run` would stop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct RecycleReply {
    /// The environments on the list.
    pub candidates: Vec<FleetEntry>,
    /// How many were stopped.
    pub stopped: i64,
    /// How many were left alone because the caller may not manage them.
    pub skipped_no_permission: i64,
    /// How many were left alone because they are active.
    pub skipped_active: i64,
}

/// Asks for a plan to move several environments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeEnvironmentMovePreflightBody {
    /// A machine offering a directory lifecycle, instead of a host making units.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_id: Option<Id>,
    /// A unit template when the selected sources have no current unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template_id: Option<Id>,
    /// Explicit consent to copy existing directories while leaving their originals untouched.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_directories: Option<bool>,
    /// The environments, with either a unit or directory lifecycle.
    pub environment_ids: Vec<Id>,
    /// The host; wherever there is most room for each when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<Id>,
}

/// Asks for a plan to update several environments.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeEnvironmentUpdatePreflightBody {
    /// The environments.
    pub environment_ids: Vec<Id>,
}

/// Registers a directory on the caller's own machine as an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineEnvironmentCreateBody {
    /// Data root choices. When absent the legacy root selects an existing workspace directory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data_sources: Option<orch_driver_contract::runtime::EnvironmentDataSources>,
    /// Its home org: the caller's domain or an org inside it.
    pub home_id: Id,
    /// Its name.
    pub name: String,
    /// The directory that becomes the workspace root; the machine checks it against its local
    /// policy.
    #[serde(default)]
    pub root: String,
    /// A project or topic of that org to take settings from instead of the org.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_node_id: Option<Id>,
    /// Whether files may be written from outside.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remote_write: Option<bool>,
    /// Whether shells may be opened in it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal: Option<bool>,
    /// Who may use it: as the defaults of its home say, or its maker alone.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<crate::binding::NewEnvironmentAccess>,
}

/// The durable phase of environment removal.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentRemovalPhase {
    /// Waiting for unit removal.
    Scheduled,
    /// Removing the unit.
    Removing,
    /// Keeping the owned volumes.
    Kept,
    /// Reserving volumes for restoration.
    Restoring,
    /// Removing the kept volumes.
    Purging,
    /// Physical removal was confirmed.
    Purged,
    /// The host was declared lost; deletion was never confirmed.
    Lost,
}

/// Retained files, their promise and the actions this reader may take.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentRemoval {
    /// The recorded phase.
    pub phase: EnvironmentRemovalPhase,
    /// Actual unit removal time.
    pub deleted_at: Option<Time>,
    /// The person whose accepted removal this was.
    pub deleted_by: Option<UserRef>,
    /// Days frozen when removal was accepted.
    pub keep_days: i64,
    /// Actual unit removal time plus the frozen window; unknown until removal finishes.
    pub purge_at: Option<Time>,
    /// Actual confirmed volume removal time.
    pub purged_at: Option<Time>,
    /// Measured bytes, or unknown.
    pub bytes: Option<i64>,
    /// When the size was measured.
    pub size_at: Option<Time>,
    /// Safe unresolved reason, including an unknown measurement.
    pub reason: Option<String>,
    /// The deletion revision used by restore and purge.
    pub revision: Revision,
    /// Whether this reader may inspect retained metadata.
    pub can_inspect: bool,
    /// Whether this reader may reserve the kept volumes for restoration.
    pub can_restore: bool,
    /// Whether this reader may confirm permanent removal.
    pub can_purge: bool,
    /// The lifecycle job, if any.
    pub job: Option<JobRef>,
}

/// One inspectable environment kept after removal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DeletedEnvironment {
    /// The environment.
    pub environment: EnvironmentRef,
    /// Its current home.
    pub home: NodeRef,
    /// The host that keeps the files.
    pub host: Option<HostRef>,
    /// Workspace storage where known.
    pub storage: Option<String>,
    /// The kept-volume facts and current acts.
    pub removal: EnvironmentRemoval,
}

/// Selects kept environments in a place.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NodeDeletedEnvironmentListQuery {
    /// Also include environments whose home lies below this place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub under: Option<bool>,
    /// Maximum entries.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// Previous page's cursor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
}

/// Reserves a kept environment at its removal revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentRestoreBody {
    /// The current removal revision.
    pub expected_revision: Revision,
}

/// Confirms permanent volume removal at its removal revision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EnvironmentPurgeQuery {
    /// The environment's current name.
    pub confirm: String,
    /// The current removal revision.
    pub expected_revision: Revision,
}

/// What the agents of an environment can reach, as far as the caller may see it: its network, the names of the
/// secrets its commands are given, the mounts attached to it, where each thread of it the caller may view keeps its
/// model key, and the browser profiles whose sign-ins it keeps. It is read from what is already known and fetches
/// nothing: no key, secret value, cookie or browsing history is ever in it. A member the caller may not see is `null`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReachOverview {
    /// Native home ancestry for inherited-setting updates, with names redacted by the path reader.
    /// Absent for older replies and for a reader admitted only through a thread.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub home_path: Option<Vec<NodeRef>>,
    /// Its network, as the environment's own view shows it to the caller.
    pub network: NetworkState,
    /// The names of the secrets its commands are given.
    pub secrets: AgentReachOverviewSecrets,
    /// The mounts attached to it, with the path the agent finds each at and how it is attached; `null` for a caller
    /// who may not read its files.
    pub mounts: Option<Vec<AgentReachOverviewMount>>,
    /// Where each thread of it the caller may view keeps its model key: one line for each, since threads of one
    /// environment can have different arrangements. A thread the caller may not view is not named.
    pub credentials: Vec<AgentReachOverviewCredential>,
    /// The browser profiles whose sign-ins it keeps; `null` for a caller who may not see its browsers.
    pub browser_profiles: Option<Vec<AgentReachOverviewBrowserProfile>>,
}

/// The names of the secrets an environment's commands are given.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReachOverviewSecrets {
    /// The names, never a value; `null` for a caller who does not manage the environment.
    pub names: Option<Vec<String>>,
    /// The place whose settings name those it inherits; `null` where the names are not shown.
    pub source: Option<NodeRef>,
}

/// One mount attached to an environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReachOverviewMount {
    /// The mount.
    pub mount: MountRef,
    /// Where the agent finds it.
    pub path: String,
    /// How it is attached: `read_only`, `shared` or `private`.
    pub mode: String,
}

/// Where one thread keeps its model key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReachOverviewCredential {
    /// The thread.
    pub thread: NodeRef,
    /// Its arrangement, which says where the key is and what of it the environment holds: `separated` (nothing),
    /// `key_separated` (a token that works only at Orch's forwarder), `credential_shared` (a copy), `handed_over`
    /// (the whole login), `together` (one machine, its owner's), or `unbound` before the thread first runs.
    pub placement: String,
    /// When the copy the environment holds runs out, for a copy that does; `null` otherwise.
    pub expires_at: Option<Time>,
}

/// A browser profile whose sign-ins an environment keeps; a profile is known by its name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct AgentReachOverviewBrowserProfile {
    /// Its name, which is the name of its directory.
    pub name: String,
}

crate::endpoints! {
    /// Lists kept environments under each one's restoration authority.
    NodeDeletedEnvironmentList: GET "/nodes/{id}/environments/deleted", Rule("environment manager or current home admin for each entry"), query NodeDeletedEnvironmentListQuery, none, json crate::common::Page<DeletedEnvironment>, 200;
    /// Restores kept files into a verified stopped generation.
    EnvironmentRestore: POST "/environments/{id}/restore", Rule("environment manager or current home admin"), none, json EnvironmentRestoreBody, json crate::workspace::Job, 202;
    /// Permanently removes kept files after confirmation by a current home admin.
    EnvironmentPurge: DELETE "/environments/{id}/purge", Rule("current home admin"), query EnvironmentPurgeQuery, none, json crate::workspace::Job, 202;
    /// Lists where an environment made for an org or a project could be, for the caller.
    NodePlaceList: GET "/nodes/{id}/places", Permission("node.interact"), query PlaceListQuery, none, json Vec<Place>, 200;
    /// Lists the environments the caller owns or holds a permission on, of whatever org.
    EnvironmentList: GET "/environments", User, query EnvironmentListQuery, none, json Vec<crate::common::Listed<Environment>>, 200;
    /// Makes an environment, the caller's own, in status `none` until it is first used or started.
    EnvironmentCreate: POST "/environments", Rule("what creating an environment needs in its home"), none, json NewEnvironment, json Environment, 201;
    /// Reads an environment.
    EnvironmentGet: GET "/environments/{id}", Rule("any permission on the environment, or node.view on a thread bound to it"), query crate::nodes::DeletedInspectionQuery, none, json Environment, 200;
    /// Brings a client up to date on an environment: the environment, its feed's sequence and the jobs about it.
    EnvironmentSyncGet: GET "/environments/{id}/sync", Rule("any permission on the environment, or node.view on a thread bound to it"), query EnvironmentSyncGetQuery, none, json EnvironmentSyncGetReply, 200;
    /// Lists everybody who can reach into an environment now, and how.
    EnvironmentReachGet: GET "/environments/{id}/reach", Rule("any permission on the environment"), none, none, json EnvironmentReach, 200;
    /// Says what the agents of an environment can reach, cut by the caller's own rights on its files, browsers, threads and settings.
    EnvironmentAgentReachGet: GET "/environments/{id}/agent-reach", Rule("environment.view, or node.view on a thread bound to it for the cut view"), none, none, json AgentReachOverview, 200;
    /// Changes an environment; what the running unit cannot take is stored and listed in `updates`.
    EnvironmentUpdate: PATCH "/environments/{id}", Rule("environment.manage, and for another home also node.interact on the new home"), none, json EnvironmentUpdateBody, json Environment, 200;
    /// Lists the MCP servers of an environment and those found in the harness configuration there that nobody imported.
    EnvironmentMcpGet: GET "/environments/{id}/mcp", Rule("any permission on the environment"), none, none, json EnvironmentMcp, 200;
    /// Imports an MCP server found in the harness configuration of an environment; the connector keeps a copy of its definition.
    EnvironmentMcpImportCreate: POST "/environments/{id}/mcp/imports", Rule("environment.manage; on a person's machine also being the machine's owner, or managing every environment of that machine once its owner handed the policy key for imports over"), none, json EnvironmentMcpImportCreateBody, json crate::resources::McpServer, 201;
    /// Writes an MCP server of the environment's own, switched on there at once.
    EnvironmentMcpServerCreate: POST "/environments/{id}/mcp/servers", Rule("environment.manage; on a person's machine also that the machine's local policy takes a server written from here"), none, json EnvironmentMcpServerCreateBody, json crate::resources::McpServer, 201;
    /// Restarts only the named server with its failure count cleared; the environment and turns keep running.
    EnvironmentMcpServerRestart: POST "/environments/{id}/mcp/servers/{name}/restart", Permission("environment.manage"), none, none, json crate::workspace::Job, 202;
    /// Starts a stopped environment by hand; a turn's pre-flight starts one by itself.
    EnvironmentStart: POST "/environments/{id}/start", Permission("environment.manage"), none, none, json Job, 202;
    /// Stops an environment, returning a handed-over credential first.
    EnvironmentStop: POST "/environments/{id}/stop", Permission("environment.manage"), none, json EnvironmentStopBody, json Environment, 200;
    /// Restarts the same managed unit and preserves queued inputs and every file.
    EnvironmentRestart: POST "/environments/{id}/restart", Permission("environment.manage"), none, json EnvironmentRestartBody, json Job, 202;
    /// Counts as activity now, so that the idle timeout starts over; keeping it running is managing it.
    EnvironmentKeepAwake: POST "/environments/{id}/keep-awake", Permission("environment.manage"), none, none, json Environment, 200;
    /// Plans an update of an environment's unit, or a plain recreation, before anything is done.
    EnvironmentUpdatePreflight: POST "/environments/{id}/updates/preflight", Permission("environment.manage"), none, json UpdatePreflightBody, json EnvironmentUpdatePlan, 200;
    /// Carries out a plan of an update as a job; the environment and its threads are locked at once.
    EnvironmentUpdateCreate: POST "/environments/{id}/updates", Permission("environment.manage"), query UpdateConfirmQuery, json ForkCreateBody, json Job, 202;
    /// Takes back a capability that was granted to an environment, which recreates it.
    EnvironmentCapabilityRevoke: DELETE "/environments/{id}/capabilities/{key}", Rule("ports.manage for a gateway capability; environment.manage or host.manage for a native capability"), none, none, json Option<Job>, 202;
    /// Switches an environment off.
    EnvironmentDisable: POST "/environments/{id}/disable", Permission("environment.manage"), query crate::resource_facts::ResourcePauseQuery, none, json Environment, 200;
    /// Switches an environment on again.
    EnvironmentEnable: POST "/environments/{id}/enable", Permission("environment.manage"), query crate::resource_facts::ResourceResumeQuery, none, json Environment, 200;
    /// Schedules an environment's removal, which the reaper does; of a directory on a person's machine only the registration goes.
    EnvironmentDelete: DELETE "/environments/{id}", Permission("environment.manage"), query EnvironmentDeleteQuery, none, json Environment, 202;
    /// Withdraws a scheduled removal that has not begun.
    EnvironmentKeep: POST "/environments/{id}/keep", Permission("environment.manage"), none, none, json Environment, 200;
    /// Lists permitted destination lifecycles and their actual facts before a move.
    EnvironmentMovePlaceList: GET "/environments/{id}/moves/places", Permission("environment.manage"), query PlaceListQuery, none, json Vec<EnvironmentMovePlace>, 200;
    /// Plans a move by data ownership to a permitted machine, before anything is done.
    EnvironmentMovePreflight: POST "/environments/{id}/moves/preflight", Rule("environment.manage and use of the destination machine's declared lifecycle and template"), none, json EnvironmentMovePreflightBody, json EnvironmentMovePlan, 200;
    /// Carries out a plan of a move as a job; the environment and its threads are locked at once.
    EnvironmentMoveCreate: POST "/environments/{id}/moves", Rule("environment.manage and current use of the reviewed destination machine and template"), none, json ForkCreateBody, json Job, 202;
    /// Lists the environments that threads under a node run in and those whose home is there, with the hosts the caller manages.
    NodeEnvironmentList: GET "/nodes/{id}/environments", Permission("node.view"), query NodeEnvironmentListQuery, none, json NodeEnvironmentListReply, 200;
    /// Stops the environments under a node that have been idle for long enough, or only lists them.
    NodeEnvironmentRecycle: POST "/nodes/{id}/environments/recycle", Rule("environment.manage, on each environment"), none, json RecycleBody, json RecycleReply, 200;
    /// Plans moves to a declared unit or directory lifecycle, or an available destination for each.
    NodeEnvironmentMovePreflight: POST "/nodes/{id}/environments/moves/preflight", Rule("environment.manage on each environment and use of the destination machine's declared lifecycle and template"), none, json NodeEnvironmentMovePreflightBody, json EnvironmentMoveBatchPlan, 200;
    /// Carries out a plan of several moves: one job for each plan without blockers, a few at a time.
    NodeEnvironmentMoveCreate: POST "/nodes/{id}/environments/moves", Rule("environment.manage on each environment and current use of the reviewed destination machine and template"), none, json ForkCreateBody, json EnvironmentBatch, 202;
    /// Plans an update of several environments.
    NodeEnvironmentUpdatePreflight: POST "/nodes/{id}/environments/updates/preflight", Rule("environment.manage, on each environment"), none, json NodeEnvironmentUpdatePreflightBody, json EnvironmentUpdateBatchPlan, 200;
    /// Carries out a plan of several updates: one job for each plan without blockers, each rolling back alone.
    NodeEnvironmentUpdateCreate: POST "/nodes/{id}/environments/updates", Rule("environment.manage, on each environment"), query UpdateConfirmQuery, json ForkCreateBody, json EnvironmentBatch, 202;
    /// Plans updates of visible environments across hosts and places.
    EnvironmentUpdateBatchPreflight: POST "/environments/updates/preflight", Rule("environment.manage, on each environment"), none, json NodeEnvironmentUpdatePreflightBody, json EnvironmentUpdateBatchPlan, 200;
    /// Confirms a reviewed cross-place batch, checking every member again.
    EnvironmentUpdateBatchCreate: POST "/environments/updates", Rule("environment.manage, on each environment"), query UpdateConfirmQuery, json ForkCreateBody, json EnvironmentBatch, 202;
    /// Registers a permitted machine directory as an environment whose first owner is the caller.
    MachineEnvironmentCreate: POST "/machines/{id}/environments", Rule("machine.use within its entry limits and machine policy; the home is their domain or an org inside it"), none, json MachineEnvironmentCreateBody, json Environment, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{Value, json};

    const AT: &str = "2026-10-08T17:04:05.123Z";

    fn user() -> Value {
        json!({ "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "human", "username": "alice", "display_name": "Alice", "avatar": { "text": "A", "color": 4 } })
    }

    fn node(kind: &str, id: &str, name: &str) -> Value {
        json!({ "id": id, "kind": kind, "name": name, "redacted": false })
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

    #[test]
    fn an_environment_says_what_became_of_its_mounts_and_what_its_exec_node_offers() {
        let written = merged(&[
            json!({
                "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01", "name": "Main", "owner": user(),
                "address": "acme.research.alice.main", "ports_exposed": false, "placement": "managed", "status": "running",
                "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02",
                "machine": null,
                "config_trust": [],
                "network": {
                    "mode": "restricted", "required_mode": "restricted", "network_mode": "bridge",
                    "proxy_url": "http://10.199.4.18:3128", "allowed_hosts": ["pypi.org"],
                    "networks": [],
                    "sources": [{ "scope": node("topic", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "Fix login"), "hosts": ["pypi.org", "crates.io"] }],
                    "revision": "r7",
                    "always": [{ "authority": "gateway.orch.internal", "protocol": "wss", "port": 9443, "purpose": "gateway" }],
                    "always_revision": "a1", "always_source": "orch_configuration",
                    "state": "ready", "reason": null,
                    "last_denied": { "host": "evil.example", "at": AT, "reason": "not an allowed host" }
                },
                "home": node("topic", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "Fix login"),
                "default_for": [node("topic", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "Fix login")],
                "threads": [{
                    "thread": node("thread", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a04", "Try it"),
                    "topic": node("topic", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a03", "Fix login"),
                    "directory": "web", "owner": user()
                }],
                "threads_hidden": 1, "reach": 3
            }),
            json!({
                "workspace_root": "/work", "workspace_storage": "fast",
                "config": { "env": { "idle_stop_minutes": 60 } },
                "host": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a05", "name": "build-1", "address": "build-1" },
                "mounts": [{
                    "mount": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a06", "name": "datasets", "address": "acme.datasets" },
                    "state": "attached", "reason": null, "path": "/mnt/datasets",
                    "mode": "read_only", "asked": "private", "enforced": false, "from": null
                }],
                "exec": {
                    "offers": [{ "name": "docker", "available": true, "reason": null }, { "name": "orch", "available": false, "reason": "no executor on this host" }],
                    "orch_tools": true, "machine_offers": null
                },
                "mcp_found": [{ "name": "github", "source": "codex", "scope": "user", "directory": null, "transport": "stdio", "imported": null }],
                "index": "full", "index_reason": null
            }),
            json!({
                "template": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a07", "name": "Ubuntu", "driver": "docker" },
                "generation": 2,
                "updates": [{ "what": "image", "name": "ubuntu", "from": "24.04.1", "to": "24.04.2" }],
                "update_required": false,
                "remote_write": true, "terminal": true, "browser": true, "vnc": false,
                "support": {
                    "looked_at": AT,
                    "browser": {
                        "programs": [{ "name": "chromium", "path": "/opt/ms-playwright/chromium-1247/chrome-linux64/chrome", "version": "155.0.8059.12" }],
                        "window": true, "playwright_cli": "0.1.22", "sandbox": false
                    },
                    "vnc": { "servers": [{ "name": "Xvnc", "path": "/usr/bin/Xvnc", "version": null }], "window_manager": "openbox" }
                },
                "granted_capabilities": ["ptrace"],
                "status_detail": null, "facts": { "os": "linux", "cpus": 8 },
                "manifest_at": AT, "last_activity_at": AT, "idle_stop_at": null, "unused_since": null,
                "removal": null, "remove_at": null, "remove_reason": null, "job": null,
                "seq": 42, "revision": 5,
                "access": { "permissions": ["environment.manage"], "administers": false, "through": [{ "kind": "owner" }] }
            }),
        ]);
        let mut environment: Environment = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(environment.base.status, EnvironmentStatus::Running);
        assert_eq!(
            environment.mounts[0].asked,
            crate::mounts::MountMode::Private
        );
        assert_eq!(
            environment.mounts[0].mode,
            crate::mounts::MountMode::ReadOnly,
            "a private view that was not possible fell back to read-only"
        );
        assert!(!environment.exec.offers[1].available);
        assert_eq!(environment.updates[0].what, UpdateWhat::Image);
        assert!(environment.browser && !environment.vnc);
        assert_eq!(environment.support.browser.sandbox, Some(false));
        assert_eq!(
            environment.support.vnc.window_manager.as_deref(),
            Some("openbox")
        );
        assert_eq!(serde_json::to_value(&environment).unwrap(), written);
        assert!(!environment.can_declare_lost);
        environment.can_declare_lost = true;
        let mut unavailable = written.clone();
        unavailable["can_declare_lost"] = json!(true);
        assert_eq!(serde_json::to_value(&environment).unwrap(), unavailable);
        assert!(
            serde_json::from_value::<Environment>(unavailable)
                .unwrap()
                .can_declare_lost
        );
        environment.can_declare_lost = false;
        assert!(
            environment.workspace_incarnation.is_none(),
            "an older producer can omit opaque source metadata"
        );
        let token = format!("wsi1_{}", "0".repeat(64));
        environment.workspace_incarnation = Some(token.clone());
        let mut current = written;
        current["workspace_incarnation"] = json!(token);
        assert_eq!(serde_json::to_value(&environment).unwrap(), current);
        let read: Environment = serde_json::from_value(current).unwrap();
        assert_eq!(
            read.workspace_incarnation,
            environment.workspace_incarnation
        );
    }

    #[test]
    fn an_environment_where_nothing_was_found_says_so_with_empty_members() {
        let nothing = EnvironmentSupport::nothing_found();
        assert_eq!(
            serde_json::to_value(&nothing).unwrap(),
            json!({
                "looked_at": null,
                "browser": { "programs": [], "window": false, "playwright_cli": null, "sandbox": null },
                "vnc": { "servers": [], "window_manager": null }
            })
        );
        let read: EnvironmentSupport = serde_json::from_value(json!({
            "looked_at": AT,
            "browser": { "programs": [], "window": false, "playwright_cli": null, "sandbox": null },
            "vnc": { "servers": [], "window_manager": null }
        }))
        .unwrap();
        assert!(read.looked_at.is_some());
        assert_eq!(read.browser, nothing.browser);
    }

    #[test]
    fn a_person_reaches_into_an_environment_through_its_page_or_through_an_agent_there() {
        let reach: EnvironmentReach = serde_json::from_value(json!({
            "people": [{
                "user": user(),
                "through": [
                    { "kind": "owner" },
                    { "kind": "page", "permission": "environment.manage" },
                    { "kind": "page", "permission": "files.view" },
                    { "kind": "page", "permission": "browser.manage" },
                    { "kind": "thread", "thread": node("thread", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a04", "Try it"), "permission": "thread.interact" }
                ]
            }],
            "hidden": 2
        }))
        .unwrap();
        assert_eq!(reach.people[0].through.len(), 5);
        assert!(matches!(
            &reach.people[0].through[3],
            ReachThrough::Page {
                permission: ReachPagePermission::BrowserManage
            }
        ));
        assert!(matches!(
            &reach.people[0].through[4],
            ReachThrough::Thread {
                permission: ReachThreadPermission::ThreadInteract,
                ..
            }
        ));
        for permission in [
            "environment.manage",
            "files.manage",
            "files.view",
            "terminal.manage",
            "terminal.view",
            "browser.manage",
            "browser.view",
            "vnc.manage",
            "vnc.view",
        ] {
            assert!(
                serde_json::from_value::<ReachThrough>(
                    json!({ "kind": "page", "permission": permission })
                )
                .is_ok(),
                "{permission}: the page's own Manage and the eight rows of its doors let a person in"
            );
        }
        assert!(
            serde_json::from_value::<ReachThrough>(
                json!({ "kind": "page", "permission": "thread.manage" })
            )
            .is_err(),
            "nothing of a thread's page lets a person in by the page"
        );
        for permission in ["files.view", "terminal.manage", "thread.manage"] {
            assert!(
                serde_json::from_value::<ReachThrough>(json!({
                    "kind": "thread",
                    "thread": node("thread", "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a04", "Try it"),
                    "permission": permission
                }))
                .is_err(),
                "{permission}: a thread lets a person in by talking to its agent and by nothing else"
            );
        }
    }

    #[test]
    fn an_mcp_server_is_switched_on_by_a_place_by_the_template_or_by_the_environment_itself() {
        let by_place: Option<McpFrom> = serde_json::from_value(node(
            "project",
            "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a08",
            "Web",
        ))
        .unwrap();
        assert!(matches!(by_place, Some(McpFrom::Node(_))));
        let by_template: Option<McpFrom> = serde_json::from_value(json!("template")).unwrap();
        assert_eq!(by_template, Some(McpFrom::Template(TemplateMark::Template)));
        let by_itself: Option<McpFrom> = serde_json::from_value(Value::Null).unwrap();
        assert_eq!(by_itself, None);
    }

    #[test]
    fn a_recycle_says_how_idle_and_whether_to_only_look() {
        let body: RecycleBody =
            serde_json::from_value(json!({ "idle_minutes": 0, "dry_run": true })).unwrap();
        assert_eq!(body.include_with_processes, None);
        assert!(
            serde_json::from_value::<RecycleBody>(json!({ "idle_minutes": 60 })).is_err(),
            "whether anything is stopped is never left to a default"
        );
    }

    #[test]
    fn a_change_to_an_environment_names_the_revision_it_was_made_on() {
        let body: EnvironmentUpdateBody = serde_json::from_value(
            json!({ "remote_write": false, "home_node_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a08", "expected_revision": 5 }),
        )
        .unwrap();
        assert_eq!(body.remote_write, Some(false));
        assert_eq!(body.config, None);
        assert_eq!((body.browser, body.vnc), (None, None));
        let switches: EnvironmentUpdateBody = serde_json::from_value(
            json!({ "browser": true, "vnc": false, "expected_revision": 6 }),
        )
        .unwrap();
        assert_eq!((switches.browser, switches.vnc), (Some(true), Some(false)));
        assert!(
            serde_json::from_value::<EnvironmentUpdateBody>(json!({ "terminal": true })).is_err()
        );
    }

    #[test]
    fn an_imported_mcp_server_may_take_another_name_and_be_switched_on_in_the_same_step() {
        let body: EnvironmentMcpImportCreateBody = serde_json::from_value(json!({
            "source": "claude-code", "scope": "project", "name": "db", "directory": "services/api",
            "as": "api-db", "enable": true
        }))
        .unwrap();
        assert_eq!(body.as_name.as_deref(), Some("api-db"));
        assert_eq!(body.scope, McpScope::Project);
        assert_eq!(
            serde_json::to_value(&body).unwrap()["as"],
            json!("api-db"),
            "the new name is written under the word `as`"
        );
    }
}
