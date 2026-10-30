//! Synthetic quota reports for native control and turn handling; model contexts preserve explicit values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use futures::StreamExt as _;
use orch_adapter_claude_code::config::ProfileConfig;
use orch_adapter_claude_code::process::{LaunchSpec, Launcher};
use orch_adapter_claude_code::provider;
use orch_adapter_claude_code::sessions::HarnessSetup;
use orch_agent_contract::adapter::{AdapterEvent, EventSink, EventStream};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::loopback::Loopback;
use orch_plugin_contract::Secret;
use orch_types::{Json, Time};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug)]
struct Harness {
    directory: tempfile::TempDir,
    mode: &'static str,
    launches: Mutex<Vec<LaunchSpec>>,
}

impl Harness {
    fn new(mode: &'static str) -> Self {
        let directory = tempfile::tempdir().unwrap();
        let harness = Self {
            directory,
            mode,
            launches: Mutex::new(Vec::new()),
        };
        harness.answer(synthetic_usage());
        harness
    }

    fn setup(&self) -> HarnessSetup {
        HarnessSetup {
            program: "recorded-claude".to_owned(),
            home: Some(self.directory.path().join("home")),
            managed_home: true,
            loopback: Loopback::new(41000, Secret::new("test-loopback")),
            profile_env: BTreeMap::new(),
            config: ProfileConfig::default(),
            inherited: BTreeMap::new(),
            as_root: false,
        }
    }

    fn requests(&self) -> Vec<Json> {
        std::fs::read_to_string(self.directory.path().join("requests.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect()
    }

    fn answer(&self, body: Json) {
        std::fs::write(
            self.directory.path().join("answer.json"),
            json!({"response":{"subtype":"success","request_id":"usage","response":body}})
                .to_string(),
        )
        .unwrap();
    }
}

impl Launcher for Harness {
    fn command(&self, spec: &LaunchSpec) -> tokio::process::Command {
        self.launches.lock().unwrap().push(spec.clone());
        let mut command = tokio::process::Command::new("python3");
        command
            .arg("-u")
            .arg(
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures/provider/harness.py"),
            )
            .arg(self.directory.path().join("answer.json"))
            .arg(self.directory.path().join("requests.jsonl"))
            .arg(self.mode)
            .arg(&spec.label)
            .env_clear()
            .envs(&spec.env)
            .current_dir(&spec.cwd);
        command
    }
}

fn now() -> Time {
    "2026-10-13T09:00:00.000Z".parse().unwrap()
}

/// These protocol examples are synthetic, independent of any account or captured reply.
/// Reset times follow the declared window durations from the deterministic test clock.
fn synthetic_reset(after: chrono::Duration) -> Time {
    Time::new(now().get() + after)
}

fn synthetic_usage() -> Json {
    let short_reset = synthetic_reset(chrono::Duration::hours(5));
    let weekly_reset = synthetic_reset(chrono::Duration::days(7));
    let limit = 5000.0;
    let used = 123.0;
    json!({
        "subscription_type": "max",
        "rate_limits_available": true,
        "rate_limits": {
            "five_hour": {"utilization": 25.0, "resets_at": short_reset},
            "seven_day": {"utilization": 79.0, "resets_at": weekly_reset},
            "seven_day_opus": null,
            "seven_day_sonnet": null,
            "seven_day_oauth_apps": null,
            "model_scoped": [{"display_name": "Fable", "utilization": 12.0, "resets_at": weekly_reset}],
            "extra_usage": {
                "is_enabled": false, "monthly_limit": limit, "used_credits": used,
                "utilization": used / limit * 100.0, "currency": "USD"
            }
        }
    })
}

#[tokio::test]
async fn the_native_control_answer_tells_the_same_windows_families_and_extra_spending_without_a_turn()
 {
    let harness = Harness::new("answer");
    let mut setup = harness.setup();
    setup.profile_env.insert(
        "CLAUDE_CONFIG_DIR".to_owned(),
        "/another-persons-home".to_owned(),
    );
    setup.profile_env.insert(
        "CLAUDE_CODE_OAUTH_TOKEN".to_owned(),
        "another-token".to_owned(),
    );
    let facts = provider::native_usage(&harness, &setup, Some("made-up-access"), now())
        .await
        .unwrap();
    assert_eq!(facts.windows.len(), 3);
    assert_eq!(facts.credential_verified, Some(true));
    assert_eq!(
        (&facts.windows[0].id, &facts.windows[0].label),
        (&"primary".to_owned(), &"5h".to_owned())
    );
    assert_eq!(facts.windows[1].used_percent, 79.0);
    assert_eq!(
        facts.windows[1].is_active, None,
        "this control answer does not report the binding window"
    );
    assert_eq!(facts.windows[1].severity, None);
    assert_eq!(facts.windows[1].warning_percent, Some(75.0));
    let extra = facts.extra_usage.as_ref().unwrap();
    assert!(!extra.enabled);
    assert_eq!(
        extra.disabled_reason, None,
        "a turn frame may supply the reason later"
    );
    assert_eq!(facts.windows[2].model_family.as_deref(), Some("Fable"));
    assert_eq!(facts.windows[2].label, "weekly");
    let extra = facts.extra_usage.unwrap();
    assert!(!extra.enabled);
    assert_eq!(
        (extra.limit, extra.used, extra.decimal_places),
        (Some(5000.0), Some(123.0), None)
    );
    assert_eq!(extra.unit.as_deref(), Some("USD"));
    assert_eq!(facts.plan.as_deref(), Some("max"));
    assert_eq!(facts.field_times["windows.primary"], now());
    assert_eq!(facts.field_times["extra_usage"], now());
    let requests = harness.requests();
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0]["request"]["subtype"], "initialize");
    assert_eq!(
        requests[1]["request"],
        json!({"subtype":"get_usage","skip_behaviors":true})
    );
    assert!(
        requests
            .iter()
            .all(|request| request["type"] == "control_request")
    );
    assert!(
        !serde_json::to_string(&requests)
            .unwrap()
            .contains("made-up-access")
    );
    let launches = harness.launches.lock().unwrap();
    assert_eq!(
        launches[0].env["CLAUDE_CONFIG_DIR"],
        setup.home.as_ref().unwrap().display().to_string()
    );
    assert_eq!(
        launches[0].env["CLAUDE_CODE_OAUTH_TOKEN"], "made-up-access",
        "only the native process receives its own sign-in"
    );
}

#[tokio::test]
async fn a_refusal_or_an_unknown_native_shape_leaves_usage_unread_and_unknown_members_do_not_hide_known_windows()
 {
    let refused = Harness::new("refused");
    let facts = provider::facts(&refused, &refused.setup(), None, now())
        .await
        .unwrap();
    assert!(facts.windows.is_empty());
    assert!(facts.extra_usage.is_none());
    assert_eq!(facts.credential_verified, None);
    assert!(!facts.field_times.contains_key("windows"));
    assert!(!facts.field_times.contains_key("extra_usage"));
    let harness = Harness::new("answer");
    harness.answer(json!({"next_format":{"percent_used":60}}));
    assert!(
        provider::native_usage(&harness, &harness.setup(), None, now())
            .await
            .is_none()
    );
    let mut body = synthetic_usage();
    body["unrecognised"] = json!({"a":"future field"});
    body["rate_limits"]["model_scoped"]
        .as_array_mut()
        .unwrap()
        .push(json!({"unknown_kind":"future_kind","utilization":88}));
    harness.answer(body);
    assert_eq!(
        provider::native_usage(&harness, &harness.setup(), None, now())
            .await
            .unwrap()
            .windows
            .len(),
        3
    );
}

#[tokio::test]
async fn an_unanswered_native_request_ends_and_leaves_usage_not_read_yet() {
    let harness = Harness::new("unanswered");
    let facts = tokio::time::timeout(
        std::time::Duration::from_secs(12),
        provider::facts(&harness, &harness.setup(), None, now()),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        harness
            .requests()
            .iter()
            .any(|frame| frame["request"]["subtype"] == "get_usage")
    );
    assert!(facts.windows.is_empty());
    assert!(facts.extra_usage.is_none());
    assert!(facts.limit_reached.is_none());
    assert_eq!(facts.credential_verified, None);
    assert!(!facts.field_times.contains_key("windows"));
    assert!(!facts.field_times.contains_key("credential_verified"));
}

#[tokio::test]
async fn refresh_asks_the_harness_again_and_an_unavailable_read_keeps_older_usage_and_its_time() {
    let harness = Harness::new("answer");
    let setup = harness.setup();
    let first = provider::facts(&harness, &setup, None, now())
        .await
        .unwrap();
    let later = Time::new(now().get() + chrono::Duration::minutes(2));
    harness.answer(json!({"subscription_type":"max","rate_limits_available":false,"rate_limits":null,"session":{"total_cost_usd":999}}));
    let second = provider::facts(&harness, &setup, None, later)
        .await
        .unwrap();
    assert_eq!(second.windows, first.windows);
    assert_eq!(second.extra_usage, first.extra_usage);
    assert_eq!(second.field_times["windows.secondary"], now());
    assert_eq!(second.field_times["extra_usage"], now());
    assert_eq!(second.credential_verified, None);
    let mut body = synthetic_usage();
    body["rate_limits"]["seven_day"]["utilization"] = json!(80);
    harness.answer(body);
    let third = provider::facts(&harness, &setup, None, later)
        .await
        .unwrap();
    assert_eq!(third.windows[1].used_percent, 80.0);
    assert_eq!(third.field_times["windows.secondary"], later);
    assert_eq!(
        harness
            .requests()
            .iter()
            .filter(|frame| frame["request"]["subtype"] == "get_usage")
            .count(),
        3
    );
}

#[test]
fn a_changed_experimental_member_does_not_invent_zeroes_or_hide_other_told_members() {
    let mut body = synthetic_usage();
    body["rate_limits"]["five_hour"]["utilization"] = Json::Null;
    body["rate_limits"]["model_scoped"]
        .as_array_mut()
        .unwrap()
        .push(json!({"display_name":"Not yet read","utilization":null,"resets_at":null}));
    let facts = provider::usage_answer(&body, now()).unwrap();
    assert_eq!(facts.windows.len(), 2);
    assert_eq!(facts.windows[0].id, "secondary");
    assert!(!facts.field_times.contains_key("windows.primary"));
    assert_eq!(facts.extra_usage.unwrap().used, Some(123.0));
    assert!(provider::usage_answer(&json!({"session":{"total_cost_usd":999},"rate_limits_available":false,"rate_limits":null}), now()).is_none());
}

#[tokio::test]
async fn explicitly_empty_extra_usage_differs_from_an_omitted_or_unavailable_read() {
    let harness = Harness::new("answer");
    let setup = harness.setup();
    let first = provider::facts(&harness, &setup, None, now())
        .await
        .unwrap();
    assert!(first.extra_usage.is_some());
    let later = Time::new(now().get() + chrono::Duration::minutes(2));
    let mut body = synthetic_usage();
    body["rate_limits"]
        .as_object_mut()
        .unwrap()
        .remove("extra_usage");
    harness.answer(body.clone());
    let omitted = provider::facts(&harness, &setup, None, later)
        .await
        .unwrap();
    assert_eq!(omitted.extra_usage, first.extra_usage);
    assert_eq!(omitted.field_times["extra_usage"], now());
    body["rate_limits"]["extra_usage"] = Json::Null;
    harness.answer(body);
    let empty = provider::facts(&harness, &setup, None, later)
        .await
        .unwrap();
    assert!(empty.extra_usage.is_none());
    assert_eq!(empty.field_times["extra_usage"], later);
}

#[test]
fn the_adapter_has_no_http_client_or_vendor_address_for_reading_plan_usage() {
    let runtime_dependencies = include_str!("../Cargo.toml")
        .split("[dev-dependencies]")
        .next()
        .unwrap();
    assert!(!runtime_dependencies.contains("reqwest"));
    let adapter = include_str!("../src/provider.rs");
    assert!(!adapter.contains("reqwest::"));
    assert!(!adapter.contains("api.anthropic.com"));
    assert!(!adapter.contains("api/oauth/usage"));
}

/// Turn notices express synthetic use as fractions; control answers express percentages.
fn synthetic_rate_frame() -> Json {
    let short_reset = synthetic_reset(chrono::Duration::hours(5))
        .get()
        .timestamp();
    let weekly_reset = synthetic_reset(chrono::Duration::days(7)).get().timestamp();
    json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed_warning", "rateLimitType": "seven_day",
            "utilization": 78.0 / 100.0, "resetsAt": weekly_reset,
            "surpassedThreshold": 75.0 / 100.0,
            "overageStatus": "rejected", "overageDisabledReason": "org_level_disabled",
            "isUsingOverage": false, "overageEnabled": false,
            "unifiedWindows": {
                "five_hour": {"utilization": 32.0 / 100.0, "resetsAt": short_reset},
                "seven_day": {"utilization": 78.0 / 100.0, "resetsAt": weekly_reset}
            }
        }
    })
}

async fn next_facts(events: &mut EventStream) -> orch_agent_contract::facts::ProviderFacts {
    let AdapterEvent::Facts(facts) =
        tokio::time::timeout(std::time::Duration::from_secs(2), events.next())
            .await
            .unwrap()
            .unwrap()
    else {
        panic!("the provider receives the observation");
    };
    facts
}

#[tokio::test]
async fn the_synthetic_rate_frame_refreshes_the_same_provider_windows_between_vendor_reads() {
    let home = tempfile::tempdir().unwrap();
    let (sink, mut events) = EventSink::channel();
    futures::executor::block_on(provider::observe_frame(
        &synthetic_rate_frame(),
        Some(home.path()),
        &sink,
        now(),
    ));
    let facts = next_facts(&mut events).await;
    assert_eq!(
        facts
            .windows
            .iter()
            .map(|window| window.id.as_str())
            .collect::<Vec<_>>(),
        ["primary", "secondary"]
    );
    assert_eq!(facts.windows[0].used_percent, 32.0);
    assert_eq!(facts.windows[1].used_percent, 78.0);
    assert_eq!(facts.windows[1].is_active, Some(true));
    assert_eq!(facts.windows[1].warning_percent, Some(75.0));
    assert!(
        facts
            .windows
            .iter()
            .all(|window| window.resets_at.is_some())
    );
    assert!(
        !facts.is_exhausted(),
        "a warning is display data and is not exhaustion"
    );
}

#[tokio::test]
async fn turn_lines_update_the_native_read_without_losing_its_plan_family_or_credit_amounts() {
    let harness = Harness::new("answer");
    let setup = harness.setup();
    let first = provider::facts(&harness, &setup, None, now())
        .await
        .unwrap();
    let later = Time::new(now().get() + chrono::Duration::minutes(1));
    let (sink, mut events) = EventSink::channel();
    futures::executor::block_on(provider::observe_frame(
        &synthetic_rate_frame(),
        setup.home.as_deref(),
        &sink,
        later,
    ));
    let facts = next_facts(&mut events).await;
    assert_eq!(facts.plan, first.plan);
    assert_eq!(facts.identity, first.identity);
    assert_eq!(
        facts
            .windows
            .iter()
            .map(|window| window.used_percent)
            .collect::<Vec<_>>(),
        [32.0, 78.0, 12.0]
    );
    assert_eq!(facts.windows[1].is_active, Some(true));
    assert_eq!(
        facts.windows[1].severity.as_deref(),
        Some("allowed_warning")
    );
    assert_eq!(facts.field_times["windows.secondary"], later);
    assert_eq!(facts.field_times["windows.secondary:fable"], now());
    let extra = facts.extra_usage.unwrap();
    assert_eq!((extra.limit, extra.used), (Some(5000.0), Some(123.0)));
    assert_eq!(extra.disabled_reason.as_deref(), Some("org_level_disabled"));
    assert_eq!(facts.field_times["extra_usage"], now());
    assert_eq!(facts.field_times["extra_usage.disabled_reason"], later);
}

#[tokio::test]
async fn a_partial_rate_frame_keeps_an_untold_window_and_disabled_reason_with_their_source_times() {
    let home = tempfile::tempdir().unwrap();
    let (sink, mut events) = EventSink::channel();
    futures::executor::block_on(provider::observe_frame(
        &synthetic_rate_frame(),
        Some(home.path()),
        &sink,
        now(),
    ));
    let before = next_facts(&mut events).await;
    let later = Time::new(now().get() + chrono::Duration::minutes(5));
    let mut frame = synthetic_rate_frame();
    frame["rate_limit_info"]
        .as_object_mut()
        .unwrap()
        .remove("overageDisabledReason");
    frame["rate_limit_info"]["unifiedWindows"] = json!({"five_hour":{"utilization":0.35,"resetsAt":synthetic_reset(chrono::Duration::hours(5)).get().timestamp()}});
    futures::executor::block_on(provider::observe_frame(
        &frame,
        Some(home.path()),
        &sink,
        later,
    ));
    let facts = next_facts(&mut events).await;
    assert_eq!(facts.windows[0].used_percent, 35.0);
    assert_eq!(facts.windows[1].used_percent, 78.0);
    assert_eq!(facts.field_times["windows.primary"], later);
    assert_eq!(
        facts.field_times["windows.secondary"],
        before.field_times["windows.secondary"]
    );
    assert_eq!(facts.field_times["extra_usage.enabled"], later);
    assert_eq!(
        facts.field_times["extra_usage.disabled_reason"],
        before.field_times["extra_usage.disabled_reason"]
    );
    assert_eq!(
        facts.extra_usage.unwrap().disabled_reason.as_deref(),
        Some("org_level_disabled")
    );
}

#[test]
fn a_result_makes_the_model_context_known_and_keeps_a_context_a_manager_wrote() {
    let home = tempfile::tempdir().unwrap();
    let (sink, _events) = EventSink::channel();
    futures::executor::block_on(provider::observe_frame(
        &json!({"type":"result","modelUsage":{
            "claude-haiku-5-5":{"contextWindow":1000000},"explicit-model":{"contextWindow":1000000}
        }}),
        Some(home.path()),
        &sink,
        now(),
    ));
    let mut found = vec![
        ModelSuggestion::named("claude-haiku-5-5", None),
        ModelSuggestion::named("explicit-model", None),
        ModelSuggestion::named("unknown-model", None),
    ];
    found[1].parameters.context_window = Some(200000);
    provider::add_context_windows(&mut found, Some(home.path()));
    assert_eq!(found[0].parameters.context_window, Some(1000000));
    assert_eq!(found[1].parameters.context_window, Some(200000));
    assert_eq!(found[2].parameters.context_window, None);
}

#[test]
fn usage_screen_percentages_and_native_fractions_are_normalized_by_their_source() {
    let endpoint =
        provider::usage_from(&json!({"five_hour":{"utilization":0.32,"resets_at":null}})).unwrap();
    assert_eq!(endpoint.windows[0].used_percent, 0.32);
    let native = provider::rate_limit_facts(&synthetic_rate_frame()).unwrap();
    assert_eq!(native.windows[0].used_percent, 32.0);
    let over =
        provider::usage_from(&json!({"five_hour":{"utilization":125.0,"resets_at":null}})).unwrap();
    assert_eq!(over.windows[0].used_percent, 125.0);
    assert!(
        !over.is_exhausted(),
        "a percentage is not the vendor's admission decision"
    );
}

#[tokio::test]
async fn the_pinned_limit_lines_keep_the_vendors_decision_and_reset_even_when_it_tells_no_percentage()
 {
    for (fixture, kind, reset) in [
        (
            include_str!("fixtures/failures/plan_limit_weekly_dated.json"),
            "seven_day",
            Some("2026-10-14T09:00:00.000Z".parse::<Time>().unwrap()),
        ),
        (
            include_str!("fixtures/failures/plan_limit_without_a_time.json"),
            "five_hour",
            None,
        ),
    ] {
        let recorded: Json = serde_json::from_str(fixture).unwrap();
        let frame = recorded["frames"]
            .as_array()
            .unwrap()
            .iter()
            .find(|frame| frame["type"] == "rate_limit_event")
            .unwrap();
        let home = tempfile::tempdir().unwrap();
        let (sink, mut events) = EventSink::channel();
        futures::executor::block_on(provider::observe_frame(
            frame,
            Some(home.path()),
            &sink,
            now(),
        ));
        let facts = next_facts(&mut events).await;
        let limit = facts.limit_reached.unwrap();
        assert_eq!(limit.kind, kind);
        assert_eq!(limit.resets_at, reset);
        assert_eq!(facts.field_times["limit_reached"], now());
        if reset.is_none() {
            assert!(
                facts.windows.is_empty(),
                "the vendor supplied no percentage to make a window"
            );
            assert!(!facts.field_times.contains_key("windows"));
        } else {
            assert_eq!(facts.windows[0].used_percent, 100.0);
            assert_eq!(facts.windows[0].resets_at, reset);
        }
        let later = Time::new(now().get() + chrono::Duration::minutes(1));
        futures::executor::block_on(provider::observe_frame(
            &json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed"}}),
            Some(home.path()),
            &sink,
            later,
        ));
        let admitted = next_facts(&mut events).await;
        assert!(admitted.limit_reached.is_none());
        assert_eq!(admitted.field_times["limit_reached"], later);
        assert_eq!(
            admitted.windows, facts.windows,
            "an admission-only frame leaves untold windows alone"
        );
    }
}
