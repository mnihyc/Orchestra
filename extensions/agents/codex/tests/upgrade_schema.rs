//! Published native schemas anchor consumed fields and every managed pin independently of a binary run.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "fixture assertions"
)]

use orch_adapter_codex::{descriptor, process, protocol::RequestId};
use orch_agent_contract::descriptor::HarnessSupport;
use orch_types::Json;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

const VERSIONS: &[&str] = &["0.165.0", "0.167.0", "0.167.2"];
fn directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/upgrades")
}
fn json(path: &Path) -> Json {
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}
fn schema_directory() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("schema")
}
fn schema(version: &str, name: &str) -> Json {
    json(&schema_directory().join(version).join(name))
}

fn resolve<'a>(root: &'a Json, mut value: &'a Json) -> Option<&'a Json> {
    let mut visited = BTreeSet::new();
    while let Some(reference) = value.get("$ref").and_then(Json::as_str) {
        if !visited.insert(reference) {
            return None;
        }
        value = root.pointer(reference.strip_prefix('#')?)?;
    }
    Some(value)
}
fn property<'a>(root: &'a Json, value: &'a Json, name: &str) -> Option<&'a Json> {
    let value = resolve(root, value)?;
    if let Some(found) = value.get("properties").and_then(|fields| fields.get(name)) {
        return Some(found);
    }
    for operator in ["allOf", "anyOf", "oneOf"] {
        for branch in value
            .get(operator)
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            if let Some(found) = property(root, branch, name) {
                return Some(found);
            }
        }
    }
    None
}
fn dotted_property<'a>(root: &'a Json, path: &str) -> Option<&'a Json> {
    path.split('.')
        .try_fold(root, |schema, field| property(root, schema, field))
}
fn method<'a>(root: &'a Json, family: &str, name: &str) -> Option<&'a Json> {
    root["definitions"][family]["oneOf"]
        .as_array()?
        .iter()
        .find(|variant| {
            variant["properties"]["method"]["enum"]
                .as_array()
                .is_some_and(|values| values.iter().any(|value| value == name))
        })
}
type Consumed = BTreeMap<String, BTreeMap<String, Vec<String>>>;
fn consumed() -> Consumed {
    serde_json::from_value(json(&directory().join("consumed-fields.json"))).unwrap()
}
fn check_consumed(root: &Json) -> Result<(), String> {
    for (family, methods) in consumed() {
        for (name, fields) in methods {
            let variant = method(root, &family, &name)
                .ok_or_else(|| format!("native method removed or renamed: {family}/{name}"))?;
            let params = &variant["properties"]["params"];
            for field in fields {
                if property(root, params, &field).is_none() {
                    return Err(format!(
                        "consumed native field removed or renamed: {name}.{field}"
                    ));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn published_schemas_keep_their_exact_source_commit_and_checksum() {
    for &version in VERSIONS {
        let provenance = schema(version, "sources.json");
        assert_eq!(provenance["program"], "codex");
        assert_eq!(provenance["version"], version);
        let commit = provenance["commit"].as_str().unwrap();
        assert_eq!(commit.len(), 40);
        for file in provenance["files"].as_array().unwrap() {
            let bytes = std::fs::read(
                schema_directory()
                    .join(version)
                    .join(file["path"].as_str().unwrap()),
            )
            .unwrap();
            let actual: String = Sha256::digest(&bytes)
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect();
            assert_eq!(actual, file["sha256"].as_str().unwrap());
            assert!(file["source"].as_str().unwrap().contains(commit));
        }
    }
}

#[test]
fn schema_drift_fails_when_a_consumed_native_method_or_field_is_removed() {
    for &version in VERSIONS {
        let original = schema(version, "app-server.schema.json");
        check_consumed(&original).unwrap();
        let mut removed = original.clone();
        removed["definitions"]["v2"]["ItemCompletedNotification"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("item");
        assert!(
            check_consumed(&removed)
                .unwrap_err()
                .contains("item/completed.item")
        );
        let mut renamed = original;
        let branches = renamed["definitions"]["ClientRequest"]["oneOf"]
            .as_array_mut()
            .unwrap();
        let variant = branches
            .iter_mut()
            .find(|variant| variant["properties"]["method"]["enum"][0] == "turn/start")
            .unwrap();
        variant["properties"]["method"]["enum"][0] = Json::from("turn/startRenamed");
        assert!(check_consumed(&renamed).unwrap_err().contains("turn/start"));
    }
}

#[test]
fn the_real_lite_rpc_reader_consumes_fields_present_in_both_native_envelopes() {
    let source = include_str!("../src/protocol.rs")
        .split("#[cfg(test)]")
        .next()
        .unwrap();
    let fields: BTreeSet<_> = source
        .split(".get(\"")
        .skip(1)
        .filter_map(|part| part.split('"').next())
        .collect();
    assert!(!fields.is_empty());
    for &version in VERSIONS {
        let root = schema(version, "app-server.schema.json");
        for field in fields.iter().copied().chain([
            "id", "method", "params", "result", "error", "code", "message", "data",
        ]) {
            assert!(
                [
                    "JSONRPCRequest",
                    "JSONRPCNotification",
                    "JSONRPCResponse",
                    "JSONRPCError",
                    "JSONRPCErrorError"
                ]
                .iter()
                .any(|name| property(&root, &root["definitions"][name], field).is_some()),
                "{version}: native RPC no longer declares {field}"
            );
        }
    }
    for id in [
        RequestId::Number(0),
        RequestId::Text("native-request".into()),
    ] {
        assert_eq!(RequestId::from_json(&id.to_json()), Some(id));
    }
}

#[derive(Deserialize)]
struct Pin {
    name: String,
    value: Json,
    since: String,
}
#[test]
fn every_actual_managed_pin_exists_in_its_native_build_and_overrides_a_profile() {
    let pins: Vec<Pin> =
        serde_json::from_value(json(&directory().join("managed-pins.json"))).unwrap();
    let contract: BTreeMap<_, _> = pins
        .iter()
        .map(|pin| (pin.name.clone(), pin.value.clone()))
        .collect();
    assert_eq!(
        contract,
        process::automation_pins(),
        "a new or renamed launch pin needs a reviewed native schema contract"
    );
    for &version in VERSIONS {
        let native = schema(version, "config.schema.json");
        for pin in &pins {
            let exists = dotted_property(&native, &pin.name).is_some();
            assert_eq!(
                exists,
                version >= pin.since.as_str(),
                "{version}: {} changed its native spelling or availability",
                pin.name
            );
        }
    }
    let options = process::home_options(&serde_json::json!({}), true);
    let config = process::effective_config(
        &options,
        &serde_json::json!({"features":{"memories":true,"hooks":true},"memories":{"use_memories":true}}),
    );
    let actual = serde_json::to_value(config).unwrap();
    for pin in &pins {
        let pointer = format!("/{}", pin.name.replace('.', "/"));
        assert_eq!(
            actual.pointer(&pointer),
            Some(&pin.value),
            "profile cannot remove pin {}",
            pin.name
        );
    }
    assert_eq!(
        descriptor::descriptor("0.165.0").harness.status,
        HarnessSupport::Unsupported
    );
    assert_eq!(
        descriptor::descriptor("0.167.0").harness.status,
        HarnessSupport::Supported
    );
    assert_eq!(
        descriptor::descriptor("0.167.3").harness.status,
        HarnessSupport::Supported
    );
    assert_eq!(
        descriptor::descriptor("0.168.0").harness.status,
        HarnessSupport::Untested
    );
    assert_eq!(
        descriptor::descriptor("0.167.2").harness.status,
        HarnessSupport::Supported
    );
}
