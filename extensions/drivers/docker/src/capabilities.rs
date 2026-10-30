//! Docker capability patches.
//!
//! A capability widens what an environment may do to the machine it runs on, and does it for the Docker driver by
//! a patch of the container's creation options: kernel capabilities, devices, groups, security options, the
//! privileged mode, the network mode, bind mounts, device requests. The patches of all the capabilities an
//! environment has are merged (lists united, booleans OR-ed, the last network mode that is given wins) and come
//! in the members of the spec; [`CapabilityPatch`] reads them and [`CapabilityPatch::apply`] adds them to the
//! options the unit is made with, over what the spec's `security` already says.

use std::collections::BTreeMap;
use std::path::Path;

use bollard::models::{DeviceMapping, DeviceRequest, HostConfig};
use orch_types::Json;
use serde::Deserialize;

/// The capability keys the driver can apply: each has a patch of the shape [`schema`] describes.
pub const KEYS: [&str; 9] = [
    "ptrace",
    "kvm",
    "gpu",
    "net_admin",
    "unconfined",
    "privileged",
    "host_network",
    "docker_socket",
    "docker_inside",
];

/// The capability that lets an environment run Docker of its own, inside the guest. Its patch gives every kernel
/// capability and takes the container's filters off, which is moderate only where a kernel of the guest's own stands
/// between the unit and the host; the driver also prepares the place where that Docker keeps its data.
pub const DOCKER_INSIDE: &str = "docker_inside";

/// What permissions a device is given when its patch names none: read, write and make nodes.
const DEFAULT_DEVICE_PERMISSIONS: &str = "rwm";

/// A device the unit is given.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DevicePatch {
    /// The device on the host, which is also its place in the unit.
    pub path: String,
    /// What the unit may do with it: any of `r`, `w` and `m`.
    #[serde(default)]
    pub permissions: Option<String>,
}

/// A request for devices of a driver (a graphics processor, say).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct DeviceRequestPatch {
    /// How many devices; -1 is all of them.
    #[serde(default)]
    pub count: Option<i64>,
    /// An OR list of AND lists of capabilities the devices must have.
    #[serde(default)]
    pub capabilities: Vec<Vec<String>>,
    /// The device driver, when one is named.
    #[serde(default)]
    pub driver: Option<String>,
    /// Particular devices, when they are named.
    #[serde(default)]
    pub device_ids: Vec<String>,
}

/// The members of a spec that capabilities merge into.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Wire {
    cap_add: Option<Vec<String>>,
    devices: Option<Vec<DevicePatch>>,
    group_add: Option<Vec<String>>,
    security_opt: Option<Vec<String>>,
    privileged: Option<bool>,
    network_mode: Option<String>,
    binds: Option<Vec<String>>,
    device_requests: Option<Vec<DeviceRequestPatch>>,
    device_cgroup_rules: Option<Vec<String>>,
}

/// The merged patch of an environment's capabilities.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CapabilityPatch {
    /// Kernel capabilities to add.
    pub cap_add: Vec<String>,
    /// Devices to give.
    pub devices: Vec<DevicePatch>,
    /// Groups the unit's process joins.
    pub group_add: Vec<String>,
    /// Security options (`seccomp=…`, `apparmor=…`).
    pub security_opt: Vec<String>,
    /// Whether the unit is privileged.
    pub privileged: bool,
    /// The network mode, which replaces the managed network.
    pub network_mode: Option<String>,
    /// Bind mounts, `host:container[:options]`.
    pub binds: Vec<String>,
    /// Requests for devices of a driver.
    pub device_requests: Vec<DeviceRequestPatch>,
    /// Rules of the device control group, `b 7:* rwm`: which device nodes the unit may make and open.
    pub device_cgroup_rules: Vec<String>,
}

impl CapabilityPatch {
    /// Reads the patch members of a spec. A spec without any is the patch of no capability.
    pub fn from_spec(spec: &Json) -> Result<Self, String> {
        let wire: Wire = serde_json::from_value(spec.clone())
            .map_err(|error| format!("a capability's patch: {error}"))?;
        let patch = Self {
            cap_add: wire.cap_add.unwrap_or_default(),
            devices: wire.devices.unwrap_or_default(),
            group_add: wire.group_add.unwrap_or_default(),
            security_opt: wire.security_opt.unwrap_or_default(),
            privileged: wire.privileged.unwrap_or(false),
            network_mode: wire.network_mode,
            binds: wire.binds.unwrap_or_default(),
            device_requests: wire.device_requests.unwrap_or_default(),
            device_cgroup_rules: wire.device_cgroup_rules.unwrap_or_default(),
        };
        patch.check()?;
        Ok(patch)
    }

    /// What is wrong with the patch, if anything.
    fn check(&self) -> Result<(), String> {
        for device in &self.devices {
            if !device.path.starts_with('/') {
                return Err(format!(
                    "a capability's device {:?} is an absolute path",
                    device.path
                ));
            }
            if let Some(permissions) = &device.permissions
                && (permissions.is_empty()
                    || !permissions.chars().all(|c| matches!(c, 'r' | 'w' | 'm')))
            {
                return Err(format!(
                    "a capability's device permissions are some of r, w and m, not {permissions:?}"
                ));
            }
        }
        for bind in &self.binds {
            let host = bind.split(':').next().unwrap_or_default();
            if !host.starts_with('/') {
                return Err(format!(
                    "a capability's bind mount {bind:?} starts with an absolute path of the host"
                ));
            }
        }
        for rule in &self.device_cgroup_rules {
            // Docker's own form: a kind (a, b or c), major and minor numbers or *, and some of r, w and m.
            let parts: Vec<&str> = rule.split_whitespace().collect();
            let valid = parts.len() == 3
                && matches!(parts[0], "a" | "b" | "c")
                && parts[1].split(':').count() == 2
                && parts[1]
                    .split(':')
                    .all(|number| number == "*" || number.parse::<u32>().is_ok())
                && !parts[2].is_empty()
                && parts[2].chars().all(|c| matches!(c, 'r' | 'w' | 'm'));
            if !valid {
                return Err(format!(
                    "a capability's device rule {rule:?} is a kind, major:minor and some of r, w and m"
                ));
            }
        }
        if let Some(mode) = &self.network_mode
            && (mode.is_empty() || mode.starts_with("container:"))
        {
            // Joining another container's network would join another unit's.
            return Err(format!(
                "a capability's network mode {mode:?} is not available"
            ));
        }
        Ok(())
    }

    /// Adds the patch to the options of a container: lists are united with what is there, the privileged mode
    /// is OR-ed, and the network mode replaces the one that was chosen. `seccomp_dir` is where profiles that a
    /// security option names are kept.
    pub fn apply(&self, host: &mut HostConfig, seccomp_dir: Option<&Path>) {
        unite(&mut host.cap_add, &self.cap_add);
        unite(&mut host.group_add, &self.group_add);
        unite(&mut host.binds, &self.binds);
        unite(&mut host.device_cgroup_rules, &self.device_cgroup_rules);
        let resolved: Vec<String> = self
            .security_opt
            .iter()
            .filter_map(|option| resolve_security_option(option, seccomp_dir))
            .collect();
        unite(&mut host.security_opt, &resolved);
        if self.privileged {
            host.privileged = Some(true);
        }
        if let Some(mode) = &self.network_mode {
            host.network_mode = Some(mode.clone());
        }
        if !self.devices.is_empty() {
            host.devices
                .get_or_insert_with(Vec::new)
                .extend(self.devices.iter().map(|device| {
                    DeviceMapping {
                        path_on_host: Some(device.path.clone()),
                        path_in_container: Some(device.path.clone()),
                        cgroup_permissions: Some(
                            device
                                .permissions
                                .clone()
                                .unwrap_or_else(|| DEFAULT_DEVICE_PERMISSIONS.to_owned()),
                        ),
                    }
                }));
        }
        if !self.device_requests.is_empty() {
            host.device_requests.get_or_insert_with(Vec::new).extend(
                self.device_requests.iter().map(|request| DeviceRequest {
                    driver: request.driver.clone(),
                    count: request.count,
                    device_ids: (!request.device_ids.is_empty())
                        .then(|| request.device_ids.clone()),
                    capabilities: (!request.capabilities.is_empty())
                        .then(|| request.capabilities.clone()),
                    options: None,
                }),
            );
        }
    }
}

/// Adds the values of `more` to `list` that it does not hold, keeping the order they are first seen in.
fn unite(list: &mut Option<Vec<String>>, more: &[String]) {
    if more.is_empty() {
        return;
    }
    let list = list.get_or_insert_with(Vec::new);
    for value in more {
        if !list.contains(value) {
            list.push(value.clone());
        }
    }
}

/// A security option as the daemon wants it. The daemon takes the text of a seccomp profile and not its name,
/// so `seccomp=<name>` becomes the content of `<seccomp_dir>/<name>.json`; a profile that is not installed is
/// left out (the daemon's own default profile then applies, which is the stricter one) and logged.
fn resolve_security_option(option: &str, seccomp_dir: Option<&Path>) -> Option<String> {
    let Some(profile) = option.strip_prefix("seccomp=") else {
        return Some(option.to_owned());
    };
    if profile == "unconfined" || profile.trim_start().starts_with('{') {
        return Some(option.to_owned());
    }
    let found = seccomp_dir
        .filter(|_| {
            profile
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        })
        .and_then(|dir| std::fs::read_to_string(dir.join(format!("{profile}.json"))).ok());
    if found.is_none() {
        tracing::warn!(
            profile,
            "a seccomp profile is not installed on this host; the default profile applies"
        );
    }
    found.map(|text| format!("seccomp={text}"))
}

/// The JSON Schema of the patch the driver reads for every capability key.
#[must_use]
pub fn schema() -> Json {
    serde_json::json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "cap_add": { "type": "array", "items": { "type": "string" } },
            "devices": { "type": "array", "items": {
                "type": "object", "required": ["path"], "additionalProperties": false,
                "properties": { "path": { "type": "string" }, "permissions": { "type": "string", "pattern": "^[rwm]+$" } } } },
            "group_add": { "type": "array", "items": { "type": "string" } },
            "security_opt": { "type": "array", "items": { "type": "string" } },
            "privileged": { "type": "boolean" },
            "network_mode": { "type": ["string", "null"] },
            "binds": { "type": "array", "items": { "type": "string" } },
            "device_cgroup_rules": { "type": "array", "items": { "type": "string" } },
            "device_requests": { "type": "array", "items": {
                "type": "object", "additionalProperties": false,
                "properties": {
                    "count": { "type": "integer" },
                    "driver": { "type": "string" },
                    "device_ids": { "type": "array", "items": { "type": "string" } },
                    "capabilities": { "type": "array", "items": { "type": "array", "items": { "type": "string" } } } } } }
        }
    })
}

/// The capabilities the driver declares: every key with the schema of its patch.
#[must_use]
pub fn declared() -> BTreeMap<String, Json> {
    KEYS.iter()
        .map(|key| ((*key).to_owned(), schema()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn the_merged_patch_is_read_from_the_members_of_the_spec() {
        let patch = CapabilityPatch::from_spec(&json!({
            "image": "x",
            "cap_add": ["SYS_PTRACE", "NET_ADMIN"],
            "devices": [ { "path": "/dev/kvm", "permissions": "rwm" } ],
            "group_add": ["993"], "security_opt": ["seccomp=unconfined"],
            "privileged": true, "network_mode": null,
            "binds": ["/var/run/docker.sock:/var/run/docker.sock"],
            "device_requests": [ { "count": -1, "capabilities": [["gpu"]] } ]
        }))
        .unwrap();
        assert_eq!(patch.cap_add, ["SYS_PTRACE", "NET_ADMIN"]);
        assert!(patch.privileged);
        assert_eq!(
            patch.network_mode, None,
            "a null network mode is none given"
        );
        assert_eq!(patch.devices[0].path, "/dev/kvm");
        assert_eq!(patch.device_requests[0].count, Some(-1));
        assert_eq!(
            CapabilityPatch::from_spec(&json!({ "image": "x" })).unwrap(),
            CapabilityPatch::default()
        );
    }

    #[test]
    fn a_patch_that_cannot_be_applied_is_refused() {
        for bad in [
            json!({ "devices": [ { "path": "dev/kvm" } ] }),
            json!({ "devices": [ { "path": "/dev/kvm", "permissions": "x" } ] }),
            json!({ "binds": ["relative:/x"] }),
            json!({ "network_mode": "container:other-unit" }),
            json!({ "cap_add": "NET_ADMIN" }),
        ] {
            assert!(CapabilityPatch::from_spec(&bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_patch_is_added_to_the_options_the_unit_is_made_with() {
        let patch = CapabilityPatch::from_spec(&json!({
            "cap_add": ["SYS_PTRACE", "NET_ADMIN"], "privileged": true, "network_mode": "host",
            "devices": [ { "path": "/dev/kvm" } ], "binds": ["/a:/b"],
            "device_requests": [ { "count": -1, "capabilities": [["gpu"]] } ]
        }))
        .unwrap();
        let mut host = HostConfig {
            cap_add: Some(vec!["NET_ADMIN".into(), "CHOWN".into()]),
            network_mode: Some("orch-env".into()),
            ..HostConfig::default()
        };
        patch.apply(&mut host, None);
        assert_eq!(host.cap_add.unwrap(), ["NET_ADMIN", "CHOWN", "SYS_PTRACE"]);
        assert_eq!(host.network_mode.as_deref(), Some("host"));
        assert_eq!(host.privileged, Some(true));
        let device = &host.devices.unwrap()[0];
        assert_eq!(device.cgroup_permissions.as_deref(), Some("rwm"));
        assert_eq!(device.path_in_container.as_deref(), Some("/dev/kvm"));
        assert_eq!(host.binds.unwrap(), ["/a:/b"]);
        assert_eq!(
            host.device_requests.unwrap()[0].capabilities,
            Some(vec![vec!["gpu".to_owned()]])
        );
    }

    #[test]
    fn no_patch_leaves_the_options_as_they_were() {
        let mut host = HostConfig::default();
        CapabilityPatch::default().apply(&mut host, None);
        assert_eq!(host, HostConfig::default());
    }

    #[test]
    fn a_seccomp_profile_is_named_by_the_patch_and_given_to_the_daemon_by_its_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("orch-ptrace.json"),
            "{\"defaultAction\":\"SCMP_ACT_ALLOW\"}",
        )
        .unwrap();
        assert_eq!(
            resolve_security_option("seccomp=orch-ptrace", Some(dir.path())),
            Some("seccomp={\"defaultAction\":\"SCMP_ACT_ALLOW\"}".to_owned())
        );
        assert_eq!(
            resolve_security_option("seccomp=unconfined", None),
            Some("seccomp=unconfined".to_owned())
        );
        assert_eq!(
            resolve_security_option("apparmor=unconfined", None),
            Some("apparmor=unconfined".to_owned())
        );
        assert_eq!(
            resolve_security_option("seccomp=missing", Some(dir.path())),
            None,
            "a profile that is not installed is left out"
        );
        assert_eq!(
            resolve_security_option("seccomp=../../etc/passwd", Some(dir.path())),
            None
        );
        assert_eq!(resolve_security_option("seccomp=orch-ptrace", None), None);
    }

    #[test]
    fn every_key_the_driver_declares_has_the_patch_schema() {
        let declared = declared();
        assert_eq!(declared.len(), KEYS.len());
        assert!(declared.values().all(|schema| schema["type"] == "object"));
    }
}
