//! Managed connector launch paths and private one-run files.

use std::collections::BTreeMap;

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::mounts::AttachedMount;
use orch_driver_contract::runtime::{LAUNCH_VARIABLES, RuntimeKind, RuntimeSpec, mounts_file};
use orch_plugin_contract::Secret;

use crate::admission::UnitPlan;

/// The connector's executable and tools are staged independently of the requested image.
pub(crate) const CONNECTOR: &str = "/opt/orch/bin/orch-connector";
/// The tool cache is exposed read-only to the unit.
const TOOLS: &str = "/opt/orch/tools";
/// The private connector identity stays on the state claim.
const STATE: &str = "/var/lib/orch";
/// One run's launch files are kept separately from persistent claims.
const CA_FILE: &str = "/run/orch/ca.pem";
const ENROLL_FILE: &str = "/run/orch/enroll";
const MOUNTS_FILE: &str = "/run/orch/mounts.json";

/// Files passed privately for the current run; the enrollment token's debug representation is redacted.
#[derive(Debug, Clone)]
pub(crate) struct LaunchFiles {
    pub ca: String,
    pub enrollment: Secret,
    /// User-supplied environment is private too: it may contain credentials.
    pub environment: Secret,
    pub mounts: Option<String>,
}

/// Values visible to the connector and the separate private launch files it reads.
#[derive(Debug, Clone)]
pub(crate) struct LaunchPlan {
    pub environment: BTreeMap<String, String>,
    pub files: LaunchFiles,
}

/// The same rules apply before creating a Pod and when the supervisor reads its private file.
pub(crate) fn valid_variable(name: &str, value: &str) -> bool {
    !name.is_empty()
        && !name.contains(['=', '\0'])
        && !value.contains('\0')
        && !LAUNCH_VARIABLES
            .iter()
            .any(|variable| variable.name == name)
        && !matches!(name, "ORCH_HARNESS_HOME" | "ORCH_HARNESS_HOME_SOURCE")
}

/// Builds launch inputs without placing the enrollment token in an environment variable.
pub(crate) fn prepare(
    spec: &RuntimeSpec,
    plan: &UnitPlan,
    attached: &[AttachedMount],
) -> Result<LaunchPlan, DriverError> {
    let launch = spec.launch();
    let mut environment = launch.values;
    for (key, value) in &spec.env {
        if !valid_variable(key, value) {
            return Err(DriverError::InvalidSpec(
                "a private environment variable is invalid or reserved".into(),
            ));
        }
        environment.remove(key);
    }
    let private_environment = serde_json::to_string(&spec.env).map_err(|_| {
        DriverError::InvalidSpec("the environment variables could not be prepared".into())
    })?;
    for (name, value) in [
        ("ORCH_CA_FILE", CA_FILE),
        ("ORCH_ENROLL_FILE", ENROLL_FILE),
        ("ORCH_DATA_DIR", STATE),
        ("ORCH_TOOLS_DIR", TOOLS),
    ] {
        environment.insert(name.into(), value.into());
    }
    let mounts = if spec.kind == RuntimeKind::Environment {
        environment.insert("ORCH_WORKSPACE_DIR".into(), plan.workspace.clone());
        environment.insert("ORCH_HOME_DIR".into(), plan.home.clone());
        environment.insert("ORCH_MOUNTS_FILE".into(), MOUNTS_FILE.into());
        Some(mounts_file(attached))
    } else {
        None
    };
    Ok(LaunchPlan {
        environment,
        files: LaunchFiles {
            ca: launch.ca_file,
            enrollment: launch.enroll_file,
            environment: Secret::new(private_environment),
            mounts,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_driver_contract::runtime::{RuntimeKind, VolumeRole, unit_labels, unit_name};
    use serde_json::json;

    #[test]
    fn enrollment_is_only_a_private_file_and_each_managed_path_names_the_units_data() {
        let id = orch_types::new_id();
        let spec: RuntimeSpec = serde_json::from_value(json!({
            "runtime_id": id, "kind": "environment", "name": unit_name(RuntimeKind::Environment, &id),
            "driver": "kubernetes", "spec": {"image":"example/base"}, "resources": {}, "roles": ["environment"],
            "gateway_url":"wss://gateway.example:9443", "enroll_token":"synthetic-once", "ca_cert":"test-ca",
            "tools": [], "volumes": {"workspace":{"name":"work","storage":null},
                "home":{"name":"home","storage":null}, "state":{"name":"state","storage":null}},
            "mounts": [], "env": {"LANG":"C"}, "labels": unit_labels("system", RuntimeKind::Environment, &id)
        })).unwrap();
        let plan = UnitPlan {
            image: "example/base".into(),
            class_name: None,
            runtime: "runc".into(),
            resources: json!({}),
            security_context: json!({}),
            claims: BTreeMap::from([
                (VolumeRole::Workspace, "work".into()),
                (VolumeRole::Home, "home".into()),
                (VolumeRole::State, "state".into()),
            ]),
            workspace: "/workspace".into(),
            home: "/home/agent".into(),
        };
        let launch = prepare(&spec, &plan, &[]).unwrap();
        assert_eq!(launch.environment["ORCH_ENROLL_FILE"], ENROLL_FILE);
        assert_eq!(launch.environment["ORCH_WORKSPACE_DIR"], "/workspace");
        assert_eq!(launch.environment["ORCH_HOME_DIR"], "/home/agent");
        assert_eq!(launch.files.mounts.as_deref(), Some("[]"));
        assert_eq!(launch.files.enrollment.expose(), "synthetic-once");
        assert!(!launch.environment.contains_key("LANG"));
        assert_eq!(
            serde_json::from_str::<BTreeMap<String, String>>(launch.files.environment.expose())
                .unwrap()["LANG"],
            "C"
        );
        assert!(!format!("{launch:?}").contains("synthetic-once"));
        assert!(
            !serde_json::to_string(&launch.environment)
                .unwrap()
                .contains("synthetic-once")
        );
        assert_eq!(CONNECTOR, "/opt/orch/bin/orch-connector");
        for (name, value) in [
            ("", "synthetic-secret"),
            ("BAD=NAME", "synthetic-secret"),
            ("NAME", "synthetic-secret\0"),
            ("ORCH_DATA_DIR", "synthetic-secret"),
        ] {
            let mut invalid = spec.clone();
            invalid.env.clear();
            invalid.env.insert(name.into(), value.into());
            let error = prepare(&invalid, &plan, &[]).unwrap_err();
            assert!(!error.to_string().contains("synthetic-secret"));
        }
    }
}
