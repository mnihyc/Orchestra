//! Discovery as suggestions only: each model with the name that is sent and, where the endpoint's list has it, the name that is shown.
//!
//! The list of an endpoint (`GET /v1/models`) is read for the two names a model has and for what the list says
//! of it. Each entry becomes a [`ModelSuggestion`], a proposal that a manager accepts or not:
//!
//! | Member of an entry | Becomes |
//! |--------------------|---------|
//! | `id` | the routing name, taken as it is: `claude-opus-5`, `anthropic/claude-opus-5` from a gateway, a variant such as `…:free`. Nothing of it is parsed |
//! | `display_name`, else `name` | the proposed display name, which the vendor's list has for every model; the routing name where the entry has none |
//! | `max_input_tokens`, else `context_length` | the largest context the endpoint says the model takes, **on offer** (`offered.context_max`): what a thread starts with and whether it may choose more are for a manager to write |
//! | `pricing` (`prompt`, `completion`, `input_cache_read`, `input_cache_write`) | the rate for a million tokens, where a gateway's list states prices; the vendor's own list states none |
//!
//! The vendor's list pages: it gives 20 models unless it is asked for more, and at most 1000 in a page. A
//! discovery follows bounded pages ([`LIST_PATH`]); an endpoint that does not
//! know the parameter ignores it. A model the vendor lists as `retired` can no longer be called and is not
//! suggested; one it lists as `deprecated` still can be, and is.
//!
//! A gateway writes its prices in US dollars for one token, as text. The rate is the same prices for a million
//! tokens, in dollars, moved by digits and not computed, so `"0.0000001"` is `0.1` exactly; turning dollars
//! into the credits of a place is for whoever accepts the suggestion. A model whose entry has no cache price
//! has its cached tokens charged as ordinary input, which is what no discount means. A price below zero is how
//! a gateway marks a price it cannot state; that entry gets no rate, and a free variant, whose prices are zero,
//! gets a rate of nothing. Tiers a gateway lists for long prompts are not read.
//!
//! An entry without an `id` names nothing and is left out; a name listed twice is listed once.
//!
//! The same module answers the harness's own `GET /v1/models` from a lease: [`list_response`] writes the names
//! of the lease, and no others, in the shape of the vendor's list.

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, Response};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::parameters::{Offered, Rate};
use serde_json::{Value, json};

use crate::decimal;
pub use crate::requests::MODELS_PATH;

/// A discovery page of 100 models: paired with the runtime's 100-page and 10,000-model bounds.
pub const LIST_PATH: &str = "/v1/models?limit=100";

/// The lifecycle stage of a model that can no longer be called.
const RETIRED: &str = "retired";

/// When the list a lease answers says its models were created: nothing is known, and the vendor allows the
/// epoch for that.
const LISTED_CREATED: &str = "1970-01-01T00:00:00Z";

/// The request that lists the models of an endpoint, for discovery.
#[must_use]
pub fn request() -> (Method, &'static str) {
    (Method::GET, LIST_PATH)
}

/// A whole, positive number in an entry, such as a context length.
fn size(value: Option<&Value>) -> Option<i64> {
    let number = match value? {
        Value::Number(number) => number.as_i64().or_else(|| {
            number
                .as_f64()
                .filter(|fraction| fraction.is_finite())
                .map(|fraction| fraction.round() as i64)
        })?,
        Value::String(text) => text.trim().parse::<i64>().ok()?,
        _ => return None,
    };
    (number > 0).then_some(number)
}

/// A price of the list as the price for a million tokens, from a string or a number.
fn price(pricing: &Value, member: &str) -> Option<String> {
    match pricing.get(member)? {
        Value::String(written) => decimal::per_million(written),
        Value::Number(written) => decimal::per_million(&written.to_string()),
        _ => None,
    }
}

/// The rate an entry's `pricing` states, if it states one.
fn rate(pricing: &Value) -> Option<Rate> {
    let input = price(pricing, "prompt")?;
    let output = price(pricing, "completion")?;
    Some(Rate {
        cached_input_per_mtok: price(pricing, "input_cache_read").unwrap_or_else(|| input.clone()),
        cache_write_per_mtok: price(pricing, "input_cache_write"),
        input_per_mtok: input,
        output_per_mtok: output,
        tiers: None,
        above: None,
    })
}

/// The suggestion an entry makes, if it names a model that can be called.
fn suggestion(entry: &Value) -> Option<ModelSuggestion> {
    let id = entry.get("id")?.as_str().filter(|id| !id.is_empty())?;
    if entry.get("lifecycle").and_then(Value::as_str) == Some(RETIRED) {
        return None;
    }
    let display_name = ["display_name", "name"].into_iter().find_map(|member| {
        entry
            .get(member)?
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    });
    let mut suggestion = ModelSuggestion::named(id, display_name);
    // An endpoint states one number, its largest. It is on offer and nothing more: a manager opens it.
    suggestion.parameters.offered = size(entry.get("max_input_tokens"))
        .or_else(|| size(entry.get("context_length")))
        .map(|largest| Offered {
            context_max: Some(largest),
            ..Offered::default()
        });
    suggestion.parameters.rate = entry.get("pricing").and_then(rate);
    Some(suggestion)
}

/// Reads the answer to [`request`] into suggestions, in the order of the list. An answer that is not a list
/// of models suggests none.
#[must_use]
pub fn parse(body: &[u8]) -> Vec<ModelSuggestion> {
    let Ok(Value::Object(list)) = serde_json::from_slice::<Value>(body) else {
        return Vec::new();
    };
    let Some(Value::Array(entries)) = list.get("data") else {
        return Vec::new();
    };
    let mut suggestions: Vec<ModelSuggestion> = Vec::new();
    for entry in entries {
        if let Some(found) = suggestion(entry)
            && !suggestions
                .iter()
                .any(|known| known.model_name == found.model_name)
        {
            suggestions.push(found);
        }
    }
    suggestions
}

/// The answer to a harness's `GET /v1/models` for a lease: exactly the named models, in the vendor's shape,
/// all on one page.
#[must_use]
pub fn list_response(names: &[String]) -> Response<Bytes> {
    let data: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({ "type": "model", "id": name, "display_name": name, "created_at": LISTED_CREATED })
        })
        .collect();
    let body = json!({
        "data": data,
        "has_more": false,
        "first_id": names.first(),
        "last_id": names.last(),
    });
    let mut response = Response::new(Bytes::from(body.to_string()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vendor's own list, trimmed from its description: names, windows and lifecycle, and no prices.
    const VENDOR: &str = r#"{"data":[
        {"type":"model","id":"claude-opus-5","display_name":"Claude Opus 5","created_at":"2026-07-24T00:00:00Z",
         "max_input_tokens":1000000,"max_tokens":128000,"lifecycle":"active","capabilities":{"image_input":{"supported":true}}},
        {"type":"model","id":"claude-sonnet-4-5","display_name":"Claude Sonnet 4.5","created_at":"2025-09-29T00:00:00Z",
         "max_input_tokens":200000,"max_tokens":64000,"lifecycle":"deprecated","deprecated_at":"2026-09-01T00:00:00Z"},
        {"type":"model","id":"claude-2.1","display_name":"Claude 2.1","lifecycle":"retired"}],
        "first_id":"claude-opus-5","has_more":false,"last_id":"claude-2.1"}"#;

    /// A gateway's list, trimmed from entries of a real one.
    const GATEWAY: &str = r#"{"data":[
        {"id":"anthropic/claude-haiku-5.5","name":"Anthropic: Claude Haiku 5.5","context_length":1000000,
         "pricing":{"prompt":"0.0000001","completion":"0.0000005","web_search":"0.01",
                    "input_cache_read":"0.00000001","input_cache_write":"0.000000125","input_cache_write_1h":"0.0000002"}},
        {"id":"apodex/apodex-1.1-mini:free","name":"Apodex: Apodex 1.1 Mini (free)","context_length":262144,
         "pricing":{"prompt":"0","completion":"0"}},
        {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
         "pricing":{"prompt":"-1","completion":"-1"}}]}"#;

    fn names(suggestions: &[ModelSuggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.model_name.as_str()).collect()
    }

    #[test]
    fn the_vendors_list_gives_each_callable_model_with_its_shown_name_and_window() {
        let found = parse(VENDOR.as_bytes());
        assert_eq!(names(&found), ["claude-opus-5", "claude-sonnet-4-5"]);
        assert_eq!(found[0].display_name, "Claude Opus 5");
        assert_eq!(
            found[0]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(1_000_000)
        );
        assert_eq!(found[0].parameters.context_window, None);
        assert_eq!(
            found[0].parameters.rate, None,
            "the vendor's list states no prices"
        );
        assert!(found.iter().all(|s| s.listed));
    }

    #[test]
    fn a_deprecated_model_is_still_suggested_and_a_retired_one_is_not() {
        let found = parse(VENDOR.as_bytes());
        assert!(names(&found).contains(&"claude-sonnet-4-5"));
        assert!(!names(&found).contains(&"claude-2.1"));
    }

    #[test]
    fn a_gateways_list_gives_the_shown_name_the_window_and_the_rate_for_a_million_tokens() {
        let found = parse(GATEWAY.as_bytes());
        assert_eq!(found[0].model_name, "anthropic/claude-haiku-5.5");
        assert_eq!(found[0].display_name, "Anthropic: Claude Haiku 5.5");
        assert_eq!(
            found[0]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(1_000_000)
        );
        assert_eq!(found[0].parameters.context_window, None);
        let rate = found[0].parameters.rate.clone().unwrap();
        assert_eq!(rate.input_per_mtok, "0.1");
        assert_eq!(rate.cached_input_per_mtok, "0.01");
        assert_eq!(rate.output_per_mtok, "0.5");
        assert_eq!(rate.cache_write_per_mtok.as_deref(), Some("0.125"));
    }

    #[test]
    fn a_free_variant_is_rated_at_nothing_and_a_variable_price_at_no_rate_at_all() {
        let found = parse(GATEWAY.as_bytes());
        let free = found[1].parameters.rate.clone().unwrap();
        assert_eq!(
            (free.input_per_mtok.as_str(), free.output_per_mtok.as_str()),
            ("0", "0")
        );
        assert_eq!(free.cached_input_per_mtok, "0");
        assert_eq!(found[2].parameters.rate, None, "nobody said what it costs");
    }

    #[test]
    fn a_model_with_no_cache_price_charges_cached_tokens_as_input() {
        let found = parse(
            br#"{"data":[{"id":"m","pricing":{"prompt":"0.000003","completion":"0.000015"}}]}"#,
        );
        let rate = found[0].parameters.rate.clone().unwrap();
        assert_eq!(rate.cached_input_per_mtok, "3");
        assert_eq!(rate.cache_write_per_mtok, None, "absent means as input");
    }

    #[test]
    fn entries_that_name_nothing_are_left_out_and_a_name_is_listed_once() {
        let body = br#"{"data":[{"id":"a"},{"type":"model"},{"id":""},{"id":7},null,{"id":"a","display_name":"Again"},{"id":"b"}]}"#;
        let found = parse(body);
        assert_eq!(names(&found), ["a", "b"]);
        assert_eq!(
            found[0].display_name, "a",
            "the first entry of a name counts"
        );
    }

    #[test]
    fn what_is_not_a_list_of_models_suggests_none() {
        for body in [
            &b""[..],
            b"not json",
            b"[]",
            b"{}",
            br#"{"data":{}}"#,
            br#"{"type":"error","error":{"type":"authentication_error","message":"no"}}"#,
        ] {
            assert!(parse(body).is_empty(), "{}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn a_discovery_asks_for_the_biggest_page_so_that_one_request_sees_every_model() {
        let (method, path) = request();
        assert_eq!(method, Method::GET);
        assert_eq!(path, "/v1/models?limit=100");
        assert!(
            crate::requests::allowed(&method, path),
            "the request is one the format lets through"
        );
    }

    #[test]
    fn the_list_a_lease_answers_holds_exactly_its_models_in_the_vendors_shape() {
        let response = list_response(&["claude-opus-5".to_owned(), "vendor/x:free".to_owned()]);
        assert_eq!(response.status(), 200);
        let body: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(body["has_more"], false);
        assert_eq!(body["first_id"], "claude-opus-5");
        assert_eq!(body["last_id"], "vendor/x:free");
        let again = parse(response.body());
        assert_eq!(names(&again), ["claude-opus-5", "vendor/x:free"]);
        assert_eq!(again[1].display_name, "vendor/x:free");
        let empty: Value = serde_json::from_slice(list_response(&[]).body()).unwrap();
        assert_eq!(empty["data"], json!([]));
        assert_eq!(empty["first_id"], Value::Null);
    }
}
