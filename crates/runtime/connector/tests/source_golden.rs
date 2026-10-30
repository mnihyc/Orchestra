//! Recorded native replies retain the same canonical stored members and displayed numbers through the common source reader; the vendors' readers have theirs beside the program that registers them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_agent_contract::facts::{ProviderFacts, normalize};
use orch_connector_core::sources::{adapter, reports};
use orch_provider_contract::FactMember;
use orch_types::{Json, Time};
use serde_json::json;

fn at() -> Time {
    "2026-10-13T00:00:00.000Z".parse().unwrap()
}

fn same_members(before: &ProviderFacts, after: &ProviderFacts) {
    let mut before = before.clone();
    normalize(&mut before);
    let old = serde_json::to_value(&before).unwrap();
    let new = serde_json::to_value(after).unwrap();
    for member in FactMember::ALL {
        assert_eq!(
            new[member.as_str()],
            old[member.as_str()],
            "{}",
            member.as_str()
        );
    }
}

fn native(before: &ProviderFacts) -> ProviderFacts {
    let report = adapter::report(before).unwrap();
    let read = reports::read(&report.0, at(), true, &[]).unwrap();
    assert!(read.notes.is_empty(), "{:?}", read.notes);
    let after = reports::normalized(&read).unwrap();
    same_members(before, &after);
    after
}

#[test]
fn codex_recorded_windows_credits_identity_and_limit_keep_their_numbers_and_dates() {
    use orch_adapter_codex::provider::{LoginTracker, facts_from_snapshot};
    let tracker = LoginTracker::default();
    tracker.signed_in_as(
        Some("ada@example.invalid".to_owned()),
        Some("plus".to_owned()),
    );
    let before = facts_from_snapshot(
        &json!({"planType":"plus",
        "primary":{"usedPercent":20,"windowDurationMins":300,"resetsAt":1791849900},
        "secondary":{"usedPercent":99,"windowDurationMins":10080,"resetsAt":1792454400},
        "credits":{"balance":"4.125","unlimited":false},"rateLimitReachedType":"weekly"}),
        &tracker,
        at(),
    );
    let after = native(&before);
    assert_eq!(
        after
            .windows
            .iter()
            .map(|window| window.used_percent)
            .collect::<Vec<_>>(),
        [20.0, 99.0]
    );
    assert_eq!(after.balances[0].remaining.as_deref(), Some("4.125"));
    assert_eq!(after.identity.as_deref(), Some("ada@example.invalid"));
    assert_eq!(after.limit_reached.unwrap().kind, "weekly");
    assert_eq!(after.field_times["windows.primary"], at());
    assert_eq!(after.field_times["balances.plan"], at());
}

#[test]
fn claude_code_recorded_windows_and_extra_usage_keep_vendor_units_and_overage() {
    let before = orch_adapter_claude_code::provider::usage_from(&json!({
        "five_hour":{"utilization":130,"resets_at":"2026-10-13T05:00:00Z"},
        "seven_day":{"utilization":72,"resets_at":"2026-10-20T00:00:00Z"},
        "seven_day_sonnet":{"utilization":4,"resets_at":"2026-10-20T00:00:00Z"},
        "extra_usage":{"is_enabled":true,"monthly_limit":2000,"used_credits":400,"utilization":20,"currency":"USD","decimal_places":2}
    })).unwrap();
    let after = native(&before);
    assert_eq!(
        after
            .windows
            .iter()
            .map(|window| window.used_percent)
            .collect::<Vec<_>>(),
        [130.0, 72.0, 4.0]
    );
    let extra = after.extra_usage.unwrap();
    assert_eq!(extra.used, Some(400.0));
    assert_eq!(extra.unit.as_deref(), Some("USD"));
    assert_eq!(extra.decimal_places, Some(2));
    assert_eq!(after.field_times["windows.secondary:sonnet"], at());
}

#[test]
fn claude_native_control_observations_keep_the_same_members_after_the_source_reader() {
    // Synthetic control-protocol values exercise every stored member without an account recording.
    let answer = json!({"rate_limits_available":true,"subscription_type":"max","rate_limits":{
        "limits":[
            {"kind":"session","percent":36,"resets_at":"2026-10-13T05:00:00Z"},
            {"kind":"weekly_all","percent":79,"resets_at":"2026-10-14T09:00:00.000Z"},
            {"kind":"weekly_scoped","percent":12,"scope":{"model":{"id":"fable","display_name":"Fable"}}}
        ],
        "extra_usage":{"is_enabled":true,"monthly_limit":5000,"used_credits":123,"currency":"USD"}
    }});
    let before = orch_adapter_claude_code::provider::usage_answer(&answer, at()).unwrap();
    let after = native(&before);
    assert_eq!(after.plan.as_deref(), Some("max"));
    assert_eq!(
        after
            .windows
            .iter()
            .map(|window| window.used_percent)
            .collect::<Vec<_>>(),
        [36.0, 79.0, 12.0]
    );
    assert_eq!(after.windows[2].model_family.as_deref(), Some("Fable"));
    assert_eq!(
        after.windows[1].resets_at,
        Some("2026-10-14T09:00:00.000Z".parse().unwrap())
    );
    let extra = after.extra_usage.unwrap();
    assert_eq!((extra.limit, extra.used), (Some(5000.0), Some(123.0)));
    assert_eq!(extra.unit.as_deref(), Some("USD"));
    assert_eq!(
        extra.decimal_places, None,
        "the native control answer does not state a scale"
    );
    assert_eq!(after.field_times["windows.secondary:fable"], at());
}

#[test]
fn claude_native_limit_lines_keep_the_limit_and_reset_through_the_same_source_reader() {
    let record: Json = serde_json::from_str(include_str!("../../../../extensions/agents/claude-code/tests/fixtures/failures/plan_limit_weekly_dated.json")).unwrap();
    let frame = record["frames"]
        .as_array()
        .unwrap()
        .iter()
        .find(|frame| frame["type"] == "rate_limit_event")
        .unwrap();
    let before = orch_adapter_claude_code::provider::rate_limit_facts(frame).unwrap();
    let after = native(&before);
    assert_eq!(after.windows[0].used_percent, 100.0);
    assert_eq!(after.windows[0].is_active, Some(true));
    let limit = after.limit_reached.unwrap();
    assert_eq!(limit.kind, "seven_day");
    assert_eq!(
        limit.resets_at,
        Some("2026-10-14T09:00:00.000Z".parse().unwrap())
    );
    assert_eq!(after.field_times["limit_reached"], at());
}
