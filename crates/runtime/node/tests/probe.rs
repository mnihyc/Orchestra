//! What the node tells about the host, on the in-memory hub: its facts when it starts and when they change, what it uses, and the answer to `node.probe`.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use common::{Rig, hosted};
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::RuntimeKind;
use orch_link_contract::commands::{NodeProbe, NodeProbeResult, SerialKey};
use orch_node_core::drivers::{DriverHost, Factory, Registrations};
use serde_json::json;

fn probe_of(result: serde_json::Value) -> NodeProbeResult {
    serde_json::from_value(result).unwrap()
}

#[tokio::test]
async fn a_node_tells_what_the_host_has_which_drivers_it_loaded_and_its_storages_when_it_starts() {
    let mut rig = Rig::new();
    rig.start().await;
    rig.wait_told("node.facts", |events| {
        events.iter().any(|event| event.kind == "node.facts")
    })
    .await;
    let facts = rig
        .applied()
        .into_iter()
        .find(|event| event.kind == "node.facts")
        .unwrap()
        .body;
    assert!(facts["capacity"]["cpus"].as_i64().unwrap() >= 1);
    assert!(facts["capacity"]["memory_mb"].as_i64().unwrap() > 0);
    assert_eq!(facts["drivers"]["memory"]["name"], "memory");
    assert_eq!(facts["drivers"]["memory"]["recreate"], "replace");
    let storages = facts["storages"].as_array().unwrap();
    assert_eq!(storages.len(), 1, "a host that declares none has one");
    assert_eq!(
        (
            storages[0]["name"].as_str(),
            storages[0]["default"].as_bool()
        ),
        (Some("default"), Some(true))
    );
    rig.stop().await;
}

#[tokio::test]
async fn what_the_host_uses_is_told_as_soon_as_the_node_runs() {
    let mut rig = Rig::new();
    rig.start().await;
    rig.wait_told("node.load", |events| {
        events.iter().any(|event| event.kind == "node.load")
    })
    .await;
    let load = rig
        .applied()
        .into_iter()
        .find(|event| event.kind == "node.load")
        .unwrap()
        .body;
    for member in [
        "cpu_percent",
        "memory_used_mb",
        "disk_used_gb",
        "disk_free_gb",
    ] {
        assert!(
            load.get(member).is_some_and(|value| value.is_number()),
            "{member}: {load}"
        );
    }
    assert!(load["memory_used_mb"].as_i64().unwrap() > 0);
    rig.stop().await;
}

#[tokio::test]
async fn a_probe_answers_the_facts_the_units_and_the_tools() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "probed");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;

    let probe = probe_of(rig.succeed(&SerialKey::Machine, &NodeProbe {}).await);
    assert!(probe.capacity.cpus >= 1);
    assert!(probe.drivers.contains_key("memory"));
    assert_eq!(probe.storages.len(), 1);
    assert_eq!(probe.units.len(), 1);
    assert_eq!(probe.units[0].runtime_id, spec.runtime_id);
    assert_eq!(probe.units[0].state, "running");
    assert_eq!(
        serde_json::to_value(&probe.tools).unwrap(),
        json!([{ "name": "orch-connector", "version": "0.1.0" }])
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_driver_that_could_not_be_loaded_joins_the_host_and_the_facts_are_told_again() {
    let mut rig = Rig::new();
    rig.settings.reconcile_every = Duration::from_secs(1);
    rig.settings.drivers = vec!["memory".to_owned()];
    let up = Arc::new(AtomicBool::new(false));
    let factory: Factory = {
        let (up, driver, extras) = (
            Arc::clone(&up),
            Arc::clone(&rig.driver),
            Arc::clone(&rig.extras),
        );
        Arc::new(move |_| {
            let (up, driver, extras) = (Arc::clone(&up), Arc::clone(&driver), Arc::clone(&extras));
            Box::pin(async move {
                if up.load(Ordering::SeqCst) {
                    Ok(hosted(&driver, &extras))
                } else {
                    Err(DriverError::Unavailable("the daemon is not up yet".into()))
                }
            })
        })
    };
    let drivers = DriverHost::load(
        &rig.settings,
        Registrations::new().register("memory", factory),
        Arc::clone(&rig.desk),
    )
    .await;
    assert!(drivers.names().is_empty());
    rig.start_with(drivers).await;
    rig.wait_told("the first facts", |events| {
        events.iter().any(|event| event.kind == "node.facts")
    })
    .await;
    let first = rig
        .applied()
        .into_iter()
        .find(|event| event.kind == "node.facts")
        .unwrap();
    assert_eq!(
        first.body["drivers"],
        json!({}),
        "a host with no driver says so"
    );

    up.store(true, Ordering::SeqCst);
    rig.wait_told("facts with the driver", |events| {
        events
            .iter()
            .filter(|event| event.kind == "node.facts")
            .any(|event| event.body["drivers"].get("memory").is_some())
    })
    .await;
    rig.stop().await;
}
