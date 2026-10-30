//! The first thing the node has to get right, against a real Docker daemon, a real gateway and a real API: a managed runtime made through the node from a small image, its connector enrolled with the single-use token it was given and online at the API.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fleet, Lab, NodeUnderTest, World, eventually};

#[tokio::test]
async fn a_runtime_made_through_the_node_enrolls_with_its_single_use_token_and_is_online_at_the_api()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mut world = World::start(&lab.bridge).await;
    let mut node = NodeUnderTest::start(&world, &lab).await;
    let (api, host) = (world.api.link.clone(), node.machine_id);
    eventually("the node to be online at the API", || {
        let api = api.clone();
        async move { api.is_online(host) }
    })
    .await;

    // What the system sends, in order, to get a runtime with an enrolled connector: the program the unit runs
    // and its image, then its volumes, the unit, and its start.
    let fleet = Fleet::new(&world, &lab, &node);
    let _program = fleet.prepare().await;
    let unit = fleet.unit("one");
    let made = fleet
        .make(&unit, None)
        .await
        .expect("the unit could not be made");
    assert_eq!(made.unit.name, unit.name);
    assert!(lab.has_container(&unit.name).await);
    fleet
        .start(&unit)
        .await
        .expect("the unit could not be started");

    // Its connector starts inside the unit, enrolls with the token, and holds a session at the API.
    let enrolled = world.wait_enrolled(&unit.hint).await;
    world.wait_online(enrolled, true).await;

    // And it reports itself.
    eventually("the connector to report its facts", || {
        let (world, machine) = (&world, enrolled);
        async move {
            world
                .told(machine, "machine")
                .await
                .iter()
                .any(|(kind, _)| kind == "machine.facts")
        }
    })
    .await;
    println!(
        "FIRST MILESTONE: the unit {} enrolled as machine {enrolled} and is online",
        unit.name
    );

    node.stop().await;
    world.stop().await;
}
