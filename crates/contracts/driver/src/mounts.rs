//! Mounts a driver attaches: placed, in place or not supported; read-only, shared or a private view.
//!
//! A mount is data that lives on a machine and belongs to no topic. A driver attaches it beside the
//! workspace in one of three ways, which it declares ([`MountPlacement`]): at the path it is given, where
//! the data already is, or not at all. How it is attached ([`MountMode`]) is read-only, shared, or
//! read-only with a private view of the environment's own changes on top. This module holds those words,
//! what a request to attach one looks like ([`MountSpec`]), what was done about it ([`MountFact`]), and the
//! two rules that need no driver: what a mode comes to where a private view cannot be made
//! ([`attach_mode`]), and which targets cannot be attached together ([`check_targets`]).

use orch_types::Id;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// How a driver attaches mounts, as its descriptor declares it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountPlacement {
    /// At the path it is given: a bind mount or a named volume at the mount's target.
    Placed,
    /// Where the data already is: nothing is created, and the agent sees the location's own path.
    InPlace,
    /// Not at all.
    None,
}

/// How a mount is attached to an environment.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountMode {
    /// Nobody writes.
    ReadOnly,
    /// Writable, and the very same files for every environment that attaches it so.
    Shared,
    /// Read-only underneath, with the environment's own changes kept in a layer of its own on top.
    Private,
}

/// What a mount's location is on the host that holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountKind {
    /// A directory that exists on the host.
    Path,
    /// A volume that Orch made on a storage of the host.
    Volume,
}

/// The volume that keeps what one environment wrote in a private view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MountLayer {
    /// The volume's name.
    pub name: String,
    /// The storage of the host it is made on; `null` for the host's default.
    pub storage: Option<String>,
}

/// One mount to attach, as a runtime is asked to be made with it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MountSpec {
    /// The mount's name.
    pub name: String,
    /// Whether the location is a directory or a volume.
    pub kind: MountKind,
    /// The location on the host: a path, or the volume's name.
    pub source: String,
    /// The place it has inside the environment: absolute, and not `/`.
    pub target: String,
    /// How it is to be attached.
    pub mode: MountMode,
    /// Where a private view keeps its changes; only with the mode `private`.
    pub layer: Option<MountLayer>,
}

/// The complete before and after sets for one live change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct MountChange {
    /// The attachments believed to be installed; their sources stay on the node.
    pub before: Vec<MountSpec>,
    /// The complete desired set, not a partial patch.
    pub after: Vec<MountSpec>,
    /// Workspace, home, state and tool roots that no data mount may replace.
    pub reserved: Vec<String>,
}

/// Whether a mount was attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountState {
    /// It is there.
    Attached,
    /// It is not, for the reason given beside it.
    Unavailable,
}

/// Why a mount is not attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MountReason {
    /// This machine has no location for the mount.
    NoLocation,
    /// The environment's driver cannot attach mounts.
    Driver,
    /// The location's directory does not exist on the machine.
    Missing,
    /// The machine's local policy does not allow that path.
    Refused,
    /// Its target collides with the workspace, the home or another mount.
    TargetConflict,
}

/// What became of one mount, as an environment reports it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct MountFact {
    /// The mount.
    pub mount_id: Id,
    /// Its name.
    pub name: String,
    /// Whether it is attached.
    pub state: MountState,
    /// Why not, when it is not.
    pub reason: Option<MountReason>,
    /// The path it has for the agent.
    pub path: Option<String>,
    /// The mode it got.
    pub mode: Option<MountMode>,
    /// The mode it was asked for.
    pub asked: Option<MountMode>,
    /// Whether the mount itself keeps a read-only mount from being written; where it does not, only the
    /// harness's own sandbox and the file permissions do.
    pub enforced: Option<bool>,
}

/// One line of the file that lists what a driver attached: where the connector learns what it has.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct AttachedMount {
    /// The mount's name.
    pub name: String,
    /// The path it has inside the unit.
    pub path: String,
    /// The mode it got.
    pub mode: MountMode,
}

/// The mode a mount asked for as `asked` gets on a machine that can make a private view only if
/// `private_views` says so. A private view that cannot be made is attached read-only: nothing falls back
/// to shared, because that would let one environment change what others read.
#[must_use]
pub const fn attach_mode(asked: MountMode, private_views: bool) -> MountMode {
    match asked {
        MountMode::Private if !private_views => MountMode::ReadOnly,
        other => other,
    }
}

/// Whether the mount itself keeps a read-only mount from being written: it does where the driver places
/// the mount, and it does not where the data is used in place.
#[must_use]
pub const fn read_only_is_enforced(placement: MountPlacement) -> bool {
    matches!(placement, MountPlacement::Placed)
}

/// Why a target cannot be a mount's place.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    /// It is not an absolute path.
    #[error("a mount's target is an absolute path")]
    NotAbsolute,
    /// It is the root of the file system.
    #[error("a mount's target is not /")]
    Root,
    /// It holds an empty segment, `.` or `..`, or ends in `/`: it is not normalised.
    #[error("a mount's target is a normalised path: no empty segments, no . or .., no trailing /")]
    NotNormalised,
}

/// Checks that `target` is an absolute, normalised path other than `/`.
pub fn check_target(target: &str) -> Result<(), TargetError> {
    if !target.starts_with('/') {
        return Err(TargetError::NotAbsolute);
    }
    if target == "/" {
        return Err(TargetError::Root);
    }
    let rest = &target[1..];
    if target.contains('\0')
        || rest
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return Err(TargetError::NotNormalised);
    }
    Ok(())
}

/// Whether `a` and `b` are the same path, or one lies inside the other.
fn overlaps(a: &str, b: &str) -> bool {
    let inside = |outer: &str, inner: &str| {
        inner == outer
            || inner
                .strip_prefix(outer)
                .is_some_and(|rest| rest.starts_with('/'))
    };
    inside(a, b) || inside(b, a)
}

/// Why a mount of a list was left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetFault {
    /// Its target cannot be one.
    Invalid(TargetError),
    /// Its target is, contains or lies inside a place that belongs to the environment itself.
    Reserved,
    /// A mount that comes before it in the list has the same target.
    Taken,
}

/// Which of `mounts`, given as (name, target) in the order of the scopes that enabled them with the
/// nearest first, cannot be attached: those with a target that is not valid, those whose target is, contains
/// or lies inside one of `reserved` (the workspace, the home and `/run/orch`), and those whose target a
/// mount before them in the list has too. The nearer scope's mount is the one that is attached. A mount
/// that is left out is reported as `target_conflict`.
#[must_use]
pub fn check_targets<'a>(
    mounts: &'a [(&'a str, &'a str)],
    reserved: &[&str],
) -> Vec<(&'a str, TargetFault)> {
    let mut left_out = Vec::new();
    let mut kept: Vec<&str> = Vec::new();
    for &(name, target) in mounts {
        if let Err(error) = check_target(target) {
            left_out.push((name, TargetFault::Invalid(error)));
        } else if reserved.iter().any(|place| overlaps(place, target)) {
            left_out.push((name, TargetFault::Reserved));
        } else if kept.contains(&target) {
            left_out.push((name, TargetFault::Taken));
        } else {
            kept.push(target);
        }
    }
    left_out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_private_view_that_cannot_be_made_is_attached_read_only_and_never_shared() {
        assert_eq!(attach_mode(MountMode::Private, true), MountMode::Private);
        assert_eq!(attach_mode(MountMode::Private, false), MountMode::ReadOnly);
        assert_eq!(attach_mode(MountMode::Shared, false), MountMode::Shared);
        assert_eq!(attach_mode(MountMode::ReadOnly, false), MountMode::ReadOnly);
    }

    #[test]
    fn read_only_is_held_by_the_mount_itself_only_where_the_driver_places_it() {
        assert!(read_only_is_enforced(MountPlacement::Placed));
        assert!(!read_only_is_enforced(MountPlacement::InPlace));
        assert!(!read_only_is_enforced(MountPlacement::None));
    }

    #[test]
    fn a_target_is_absolute_normalised_and_not_the_root() {
        assert_eq!(check_target("/data/src"), Ok(()));
        assert_eq!(check_target("data/src"), Err(TargetError::NotAbsolute));
        assert_eq!(check_target("/"), Err(TargetError::Root));
        for bad in ["/data//src", "/data/./src", "/data/../etc", "/data/src/"] {
            assert_eq!(check_target(bad), Err(TargetError::NotNormalised), "{bad}");
        }
    }

    #[test]
    fn a_mount_on_the_environments_own_places_or_on_a_nearer_mounts_target_is_left_out() {
        let reserved = ["/workspace", "/home/agent", "/run/orch"];
        let mounts = [
            ("src", "/data/src"),
            ("again", "/data/src"),
            ("inside", "/data/src/vendor"),
            ("work", "/workspace/cache"),
            ("above", "/home"),
            ("kept", "/mnt/cache"),
            ("bad", "mnt"),
        ];
        let left_out = check_targets(&mounts, &reserved);
        assert_eq!(
            left_out,
            vec![
                ("again", TargetFault::Taken),
                ("work", TargetFault::Reserved),
                ("above", TargetFault::Reserved),
                ("bad", TargetFault::Invalid(TargetError::NotAbsolute)),
            ]
        );
    }

    #[test]
    fn a_names_prefix_is_not_a_place_that_contains_it() {
        assert_eq!(
            check_targets(&[("a", "/workspace2")], &["/workspace"]),
            vec![]
        );
    }

    #[test]
    fn what_an_environment_reports_of_a_mount_reads_as_the_spec_writes_it() {
        let fact: MountFact = serde_json::from_value(json!({
            "mount_id": "0199abcd-1234-7000-8000-000000000001", "name": "src", "state": "attached",
            "reason": null, "path": "/data/src", "mode": "private", "asked": "private", "enforced": true
        }))
        .unwrap();
        assert_eq!(fact.mode, Some(MountMode::Private));
        let gone: MountFact = serde_json::from_value(json!({
            "mount_id": "0199abcd-1234-7000-8000-000000000002", "name": "data", "state": "unavailable",
            "reason": "no_location", "path": null, "mode": null, "asked": "shared", "enforced": null
        }))
        .unwrap();
        assert_eq!(gone.reason, Some(MountReason::NoLocation));
        assert_eq!(
            serde_json::to_value(MountPlacement::InPlace).unwrap(),
            json!("in_place")
        );
    }

    #[test]
    fn a_mount_to_attach_reads_as_the_runtime_spec_writes_it() {
        let spec: MountSpec = serde_json::from_value(json!({
            "name": "src", "kind": "path", "source": "/srv/src", "target": "/data/src", "mode": "private",
            "layer": { "name": "orch-over-0199-0001", "storage": "fast" }
        }))
        .unwrap();
        assert_eq!(spec.kind, MountKind::Path);
        assert_eq!(
            spec.layer.as_ref().map(|l| l.storage.as_deref()),
            Some(Some("fast"))
        );
    }
}
