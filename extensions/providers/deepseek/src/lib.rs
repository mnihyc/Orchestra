//! DeepSeek's documented key balance, retaining the vendor's decimal text and currency parts.

use orch_plugin_contract::Secret;
use orch_provider_contract::builtins::VendorClient;
use orch_types::{Clock, Json};
use serde_json::json;
use std::collections::BTreeSet;
use std::time::Duration;
use url::Url;

pub mod source;
pub use source::{DeepSeek, builtin};

/// The vendor's description of the response this source reads.
pub const DOCUMENTATION: &str = "https://api-docs.deepseek.com/api/get-user-balance/";
/// The source answers within an interactive provider read's budget.
const READ_TIMEOUT: Duration = orch_provider_contract::wire::READ_DEADLINE;
/// A balance response is small; an unexpected reply must not grow without a bound.
const REPLY_BYTES: usize = orch_provider_contract::limits::REPORT_MAX_BYTES;
/// A source report admits at most this many balances, independent of the vendor's present currency set.
const MAX_BALANCES: usize = orch_provider_contract::limits::BALANCES_MAX;

/// Whether this is the vendor's own documented base, with no alternate port or embedded authorization.
#[must_use]
pub fn known_vendor(base: &Url) -> bool {
    base.scheme() == "https"
        && base.host_str() == Some("api.deepseek.com")
        && base.port_or_known_default() == Some(443)
        && matches!(base.path().trim_end_matches('/'), "" | "/anthropic")
        && base.username().is_empty()
        && base.password().is_none()
        && base.query().is_none()
        && base.fragment().is_none()
}

fn decimal(value: Option<&Json>) -> Option<&str> {
    let text = value?.as_str()?;
    orch_provider_contract::limits::is_decimal(text).then_some(text)
}

/// Reads the documented response, leaving a refusal or a changed response shape untold.
/// The caller must have used the provider's key; the common report reader enforces that proof boundary too.
#[must_use]
pub fn balance(value: &Json) -> Option<Json> {
    let allowed = value.get("is_available")?.as_bool()?;
    let infos = value.get("balance_infos")?.as_array()?;
    if infos.len() > MAX_BALANCES {
        return None;
    }
    let mut ids = BTreeSet::new();
    let mut balances = Vec::with_capacity(infos.len());
    for info in infos {
        let currency = info.get("currency")?.as_str()?;
        if !matches!(currency, "CNY" | "USD") || !ids.insert(currency) {
            return None;
        }
        let total = decimal(info.get("total_balance"))?;
        let granted = decimal(info.get("granted_balance"))?;
        let topped_up = decimal(info.get("topped_up_balance"))?;
        balances.push(json!({
            "id":currency, "unit":currency, "remaining":total,
            "parts":[{"label":"Granted","amount":granted},{"label":"Topped up","amount":topped_up}]
        }));
    }
    Some(json!({"balances":balances,"ordinary_usage_allowed":allowed,"credential_verified":true}))
}

/// Reads a key's current balance through the keeping machine's transport; the caller selects this source after
/// checking its base or an explicit choice.
pub async fn read(
    client: &dyn VendorClient,
    base: &Url,
    key: &Secret,
    clock: &dyn Clock,
) -> Option<Json> {
    let reply = tokio::time::timeout(
        READ_TIMEOUT,
        client.get(base, "/user/balance", key, REPLY_BYTES),
    )
    .await
    .ok()
    .flatten()?;
    let mut report = balance(&reply)?;
    let mut times = serde_json::Map::new();
    for member in ["balances", "ordinary_usage_allowed", "credential_verified"] {
        times.insert(member.to_owned(), json!(clock.time()));
    }
    if let Some(balances) = report["balances"].as_array() {
        for entry in balances {
            if let Some(id) = entry["id"].as_str() {
                times.insert(format!("balances.{id}"), json!(clock.time()));
            }
        }
    }
    report["field_times"] = Json::Object(times);
    Some(report)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "a test fails by panicking")]
    use super::*;

    fn documented() -> Json {
        json!({"is_available":true,"balance_infos":[{"currency":"CNY","total_balance":"110.00","granted_balance":"10.00","topped_up_balance":"100.00"}]})
    }

    #[test]
    fn the_documented_reply_keeps_the_balance_and_both_parts_in_the_vendors_currency() {
        let report = balance(&documented()).unwrap();
        assert_eq!(report["ordinary_usage_allowed"], true);
        assert_eq!(report["credential_verified"], true);
        assert_eq!(report["balances"][0]["remaining"], "110.00");
        assert_eq!(report["balances"][0]["unit"], "CNY");
        assert_eq!(
            report["balances"][0]["parts"],
            json!([{"label":"Granted","amount":"10.00"},{"label":"Topped up","amount":"100.00"}])
        );
    }

    #[test]
    fn zero_balance_and_unavailable_calls_are_told_without_becoming_an_unknown_read() {
        let mut reply = documented();
        reply["is_available"] = json!(false);
        reply["balance_infos"][0]["total_balance"] = json!("0.00");
        let report = balance(&reply).unwrap();
        assert_eq!(report["ordinary_usage_allowed"], false);
        assert_eq!(report["balances"][0]["remaining"], "0.00");
    }

    #[test]
    fn a_reshaped_response_never_invents_a_zero_balance_or_credential_proof() {
        assert!(balance(&json!({"error":"a refused read"})).is_none());
        assert!(balance(&json!({"is_available":true,"data":{"balance":110}})).is_none());
        for amount in ["NaN", "1e3", "", ".5", "1.", "+2", "1.2.3"] {
            let mut reply = documented();
            reply["balance_infos"][0]["total_balance"] = json!(amount);
            assert!(balance(&reply).is_none(), "{amount}");
        }
    }

    #[test]
    fn only_the_documented_vendor_bases_select_this_reader_by_default() {
        for base in [
            "https://api.deepseek.com",
            "https://api.deepseek.com/anthropic/",
        ] {
            assert!(known_vendor(&Url::parse(base).unwrap()));
        }
        for base in [
            "http://api.deepseek.com",
            "https://api.deepseek.com.example.invalid",
            "https://api.deepseek.com:444",
            "https://user@api.deepseek.com",
            "https://api.deepseek.com/other",
            "https://api.deepseek.com?token=example",
        ] {
            assert!(!known_vendor(&Url::parse(base).unwrap()), "{base}");
        }
    }
}
