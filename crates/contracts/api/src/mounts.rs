//! Mounts, their locations, and how each is attached to an environment: read-only, shared or as a private view.
//!
//! A mount is data that environments attach beside their workspace: a path on
//! a machine, or a volume on a host's storage. Its [`MountLocation`]s say where
//! it is on each machine. Whether an environment attaches it, and how, is set
//! by the scopes that enable it (the setting `env.mounts`), not here; each
//! environment reports what became of every mount it was given as an
//! [`EnvMount`].

use orch_types::{Id, Revision, Time};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::access::{NewResourceAccess, ResourceAccess};
use crate::environment::EnvironmentRef;
use crate::nodes::NodeRef;
use crate::resources::{HostRef, MachineRef};
use crate::routes::Access::{Permission, Rule, User};
use crate::users::UserRef;

/// A mount, named wherever another thing points at one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountRef {
    /// The mount's identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// Where it comes from, as short names.
    pub address: String,
}

/// What a mount is on a machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountLocationKind {
    /// A directory that already exists.
    Path,
    /// A volume that the host keeps on one of its storages.
    Volume,
}

/// What becomes of a mount when an environment goes to another machine: what a move, a fork or a switch plan
/// does with it, and what its manager has a plan preselect.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountFate {
    /// Mounted as it is where the target machine has a location for it, and detached where it has none.
    #[default]
    AsIs,
    /// Not attached on the target, even where it has a location there.
    Detach,
    /// Its data copied to the target machine's location for it, and attached there.
    Copy,
}

/// What a location's data is, as the mount's managers describe it. A hint shown beside the location; nothing is
/// decided by it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountDataKind {
    /// Own data: the data itself lives here.
    Own,
    /// Shared storage: others write the same files.
    Shared,
    /// Synced copy: a copy kept in step with another place.
    Synced,
}

/// Where a mount is on one machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountLocation {
    /// Bytes reported for this exact machine, source and storage; absent when not measured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub measured_bytes: Option<i64>,
    /// The machine.
    pub machine: MachineRef,
    /// The host that machine is, when it is one.
    pub host: Option<HostRef>,
    /// A path or a volume.
    pub kind: MountLocationKind,
    /// For a volume, the storage of the host it is on; `null` is the host's default.
    pub storage: Option<String>,
    /// The path, or the name of the volume; `null` unless the caller may change this location.
    pub source: Option<String>,
    /// What its data is, as the mount's managers describe it; `null` when nobody said.
    pub data_kind: Option<MountDataKind>,
    /// Whether the caller may remove this location.
    pub can_remove: bool,
    /// Who added it.
    pub created_by: UserRef,
    /// When it was added.
    pub created_at: Time,
}

/// Data a copy displaced from a machine's location of a mount: kept where it is, attached nowhere, until somebody
/// purges it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountKeptData {
    /// Its identifier, which a purge names.
    pub id: Id,
    /// The machine it is on.
    pub machine: MachineRef,
    /// The host that machine is, when it is one.
    pub host: Option<HostRef>,
    /// A directory or a volume.
    pub kind: MountLocationKind,
    /// For a volume, the storage of the host it is on; `null` is the host's default.
    pub storage: Option<String>,
    /// The directory, or the volume's name; `null` unless the caller may purge it.
    pub source: Option<String>,
    /// What its data was, as the mount's managers said.
    pub data_kind: Option<MountDataKind>,
    /// How many files it held when it was kept; `null` when not measured.
    pub file_count: Option<i64>,
    /// How many bytes.
    pub total_bytes: Option<i64>,
    /// Who confirmed the copy that displaced it.
    pub kept_by: UserRef,
    /// When it was kept.
    pub kept_at: Time,
    /// Whether the caller may purge it: whoever may remove that machine's location of the mount.
    pub can_purge: bool,
}

/// Data that environments attach beside their workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Mount {
    /// Former volume locations on machines declared lost or deleted, never attempted again.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lost_locations: Vec<MountLostLocation>,
    /// Environments with a current reported attachment, without disclosing their names.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attached_environments: Option<i64>,
    /// How a mount is named wherever it is pointed at.
    #[serde(flatten)]
    pub base: MountRef,
    /// What it is for.
    pub description: String,
    /// The root org of its home; `null` for the system's.
    pub org_id: Option<Id>,
    /// The org it belongs to, at any level; `null` for the system's.
    pub home: Option<NodeRef>,
    /// Its first owner.
    pub owner: Option<UserRef>,
    /// Where the agent finds it inside an environment.
    pub target: String,
    /// Whether it is attached read-only.
    pub read_only: bool,
    /// Whether it may be attached at all.
    pub enabled: bool,
    /// What a plan preselects for it when an environment goes to another machine.
    pub default_fate: MountFate,
    /// Where it is on the machines the caller may see.
    pub locations: Vec<MountLocation>,
    /// Saved locations concealed because their machine/host is not readable; null until reported.
    /// A known zero with no visible locations means none is configured, not an offline host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locations_hidden: Option<usize>,
    /// Data copies displaced, on the machines the caller may see, until somebody purges it.
    pub kept: Vec<MountKeptData>,
    /// The revision, which a change names as `expected_revision`.
    pub revision: Revision,
    /// What the caller holds on it.
    pub access: ResourceAccess,
}

/// Metadata retained after a machine's owned mount volume became unreachable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountLostLocation {
    /// Its historical record.
    pub id: Id,
    /// The machine's safe name at loss.
    pub machine_name: String,
    /// The host's name at loss, when present.
    pub host_name: Option<String>,
    /// Its storage, where named.
    pub storage: Option<String>,
    /// The last measured bytes, or null when unknown.
    pub total_bytes: Option<i64>,
    /// When that size was measured.
    pub size_at: Option<Time>,
    /// When access to the location ended.
    pub lost_at: Time,
}

/// A readable environment whose last report says this mount is attached on the selected machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountEnvironment {
    /// Its environment page.
    pub id: Id,
    /// Its name, without unrelated owner or configuration details.
    pub name: String,
    /// Its current state, so a stored attachment is not mistaken for a running unit.
    pub status: crate::environment::EnvironmentStatus,
}

/// One readable mount located on the machine whose page is open.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineMount {
    /// The named data, which stays a resource independent of its machines.
    pub mount: MountRef,
    /// The common path at which environments find it.
    pub target: String,
    /// An existing directory or data Orch made on this machine (a volume or owned directory).
    pub kind: MountLocationKind,
    /// Its local path or volume name; null unless the caller may change this location.
    pub source: Option<String>,
    /// Its host storage; null for a directory or the default storage.
    pub storage: Option<String>,
    /// What its data is, as the mount's managers describe it; null when nobody said.
    pub data_kind: Option<MountDataKind>,
    /// The host's reported size for this exact location; null when unreported.
    pub size_bytes: Option<u64>,
    /// Readable environments on this machine that report this mount attached.
    pub environments: Vec<MountEnvironment>,
    /// Whether the caller may remove this location under the ordinary location rules.
    pub can_remove: bool,
}

/// A mount the caller manages and can give a new location on this machine.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineMountChoice {
    /// The manageable mount, not already located here.
    pub mount: MountRef,
    /// Whether the caller may expose a directory here.
    pub path: bool,
    /// Whether this machine's default storage permits data made by Orch.
    pub default_storage: bool,
    /// Reported named storages permitted by the machine's management and storage policy.
    pub storages: Vec<String>,
}

/// The host side of mounts, read without a probe or a separate query for each mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineMounts {
    /// Located mounts the caller may read, with only readable attached environments.
    pub locations: Vec<MachineMount>,
    /// Mounts the caller manages and may locate here; excludes existing locations.
    pub available: Vec<MachineMountChoice>,
}

/// Which independent environment data root uses a directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvironmentDataRole {
    /// Workspace files.
    Workspace,
    /// The environment user's home.
    Home,
}

/// A readable environment's use of one of the machine's allowed directories.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentDirectoryUse {
    /// Only environments the caller can read are named.
    pub environment: MountEnvironment,
    /// The data root.
    pub role: EnvironmentDataRole,
    /// Its selected path on this machine.
    pub path: String,
}

/// One entry in the machine's environment.roots policy, with visible users.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvironmentDirectoryRoot {
    /// The path its owners allow; empty policy means no existing directories are offered.
    pub path: String,
    /// Only currently placed, readable environments on this machine.
    pub uses: Vec<EnvironmentDirectoryUse>,
}

/// The same directory choices and usage projection on personal machines and managed hosts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MachineEnvironmentData {
    /// The machine, regardless of who places its environments.
    pub machine_id: Id,
    /// The existing machine policy, never a second host-only allowlist.
    pub roots: Vec<EnvironmentDirectoryRoot>,
    /// Facts of its local environment role, if offered.
    pub direct: Option<orch_driver_contract::descriptor::EnvironmentDataStorage>,
    /// Actual storage choices reported by each host driver.
    pub drivers: std::collections::BTreeMap<
        String,
        orch_driver_contract::descriptor::EnvironmentDataStorage,
    >,
}

/// What became of a mount in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvMountState {
    /// It is attached.
    Attached,
    /// It could not be attached.
    Unavailable,
    /// It is not created or attached yet.
    Pending,
}

/// Why a mount is not attached in an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum EnvMountReason {
    /// It has no location on the machine the environment is on.
    NoLocation,
    /// The environment's driver cannot attach it.
    Driver,
    /// What the location names is not there.
    Missing,
    /// The machine refused it.
    Refused,
    /// Another mount, or the workspace, is at the same path.
    TargetConflict,
}

/// How a mount is attached to an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountMode {
    /// Read-only.
    ReadOnly,
    /// Writable, and shared with every other environment that attaches it.
    Shared,
    /// Read-only underneath, with the environment's own changes kept apart on top.
    Private,
}

/// A mount as one environment has it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct EnvMount {
    /// The mount.
    pub mount: MountRef,
    /// What became of it.
    pub state: EnvMountState,
    /// Why it is not attached, when it is not.
    pub reason: Option<EnvMountReason>,
    /// Where the agent finds it.
    pub path: Option<String>,
    /// How it is attached.
    pub mode: MountMode,
    /// What was asked for; it differs when a private view is not possible there and the mount
    /// fell back to read-only.
    pub asked: MountMode,
    /// False when only the harness's sandbox or file permissions keep it read-only.
    pub enforced: bool,
    /// The scope whose `env.mounts` enables it; `null` is the environment itself.
    pub from: Option<NodeRef>,
}

/// The mounts the caller may use or manage.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountListQuery {
    /// Only the mounts whose home is that org or an org inside it, with the system's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub org_id: Option<Id>,
}

/// Makes a mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountCreateBody {
    /// Its name.
    pub name: String,
    /// What it is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Its home: an org at any level; `null` for the system's.
    pub home_id: Option<Id>,
    /// Where the agent finds it inside an environment.
    pub target: String,
    /// Whether it is attached read-only; no when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// What a plan preselects for it; mounted as it is when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_fate: Option<MountFate>,
    /// Who may use it; as the defaults of its home say when left out.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub access: Option<NewResourceAccess>,
}

/// Changes a mount. A change to the target, to `read_only` or to a location reaches an
/// environment at once where the mount is in place, and at its next recreation where it is placed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountUpdateBody {
    /// A new name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// A new description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// A new target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<String>,
    /// Whether it is attached read-only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only: Option<bool>,
    /// Whether it may be attached at all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// What a plan preselects for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_fate: Option<MountFate>,
    /// The revision the caller saw.
    pub expected_revision: Revision,
}

/// Says what a location's data is. Only the hint changes: where the data is stays as it is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountLocationUpdateBody {
    /// What its data is; `null` takes the hint away.
    pub data_kind: Option<MountDataKind>,
}

/// Where a mount is on a machine: a directory that exists, or a volume to be kept on a host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MountLocationSetBody {
    /// A directory. On a person's machine it has to lie inside what that machine's own policy
    /// allows as environment roots, which the machine checks itself.
    Path {
        /// The directory.
        source: String,
    },
    /// A volume on a host.
    Volume {
        /// One of that host's storages; its default when left out.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        storage: Option<String>,
    },
}

/// Which location to remove.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MountLocationDeleteQuery {
    /// The mount's name, which removing a volume asks for because its data is deleted with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confirm: Option<String>,
}

/// A mode other than the plain shared one, in which a scope enables a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountScopeMode {
    /// Read-only.
    ReadOnly,
    /// With a private view for each environment.
    Private,
}

/// How a scope enables a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum MountScopeValue {
    /// Writable and shared; always `true`.
    Enabled(bool),
    /// In one of the other modes.
    Mode(MountScopeMode),
}

/// A place whose settings enable a mount.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountScope {
    /// The place.
    pub node: NodeRef,
    /// How it enables the mount.
    pub value: MountScopeValue,
}

/// An environment that has a mount attached, or has it as its workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountUse {
    /// The environment.
    pub environment: EnvironmentRef,
    /// What became of the mount there.
    pub state: String,
    /// Whether the mount is the environment's workspace.
    pub workspace: bool,
}

/// Where a mount is enabled and attached, and where it is somebody's workspace, as far as the
/// caller may see.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountUsesGetReply {
    /// The places that enable it.
    pub scopes: Vec<MountScope>,
    /// The environments it is attached to.
    pub environments: Vec<MountUse>,
    /// How many more there are than the caller may see.
    pub hidden: i64,
}

crate::endpoints! {
    /// Mounts located on a readable host or the caller's own machine, and manageable add choices.
    MachineMountList: GET "/machines/{id}/mounts", Rule("the machine's owner, or host.view on its host; each mount and attached environment is independently readable"), none, none, json MachineMounts, 200;
    /// Lists configured workspace roots and environment data on a machine.
    MachineEnvironmentDataGet: GET "/machines/{id}/environment-data", Rule("the machine's owner or host.view/host.use on its host; only readable environment uses are named; roots are changed only through the machine's own environment.roots policy"), none, none, json MachineEnvironmentData, 200;
    /// Lists the mounts the caller may use or manage, of whatever org.
    MountList: GET "/mounts", User, query MountListQuery, none, json Vec<crate::common::Listed<Mount>>, 200;
    /// Makes a mount, the caller's own, without locations.
    MountCreate: POST "/mounts", Rule("its home is the caller's domain or an org inside it and the members of that home may make mounts there; else settings.manage on that org; a system admin when it has no home"), none, json MountCreateBody, json Mount, 200;
    /// Reads a mount.
    MountGet: GET "/mounts/{id}", Rule("any permission on the mount"), none, none, json Mount, 200;
    /// Changes a mount.
    MountUpdate: PATCH "/mounts/{id}", Permission("mount.manage"), none, json MountUpdateBody, json Mount, 200;
    /// Deletes a mount that no scope enables.
    MountDelete: DELETE "/mounts/{id}", Permission("mount.manage"), none, none, none, 204;
    /// Sets where a mount is on a machine: a path, or a volume on a host's storage.
    MountLocationSet: PUT "/mounts/{id}/locations/{machine_id}", Rule("management of the machine and mount.use; environment.roots bounds existing paths on every machine; owned data requires the machine's declared data-making support"), none, json MountLocationSetBody, json Mount, 200;
    /// Says what the data of a mount's location on a machine is: a hint shown beside it, which decides nothing.
    MountLocationUpdate: PATCH "/mounts/{id}/locations/{machine_id}", Permission("mount.manage"), none, json MountLocationUpdateBody, json Mount, 200;
    /// Purges data a copy displaced: a volume is removed with its data, a directory is only forgotten.
    MountKeptPurge: DELETE "/mounts/{id}/kept/{kept_id}", Rule("whoever may remove that machine's location of the mount"), query MountLocationDeleteQuery, none, none, 204;
    /// Removes the place a mount has on a machine; a volume's data goes with it.
    MountLocationDelete: DELETE "/mounts/{id}/locations/{machine_id}", Rule("whoever may add that location"), query MountLocationDeleteQuery, none, none, 204;
    /// Lists where a mount is enabled and attached, and where it is somebody's workspace.
    MountUsesGet: GET "/mounts/{id}/uses", Rule("any permission on the mount"), none, none, json MountUsesGetReply, 200;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_mount_shows_its_locations_and_hides_a_source_the_caller_may_not_change() {
        let written = json!({
            "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "name": "datasets", "address": "acme.research.datasets",
            "description": "Training data", "org_id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c",
            "home": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "kind": "org", "name": "Research", "redacted": false },
            "owner": null, "target": "/mnt/datasets", "read_only": true, "enabled": true, "default_fate": "as_is",
            "locations": [{
                "machine": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6d", "name": "build-1", "online": true, "platform_os": "linux", "placement": "managed" },
                "host": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6e", "name": "build-1", "address": "build-1" },
                "kind": "volume", "storage": null, "source": null, "data_kind": "synced", "can_remove": false,
                "created_by": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6f", "kind": "human", "username": "bo", "display_name": "Bo", "avatar": { "text": "B", "color": 2 } },
                "created_at": "2026-10-08T17:04:05.123Z"
            }],
            "kept": [],
            "revision": 3,
            "access": { "permissions": ["mount.use"], "administers": false, "through": [{ "kind": "owner" }] }
        });
        let mount: Mount = serde_json::from_value(written.clone()).unwrap();
        assert_eq!(mount.base.name, "datasets");
        assert_eq!(mount.locations[0].kind, MountLocationKind::Volume);
        assert_eq!(mount.locations[0].source, None);
        assert_eq!(mount.locations[0].data_kind, Some(MountDataKind::Synced));
        assert_eq!(mount.default_fate, MountFate::AsIs);
        assert_eq!(serde_json::to_value(&mount).unwrap(), written);
    }

    #[test]
    fn a_manager_sets_the_default_fate_and_a_location_hint_that_can_be_taken_away() {
        let update: MountUpdateBody =
            serde_json::from_value(json!({ "default_fate": "copy", "expected_revision": 4 }))
                .unwrap();
        assert_eq!(update.default_fate, Some(MountFate::Copy));
        assert!(
            serde_json::from_value::<MountUpdateBody>(
                json!({ "default_fate": "move", "expected_revision": 4 })
            )
            .is_err(),
            "a fate is one of the three"
        );
        let hint: MountLocationUpdateBody =
            serde_json::from_value(json!({ "data_kind": "shared" })).unwrap();
        assert_eq!(hint.data_kind, Some(MountDataKind::Shared));
        let cleared: MountLocationUpdateBody =
            serde_json::from_value(json!({ "data_kind": null })).unwrap();
        assert_eq!(cleared.data_kind, None);
    }

    #[test]
    fn a_location_is_a_path_or_a_volume_and_nothing_else() {
        let path: MountLocationSetBody =
            serde_json::from_value(json!({ "kind": "path", "source": "/data/sets" })).unwrap();
        assert_eq!(
            path,
            MountLocationSetBody::Path {
                source: "/data/sets".into()
            }
        );
        let volume: MountLocationSetBody =
            serde_json::from_value(json!({ "kind": "volume" })).unwrap();
        assert_eq!(volume, MountLocationSetBody::Volume { storage: None });
        assert!(
            serde_json::from_value::<MountLocationSetBody>(json!({ "kind": "path" })).is_err(),
            "a path names its source"
        );
        assert!(
            serde_json::from_value::<MountLocationSetBody>(
                json!({ "kind": "volume", "source": "/data" })
            )
            .is_err(),
            "a volume has no source of its own"
        );
    }

    #[test]
    fn a_scope_enables_a_mount_as_it_is_or_in_a_mode() {
        let uses: MountUsesGetReply = serde_json::from_value(json!({
            "scopes": [
                { "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6b", "kind": "org", "name": "Research", "redacted": false }, "value": true },
                { "node": { "id": "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a6c", "kind": "project", "name": "Web", "redacted": false }, "value": "private" }
            ],
            "environments": [],
            "hidden": 2
        }))
        .unwrap();
        assert_eq!(uses.scopes[0].value, MountScopeValue::Enabled(true));
        assert_eq!(
            uses.scopes[1].value,
            MountScopeValue::Mode(MountScopeMode::Private)
        );
    }
}
