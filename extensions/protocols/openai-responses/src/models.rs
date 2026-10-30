//! Discovery as suggestions only: each model with the name that is sent and, where the endpoint's list has them, the name that is shown and its rate.
//!
//! The list of an endpoint (`GET /models`) is read for the two names a model has and for what the list says of
//! it. Each entry becomes a [`ModelSuggestion`], a proposal that a manager accepts or not:
//!
//! | Member of an entry | Becomes |
//! |--------------------|---------|
//! | `id` | the routing name, taken as it is: `gpt-6-sol`, `openai/gpt-6-sol` from a gateway, a variant such as `…:free`. Nothing of it is parsed |
//! | `name`, else `display_name` | the proposed display name; the routing name where the entry has none, which is what the vendor's own list does |
//! | `context_length` | the largest context the endpoint says the model takes, **on offer** (`offered.context_max`): what a thread starts with and whether it may choose more are for a manager to write |
//! | `pricing` (`prompt`, `completion`, `input_cache_read`, `input_cache_write`) | the rate for a million tokens |
//!
//! A gateway writes its prices in US dollars for one token, as text. The rate is the same prices for a million
//! tokens, in dollars, moved by digits and not computed, so `"0.0000002"` is `0.2` exactly; turning dollars
//! into the credits of a place is for whoever accepts the suggestion. A model whose entry has no cache price
//! has its cached tokens charged as ordinary input, which is what no discount means. A price below zero is how
//! a gateway marks a price it cannot state (a router that picks the model); that entry gets no rate, and a free
//! variant, whose prices are zero, gets a rate of nothing. Tiers a gateway lists for long prompts are not read.
//!
//! An entry without an `id` names nothing and is left out; a name listed twice is listed once.
//!
//! The same module answers the harness's own `GET /models` from a lease: [`list_response`] writes the names of
//! the lease, and no others, in the shape of the vendor's list.

use bytes::Bytes;
use http::header::CONTENT_TYPE;
use http::{HeaderValue, Method, Response};
use orch_agent_contract::discovery::ModelSuggestion;
use orch_agent_contract::parameters::{Offered, Rate};
use serde_json::{Value, json};

use crate::decimal;
pub use crate::requests::MODELS_PATH;

/// What the list a lease answers says of when its models were created: nothing is known, so the epoch.
const LISTED_CREATED: i64 = 0;

/// Who the list a lease answers says owns its models.
const LISTED_OWNER: &str = "orch";

/// The request that lists the models of an endpoint, for discovery.
#[must_use]
pub fn request() -> (Method, &'static str) {
    (Method::GET, MODELS_PATH)
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

/// The suggestion an entry makes, if it names a model.
fn suggestion(entry: &Value) -> Option<ModelSuggestion> {
    let id = entry.get("id")?.as_str().filter(|id| !id.is_empty())?;
    let display_name = ["name", "display_name"].into_iter().find_map(|member| {
        entry
            .get(member)?
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty())
    });
    let mut suggestion = ModelSuggestion::named(id, display_name);
    // An endpoint states one number, its largest. It is on offer and nothing more: a manager opens it.
    suggestion.parameters.offered = size(entry.get("context_length")).map(|largest| Offered {
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

/// The answer to a harness's `GET /models` for a lease: exactly the named models, in the vendor's shape.
#[must_use]
pub fn list_response(names: &[String]) -> Response<Bytes> {
    let data: Vec<Value> = names
        .iter()
        .map(|name| {
            json!({ "id": name, "object": "model", "created": LISTED_CREATED, "owned_by": LISTED_OWNER })
        })
        .collect();
    let body = json!({ "object": "list", "data": data });
    let mut response = Response::new(Bytes::from(body.to_string()));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vendor's own list: ids and nothing to propose beyond them.
    const VENDOR: &str = r#"{"object":"list","data":[
        {"id":"gpt-6-sol","object":"model","created":1686935002,"owned_by":"openai"},
        {"id":"gpt-6-luna","object":"model","created":1686935003,"owned_by":"openai"}]}"#;

    /// A gateway's list, trimmed from entries of a real one.
    const GATEWAY: &str = r#"{"data":[
        {"id":"openai/gpt-5.6-luna","name":"OpenAI: GPT-5.6 Luna","context_length":1050000,
         "pricing":{"prompt":"0.0000002","completion":"0.0000012","web_search":"0.01",
                    "input_cache_read":"0.00000002","input_cache_write":"0.00000025",
                    "overrides":[{"min_prompt_tokens":272000,"prompt":"0.0000004"}]}},
        {"id":"deepseek/deepseek-v4.1-flash","name":"DeepSeek: DeepSeek V4.1 Flash","context_length":1048576,
         "pricing":{"prompt":"0.0000003","completion":"0.0000012","input_cache_read":"0.000000006"}},
        {"id":"apodex/apodex-1.1-mini:free","name":"Apodex: Apodex 1.1 Mini (free)","context_length":262144,
         "pricing":{"prompt":"0","completion":"0"}},
        {"id":"openrouter/auto","name":"Auto Router","context_length":2000000,
         "pricing":{"prompt":"-1","completion":"-1"}},
        {"id":"vendor/plain","pricing":{"prompt":0.000001,"completion":2e-6}}]}"#;

    fn names(suggestions: &[ModelSuggestion]) -> Vec<&str> {
        suggestions.iter().map(|s| s.model_name.as_str()).collect()
    }

    #[test]
    fn the_vendors_list_gives_each_model_by_the_name_that_is_sent_and_proposes_nothing_more() {
        let found = parse(VENDOR.as_bytes());
        assert_eq!(names(&found), ["gpt-6-sol", "gpt-6-luna"]);
        assert_eq!(
            found[0].display_name, "gpt-6-sol",
            "the routing name is shown where the list has no name"
        );
        assert!(found.iter().all(|s| s.listed));
        assert_eq!(found[0].parameters.rate, None);
        assert_eq!(found[0].parameters.context_window, None);
        assert_eq!(found[0].parameters.offered, None);
    }

    #[test]
    fn a_gateways_list_gives_the_shown_name_the_largest_context_on_offer_and_the_rate_for_a_million_tokens()
     {
        let found = parse(GATEWAY.as_bytes());
        assert_eq!(found[0].model_name, "openai/gpt-5.6-luna");
        assert_eq!(found[0].display_name, "OpenAI: GPT-5.6 Luna");
        assert_eq!(
            found[0]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(1_050_000),
            "the endpoint's one number is the largest it takes, on offer and not open"
        );
        assert_eq!(
            found[0].parameters.context_window, None,
            "what a thread starts with is for a manager to write"
        );
        assert_eq!(found[0].parameters.context_max, None);
        let rate = found[0].parameters.rate.clone().unwrap();
        assert_eq!(rate.input_per_mtok, "0.2");
        assert_eq!(rate.cached_input_per_mtok, "0.02");
        assert_eq!(rate.output_per_mtok, "1.2");
        assert_eq!(rate.cache_write_per_mtok.as_deref(), Some("0.25"));
        assert_eq!(rate.above, None, "tiers for long prompts are not read");
    }

    #[test]
    fn a_model_with_no_cache_price_charges_cached_tokens_as_input() {
        let rate = parse(GATEWAY.as_bytes())[4]
            .parameters
            .rate
            .clone()
            .unwrap();
        assert_eq!(rate.input_per_mtok, "1");
        assert_eq!(rate.output_per_mtok, "2");
        assert_eq!(rate.cached_input_per_mtok, "1");
        assert_eq!(rate.cache_write_per_mtok, None, "absent means as input");
    }

    #[test]
    fn a_free_variant_is_rated_at_nothing_and_a_variable_price_at_no_rate_at_all() {
        let found = parse(GATEWAY.as_bytes());
        let free = found[2].parameters.rate.clone().unwrap();
        assert_eq!(
            (free.input_per_mtok.as_str(), free.output_per_mtok.as_str()),
            ("0", "0")
        );
        assert_eq!(found[3].model_name, "openrouter/auto");
        assert_eq!(found[3].parameters.rate, None, "nobody said what it costs");
        assert_eq!(
            found[3]
                .parameters
                .offered
                .as_ref()
                .and_then(|o| o.context_max),
            Some(2_000_000)
        );
    }

    #[test]
    fn entries_that_name_nothing_are_left_out_and_a_name_is_listed_once() {
        let body = br#"{"data":[{"id":"a"},{"object":"model"},{"id":""},{"id":7},"x",{"id":"a","name":"Again"},{"id":"b"}]}"#;
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
            br#"{"data":"x"}"#,
            br#"{"error":{"message":"no"}}"#,
        ] {
            assert!(parse(body).is_empty(), "{}", String::from_utf8_lossy(body));
        }
    }

    #[test]
    fn a_name_with_surrounding_space_is_shown_without_it_but_sent_as_it_is() {
        let found = parse(br#"{"data":[{"id":" odd/name ","name":"  Odd  "}]}"#);
        assert_eq!(found[0].model_name, " odd/name ");
        assert_eq!(found[0].display_name, "Odd");
    }

    #[test]
    fn the_list_request_is_a_get_of_the_models_path() {
        assert_eq!(request(), (Method::GET, "/models"));
    }

    #[test]
    fn the_list_a_lease_answers_holds_exactly_its_models_in_the_vendors_shape() {
        let response = list_response(&["gpt-6-sol".to_owned(), "openai/x:free".to_owned()]);
        assert_eq!(response.status(), 200);
        let body: Value = serde_json::from_slice(response.body()).unwrap();
        assert_eq!(body["object"], "list");
        let ids: Vec<_> = body["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| (m["id"].as_str().unwrap(), m["object"].as_str().unwrap()))
            .collect();
        assert_eq!(ids, [("gpt-6-sol", "model"), ("openai/x:free", "model")]);
        // What the endpoint listed for itself reads back as the same names.
        let again = parse(response.body());
        assert_eq!(names(&again), ["gpt-6-sol", "openai/x:free"]);
        assert_eq!(
            serde_json::from_slice::<Value>(list_response(&[]).body()).unwrap()["data"],
            json!([])
        );
    }
}
