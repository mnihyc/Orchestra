//! Machine-policy schema for the Kubernetes connection.

use orch_driver_contract::driver::DriverError;
use orch_types::Json;
use serde_json::json;

use crate::config::KubernetesConfig;

/// The same typed connection object with constraints that the form can report before activation.
pub fn configuration_schema() -> Result<Json, DriverError> {
    let mut schema =
        serde_json::to_value(schemars::schema_for!(KubernetesConfig)).map_err(|_| {
            DriverError::Internal("the Kubernetes configuration schema cannot be written".into())
        })?;
    for path in [
        "/properties/namespace",
        "/properties/storage_class",
        "/properties/volume_request",
        "/$defs/RuntimeClass/properties/runtime",
        "/$defs/RuntimeClass/properties/class_name",
        "/properties/image_pull_secrets/items",
    ] {
        let field = schema
            .pointer_mut(path)
            .and_then(Json::as_object_mut)
            .ok_or_else(|| {
                DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
            })?;
        field.insert("minLength".into(), json!(1));
        field.insert("pattern".into(), json!("\\S"));
    }
    // Match the Kubernetes v1.35 resource-name validators, including their
    // total subdomain bound rather than adding a narrower per-label bound.
    const DNS_LABEL: &str = r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?$";
    const DNS_SUBDOMAIN: &str =
        r"^[a-z0-9]([-a-z0-9]*[a-z0-9])?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$";
    for (path, pattern, max_length) in [
        ("/properties/namespace", DNS_LABEL, 63),
        ("/properties/storage_class", DNS_SUBDOMAIN, 253),
        (
            "/$defs/RuntimeClass/properties/class_name",
            DNS_SUBDOMAIN,
            253,
        ),
        ("/properties/image_pull_secrets/items", DNS_SUBDOMAIN, 253),
    ] {
        let field = schema
            .pointer_mut(path)
            .and_then(Json::as_object_mut)
            .ok_or_else(|| {
                DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
            })?;
        field.insert("pattern".into(), json!(pattern));
        field.insert("maxLength".into(), json!(max_length));
    }
    let storage_request = schema
        .pointer_mut("/properties/volume_request")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| {
            DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
        })?;
    storage_request.insert("pattern".into(), json!(r"^\+?(?:0*[1-9][0-9]*(?:\.[0-9]*)?|[0-9]*\.[0-9]*[1-9][0-9]*)(?:n|u|m|k|M|G|T|P|E|Ki|Mi|Gi|Ti|Pi|Ei|[eE][+-]?[0-9]+)?$"));
    let image = schema
        .pointer_mut("/properties/tool_stager_image")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| {
            DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
        })?;
    image.insert("pattern".into(), json!("^[^@\\s]+@sha256:[0-9a-f]{64}$"));
    let kubeconfig = schema
        .pointer_mut("/properties/kubeconfig")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| {
            DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
        })?;
    kubeconfig.insert("pattern".into(), json!("^/"));
    let classes = schema
        .pointer_mut("/properties/runtime_classes")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| {
            DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
        })?;
    classes.insert("minItems".into(), json!(1));
    // The shared boundary enum also describes an unbounded host driver; a
    // managed Kubernetes runtime must always declare an isolation boundary.
    let boundary = schema
        .pointer_mut("/$defs/RuntimeClass/properties/boundary")
        .and_then(Json::as_object_mut)
        .ok_or_else(|| {
            DriverError::Internal("the Kubernetes configuration schema is incomplete".into())
        })?;
    boundary.insert(
        "enum".into(),
        json!(["shared_kernel", "user_space_kernel", "own_kernel"]),
    );
    Ok(schema)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_cluster_configuration_is_rejected_at_the_fields_it_needs() {
        let schema = configuration_schema().unwrap();
        let validator = jsonschema::validator_for(&schema).unwrap();
        let valid = json!({"kubeconfig": null, "namespace": "orch", "storage_class": "local-path",
            "volume_request": "10Gi", "runtime_classes": [{"runtime": "runc", "class_name": null}],
            "tool_stager_image": format!("registry.example/stager@sha256:{}", "a".repeat(64))});
        assert!(validator.is_valid(&valid));
        let mut bad_names = valid.clone();
        bad_names["namespace"] = json!("Upper.case");
        bad_names["storage_class"] = json!("bad..class");
        bad_names["runtime_classes"][0]["class_name"] = json!("wrong/name");
        bad_names["image_pull_secrets"] = json!(["bad_secret"]);
        let paths: Vec<_> = validator
            .iter_errors(&bad_names)
            .map(|fault| fault.instance_path.to_string())
            .collect();
        for path in [
            "/namespace",
            "/storage_class",
            "/runtime_classes/0/class_name",
            "/image_pull_secrets/0",
        ] {
            assert!(
                paths.iter().any(|found| found == path),
                "missing name fault at {path}: {paths:?}"
            );
        }
        let mut unbounded = valid.clone();
        unbounded["runtime_classes"][0]["boundary"] = json!("none");
        assert!(!validator.is_valid(&unbounded));
        let mut invalid = valid;
        invalid["namespace"] = json!(" ");
        invalid["kubeconfig"] = json!("relative");
        invalid["runtime_classes"] = json!([]);
        invalid["volume_request"] = json!("0");
        invalid["tool_stager_image"] = json!("registry.example/stager:latest");
        let paths: Vec<_> = validator
            .iter_errors(&invalid)
            .map(|fault| fault.instance_path.to_string())
            .collect();
        for path in [
            "/namespace",
            "/kubeconfig",
            "/runtime_classes",
            "/volume_request",
            "/tool_stager_image",
        ] {
            assert!(
                paths.iter().any(|found| found == path),
                "missing fault at {path}: {paths:?}"
            );
        }
    }
}
