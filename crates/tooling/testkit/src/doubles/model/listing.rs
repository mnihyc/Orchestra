//! The lists of models the scripted server answers: the shape of each format, and the paging of the Messages format.
//!
//! Both formats are asked at `GET …/models`, and the one that asks is told by its `anthropic-version` header.
//! The Responses format's list is `{"object": "list", "data": [{"id", "object": "model", "created",
//! "owned_by"}]}`. The Messages format's list is `{"data": [{"type": "model", "id", "display_name",
//! "created_at", "max_input_tokens", …}], "has_more", "first_id", "last_id"}`; it pages, with 20 models unless
//! `limit` asks for more and at most 1000, and `after_id` names the model after which the page starts. A model
//! the server was given a name, a window or prices for carries them the way a gateway's list does, so that a
//! reader of gateway lists has something to read.

use chrono::{DateTime, SecondsFormat};
use serde_json::{Value, json};

use super::request::Format;
use super::{MockConfig, MockModel};

/// How many models the Messages format's list gives unless it is asked for more: the vendor's default.
pub const PAGE_DEFAULT: usize = 20;

/// The most models the Messages format's list gives in a page: the vendor's maximum.
pub const PAGE_MAX: usize = 1000;

/// Who the list of the Responses format says owns the models.
const OWNER: &str = "mock";

/// The extra members of an entry that a gateway's list carries.
fn gateway_members(model: &MockModel, entry: &mut Value) {
    if let Some(context) = model.context_length {
        entry["context_length"] = Value::from(context);
    }
    if let Some(pricing) = &model.pricing {
        let mut prices = json!({ "prompt": pricing.prompt, "completion": pricing.completion });
        if let Some(read) = &pricing.input_cache_read {
            prices["input_cache_read"] = Value::from(read.as_str());
        }
        if let Some(write) = &pricing.input_cache_write {
            prices["input_cache_write"] = Value::from(write.as_str());
        }
        entry["pricing"] = prices;
    }
}

fn responses_list(config: &MockConfig) -> Value {
    let data: Vec<Value> = config
        .models
        .iter()
        .map(|model| {
            let mut entry = json!({
                "id": model.id, "object": "model", "created": config.created_at, "owned_by": OWNER
            });
            if let Some(name) = &model.display_name {
                entry["name"] = Value::from(name.as_str());
            }
            gateway_members(model, &mut entry);
            entry
        })
        .collect();
    json!({ "object": "list", "data": data })
}

fn messages_list(config: &MockConfig, query: Option<&str>) -> Value {
    let mut limit = PAGE_DEFAULT;
    let mut after: Option<String> = None;
    for (name, value) in form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match name.as_ref() {
            "limit" => {
                if let Ok(asked) = value.parse::<usize>() {
                    limit = asked.clamp(1, PAGE_MAX);
                }
            }
            "after_id" => after = Some(value.into_owned()),
            _ => {}
        }
    }
    let created_at = DateTime::from_timestamp(config.created_at, 0)
        .map(|moment| moment.to_rfc3339_opts(SecondsFormat::Secs, true))
        .unwrap_or_default();
    let start = after
        .and_then(|id| config.models.iter().position(|model| model.id == id))
        .map_or(0, |at| at + 1);
    let page: Vec<&MockModel> = config.models.iter().skip(start).take(limit).collect();
    let data: Vec<Value> = page
        .iter()
        .map(|model| {
            let mut entry = json!({
                "type": "model", "id": model.id,
                "display_name": model.display_name.as_deref().unwrap_or(&model.id),
                "created_at": created_at, "lifecycle": "active"
            });
            if let Some(context) = model.context_length {
                entry["max_input_tokens"] = Value::from(context);
            }
            gateway_members(model, &mut entry);
            entry
        })
        .collect();
    json!({
        "data": data,
        "has_more": start + page.len() < config.models.len(),
        "first_id": page.first().map(|model| &model.id),
        "last_id": page.last().map(|model| &model.id)
    })
}

/// The list of models in the format asked for.
#[must_use]
pub fn list(config: &MockConfig, format: Format, query: Option<&str>) -> Value {
    match format {
        Format::Responses | Format::Chat => responses_list(config),
        Format::Messages => messages_list(config, query),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::doubles::model::MockPricing;

    fn config(count: usize) -> MockConfig {
        MockConfig {
            models: (0..count)
                .map(|n| MockModel::named(&format!("mock-{n:02}")))
                .collect(),
            ..MockConfig::default()
        }
    }

    fn ids(list: &Value) -> Vec<&str> {
        list["data"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| m["id"].as_str().unwrap())
            .collect()
    }

    #[test]
    fn the_responses_list_has_each_model_with_the_members_the_vendor_lists() {
        let list = list(&config(2), Format::Responses, None);
        assert_eq!(list["object"], "list");
        assert_eq!(ids(&list), ["mock-00", "mock-01"]);
        assert_eq!(list["data"][0]["object"], "model");
        assert_eq!(list["data"][0]["owned_by"], "mock");
        assert!(
            list["data"][0].get("name").is_none(),
            "the vendor's own list has no shown name"
        );
    }

    #[test]
    fn a_model_with_a_name_a_window_and_prices_carries_them_as_a_gateway_would() {
        let mut config = config(1);
        config.models[0] = MockModel {
            id: "vendor/m:free".into(),
            display_name: Some("Vendor: M (free)".into()),
            context_length: Some(262_144),
            pricing: Some(MockPricing {
                prompt: "0".into(),
                completion: "0".into(),
                input_cache_read: Some("0.1".into()),
                input_cache_write: None,
            }),
        };
        let entry = &list(&config, Format::Responses, None)["data"][0];
        assert_eq!(entry["name"], "Vendor: M (free)");
        assert_eq!(entry["context_length"], 262_144);
        assert_eq!(
            entry["pricing"],
            json!({ "prompt": "0", "completion": "0", "input_cache_read": "0.1" })
        );
        let entry = &list(&config, Format::Messages, None)["data"][0];
        assert_eq!(entry["display_name"], "Vendor: M (free)");
        assert_eq!(entry["max_input_tokens"], 262_144);
        assert!(entry["pricing"].is_object());
    }

    #[test]
    fn the_messages_list_has_the_shape_of_the_vendor_and_the_routing_name_where_there_is_no_other()
    {
        let list = list(&config(2), Format::Messages, None);
        assert_eq!(list["data"][0]["type"], "model");
        assert_eq!(list["data"][0]["display_name"], "mock-00");
        assert_eq!(list["data"][0]["created_at"], "2026-10-11T00:00:00Z");
        assert_eq!(list["has_more"], false);
        assert_eq!(list["first_id"], "mock-00");
        assert_eq!(list["last_id"], "mock-01");
    }

    #[test]
    fn the_messages_list_gives_twenty_models_unless_asked_for_more_and_pages_after_a_name() {
        let config = config(45);
        let first = list(&config, Format::Messages, None);
        assert_eq!(ids(&first).len(), PAGE_DEFAULT);
        assert_eq!(first["has_more"], true);
        assert_eq!(first["last_id"], "mock-19");
        let second = list(&config, Format::Messages, Some("limit=20&after_id=mock-19"));
        assert_eq!(ids(&second)[0], "mock-20");
        assert_eq!(second["has_more"], true);
        let third = list(&config, Format::Messages, Some("after_id=mock-39"));
        assert_eq!(ids(&third).len(), 5);
        assert_eq!(third["has_more"], false);
        let all = list(&config, Format::Messages, Some("limit=1000"));
        assert_eq!(ids(&all).len(), 45);
        assert_eq!(all["has_more"], false);
        let one = list(&config, Format::Messages, Some("limit=0"));
        assert_eq!(ids(&one).len(), 1, "a page holds at least one");
    }

    #[test]
    fn a_name_in_the_query_is_read_with_its_percent_encoding() {
        let mut config = config(0);
        config.models = ["a/b:free", "c"]
            .iter()
            .map(|id| MockModel::named(id))
            .collect();
        let page = list(&config, Format::Messages, Some("after_id=a%2Fb%3Afree"));
        assert_eq!(ids(&page), ["c"]);
    }

    #[test]
    fn an_empty_list_is_an_empty_page() {
        let list = list(&config(0), Format::Messages, None);
        assert_eq!(list["data"], json!([]));
        assert_eq!(list["first_id"], Value::Null);
        assert_eq!(list["has_more"], false);
    }
}
