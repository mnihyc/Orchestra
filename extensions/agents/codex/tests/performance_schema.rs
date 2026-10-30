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

#[tokio::test]
async fn the_generated_protocol_preserves_the_request_events_the_adapter_measures() {
    let mut captured: Value =
        serde_json::from_str(include_str!("fixtures/performance/protocol.json")).unwrap();
    // Supply this optional schema member synthetically; the captured request fixture carries no reset data.
    let image_failure =
        &mut captured["definitions"]["ImageGenerationFailure"]["oneOf"][0]["properties"];
    assert!(
        image_failure.get("resetsAt").is_none(),
        "the optional reset schema belongs to synthetic setup"
    );
    image_failure["resetsAt"] = serde_json::json!({"format":"int64","type":["integer","null"]});
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
    // Keep all generator writes inside the container, then copy only its completed schema.
    let copied = tokio::time::timeout(
        common::WAIT,
        tokio::process::Command::new("docker")
            .arg("cp")
            .arg(format!(
                "{0}:/tmp/out/codex_app_server_protocol.v2.schemas.json",
                guard.0
            ))
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
