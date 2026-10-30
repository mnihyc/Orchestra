//! The limits a person's machine keeps to itself: keys, values and what was handed over.
//!
//! A machine that a person connected obeys the system only inside limits
//! its owner set on the machine. They come before every permission, for
//! whoever asks through the system, and the system can never widen them.
//! The machine reports them with its facts. Its owner may hand single
//! keys over, or all of them: those can then be changed from the system,
//! and the machine still checks, when such a change arrives, that the key
//! is one it handed over.
//!
//! Hosts and personal machines use the same keys and delegation. Unit connectors
//! inherit their host's ceiling rather than owning another machine policy.

use crate::commands::{ArrangementType, ErrorCode, SandboxMode};
use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub mod drivers;
pub use drivers::{ImageRules, MachineDrivers, ProcessRules};
pub use orch_api_contract::resources::{
    MachinePolicyDefinition as PolicyDefinition, MachinePolicySection as PolicySection,
};

/// The entry of [`PolicyKey::PolicyDelegated`] that hands every key over.
pub const ALL_KEYS: &str = "*";

/// Sparse explicit values; omission retains the delegation-dependent installed default.
pub type PolicyValues = BTreeMap<String, Json>;

/// Effective environment doors after its own narrowing and the physical machine's ceiling.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentSwitches {
    /// File writes made through Orch.
    pub remote_write: bool,
    /// Interactive shells.
    pub terminal: bool,
    /// Browser operations.
    pub browser: bool,
    /// Display operations.
    pub vnc: bool,
}

/// Which installed defaults a machine starts from; these never decide a command's permission.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PolicyDefaults {
    /// Values used for keys handed to Orch.
    Managed,
    /// Values used for keys kept on the machine.
    Cautious,
}

/// What a key bounds, which decides who may change it from the system
/// once its owner handed it over.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Bounds {
    /// The environments registered from the machine: changed by whoever
    /// manages every one of them.
    Environments,
    /// The providers registered from the machine: changed by whoever
    /// manages every one of them.
    Providers,
    /// The machine itself: changed by its owner.
    Machine,
}

/// An absolute path written out with `.` and `..` resolved by name, or
/// `None` when it is not absolute or climbs above its own root.
fn lexical(path: &std::path::Path) -> Option<std::path::PathBuf> {
    use std::path::Component;
    if !path.is_absolute() {
        return None;
    }
    let mut resolved = std::path::PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                resolved.push(component)
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !resolved.pop() {
                    return None;
                }
            }
        }
    }
    Some(resolved)
}

macro_rules! policy_keys {
    ($( $(#[doc = $doc:literal])* $variant:ident = $key:literal, $bounds:ident; )*) => {
        /// A key of a machine's local policy.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
        pub enum PolicyKey {
            $( $(#[doc = $doc])* #[serde(rename = $key)] $variant, )*
        }

        impl PolicyKey {
            /// Every key, in the order a machine lists them.
            pub const ALL: &'static [PolicyKey] = &[ $( PolicyKey::$variant, )* ];

            /// The key as it is written.
            #[must_use]
            pub const fn as_str(self) -> &'static str {
                match self { $( Self::$variant => $key, )* }
            }

            /// What the key bounds.
            #[must_use]
            pub const fn bounds(self) -> Bounds {
                match self { $( Self::$variant => Bounds::$bounds, )* }
            }

            /// The meaning shared by local commands and policy readers.
            #[must_use]
            pub const fn meaning(self) -> &'static str {
                match self { $( Self::$variant => concat!($($doc, " ",)*), )* }
            }
        }

        impl std::str::FromStr for PolicyKey {
            type Err = PolicyError;

            fn from_str(text: &str) -> Result<Self, Self::Err> {
                match text {
                    $( $key => Ok(Self::$variant), )*
                    _ => Err(PolicyError::UnknownKey(text.to_owned())),
                }
            }
        }
    };
}

policy_keys! {
    /// Drivers this node offers, their image/capability/runtime ceilings and allowed host accounts and programs.
    Drivers = "drivers", Machine;
    /// Named directories where this machine may make persistent data.
    Storages = "storages", Machine;
    /// The largest resource declaration accepted for any unit on this machine.
    ResourceMax = "resources.max", Machine;
    /// Operator programs run around unit start/stop and directory attach/detach.
    LifecycleHooks = "hooks", Machine;
    /// The directories inside which an environment of the machine may
    /// have its root, and a mount its location.
    EnvironmentRoots = "environment.roots", Machine;
    /// Whether files written through the system are accepted.
    EnvironmentRemoteWrite = "environment.remote_write", Environments;
    /// Whether shells may be opened on the machine through the system.
    EnvironmentTerminal = "environment.terminal", Environments;
    /// Whether configured commands run after a unit starts or a directory attaches.
    EnvironmentAfterUp = "environment.after_up", Environments;
    /// Whether browsers may be started, shown and driven on the machine.
    EnvironmentBrowser = "environment.browser", Environments;
    /// Permits virtual displays and announced VNC servers, including a served local desktop.
    EnvironmentVnc = "environment.vnc", Environments;
    /// The opening audiences and reachable host declaration this machine permits.
    Ports = "ports", Machine;
    /// Whether the tool servers its harnesses have configured may be
    /// listed for the system and imported.
    McpImport = "mcp.import", Environments;
    /// Whether a tool server may be written from the system for an
    /// environment of the machine.
    McpAdd = "mcp.add", Environments;
    /// What the machine's exec node may offer a harness that runs
    /// elsewhere.
    ExecOffers = "exec.offers", Environments;
    /// Whether files that arrive with a message to an agent are written
    /// into the workspace.
    EnvironmentDeliverFiles = "environment.deliver_files", Environments;
    /// The widest sandbox mode the system may ask of a harness there.
    AgentMaxSandbox = "agent.max_sandbox", Environments;
    /// Whether Orch may sign in or renew a provider in a harness home the person owns.
    ProviderSignIn = "provider.signin", Machine;
    /// The addresses the machine may forward model calls to.
    ProviderUpstreams = "provider.upstreams", Providers;
    /// The ways of placing a harness in which a credential kept on the
    /// machine may be used.
    ProviderArrangements = "provider.arrangements", Providers;
    /// The harness kinds the machine may run, each with where its
    /// program is.
    AgentHarnesses = "agent.harnesses", Machine;
    /// Let machine managers register and remove agent adapter programs at run time.
    AgentPrograms = "agent.programs", Machine;
    /// Let machine managers register and remove provider source programs at run time.
    ProviderPrograms = "provider.programs", Machine;
    /// Let machine managers change the roles this connector offers; node roles belong to orch-node.
    MachineRoles = "machine.roles", Machine;
    /// Whether the connector may download the harness and executor
    /// builds the system names.
    ToolsFetch = "tools.fetch", Machine;
    /// Whether the connector updates itself.
    UpdateAuto = "update.auto", Machine;
    /// The keys that may be changed from the system. This one never may.
    PolicyDelegated = "policy.delegated", Machine;
}

impl std::fmt::Display for PolicyKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod role_policy_tests {
    use super::*;

    #[test]
    fn environment_handover_does_not_hand_over_role_editing_and_local_values_always_win()
    -> Result<(), PolicyError> {
        let empty = PolicyValues::new();
        let environment = LocalPolicy::resolve(&["environment.*".into()], &empty, &empty)?;
        assert!(!environment.machine_roles);
        assert!(!environment.delegates(PolicyKey::MachineRoles));
        let handed = LocalPolicy::resolve(&["machine.roles".into()], &empty, &empty)?;
        assert!(handed.machine_roles);
        assert!(handed.delegates(PolicyKey::MachineRoles));
        let local = PolicyValues::from([("machine.roles".into(), Json::Bool(false))]);
        let kept = LocalPolicy::resolve(&[ALL_KEYS.into()], &local, &empty)?;
        assert!(!kept.machine_roles);
        assert!(!kept.delegates(PolicyKey::MachineRoles));
        assert_eq!(
            PolicyKey::MachineRoles.sections(),
            &[PolicySection::Machine]
        );
        Ok(())
    }
}

/// Why a key or a value is not accepted.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PolicyError {
    /// No such key.
    #[error("`{0}` is not a key of the local policy")]
    UnknownKey(String),
    /// The value does not fit the key.
    #[error("{key} must be {expected}")]
    Value {
        /// The key.
        key: PolicyKey,
        /// What its value has to be.
        expected: &'static str,
    },
}

/// One harness kind a machine may run.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct PolicyHarness {
    /// The harness kind.
    pub kind: String,
    /// Where its program is on the machine, or the daemon to attach to.
    pub path: String,
}

impl PolicyKey {
    /// A key, a role section or the whole machine; the handover list itself is always local.
    #[must_use]
    pub fn delegation_name(name: &str) -> bool {
        name == ALL_KEYS
            || PolicySection::ALL
                .iter()
                .any(|section| *section != PolicySection::Machine && name == section.selector())
            || name
                .parse::<Self>()
                .is_ok_and(|key| key != Self::PolicyDelegated)
    }
    /// The roles a key bounds; the same roots protect directory and unit attachments.
    #[must_use]
    pub const fn sections(self) -> &'static [PolicySection] {
        use PolicySection as S;
        match self {
            Self::EnvironmentRoots => &[S::Environment, S::Node],
            Self::EnvironmentRemoteWrite
            | Self::EnvironmentTerminal
            | Self::EnvironmentAfterUp
            | Self::EnvironmentBrowser
            | Self::EnvironmentVnc
            | Self::EnvironmentDeliverFiles
            | Self::McpImport
            | Self::McpAdd
            | Self::ExecOffers
            | Self::AgentMaxSandbox
            | Self::Ports => &[S::Environment],
            Self::Drivers | Self::LifecycleHooks | Self::Storages | Self::ResourceMax => &[S::Node],
            Self::AgentHarnesses | Self::AgentPrograms | Self::ToolsFetch => &[S::Agent],
            Self::ProviderSignIn
            | Self::ProviderPrograms
            | Self::ProviderUpstreams
            | Self::ProviderArrangements => &[S::Provider],
            Self::MachineRoles | Self::UpdateAuto | Self::PolicyDelegated => &[S::Machine],
        }
    }

    /// Whether a role section or this exact key has been handed to Orch.
    #[must_use]
    pub fn handed_by(self, names: &[String]) -> bool {
        self != Self::PolicyDelegated
            && names.iter().any(|name| {
                name == ALL_KEYS
                    || name == self.as_str()
                    || self.sections().iter().any(|section| {
                        *section != PolicySection::Machine && name == section.selector()
                    })
            })
    }

    /// The sections this physical machine offers, including a node's ceiling for its units.
    #[must_use]
    pub fn applies_to_roles(self, roles: &[crate::session::Role]) -> bool {
        use crate::session::Role;
        // Agent adapters keep native accounts; Agent and Provider roles both host provider directories.
        // These services consume the existing ceilings even without a general Environment role.
        if matches!(
            self,
            Self::ProviderSignIn | Self::ProviderPrograms | Self::ProviderArrangements
        ) && roles.contains(&Role::Agent)
            || self == Self::AgentMaxSandbox && roles.contains(&Role::Agent)
            || self == Self::AgentHarnesses && roles.contains(&Role::Environment)
            || self == Self::ToolsFetch && roles.contains(&Role::Node)
            || matches!(self, Self::EnvironmentRoots | Self::EnvironmentAfterUp)
                && roles
                    .iter()
                    .any(|role| matches!(role, Role::Agent | Role::Provider))
        {
            return true;
        }
        self.sections().iter().any(|section| match section {
            PolicySection::Environment => {
                roles.contains(&Role::Environment) || roles.contains(&Role::Node)
            }
            PolicySection::Node => roles.contains(&Role::Node),
            PolicySection::Agent => roles.contains(&Role::Agent),
            PolicySection::Provider => roles.contains(&Role::Provider),
            PolicySection::Machine => true,
        })
    }

    /// The short name of the setting, also used by local commands.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::MachineRoles => "Offered roles",
            Self::AgentPrograms => "Agent adapter registration",
            Self::ProviderPrograms => "Provider source registration",
            Self::Drivers => "Drivers and template limits",
            Self::Storages => "Storage",
            Self::ResourceMax => "Resource limits",
            Self::EnvironmentRoots => "Directories Orch may expose",
            Self::EnvironmentRemoteWrite => "File writes",
            Self::EnvironmentTerminal => "Shells",
            Self::EnvironmentAfterUp => "Commands after start",
            Self::EnvironmentBrowser => "Browsers",
            Self::EnvironmentVnc => "Displays",
            Self::EnvironmentDeliverFiles => "Files delivered with messages",
            Self::McpImport => "Import tool servers",
            Self::McpAdd => "Add tool servers",
            Self::ExecOffers => "Tools offered to agents",
            Self::AgentMaxSandbox => "Widest sandbox",
            Self::Ports => "Port audiences and listeners",
            Self::LifecycleHooks => "Lifecycle programs",
            Self::AgentHarnesses => "Harness programs",
            Self::ToolsFetch => "Downloading builds",
            Self::ProviderSignIn => "Provider sign-in",
            Self::ProviderUpstreams => "Model service addresses",
            Self::ProviderArrangements => "Ways credentials may be used",
            Self::UpdateAuto => "Automatic updates",
            Self::PolicyDelegated => "Settings handed to Orch",
        }
    }
    /// Definition data for all machine kinds; permissions never follow a defaults profile.
    #[must_use]
    pub fn definition(self) -> PolicyDefinition {
        PolicyDefinition {
            key: self.as_str().into(),
            title: self.title().into(),
            meaning: self.meaning().trim().into(),
            sections: self.sections().to_vec(),
            cautious: LocalPolicy::installed(PolicyDefaults::Cautious).get(self),
            managed: LocalPolicy::installed(PolicyDefaults::Managed).get(self),
            delegable: self != Self::PolicyDelegated,
        }
    }
}

/// The same operator-owned lifecycle programs on every machine.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MachineStorage {
    /// The name selected by environments and runtimes.
    pub name: String,
    /// Its absolute physical path, bounded by the one roots policy.
    pub path: String,
    /// Whether it is selected when no name is supplied.
    pub default: bool,
}

/// Resource ceilings of units created by this machine's node role.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MachineResources {
    /// No omitted resource imposes a numeric ceiling.
    pub max: orch_driver_contract::runtime::Resources,
}

/// The same operator-owned lifecycle programs on every machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MachineHooks {
    /// An absolute executable run before start or attach.
    pub pre_up: Option<String>,
    /// An absolute executable run after start or attach.
    pub post_up: Option<String>,
    /// An absolute executable run before stop or detach.
    pub pre_down: Option<String>,
    /// An absolute executable run after stop or detach.
    pub post_down: Option<String>,
    /// The bound on a hook's whole process group, in seconds.
    pub timeout_secs: u64,
}

/// One machine-wide port ceiling and its independently reviewed public listener declaration.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MachinePorts {
    /// Which audiences can ever be admitted; templates may only narrow these.
    pub allowed: Vec<orch_api_contract::ports::PortOpeningKind>,
    /// No reachable address means publishing on the machine is unavailable.
    pub publish: Option<orch_api_contract::ports::HostPorts>,
    /// Optional local listener constraints imported from an operator configuration.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<orch_api_contract::ports::HostPortsLimits>,
}

impl MachinePorts {
    /// A stable 48-bit review revision, exactly representable by browser numbers on every machine kind.
    #[must_use]
    pub fn revision(&self) -> i64 {
        let hash = orch_types::Hash::of(&serde_json::to_vec(self).unwrap_or_default());
        hash.as_bytes()[..6]
            .iter()
            .fold(0i64, |revision, byte| (revision << 8) | i64::from(*byte))
    }
    /// A declared listener has a real address, local listen address and nonzero ordered ranges.
    #[must_use]
    pub fn valid(&self) -> bool {
        let listen =
            |address: &str| address == "all" || address.parse::<std::net::IpAddr>().is_ok();
        let ranges = |ranges: &[orch_api_contract::ports::PortRange]| {
            ranges
                .iter()
                .all(|range| range.from > 0 && range.from <= range.to)
        };
        self.limits.as_ref().is_none_or(|limits| {
            limits
                .incoming_address
                .as_ref()
                .is_none_or(|address| !address.trim().is_empty())
                && limits.listen.as_deref().is_none_or(listen)
                && limits.ranges.as_deref().is_none_or(ranges)
        }) && self.publish.as_ref().is_none_or(|publish| {
            !publish.incoming_address.trim().is_empty()
                && !publish.ranges.is_empty()
                && ranges(&publish.ranges)
                && listen(&publish.listen)
                && self.limits.as_ref().is_none_or(|limits| {
                    limits
                        .incoming_address
                        .as_ref()
                        .is_none_or(|fixed| fixed == &publish.incoming_address)
                        && limits
                            .listen
                            .as_deref()
                            .is_none_or(|fixed| fixed == "all" || fixed == publish.listen)
                        && limits.ranges.as_ref().is_none_or(|allowed| {
                            publish.ranges.iter().all(|range| {
                                allowed
                                    .iter()
                                    .any(|outer| outer.from <= range.from && range.to <= outer.to)
                            })
                        })
                })
        })
    }
    /// Whether an audience is allowed, including a reachable declaration for a host listener.
    #[must_use]
    pub fn permits(&self, kind: orch_api_contract::ports::PortOpeningKind) -> bool {
        self.allowed.contains(&kind)
            && (kind != orch_api_contract::ports::PortOpeningKind::Host
                || self
                    .publish
                    .as_ref()
                    .is_some_and(|d| !d.incoming_address.trim().is_empty() && !d.ranges.is_empty()))
    }
}

impl Default for MachineHooks {
    fn default() -> Self {
        // A short operator helper may need to update routes or contact a monitor.
        const HOOK_TIMEOUT_SECS: u64 = 30;
        Self {
            pre_up: None,
            post_up: None,
            pre_down: None,
            post_down: None,
            timeout_secs: HOOK_TIMEOUT_SECS,
        }
    }
}

impl MachineHooks {
    /// Hook programs are absolute paths and have a nonzero time limit.
    #[must_use]
    pub fn valid(&self) -> bool {
        self.timeout_secs > 0
            && [&self.pre_up, &self.post_up, &self.pre_down, &self.post_down]
                .into_iter()
                .flatten()
                .all(|path| std::path::Path::new(path).is_absolute())
    }
}

/// A machine's local policy: every key with its value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct LocalPolicy {
    /// The node's driver offers and template ceilings.
    pub drivers: MachineDrivers,
    /// Named storage locations declared by the node role.
    #[serde(default)]
    pub storages: Vec<MachineStorage>,
    /// The largest unit resource declaration admitted on this machine.
    #[serde(default, rename = "resources.max")]
    pub resource_max: orch_driver_contract::runtime::Resources,
    /// Offered roles; absent only for older reports that contained every section.
    #[serde(
        default,
        rename = "policy.roles",
        skip_serializing_if = "Option::is_none"
    )]
    pub roles: Option<Vec<crate::session::Role>>,
    /// Explicit constraints kept locally; these keys cannot be changed by Orch even under Full Access.
    #[serde(
        default,
        rename = "policy.local",
        skip_serializing_if = "BTreeMap::is_empty"
    )]
    pub local_values: PolicyValues,
    /// See [`PolicyKey::LifecycleHooks`].
    #[serde(default)]
    pub hooks: MachineHooks,
    /// See [`PolicyKey::EnvironmentRoots`].
    #[serde(rename = "environment.roots")]
    pub environment_roots: Vec<String>,
    /// See [`PolicyKey::EnvironmentRemoteWrite`].
    #[serde(rename = "environment.remote_write")]
    pub environment_remote_write: bool,
    /// See [`PolicyKey::EnvironmentTerminal`].
    #[serde(rename = "environment.terminal")]
    pub environment_terminal: bool,
    /// See [`PolicyKey::EnvironmentAfterUp`].
    #[serde(default, rename = "environment.after_up")]
    pub environment_after_up: bool,
    /// See [`PolicyKey::EnvironmentBrowser`].
    #[serde(default, rename = "environment.browser")]
    pub environment_browser: bool,
    /// See [`PolicyKey::EnvironmentVnc`]; installed connectors deny it.
    #[serde(default, rename = "environment.vnc")]
    pub environment_vnc: bool,
    /// See [`PolicyKey::Ports`]; installed personal machines allow no audiences.
    #[serde(default)]
    pub ports: MachinePorts,
    /// See [`PolicyKey::McpImport`].
    #[serde(rename = "mcp.import")]
    pub mcp_import: bool,
    /// See [`PolicyKey::McpAdd`].
    #[serde(rename = "mcp.add")]
    pub mcp_add: bool,
    /// See [`PolicyKey::ExecOffers`].
    #[serde(rename = "exec.offers")]
    pub exec_offers: Vec<String>,
    /// See [`PolicyKey::EnvironmentDeliverFiles`].
    #[serde(rename = "environment.deliver_files")]
    pub environment_deliver_files: bool,
    /// See [`PolicyKey::AgentMaxSandbox`].
    #[serde(rename = "agent.max_sandbox")]
    pub agent_max_sandbox: SandboxMode,
    /// See [`PolicyKey::ProviderSignIn`]; connector-owned homes do not need this permission.
    #[serde(default, rename = "provider.signin")]
    pub provider_signin: bool,
    /// See [`PolicyKey::ProviderUpstreams`].
    #[serde(rename = "provider.upstreams")]
    pub provider_upstreams: Vec<String>,
    /// See [`PolicyKey::ProviderArrangements`].
    #[serde(rename = "provider.arrangements")]
    pub provider_arrangements: Vec<ArrangementType>,
    /// See [`PolicyKey::AgentHarnesses`].
    #[serde(rename = "agent.harnesses")]
    pub agent_harnesses: Vec<PolicyHarness>,
    /// See [`PolicyKey::AgentPrograms`].
    #[serde(default, rename = "agent.programs")]
    pub agent_programs: bool,
    /// See [`PolicyKey::ProviderPrograms`].
    #[serde(default, rename = "provider.programs")]
    pub provider_programs: bool,
    /// See [`PolicyKey::MachineRoles`]; changing roles still requires explicit delegation.
    #[serde(default, rename = "machine.roles")]
    pub machine_roles: bool,
    /// See [`PolicyKey::ToolsFetch`].
    #[serde(rename = "tools.fetch")]
    pub tools_fetch: bool,
    /// See [`PolicyKey::UpdateAuto`].
    #[serde(rename = "update.auto")]
    pub update_auto: bool,
    /// See [`PolicyKey::PolicyDelegated`].
    #[serde(rename = "policy.delegated")]
    pub policy_delegated: Vec<String>,
}

impl Default for LocalPolicy {
    /// The policy of a connector as it is installed: nothing is handed
    /// over, nothing is written or opened from outside, and nothing of a
    /// credential leaves the machine.
    fn default() -> Self {
        Self {
            drivers: MachineDrivers::default(),
            storages: Vec::new(),
            resource_max: orch_driver_contract::runtime::Resources::default(),
            roles: None,
            local_values: PolicyValues::new(),
            hooks: MachineHooks::default(),
            environment_roots: Vec::new(),
            environment_remote_write: false,
            environment_terminal: false,
            environment_after_up: false,
            environment_browser: false,
            environment_vnc: false,
            ports: MachinePorts::default(),
            mcp_import: true,
            mcp_add: false,
            exec_offers: orch_agent_programs::Programs::builtin().every_offer(),
            environment_deliver_files: true,
            agent_max_sandbox: SandboxMode::Workspace,
            provider_signin: false,
            provider_upstreams: Vec::new(),
            provider_arrangements: vec![ArrangementType::Separated, ArrangementType::KeySeparated],
            agent_harnesses: Vec::new(),
            agent_programs: false,
            provider_programs: false,
            machine_roles: false,
            tools_fetch: false,
            update_auto: true,
            policy_delegated: Vec::new(),
        }
    }
}

/// What a change that arrived from the system did to a policy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyChange {
    /// The keys that were changed.
    pub applied: Vec<String>,
    /// The keys that were left as they were: unknown, not handed over, or
    /// given a value that does not fit.
    pub refused: Vec<String>,
}

impl LocalPolicy {
    /// A machine reports only the sections it actually offers; a node includes its units' ceiling.
    #[must_use]
    pub fn reported(&self) -> Json {
        let mut values = serde_json::Map::new();
        for key in PolicyKey::ALL {
            if self
                .roles
                .as_ref()
                .is_none_or(|roles| key.applies_to_roles(roles))
            {
                values.insert(key.as_str().into(), self.get(*key));
            }
        }
        if let Some(roles) = &self.roles {
            values.insert("policy.roles".into(), serde_json::json!(roles));
        }
        let local: PolicyValues = self
            .local_values
            .iter()
            .filter(|(name, _)| {
                name.parse::<PolicyKey>().is_ok_and(|key| {
                    self.roles
                        .as_ref()
                        .is_none_or(|roles| key.applies_to_roles(roles))
                })
            })
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        if !local.is_empty() {
            values.insert("policy.local".into(), serde_json::json!(local));
        }
        Json::Object(values)
    }
    /// Unset environment switches follow this machine; an explicit false can only narrow it.
    #[must_use]
    pub fn environment_switches(
        &self,
        remote_write: Option<bool>,
        terminal: Option<bool>,
        browser: Option<bool>,
        vnc: Option<bool>,
    ) -> EnvironmentSwitches {
        EnvironmentSwitches {
            remote_write: self.environment_remote_write && remote_write.unwrap_or(true),
            terminal: self.environment_terminal && terminal.unwrap_or(true),
            browser: self.environment_browser && browser.unwrap_or(true),
            vnc: self.environment_vnc && vnc.unwrap_or(true),
        }
    }
    /// The environment audience ceiling applies to directories and to every unit of a node.
    #[must_use]
    pub fn permits_port(
        &self,
        audience: Option<orch_api_contract::ports::PortOpeningKind>,
    ) -> bool {
        audience.map_or_else(
            || !self.ports.allowed.is_empty(),
            |kind| self.ports.permits(kind),
        )
    }
    /// Fill omitted resource declarations from this node's ceiling, and reject requests above it.
    pub fn bounded_resources(
        &self,
        mut requested: orch_driver_contract::runtime::Resources,
    ) -> Result<orch_driver_contract::runtime::Resources, String> {
        if self
            .roles
            .as_ref()
            .is_some_and(|roles| !PolicyKey::ResourceMax.applies_to_roles(roles))
        {
            return Ok(requested);
        }
        macro_rules! bound {($($field:ident),*)=>{$(if let Some(maximum)=self.resource_max.$field {
            if requested.$field.is_some_and(|value|value<=0 || value>maximum) {return Err(format!("resources.max (node role): {} exceeds the machine operator's ceiling of {maximum}",stringify!($field)));}
            requested.$field=Some(requested.$field.unwrap_or(maximum));
        })*};}
        bound!(
            cpu_millis,
            memory_mb,
            memory_swap_mb,
            blkio_weight,
            pids,
            disk_gb
        );
        Ok(requested)
    }

    /// Preserve an absent declaration unless an active ceiling actually supplies a value.
    pub fn bounded_resources_optional(
        &self,
        requested: Option<orch_driver_contract::runtime::Resources>,
    ) -> Result<Option<orch_driver_contract::runtime::Resources>, String> {
        let bounded = self.bounded_resources(requested.unwrap_or_default())?;
        Ok((requested.is_some() || bounded != Default::default()).then_some(bounded))
    }

    /// Installed values for every key, before explicit machine configuration.
    #[must_use]
    pub fn installed(kind: PolicyDefaults) -> Self {
        let mut policy = Self::default();
        // Hosts preserve the facilities that their units historically offered. Every command still
        // goes through these same fields; selecting defaults never bypasses a ceiling.
        const MANAGED_SWITCHES: &[(PolicyKey, bool)] = &[
            (PolicyKey::EnvironmentRemoteWrite, true),
            (PolicyKey::EnvironmentTerminal, true),
            (PolicyKey::EnvironmentAfterUp, true),
            (PolicyKey::ProviderSignIn, true),
            (PolicyKey::EnvironmentBrowser, true),
            (PolicyKey::EnvironmentVnc, true),
            (PolicyKey::McpImport, true),
            (PolicyKey::McpAdd, true),
            (PolicyKey::EnvironmentDeliverFiles, true),
            (PolicyKey::UpdateAuto, true),
            (PolicyKey::ToolsFetch, true),
            (PolicyKey::AgentPrograms, true),
            (PolicyKey::ProviderPrograms, true),
            (PolicyKey::MachineRoles, true),
        ];
        if kind == PolicyDefaults::Managed {
            policy.drivers = MachineDrivers::managed();
            for (key, value) in MANAGED_SWITCHES {
                let _ = policy.set(*key, &Json::Bool(*value));
            }
            policy.agent_max_sandbox = SandboxMode::Open;
            policy.ports.allowed = vec![
                orch_api_contract::ports::PortOpeningKind::Place,
                orch_api_contract::ports::PortOpeningKind::Link,
                orch_api_contract::ports::PortOpeningKind::Host,
            ];
            policy.provider_upstreams = vec![ALL_KEYS.into()];
            policy.provider_arrangements = vec![
                ArrangementType::Separated,
                ArrangementType::KeySeparated,
                ArrangementType::CredentialShared,
                ArrangementType::HandedOver,
            ];
        }
        policy
    }

    /// Resolve one effective policy: delegated omissions use managed defaults, kept omissions cautious
    /// defaults, Orch values apply only to delegated keys, and explicit local values always win.
    pub fn resolve(
        delegated: &[String],
        local: &PolicyValues,
        orch: &PolicyValues,
    ) -> Result<Self, PolicyError> {
        for name in local.keys().chain(orch.keys()) {
            let _: PolicyKey = name.parse()?;
        }
        let cautious = Self::installed(PolicyDefaults::Cautious);
        let managed = Self::installed(PolicyDefaults::Managed);
        let mut resolved = cautious.clone();
        resolved.set(PolicyKey::PolicyDelegated, &serde_json::json!(delegated))?;
        for key in PolicyKey::ALL
            .iter()
            .copied()
            .filter(|key| *key != PolicyKey::PolicyDelegated)
        {
            let handed = key.handed_by(delegated);
            resolved.set(
                key,
                &if handed {
                    managed.get(key)
                } else {
                    cautious.get(key)
                },
            )?;
            if handed && let Some(value) = orch.get(key.as_str()) {
                resolved.set(key, value)?;
            }
            if let Some(value) = local.get(key.as_str()) {
                resolved.set(key, value)?;
            }
        }
        resolved.local_values = local
            .iter()
            .filter(|(key, _)| key.as_str() != PolicyKey::PolicyDelegated.as_str())
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        Ok(resolved)
    }

    /// The shipped value of each known key, available to enrolment and policy screens as data.
    #[must_use]
    pub fn shipped_values(kind: PolicyDefaults) -> BTreeMap<String, Json> {
        let policy = Self::installed(kind);
        PolicyKey::ALL
            .iter()
            .map(|key| (key.as_str().into(), policy.get(*key)))
            .collect()
    }

    /// The value of `key`.
    #[must_use]
    pub fn get(&self, key: PolicyKey) -> Json {
        let texts = |list: &[String]| Json::from(list.to_vec());
        match key {
            PolicyKey::Drivers => serde_json::json!(self.drivers),
            PolicyKey::Storages => serde_json::json!(self.storages),
            PolicyKey::ResourceMax => serde_json::json!(self.resource_max),
            PolicyKey::LifecycleHooks => serde_json::json!(self.hooks),
            PolicyKey::EnvironmentRoots => texts(&self.environment_roots),
            PolicyKey::EnvironmentRemoteWrite => Json::Bool(self.environment_remote_write),
            PolicyKey::EnvironmentTerminal => Json::Bool(self.environment_terminal),
            PolicyKey::EnvironmentAfterUp => Json::Bool(self.environment_after_up),
            PolicyKey::EnvironmentBrowser => Json::Bool(self.environment_browser),
            PolicyKey::EnvironmentVnc => Json::Bool(self.environment_vnc),
            PolicyKey::Ports => serde_json::json!(self.ports),
            PolicyKey::McpImport => Json::Bool(self.mcp_import),
            PolicyKey::McpAdd => Json::Bool(self.mcp_add),
            PolicyKey::ExecOffers => texts(&self.exec_offers),
            PolicyKey::EnvironmentDeliverFiles => Json::Bool(self.environment_deliver_files),
            PolicyKey::AgentMaxSandbox => Json::from(self.agent_max_sandbox.as_str()),
            PolicyKey::ProviderSignIn => Json::Bool(self.provider_signin),
            PolicyKey::ProviderUpstreams => texts(&self.provider_upstreams),
            PolicyKey::ProviderArrangements => Json::from(
                self.provider_arrangements
                    .iter()
                    .map(|kind| kind.as_str())
                    .collect::<Vec<_>>(),
            ),
            PolicyKey::AgentHarnesses => Json::from(
                self.agent_harnesses
                    .iter()
                    .map(
                        |harness| serde_json::json!({ "kind": harness.kind, "path": harness.path }),
                    )
                    .collect::<Vec<_>>(),
            ),
            PolicyKey::ToolsFetch => Json::Bool(self.tools_fetch),
            PolicyKey::AgentPrograms => Json::Bool(self.agent_programs),
            PolicyKey::ProviderPrograms => Json::Bool(self.provider_programs),
            PolicyKey::MachineRoles => Json::Bool(self.machine_roles),
            PolicyKey::UpdateAuto => Json::Bool(self.update_auto),
            PolicyKey::PolicyDelegated => texts(&self.policy_delegated),
        }
    }

    /// Sets `key` to `value`, as the machine's owner does on the machine.
    /// A value that does not fit the key changes nothing.
    pub fn set(&mut self, key: PolicyKey, value: &Json) -> Result<(), PolicyError> {
        let wrong = |expected: &'static str| PolicyError::Value { key, expected };
        let flag = || value.as_bool().ok_or_else(|| wrong("true or false"));
        let texts = |expected: &'static str| {
            Vec::<String>::deserialize(value)
                .ok()
                .filter(|list| list.iter().all(|text| !text.is_empty()))
                .ok_or_else(|| wrong(expected))
        };
        match key {
            PolicyKey::Drivers => {
                self.drivers = serde_json::from_value::<MachineDrivers>(value.clone())
                    .ok()
                    .filter(MachineDrivers::valid)
                    .ok_or_else(|| {
                        wrong("driver offers, allow-lists and absolute host program paths")
                    })?;
            }
            PolicyKey::Storages => {
                let storages: Vec<MachineStorage> = serde_json::from_value(value.clone())
                    .map_err(|_| wrong("named storage directories"))?;
                let names: std::collections::BTreeSet<_> =
                    storages.iter().map(|s| &s.name).collect();
                if names.len() != storages.len()
                    || storages.iter().any(|s| {
                        s.name.trim().is_empty() || !std::path::Path::new(&s.path).is_absolute()
                    })
                    || (!storages.is_empty() && storages.iter().filter(|s| s.default).count() != 1)
                {
                    return Err(wrong(
                        "unique storage names, absolute paths and exactly one default",
                    ));
                }
                self.storages = storages;
            }
            PolicyKey::ResourceMax => {
                let limits: orch_driver_contract::runtime::Resources =
                    serde_json::from_value(value.clone())
                        .map_err(|_| wrong("resource ceilings"))?;
                if [
                    limits.cpu_millis,
                    limits.memory_mb,
                    limits.memory_swap_mb,
                    limits.blkio_weight,
                    limits.pids,
                    limits.disk_gb,
                ]
                .into_iter()
                .flatten()
                .any(|value| value <= 0)
                {
                    return Err(wrong("positive per-resource ceilings"));
                }
                self.resource_max = limits;
            }
            PolicyKey::LifecycleHooks => {
                self.hooks = MachineHooks::deserialize(value)
                    .ok()
                    .filter(MachineHooks::valid)
                    .ok_or_else(|| wrong("absolute hook programs and a nonzero timeout_secs"))?;
            }
            PolicyKey::EnvironmentRoots => self.environment_roots = texts("a list of directories")?,
            PolicyKey::EnvironmentRemoteWrite => self.environment_remote_write = flag()?,
            PolicyKey::EnvironmentTerminal => self.environment_terminal = flag()?,
            PolicyKey::EnvironmentAfterUp => self.environment_after_up = flag()?,
            PolicyKey::EnvironmentBrowser => self.environment_browser = flag()?,
            PolicyKey::EnvironmentVnc => self.environment_vnc = flag()?,
            PolicyKey::Ports => {
                self.ports = MachinePorts::deserialize(value).ok().filter(MachinePorts::valid)
                    .ok_or_else(||wrong("allowed audiences, a reachable address, all or a local IP, and valid port ranges"))?;
            }
            PolicyKey::McpImport => self.mcp_import = flag()?,
            PolicyKey::McpAdd => self.mcp_add = flag()?,
            PolicyKey::ExecOffers => {
                self.exec_offers = texts("a list of executor kinds and tool sets")?
            }
            PolicyKey::EnvironmentDeliverFiles => self.environment_deliver_files = flag()?,
            PolicyKey::AgentMaxSandbox => {
                self.agent_max_sandbox = SandboxMode::deserialize(value)
                    .map_err(|_| wrong("read_only, workspace or open"))?;
            }
            PolicyKey::ProviderSignIn => self.provider_signin = flag()?,
            PolicyKey::ProviderUpstreams => self.provider_upstreams = texts("a list of addresses")?,
            PolicyKey::ProviderArrangements => {
                self.provider_arrangements = Vec::<ArrangementType>::deserialize(value)
                    .ok()
                    .filter(|list| list.iter().all(|kind| kind.needs_consent()))
                    .ok_or_else(|| {
                        wrong(
                            "a list of separated, key_separated, credential_shared and handed_over",
                        )
                    })?;
            }
            PolicyKey::AgentHarnesses => {
                self.agent_harnesses = Vec::<PolicyHarness>::deserialize(value)
                    .ok()
                    .filter(|list| {
                        list.iter()
                            .all(|harness| !harness.kind.is_empty() && !harness.path.is_empty())
                    })
                    .ok_or_else(|| {
                        wrong("a list of harness kinds, each with the path of its program")
                    })?;
            }
            PolicyKey::ToolsFetch => self.tools_fetch = flag()?,
            PolicyKey::AgentPrograms => self.agent_programs = flag()?,
            PolicyKey::ProviderPrograms => self.provider_programs = flag()?,
            PolicyKey::MachineRoles => self.machine_roles = flag()?,
            PolicyKey::UpdateAuto => self.update_auto = flag()?,
            PolicyKey::PolicyDelegated => {
                self.policy_delegated = texts("a list of keys")?
                    .into_iter()
                    .map(|name| {
                        if PolicyKey::delegation_name(&name) {
                            Ok(name)
                        } else {
                            Err(wrong("keys other than itself, or `*`"))
                        }
                    })
                    .collect::<Result<_, _>>()?;
            }
        }
        Ok(())
    }

    /// Whether `key` may be changed from the system: its owner handed it
    /// over, by name or with all the others. The list of what was handed
    /// over is itself never changed from the system.
    #[must_use]
    pub fn delegates(&self, key: PolicyKey) -> bool {
        key != PolicyKey::PolicyDelegated
            && !self.local_values.contains_key(key.as_str())
            && self
                .roles
                .as_ref()
                .is_none_or(|roles| key.applies_to_roles(roles))
            && key.handed_by(&self.policy_delegated)
    }

    /// Applies a change that arrived from the system, key by key: a key
    /// is changed only when it is known, was handed over and is given a
    /// value that fits; every other key is refused and left as it was.
    pub fn apply(&mut self, values: &BTreeMap<String, Json>) -> PolicyChange {
        let mut change = PolicyChange::default();
        for (name, value) in values {
            let applied = name
                .parse::<PolicyKey>()
                .ok()
                .filter(|key| self.delegates(*key))
                .is_some_and(|key| self.set(key, value).is_ok());
            if applied {
                change.applied.push(name.clone())
            } else {
                change.refused.push(name.clone())
            }
        }
        change
    }

    /// The sandbox mode a harness is started with when the system asks
    /// for `asked`: never wider than the machine's ceiling.
    #[must_use]
    pub fn sandbox(&self, asked: SandboxMode) -> SandboxMode {
        asked.min(self.agent_max_sandbox)
    }

    /// Whether a credential kept on the machine may be used in an
    /// arrangement of `kind`. One in which everything is on this machine
    /// anyway needs nobody's consent.
    pub fn check_arrangement(&self, kind: ArrangementType) -> Result<(), ErrorCode> {
        if kind.needs_consent() && !self.provider_arrangements.contains(&kind) {
            return Err(ErrorCode::PolicyArrangementNotAllowed);
        }
        Ok(())
    }

    /// Whether the machine may forward model calls to `base_url`: the
    /// address has to be listed exactly.
    pub fn check_upstream(&self, base_url: &str) -> Result<(), ErrorCode> {
        if !self
            .provider_upstreams
            .iter()
            .any(|allowed| allowed == ALL_KEYS || allowed == base_url)
        {
            return Err(ErrorCode::PolicyUpstreamNotAllowed);
        }
        Ok(())
    }

    /// Whether the machine's exec node may offer `offer`: an executor kind, or a tool set of an executor with
    /// several (`executor:tool_set`, or the set alone as older policies wrote it).
    pub fn check_offer(&self, offer: &str) -> Result<(), ErrorCode> {
        if !orch_agent_programs::Programs::builtin().offers_include(&self.exec_offers, offer) {
            return Err(ErrorCode::PolicyExecNotOffered);
        }
        Ok(())
    }

    /// Whether a file written through the system is accepted.
    pub fn check_remote_write(&self) -> Result<(), ErrorCode> {
        if self.environment_remote_write {
            Ok(())
        } else {
            Err(ErrorCode::PolicyRemoteWriteDisabled)
        }
    }

    /// Whether files that arrive with a message are written into the
    /// workspace.
    pub fn check_deliver_files(&self) -> Result<(), ErrorCode> {
        if self.environment_deliver_files {
            Ok(())
        } else {
            Err(ErrorCode::PolicyDeliverFilesDisabled)
        }
    }

    /// Whether the tool servers found on the machine may be listed and
    /// imported.
    pub fn check_mcp_import(&self) -> Result<(), ErrorCode> {
        if self.mcp_import {
            Ok(())
        } else {
            Err(ErrorCode::PolicyMcpImportDisabled)
        }
    }

    /// Whether a tool server may be written from the system for an
    /// environment of the machine. Importing one the machine's own harnesses
    /// configured is another matter: [`LocalPolicy::check_mcp_import`].
    pub fn check_mcp_add(&self) -> Result<(), ErrorCode> {
        if self.mcp_add {
            Ok(())
        } else {
            Err(ErrorCode::PolicyMcpAddDisabled)
        }
    }

    /// Whether an environment may have its root, or a mount its location, at
    /// `directory`: it has to be inside one of the directories the machine's
    /// owner offers. Judged by the names alone, with `.` and `..` resolved:
    /// a path that climbs out of an offered directory is outside it. Whoever
    /// calls this resolves symbolic links first, because only the file system
    /// knows where they lead.
    pub fn check_root(&self, directory: &std::path::Path) -> Result<(), ErrorCode> {
        let Some(wanted) = lexical(directory) else {
            return Err(ErrorCode::PolicyRootNotAllowed);
        };
        let offered = self
            .environment_roots
            .iter()
            .filter_map(|root| lexical(std::path::Path::new(root)))
            .any(|root| wanted.starts_with(root));
        if offered {
            Ok(())
        } else {
            Err(ErrorCode::PolicyRootNotAllowed)
        }
    }

    /// The entry that lets the machine run harnesses of `kind`.
    #[must_use]
    pub fn harness(&self, kind: &str) -> Option<&PolicyHarness> {
        self.agent_harnesses
            .iter()
            .find(|harness| harness.kind == kind)
    }
}

/// The scoped policy in a machine report; older unscoped policies retain their wire shape.
pub(crate) fn serialize_reported<S: serde::Serializer>(
    policy: &Option<LocalPolicy>,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    policy
        .as_ref()
        .map(LocalPolicy::reported)
        .serialize(serializer)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn an_environment_role_can_report_harness_programs_without_delegating_them() {
        use crate::session::Role;
        assert!(PolicyKey::AgentHarnesses.applies_to_roles(&[Role::Environment]));
        assert!(!PolicyKey::AgentHarnesses.handed_by(&["environment.*".into()]));
        assert!(PolicyKey::AgentHarnesses.handed_by(&["agent.harnesses".into()]));
    }

    #[test]
    fn environment_switches_follow_the_same_machine_ceiling_and_an_explicit_narrowing_stays_off() {
        let full = LocalPolicy::resolve(
            &[ALL_KEYS.into()],
            &PolicyValues::new(),
            &PolicyValues::new(),
        )
        .unwrap();
        let host = LocalPolicy::installed(PolicyDefaults::Managed);
        assert_eq!(
            full.environment_switches(None, None, None, None),
            host.environment_switches(None, None, None, None)
        );
        assert!(
            full.environment_switches(None, None, None, None)
                .remote_write
        );
        assert!(
            !full
                .environment_switches(Some(false), None, None, None)
                .remote_write
        );
        let personal = LocalPolicy::default();
        assert!(
            !personal
                .environment_switches(Some(true), Some(true), Some(true), Some(true))
                .remote_write
        );
    }

    #[test]
    fn full_access_uses_the_hosts_answers_and_local_constraints_win_on_both() {
        let handed = vec![ALL_KEYS.to_owned()];
        let orch = values(&[
            ("environment.browser", json!(false)),
            ("agent.max_sandbox", json!("workspace")),
        ]);
        let full = LocalPolicy::resolve(&handed, &PolicyValues::new(), &orch).unwrap();
        let host = LocalPolicy::resolve(&handed, &PolicyValues::new(), &orch).unwrap();
        assert_eq!(full, host);
        assert!(full.environment_terminal && full.environment_after_up && full.mcp_add);
        assert!(!full.environment_browser);
        assert_eq!(full.agent_max_sandbox, SandboxMode::Workspace);
        let local = values(&[
            ("environment.terminal", json!(false)),
            ("agent.max_sandbox", json!("read_only")),
        ]);
        for delegated in [&handed, &host.policy_delegated] {
            let mut policy = LocalPolicy::resolve(delegated, &local, &orch).unwrap();
            let answer = policy.apply(&values(&[
                ("environment.terminal", json!(true)),
                ("agent.max_sandbox", json!("open")),
            ]));
            assert_eq!(
                answer.refused,
                ["agent.max_sandbox", "environment.terminal"]
            );
            assert!(!policy.environment_terminal);
            assert_eq!(policy.agent_max_sandbox, SandboxMode::ReadOnly);
        }
        let kept = LocalPolicy::resolve(&[], &PolicyValues::new(), &orch).unwrap();
        assert!(
            !kept.environment_terminal && !kept.environment_browser && !kept.environment_after_up
        );
    }

    #[test]
    fn environment_full_access_and_a_nodes_units_have_the_same_environment_answers() {
        use crate::session::Role;
        let orch = values(&[
            ("environment.browser", json!(false)),
            ("agent.max_sandbox", json!("workspace")),
        ]);
        let mut directory =
            LocalPolicy::resolve(&["environment.*".into()], &PolicyValues::new(), &orch).unwrap();
        directory.roles = Some(vec![Role::Environment]);
        let mut node =
            LocalPolicy::resolve(&[ALL_KEYS.into()], &PolicyValues::new(), &orch).unwrap();
        node.roles = Some(vec![Role::Node]);
        for key in PolicyKey::ALL
            .iter()
            .filter(|key| key.sections().contains(&PolicySection::Environment))
        {
            assert_eq!(directory.get(*key), node.get(*key), "{}", key.as_str());
            assert_eq!(directory.delegates(*key), node.delegates(*key));
        }
        let local = values(&[("environment.terminal", json!(false))]);
        let kept = LocalPolicy::resolve(&[ALL_KEYS.into()], &local, &orch).unwrap();
        assert!(
            !kept
                .environment_switches(None, Some(true), None, None)
                .terminal
        );
    }

    #[test]
    fn role_handover_never_hands_over_another_roles_key_and_reports_only_offered_sections() {
        use crate::session::Role;
        let mut policy = LocalPolicy::resolve(
            &["environment.*".into()],
            &values(&[("provider.signin", json!(true))]),
            &PolicyValues::new(),
        )
        .unwrap();
        policy.roles = Some(vec![Role::Environment]);
        assert!(policy.delegates(PolicyKey::EnvironmentTerminal));
        assert!(policy.delegates(PolicyKey::AgentMaxSandbox));
        assert!(!policy.delegates(PolicyKey::ProviderSignIn));
        assert!(!PolicyKey::AgentMaxSandbox.handed_by(&["agent.*".into()]));
        let report = policy.reported();
        assert_eq!(report["environment.terminal"], json!(true));
        assert!(report.get("ports").is_some());
        assert!(report.get("storages").is_none());
        assert!(report.get("provider.signin").is_none());
        assert!(report.get("policy.local").is_none());
        assert!(
            PolicyKey::EnvironmentTerminal
                .definition()
                .sections
                .contains(&PolicySection::Environment)
        );
    }

    #[test]
    fn native_accounts_and_provider_directories_report_the_same_policies_their_commands_consume() {
        use crate::session::Role;
        for role in [Role::Agent, Role::Provider] {
            let mut policy = LocalPolicy::resolve(
                &["agent.*".into()],
                &values(&[
                    ("provider.signin", json!(true)),
                    ("environment.after_up", json!(true)),
                    ("environment.roots", json!(["/allowed/home"])),
                ]),
                &PolicyValues::new(),
            )
            .unwrap();
            policy.roles = Some(vec![role]);
            let report = policy.reported();
            assert_eq!(report["environment.after_up"], json!(true));
            assert_eq!(report["environment.roots"], json!(["/allowed/home"]));
            assert_eq!(report["provider.signin"], json!(true));
            assert!(report.get("provider.arrangements").is_some());
            assert!(
                !policy.delegates(PolicyKey::ProviderSignIn),
                "agent handover never hands over a provider-section key"
            );
            assert!(!PolicyKey::ProviderSignIn.applies_to_roles(&[Role::Environment]));
        }
    }

    #[test]
    fn resolve_names_an_unknown_key_before_resolving_any_defaults() {
        let error = LocalPolicy::resolve(
            &[ALL_KEYS.into()],
            &values(&[("environment.typo", json!(false))]),
            &PolicyValues::new(),
        )
        .unwrap_err();
        assert_eq!(error, PolicyError::UnknownKey("environment.typo".into()));
    }

    #[test]
    fn a_sandboxed_environment_may_keep_its_inbound_audiences_without_handing_over_other_doors() {
        use orch_api_contract::ports::PortOpeningKind as Audience;
        let local = values(&[(
            "ports",
            json!({"allowed":["place","link","host"],"publish":{"incoming_address":"vm.example.test","listen":"all","ranges":[{"from":41000,"to":41010}]}}),
        )]);
        let policy = LocalPolicy::resolve(&[], &local, &PolicyValues::new()).unwrap();
        assert!(policy.permits_port(Some(Audience::Place)));
        assert!(policy.permits_port(Some(Audience::Link)));
        assert!(policy.permits_port(Some(Audience::Host)));
        assert!(!policy.environment_remote_write && !policy.environment_terminal);
        assert_eq!(policy.agent_max_sandbox, SandboxMode::Workspace);
        assert!(PolicyKey::Ports.handed_by(&["environment.*".into()]));
        assert!(!PolicyKey::Ports.handed_by(&["node.*".into()]));
        assert!(
            "connector.dedicated".parse::<PolicyKey>().is_err(),
            "a dedicated-machine fact is never a delegated setting"
        );
    }

    #[test]
    fn reported_sections_include_services_a_role_supplies_to_its_units_or_native_harness() {
        use crate::session::Role;
        assert!(PolicyKey::ToolsFetch.applies_to_roles(&[Role::Node]));
        assert!(!PolicyKey::AgentHarnesses.applies_to_roles(&[Role::Node]));
        assert!(PolicyKey::AgentMaxSandbox.applies_to_roles(&[Role::Agent]));
        assert!(!PolicyKey::ToolsFetch.handed_by(&["environment.*".into()]));
    }

    fn values(pairs: &[(&str, Json)]) -> BTreeMap<String, Json> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_owned(), value.clone()))
            .collect()
    }

    #[test]
    fn a_new_machine_keeps_everything_to_itself() {
        let policy = LocalPolicy::default();
        assert!(PolicyKey::ALL.iter().all(|key| !policy.delegates(*key)));
        assert!(!policy.environment_remote_write);
        assert!(!policy.environment_terminal);
        assert!(!policy.mcp_add);
        assert_eq!(
            policy.check_arrangement(ArrangementType::CredentialShared),
            Err(ErrorCode::PolicyArrangementNotAllowed)
        );
    }

    #[test]
    fn an_environment_may_have_its_root_only_inside_a_directory_the_owner_offers() {
        let mut policy = LocalPolicy::default();
        let nowhere = std::path::Path::new("/srv/work");
        assert_eq!(
            policy.check_root(nowhere),
            Err(ErrorCode::PolicyRootNotAllowed),
            "nothing is offered as installed"
        );
        policy
            .set(
                PolicyKey::EnvironmentRoots,
                &json!(["/srv/work", "/srv/data"]),
            )
            .unwrap();
        for inside in [
            "/srv/work",
            "/srv/work/api/src",
            "/srv/data/./set",
            "/srv/work/a/../b",
        ] {
            assert_eq!(
                policy.check_root(std::path::Path::new(inside)),
                Ok(()),
                "{inside}"
            );
        }
        for outside in [
            "/srv",
            "/srv/workshop",
            "/srv/work/../secrets",
            "/etc",
            "/",
            "relative/path",
            "/srv/work/../../..",
        ] {
            assert_eq!(
                policy.check_root(std::path::Path::new(outside)),
                Err(ErrorCode::PolicyRootNotAllowed),
                "{outside}"
            );
        }
    }

    #[test]
    fn a_tool_server_written_from_the_system_is_refused_while_the_owner_has_not_allowed_it() {
        let mut policy = LocalPolicy::default();
        assert_eq!(policy.check_mcp_add(), Err(ErrorCode::PolicyMcpAddDisabled));
        assert_eq!(
            policy.check_mcp_import(),
            Ok(()),
            "importing what the machine's own harnesses configured is another matter"
        );
        // Only the owner changes it, unless the owner handed it over.
        let refused = policy.apply(&values(&[("mcp.add", json!(true))]));
        assert_eq!(refused.refused, ["mcp.add"]);
        assert_eq!(policy.check_mcp_add(), Err(ErrorCode::PolicyMcpAddDisabled));
        policy
            .set(PolicyKey::PolicyDelegated, &json!(["mcp.add"]))
            .unwrap();
        let applied = policy.apply(&values(&[("mcp.add", json!(true))]));
        assert_eq!(applied.applied, ["mcp.add"]);
        assert_eq!(policy.check_mcp_add(), Ok(()));
    }

    #[test]
    fn a_sandbox_wider_than_the_owner_allows_is_clamped_to_what_the_owner_allows() {
        let policy = LocalPolicy::default();
        assert_eq!(policy.sandbox(SandboxMode::Open), SandboxMode::Workspace);
        assert_eq!(policy.sandbox(SandboxMode::ReadOnly), SandboxMode::ReadOnly);
        assert_eq!(
            policy.sandbox(SandboxMode::Workspace),
            SandboxMode::Workspace
        );
    }

    #[test]
    fn a_policy_is_reported_with_every_key_under_its_own_name() {
        let written = serde_json::to_value(LocalPolicy::default()).unwrap();
        let members = written.as_object().unwrap();
        assert_eq!(members.len(), PolicyKey::ALL.len());
        for key in PolicyKey::ALL {
            assert_eq!(
                members.get(key.as_str()),
                Some(&LocalPolicy::default().get(*key)),
                "{key}"
            );
        }
        assert_eq!(written["agent.max_sandbox"], "workspace");
        assert_eq!(
            written["provider.arrangements"],
            json!(["separated", "key_separated"])
        );
        assert_eq!(
            serde_json::from_value::<LocalPolicy>(written).unwrap(),
            LocalPolicy::default()
        );
    }

    #[test]
    fn the_system_changes_only_what_was_handed_over() {
        let mut policy = LocalPolicy::default();
        policy
            .set(
                PolicyKey::PolicyDelegated,
                &json!(["environment.roots", "environment.terminal"]),
            )
            .unwrap();
        let change = policy.apply(&values(&[
            ("environment.roots", json!(["/srv/work"])),
            ("environment.terminal", json!(true)),
            ("environment.remote_write", json!(true)),
        ]));
        assert_eq!(
            change.applied,
            ["environment.roots", "environment.terminal"]
        );
        assert_eq!(change.refused, ["environment.remote_write"]);
        assert_eq!(policy.environment_roots, ["/srv/work"]);
        assert!(policy.environment_terminal);
        assert!(!policy.environment_remote_write);
    }

    #[test]
    fn a_machine_that_handed_everything_over_still_keeps_the_handing_over() {
        let mut policy = LocalPolicy::default();
        policy
            .set(PolicyKey::PolicyDelegated, &json!(["*"]))
            .unwrap();
        assert!(policy.delegates(PolicyKey::ToolsFetch));
        assert!(!policy.delegates(PolicyKey::PolicyDelegated));
        let change = policy.apply(&values(&[
            ("policy.delegated", json!([])),
            ("tools.fetch", json!(true)),
        ]));
        assert_eq!(change.applied, ["tools.fetch"]);
        assert_eq!(change.refused, ["policy.delegated"]);
        assert_eq!(policy.policy_delegated, ["*"]);
    }

    #[test]
    fn an_unknown_key_and_a_value_that_does_not_fit_are_refused_and_change_nothing() {
        let mut policy = LocalPolicy::default();
        policy
            .set(PolicyKey::PolicyDelegated, &json!(["*"]))
            .unwrap();
        let before = policy.clone();
        let change = policy.apply(&values(&[
            ("agent.max_sandbox", json!("wide_open")),
            ("environment.terminal", json!("yes")),
            ("provider.arrangements", json!(["together"])),
            ("no.such_key", json!(true)),
        ]));
        assert!(change.applied.is_empty());
        assert_eq!(change.refused.len(), 4);
        assert_eq!(policy, before);
    }

    #[test]
    fn handing_over_names_known_keys_only_and_never_itself() {
        let mut policy = LocalPolicy::default();
        assert!(
            policy
                .set(PolicyKey::PolicyDelegated, &json!(["policy.delegated"]))
                .is_err()
        );
        assert!(
            policy
                .set(PolicyKey::PolicyDelegated, &json!(["environment.root"]))
                .is_err()
        );
        assert!(
            policy
                .set(PolicyKey::PolicyDelegated, &json!(["mcp.add", "*"]))
                .is_ok()
        );
    }

    #[test]
    fn a_sandbox_wider_than_the_machine_allows_is_narrowed_to_its_ceiling() {
        let policy = LocalPolicy::default();
        assert_eq!(policy.sandbox(SandboxMode::Open), SandboxMode::Workspace);
        assert_eq!(policy.sandbox(SandboxMode::ReadOnly), SandboxMode::ReadOnly);
    }

    #[test]
    fn model_calls_go_only_to_the_addresses_the_machine_lists() {
        let mut policy = LocalPolicy::default();
        assert_eq!(
            policy.check_upstream("https://api.example.com/v1"),
            Err(ErrorCode::PolicyUpstreamNotAllowed)
        );
        policy
            .provider_upstreams
            .push("https://api.example.com/v1".into());
        assert_eq!(policy.check_upstream("https://api.example.com/v1"), Ok(()));
        assert_eq!(
            policy.check_upstream("https://api.example.com/v2"),
            Err(ErrorCode::PolicyUpstreamNotAllowed)
        );
    }

    #[test]
    fn an_exec_node_offers_only_what_its_machine_lets_it() {
        let mut policy = LocalPolicy::default();
        assert_eq!(policy.check_offer("plain"), Ok(()));
        policy.exec_offers = vec!["plain".into()];
        assert_eq!(
            policy.check_offer("codex-exec-server"),
            Err(ErrorCode::PolicyExecNotOffered)
        );
    }

    #[test]
    fn an_arrangement_that_keeps_everything_on_one_machine_needs_no_consent() {
        let policy = LocalPolicy::default();
        assert_eq!(policy.check_arrangement(ArrangementType::Together), Ok(()));
        assert_eq!(policy.check_arrangement(ArrangementType::Separated), Ok(()));
        assert_eq!(
            policy.check_arrangement(ArrangementType::HandedOver),
            Err(ErrorCode::PolicyArrangementNotAllowed)
        );
    }

    #[test]
    fn writes_and_delivered_files_follow_their_own_switches() {
        let policy = LocalPolicy::default();
        assert_eq!(
            policy.check_remote_write(),
            Err(ErrorCode::PolicyRemoteWriteDisabled)
        );
        assert_eq!(policy.check_deliver_files(), Ok(()));
        assert_eq!(policy.check_mcp_import(), Ok(()));
    }

    #[test]
    fn each_key_says_whose_it_is_to_change() {
        assert_eq!(PolicyKey::EnvironmentRoots.bounds(), Bounds::Machine);
        assert_eq!(PolicyKey::ProviderUpstreams.bounds(), Bounds::Providers);
        assert_eq!(PolicyKey::UpdateAuto.bounds(), Bounds::Machine);
        assert_eq!(
            "exec.offers".parse::<PolicyKey>(),
            Ok(PolicyKey::ExecOffers)
        );
        assert!("exec.offer".parse::<PolicyKey>().is_err());
    }
}
