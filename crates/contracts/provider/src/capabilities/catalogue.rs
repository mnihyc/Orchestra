//! Shared semantics and protocol-declared interfaces, with no provider or program selection.
use super::{Binding, Declarations, Interface};
use orch_types::Json;
use std::collections::BTreeMap;

/// The shipped neutral capabilities and open routes. Extensions register additional data beside these.
pub fn catalogue() -> Result<Declarations, serde_json::Error> {
    let mut declarations: Declarations =
        serde_json::from_str(include_str!("../../../../../shared/capabilities.json"))?;
    declarations.interfaces = protocol_interfaces()?;
    Ok(declarations)
}

/// Every wire shape declares its kind and native bindings in the same protocol option document.
/// A protocol without a native tool never gains one by sharing another protocol's request format.
pub fn protocol_interfaces() -> Result<BTreeMap<String, Interface>, serde_json::Error> {
    let source: Json =
        serde_json::from_str(include_str!("../../../../../shared/protocol-options.json"))?;
    Ok(source
        .get("protocols")
        .and_then(Json::as_object)
        .into_iter()
        .flatten()
        .map(|(id, value)| {
            let mut native_tools: BTreeMap<String, Binding> = value
                .get("native_tools")
                .and_then(|tools| serde_json::from_value(tools.clone()).ok())
                .unwrap_or_default();
            if let Some(search) = value.get("web_search") {
                let binding = native_tools.entry("web_search".into()).or_default();
                if binding.request.is_null() {
                    binding.request = search.clone();
                }
            }
            (
                id.clone(),
                Interface {
                    title: value
                        .get("title")
                        .and_then(Json::as_str)
                        .unwrap_or(id)
                        .into(),
                    kind: value
                        .get("kind")
                        .and_then(Json::as_str)
                        .unwrap_or_default()
                        .into(),
                    implementation: id.clone(),
                    native_tools,
                    ..Interface::default()
                },
            )
        })
        .collect())
}

#[cfg(test)]
#[allow(clippy::expect_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    #[test]
    fn shipped_capabilities_have_one_setting_item_unit_and_declared_routes() {
        let declarations = catalogue().expect("shipped declaration JSON");
        for (id, capability) in &declarations.capabilities {
            assert!(!capability.setting.is_empty(), "{id}");
            assert!(!capability.item.is_empty(), "{id}");
            assert!(!capability.ledger_unit.is_empty(), "{id}");
            for route in capability.modes.values().flat_map(|mode| &mode.routes) {
                assert!(
                    declarations.routes[route].capabilities.contains(id),
                    "{id}: {route}"
                );
            }
        }
        assert!(
            declarations
                .routes
                .values()
                .filter(|route| route.uses_backend)
                .all(|route| route.requires_opt_in)
        );
        assert!(
            declarations
                .capabilities
                .values()
                .all(|capability| capability.default_mode.is_none())
        );
        assert!(
            declarations
                .interfaces
                .values()
                .all(|interface| !interface.kind.is_empty())
        );
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod native_result_tests {
    use super::*;
    use crate::capabilities::{project_result, render_auxiliary};
    use serde_json::json;
    #[test]
    fn declared_search_results_keep_real_sources_and_fetch_selects_the_actual_page_block() {
        let interfaces = protocol_interfaces().unwrap();
        let response_search = &interfaces["openai-responses"].native_tools["web_search"];
        assert_eq!(response_search.request["tool"]["type"], "web_search");
        let query = json!({"query":"{\"from_input\":\"/credential\"}","count":2});
        assert_eq!(
            render_auxiliary(response_search, &query).unwrap()["prompt"],
            query["query"]
        );
        let response = json!({"output":[{"type":"web_search_call","action":{"sources":[{"title":"Actual page","url":"https://page.example.invalid"}]}},{"type":"message","content":[{"type":"output_text","annotations":[{"type":"url_citation","title":"Actual page","url":"https://page.example.invalid"}]}]}]});
        let rows = project_result(response_search, &query, &response, 2).unwrap();
        assert_eq!(rows.as_array().unwrap().len(), 1);
        assert_eq!(rows[0]["url"], "https://page.example.invalid");
        assert!(project_result(response_search, &query, &json!({}), 2).is_err());
        let messages = &interfaces["anthropic-messages"].native_tools;
        let message_search = &messages["web_search"];
        let found = json!({"content":[{"type":"server_tool_use","name":"web_search"},{"type":"web_search_tool_result","content":[{"type":"web_search_result","title":"Actual source","url":"https://page.example.invalid","encrypted_content":"opaque-not-presented"}]}]});
        let rows = project_result(message_search, &query, &found, 2).unwrap();
        assert_eq!(rows[0]["title"], "Actual source");
        assert!(!rows.to_string().contains("opaque-not-presented"));
        let fetch = &messages["page_fetch"];
        let input = json!({"url":"https://page.example.invalid"});
        assert_eq!(
            render_auxiliary(fetch, &input).unwrap()["prompt"],
            input["url"]
        );
        let reply = json!({"content":[{"type":"text","text":"summary is not the fetched source"},{"type":"web_fetch_tool_result","content":{"type":"web_fetch_result","url":"https://page.example.invalid","content":{"type":"document","title":"Fetched title","source":{"type":"text","media_type":"text/plain","data":"Actual fetched content"}}}}]});
        assert_eq!(
            project_result(fetch, &input, &reply, 1).unwrap(),
            json!({"url":"https://page.example.invalid","title":"Fetched title","content":"Actual fetched content","media_type":"text/plain"})
        );
        let refused = json!({"content":[{"type":"web_fetch_tool_result","content":{"type":"web_fetch_tool_result_error","error_code":"domain_not_allowed"}}]});
        assert!(project_result(fetch, &input, &refused, 1).is_err());
        let mut binary = reply.clone();
        binary["content"][1]["content"]["content"]["source"]["type"] = json!("base64");
        assert!(
            project_result(fetch, &input, &binary, 1).is_err(),
            "binary content is never mislabeled as page text"
        );
        let mut repeated = reply.clone();
        repeated["content"]
            .as_array_mut()
            .unwrap()
            .push(reply["content"][1].clone());
        assert!(
            project_result(fetch, &input, &repeated, 1).is_err(),
            "multiple fetched pages are not silently collapsed"
        );
        assert_eq!(fetch.prices["page"].amount, "0");
        assert!(
            !interfaces["openai-responses"]
                .native_tools
                .contains_key("page_fetch")
        );
    }
}
