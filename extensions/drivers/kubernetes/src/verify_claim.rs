//! Verify an admitted PVC from its raw API representation, including unknown fields.

use k8s_openapi::apimachinery::pkg::api::resource::Quantity;
use orch_driver_contract::driver::DriverError;
use orch_types::Json;

fn refused() -> DriverError {
    DriverError::Unavailable("the cluster changed the persistent claim's storage settings".into())
}

/// `bound_volume` is an independently verified PV name, never a value trusted from this claim alone.
/// Ownership, namespace and claim UID are checked separately before using this result.
pub(crate) fn same_spec(
    claim: &Json,
    storage_class: &str,
    requested: &str,
    bound_volume: Option<&str>,
) -> Result<(), DriverError> {
    let spec = claim
        .get("spec")
        .and_then(Json::as_object)
        .ok_or_else(refused)?;
    if spec.keys().any(|key| {
        ![
            "accessModes",
            "storageClassName",
            "resources",
            "volumeMode",
            "volumeName",
            "selector",
            "dataSource",
            "dataSourceRef",
            "volumeAttributesClassName",
        ]
        .contains(&key.as_str())
    }) {
        return Err(refused());
    }
    if spec.get("accessModes") != Some(&serde_json::json!(["ReadWriteOnce"]))
        || spec.get("storageClassName").and_then(Json::as_str) != Some(storage_class)
        || spec
            .get("volumeMode")
            .is_some_and(|mode| !mode.is_null() && mode.as_str() != Some("Filesystem"))
    {
        return Err(refused());
    }
    for key in [
        "selector",
        "dataSource",
        "dataSourceRef",
        "volumeAttributesClassName",
    ] {
        if spec.get(key).is_some_and(|value| !value.is_null()) {
            return Err(refused());
        }
    }
    let actual_volume = match spec.get("volumeName") {
        None | Some(Json::Null) => None,
        Some(Json::String(name)) if !name.is_empty() => Some(name.as_str()),
        _ => return Err(refused()),
    };
    if actual_volume != bound_volume {
        return Err(refused());
    }
    let resources = spec
        .get("resources")
        .and_then(Json::as_object)
        .ok_or_else(refused)?;
    if resources
        .keys()
        .any(|key| key != "requests" && key != "limits")
        || resources.get("limits").is_some_and(|limits| {
            !limits.is_null() && limits.as_object().is_none_or(|limits| !limits.is_empty())
        })
    {
        return Err(refused());
    }
    let requests = resources
        .get("requests")
        .and_then(Json::as_object)
        .ok_or_else(refused)?;
    if requests.len() != 1 {
        return Err(refused());
    }
    let actual = requests
        .get("storage")
        .and_then(Json::as_str)
        .ok_or_else(refused)?;
    if !crate::capacity::at_least(&Quantity(actual.into()), &Quantity(requested.into()))? {
        return Err(refused());
    }
    Ok(())
}

/// Confirm the exact bound PV refers back to this PVC incarnation before accepting its name.
/// The caller must retain and compare the returned PV UID on later reads.
pub(crate) fn bound_pv(
    pv: &Json,
    pv_name: &str,
    namespace: &str,
    claim_name: &str,
    claim_uid: &str,
    storage_class: &str,
    requested: &str,
) -> Result<String, DriverError> {
    let uid = pv
        .pointer("/metadata/uid")
        .and_then(Json::as_str)
        .filter(|uid| !uid.is_empty())
        .ok_or_else(refused)?;
    if pv.pointer("/metadata/name").and_then(Json::as_str) != Some(pv_name)
        || pv
            .pointer("/metadata/deletionTimestamp")
            .is_some_and(|value| !value.is_null())
        || pv.pointer("/status/phase").and_then(Json::as_str) != Some("Bound")
        || pv.pointer("/spec/storageClassName").and_then(Json::as_str) != Some(storage_class)
        || pv
            .pointer("/spec/claimRef/namespace")
            .and_then(Json::as_str)
            != Some(namespace)
        || pv.pointer("/spec/claimRef/name").and_then(Json::as_str) != Some(claim_name)
        || claim_uid.is_empty()
        || pv.pointer("/spec/claimRef/uid").and_then(Json::as_str) != Some(claim_uid)
        || pv
            .pointer("/spec/accessModes")
            .and_then(Json::as_array)
            .is_none_or(|modes| {
                !modes
                    .iter()
                    .any(|mode| mode.as_str() == Some("ReadWriteOnce"))
            })
        || pv
            .pointer("/spec/volumeMode")
            .is_some_and(|mode| !mode.is_null() && mode.as_str() != Some("Filesystem"))
    {
        return Err(refused());
    }
    let capacity = pv
        .pointer("/spec/capacity/storage")
        .and_then(Json::as_str)
        .ok_or_else(refused)?;
    if !crate::capacity::at_least(&Quantity(capacity.into()), &Quantity(requested.into()))? {
        return Err(refused());
    }
    Ok(uid.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn claim() -> Json {
        json!({"spec":{"accessModes":["ReadWriteOnce"], "storageClassName":"local-path", "volumeMode":"Filesystem",
            "resources":{"requests":{"storage":"1536Mi"}}}})
    }

    #[test]
    fn only_the_expected_storage_request_and_an_independently_verified_binding_are_accepted() {
        let claim = claim();
        assert!(same_spec(&claim, "local-path", "1.5Gi", None).is_ok());
        assert!(same_spec(&claim, "local-path", "2Gi", None).is_err());
        assert!(same_spec(&claim, "other-class", "1Gi", None).is_err());
        let mut bound = claim;
        bound["spec"]["volumeName"] = json!("pv-one");
        assert!(same_spec(&bound, "local-path", "1Gi", None).is_err());
        assert!(same_spec(&bound, "local-path", "1Gi", Some("pv-other")).is_err());
        assert!(same_spec(&bound, "local-path", "1Gi", Some("pv-one")).is_ok());
    }

    #[test]
    fn injected_sources_selectors_block_mode_and_unknown_fields_are_refused() {
        for (key, value) in [
            (
                "dataSource",
                json!({"kind":"PersistentVolumeClaim","name":"foreign"}),
            ),
            (
                "dataSourceRef",
                json!({"kind":"PersistentVolumeClaim","name":"foreign"}),
            ),
            ("selector", json!({"matchLabels":{"foreign":"true"}})),
            ("volumeMode", json!("Block")),
            ("futureField", json!({"unrecognized":true})),
        ] {
            let mut altered = claim();
            altered["spec"][key] = value;
            assert!(
                same_spec(&altered, "local-path", "1Gi", None).is_err(),
                "{key}"
            );
        }
        let mut altered = claim();
        altered["spec"]["resources"]["requests"]["unexpected"] = json!("1");
        assert!(same_spec(&altered, "local-path", "1Gi", None).is_err());
    }

    #[test]
    fn a_dynamic_claim_read_keeps_unknown_spec_fields_for_the_check() {
        let mut original = claim();
        original["apiVersion"] = json!("v1");
        original["kind"] = json!("PersistentVolumeClaim");
        original["metadata"] = json!({"name":"fixture"});
        original["spec"]["futureField"] = json!({"unrecognized":true});
        let dynamic: kube::api::DynamicObject = serde_json::from_value(original).unwrap();
        let read = serde_json::to_value(dynamic).unwrap();
        assert_eq!(read["spec"]["futureField"]["unrecognized"], true);
        assert!(same_spec(&read, "local-path", "1Gi", None).is_err());
    }

    #[test]
    fn a_bound_volume_must_point_back_to_the_exact_claim_and_have_enough_storage() {
        let pv = json!({"metadata":{"name":"pv-one","uid":"pv-uid"}, "status":{"phase":"Bound"},
            "spec":{"storageClassName":"local-path", "volumeMode":"Filesystem", "accessModes":["ReadWriteOnce"], "capacity":{"storage":"1Gi"},
                "claimRef":{"namespace":"orch", "name":"claim-one", "uid":"claim-uid"}}});
        assert_eq!(
            bound_pv(
                &pv,
                "pv-one",
                "orch",
                "claim-one",
                "claim-uid",
                "local-path",
                "1Gi"
            )
            .unwrap(),
            "pv-uid"
        );
        assert!(
            bound_pv(
                &pv,
                "pv-one",
                "orch",
                "claim-one",
                "other-uid",
                "local-path",
                "1Gi"
            )
            .is_err()
        );
        assert!(
            bound_pv(
                &pv,
                "pv-one",
                "orch",
                "claim-one",
                "claim-uid",
                "local-path",
                "2Gi"
            )
            .is_err()
        );
        let mut replaced = pv;
        replaced["metadata"]["deletionTimestamp"] = json!("2026-01-01T00:00:00Z");
        assert!(
            bound_pv(
                &replaced,
                "pv-one",
                "orch",
                "claim-one",
                "claim-uid",
                "local-path",
                "1Gi"
            )
            .is_err()
        );
    }
}
