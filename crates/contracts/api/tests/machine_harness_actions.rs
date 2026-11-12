//! Harness confirmation uses the machine's existing program command, without a shell command.
#![allow(clippy::unwrap_used, reason = "a contract test fails by panicking")]

use orch_api_contract::machine_programs::MachineProgramsCommand;
use serde_json::json;

#[test]
fn a_found_harness_is_confirmed_by_its_kind_and_path() {
    let body = json!({"action":"confirm_harness","kind":"codex","path":"/connected/bin/codex"});
    let request: MachineProgramsCommand = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), body);
}

#[test]
fn removing_a_harness_names_the_confirmed_path_without_replacement_arguments() {
    let body =
        json!({"action":"remove_harness","kind":"claude-code","path":"/connected/bin/claude"});
    let request: MachineProgramsCommand = serde_json::from_value(body.clone()).unwrap();
    assert_eq!(serde_json::to_value(request).unwrap(), body);
    assert!(
        serde_json::from_value::<MachineProgramsCommand>(json!({
            "action":"remove_harness","kind":"claude-code","path":"/connected/bin/claude",
            "command":["another-program"]
        }))
        .is_err()
    );
}

#[test]
fn refresh_preserves_the_existing_registry_reload_action() {
    let request: MachineProgramsCommand =
        serde_json::from_value(json!({"action":"reload"})).unwrap();
    assert_eq!(
        serde_json::to_value(request).unwrap(),
        json!({"action":"reload"})
    );
}
