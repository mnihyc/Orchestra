//! The pinned app server's generated request-boundary surface matches its recorded conformance schema.
#![cfg(target_os = "linux")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

struct Container(String);
impl Drop for Container {
    fn drop(&mut self) {
        common::remove_containers(std::slice::from_ref(&self.0));
    }
}

fn references(value: &Value, waiting: &mut Vec<String>) {
    match value {
        Value::Object(object) => {
            if let Some(name) = object
                .get("$ref")
                .and_then(Value::as_str)
                .and_then(|value| value.strip_prefix("#/definitions/"))
            {
                waiting.push(name.to_owned());
            }
            for child in object.values() {
                references(child, waiting);
            }
        }
        Value::Array(array) => {
            for child in array {
                references(child, waiting);
            }
        }
        _ => {}
    }
}

/// Keep the exact runtime interface observation beside the existing native schema proof.
/// Published stable schemas can omit controls behind the initialized experimental API.
fn terminal_declarations(schema: &Value) -> Value {
    let mut methods = BTreeMap::new();
    let mut waiting = Vec::new();
    for method in [
        "thread/backgroundTerminals/list",
        "thread/backgroundTerminals/terminate",
    ] {
        let declared = schema
            .pointer("/definitions/ClientRequest/oneOf")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .find(|variant| {
                variant
                    .pointer("/properties/method/enum")
                    .and_then(Value::as_array)
                    .is_some_and(|names| names.iter().any(|name| name.as_str() == Some(method)))
            })
            .cloned()
            .unwrap_or(Value::Null);
        references(&declared, &mut waiting);
        methods.insert(method, declared);
    }
    waiting.extend([
        "v2/ThreadBackgroundTerminalsListResponse".into(),
        "v2/ThreadBackgroundTerminalsTerminateResponse".into(),
    ]);
    let mut seen = HashSet::new();
    let mut definitions = BTreeMap::new();
    while let Some(name) = waiting.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let definition = schema
            .pointer(&format!("/definitions/{name}"))
            .cloned()
            .unwrap_or(Value::Null);
        references(&definition, &mut waiting);
        definitions.insert(name, definition);
    }
    serde_json::json!({
        "fixture_image":common::image(),
        "client_request_present":schema.pointer("/definitions/ClientRequest/oneOf").is_some_and(Value::is_array),
        "methods":methods,
        "definitions":definitions,
    })
}

#[tokio::test]
async fn the_generated_protocol_preserves_the_request_events_the_adapter_measures() {
    let captured: Value =
        serde_json::from_str(include_str!("../schema/0.167.0/request-boundaries.json")).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let guard = Container(format!(
        "orchprobe-performance-schema-{}",
        orch_types::new_id()
    ));
    let output = tokio::time::timeout(
        common::WAIT,
        tokio::process::Command::new("docker")
            .args(["run", "--network", "none", "--name", &guard.0])
            .arg(common::image())
            .args([
                "codex",
                "app-server",
                "generate-json-schema",
                "--experimental",
                "--out",
                // The image runs as an ordinary user, who may write under /tmp and not at the root.
                "/tmp/out",
            ])
            .output(),
    )
    .await
    .expect("schema generation finishes within the native conformance budget")
    .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // Keep all generator writes inside the container, then copy its completed schema outputs.
    let copied = tokio::time::timeout(
        common::WAIT,
        tokio::process::Command::new("docker")
            .arg("cp")
            .arg(format!("{0}:/tmp/out/.", guard.0))
            .arg(directory.path())
            .output(),
    )
    .await
    .expect("the generated schema can be copied within the native conformance budget")
    .unwrap();
    assert!(
        copied.status.success(),
        "{}",
        String::from_utf8_lossy(&copied.stderr)
    );
    let complete: Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .path()
                .join("codex_app_server_protocol.schemas.json"),
        )
        .unwrap(),
    )
    .unwrap();
    // Only public interface declarations, never a session, account, token or native frame body.
    let terminals = terminal_declarations(&complete);
    eprintln!("native_terminal_method_declarations={}", terminals);
    let recorded: Value =
        serde_json::from_str(include_str!("../schema/0.167.0/experimental-controls.json")).unwrap();
    let recorded_methods: BTreeMap<_, _> = recorded["definitions"]["ClientRequest"]["oneOf"]
        .as_array()
        .unwrap()
        .iter()
        .map(|variant| {
            (
                variant["properties"]["method"]["enum"][0]
                    .as_str()
                    .unwrap()
                    .to_owned(),
                variant.clone(),
            )
        })
        .collect();
    assert_eq!(
        terminals["methods"],
        serde_json::to_value(recorded_methods).unwrap(),
        "the native control gate must match this exact initialized experimental interface"
    );
    let generated: Value = serde_json::from_slice(
        &std::fs::read(
            directory
                .path()
                .join("codex_app_server_protocol.v2.schemas.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let mut waiting: Vec<String> = captured["roots"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value.as_str().unwrap().to_owned())
        .collect();
    let mut seen = HashSet::new();
    let mut surface = BTreeMap::new();
    while let Some(name) = waiting.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let definition = generated["definitions"]
            .get(&name)
            .unwrap_or_else(|| panic!("the pinned protocol no longer declares {name}"));
        references(definition, &mut waiting);
        surface.insert(name, definition.clone());
    }
    assert_eq!(
        serde_json::to_value(surface).unwrap(),
        captured["definitions"],
        "a harness upgrade must re-establish the recorded request boundaries"
    );
}
