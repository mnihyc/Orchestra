//! Move requests and machine choices carry the same data ownership and policy facts to every client.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::environment::{
    EnvironmentMovePlace, EnvironmentMovePreflightBody, NodeEnvironmentMovePreflightBody,
};
use serde_json::json;

#[test]
fn a_move_and_a_batch_choose_a_machine_and_explicit_copy_without_a_placement_switch() {
    let machine = "0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01";
    let one = json!({"machine_id":machine,"copy_directories":true,"home":true});
    let read: EnvironmentMovePreflightBody = serde_json::from_value(one.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), one);
    let batch = json!({"machine_id":machine,"copy_directories":true,"environment_ids":["0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a02"]});
    let read: NodeEnvironmentMovePreflightBody = serde_json::from_value(batch.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), batch);
}

#[test]
fn a_declared_machine_choice_explains_its_policy_refusal_to_a_person_or_an_agent() {
    let value = json!({"kind":"machine","machine":{"id":"0199d1a8-7f3c-7a1e-8b3a-1c2d3e4f5a01",
        "name":"Team workstation","online":true,"platform_os":"linux","placement":"connected"},
        "host":null,"usable":false,"reasons":["The machine policy has remote writes off."]});
    let read: EnvironmentMovePlace = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(read).unwrap(), value);
}
