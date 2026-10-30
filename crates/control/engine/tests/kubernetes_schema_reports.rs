//! A disconnected driver can report its setup schema without becoming schedulable.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually};
use hosting::{ScriptedHost, serve};
use orch_link_contract::events::{Event, NodeCapacity, NodeFacts};
use orch_link_contract::streams::Stream;
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;

#[tokio::test]
async fn reported_schemas_are_separate_from_drivers_and_an_old_report_clears_them() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "schema-node", 16_384).await;
    let schemas = json!({"cluster": {"type":"object", "required":["namespace"]}});
    let mut facts = NodeFacts {
        capacity: NodeCapacity {
            cpus: 8,
            cpu_millis: None,
            memory_mb: 16_384,
            disk_gb: 200,
        },
        drivers: BTreeMap::new(),
        driver_configuration_schemas: serde_json::from_value(schemas.clone()).unwrap(),
        storages: Vec::new(),
        mount_sizes: Vec::new(),
    };
    host.added
        .machine
        .publish(&Stream::Node, &Event::NodeFacts(facts.clone()))
        .await;
    eventually("the schema-only host report", || async {
        world
            .scalar::<Json>(
                "SELECT driver_configuration_schemas FROM hosts WHERE id = $1",
                host.id,
            )
            .await
            == schemas
    })
    .await;
    assert_eq!(
        world
            .scalar::<Json>("SELECT drivers FROM hosts WHERE id = $1", host.id)
            .await,
        json!({})
    );
    // An older node omits the new map on the wire. Its next report replaces,
    // rather than silently retaining, a schema it may no longer have.
    facts.driver_configuration_schemas.clear();
    assert!(
        serde_json::to_value(&facts)
            .unwrap()
            .get("driver_configuration_schemas")
            .is_none()
    );
    host.added
        .machine
        .publish(&Stream::Node, &Event::NodeFacts(facts))
        .await;
    eventually("an older report to clear the schema", || async {
        world
            .scalar::<Json>(
                "SELECT driver_configuration_schemas FROM hosts WHERE id = $1",
                host.id,
            )
            .await
            == json!({})
    })
    .await;
    world.engine().stop();
}
