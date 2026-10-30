//! Persistent storage operations on a host's named storages.
//!
//! A unit's data lives in volumes that outlive the unit: an environment's workspace, its home, and the
//! `state` of its connector; a provider's runtime has `state` alone. A host often has more than one kind of
//! disk, so which disk a volume lands on is a choice and not an accident of where the driver keeps things.
//! A **storage** is a named place on a host where volumes are created ([`StorageInfo`]); the choice is the
//! name of one, or the host's default ([`resolve_storage`]). The names of the volumes Orch makes are fixed,
//! so that a template and a setting work wherever they are scheduled ([`names`]).
//!
//! An existing directory is never used as a storage for volumes. Existing data comes in as a mount.

use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// Counts from a streaming filesystem operation, independent of a controller's job representation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VolumeProgress {
    /// Completed regular files.
    pub files_done: i64,
    /// Logical or transferred bytes completed in this step.
    pub bytes_done: i64,
    /// Directories visited, when counted.
    pub directories_done: Option<i64>,
    /// Total files, when the operation already knows them.
    pub files_total: Option<i64>,
    /// Total bytes, when the operation already knows them.
    pub bytes_total: Option<i64>,
    /// What the current step is doing, without file names or secret data.
    pub message: Option<String>,
}

/// A plugin's live volume counts, fenced to the exact request that owns them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriverVolumeProgress {
    /// The fresh token supplied to one call; finished or cancelled calls accept no further counts.
    pub progress_id: orch_types::Id,
    /// The latest cumulative observation, without file names or credential data.
    pub progress: VolumeProgress,
}

/// The optional plugin event carrying typed live filesystem progress.
pub const VOLUME_PROGRESS_EVENT: &str = "driver.volume_progress";

/// A plugin has opened one request's input endpoint and can receive its bounded stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct DriverVolumeInputReady {
    /// The fresh token supplied to the import call, never a token from an earlier request.
    pub progress_id: orch_types::Id,
}

/// Opening an input endpoint is separate from observing transferred byte counts.
pub const VOLUME_INPUT_READY_EVENT: &str = "driver.volume_input_ready";

/// Cooperative cancellation and live counts for a driver's long filesystem work.
#[async_trait::async_trait]
pub trait VolumeWork: Send + Sync {
    /// An importer calls this after opening its endpoint and before reading from it.
    /// The node starts feeding only then, so a small stream cannot close before its reader opens.
    fn input_ready(&self) {}
    /// The durable command owning this helper, so its cleanup survives a node restart.
    fn command_id(&self) -> Option<orch_types::Id> {
        None
    }
    /// Keeps the latest counts; implementations bound how often they publish them.
    fn report(&self, progress: VolumeProgress);
    /// Whether the caller asked this operation to stop.
    fn is_cancelled(&self) -> bool;
    /// Resolves when the caller asks this operation to stop.
    async fn cancelled(&self);
    /// The controller's configured allowance for this operation and its helper.
    fn timeout_secs(&self) -> u64;
}

/// The name of the storage a host has when its operator declares none: where its driver keeps volumes
/// anyway.
pub const DEFAULT_STORAGE: &str = "default";

/// Directory attachments make data for these roles; connector identity stays machine-private.
pub const DIRECTORY_STORAGE_ROLES: &[&str] = &["workspace", "home", "mount"];

/// An environment-owned volume to measure or carry whole. A private selector names its layer, never its backing mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub enum OwnedVolumeSelector {
    /// All of the environment's home volume.
    Home,
    /// The environment's immutable file-state store under its home, independently of other home contents.
    FileStates,
    /// All of the owned layer of this mount, including whiteouts and opaque markers.
    Private(orch_types::Id),
}

impl JsonSchema for OwnedVolumeSelector {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "OwnedVolumeSelector".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({ "type": "string", "pattern": "^(home|file_states|private:[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12})$" })
    }
}

impl std::fmt::Display for OwnedVolumeSelector {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Home => out.write_str("home"),
            Self::FileStates => out.write_str("file_states"),
            Self::Private(mount) => write!(out, "private:{mount}"),
        }
    }
}

impl From<OwnedVolumeSelector> for String {
    fn from(selector: OwnedVolumeSelector) -> Self {
        selector.to_string()
    }
}

impl TryFrom<String> for OwnedVolumeSelector {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value == "home" {
            return Ok(Self::Home);
        }
        if value == "file_states" {
            return Ok(Self::FileStates);
        }
        let mount = value
            .strip_prefix("private:")
            .ok_or_else(|| "a volume selector is home or private:<mount id>".to_owned())?;
        mount
            .parse()
            .map(Self::Private)
            .map_err(|_| "a private selector names a mount id".to_owned())
    }
}

/// A measured whole volume. Logical file sizes are counted without opening file contents or following links.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VolumeMeasure {
    /// Regular files, including each name of a hard link.
    pub file_count: i64,
    /// Logical bytes in those files; metadata and deletion markers are preserved by the archive.
    pub total_bytes: i64,
    /// The complete store's measurements for a file_states selector; absent for an ordinary whole volume.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file_states: Option<FileStatesMeasure>,
}

/// Every persistent part of the environment-owned file-state store, excluding transient credentials.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct FileStatesMeasure {
    /// Saved root metadata records, including expired metadata.
    pub roots: i64,
    /// Owned immutable Git objects.
    pub objects: i64,
    /// Logical bytes of the persistent store parts that will be carried.
    pub total_bytes: i64,
    /// Restore, worktree and publication/materialization journals.
    pub journals: i64,
    /// Whether all store parts were measured under the supported format.
    pub complete: bool,
}

/// A request to make a volume, or to be sure it exists: the body of `node.volume.ensure`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct VolumeRequest {
    /// The volume's name.
    pub name: String,
    /// The storage of the host to make it on; `null` is the host's default.
    pub storage: Option<String>,
    /// Labels by which the system finds its own volumes.
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
}

/// The protected identity of an existing volume, read without creating or adopting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct VolumeIdentity {
    /// The labels actually recorded on the existing volume.
    pub labels: BTreeMap<String, String>,
}

/// A named place on a host where volumes are made, as the host reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct StorageInfo {
    /// Its name.
    pub name: String,
    /// Whether it is the host's default.
    pub default: bool,
    /// Its size in GB.
    pub capacity_gb: i64,
    /// How much of it is free, in GB.
    pub free_gb: i64,
    /// Whether private views can be made on it.
    pub private_views: bool,
}

/// Why a storage cannot be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum StorageFault {
    /// The host has no storage of that name; an environment that names it cannot be placed on this host.
    #[error("the host has no storage {0:?}")]
    Unknown(String),
    /// The host has storages and none is its default.
    #[error("the host has no default storage")]
    NoDefault,
}

/// The storage a volume is made on: the one named, or the host's default when none is named.
pub fn resolve_storage<'a>(
    requested: Option<&str>,
    storages: &'a [StorageInfo],
) -> Result<&'a StorageInfo, StorageFault> {
    match requested {
        Some(name) => storages
            .iter()
            .find(|storage| storage.name == name)
            .ok_or_else(|| StorageFault::Unknown(name.to_owned())),
        None => storages
            .iter()
            .find(|storage| storage.default)
            .ok_or(StorageFault::NoDefault),
    }
}

/// The storage list of a host that declares none: one, `default`.
#[must_use]
pub fn implicit_storages(capacity_gb: i64, free_gb: i64) -> Vec<StorageInfo> {
    vec![StorageInfo {
        name: DEFAULT_STORAGE.to_owned(),
        default: true,
        capacity_gb,
        free_gb,
        private_views: false,
    }]
}

/// The names of the volumes Orch makes. They are the same on every host.
pub mod names {
    use orch_types::Id;

    /// An environment's workspace.
    #[must_use]
    pub fn workspace(environment: &Id) -> String {
        format!("orch-ws-{environment}")
    }

    /// An environment's home.
    #[must_use]
    pub fn home(environment: &Id) -> String {
        format!("orch-home-{environment}")
    }

    /// An environment's connector state.
    #[must_use]
    pub fn environment_state(environment: &Id) -> String {
        format!("orch-state-{environment}")
    }

    /// A provider's runtime state: the connector's identity and every harness home.
    #[must_use]
    pub fn provider_state(provider: &Id) -> String {
        format!("orch-state-{provider}")
    }

    /// What one environment wrote into a private view of a mount.
    #[must_use]
    pub fn private_layer(environment: &Id, mount: &Id) -> String {
        format!("orch-over-{environment}-{mount}")
    }

    /// The volume of a mount's location on a host.
    #[must_use]
    pub fn mount(mount: &Id) -> String {
        format!("orch-mount-{mount}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::Id;

    #[test]
    fn a_whole_volume_selector_names_the_home_or_one_private_layer_and_no_backing_data() {
        let mount = orch_types::new_id();
        assert_eq!(
            serde_json::to_string(&OwnedVolumeSelector::Home).unwrap(),
            "\"home\""
        );
        let private = OwnedVolumeSelector::Private(mount);
        assert_eq!(
            serde_json::to_value(private).unwrap(),
            format!("private:{mount}")
        );
        assert_eq!(
            serde_json::from_value::<OwnedVolumeSelector>(serde_json::json!(format!(
                "private:{mount}"
            )))
            .unwrap(),
            private
        );
        for refused in [
            "workspace",
            "state",
            "mount:data",
            "private:../data",
            "home/../state",
        ] {
            assert!(
                serde_json::from_value::<OwnedVolumeSelector>(serde_json::json!(refused)).is_err()
            );
        }
    }

    fn storage(name: &str, default: bool) -> StorageInfo {
        StorageInfo {
            name: name.into(),
            default,
            capacity_gb: 1000,
            free_gb: 400,
            private_views: default,
        }
    }

    #[test]
    fn a_volume_lands_on_the_storage_named_or_on_the_hosts_default() {
        let storages = [storage("fast", true), storage("bulk", false)];
        assert_eq!(
            resolve_storage(Some("bulk"), &storages).unwrap().name,
            "bulk"
        );
        assert_eq!(resolve_storage(None, &storages).unwrap().name, "fast");
        assert_eq!(
            resolve_storage(Some("slow"), &storages),
            Err(StorageFault::Unknown("slow".into())),
            "a host without the storage an environment names cannot take it"
        );
        assert_eq!(
            resolve_storage(None, &[storage("bulk", false)]),
            Err(StorageFault::NoDefault)
        );
    }

    #[test]
    fn a_host_that_declares_no_storage_has_one_called_default() {
        let storages = implicit_storages(500, 200);
        assert_eq!(
            resolve_storage(None, &storages).unwrap().name,
            DEFAULT_STORAGE
        );
        assert_eq!(
            resolve_storage(Some("default"), &storages).unwrap().free_gb,
            200
        );
        assert!(!storages[0].private_views);
    }

    #[test]
    fn the_names_of_volumes_are_fixed_so_that_a_template_works_wherever_it_is_scheduled() {
        let environment: Id = "0199abcd-1234-7000-8000-0000000000e1".parse().unwrap();
        let mount: Id = "0199abcd-1234-7000-8000-0000000000a1".parse().unwrap();
        assert_eq!(
            names::workspace(&environment),
            "orch-ws-0199abcd-1234-7000-8000-0000000000e1"
        );
        assert_eq!(
            names::home(&environment),
            "orch-home-0199abcd-1234-7000-8000-0000000000e1"
        );
        assert_eq!(
            names::environment_state(&environment),
            "orch-state-0199abcd-1234-7000-8000-0000000000e1"
        );
        assert_eq!(
            names::provider_state(&mount),
            "orch-state-0199abcd-1234-7000-8000-0000000000a1"
        );
        assert_eq!(
            names::private_layer(&environment, &mount),
            "orch-over-0199abcd-1234-7000-8000-0000000000e1-0199abcd-1234-7000-8000-0000000000a1"
        );
        assert_eq!(
            names::mount(&mount),
            "orch-mount-0199abcd-1234-7000-8000-0000000000a1"
        );
    }

    #[test]
    fn a_request_for_a_volume_reads_as_the_node_command_writes_it() {
        let request: VolumeRequest = serde_json::from_value(serde_json::json!({
            "name": "orch-ws-1", "storage": null, "labels": { "orch.system": "prod" }
        }))
        .unwrap();
        assert_eq!(request.storage, None);
        assert_eq!(request.labels["orch.system"], "prod");
    }
}
