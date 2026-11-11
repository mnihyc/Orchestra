//! The ordinary connector report carries metadata-only sizes without a role, credential reader or model call.
#![allow(clippy::unwrap_used, clippy::expect_used, reason = "test assertions")]
mod common;
use common::Rig;
use orch_api_contract::storage::MachineDataInventory;
use std::time::Duration;

#[tokio::test]
async fn regular_machine_facts_publish_directory_parts_and_fast_facts_keep_the_actual_observation_time()
 {
    let mut rig = Rig::new();
    rig.roles.clear();
    for name in [
        "secrets",
        "mcp",
        "harness/provider",
        "environment-data/env",
        "storage-bindings",
        "logs",
        "unrecorded",
    ] {
        std::fs::create_dir_all(rig.data.path().join(name)).unwrap();
    }
    for (name, value) in [
        ("secrets/private-name", "never-report-credential"),
        ("mcp/server.json", "never-report-copy"),
        ("harness/provider/history", "history"),
        ("environment-data/env/file", "workspace"),
        ("storage-bindings/binding.json", "binding"),
        ("logs/connector.log", "log"),
        ("unrecorded/file", "unrecorded-data"),
    ] {
        std::fs::write(rig.data.path().join(name), value.as_bytes()).unwrap();
    }
    rig.start().await;
    let inventory = tokio::time::timeout(rig.patience, async {
        loop {
            if let Some(inventory) = rig
                .controller
                .applied("machine")
                .iter()
                .rev()
                .filter(|event| event.kind == "machine.facts")
                .find_map(|event| {
                    event.body.get("data_directory").cloned().and_then(|value| {
                        serde_json::from_value::<MachineDataInventory>(value)
                            .ok()
                            .filter(|inventory| inventory.complete)
                    })
                })
            {
                break inventory;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the existing initial facts tick did not report directory metadata");
    assert_eq!(
        inventory
            .parts
            .iter()
            .find(|part| part.name == "secrets")
            .unwrap()
            .bytes,
        Some(23)
    );
    assert!(inventory.parts.iter().any(|part| part.name == "unrecorded"));
    let serialized = serde_json::to_string(&inventory).unwrap();
    for secret in [
        "private-name",
        "never-report-credential",
        "never-report-copy",
        "unrecorded-data",
    ] {
        assert!(!serialized.contains(secret));
    }
    assert!(!serialized.contains(rig.data.path().to_str().unwrap()));
    assert_eq!(
        rig.connector.as_ref().unwrap().facts().data_directory,
        Some(inventory.clone())
    );
    std::fs::write(
        rig.data.path().join("logs/new.log"),
        b"new data after the actual scan",
    )
    .unwrap();
    rig.manual.advance(chrono::Duration::seconds(10));
    assert_eq!(
        rig.connector.as_ref().unwrap().facts().data_directory,
        Some(inventory),
        "fast paths do not replace a scan time with a new claim"
    );
    rig.connector.take().unwrap().stop().await;
}
