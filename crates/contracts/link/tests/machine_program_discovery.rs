//! Confirmed, discovered and not-yet-searched program facts remain distinct on the link.
#![allow(clippy::unwrap_used, reason = "a contract test fails by panicking")]

use orch_link_contract::session::MachineFacts;
use orch_types::Json;
use serde_json::json;

fn machine() -> Json {
    json!({
        "hostname":"connected-machine", "os_version":"test", "cpus":1, "memory_bytes":1024,
        "harnesses":[], "adapters":[], "plugins":[], "policy":null,
        "rates":{"up_bps":0,"down_bps":0,"round_trip_ms":0}, "clock_offset_ms":0
    })
}

#[test]
fn confirmed_programs_and_unconfirmed_discoveries_keep_their_versions_and_paths() {
    let mut reported = machine();
    reported["harnesses"] = json!([
        {"kind":"codex","path":"/connected/bin/codex","version":"0.165.0","daemon":true}
    ]);
    reported["discovered_harnesses"] = json!([
        {"kind":"claude-code","path":"/connected/bin/claude","version":"unknown","daemon":false}
    ]);
    reported["searched_harness_kinds"] = json!(["codex", "claude-code"]);
    let facts: MachineFacts = serde_json::from_value(reported.clone()).unwrap();
    let returned = serde_json::to_value(facts).unwrap();
    for field in [
        "harnesses",
        "discovered_harnesses",
        "searched_harness_kinds",
    ] {
        assert_eq!(returned[field], reported[field], "{field}");
    }
}

#[test]
fn a_completed_empty_search_is_not_an_unobserved_search() {
    let mut reported = machine();
    reported["discovered_harnesses"] = json!([]);
    reported["searched_harness_kinds"] = json!(["codex", "claude-code"]);
    let facts: MachineFacts = serde_json::from_value(reported).unwrap();
    let returned = serde_json::to_value(facts).unwrap();
    assert_eq!(returned["discovered_harnesses"], json!([]));
    assert_eq!(
        returned["searched_harness_kinds"],
        json!(["codex", "claude-code"])
    );
}

#[test]
fn an_older_report_does_not_claim_that_program_discovery_ran() {
    let facts: MachineFacts = serde_json::from_value(machine()).unwrap();
    let returned = serde_json::to_value(facts).unwrap();
    assert!(returned.get("discovered_harnesses").is_none());
    assert!(returned.get("searched_harness_kinds").is_none());
}
