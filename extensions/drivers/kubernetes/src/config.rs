//! Cluster connection, storage and runtime-class configuration.

use std::collections::BTreeSet;
use std::path::PathBuf;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The same declared boundary used by all managed runtime classes.
pub use orch_driver_contract::isolation::Boundary as RuntimeBoundary;

/// An admitted Kubernetes class and the runtime it represents.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClass {
    /// Runtime name shown and selected by Orch.
    pub runtime: String,
    /// Kubernetes RuntimeClass name; absent selects the cluster's default runtime.
    #[serde(default)]
    pub class_name: Option<String>,
    /// The boundary declared by the operator; undeclared boundaries are shared-kernel.
    #[serde(default)]
    pub boundary: RuntimeBoundary,
}

/// Operator-owned settings for one Kubernetes driver connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct KubernetesConfig {
    /// An absolute kubeconfig path; absent uses only the in-cluster identity.
    #[serde(default)]
    pub kubeconfig: Option<PathBuf>,
    /// Namespace for this driver's owned objects.
    pub namespace: String,
    /// StorageClass used for new claims.
    pub storage_class: String,
    /// Kubernetes storage quantity requested for each new claim, chosen by the operator.
    pub volume_request: String,
    /// The classes the operator permits; discovery must also confirm named classes exist.
    pub runtime_classes: Vec<RuntimeClass>,
    /// Names of existing namespace-local image pull secrets.
    #[serde(default)]
    pub image_pull_secrets: Vec<String>,
    /// Immutable trusted image used to stage verified connector and tool builds.
    pub tool_stager_image: String,
}

// Kubernetes' DNS-1123 name syntax permits lowercase ASCII digits at either end.
fn dns_label_syntax(value: &str) -> bool {
    let is_alphanumeric = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let bytes = value.as_bytes();
    bytes.first().copied().is_some_and(is_alphanumeric)
        && bytes.last().copied().is_some_and(is_alphanumeric)
        && bytes
            .iter()
            .all(|byte| is_alphanumeric(*byte) || *byte == b'-')
}

fn dns_label(value: &str) -> bool {
    value.len() <= 63 && dns_label_syntax(value)
}

fn dns_subdomain(value: &str) -> bool {
    value.len() <= 253 && value.split('.').all(dns_label_syntax)
}

/// A configuration cannot safely identify its cluster resources.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ConfigFault {
    /// A kubeconfig must not depend on the node's working directory.
    #[error("kubeconfig must be an absolute path")]
    RelativeKubeconfig,
    /// A required value is blank.
    #[error("{0} must not be empty")]
    Empty(&'static str),
    /// No runtime can be scheduled without an admitted mapping.
    #[error("at least one runtime class must be admitted")]
    NoRuntimeClasses,
    /// Two mappings cannot name the same runtime.
    #[error("a runtime name is declared more than once: {0}")]
    DuplicateRuntime(String),
    /// Two mappings cannot refer to the same native class or default.
    #[error("a Kubernetes runtime class is declared more than once: {0}")]
    DuplicateClass(String),
    /// Mutable tags cannot identify the trusted helper.
    #[error("the tool staging image must be pinned to a sha256 digest")]
    UnpinnedToolImage,
    /// Kubernetes units cannot be the machine itself.
    #[error("a Kubernetes runtime must declare a bounded isolation boundary: {0}")]
    UnboundedRuntime(String),
    /// Native object names must be valid for their Kubernetes resource kind.
    #[error("{0} is not a valid Kubernetes name")]
    InvalidName(&'static str),
    /// A claim cannot be made with an invalid or nonpositive storage request.
    #[error(
        "volume request must be positive, within the Kubernetes quantity range and exact to a thousandth"
    )]
    InvalidVolumeRequest,
}

impl KubernetesConfig {
    /// Checks local configuration before the driver contacts the cluster.
    pub fn check(&self) -> Result<(), Vec<ConfigFault>> {
        let mut faults = Vec::new();
        if self
            .kubeconfig
            .as_ref()
            .is_some_and(|path| !path.is_absolute())
        {
            faults.push(ConfigFault::RelativeKubeconfig);
        }
        for (name, value) in [
            ("namespace", self.namespace.as_str()),
            ("storage class", self.storage_class.as_str()),
            ("volume request", self.volume_request.as_str()),
        ] {
            if value.trim().is_empty() {
                faults.push(ConfigFault::Empty(name));
            }
        }
        if !self.namespace.trim().is_empty() && !dns_label(&self.namespace) {
            faults.push(ConfigFault::InvalidName("namespace"));
        }
        if !self.storage_class.trim().is_empty() && !dns_subdomain(&self.storage_class) {
            faults.push(ConfigFault::InvalidName("storage class"));
        }
        if !self.volume_request.trim().is_empty()
            && !crate::capacity::positive(&k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                self.volume_request.clone(),
            ))
        {
            faults.push(ConfigFault::InvalidVolumeRequest);
        }
        if self.runtime_classes.is_empty() {
            faults.push(ConfigFault::NoRuntimeClasses);
        }
        let mut runtimes = BTreeSet::new();
        let mut classes = BTreeSet::new();
        for class in &self.runtime_classes {
            if class.boundary == RuntimeBoundary::Unbounded {
                faults.push(ConfigFault::UnboundedRuntime(class.runtime.clone()));
            }
            if class.runtime.trim().is_empty() {
                faults.push(ConfigFault::Empty("runtime name"));
            }
            if !runtimes.insert(&class.runtime) {
                faults.push(ConfigFault::DuplicateRuntime(class.runtime.clone()));
            }
            if class
                .class_name
                .as_ref()
                .is_some_and(|name| name.trim().is_empty())
            {
                faults.push(ConfigFault::Empty("runtime class name"));
            }
            if class
                .class_name
                .as_ref()
                .is_some_and(|name| !name.trim().is_empty() && !dns_subdomain(name))
            {
                faults.push(ConfigFault::InvalidName("runtime class name"));
            }
            if !classes.insert(&class.class_name) {
                faults.push(ConfigFault::DuplicateClass(
                    class.class_name.clone().unwrap_or_else(|| "default".into()),
                ));
            }
        }
        if self
            .image_pull_secrets
            .iter()
            .any(|name| name.trim().is_empty())
        {
            faults.push(ConfigFault::Empty("image pull secret"));
        }
        if self
            .image_pull_secrets
            .iter()
            .any(|name| !name.trim().is_empty() && !dns_subdomain(name))
        {
            faults.push(ConfigFault::InvalidName("image pull secret"));
        }
        // A content digest makes a trusted helper immutable even when registry tags move.
        const SHA256_HEX_DIGITS: usize = 64;
        let pinned =
            self.tool_stager_image
                .rsplit_once("@sha256:")
                .is_some_and(|(image, digest)| {
                    !image.trim().is_empty()
                        && !image.contains('@')
                        && !image.chars().any(char::is_whitespace)
                        && digest.len() == SHA256_HEX_DIGITS
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                });
        if !pinned {
            faults.push(ConfigFault::UnpinnedToolImage);
        }
        if faults.is_empty() {
            Ok(())
        } else {
            Err(faults)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn configuration() -> KubernetesConfig {
        serde_json::from_value(json!({
            "namespace": "work",
            "storage_class": "local-path",
            "volume_request": "10Gi",
            "runtime_classes": [
                {"runtime": "runc"},
                {"runtime": "gvisor", "class_name": "gvisor", "boundary": "user_space_kernel"},
                {"runtime": "kata", "class_name": "kata-qemu", "boundary": "own_kernel"}
            ],
            "tool_stager_image": format!("registry.example/stager@sha256:{}", "a".repeat(64))
        }))
        .unwrap()
    }

    #[test]
    fn an_undeclared_boundary_is_treated_as_a_shared_kernel() {
        let config = configuration();
        assert_eq!(config.check(), Ok(()));
        assert_eq!(
            config.runtime_classes[0].boundary,
            RuntimeBoundary::SharedKernel
        );
        assert_eq!(config.kubeconfig, None);
    }

    #[test]
    fn ambiguous_classes_and_mutable_helper_images_are_refused() {
        let mut config = configuration();
        config
            .runtime_classes
            .push(config.runtime_classes[0].clone());
        config.tool_stager_image = "registry.example/stager:latest".into();
        let faults = config.check().unwrap_err();
        assert!(faults.contains(&ConfigFault::DuplicateRuntime("runc".into())));
        assert!(faults.contains(&ConfigFault::DuplicateClass("default".into())));
        assert!(faults.contains(&ConfigFault::UnpinnedToolImage));
        config.tool_stager_image = format!("registry.example/stager@sha256:{}", "A".repeat(64));
        assert!(
            config
                .check()
                .unwrap_err()
                .contains(&ConfigFault::UnpinnedToolImage)
        );
        config.tool_stager_image =
            format!("user@registry.example/stager@sha256:{}", "a".repeat(64));
        assert!(
            config
                .check()
                .unwrap_err()
                .contains(&ConfigFault::UnpinnedToolImage)
        );
    }

    #[test]
    fn a_kubernetes_runtime_cannot_claim_to_be_unbounded() {
        let mut config = configuration();
        config.runtime_classes[0].boundary = RuntimeBoundary::Unbounded;
        assert!(
            config
                .check()
                .unwrap_err()
                .contains(&ConfigFault::UnboundedRuntime("runc".into()))
        );
    }

    #[test]
    fn native_names_follow_their_kubernetes_resource_syntax() {
        let mut config = configuration();
        config.namespace = "123-space".into();
        config.storage_class = "storage.example-1".into();
        config.runtime_classes[1].class_name = Some("class.example-1".into());
        config.image_pull_secrets = vec!["registry.example-1".into()];
        assert!(config.check().is_ok());
        config.namespace = "Upper.case".into();
        config.storage_class = "bad..class".into();
        config.runtime_classes[1].class_name = Some("wrong/name".into());
        config.image_pull_secrets = vec!["bad_secret".into()];
        let faults = config.check().unwrap_err();
        for field in [
            "namespace",
            "storage class",
            "runtime class name",
            "image pull secret",
        ] {
            assert!(faults.contains(&ConfigFault::InvalidName(field)));
        }
        assert!(!dns_label(&"a".repeat(64)));
        assert!(!dns_subdomain(&"a".repeat(254)));
        assert!(
            dns_subdomain(&"a".repeat(64)),
            "the API's subdomain check limits total length, not each label"
        );
    }

    #[test]
    fn a_storage_request_must_be_a_positive_quantity() {
        let mut config = configuration();
        for value in [
            "0",
            "-1Gi",
            "a lot",
            "9999999999999999999999999999999999999999999999Gi",
        ] {
            config.volume_request = value.into();
            assert!(
                config
                    .check()
                    .unwrap_err()
                    .contains(&ConfigFault::InvalidVolumeRequest)
            );
        }
        config.volume_request = "1.5Gi".into();
        assert!(config.check().is_ok());
    }

    #[test]
    fn the_operator_must_name_storage_and_at_least_one_runtime() {
        let mut config = configuration();
        config.kubeconfig = Some("relative".into());
        config.storage_class.clear();
        config.volume_request.clear();
        config.runtime_classes.clear();
        let faults = config.check().unwrap_err();
        assert!(faults.contains(&ConfigFault::RelativeKubeconfig));
        assert!(faults.contains(&ConfigFault::Empty("storage class")));
        assert!(faults.contains(&ConfigFault::Empty("volume request")));
        assert!(faults.contains(&ConfigFault::NoRuntimeClasses));
        assert!(serde_json::from_value::<KubernetesConfig>(json!({"namesapce": "typo"})).is_err());
    }
}
