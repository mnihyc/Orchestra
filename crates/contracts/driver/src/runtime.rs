//! Driver-neutral RuntimeSpec and required connector launch inputs.
//!
//! Everything Orch runs on a host for people is a **runtime**: a unit that a driver makes from a template.
//! There are two pools of them ([`RuntimeKind`]): the environments that threads work in, and a provider's
//! runtime, where its credential is kept. The body of the request to make one is a [`RuntimeSpec`], which
//! is the same for every driver; the driver's own part of it is `spec`, in the schema the driver publishes.
//!
//! **What every driver must deliver.** The unit's main process is `orch-connector run --managed`, started
//! with the variables of [`LAUNCH_VARIABLES`]. How the unit comes to exist, which user it runs as, and how
//! limits and confinement are applied are the driver's business; the variables are the whole of what the
//! rest of Orch relies on. Nothing outside a driver knows a path, an image, a user or a network.
//! [`RuntimeSpec::launch`] works out what the spec fixes of them.

use std::collections::BTreeMap;

use orch_plugin_contract::Secret;
use orch_types::{Id, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::mounts::{AttachedMount, MountSpec};
use crate::network::NetworkPolicy;

/// The label that names the system a unit belongs to; a driver never touches a unit without it.
pub const LABEL_SYSTEM: &str = "orch.system";
/// The label that holds the runtime's identifier.
pub const LABEL_RUNTIME_ID: &str = "orch.runtime_id";
/// The label that holds the pool: `environment` or `provider`.
pub const LABEL_KIND: &str = "orch.kind";

/// How many hexadecimal digits of the runtime's identifier are in the name of its unit.
pub const NAME_ID_DIGITS: usize = 12;

/// The two pools of runtimes.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeKind {
    /// An environment: where the work is, its workspace and its tools.
    Environment,
    /// A provider's runtime: where its credential is kept, and its harness runs when the arrangement leaves
    /// it with the provider.
    Provider,
}

impl RuntimeKind {
    /// The kind as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Provider => "provider",
        }
    }
}

/// The roles a connector plays, as `ORCH_ROLES` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorRole {
    /// The environment's exec node.
    Environment,
    /// The provider role: the forwarder, and where a credential is issued.
    Provider,
    /// The agent role: hosts harnesses.
    Agent,
}

impl ConnectorRole {
    /// The role as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::Provider => "provider",
            Self::Agent => "agent",
        }
    }
}

/// The most a unit may use. A member that is absent is left to the driver's own default.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct Resources {
    /// Thousandths of a processor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu_millis: Option<i64>,
    /// Memory, in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<i64>,
    /// Memory plus swap in MiB; absent keeps swap disabled beyond the memory limit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_swap_mb: Option<i64>,
    /// Docker's relative block IO weight, from 10 to 1000; absent uses its default 500.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blkio_weight: Option<i64>,
    /// Processes and threads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pids: Option<i64>,
    /// Disk, in GB, where the storage driver has quotas.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_gb: Option<i64>,
}

impl Resources {
    /// Whether no member is larger than the same member of `most`; a member that is absent from `most`
    /// is no limit, and one that is absent here asks for nothing.
    #[must_use]
    pub fn within(&self, most: &Self) -> bool {
        let fits = |wanted: Option<i64>, allowed: Option<i64>| match (wanted, allowed) {
            (Some(wanted), Some(allowed)) => wanted <= allowed,
            _ => true,
        };
        fits(self.cpu_millis, most.cpu_millis)
            && fits(self.memory_mb, most.memory_mb)
            && fits(self.memory_swap_mb, most.memory_swap_mb)
            && fits(self.blkio_weight, most.blkio_weight)
            && fits(self.pids, most.pids)
            && fits(self.disk_gb, most.disk_gb)
    }
}

/// The current unit's limits, available live changes and measured use for a change form.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct RuntimeChanges {
    /// A desired resource change the node could not apply; the actual limits below remain in force.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_reason: Option<String>,
    pub live_changes: crate::descriptor::LiveChanges,
    pub resources: Resources,
    /// Absent when the stopped or offline unit cannot be measured; never replaced by a zero.
    pub memory_used_bytes: Option<i64>,
}

/// A harness build to make available inside the unit, from the host's tool cache.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ToolRef {
    /// The tool, such as `codex`.
    pub name: String,
    /// The build.
    pub version: String,
}

/// The volumes a unit keeps its data on.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum VolumeRole {
    /// The environment's workspace.
    Workspace,
    /// The user's home in the environment.
    Home,
    /// Private connector identity and the default owned harness homes.
    State,
}

/// Where one of an environment's two data roots comes from, independently of its placement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EnvironmentDataSource {
    /// The driver makes and owns the storage: a volume or a new directory as its facts declare.
    #[default]
    MadeByOrch,
    /// A directory allowed by this machine's environment.roots policy, never owned by Orch.
    ExistingDirectory {
        /// Absolute path on the machine running the environment.
        path: String,
    },
}

impl EnvironmentDataSource {
    /// A borrowed path, if this source borrows one.
    #[must_use]
    pub fn directory(&self) -> Option<&str> {
        match self {
            Self::MadeByOrch => None,
            Self::ExistingDirectory { path } => Some(path),
        }
    }
}

/// Independent workspace and home selections. Omitted selections are made by Orch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct EnvironmentDataSources {
    /// Where workspace files live.
    #[schemars(required)]
    pub workspace: EnvironmentDataSource,
    /// Where the environment user's home lives.
    #[schemars(required)]
    pub home: EnvironmentDataSource,
}

impl EnvironmentDataSources {
    /// Read a selection from own configuration; a controller resolves inherited settings before use.
    pub fn from_config(config: &Json) -> Result<Self, serde_json::Error> {
        config
            .get("env.data_sources")
            .cloned()
            .map_or_else(|| Ok(Self::default()), serde_json::from_value)
    }
    /// Whether moving to another machine requires explicit permission to copy borrowed data.
    #[must_use]
    pub fn borrows_directories(&self) -> bool {
        self.workspace.directory().is_some() || self.home.directory().is_some()
    }
}

/// A borrowed directory resolved by the keeping machine. Identity is filled by the node, never the API.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectoryBinding {
    /// Canonical absolute path on the machine.
    pub path: String,
    /// Filesystem identity checked again inside the unit before the connector runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<u64>,
    /// Directory identity, so replacement between validation and bind cannot widen the policy.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inode: Option<u64>,
}

/// An owned volume, or an explicitly borrowed workspace/home directory on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct VolumeRef {
    /// The volume's name.
    pub name: String,
    /// The storage of the host it is on; `null` is the host's default.
    pub storage: Option<String>,
    /// Existing directory instead of a volume. It is never ensured, chowned, quota limited or purged.
    /// The node checks it against its own environment.roots before passing it to a driver.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directory: Option<DirectoryBinding>,
}

/// The request to make a unit: the body of `node.runtime.create`, the same for every driver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeSpec<Credential = Secret> {
    /// Selected provider harness home before the node resolves its directory identity.
    /// Absent on older runtimes whose harness homes remain inside private state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub harness_home: Option<EnvironmentDataSource>,
    /// An owned kept image selected by the controller for a same-image recreate or restore.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kept_image: Option<crate::kept::KeptImage>,
    /// Prepared template defaults/allowlist and environment grants, distinct for replay and withdrawal.
    #[serde(
        default,
        skip_serializing_if = "crate::capabilities::CapabilityState::is_empty"
    )]
    pub capabilities: crate::capabilities::CapabilityState,
    /// The runtime.
    pub runtime_id: Id,
    /// The pool.
    pub kind: RuntimeKind,
    /// The unit's name: `orch-<kind>-<first 12 hex digits of the runtime id>`. Creating is idempotent by
    /// name.
    pub name: String,
    /// The driver.
    pub driver: String,
    /// The template's spec, in the driver's own schema; for an environment, with the driver's capability
    /// patches merged in.
    pub spec: Json,
    /// The most the unit may use, after the environment's own values.
    pub resources: Resources,
    /// The roles its connector plays.
    pub roles: Vec<ConnectorRole>,
    /// Where the connector reaches the gateway.
    pub gateway_url: String,
    /// The network policy, resolved at the runtime's home and narrowed by its own: a restricted one is
    /// enforced before the unit starts. Absent: open, as before there was one.
    #[serde(default)]
    pub network_policy: NetworkPolicy,
    /// The single-use token the connector enrolls with. Unsealed by the node for the driver.
    pub enroll_token: Credential,
    /// The certificate authority the connector trusts, as PEM.
    pub ca_cert: String,
    /// The harness builds to make available.
    pub tools: Vec<ToolRef>,
    /// An environment's workspace, home and state; a provider's private state and optional harness home.
    pub volumes: BTreeMap<VolumeRole, VolumeRef>,
    /// The mounts to attach, each with this host's location; none for a provider's runtime.
    pub mounts: Vec<MountSpec>,
    /// The spec's `env`.
    pub env: BTreeMap<String, String>,
    /// The labels by which the system finds its own units again.
    pub labels: BTreeMap<String, String>,
    /// What the node's operator hooks are told about what the unit serves, and why it was made. No driver
    /// reads it; a request made without it tells the hooks only what the node knows itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subject: Option<UnitSubject>,
}

/// Why a unit comes up or goes down, as the node's operator hooks are told it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UnitReason {
    /// An ordinary start: the first one of a unit made for a new environment or runtime, or any later one.
    Start,
    /// An ordinary stop: by a person, for being idle, by a recycle, or a unit that ended by itself.
    Stop,
    /// A unit replaced by a successor on the same volumes, or the successor that replaces it.
    Recreate,
    /// An environment that leaves its host, or arrives on another.
    Move,
    /// An environment or a provider's runtime that is removed.
    Delete,
}

impl UnitReason {
    /// The reason as it is written.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Start => "start",
            Self::Stop => "stop",
            Self::Recreate => "recreate",
            Self::Move => "move",
            Self::Delete => "delete",
        }
    }
}

/// What a unit serves, as the node's operator hooks are told it: names and identifiers, never a secret or
/// anything of what is worked on.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct UnitSubject {
    /// Why the unit was made: for a new environment or runtime (`start`), as the successor of another
    /// (`recreate`), or on the host an environment moves to (`move`). Its first start is told this reason.
    pub made_for: UnitReason,
    /// The environment, for an environment's unit.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<SubjectEnvironment>,
    /// The provider, for a provider's runtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<SubjectProvider>,
}

/// The environment a unit serves, as it was when the unit was made.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectEnvironment {
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// The names of the places from its root org down to its home, joined by ` / `.
    pub path: String,
    /// The template the unit was made from; `null` for none.
    pub template_id: Option<Id>,
    /// That template's name; empty for none.
    pub template: String,
    /// The environment's generation the unit belongs to.
    pub generation: i64,
}

/// The provider a provider's runtime serves.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SubjectProvider {
    /// Its identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
}

/// The name of the unit of runtime `runtime_id`: `orch-<kind>-<last 12 hex digits of the id>`.
///
/// The last digits, because an identifier begins with the moment it was made:
/// its first twelve digits are that moment to the millisecond, the same for
/// every runtime made in it, and its last twelve are the part that differs.
#[must_use]
pub fn unit_name(kind: RuntimeKind, runtime_id: &Id) -> String {
    let simple = runtime_id.simple().to_string();
    let digits = &simple[simple.len().saturating_sub(NAME_ID_DIGITS)..];
    format!("orch-{}-{digits}", kind.as_str())
}

/// The labels every unit carries: the system, the runtime and the pool.
#[must_use]
pub fn unit_labels(system: &str, kind: RuntimeKind, runtime_id: &Id) -> BTreeMap<String, String> {
    BTreeMap::from([
        (LABEL_SYSTEM.to_owned(), system.to_owned()),
        (LABEL_RUNTIME_ID.to_owned(), runtime_id.to_string()),
        (LABEL_KIND.to_owned(), kind.as_str().to_owned()),
    ])
}

/// What a variable of the launch inputs holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LaunchValue {
    /// The text itself.
    Text,
    /// The path of a file the driver makes.
    File,
    /// The path of a directory.
    Directory,
}

/// One variable that every driver sets for the connector's process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LaunchVariable {
    /// The name.
    pub name: &'static str,
    /// What it holds.
    pub value: LaunchValue,
    /// The pool it is for; `None` for both.
    pub only_for: Option<RuntimeKind>,
    /// What the driver puts there, in a sentence.
    pub meaning: &'static str,
}

/// The variables every driver sets for `orch-connector run --managed`.
pub const LAUNCH_VARIABLES: [LaunchVariable; 9] = [
    LaunchVariable {
        name: "ORCH_ROLES",
        value: LaunchValue::Text,
        only_for: None,
        meaning: "the roles, comma-separated",
    },
    LaunchVariable {
        name: "ORCH_GATEWAY_URL",
        value: LaunchValue::Text,
        only_for: None,
        meaning: "where the connector reaches the gateway",
    },
    LaunchVariable {
        name: "ORCH_CA_FILE",
        value: LaunchValue::File,
        only_for: None,
        meaning: "a file holding the certificate authority",
    },
    LaunchVariable {
        name: "ORCH_ENROLL_FILE",
        value: LaunchValue::File,
        only_for: None,
        meaning: "a file holding the enrollment token, readable only by the connector's user, on storage that does not outlive the unit's current run; used at the first start only",
    },
    LaunchVariable {
        name: "ORCH_DATA_DIR",
        value: LaunchValue::Directory,
        only_for: None,
        meaning: "a directory on the state volume",
    },
    LaunchVariable {
        name: "ORCH_TOOLS_DIR",
        value: LaunchValue::Directory,
        only_for: None,
        meaning: "a read-only directory with a directory for each tool, from the node's tool cache",
    },
    LaunchVariable {
        name: "ORCH_WORKSPACE_DIR",
        value: LaunchValue::Directory,
        only_for: Some(RuntimeKind::Environment),
        meaning: "the workspace volume",
    },
    LaunchVariable {
        name: "ORCH_HOME_DIR",
        value: LaunchValue::Directory,
        only_for: Some(RuntimeKind::Environment),
        meaning: "the home volume",
    },
    LaunchVariable {
        name: "ORCH_MOUNTS_FILE",
        value: LaunchValue::File,
        only_for: Some(RuntimeKind::Environment),
        meaning: "a file listing each mount the driver attached: its name, its path inside the unit and the mode it got",
    },
];

/// What a spec fixes of the launch inputs: the values and the file contents that need no driver, and the
/// names the driver has to supply paths for.
#[derive(Debug, Clone, PartialEq)]
pub struct Launch {
    /// The variables whose value the spec states: `ORCH_ROLES`, `ORCH_GATEWAY_URL`, and each member of
    /// `env`.
    pub values: BTreeMap<String, String>,
    /// The text of the file `ORCH_CA_FILE` names.
    pub ca_file: String,
    /// The text of the file `ORCH_ENROLL_FILE` names.
    pub enroll_file: Secret,
    /// The variables whose value is a path the driver chooses, for this pool.
    pub paths: Vec<LaunchVariable>,
}

/// What is wrong with a runtime spec.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SpecFault {
    /// The name is not the one the runtime's identifier gives.
    #[error("the unit is named {found:?} and must be named {expected:?}")]
    WrongName {
        /// What the spec says.
        found: String,
        /// What it must be.
        expected: String,
    },
    /// The connector plays no role the pool needs.
    #[error("a runtime of kind {0} needs its connector to play the role {0}")]
    MissingRole(&'static str),
    /// A provider's runtime is given something that belongs to an environment.
    #[error(
        "a provider's runtime has private state, an optional harness home, and no workspace or mounts"
    )]
    ProviderWithWork,
    /// An environment lacks a volume.
    #[error("an environment has the volumes workspace, home and state")]
    EnvironmentVolumes,
    /// Only workspace/home may borrow an absolute path; connector state always stays private.
    #[error("a borrowed directory must be an absolute workspace/home path, never connector state")]
    DataDirectory,
    /// A label the system finds its units by is wrong or missing.
    #[error("the label {0} is missing or does not match the runtime")]
    Label(&'static str),
    /// A variable of the spec's `env` is one the launch sets itself.
    #[error("the variable {0} is set by the launch and cannot be given in env")]
    ReservedVariable(String),
}

impl RuntimeSpec {
    /// Checks that the spec is the shape every driver is promised: the name is the one the identifier
    /// gives; the connector plays the role its pool needs; a provider's runtime has private state and
    /// optional home, an environment all three volumes; the labels name the system, the runtime and
    /// the pool; and `env` sets none of the variables the launch sets. Every fault is returned.
    pub fn check(&self) -> Result<(), Vec<SpecFault>> {
        let mut faults = Vec::new();
        for (role, volume) in &self.volumes {
            if let Some(directory) = &volume.directory
                && (*role == VolumeRole::State
                    || !std::path::Path::new(&directory.path).is_absolute())
            {
                faults.push(SpecFault::DataDirectory);
            }
        }
        let expected = unit_name(self.kind, &self.runtime_id);
        if self.name != expected {
            faults.push(SpecFault::WrongName {
                found: self.name.clone(),
                expected,
            });
        }
        let needs = match self.kind {
            RuntimeKind::Environment => ConnectorRole::Environment,
            RuntimeKind::Provider => ConnectorRole::Provider,
        };
        if !self.roles.contains(&needs) {
            faults.push(SpecFault::MissingRole(needs.as_str()));
        }
        match self.kind {
            RuntimeKind::Provider => {
                if !self.volumes.contains_key(&VolumeRole::State)
                    || self.volumes.contains_key(&VolumeRole::Workspace)
                    || !self.mounts.is_empty()
                {
                    faults.push(SpecFault::ProviderWithWork);
                }
            }
            RuntimeKind::Environment => {
                if ![VolumeRole::Workspace, VolumeRole::Home, VolumeRole::State]
                    .iter()
                    .all(|role| self.volumes.contains_key(role))
                {
                    faults.push(SpecFault::EnvironmentVolumes);
                }
            }
        }
        if let Some(source) = &self.harness_home {
            let binding = self.volumes.get(&VolumeRole::Home);
            if self.kind != RuntimeKind::Provider
                || binding.is_none()
                || source.directory().is_some()
                    != binding.is_some_and(|volume| volume.directory.is_some())
            {
                faults.push(SpecFault::DataDirectory);
            }
        }
        if self.labels.get(LABEL_SYSTEM).is_none_or(String::is_empty) {
            faults.push(SpecFault::Label(LABEL_SYSTEM));
        }
        if self.labels.get(LABEL_RUNTIME_ID) != Some(&self.runtime_id.to_string()) {
            faults.push(SpecFault::Label(LABEL_RUNTIME_ID));
        }
        if self.labels.get(LABEL_KIND).map(String::as_str) != Some(self.kind.as_str()) {
            faults.push(SpecFault::Label(LABEL_KIND));
        }
        for name in self.env.keys() {
            if matches!(
                name.as_str(),
                "ORCH_HARNESS_HOME" | "ORCH_HARNESS_HOME_SOURCE"
            ) || LAUNCH_VARIABLES
                .iter()
                .any(|variable| variable.name == name)
            {
                faults.push(SpecFault::ReservedVariable(name.clone()));
            }
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }

    /// What the spec fixes of the launch inputs.
    #[must_use]
    pub fn launch(&self) -> Launch {
        let roles: Vec<&str> = self.roles.iter().map(|role| role.as_str()).collect();
        let mut values = self.env.clone();
        values.insert("ORCH_ROLES".to_owned(), roles.join(","));
        values.insert("ORCH_GATEWAY_URL".to_owned(), self.gateway_url.clone());
        Launch {
            values,
            ca_file: self.ca_cert.clone(),
            enroll_file: self.enroll_token.clone(),
            paths: LAUNCH_VARIABLES
                .into_iter()
                .filter(|variable| variable.value != LaunchValue::Text)
                .filter(|variable| variable.only_for.is_none_or(|kind| kind == self.kind))
                .collect(),
        }
    }
}

/// The text of the file `ORCH_MOUNTS_FILE` names: the mounts the driver attached, as a JSON array of their
/// names, the paths they have inside the unit and the modes they got.
/// Each entry is exactly `{"name":"source","path":"/data/source","mode":"read_only"}`;
/// modes are `read_only`, `shared` or `private`. Paths are absolute inside the unit,
/// and an empty attachment set writes `[]`. The variable names the file, not its JSON.
#[must_use]
pub fn mounts_file(attached: &[AttachedMount]) -> String {
    serde_json::to_string(attached).unwrap_or_else(|_| "[]".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mounts::MountMode;
    use serde_json::json;

    const RUNTIME: &str = "01990000-0000-7000-8000-0199abcd1234";

    /// The runtime spec of an environment, as it is written out.
    fn environment() -> Json {
        json!({
          "runtime_id": RUNTIME, "kind": "environment", "name": "orch-environment-0199abcd1234", "driver": "docker",
          "spec": { "image": "ghcr.io/OWNER/orch-env-base:0.1.0" },
          "resources": { "cpu_millis": 4000, "memory_mb": 8192, "pids": 4096, "disk_gb": 50 },
          "roles": ["environment"],
          "gateway_url": "wss://gateway.orch.internal:9443",
          "network_policy": {"mode":"open","allowed_hosts":[],"always":[],"always_revision":"","revision":""},
          "enroll_token": "enroll-123",
          "ca_cert": "-----BEGIN CERTIFICATE-----\nMIIB\n-----END CERTIFICATE-----\n",
          "tools": [ { "name": "codex", "version": "0.160.1" } ],
          "volumes": { "workspace": { "name": "orch-ws-0199abcd-1234-7000-8000-0000000000e1", "storage": "fast" },
                       "home": { "name": "orch-home-0199abcd-1234-7000-8000-0000000000e1", "storage": null },
                       "state": { "name": "orch-state-0199abcd-1234-7000-8000-0000000000e1", "storage": null } },
          "mounts": [ { "name": "src", "kind": "path", "source": "/srv/src", "target": "/data/src", "mode": "private",
                        "layer": { "name": "orch-over-e1-m1", "storage": "fast" } } ],
          "env": { "NAME": "value" },
          "labels": { "orch.system": "prod", "orch.runtime_id": RUNTIME, "orch.kind": "environment" }
        })
    }

    #[test]
    fn the_request_to_make_a_unit_reads_and_writes_as_the_spec_shows_it() {
        let spec: RuntimeSpec = serde_json::from_value(environment()).unwrap();
        assert_eq!(spec.kind, RuntimeKind::Environment);
        assert_eq!(spec.roles, [ConnectorRole::Environment]);
        assert_eq!(
            spec.volumes[&VolumeRole::Workspace].storage.as_deref(),
            Some("fast")
        );
        assert_eq!(spec.mounts[0].mode, MountMode::Private);
        assert_eq!(spec.check(), Ok(()));
        assert_eq!(serde_json::to_value(&spec).unwrap(), environment());
        assert!(
            !format!("{spec:?}").contains("enroll-123"),
            "the enrollment token is never printed"
        );
    }

    #[test]
    fn two_runtimes_made_in_the_same_millisecond_are_named_differently() {
        let first: Id = "01990000-0000-7000-8000-0199abcd1234".parse().unwrap();
        let second: Id = "01990000-0000-7000-8000-0199abcd1235".parse().unwrap();
        assert_ne!(
            unit_name(RuntimeKind::Environment, &first),
            unit_name(RuntimeKind::Environment, &second)
        );
        let made_now = orch_types::new_id();
        let made_next = orch_types::new_id();
        assert_ne!(
            unit_name(RuntimeKind::Provider, &made_now),
            unit_name(RuntimeKind::Provider, &made_next)
        );
    }

    #[test]
    fn the_unit_is_named_after_its_pool_and_the_last_twelve_digits_of_its_identifier() {
        let id: Id = RUNTIME.parse().unwrap();
        assert_eq!(
            unit_name(RuntimeKind::Environment, &id),
            "orch-environment-0199abcd1234"
        );
        assert_eq!(
            unit_name(RuntimeKind::Provider, &id),
            "orch-provider-0199abcd1234"
        );
        let labels = unit_labels("prod", RuntimeKind::Provider, &id);
        assert_eq!(labels[LABEL_SYSTEM], "prod");
        assert_eq!(labels[LABEL_RUNTIME_ID], RUNTIME);
        assert_eq!(labels[LABEL_KIND], "provider");
    }

    #[test]
    fn a_spec_that_breaks_the_promise_to_every_driver_is_refused_for_each_reason() {
        let mut broken: RuntimeSpec = serde_json::from_value(environment()).unwrap();
        broken.name = "my-unit".into();
        broken.roles = vec![ConnectorRole::Agent];
        broken.volumes.remove(&VolumeRole::Home);
        broken.labels.remove(LABEL_SYSTEM);
        broken.labels.insert(LABEL_KIND.into(), "provider".into());
        broken.env.insert("ORCH_ROLES".into(), "agent".into());
        let faults = broken.check().unwrap_err();
        assert!(matches!(faults[0], SpecFault::WrongName { .. }));
        for expected in [
            SpecFault::MissingRole("environment"),
            SpecFault::EnvironmentVolumes,
            SpecFault::Label(LABEL_SYSTEM),
            SpecFault::Label(LABEL_KIND),
            SpecFault::ReservedVariable("ORCH_ROLES".into()),
        ] {
            assert!(faults.contains(&expected), "{expected:?} in {faults:?}");
        }
    }

    #[test]
    fn a_providers_runtime_has_private_state_and_no_environment_workspace() {
        let id: Id = RUNTIME.parse().unwrap();
        let mut spec: RuntimeSpec = serde_json::from_value(environment()).unwrap();
        spec.kind = RuntimeKind::Provider;
        spec.name = unit_name(RuntimeKind::Provider, &id);
        spec.roles = vec![ConnectorRole::Provider, ConnectorRole::Agent];
        spec.labels.insert(LABEL_KIND.into(), "provider".into());
        assert_eq!(spec.check(), Err(vec![SpecFault::ProviderWithWork]));
        spec.mounts.clear();
        spec.volumes.retain(|role, _| *role == VolumeRole::State);
        assert_eq!(spec.check(), Ok(()));
        let launch = spec.launch();
        let names: Vec<_> = launch.paths.iter().map(|variable| variable.name).collect();
        assert_eq!(
            names,
            [
                "ORCH_CA_FILE",
                "ORCH_ENROLL_FILE",
                "ORCH_DATA_DIR",
                "ORCH_TOOLS_DIR"
            ]
        );
        assert_eq!(launch.values["ORCH_ROLES"], "provider,agent");
    }

    #[test]
    fn a_provider_can_bind_a_harness_home_but_its_private_state_is_never_borrowed() {
        let mut spec: RuntimeSpec = serde_json::from_value(environment()).unwrap();
        spec.kind = RuntimeKind::Provider;
        spec.name = unit_name(spec.kind, &spec.runtime_id);
        spec.roles = vec![ConnectorRole::Provider, ConnectorRole::Agent];
        spec.mounts.clear();
        spec.labels.insert(LABEL_KIND.into(), "provider".into());
        spec.volumes.remove(&VolumeRole::Workspace);
        spec.harness_home = Some(EnvironmentDataSource::ExistingDirectory {
            path: "/srv/harness".into(),
        });
        spec.volumes.get_mut(&VolumeRole::Home).unwrap().directory = Some(DirectoryBinding {
            path: "/srv/harness".into(),
            device: Some(8),
            inode: Some(90),
        });
        assert!(spec.check().is_ok());
        spec.volumes.get_mut(&VolumeRole::State).unwrap().directory =
            spec.volumes[&VolumeRole::Home].directory.clone();
        assert!(
            spec.check()
                .unwrap_err()
                .contains(&SpecFault::DataDirectory)
        );
        spec.volumes.get_mut(&VolumeRole::State).unwrap().directory = None;
        spec.env
            .insert("ORCH_HARNESS_HOME".into(), "/unchecked".into());
        assert!(
            spec.check()
                .unwrap_err()
                .contains(&SpecFault::ReservedVariable("ORCH_HARNESS_HOME".into()))
        );
    }

    #[test]
    fn what_the_spec_fixes_of_the_launch_is_the_roles_the_gateway_the_files_and_the_env() {
        let spec: RuntimeSpec = serde_json::from_value(environment()).unwrap();
        let launch = spec.launch();
        assert_eq!(launch.values["ORCH_ROLES"], "environment");
        assert_eq!(
            launch.values["ORCH_GATEWAY_URL"],
            "wss://gateway.orch.internal:9443"
        );
        assert_eq!(launch.values["NAME"], "value");
        assert!(launch.ca_file.starts_with("-----BEGIN CERTIFICATE-----"));
        assert_eq!(launch.enroll_file.expose(), "enroll-123");
        let names: Vec<_> = launch.paths.iter().map(|variable| variable.name).collect();
        assert_eq!(
            names,
            [
                "ORCH_CA_FILE",
                "ORCH_ENROLL_FILE",
                "ORCH_DATA_DIR",
                "ORCH_TOOLS_DIR",
                "ORCH_WORKSPACE_DIR",
                "ORCH_HOME_DIR",
                "ORCH_MOUNTS_FILE"
            ]
        );
    }

    #[test]
    fn the_file_of_attached_mounts_names_each_with_its_path_and_the_mode_it_got() {
        let text = mounts_file(&[AttachedMount {
            name: "src".into(),
            path: "/data/src".into(),
            mode: MountMode::ReadOnly,
        }]);
        assert_eq!(
            serde_json::from_str::<Json>(&text).unwrap(),
            json!([ { "name": "src", "path": "/data/src", "mode": "read_only" } ])
        );
        assert_eq!(mounts_file(&[]), "[]");
        let attached = [MountMode::ReadOnly, MountMode::Shared, MountMode::Private].map(|mode| {
            AttachedMount {
                name: "媒体".into(),
                path: "/data/媒体".into(),
                mode,
            }
        });
        assert_eq!(
            serde_json::from_str::<Json>(&mounts_file(&attached)).unwrap(),
            json!([
                { "name":"媒体","path":"/data/媒体","mode":"read_only" },
                { "name":"媒体","path":"/data/媒体","mode":"shared" },
                { "name":"媒体","path":"/data/媒体","mode":"private" }
            ])
        );
    }

    #[test]
    fn limits_are_within_the_most_they_may_be_raised_to() {
        let limits = Resources {
            cpu_millis: Some(4000),
            memory_mb: Some(8192),
            pids: None,
            disk_gb: Some(50),
            ..Resources::default()
        };
        let most = Resources {
            cpu_millis: Some(16_000),
            memory_mb: Some(8192),
            pids: Some(10),
            disk_gb: None,
            ..Resources::default()
        };
        assert!(limits.within(&most));
        assert!(
            !Resources {
                memory_mb: Some(8193),
                ..limits
            }
            .within(&most)
        );
        assert!(Resources::default().within(&most));
    }
}
