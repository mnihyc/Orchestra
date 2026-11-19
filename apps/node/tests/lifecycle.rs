//! A runtime's life on a real Docker daemon, through a real gateway and API: stopped and started with its identity, recreated as a successor on the same volumes with its files kept while the old unit stays until told, and removed without its volumes; and a unit whose connector cannot enroll, reported failed and cleaned up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{Fleet, Lab, NodeUnderTest, World, code_of, eventually};
use serde_json::json;

#[tokio::test]
async fn a_runtime_is_stopped_and_started_recreated_on_its_volumes_with_its_files_kept_and_removed_without_them()
 {
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mut world = World::start(&lab.bridge).await;
    let mut node = NodeUnderTest::start(&world, &lab).await;
    let fleet = Fleet::new(&world, &lab, &node);
    let _program = fleet.prepare().await;

    // The first unit, up and enrolled.
    let first = fleet.unit("first");
    fleet.make(&first, None).await.expect("made");
    fleet.start(&first).await.expect("started");
    let machine = world.wait_enrolled(&first.hint).await;
    world.wait_online(machine, true).await;
    lab.put_file(&first.name, "/workspace", "kept.txt", "still here\n")
        .await;

    // Stopped: its connector says goodbye, the unit stays; started: the same machine comes back with the
    // identity it kept on its state volume, and nothing enrolls again.
    fleet.stop(&first).await.expect("stopped");
    world.wait_online(machine, false).await;
    assert!(
        lab.has_container(&first.name).await,
        "a stopped unit is kept"
    );
    fleet.start(&first).await.expect("started again");
    world.wait_online(machine, true).await;

    // A recreate: the old unit is stopped and kept; the successor is a runtime of its own on the same volumes,
    // with a token of its own that is never spent, because the identity is there.
    fleet
        .stop(&first)
        .await
        .expect("stopped before the recreate");
    world.wait_online(machine, false).await;
    let second = fleet.successor(&first, "second");
    fleet
        .make(&second, None)
        .await
        .expect("the successor could not be made");
    fleet
        .start(&second)
        .await
        .expect("the successor could not be started");
    world.wait_online(machine, true).await;
    assert!(
        world.machine_of_token(&second.hint).await.is_none(),
        "the successor's token was not used: its connector found its identity"
    );
    assert_eq!(
        lab.read_file(&second.name, "/workspace/kept.txt")
            .await
            .as_deref(),
        Some("still here\n"),
        "the files outlive the unit"
    );
    assert!(
        lab.has_container(&first.name).await,
        "the old unit is kept until it is told to go"
    );

    // Two units never run on one set of volumes: the old one cannot be started while its successor runs.
    let refused = fleet.start(&first).await.unwrap_err();
    assert_eq!(code_of(&refused), "conflict", "{refused}");

    // The old unit is removed when the system says so, and its volumes stay with the successor.
    fleet
        .remove(&first)
        .await
        .expect("the old unit could not be removed");
    assert!(!lab.has_container(&first.name).await);
    for (_, volume) in &first.volumes {
        assert!(lab.has_volume(volume).await, "{volume} stays");
    }
    world.wait_online(machine, true).await;

    // A volume a running unit stands on is not removed.
    let state = &first.volumes[2].1;
    assert_eq!(
        code_of(&fleet.remove_volume(state).await.unwrap_err()),
        "conflict"
    );

    // And the end: the successor stopped and removed, the volumes with it.
    fleet.stop(&second).await.expect("stopped");
    fleet.remove(&second).await.expect("removed");
    assert!(!lab.has_container(&second.name).await);
    for (_, volume) in &first.volumes {
        fleet
            .remove_volume(volume)
            .await
            .expect("a volume could not be removed");
        assert!(!lab.has_volume(volume).await, "{volume} is gone");
    }
    // Removing what is not there is done.
    fleet
        .remove(&second)
        .await
        .expect("removing a removed runtime is done");

    // What the node told the system, in order.
    eventually("the node to have told the last state", || {
        let (world, host, runtime) = (&world, node.machine_id, first.runtime_id);
        async move {
            let told = world.told(host, "node").await;
            told.iter().any(|(kind, body)| {
                kind == "node.runtime"
                    && body["runtime_id"] == json!(runtime)
                    && body["state"] == "absent"
            })
        }
    })
    .await;
    let states: Vec<String> = world
        .told(node.machine_id, "node")
        .await
        .into_iter()
        .filter(|(kind, body)| {
            kind == "node.runtime" && body["runtime_id"] == json!(first.runtime_id)
        })
        .map(|(_, body)| body["state"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        states,
        [
            "stopped", "running", "stopped", "running", "stopped", "absent"
        ],
        "made, started, stopped, started, stopped for the recreate, removed"
    );

    node.stop().await;
    world.stop().await;
}

#[tokio::test]
async fn a_unit_whose_connector_cannot_enroll_is_reported_failed_and_removed_and_its_volumes_stay()
{
    let Some(lab) = Lab::start().await else {
        return;
    };
    let mut world = World::start(&lab.bridge).await;
    let mut node = NodeUnderTest::start(&world, &lab).await;
    let fleet = Fleet::new(&world, &lab, &node);
    let _program = fleet.prepare().await;

    // The unit is given an address where nothing listens: its connector gives up, and the unit ends.
    let lost = fleet.unit("lost");
    fleet
        .make(&lost, Some("wss://gateway.orch.internal:1"))
        .await
        .expect("made");
    fleet.start(&lost).await.expect("started");
    // The start may find the unit ended already and tell it failed in the driver's words; the node's watch of
    // the enrollment then says why, which is what the test waits for.
    eventually("the node to have told why the unit failed", || {
        let (world, host, runtime) = (&world, node.machine_id, lost.runtime_id);
        async move {
            world.told(host, "node").await.iter().any(|(kind, body)| {
                kind == "node.runtime"
                    && body["runtime_id"] == json!(runtime)
                    && body["state"] == "failed"
                    && body["detail"]
                        .as_str()
                        .is_some_and(|why| why.contains("before its connector enrolled"))
            })
        }
    })
    .await;
    eventually("the unit to be removed", || {
        let (lab, name) = (&lab, lost.name.clone());
        async move { !lab.has_container(&name).await }
    })
    .await;
    // What the system is left with is that word: nothing the node saw of the unit earlier is told after it.
    let told = world.told(node.machine_id, "node").await;
    let last = told
        .iter()
        .rev()
        .find(|(kind, body)| kind == "node.runtime" && body["runtime_id"] == json!(lost.runtime_id))
        .map(|(_, body)| body.clone())
        .unwrap();
    assert_eq!(last["state"], "failed", "{last}");
    assert!(
        last["detail"]
            .as_str()
            .unwrap()
            .contains("before its connector enrolled"),
        "{last}"
    );
    for (_, volume) in &lost.volumes {
        assert!(lab.has_volume(volume).await, "{volume} stays");
    }
    assert!(
        world.machine_of_token(&lost.hint).await.is_none(),
        "nothing enrolled"
    );
    for (_, volume) in &lost.volumes {
        fleet.remove_volume(volume).await.expect("removed");
    }
    node.stop().await;
    world.stop().await;
}
