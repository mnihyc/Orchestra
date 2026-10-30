//! Node driver ceilings: available implementations and the work their templates may request.

use orch_types::Json;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Source and native execution ceilings for every image-based driver.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ImageRules {
    /// Exact image references, or `*` for any image the driver supports.
    pub images: Vec<String>,
    /// Registry authorities, with Docker Hub written as docker.io; `*` permits any.
    pub registries: Vec<String>,
    /// Native capability keys and explicitly permitted Linux capability names.
    pub capabilities: Vec<String>,
    /// Permitted declared runtime classes, or `*` for any class the daemon actually reports.
    pub runtime_classes: Vec<String>,
    /// Privileged containers require an explicit true, including under Full Access.
    pub privileged: bool,
    /// Builds are allowed only with unrestricted image/registry rules, since a Dockerfile can fetch bases.
    pub builds: bool,
    /// Loading an archive needs unrestricted image rules, since its identity is not known before loading.
    pub archives: bool,
}

/// What drivers declaring local accounts may launch on behalf of a template.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct ProcessRules {
    /// Template identifiers, or `*` for every otherwise usable template.
    pub templates: Vec<String>,
    /// Existing local account names the driver may use.
    pub accounts: Vec<String>,
    /// Absolute launch programs named by template commands, or `*` for any.
    pub programs: Vec<String>,
}

/// One node setting for its driver offers and ceilings; facts still determine actual support.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(default, deny_unknown_fields)]
pub struct MachineDrivers {
    /// Driver identifiers or `*`; this never registers an external program.
    pub allowed: Vec<String>,
    /// Image source, capability and runtime limits, independent of implementation.
    #[serde(alias = "docker")]
    pub images: ImageRules,
    /// Local account, template and launch-program limits.
    #[serde(alias = "host")]
    pub processes: ProcessRules,
    /// Locally imported driver connection/configuration values. A connection change needs a node restart.
    pub configurations: BTreeMap<String, Json>,
}

fn permits(allowed: &[String], value: &str) -> bool {
    allowed.iter().any(|entry| entry == "*" || entry == value)
}
fn unrestricted(allowed: &[String]) -> bool {
    allowed.iter().any(|entry| entry == "*")
}

impl MachineDrivers {
    /// Values for a handed driver setting; privileged remains off until explicitly enabled.
    #[must_use]
    pub fn managed() -> Self {
        let all = vec!["*".into()];
        Self {
            allowed: all.clone(),
            images: ImageRules {
                images: all.clone(),
                registries: all.clone(),
                capabilities: all.clone(),
                runtime_classes: all.clone(),
                privileged: false,
                builds: true,
                archives: true,
            },
            processes: ProcessRules {
                templates: all.clone(),
                accounts: all.clone(),
                programs: all,
            },
            configurations: BTreeMap::new(),
        }
    }

    /// Structural checks before a value replaces the current policy.
    #[must_use]
    pub fn valid(&self) -> bool {
        [
            &self.allowed,
            &self.images.images,
            &self.images.registries,
            &self.images.capabilities,
            &self.images.runtime_classes,
            &self.processes.templates,
            &self.processes.accounts,
            &self.processes.programs,
        ]
        .into_iter()
        .all(|list| list.iter().all(|entry| !entry.trim().is_empty()))
            && self
                .processes
                .programs
                .iter()
                .all(|program| program == "*" || std::path::Path::new(program).is_absolute())
            && self
                .configurations
                .iter()
                .all(|(name, value)| !name.trim().is_empty() && value.is_object())
    }

    /// Whether this declared driver is offered by policy, before its fact support is considered.
    #[must_use]
    pub fn offers(&self, driver: &str) -> bool {
        permits(&self.allowed, driver)
    }

    fn image(&self, image: &str) -> Result<(), String> {
        if !permits(&self.images.images, image) {
            return Err(format!("image {image} is outside the image allow-list"));
        }
        let first = image.split('/').next().unwrap_or("");
        let registry = if image.contains('/')
            && (first.contains('.') || first.contains(':') || first == "localhost")
        {
            first
        } else {
            "docker.io"
        };
        if !permits(&self.images.registries, registry) {
            return Err(format!(
                "registry {registry} is outside the registry allow-list"
            ));
        }
        Ok(())
    }

    /// Check a driver spec and its actual native capabilities on both the server and node.
    pub fn check(
        &self,
        descriptor: &orch_driver_contract::descriptor::DriverDescriptor,
        spec: &Json,
        template: Option<&str>,
        capabilities: &orch_driver_contract::capabilities::CapabilityState,
    ) -> Result<(), String> {
        let check = || -> Result<(), String> {
            let driver = &descriptor.name;
            if !self.offers(driver) {
                return Err(format!("driver {driver} is not offered"));
            }
            if !descriptor.sources.is_empty() {
                for (member, source) in [
                    ("image", orch_driver_contract::sources::SourceKind::Image),
                    ("build", orch_driver_contract::sources::SourceKind::Build),
                    (
                        "archive",
                        orch_driver_contract::sources::SourceKind::Archive,
                    ),
                    (
                        "image_from_blob",
                        orch_driver_contract::sources::SourceKind::Archive,
                    ),
                ] {
                    if spec.get(member).is_some_and(|value| !value.is_null())
                        && !descriptor.has_source(source)
                    {
                        return Err(descriptor
                            .unavailable
                            .get(source.as_str())
                            .cloned()
                            .unwrap_or_else(|| {
                                format!("this driver does not offer {} sources", source.as_str())
                            }));
                    }
                }
                if let Some(image) = spec.get("image").and_then(Json::as_str) {
                    self.image(image)?;
                }
                if spec.get("build").is_some_and(|value| !value.is_null())
                    && (!self.images.builds
                        || !unrestricted(&self.images.images)
                        || !unrestricted(&self.images.registries))
                {
                    return Err("builds need explicit build permission and unrestricted image and registry rules; choose an allowed image instead".into());
                }
                if ["archive", "image_from_blob"]
                    .into_iter()
                    .any(|member| spec.get(member).is_some_and(|value| !value.is_null()))
                    && (!self.images.archives
                        || !unrestricted(&self.images.images)
                        || !unrestricted(&self.images.registries))
                {
                    return Err("loading an image archive needs explicit archive permission and unrestricted image and registry rules".into());
                }
                if let Some(runtime) = spec.get("runtime").and_then(Json::as_str)
                    && !permits(&self.images.runtime_classes, runtime)
                {
                    return Err(format!("runtime class {runtime} is not allowed"));
                }
                for key in capabilities
                    .base
                    .iter()
                    .chain(&capabilities.granted)
                    .filter(|key| !orch_driver_contract::capabilities::gateway_given(key))
                {
                    if !permits(&self.images.capabilities, key) {
                        return Err(format!("capability {key} is not allowed"));
                    }
                }
                let patches = capabilities
                    .base
                    .iter()
                    .chain(&capabilities.granted)
                    .filter_map(|key| capabilities.patches.get(key));
                let privileged = spec.get("privileged").and_then(Json::as_bool) == Some(true)
                    || spec.pointer("/security/privileged").and_then(Json::as_bool) == Some(true)
                    || patches
                        .clone()
                        .any(|patch| patch.get("privileged").and_then(Json::as_bool) == Some(true));
                if privileged && !self.images.privileged {
                    return Err(
                        "privileged execution is off; the node operator must explicitly allow it"
                            .into(),
                    );
                }
                for name in spec
                    .get("cap_add")
                    .or_else(|| spec.pointer("/security/cap_add"))
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Json::as_str)
                {
                    if !permits(&self.images.capabilities, name) {
                        return Err(format!("Linux capability {name} is not allowed"));
                    }
                }
            }
            if descriptor.local_accounts {
                if !unrestricted(&self.processes.templates)
                    && template.is_none_or(|template| !permits(&self.processes.templates, template))
                {
                    return Err(
                        "this host template is outside the node's template allow-list".into(),
                    );
                }
                let account = spec.get("run_as").and_then(Json::as_str).unwrap_or("");
                if !permits(&self.processes.accounts, account) {
                    return Err(format!("host account {account} is not allowed"));
                }
                for command in spec
                    .get("after_up")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    if let Some(program) = command
                        .get("command")
                        .and_then(Json::as_array)
                        .and_then(|argv| argv.first())
                        .and_then(Json::as_str)
                    {
                        self.program(descriptor, program)?;
                    }
                }
                if let Some(program) = spec.get("program").and_then(Json::as_str) {
                    self.program(descriptor, program)?;
                }
            }
            Ok(())
        };
        check().map_err(|reason|format!("drivers (node role): {reason}; the machine's operator or its managers decide this setting"))
    }

    /// After-start commands on a local-account driver use the same launch-program ceiling.
    pub fn program(
        &self,
        descriptor: &orch_driver_contract::descriptor::DriverDescriptor,
        program: &str,
    ) -> Result<(), String> {
        if descriptor.local_accounts && !permits(&self.processes.programs, program) {
            return Err(format!(
                "drivers (node role): host launch program {program} is not allowed by the machine's operator"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn image_ceilings_apply_to_a_third_driver_and_a_new_configuration_name_is_not_refused() {
        let mut policy = MachineDrivers::managed();
        policy
            .configurations
            .insert("cluster-test".into(), json!({"namespace":"agents"}));
        policy
            .configurations
            .insert("other.cluster".into(), json!({"namespace":"other"}));
        assert!(policy.valid());
        policy.images.images = vec!["registry.example.test/agent:1".into()];
        policy.images.registries = vec!["registry.example.test".into()];
        let descriptor = descriptor("cluster-test");
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"image":"registry.example.test/agent:1"}),
                    None,
                    &Default::default()
                )
                .is_ok()
        );
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"image":"outside.test/agent:1"}),
                    None,
                    &Default::default()
                )
                .unwrap_err()
                .contains("image")
        );
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"build":{"dockerfile":"FROM x"}}),
                    None,
                    &Default::default()
                )
                .unwrap_err()
                .contains("build")
        );
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"archive":{"blob":"unverified"}}),
                    None,
                    &Default::default()
                )
                .unwrap_err()
                .contains("archive")
        );
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"archive":null,"image_from_blob":{"blob":"unverified"}}),
                    None,
                    &Default::default()
                )
                .unwrap_err()
                .contains("archive")
        );
        policy.images.images = vec!["*".into()];
        assert!(
            policy
                .check(
                    &descriptor,
                    &json!({"image":"outside.test/agent:1"}),
                    None,
                    &Default::default()
                )
                .unwrap_err()
                .contains("registry")
        );
    }

    #[test]
    fn existing_machine_rules_read_without_losing_restrictions_and_write_neutral_names() {
        let old = json!({"allowed":["docker"],"docker":{"images":["allowed:1"],"builds":false},"host":{"accounts":["agent"]}});
        let policy: MachineDrivers = serde_json::from_value(old).unwrap();
        assert_eq!(policy.images.images, ["allowed:1"]);
        assert!(!policy.images.builds);
        assert_eq!(policy.processes.accounts, ["agent"]);
        let written = serde_json::to_value(&policy).unwrap();
        assert!(written.get("docker").is_none() && written.get("host").is_none());
        assert_eq!(written["images"]["images"], json!(["allowed:1"]));
    }
    fn descriptor(name: &str) -> orch_driver_contract::descriptor::DriverDescriptor {
        serde_json::from_value(json!({"name":name,"version":"1","title":name,
            "sources": if name == "host" {json!([])} else {json!(["image","build","archive"])},
            "local_accounts": name == "host"}))
        .unwrap()
    }
    #[test]
    fn a_sandboxed_node_may_offer_custom_docker_and_named_host_programs_with_explicit_boundaries() {
        let mut policy = MachineDrivers::managed();
        policy.images.images = vec!["registry.example.test/team/agent:1".into()];
        policy.images.registries = vec!["registry.example.test".into()];
        policy.images.runtime_classes = vec!["gvisor".into()];
        policy.images.capabilities = vec!["kvm".into()];
        let caps = orch_driver_contract::capabilities::CapabilityState {
            base: vec!["kvm".into()],
            ..Default::default()
        };
        let mut spec = json!({"image":"registry.example.test/team/agent:1","runtime":"gvisor"});
        assert!(
            policy
                .check(&descriptor("docker"), &spec, None, &caps)
                .is_ok()
        );
        spec["privileged"] = json!(true);
        assert!(
            policy
                .check(&descriptor("docker"), &spec, None, &caps)
                .unwrap_err()
                .contains("privileged")
        );
        spec["privileged"] = json!(false);
        spec["image"] = json!("outside.example.test/other:1");
        assert!(
            policy
                .check(&descriptor("docker"), &spec, None, &caps)
                .unwrap_err()
                .contains("image")
        );
        policy.processes = ProcessRules {
            templates: vec!["team-tools".into()],
            accounts: vec!["orch-agent".into()],
            programs: vec!["/opt/team/prepare".into()],
        };
        let host = json!({"run_as":"orch-agent","after_up":[{"name":"prepare","command":["/opt/team/prepare"]}]});
        assert!(
            policy
                .check(
                    &descriptor("host"),
                    &host,
                    Some("team-tools"),
                    &Default::default()
                )
                .is_ok()
        );
        assert!(
            policy
                .check(
                    &descriptor("host"),
                    &host,
                    Some("other"),
                    &Default::default()
                )
                .unwrap_err()
                .contains("template")
        );
        assert!(
            policy
                .check(
                    &descriptor("host"),
                    &json!({"run_as":"root"}),
                    Some("team-tools"),
                    &Default::default()
                )
                .unwrap_err()
                .contains("account")
        );
    }
}
