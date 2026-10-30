//! The combination matrix of a provider's facts: every member of `ProviderFacts`, declared by its
//! source or not, told in two reads one after the other (absent, `null`, an empty list, or a value), for a key whose
//! reader says it can tell spending or not; and the state the member is in after both reads.
//!
//! Expected: a member present in a
//! report, even `null` or `[]`, is told that time; one absent keeps its previous value and time; then a member that
//! holds a value is *told*, one told as `null` or `[]` is *empty*, one never told is *not read yet* where its source
//! declares it (the credential always: its keeper reports it; spending where the key's reader says it can) and
//! *unsupported* where not. The two reads are merged as a machine's reports are (`merge_observation`), and the state is
//! read by the one function the API and the engine use (`member_state`).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod matrix_kit;

use matrix_kit::{Case, Matrix, Report, dim, failure};
use orch_agent_contract::facts::{FactMember, MemberState, ProviderFacts, member_state, normalize};
use orch_engine::provider_fact_reads::merge_observation;
use serde_json::{Value as Json, json};

const SOURCE: &str =
    "Provider fact merging and the told, empty, not-read-yet and unsupported member states";

const MEMBERS: &[&str] = &[
    "plan",
    "windows",
    "limit_reached",
    "ordinary_usage_allowed",
    "balances",
    "spend",
    "extra_usage",
    "available_resets",
    "identity",
    "credential",
    "credential_verified",
];

fn member(name: &str) -> FactMember {
    *FactMember::ALL
        .iter()
        .find(|each| each.as_str() == name)
        .expect("a member")
}

fn is_list(name: &str) -> bool {
    matches!(name, "windows" | "balances" | "spend")
}

/// A value each member may be told with.
fn value_of(name: &str) -> Json {
    match name {
        "plan" => json!("pro"),
        "windows" => json!([{ "id": "5h", "label": "5 hours", "used_percent": 12.5 }]),
        "limit_reached" => json!({ "type": "five_hour" }),
        "ordinary_usage_allowed" => json!(false),
        "balances" => json!([{ "id": "account", "unit": "USD", "remaining": "5.00" }]),
        "spend" => json!([{ "id": "key:day", "unit": "USD", "period": "day", "used": "1.00" }]),
        "extra_usage" => json!({ "enabled": false }),
        "available_resets" => json!({ "available_count": 0 }),
        "identity" => json!("ada@example.org"),
        "credential" => json!({ "yields": "token" }),
        _ => json!(false),
    }
}

/// One read as a machine reports it: the member (unless absent) and its time.
fn report(name: &str, told: &str, at: &str, key_usage_supported: bool) -> Json {
    let mut report = serde_json::Map::new();
    let mut times = serde_json::Map::new();
    if key_usage_supported {
        report.insert("key_usage_supported".to_owned(), json!(true));
    }
    let value = match told {
        "absent" => None,
        "null" => Some(Json::Null),
        "empty_list" => Some(json!([])),
        _ => Some(value_of(name)),
    };
    if let Some(value) = value {
        report.insert(name.to_owned(), value);
        times.insert(name.to_owned(), json!(at));
    }
    report.insert("field_times".to_owned(), Json::Object(times));
    Json::Object(report)
}

fn expected(case: &Case) -> MemberState {
    let name = case.get("member");
    let last = match (case.get("first"), case.get("second")) {
        (first, "absent") => first,
        (_, second) => second,
    };
    match last {
        "value" => MemberState::Told,
        "null" | "empty_list" => MemberState::Empty,
        _ => {
            let supported = case.is("declared", "yes")
                || name == "credential"
                || (name == "spend" && case.is("key_usage", "supported"));
            if supported {
                MemberState::NotReadYet
            } else {
                MemberState::Unsupported
            }
        }
    }
}

fn run(case: &Case) -> Result<(), matrix_kit::Failure> {
    let name = case.get("member");
    let supported = case.is("key_usage", "supported");
    let first = merge_observation(
        &json!({}),
        report(name, case.get("first"), "2026-10-15T00:00:00Z", supported),
    );
    let both = merge_observation(
        &first,
        report(name, case.get("second"), "2026-10-15T01:00:00Z", supported),
    );
    let mut facts: ProviderFacts = serde_json::from_value(both.clone()).map_err(|error| {
        failure(
            "the merged reads are facts",
            SOURCE,
            format!("{error}: {both}"),
        )
    })?;
    normalize(&mut facts);
    let declared: Vec<FactMember> = if case.is("declared", "yes") {
        vec![member(name)]
    } else {
        Vec::new()
    };
    let got = member_state(member(name), &facts, &declared);
    let want = expected(case);
    if got == want {
        Ok(())
    } else {
        Err(failure(
            format!("{want:?}"),
            SOURCE,
            format!("{got:?} after the reads {both}"),
        ))
    }
}

#[test]
fn every_member_is_in_the_state_its_reads_put_it_in() {
    let matrix = Matrix::new(
        "fact_states",
        &[
            dim("member", MEMBERS),
            dim("declared", &["yes", "no"]),
            dim("first", &["absent", "null", "empty_list", "value"]),
            dim("second", &["absent", "null", "empty_list", "value"]),
            dim("key_usage", &["unset", "supported"]),
        ],
        |pick| {
            let name = pick.get("member");
            let fits = |told: &str| match told {
                // An empty list is a list's; `null` is told of what is not a list; the keeper always reports a
                // credential, so that member is never reported as unsupported.
                "empty_list" => is_list(name),
                "null" => !is_list(name) && name != "credential",
                _ => true,
            };
            fits(pick.get("first"))
                && fits(pick.get("second"))
                && (name == "spend" || pick.is("key_usage", "unset"))
        },
    );
    // The product is small and each case is a function call: every combination runs.
    let mut cases = matrix.cases(
        19,
        &[&["member", "declared", "first", "second", "key_usage"]],
        20_261_015,
        0,
    );
    cases.retain(|case| case.kind == "full");
    let mut report = Report::default();
    for case in &cases {
        report.record(case, run(case));
    }
    report.finish("fact_states");
}
