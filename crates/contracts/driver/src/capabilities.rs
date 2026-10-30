//! Per-driver capability patches.
//!
//! A capability widens what an environment may do to the machine it runs on: a debugger, a virtualisation
//! device, a network privilege. A capability definition carries **one patch per driver name**, in the shape
//! that driver declares for the key. A driver applies only its own patch and ignores the rest; a capability
//! with no patch for the environment's driver is not offered there ([`is_offered`]). A driver that declares
//! no capabilities offers none.
//!
//! How a driver applies several patches is its own business. The built-in `docker` driver merges them into
//! the container's creation options: lists are united, booleans are OR-ed, and a setting that can have only
//! one value takes the last that is given ([`merge_patches`]).

use std::collections::BTreeMap;

use orch_types::Json;

use crate::descriptor::DriverDescriptor;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The immutable prepared template boundary and this environment's current grants, with this driver's frozen patches.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct CapabilityState {
    /// What the template permits a later grant to give.
    pub allowed: Vec<String>,
    /// Capabilities present from the template's first start.
    pub base: Vec<String>,
    /// Environment grants, kept distinct even when given before first start.
    pub granted: Vec<String>,
    /// Frozen patches for this driver alone; gateway capabilities need no patch.
    pub patches: BTreeMap<String, Json>,
}

impl CapabilityState {
    /// Whether a legacy request gives no capability metadata.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.allowed.is_empty()
            && self.base.is_empty()
            && self.granted.is_empty()
            && self.patches.is_empty()
    }

    /// Checks that a grant fits the template and every native held capability has this driver's patch.
    pub fn check(&self) -> Result<(), crate::driver::DriverError> {
        for key in &self.granted {
            if !self.allowed.contains(key) && !self.base.contains(key) {
                return Err(crate::driver::DriverError::InvalidSpec(
                    "a capability grant exceeds the prepared template boundary".into(),
                ));
            }
        }
        for key in self.base.iter().chain(&self.granted) {
            if !gateway_given(key) && self.patches.get(key).is_none_or(Json::is_null) {
                return Err(crate::driver::DriverError::Unsupported(format!(
                    "this driver has no frozen patch for capability {key}"
                )));
            }
        }
        Ok(())
    }

    /// Whether only grants changed, keeping the exact prepared allowlist, defaults and patches.
    #[must_use]
    pub fn same_boundary(&self, other: &Self) -> bool {
        self.allowed == other.allowed && self.base == other.base && self.patches == other.patches
    }

    /// Applies defaults and unavoidable grants at creation; live environment grants use the helper.
    #[must_use]
    pub fn creation_spec(&self, spec: &Json, live: &[&str]) -> Json {
        let mut patches = vec![spec];
        for key in self.base.iter().chain(
            self.granted
                .iter()
                .filter(|key| !live.contains(&key.as_str())),
        ) {
            if let Some(patch) = self.patches.get(key) {
                patches.push(patch);
            }
        }
        merge_patches(&patches)
    }
}

/// Capabilities supplied by the gateway, independent of a driver's native patch or process lifecycle.
pub const GATEWAY_GIVEN: [&str; 3] = ["port_place", "port_host", "port_link"];

/// Whether this capability is applied by the gateway rather than by a machine driver.
#[must_use]
pub fn gateway_given(key: &str) -> bool {
    GATEWAY_GIVEN.contains(&key)
}

/// The patch of `driver` among the patches a capability definition carries, by driver name.
#[must_use]
pub fn patch_for<'a>(driver: &str, driver_patch: &'a BTreeMap<String, Json>) -> Option<&'a Json> {
    driver_patch.get(driver)
}

/// Whether the capability `key`, whose definition carries `driver_patch`, is offered in an environment of
/// `driver`: the driver declares the key, and the definition has a patch for the driver.
#[must_use]
pub fn is_offered(
    driver: &DriverDescriptor,
    key: &str,
    driver_patch: &BTreeMap<String, Json>,
) -> bool {
    (gateway_given(key)
        && driver
            .live_changes
            .gateway_given
            .iter()
            .any(|given| given == key))
        || (driver.capabilities.contains_key(key)
            && patch_for(&driver.name, driver_patch).is_some())
}

/// Merges patches, in the order given, into one: lists are united (each value once, in the order first
/// seen), booleans are OR-ed, objects are merged member by member, and any other value takes the last that
/// is not `null`.
#[must_use]
pub fn merge_patches(patches: &[&Json]) -> Json {
    patches
        .iter()
        .fold(Json::Object(serde_json::Map::new()), |merged, patch| {
            merge_two(merged, patch)
        })
}

fn merge_two(into: Json, from: &Json) -> Json {
    match (into, from) {
        // Nothing is given: what was there stays.
        (into, Json::Null) => into,
        (Json::Object(mut into), Json::Object(from)) => {
            for (name, value) in from {
                let merged = merge_two(into.remove(name).unwrap_or(Json::Null), value);
                if !merged.is_null() {
                    into.insert(name.clone(), merged);
                }
            }
            Json::Object(into)
        }
        (Json::Array(mut into), Json::Array(from)) => {
            for value in from {
                if !into.contains(value) {
                    into.push(value.clone());
                }
            }
            Json::Array(into)
        }
        (Json::Bool(into), Json::Bool(from)) => Json::Bool(into || *from),
        (_, other) => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn driver(name: &str, capabilities: &[&str]) -> DriverDescriptor {
        let mut descriptor: DriverDescriptor =
            serde_json::from_value(json!({ "name": name, "version": "1", "title": name })).unwrap();
        descriptor.capabilities = capabilities
            .iter()
            .map(|key| ((*key).to_owned(), json!({ "type": "object" })))
            .collect();
        descriptor
    }

    #[test]
    fn a_capability_is_offered_only_where_the_driver_declares_it_and_its_definition_has_a_patch_for_it()
     {
        let patches = BTreeMap::from([
            (
                "docker".to_owned(),
                json!({ "devices": [ { "path": "/dev/kvm", "permissions": "rwm" } ] }),
            ),
            ("firecracker".to_owned(), json!({ "vcpus": 2 })),
        ]);
        assert!(is_offered(&driver("docker", &["kvm"]), "kvm", &patches));
        assert!(
            !is_offered(&driver("docker", &[]), "kvm", &patches),
            "a driver that declares none offers none"
        );
        assert!(
            !is_offered(&driver("host", &["kvm"]), "kvm", &patches),
            "no patch for the driver, so not offered"
        );
        assert!(!is_offered(&driver("docker", &["gpu"]), "kvm", &patches));
        assert_eq!(patch_for("docker", &patches), Some(&patches["docker"]));
        assert_eq!(patch_for("host", &patches), None);
    }

    #[test]
    fn patches_are_merged_lists_united_booleans_or_ed_and_the_last_value_kept() {
        let ptrace = json!({ "cap_add": ["SYS_PTRACE"], "privileged": false, "security_opt": ["seccomp=orch-ptrace"] });
        let net = json!({ "cap_add": ["NET_ADMIN", "NET_RAW", "SYS_PTRACE"], "privileged": false });
        let host_net = json!({ "network_mode": "host", "privileged": true });
        let bridge = json!({ "network_mode": "bridge" });
        let nothing = json!({ "network_mode": null });
        let merged = merge_patches(&[&ptrace, &net, &host_net, &bridge, &nothing]);
        assert_eq!(
            merged["cap_add"],
            json!(["SYS_PTRACE", "NET_ADMIN", "NET_RAW"])
        );
        assert_eq!(merged["privileged"], json!(true));
        assert_eq!(
            merged["network_mode"],
            json!("bridge"),
            "the last that is given"
        );
        assert_eq!(merged["security_opt"], json!(["seccomp=orch-ptrace"]));
        assert_eq!(merge_patches(&[]), json!({}));
    }

    #[test]
    fn objects_inside_a_patch_are_merged_member_by_member() {
        let one = json!({ "limits": { "memlock": true, "nofile": 1024 } });
        let two = json!({ "limits": { "memlock": false, "nofile": 4096, "nproc": 10 } });
        assert_eq!(
            merge_patches(&[&one, &two]),
            json!({ "limits": { "memlock": true, "nofile": 4096, "nproc": 10 } })
        );
    }
}
