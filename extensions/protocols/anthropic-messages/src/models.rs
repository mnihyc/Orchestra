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
use orch_agent_contract::parameters::{ModelReport, Offered, Rate};
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
        web_fetch_per_call: pricing
            .get("web_fetch")
            .and_then(|price| decimal::plain(&price.to_string())),
        web_search_per_call: pricing
            .get("web_search")
            .and_then(|price| decimal::plain(&price.to_string())),
        cached_input_per_mtok: price(pricing, "input_cache_read").unwrap_or_else(|| input.clone()),
        cache_write_per_mtok: price(pricing, "input_cache_write"),
        input_per_mtok: input,
        output_per_mtok: output,
        tiers: None,
        cache_write_1h_per_mtok: price(pricing, "input_cache_write_1h"),
        cache_write_12h_per_mtok: price(pricing, "input_cache_write_12h"),
        capacity_tiers: None,
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
    suggestion.parameters.input_images = entry
        .pointer("/capabilities/image_input/supported")
        .and_then(Value::as_bool);
    if let Some(effort) = entry
        .pointer("/capabilities/effort")
        .and_then(Value::as_object)
    {
        let levels = if effort.get("supported").and_then(Value::as_bool) == Some(false) {
            Vec::new()
        } else {
            effort
                .iter()
                .filter(|&(_level, capability)| {
                    capability.get("supported").and_then(Value::as_bool) == Some(true)
                })
                .map(|(level, _capability)| level.clone())
                .collect()
        };
        suggestion.parameters.efforts = Some(levels);
    }
    // An endpoint states one number, its largest. It is on offer and nothing more: a manager opens it.
    suggestion.parameters.offered = size(entry.get("max_input_tokens"))
        .or_else(|| size(entry.get("context_length")))
        .map(|largest| Offered {
            web_search: None,
            context_max: Some(largest),
            ..Offered::default()
        });
    if let Some(supported) = entry
        .pointer("/capabilities/server_tools/web_search/supported")
        .and_then(Value::as_bool)
    {
        suggestion
            .parameters
            .offered
            .get_or_insert_with(Offered::default)
            .web_search = Some(if supported {
            vec!["live".to_owned()]
        } else {
            Vec::new()
        });
        suggestion
            .origins
            .insert("/parameters/offered/web_search".into(), "vendor".into());
        if !supported {
            suggestion
                .parameters
                .native_tools
                .entry("anthropic-messages".into())
                .or_default()
                .insert("web_search".into(), None);
            suggestion.origins.insert(
                "/parameters/native_tools/anthropic-messages/web_search".into(),
                "vendor".into(),
            );
        }
    }
    let text = |member| {
        entry
            .get(member)
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .map(str::to_owned)
    };
    let report = ModelReport {
        capabilities: entry
            .get("capabilities")
            .and_then(Value::as_object)
            .map(|members| {
                members
                    .iter()
                    .map(|(key, value)| (key.clone(), value.clone()))
                    .collect()
            })
            .unwrap_or_default(),
        output_max: size(entry.get("max_tokens")),
        lifecycle: text("lifecycle"),
        deprecated_at: text("deprecated_at"),
        retires_at: text("retires_at"),
        line: text("line"),
        ..ModelReport::default()
    };
    if report != ModelReport::default() {
        suggestion
            .parameters
            .offered
            .get_or_insert_with(Offered::default)
            .report = Some(report);
    }
    suggestion.parameters.rate = entry.get("pricing").and_then(rate);
    suggestion
        .origins
        .insert("/model_name".into(), "vendor".into());
    if display_name.is_some() {
        suggestion
            .origins
            .insert("/display_name".into(), "vendor".into());
    }
    if suggestion.parameters.input_images.is_some() {
        suggestion
            .origins
            .insert("/parameters/input_images".into(), "vendor".into());
    }
    if suggestion.parameters.efforts.is_some() {
        suggestion
            .origins
            .insert("/parameters/efforts".into(), "vendor".into());
    }
    if let Some(offered) = suggestion.parameters.offered.as_ref() {
        if offered.context_max.is_some() {
            suggestion
                .origins
                .insert("/parameters/offered/context_max".into(), "vendor".into());
        }
        if let Some(report) = offered.report.as_ref() {
            if !report.capabilities.is_empty() {
                suggestion.origins.insert(
                    "/parameters/offered/report/capabilities".into(),
                    "vendor".into(),
                );
            }
            for (field, present) in [
                ("output_max", report.output_max.is_some()),
                ("lifecycle", report.lifecycle.is_some()),
                ("deprecated_at", report.deprecated_at.is_some()),
                ("retires_at", report.retires_at.is_some()),
                ("line", report.line.is_some()),
            ] {
                if present {
                    suggestion.origins.insert(
                        format!("/parameters/offered/report/{field}"),
                        "vendor".into(),
                    );
                }
            }
        }
    }
    if suggestion.parameters.rate.is_some()
        && let Some(pricing) = entry.get("pricing")
    {
        for (member, field) in [
            ("prompt", "input_per_mtok"),
            ("completion", "output_per_mtok"),
            ("input_cache_read", "cached_input_per_mtok"),
            ("input_cache_write", "cache_write_per_mtok"),
            ("input_cache_write_1h", "cache_write_1h_per_mtok"),
            ("input_cache_write_12h", "cache_write_12h_per_mtok"),
        ] {
            if price(pricing, member).is_some() {
                suggestion
                    .origins
                    .insert(format!("/parameters/rate/{field}"), "vendor".into());
            }
        }
    }
    if let Some(rate) = suggestion.parameters.rate.as_ref() {
        for (field, value) in [
            ("web_search_per_call", rate.web_search_per_call.as_ref()),
            ("web_fetch_per_call", rate.web_fetch_per_call.as_ref()),
        ] {
            if value.is_some() {
                suggestion
                    .origins
                    .insert(format!("/parameters/rate/{field}"), "vendor".into());
            }
        }
    }
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

    #[test]
    fn explicit_negative_vendor_support_stays_known_and_does_not_become_a_catalogue_guess() {
        let found = parse(
            br#"{"data":[{"id":"fixture-model","display_name":"Reported model",
            "max_input_tokens":200000,"line":"fixture-line","capabilities":{
                "image_input":{"supported":false},"server_tools":{"web_search":{"supported":false}}
            }}]}"#,
        );
        let model = &found[0];
        assert_eq!(model.parameters.input_images, Some(false));
        assert_eq!(
            model.parameters.offered.as_ref().unwrap().web_search,
            Some(Vec::new())
        );
        assert_eq!(
            model.parameters.native_tools["anthropic-messages"]["web_search"],
            None
        );
        for pointer in [
            "/parameters/input_images",
            "/parameters/offered/web_search",
            "/parameters/offered/report/line",
            "/parameters/native_tools/anthropic-messages/web_search",
        ] {
            assert_eq!(
                model.origins.get(pointer).map(String::as_str),
                Some("vendor")
            );
        }
        assert!(model.parameters.web_search.is_none());
    }

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
    fn the_report_keeps_capabilities_windows_and_retirement_without_enabling_them() {
        let raw = json!({"data":[{
            "id":"opaque-model","max_input_tokens":1000000,"max_tokens":128000,
            "lifecycle":"deprecated","deprecated_at":"2026-10-01T00:00:00Z",
            "retires_at":"2026-10-20T00:00:00Z","line":"future-line",
            "capabilities":{
                "pdf_input":{"supported":true},
                "thinking":{"supported":true,"types":{"adaptive":{"supported":true},"enabled":{"supported":false},"future_type":{"supported":true}}},
                "structured_outputs":{"supported":true},"context_management":{"supported":true,"future_method":{"supported":true}},
                "citations":{"supported":false},"batch":{"supported":true},
                "server_tools":{"web_search":{"supported":true},"code_execution":{"supported":false}},
                "future_capability":{"supported":true,"future_field":[1,2]}
            }
        },{"id":"old-model"},{"id":"future-model","lifecycle":"future-stage"}]});
        let found = parse(&serde_json::to_vec(&raw).unwrap());
        let offered = found[0].parameters.offered.as_ref().unwrap();
        let report = offered.report.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&report.capabilities).unwrap(),
            raw["data"][0]["capabilities"]
        );
        assert_eq!(offered.context_max, Some(1_000_000));
        assert_eq!(report.output_max, Some(128_000));
        assert_eq!(report.lifecycle.as_deref(), Some("deprecated"));
        assert_eq!(
            report.deprecated_at.as_deref(),
            Some("2026-10-01T00:00:00Z")
        );
        assert_eq!(report.retires_at.as_deref(), Some("2026-10-20T00:00:00Z"));
        assert_eq!(report.line.as_deref(), Some("future-line"));
        assert!(found[0].parameters.web_search.is_none());
        assert!(found[0].parameters.context_max.is_none());
        assert!(found[0].parameters.default_context.is_none());
        assert!(found[0].parameters.rate.is_none());
        assert!(
            found[1].parameters.offered.is_none(),
            "an old endpoint remains callable without the new report"
        );
        assert_eq!(
            found[2]
                .parameters
                .offered
                .as_ref()
                .unwrap()
                .report
                .as_ref()
                .unwrap()
                .lifecycle
                .as_deref(),
            Some("future-stage")
        );
    }

    #[test]
    fn capabilities_come_from_the_report_and_never_from_a_model_name() {
        let models = parse(br#"{"data":[
            {"id":"opaque-model","capabilities":{
                "image_input":{"supported":true},"pdf_input":{"supported":true},
                "server_tools":{"web_search":{"supported":true}},
                "effort":{"supported":true,"low":{"supported":true},"max":{"supported":true},"future_level":{"supported":true},"high":{"supported":false}},
                "future_capability":{"supported":true}}},
            {"id":"claude-looking-text-model","capabilities":{
                "image_input":{"supported":false},"effort":{"supported":false},"server_tools":{"web_search":{"supported":false}}}},
            {"id":"old-peer"},
            {"id":"malformed","capabilities":{"image_input":{"supported":"true"},"effort":{"low":{"supported":"true"}}}}
        ]}"#);
        assert_eq!(models[0].parameters.input_images, Some(true));
        let efforts = models[0].parameters.efforts.as_ref().unwrap();
        assert_eq!(efforts.len(), 3);
        assert_eq!(
            efforts
                .iter()
                .map(String::as_str)
                .collect::<std::collections::BTreeSet<_>>(),
            std::collections::BTreeSet::from(["future_level", "low", "max"])
        );
        assert_eq!(
            models[0]
                .parameters
                .offered
                .as_ref()
                .unwrap()
                .web_search
                .as_deref(),
            Some(["live".to_owned()].as_slice())
        );
        assert_eq!(
            models[0].parameters.web_search, None,
            "reported support never enables paid search"
        );
        assert_eq!(models[0].parameters.default_effort, None);
        assert_eq!(models[1].parameters.input_images, Some(false));
        assert_eq!(models[1].parameters.efforts, Some(Vec::new()));
        assert_eq!(models[2].parameters.input_images, None);
        assert_eq!(models[2].parameters.efforts, None);
        assert_eq!(models[3].parameters.input_images, None);
        assert_eq!(models[3].parameters.efforts, Some(Vec::new()));
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
        assert_eq!(rate.cache_write_1h_per_mtok.as_deref(), Some("0.2"));
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
