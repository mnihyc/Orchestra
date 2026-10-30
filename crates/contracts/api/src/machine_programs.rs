//! Machine-local adapter and provider-source registration, inspection and installation facts.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The contract a registered program implements.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum MachineProgramKind {
    /// An agent adapter with sessions, inputs and neutral events.
    Agent,
    /// A provider source with models, facts or credential presentation.
    ProviderSource,
}

/// A program and separate arguments: these are never executed through a shell.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineProgramRegistration {
    /// The contract the program implements.
    pub kind: MachineProgramKind,
    /// Its stable local registration name.
    pub name: String,
    /// Absolute executable path or a program on the connector's PATH, followed by arguments.
    pub command: Vec<String>,
}

/// The identity of an already registered program; no new command can be supplied by Test or Remove.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MachineProgramKey {
    /// The contract.
    pub kind: MachineProgramKind,
    /// Its registration name.
    pub name: String,
}

/// Who supplied a registration.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MachineProgramOrigin {
    /// The operator's configuration or runtime template.
    Local,
    /// A machine manager, accepted and persisted by this connector.
    Orch,
}

/// One registered program and the result of its latest actual declaration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineProgram {
    /// The installed command and contract.
    #[serde(flatten)]
    pub registration: MachineProgramRegistration,
    /// Local configuration is never removed from Orch.
    pub origin: MachineProgramOrigin,
    /// Ready or failed, from the program's supervisor.
    pub ready: bool,
    /// The program's reported version, when known.
    pub version: Option<String>,
    /// The validated contract descriptor returned by describe.
    pub descriptor: Json,
    /// A model catalogue returned by a source's test, when it supplies one without extra configuration.
    pub models: Option<Json>,
    /// A specific failed-start, describe or model-test explanation.
    pub detail: Option<String>,
}

/// The machine's own registration rule for one offered contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineProgramCapability {
    /// Which contract.
    pub kind: MachineProgramKind,
    /// Whether the running connector hosts this contract for one of its offered roles.
    pub supported: bool,
    /// Whether this key is handed to Orch; kept local values always make this false.
    #[serde(default)]
    pub delegated: bool,
    /// Whether registration is currently enabled in the enforcing policy.
    #[serde(default)]
    pub enabled: bool,
    /// Whether its operator handed registration to Orch and kept it enabled.
    pub remote: bool,
    /// The controlling policy key, for exact local commands and refusal attribution.
    pub policy_key: String,
    /// Why it is unavailable or locally controlled.
    pub reason: Option<String>,
}

/// One whole projection: initial reported state and every mutation's acknowledgement have this shape.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachinePrograms {
    /// The changed machine's authorized view on HTTP mutation acknowledgement; absent in native reports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub machine_view: Option<crate::resources::Machine>,
    /// Current registrations, excluding built-in implementations.
    pub entries: Vec<MachineProgram>,
    /// The contracts this running machine offers and who controls registration.
    pub capabilities: Vec<MachineProgramCapability>,
    /// The operator's local configuration file, when reported.
    pub config_file: Option<String>,
    /// Filled by the API's actual machine authority; native reports carry false.
    pub can_manage: bool,
}

/// An operation through the connector's one program registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum MachineProgramsCommand {
    /// Register and describe a command, persisting it only after successful negotiation.
    Register(MachineProgramRegistration),
    /// Call describe on an existing registration.
    Test(MachineProgramKey),
    /// Remove an Orch-owned registration after active turns have finished.
    Remove(MachineProgramKey),
    /// Reload operator configuration and the connector's persisted registrations.
    Reload,
}

/// How this physical program is installed; it is independent of machine placement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineInstallation {
    /// orch-connector or orch-node.
    pub program: String,
    /// The actual executable.
    pub executable: String,
    /// The connector identity/data directory.
    pub data_dir: String,
    /// The physical program's configuration.
    pub config_file: String,
    /// The account it runs as, when observed.
    pub user: Option<String>,
    /// Whether this installation is a system/root program.
    pub system: bool,
    /// The installed service's real name when observed; none when no service was reported.
    pub service: Option<String>,
    /// Exact commands for this installation, including its own data directory.
    pub commands: MachineInstallationCommands,
}

/// Local lifecycle commands shown together once, beside the installation facts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineInstallationCommands {
    /// Exact program/instance prefix followed by policy, for kept values and handover commands.
    pub policy: String,
    /// Exact connector role-command prefix; a node offers its role through its own program only.
    pub roles: Option<String>,
    /// Start this instance.
    pub start: String,
    /// Stop this instance.
    pub stop: String,
    /// Update this instance's executable through its verified updater.
    pub update: String,
    /// Reviewed uninstall command; null when installation ownership cannot be established.
    #[serde(default)]
    pub uninstall: Option<String>,
}
