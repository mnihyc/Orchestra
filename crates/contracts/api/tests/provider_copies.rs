//! A provider's facts and sources as the API shows them: the provider contract's own types, and the few the API keeps under names of its own carry every member of the provider contract's.
//!
//! A balance, a row of spending, a source's descriptor and the members they are made of are the provider
//! contract's types, which the API re-exports: what the API answers of them is the JSON it answered before for
//! every known member, and the source's own `x_` members are kept as the agent contract keeps them. A few types
//! stay the API's: a choice of a source is shown with every member, empty or not, and the limit reached and the
//! credential carry names of the API's own beside other types of the same name. These tests hold each of those
//! to the provider contract's members, so that a member added there is not silently left out here.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_contract::resources as api;
use orch_provider_contract::facts as provider;
use schemars::JsonSchema;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value as Json, json};
use std::collections::BTreeSet;

/// The names of the members an object of `T` is written with.
fn members<T: JsonSchema>() -> BTreeSet<String> {
    let schema = serde_json::to_value(schemars::schema_for!(T)).unwrap();
    schema["properties"]
        .as_object()
        .expect("an object's schema names its members")
        .keys()
        .cloned()
        .collect()
}

/// The words an enumeration of `T` is written with.
fn words<T: JsonSchema>() -> BTreeSet<String> {
    fn gather(schema: &Json, out: &mut BTreeSet<String>) {
        match schema {
            Json::Object(members) => {
                if let Some(Json::String(word)) = members.get("const") {
                    out.insert(word.clone());
                }
                if let Some(Json::Array(each)) = members.get("enum") {
                    out.extend(each.iter().filter_map(Json::as_str).map(str::to_owned));
                }
                members.values().for_each(|each| gather(each, out));
            }
            Json::Array(each) => each.iter().for_each(|each| gather(each, out)),
            _ => {}
        }
    }
    let mut out = BTreeSet::new();
    gather(
        &serde_json::to_value(schemars::schema_for!(T)).unwrap(),
        &mut out,
    );
    out
}

/// What `told` is once read as `T` and written again.
fn written<T: Serialize + DeserializeOwned>(told: &Json) -> Json {
    serde_json::to_value(serde_json::from_value::<T>(told.clone()).unwrap()).unwrap()
}

#[test]
fn a_balance_and_a_row_of_spending_are_answered_as_before_with_the_sources_own_members_kept() {
    let balance = json!({"id": "plan", "label": "Plan", "unit": "USD", "remaining": "1.5", "total": "10",
        "used": "8.5", "unlimited": false, "parts": [{"label": "granted", "amount": "5"}],
        "expires_at": "2026-11-01T00:00:00.000Z", "x_note": "the source's own"});
    assert_eq!(written::<api::Balance>(&balance), balance);
    let bare = json!({"id": "account", "unit": "credits"});
    assert_eq!(
        written::<api::Balance>(&bare),
        json!({"id": "account", "label": null, "unit": "credits", "remaining": null, "total": null, "used": null,
            "unlimited": null, "parts": null, "expires_at": null}),
        "every known member is written, as it was"
    );
    let spend = json!({"id": "key:day", "label": "Today", "unit": "USD", "period": "day", "period_minutes": 1440,
        "used": "1", "limit": "5", "remaining": "4", "resets_at": "2026-10-16T00:00:00.000Z", "x_note": "kept"});
    assert_eq!(written::<api::Spend>(&spend), spend);
}

#[test]
fn each_type_the_api_keeps_names_every_member_of_the_provider_contracts() {
    assert_eq!(
        members::<api::SourceChoice>(),
        members::<orch_provider_contract::SourceChoice>()
    );
    assert_eq!(
        members::<api::ProviderFactsLimitReached>(),
        members::<provider::LimitReached>()
    );
    assert_eq!(
        members::<api::ProviderFactsCredential>(),
        members::<provider::CredentialFacts>()
    );
    assert_eq!(
        words::<api::CredentialYields>(),
        words::<provider::CredentialYield>()
    );
}

#[test]
fn a_limit_reached_and_a_credential_read_into_the_apis_types_and_are_written_the_same() {
    let reached = json!({"type": "weekly", "resets_at": "2026-10-19T00:00:00.000Z"});
    assert_eq!(
        written::<api::ProviderFactsLimitReached>(&written::<provider::LimitReached>(&reached)),
        written::<provider::LimitReached>(&reached)
    );
    for yields in ["login", "token", "key", "none"] {
        let credential = json!({"method": "oauth", "yields": yields, "expires_at": "2026-10-20T00:00:00.000Z",
            "issued_at": "2026-10-13T00:00:00.000Z"});
        let told = written::<provider::CredentialFacts>(&credential);
        assert_eq!(written::<api::ProviderFactsCredential>(&told), told);
    }
}
