//! Source reports keep absent, empty and unsupported data apart, retain valid siblings and never return secret proof.
#![allow(clippy::unwrap_used, reason = "a test fails by panicking")]

use orch_connector_core::sources::reports;
use orch_types::Time;
use serde_json::json;

fn at() -> Time {
    "2026-10-14T12:00:00Z".parse().unwrap()
}

#[test]
fn only_told_members_and_items_receive_this_reads_time() {
    let result = reports::read(
        &json!({
            "plan":null, "windows":[],
            "balances":[{"id":"account","unit":"USD","remaining":"12.500","x_precision":"vendor"}],
            "field_times":{"identity":"2099-01-01T00:00:00Z"}, "unknown":"ignored"
        }),
        at(),
        false,
        &[],
    )
    .unwrap();
    assert!(result.facts.get("plan").unwrap().is_null());
    assert_eq!(result.facts["windows"], json!([]));
    assert!(result.facts.get("identity").is_none());
    assert!(result.facts.get("unknown").is_none());
    assert!(result.facts["field_times"].get("identity").is_none());
    for member in ["plan", "windows", "balances", "balances.account"] {
        assert_eq!(result.facts["field_times"][member], json!(at()));
    }
    assert_eq!(result.facts["balances"][0]["remaining"], "12.500");
    assert_eq!(result.facts["balances"][0]["x_precision"], "vendor");
}

#[test]
fn a_malformed_member_or_item_does_not_discard_a_valid_siblings_value_or_date() {
    let result = reports::read(
        &json!({
            "plan":{"wrong":true},
            "windows":[{"id":"bad","label":"No percentage"},
                {"id":"good","label":"Weekly","used_percent":123.4,"duration_minutes":10080}],
            "spend":[{"id":"month","unit":"USD","period":"month","used":"1.25"},
                {"id":"invalid","unit":"USD","period":"month","used":"NaN"}]
        }),
        at(),
        false,
        &[],
    )
    .unwrap();
    assert!(result.facts.get("plan").is_none());
    assert!(result.facts["field_times"].get("plan").is_none());
    assert_eq!(result.facts["windows"][0]["id"], "good");
    assert_eq!(result.facts["windows"][0]["used_percent"], 123.4);
    assert_eq!(result.facts["spend"].as_array().unwrap().len(), 1);
    assert!(result.notes.iter().any(|note| note == "windows[0]"));
    assert!(result.notes.iter().any(|note| note == "spend[1]"));
}

#[test]
fn a_source_cannot_claim_a_credential_it_was_never_given_or_echo_a_secret() {
    let secrets = vec!["long-token-value".to_owned(), "long-token".to_owned()];
    let input = json!({"identity":"long-token-value","credential_verified":true});
    let untold = reports::read(&input, at(), false, &secrets).unwrap();
    assert!(untold.facts.get("credential_verified").is_none());
    assert!(
        untold.facts["field_times"]
            .get("credential_verified")
            .is_none()
    );
    assert_eq!(untold.facts["identity"], "[secret]");
    assert!(!untold.facts.to_string().contains("long-token"));
    let authenticated = reports::read(&input, at(), true, &secrets).unwrap();
    assert_eq!(authenticated.facts["credential_verified"], true);
    assert_eq!(
        reports::scrub_message("long-token-value", &secrets),
        "[secret]"
    );
    let error = reports::scrub_message(&format!("{} long-token-value", "文".repeat(400)), &secrets);
    assert_eq!(error.chars().count(), 300);
    assert!(!error.contains("long-token"));
}

#[test]
fn a_source_without_the_key_cannot_claim_its_expiry_either() {
    let input = json!({"plan":"Acme","credential":{"method":null,"yields":"key","expires_at":"2027-01-01T00:00:00Z"}});
    let without = reports::read(&input, at(), false, &[]).unwrap();
    assert_eq!(without.facts["plan"], "Acme");
    assert!(without.facts.get("credential").is_none());
    assert!(without.facts["field_times"].get("credential").is_none());
    let with = reports::read(&input, at(), true, &[]).unwrap();
    assert_eq!(
        with.facts["credential"]["expires_at"],
        "2027-01-01T00:00:00Z"
    );
}

#[test]
fn an_ended_cached_denial_does_not_hold_work_but_a_new_denial_does() {
    use orch_agent_contract::facts::{LimitReached, ProviderFacts};
    use orch_connector_core::sources::dispatch::exhausted;
    let ended = Time::new(at().get() - chrono::Duration::minutes(1));
    let mut facts = ProviderFacts {
        ordinary_usage_allowed: Some(false),
        limit_reached: Some(LimitReached {
            kind: "weekly".to_owned(),
            resets_at: Some(ended),
        }),
        ..ProviderFacts::default()
    };
    facts
        .field_times
        .insert("ordinary_usage_allowed".to_owned(), ended);
    assert!(!exhausted(&facts, at()));
    facts
        .field_times
        .insert("ordinary_usage_allowed".to_owned(), at());
    assert!(exhausted(&facts, at()));
}

#[test]
fn models_are_nullable_and_empty_and_optional_suggestion_fields_do_not_accept_a_model() {
    assert!(reports::models(&json!(null), &[]).unwrap().models.is_none());
    assert_eq!(
        reports::models(&json!([]), &[]).unwrap().models.unwrap(),
        Vec::new()
    );
    let result =
        reports::models(&json!([{"model_name":"offered"},{"missing":"name"}]), &[]).unwrap();
    let models = result.models.unwrap();
    assert_eq!(models.len(), 1);
    assert_eq!(models[0].model_name, "offered");
    assert_eq!(models[0].display_name, "offered");
    assert!(models[0].listed);
    assert_eq!(result.notes, ["models[1]"]);
}

#[test]
fn a_report_keeps_the_contracts_list_bound_without_discarding_a_good_scalar() {
    let windows: Vec<_> = (0..65)
        .map(|id| json!({"id":id.to_string(),"label":"Window","used_percent":1}))
        .collect();
    let result = reports::read(&json!({"plan":"pro","windows":windows}), at(), true, &[]).unwrap();
    assert_eq!(result.facts["plan"], "pro");
    assert_eq!(result.facts["windows"].as_array().unwrap().len(), 64);
    assert!(
        result
            .notes
            .iter()
            .any(|note| note == "windows[64]: too_large")
    );
}
