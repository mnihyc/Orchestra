//! Reconciliation, on the in-memory hub with the in-memory driver: at start and on its interval the node compares the units the driver holds with what it recorded and tells the system what is new, and it removes nothing on its own.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::Rig;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::{RuntimeKind, unit_labels};
use orch_driver_contract::units::UnitStateKind;
use orch_testkit::doubles::driver::Operation;

#[tokio::test]
async fn at_start_the_node_tells_the_real_state_of_every_unit_it_made_before() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (up, down) = (
        rig.spec(RuntimeKind::Environment, "up"),
        rig.spec(RuntimeKind::Environment, "down"),
    );
    rig.made(&up).await;
    rig.made(&down).await;
    rig.start_runtime(up.runtime_id).await;
    rig.stop().await;

    // The system does not remember what it was told (a new controller would not): the node tells all again.
    let mut again =
        Rig::with_driver(orch_testkit::doubles::driver::MemoryDriver::new("memory").replacing());
    again.data = rig.data;
    again.driver = rig.driver.clone();
    again.extras = rig.extras.clone();
    again.settings = rig.settings.clone();
    again.start().await;
    again.wait_state(up.runtime_id, "running").await;
    again.wait_state(down.runtime_id, "stopped").await;
    again.stop().await;
}

#[tokio::test]
async fn a_unit_that_dies_by_itself_is_told_failed_with_the_exit_status_and_one_that_vanishes_is_told_absent()
 {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let (dies, vanishes, steady) = (
        rig.spec(RuntimeKind::Environment, "dies"),
        rig.spec(RuntimeKind::Environment, "vanishes"),
        rig.spec(RuntimeKind::Provider, "steady"),
    );
    for spec in [&dies, &vanishes, &steady] {
        rig.made(spec).await;
        rig.start_runtime(spec.runtime_id).await;
    }
    let role = rig.role();
    role.reconciler().pass().await;

    rig.driver.crash(&dies.name, 137, "out of memory");
    orch_driver_contract::driver::EnvironmentDriver::remove(
        rig.driver.as_ref(),
        &rig.driver.unit(&vanishes.name).unwrap().unit,
    )
    .await
    .unwrap();
    assert_eq!(
        role.reconciler().pass().await,
        2,
        "only what is new is told"
    );
    rig.wait_state(dies.runtime_id, "failed").await;
    rig.wait_state(vanishes.runtime_id, "absent").await;
    let (_, detail) = rig.states_of(dies.runtime_id).pop().unwrap();
    assert_eq!(detail.as_deref(), Some("out of memory (exit status 137)"));
    assert_eq!(
        rig.states_of(steady.runtime_id).len(),
        2,
        "made, started; nothing since"
    );

    assert_eq!(
        role.reconciler().pass().await,
        0,
        "and nothing is told twice"
    );
    rig.stop().await;
}

#[tokio::test]
async fn a_unit_with_the_systems_label_that_the_node_has_no_record_of_is_told_and_left_where_it_is()
{
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let known = rig.spec(RuntimeKind::Environment, "known");
    rig.made(&known).await;
    let stray = rig.spec(RuntimeKind::Environment, "stray");
    rig.driver.plant(stray.clone());
    // A unit of another system, on the same driver, is none of the node's business.
    let mut foreign = rig.spec(RuntimeKind::Environment, "foreign");
    foreign.labels = unit_labels(
        "another-system",
        RuntimeKind::Environment,
        &foreign.runtime_id,
    );
    rig.driver.plant(foreign.clone());

    rig.role().reconciler().pass().await;
    rig.wait_state(stray.runtime_id, "running").await;
    let (_, detail) = rig.states_of(stray.runtime_id).pop().unwrap();
    assert!(detail.unwrap().contains("no record of it"));
    assert!(rig.states_of(foreign.runtime_id).is_empty());
    assert!(
        rig.driver.unit(&stray.name).is_some(),
        "the node removes nothing on its own"
    );
    assert_eq!(rig.driver.state_of(&stray.name), UnitStateKind::Running);
    assert!(rig.driver.targets_of(Operation::Remove).is_empty());
    rig.stop().await;
}

#[tokio::test]
async fn a_runtime_a_command_is_being_carried_out_for_is_left_to_the_command() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "busy");
    rig.made(&spec).await;
    rig.role().reconciler().pass().await;
    let before = rig.states_of(spec.runtime_id).len();

    rig.driver.hold(Operation::Start);
    let started = rig.controller.issue(
        &orch_link_contract::commands::SerialKey::Runtime(spec.runtime_id),
        rig.deadline(),
        &orch_link_contract::commands::NodeRuntimeStart {
            runtime_id: spec.runtime_id,
        },
    );
    rig.wait_call(Operation::Start, &spec.name).await;
    rig.driver
        .crash(&spec.name, 1, "died while it was being started");
    rig.role().reconciler().pass().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(
        rig.states_of(spec.runtime_id).len(),
        before,
        "told nothing while the command runs: {:?}",
        rig.states_of(spec.runtime_id)
    );

    rig.driver.release(Operation::Start);
    tokio::time::timeout(common::PATIENCE, rig.controller.wait_command(started))
        .await
        .unwrap();
    rig.stop().await;
}

#[tokio::test]
async fn a_driver_that_cannot_list_does_not_make_its_units_absent() {
    let mut rig = Rig::new();
    rig.have_connector();
    rig.start().await;
    let spec = rig.spec(RuntimeKind::Environment, "listed");
    rig.made(&spec).await;
    rig.start_runtime(spec.runtime_id).await;
    rig.role().reconciler().pass().await;
    let before = rig.states_of(spec.runtime_id).len();

    rig.driver.fail_next(
        Operation::List,
        DriverError::Unavailable("the daemon went away".into()),
    );
    assert_eq!(rig.role().reconciler().pass().await, 0);
    assert_eq!(rig.states_of(spec.runtime_id).len(), before);
    assert_eq!(rig.driver.state_of(&spec.name), UnitStateKind::Running);
    rig.stop().await;
}
