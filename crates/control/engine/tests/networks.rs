//! Only the registered host node changes a cold runtime's network, with durable replay and no unit enrollment.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;
mod hosting;

use common::{World, eventually};
use hosting::{ScriptedHost, serve, template};
use orch_api_contract::environment::{EgressMode, NetworkState, UnitNetworkMode};
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::{Event, NodeNetwork};
use orch_link_contract::streams::Stream;
use orch_types::{Id, Json};

#[tokio::test]
async fn a_cold_runtime_accepts_its_own_nodes_network_and_refuses_a_foreign_nodes_report() {
    let world = World::new().await;
    serve(&world);
    let host = ScriptedHost::on(&world, "own-node", 16_384).await;
    // The unit is made and never started: its making stays still, since the wait for a started unit's connector
    // stamps the environment's feed as it goes, and the feed is what the reports are measured by.
    host.added
        .machine
        .handler
        .hold(CommandKind::NodeRuntimeStart);
    let template = template(&world).await;
    let made = hosting::make(&world, template, "Cold").await;
    host.wait_began(CommandKind::NodeRuntimeStart, 1).await;
    let runtime: Id = world
        .scalar("SELECT id FROM runtimes WHERE environment_id = $1", made.id)
        .await;
    let foreign = ScriptedHost::on(&world, "other-node", 16_384).await;
    let mut network = NetworkState::open(UnitNetworkMode::Bridge);
    network.mode = EgressMode::Restricted;
    network.required_mode = EgressMode::Restricted;
    network.revision = "requested-hosts".to_owned();
    network.always_revision = "trusted-transport".to_owned();
    let report = Event::NodeNetwork(NodeNetwork {
        runtime_id: runtime,
        network,
    });
    foreign.added.machine.publish(&Stream::Node, &report).await;
    eventually("the foreign report to be acknowledged", || async {
        world.commands.acknowledged(foreign.machine, &Stream::Node) >= 1
    })
    .await;
    assert_eq!(
        world
            .scalar::<Option<Json>>("SELECT network_state FROM runtimes WHERE id = $1", runtime)
            .await,
        None
    );
    let before: i64 = world
        .scalar("SELECT env_seq FROM environments WHERE id = $1", made.id)
        .await;
    host.added.machine.publish(&Stream::Node, &report).await;
    eventually(
        "the registered host report to be stored before enrollment",
        || async {
            world
                .scalar::<Option<Json>>("SELECT network_state FROM runtimes WHERE id = $1", runtime)
                .await
                .is_some()
        },
    )
    .await;
    let state: Json = world
        .scalar("SELECT network_state FROM runtimes WHERE id = $1", runtime)
        .await;
    assert_eq!(state["revision"], "requested-hosts");
    let machine: Option<orch_types::Id> = world
        .scalar("SELECT machine_id FROM runtimes WHERE id = $1", runtime)
        .await;
    assert_eq!(machine, None);
    let after: i64 = world
        .scalar("SELECT env_seq FROM environments WHERE id = $1", made.id)
        .await;
    assert!(
        after > before,
        "the environment feed observes the new network without a thread"
    );
    host.added.machine.publish(&Stream::Node, &report).await;
    eventually("an identical later report to be acknowledged", || async {
        world.commands.acknowledged(host.machine, &Stream::Node) >= 2
    })
    .await;
    assert_eq!(
        world
            .scalar::<i64>("SELECT env_seq FROM environments WHERE id = $1", made.id)
            .await,
        after
    );
    world.engine().stop();
}
