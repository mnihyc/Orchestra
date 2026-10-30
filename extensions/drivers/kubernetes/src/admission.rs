//! Admission of a managed unit before any cluster object is created.

use std::collections::{BTreeMap, BTreeSet};

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::{MountKind, MountMode, check_targets};
use orch_driver_contract::runtime::{Resources, RuntimeSpec, VolumeRole};
use orch_driver_contract::sources::Source;
use orch_types::Json;
use serde_json::json;

use crate::config::KubernetesConfig;

/// The paths used when a template does not place its data elsewhere.
const WORKSPACE: &str = "/workspace";
const HOME: &str = "/home/agent";
/// The trusted runtime and launch files are never covered by a data mount.
const RESERVED: [&str; 3] = ["/opt/orch", "/run/orch", "/var/lib/orch"];

/// The cluster-neutral choices already checked for one Pod.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitPlan {
    /// OCI image reference, with the selected digest if one is given.
    pub image: String,
    /// Selected Kubernetes RuntimeClass, absent for the worker's default runtime.
    pub class_name: Option<String>,
    /// The runtime's name in Orch.
    pub runtime: String,
    /// Kubernetes requests and limits, using native quantity strings.
    pub resources: Json,
    /// Kubernetes container security context.
    pub security_context: Json,
    /// PVC names for the workspace, home and state.
    pub claims: BTreeMap<VolumeRole, String>,
    /// The workspace and home mount paths.
    pub workspace: String,
    /// The user's home mount path.
    pub home: String,
}

fn invalid(message: impl Into<String>) -> DriverError {
    DriverError::InvalidSpec(message.into())
}

fn unsupported(message: impl Into<String>) -> DriverError {
    DriverError::Unsupported(message.into())
}

fn text<'a>(value: &'a Json, key: &str, default: &'a str) -> Result<&'a str, DriverError> {
    match value.get(key) {
        None | Some(Json::Null) => Ok(default),
        Some(Json::String(text)) if !text.is_empty() => Ok(text),
        Some(_) => Err(invalid(format!("{key} must be a nonempty string"))),
    }
}

fn bool_field(value: &Json, key: &str) -> Result<bool, DriverError> {
    match value.get(key) {
        None => Ok(false),
        Some(Json::Bool(flag)) => Ok(*flag),
        Some(_) => Err(invalid(format!("security.{key} must be a boolean"))),
    }
}

fn capabilities(value: &Json, key: &str) -> Result<Vec<String>, DriverError> {
    let Some(caps) = value.get(key) else {
        return Ok(Vec::new());
    };
    let caps = caps
        .as_array()
        .ok_or_else(|| invalid(format!("security.{key} must be a list")))?;
    caps.iter()
        .map(|cap| {
            let name = cap
                .as_str()
                .filter(|name| !name.is_empty())
                .ok_or_else(|| invalid(format!("security.{key} contains an invalid capability")))?;
            let name = name.to_ascii_uppercase();
            Ok(name.strip_prefix("CAP_").unwrap_or(&name).to_owned())
        })
        .collect()
}

/// Kubernetes writes quantities canonically; spelling them that way keeps admission comparison exact.
fn cpu_quantity(millis: i64) -> String {
    const MILLIS_PER_CPU: i64 = 1_000;
    if millis % MILLIS_PER_CPU == 0 {
        (millis / MILLIS_PER_CPU).to_string()
    } else {
        format!("{millis}m")
    }
}

fn memory_quantity(mib: i64) -> Result<String, DriverError> {
    const BYTES_PER_MIB: i64 = 1024 * 1024;
    const MIB_PER_GIB: i64 = 1024;
    if mib < 0 || mib.checked_mul(BYTES_PER_MIB).is_none() {
        return Err(invalid("memory is outside the Kubernetes quantity range"));
    }
    if mib == 0 {
        return Ok("0".into());
    }
    for (power, suffix) in [(4, "Ei"), (3, "Pi"), (2, "Ti"), (1, "Gi"), (0, "Mi")] {
        let factor = MIB_PER_GIB.pow(power);
        if mib % factor == 0 {
            return Ok(format!("{}{suffix}", mib / factor));
        }
    }
    Err(invalid("memory cannot be represented"))
}

fn resource_requirements(
    resources: &Resources,
    requested: Option<&Json>,
) -> Result<Json, DriverError> {
    for (name, value) in [
        ("cpu_millis", resources.cpu_millis),
        ("memory_mb", resources.memory_mb),
    ] {
        if value.is_some_and(|value| value <= 0) {
            return Err(invalid(format!("{name} must be positive")));
        }
    }
    // Kubernetes does not expose these as per-container enforced limits through the Pod API.
    for (name, value) in [
        ("memory_swap_mb", resources.memory_swap_mb),
        ("blkio_weight", resources.blkio_weight),
        ("pids", resources.pids),
        ("disk_gb", resources.disk_gb),
    ] {
        if value.is_some() {
            return Err(unsupported(format!(
                "Kubernetes cannot enforce the requested {name} limit"
            )));
        }
    }
    let mut limits = serde_json::Map::new();
    if let Some(cpu) = resources.cpu_millis {
        limits.insert("cpu".into(), json!(cpu_quantity(cpu)));
    }
    if let Some(memory) = resources.memory_mb {
        limits.insert("memory".into(), json!(memory_quantity(memory)?));
    }
    // Without an explicit reservation, zero prevents Kubernetes from defaulting a request to
    // its limit. These requests describe scheduling, while Resources describes maxima.
    let mut requests = serde_json::Map::from_iter([
        ("cpu".to_owned(), json!("0")),
        ("memory".to_owned(), json!("0")),
    ]);
    if let Some(requested) = requested {
        let fields = requested
            .as_object()
            .ok_or_else(|| invalid("requests must be an object"))?;
        for (name, value) in fields {
            let (resource, unit, limit) = match name.as_str() {
                "cpu_millis" => ("cpu", "m", resources.cpu_millis),
                "memory_mb" => ("memory", "Mi", resources.memory_mb),
                _ => return Err(invalid("requests has an unknown setting")),
            };
            let amount = value
                .as_i64()
                .filter(|amount| *amount >= 0)
                .ok_or_else(|| invalid(format!("requests.{name} must be a nonnegative integer")))?;
            if limit.is_some_and(|limit| amount > limit) {
                return Err(invalid(format!("requests.{name} exceeds the unit's limit")));
            }
            let quantity = match unit {
                "m" => cpu_quantity(amount),
                "Mi" => memory_quantity(amount)?,
                _ => return Err(invalid("the request unit is unknown")),
            };
            requests.insert(resource.to_owned(), json!(quantity));
        }
    }
    Ok(json!({"requests": requests, "limits": limits}))
}

/// Checks the immutable request and derives only the native settings the driver can uphold.
pub fn plan(
    spec: &RuntimeSpec,
    config: &KubernetesConfig,
    discovered: &BTreeSet<String>,
) -> Result<UnitPlan, DriverError> {
    if spec.driver != "kubernetes" {
        return Err(invalid("the request names another driver"));
    }
    if let Err(faults) = config.check() {
        return Err(invalid(
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    if let Err(faults) = spec.check() {
        return Err(invalid(
            faults
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
        ));
    }
    spec.capabilities.check()?;
    if spec.kept_image.is_some() {
        return Err(unsupported(
            "Kubernetes cannot restore a kept writable image",
        ));
    }
    let network = spec
        .spec
        .get("network")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !network.is_object() {
        return Err(invalid("network must be an object"));
    }
    if network.as_object().is_some_and(|fields| {
        fields
            .keys()
            .any(|key| !["mode", "egress", "dns"].contains(&key.as_str()))
    }) {
        return Err(invalid("network has an unknown setting"));
    }
    let mode = text(&network, "mode", "bridge")?;
    let egress = text(&network, "egress", "open")?;
    if !["bridge", "none"].contains(&mode) || !["open", "restricted"].contains(&egress) {
        return Err(invalid("the network mode or egress is unknown"));
    }
    if network
        .get("dns")
        .is_some_and(|dns| dns.as_array().is_none_or(|dns| !dns.is_empty()))
    {
        return Err(unsupported("custom DNS is unavailable on Kubernetes"));
    }
    if spec.network_policy.restricted() || egress == "restricted" || mode == "none" {
        return Err(unsupported(
            "network.restricted_unavailable: cluster network isolation has not been verified",
        ));
    }
    if spec
        .spec
        .get("mounts")
        .is_some_and(|mounts| mounts.as_array().is_none_or(|mounts| !mounts.is_empty()))
    {
        return Err(unsupported(
            "template-defined mounts are unavailable on Kubernetes",
        ));
    }
    let image = match Source::from_spec(&spec.spec).map_err(|fault| invalid(fault.to_string()))? {
        Source::Image { reference, digest } if !reference.trim().is_empty() => {
            if let Some(digest) = digest {
                // OCI sha256 digests have 64 lower-case hexadecimal digits.
                const DIGEST_HEX_DIGITS: usize = 64;
                let valid = digest.strip_prefix("sha256:").is_some_and(|hash| {
                    hash.len() == DIGEST_HEX_DIGITS
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                });
                if !valid || reference.contains('@') {
                    return Err(invalid(
                        "the image digest is invalid or was already given in the reference",
                    ));
                }
                format!("{reference}@{digest}")
            } else {
                reference
            }
        }
        Source::Image { .. } => return Err(invalid("the image reference is empty")),
        _ => {
            return Err(unsupported(
                "Kubernetes accepts image references, not builds or archives",
            ));
        }
    };
    let requested = match spec.spec.get("runtime") {
        None | Some(Json::Null) => None,
        Some(Json::String(value)) if !value.is_empty() => Some(value.as_str()),
        Some(_) => return Err(invalid("runtime must be a nonempty string")),
    };
    let class = match requested {
        Some(name) => config
            .runtime_classes
            .iter()
            .find(|class| name == class.runtime),
        None => config
            .runtime_classes
            .iter()
            .find(|class| class.class_name.is_none())
            .or_else(|| config.runtime_classes.first()),
    }
    .ok_or_else(|| unsupported("the requested runtime is not admitted on this cluster"))?;
    if class
        .class_name
        .as_ref()
        .is_some_and(|name| !discovered.contains(name))
    {
        return Err(unsupported(
            "the selected Kubernetes RuntimeClass is not available",
        ));
    }
    let mut claims = BTreeMap::new();
    for (role, volume) in &spec.volumes {
        if volume.directory.is_some() {
            return Err(unsupported(
                "Kubernetes cannot use a node directory as an owned volume",
            ));
        }
        if volume
            .storage
            .as_deref()
            .is_some_and(|name| name != config.storage_class && name != "default")
        {
            return Err(unsupported(
                "the requested storage class is not configured on this cluster",
            ));
        }
        claims.insert(*role, volume.name.clone());
    }
    let workspace = text(&spec.spec, "workspace_mount", WORKSPACE)?.to_owned();
    let home = text(&spec.spec, "home_mount", HOME)?.to_owned();
    orch_driver_contract::mounts::check_target(&workspace).map_err(|e| invalid(e.to_string()))?;
    orch_driver_contract::mounts::check_target(&home).map_err(|e| invalid(e.to_string()))?;
    if !check_targets(
        &[("workspace", workspace.as_str()), ("home", home.as_str())],
        &RESERVED,
    )
    .is_empty()
    {
        return Err(invalid("workspace or home conflicts with a reserved path"));
    }
    let targets: Vec<_> = spec
        .mounts
        .iter()
        .map(|mount| (mount.name.as_str(), mount.target.as_str()))
        .collect();
    let mut reserved = RESERVED.to_vec();
    reserved.extend([workspace.as_str(), home.as_str()]);
    if !check_targets(&targets, &reserved).is_empty() {
        return Err(invalid(
            "a mount target conflicts with a reserved path or another mount",
        ));
    }
    for (index, (_, target)) in targets.iter().enumerate() {
        if targets.iter().skip(index + 1).any(|(_, other)| {
            let (target, other) = (*target, *other);
            target == other
                || target
                    .strip_prefix(other)
                    .is_some_and(|rest| rest.starts_with('/'))
                || other
                    .strip_prefix(target)
                    .is_some_and(|rest| rest.starts_with('/'))
        }) {
            return Err(invalid("mount targets must not overlap"));
        }
    }
    if workspace == home
        || workspace.starts_with(&(home.clone() + "/"))
        || home.starts_with(&(workspace.clone() + "/"))
    {
        return Err(invalid("workspace and home paths overlap"));
    }
    for mount in &spec.mounts {
        if mount.kind != MountKind::Volume {
            return Err(unsupported("Kubernetes cannot attach a host path"));
        }
        if mount.mode == MountMode::Private {
            return Err(unsupported("Kubernetes cannot make a private mount view"));
        }
    }
    // The open-network capability has exactly these two effects. A frozen patch that
    // asks for anything else must not take over the shared workload supervisor.
    let mut network_administration = false;
    for key in spec
        .capabilities
        .base
        .iter()
        .chain(&spec.capabilities.granted)
    {
        if orch_driver_contract::capabilities::gateway_given(key) {
            continue;
        }
        if key != "net_admin" {
            return Err(unsupported(format!(
                "Kubernetes cannot apply capability {key}"
            )));
        }
        let patch = spec
            .capabilities
            .patches
            .get(key)
            .and_then(Json::as_object)
            .ok_or_else(|| invalid("the network administration patch is not an object"))?;
        if patch.len() != 1 || !patch.contains_key("security") {
            return Err(invalid(
                "the network administration patch has an unknown setting",
            ));
        }
        let security = patch
            .get("security")
            .and_then(Json::as_object)
            .ok_or_else(|| invalid("the network administration patch needs security settings"))?;
        if security.len() != 1 || !security.contains_key("cap_add") {
            return Err(invalid(
                "the network administration patch has an unknown security setting",
            ));
        }
        let capability_names: BTreeSet<_> = capabilities(&json!(security), "cap_add")?
            .into_iter()
            .collect();
        if capability_names != BTreeSet::from(["NET_ADMIN".to_owned(), "NET_RAW".to_owned()]) {
            return Err(invalid(
                "the network administration patch must add NET_ADMIN and NET_RAW",
            ));
        }
        network_administration = true;
    }
    // Native grants come from the frozen, authorized capability patch, never
    // directly from a template's arbitrary security additions.
    if let Some(security) = spec.spec.get("security")
        && !capabilities(security, "cap_add")?.is_empty()
    {
        return Err(unsupported(
            "direct native capability additions are unavailable on Kubernetes",
        ));
    }
    let creation = spec.capabilities.creation_spec(&spec.spec, &[]);
    let security = creation
        .get("security")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if !security.is_object() {
        return Err(invalid("security must be an object"));
    }
    if security.as_object().is_some_and(|fields| {
        fields.keys().any(|key| {
            ![
                "read_only_rootfs",
                "no_new_privileges",
                "cap_add",
                "cap_drop",
            ]
            .contains(&key.as_str())
        })
    }) {
        return Err(invalid("security has an unknown setting"));
    }
    let read_only = bool_field(&security, "read_only_rootfs")?;
    let no_new_privileges = match security.get("no_new_privileges") {
        None => true,
        Some(_) => bool_field(&security, "no_new_privileges")?,
    };
    if !no_new_privileges {
        return Err(unsupported(
            "the workload cannot enable privilege escalation",
        ));
    }
    let add = capabilities(&security, "cap_add")?;
    let mut drop = capabilities(&security, "cap_drop")?;
    let allowed = if network_administration {
        BTreeSet::from(["NET_ADMIN".to_owned(), "NET_RAW".to_owned()])
    } else {
        BTreeSet::new()
    };
    if add.iter().cloned().collect::<BTreeSet<_>>() != allowed {
        return Err(unsupported(
            "the requested native capabilities do not match the authorized patch",
        ));
    }
    if !drop.iter().any(|capability| capability == "ALL") {
        drop.push("ALL".into());
    }
    let security_context = json!({"readOnlyRootFilesystem": read_only,
        "allowPrivilegeEscalation": false, "capabilities": {"add": add, "drop": drop}});
    Ok(UnitPlan {
        image,
        class_name: class.class_name.clone(),
        runtime: class.runtime.clone(),
        resources: resource_requirements(&spec.resources, spec.spec.get("requests"))?,
        security_context,
        claims,
        workspace,
        home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{RuntimeBoundary, RuntimeClass};
    use orch_driver_contract::runtime::{RuntimeKind, unit_labels, unit_name};

    fn configuration() -> KubernetesConfig {
        KubernetesConfig {
            kubeconfig: None,
            namespace: "test".into(),
            storage_class: "local-path".into(),
            volume_request: "10Gi".into(),
            runtime_classes: vec![
                RuntimeClass {
                    runtime: "runc".into(),
                    class_name: None,
                    boundary: RuntimeBoundary::SharedKernel,
                },
                RuntimeClass {
                    runtime: "kata".into(),
                    class_name: Some("kata-qemu".into()),
                    boundary: RuntimeBoundary::OwnKernel,
                },
            ],
            image_pull_secrets: Vec::new(),
            tool_stager_image: format!("registry.example/stager@sha256:{}", "a".repeat(64)),
        }
    }

    fn request() -> RuntimeSpec {
        let id = orch_types::new_id();
        serde_json::from_value(json!({
            "runtime_id": id, "kind": "environment", "name": unit_name(RuntimeKind::Environment, &id),
            "driver": "kubernetes", "spec": {"image": "example/base:1", "runtime": "runc"},
            "resources": {"cpu_millis": 2000, "memory_mb": 1024}, "roles": ["environment"],
            "gateway_url": "wss://gateway.example:9443", "enroll_token": "test-token", "ca_cert": "test-ca",
            "tools": [], "volumes": {
                "workspace": {"name": "workspace", "storage": null},
                "home": {"name": "home", "storage": null},
                "state": {"name": "state", "storage": null}
            }, "mounts": [], "env": {}, "labels": unit_labels("test", RuntimeKind::Environment, &id)
        })).unwrap()
    }

    #[test]
    fn an_open_unit_uses_the_admitted_default_and_explicit_nonreserving_requests() {
        let mut spec = request();
        spec.spec["security"] = json!({"read_only_rootfs": true, "no_new_privileges": true, "cap_drop": ["CAP_NET_RAW"]});
        let plan = plan(&spec, &configuration(), &BTreeSet::new()).unwrap();
        assert_eq!(plan.class_name, None);
        assert_eq!(plan.claims[&VolumeRole::Workspace], "workspace");
        assert_eq!(
            plan.resources["limits"],
            json!({"cpu": "2", "memory": "1Gi"})
        );
        assert_eq!(
            plan.resources["requests"],
            json!({"cpu": "0", "memory": "0"})
        );
        assert_eq!(plan.security_context["allowPrivilegeEscalation"], false);
        assert_eq!(
            plan.security_context["capabilities"]["drop"],
            json!(["NET_RAW", "ALL"])
        );
    }

    #[test]
    fn the_native_pod_name_must_match_the_runtime_identity() {
        let mut spec = request();
        spec.name = "some-other-unit".into();
        assert!(matches!(
            plan(&spec, &configuration(), &BTreeSet::new()),
            Err(DriverError::InvalidSpec(_))
        ));
    }

    #[test]
    fn an_ambiguous_cluster_configuration_is_refused_before_planning() {
        let mut config = configuration();
        config
            .runtime_classes
            .push(config.runtime_classes[0].clone());
        assert!(matches!(
            plan(&request(), &config, &BTreeSet::new()),
            Err(DriverError::InvalidSpec(_))
        ));
    }

    #[test]
    fn a_declared_class_must_also_be_discovered() {
        let mut spec = request();
        spec.spec["runtime"] = json!("kata");
        assert!(matches!(
            plan(&spec, &configuration(), &BTreeSet::new()),
            Err(DriverError::Unsupported(_))
        ));
        let found = BTreeSet::from(["kata-qemu".into()]);
        assert_eq!(
            plan(&spec, &configuration(), &found)
                .unwrap()
                .class_name
                .as_deref(),
            Some("kata-qemu")
        );
    }

    #[test]
    fn a_named_first_class_is_the_default_when_the_cluster_default_was_not_admitted() {
        let mut config = configuration();
        config.runtime_classes.remove(0);
        let found = BTreeSet::from(["kata-qemu".into()]);
        let mut spec = request();
        spec.spec.as_object_mut().unwrap().remove("runtime");
        let planned = plan(&spec, &config, &found).unwrap();
        assert_eq!(planned.runtime, "kata");
        assert_eq!(planned.class_name.as_deref(), Some("kata-qemu"));
    }

    #[test]
    fn an_unadmitted_explicit_runtime_never_becomes_the_default() {
        let mut config = configuration();
        config.runtime_classes.remove(0);
        let found = BTreeSet::from(["kata-qemu".into()]);
        assert!(matches!(
            plan(&request(), &config, &found),
            Err(DriverError::Unsupported(_))
        ));
    }

    #[test]
    fn an_unverified_restriction_is_refused_before_it_can_become_open() {
        let mut spec = request();
        spec.network_policy.mode = orch_driver_contract::network::EgressMode::Restricted;
        let error = plan(&spec, &configuration(), &BTreeSet::new()).unwrap_err();
        assert!(error.to_string().contains("network.restricted_unavailable"));
        spec.network_policy.mode = orch_driver_contract::network::EgressMode::Open;
        spec.spec["network"] = json!({"mode": "none"});
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
    }

    #[test]
    fn limits_the_cluster_cannot_enforce_are_not_silently_dropped() {
        let mut spec = request();
        spec.resources.pids = Some(100);
        let error = plan(&spec, &configuration(), &BTreeSet::new()).unwrap_err();
        assert!(error.to_string().contains("pids"));
    }

    #[test]
    fn a_native_capability_and_a_host_directory_are_refused_until_supported() {
        let mut spec = request();
        spec.capabilities.allowed.push("gpu".into());
        spec.capabilities.granted.push("gpu".into());
        spec.capabilities.patches.insert("gpu".into(), json!({}));
        assert!(
            plan(&spec, &configuration(), &BTreeSet::new())
                .unwrap_err()
                .to_string()
                .contains("gpu")
        );
        spec.capabilities.granted.clear();
        spec.volumes
            .get_mut(&VolumeRole::Workspace)
            .unwrap()
            .directory = Some(orch_driver_contract::runtime::DirectoryBinding {
            path: "/srv/data".into(),
            device: None,
            inode: None,
        });
        assert!(
            plan(&spec, &configuration(), &BTreeSet::new())
                .unwrap_err()
                .to_string()
                .contains("node directory")
        );
    }

    #[test]
    fn workspace_and_home_cannot_cover_connector_identity_or_each_other() {
        let mut spec = request();
        spec.spec["workspace_mount"] = json!("/var/lib/orch/identity");
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
        spec.spec["workspace_mount"] = json!("/home/agent/project");
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
    }

    #[test]
    fn nested_mounts_cannot_hide_one_another() {
        use orch_driver_contract::mounts::MountSpec;
        let mut spec = request();
        spec.mounts = ["/data", "/data/child"]
            .into_iter()
            .enumerate()
            .map(|(index, target)| MountSpec {
                name: format!("mount-{index}"),
                kind: MountKind::Volume,
                source: format!("claim-{index}"),
                target: target.into(),
                mode: MountMode::Shared,
                layer: None,
            })
            .collect();
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
    }

    #[test]
    fn capabilities_that_can_take_over_the_supervisor_are_refused() {
        let mut spec = request();
        for capability in [
            "SYS_PTRACE",
            "cap_sys_admin",
            "ALL",
            "SETFCAP",
            "BPF",
            "NET_ADMIN",
        ] {
            spec.spec["security"] = json!({"cap_add": [capability]});
            assert!(matches!(
                plan(&spec, &configuration(), &BTreeSet::new()),
                Err(DriverError::Unsupported(_))
            ));
        }
    }

    #[test]
    fn omitted_security_drops_the_native_bounding_set_and_refuses_escalation() {
        let mut spec = request();
        let planned = plan(&spec, &configuration(), &BTreeSet::new()).unwrap();
        assert_eq!(planned.security_context["allowPrivilegeEscalation"], false);
        assert_eq!(
            planned.security_context["capabilities"]["drop"],
            json!(["ALL"])
        );
        assert_eq!(planned.security_context["capabilities"]["add"], json!([]));
        spec.spec["security"] = json!({"no_new_privileges":false});
        assert!(matches!(
            plan(&spec, &configuration(), &BTreeSet::new()),
            Err(DriverError::Unsupported(_))
        ));
    }

    #[test]
    fn explicit_requests_are_bounded_by_limits_and_never_default_to_them() {
        let mut spec = request();
        spec.spec["requests"] = json!({"cpu_millis": 250, "memory_mb": 256});
        let planned = plan(&spec, &configuration(), &BTreeSet::new()).unwrap();
        assert_eq!(
            planned.resources["requests"],
            json!({"cpu":"250m", "memory":"256Mi"})
        );
        for invalid_request in [
            json!({"cpu_millis": -1}),
            json!({"cpu_millis": 2001}),
            json!({"memory_mb": 1025}),
            json!({"memory_mb": "256"}),
            json!({"disk_gb": 1}),
            json!([]),
        ] {
            spec.spec["requests"] = invalid_request;
            assert!(matches!(
                plan(&spec, &configuration(), &BTreeSet::new()),
                Err(DriverError::InvalidSpec(_))
            ));
        }
    }

    #[test]
    fn an_open_unit_applies_the_frozen_network_administration_patch() {
        let mut spec = request();
        spec.capabilities.allowed.push("net_admin".into());
        spec.capabilities.granted.push("net_admin".into());
        spec.capabilities.patches.insert(
            "net_admin".into(),
            json!({"security":{"cap_add":["NET_ADMIN","NET_RAW"]}}),
        );
        let planned = plan(&spec, &configuration(), &BTreeSet::new()).unwrap();
        assert_eq!(
            planned.security_context["capabilities"]["add"],
            json!(["NET_ADMIN", "NET_RAW"])
        );
        spec.capabilities.patches.insert(
            "net_admin".into(),
            json!({"security":{"cap_add":["NET_ADMIN","SYS_PTRACE"]}}),
        );
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
        spec.capabilities.patches.insert(
            "net_admin".into(),
            json!({"security":{"cap_add":["NET_ADMIN","NET_RAW"]},"privileged":true}),
        );
        assert!(plan(&spec, &configuration(), &BTreeSet::new()).is_err());
    }

    #[test]
    fn quantities_are_canonical_without_rounding_or_overflow() {
        assert_eq!(cpu_quantity(1500), "1500m");
        assert_eq!(cpu_quantity(2000), "2");
        assert_eq!(cpu_quantity(0), "0");
        assert_eq!(memory_quantity(1536).unwrap(), "1536Mi");
        assert_eq!(memory_quantity(1024).unwrap(), "1Gi");
        assert_eq!(memory_quantity(0).unwrap(), "0");
        assert!(memory_quantity(i64::MAX).is_err());
    }
}
