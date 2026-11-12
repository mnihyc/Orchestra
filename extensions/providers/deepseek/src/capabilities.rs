//! The account's native and Search/Fetch API mappings, registered as common declaration data.
use orch_provider_contract::capabilities::{
    Binding, Counter, Declarations, Endpoint, EndpointRule, Interface, Offering, Price, Support,
};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

const SEARCH: &str = "deepseek-search";
const FETCH: &str = "deepseek-fetch";
const BASE: &str = "https://api.deepseek.com";

fn search_schema(api: bool) -> Json {
    let mut query = json!({"type":"string","minLength":1,"pattern":"\\S"});
    let mut count = json!({"type":"integer","minimum":1,"default":10});
    if api {
        query["maxLength"] = json!(128);
        count["maximum"] = json!(50);
    }
    json!({"type":"object","required":["query"],"properties":{"query":query,"count":count},"additionalProperties":false})
}
fn fetch_schema() -> Json {
    json!({"type":"object","required":["url"],"properties":{"url":{"type":"string","minLength":1,"maxLength":8192,"pattern":"^https?://[^/?#]+","not":{"pattern":"^https?://[^/?#]*@"}}},"additionalProperties":false})
}
fn result_item() -> Json {
    json!({"title":{"from_result":["/title","/url"]},"url":{"from_result":"/url"},
        "snippet":{"from_result":"/snippet","optional":true},"content":{"from_result":"/content","optional":true},"date":{"from_result":"/date","optional":true}})
}
fn quote(unit: &str) -> Price {
    Price {
        amount: crate::native_search::PER_CALL_USD.into(),
        currency: "USD".into(),
        unit: unit.into(),
        ..Price::default()
    }
}
fn api_binding(fetch: bool) -> Binding {
    let schema = if fetch {
        json!({"type":"object","required":["content"],"properties":{"title":{"type":"string"},"content":{"type":"string"}}})
    } else {
        json!({"type":"object","required":["results"],"properties":{"results":{"type":"array"}}})
    };
    let nonempty = if fetch {
        json!({"required":["content"],"properties":{"content":{"type":"string","minLength":1}}})
    } else {
        json!({"required":["results"],"properties":{"results":{"type":"array","minItems":1}}})
    };
    let (unit, counter, path, body) = if fetch {
        (
            "page",
            "web_fetches",
            "/v1/tools/fetch",
            json!({"url":{"from_input":"/url"}}),
        )
    } else {
        (
            "search",
            "web_searches",
            "/v1/tools/search",
            json!({"query":{"from_input":"/query"},"count":{"from_input":"/count","default":10}}),
        )
    };
    let result = if fetch {
        json!({"schema":schema,"page":{"url":{"from_input":"/url"},"title":{"from_result":"/title","optional":true},"content":{"from_result":"/content"},"media_type":"text/markdown"}})
    } else {
        json!({"schema":schema,"unique_by":"/url","collections":[{"pointer":"/results/*","item":result_item()}]})
    };
    Binding {
        input_schema: if fetch {
            fetch_schema()
        } else {
            search_schema(true)
        },
        request: json!({"method":"POST","path":path,"body":body,"success_statuses":[200]}),
        result,
        counters: BTreeMap::from([(
            counter.into(),
            Counter {
                constant: Some(1),
                valid_when: schema,
                only_when: nonempty,
                otherwise: Some(0),
                unit: unit.into(),
                successful_only: true,
                ..Counter::default()
            },
        )]),
        prices: BTreeMap::from([(unit.into(), quote(unit))]),
        discovery: json!({"documentation":crate::native_search::PRICING_DOCUMENTATION,"billing":"successful_nonempty_request"}),
        ..Binding::default()
    }
}
fn native_binding(protocol: &str, base: &Url) -> Option<Binding> {
    let mut request = crate::native_search::declaration(protocol, base, "deepseek-v4-pro")?;
    // Native wire mode fields have no cached meaning on this endpoint. Reject cached intent before stripping it.
    if protocol == "openai-responses" {
        request["strip_options"] = json!(["external_web_access"]);
        request["options_schema"]["x-orch-mode-overrides"] =
            json!([{"identity":{"external_web_access":false},"mode":"cached"}]);
    }
    let valid = if protocol == "openai-responses" {
        json!({"required":["usage"],"properties":{"usage":{"required":["input_tokens","output_tokens"],"properties":{"input_tokens":{"type":"integer","minimum":0},"output_tokens":{"type":"integer","minimum":0}}}}})
    } else {
        json!({"required":["usage"],"properties":{"usage":{"required":["output_tokens"],"properties":{"output_tokens":{"type":"integer","minimum":0}}}}})
    };
    let mut valid = valid;
    valid["type"] = json!("object");
    valid["properties"]["usage"]["type"] = json!("object");
    let only_tools = json!({"type":"object","required":["usage"],"properties":{"usage":{"type":"object","required":["server_tool_use"]}}});
    let mut counters = BTreeMap::from([(
        "web_searches".into(),
        Counter {
            pointer: "/usage/server_tool_use/tool_search".into(),
            valid_when: valid.clone(),
            only_when: only_tools.clone(),
            otherwise: Some(0),
            unit: "search".into(),
            cumulative: true,
            successful_only: true,
            ..Counter::default()
        },
    )]);
    let fetch = Counter {
        pointer: "/usage/server_tool_use/tool_fetch".into(),
        valid_when: valid,
        ..Counter::default()
    };
    counters.insert(
        "web_fetches".into(),
        if protocol == "openai-responses" {
            Counter {
                only_when: only_tools,
                otherwise: Some(0),
                unit: "page".into(),
                cumulative: true,
                successful_only: true,
                ..fetch
            }
        } else {
            Counter {
                unit: "page".into(),
                cumulative: true,
                successful_only: true,
                optional: true,
                ..fetch
            }
        },
    );
    let result = if protocol == "openai-responses" {
        json!({"unique_by":"/url","collections":[
        {"pointer":"/output/*/action/sources/*","optional":true,"item":result_item()},
        {"pointer":"/output/*/content/*/annotations/*","optional":true,"when":{"required":["type"],"properties":{"type":{"const":"url_citation"}}},"item":result_item()}]})
    } else {
        json!({"unique_by":"/url","errors":[{"pointer":"/content/*/content/*","when":{"required":["type"],"properties":{"type":{"const":"web_search_tool_result_error"}}}},{"pointer":"/content/*/content","when":{"required":["type"],"properties":{"type":{"const":"web_search_tool_result_error"}}}}],
            "collections":[{"pointer":"/content/*/content/*","optional":true,"when":{"required":["type"],"properties":{"type":{"const":"web_search_result"}}},"item":result_item()}]})
    };
    Some(Binding {
        input_schema: search_schema(false),
        request,
        result,
        counters,
        eligible_when: json!({"required":["mode"],"properties":{"mode":{"enum":["live","service"]}}}),
        prices: BTreeMap::from([
            ("search".into(), quote("search")),
            ("page".into(), quote("page")),
        ]),
        discovery: json!({"offered_modes":["live"]}),
        extensions: BTreeMap::from([(
            "auxiliary".into(),
            json!({"prompt":{"from_input":"/query"},"options":{"web_search":"live"}}),
        )]),
    })
}
fn endpoint_rule(protocol: &str, paths: Json) -> Json {
    json!({"type":"object","required":["protocol","scheme","host","port","path","has_credentials","has_query","has_fragment"],"properties":{
        "protocol":{"const":protocol},"scheme":{"const":"https"},"host":{"const":"api.deepseek.com"},"port":{"const":null},"path":{"enum":paths},"has_credentials":{"const":false},"has_query":{"const":false},"has_fragment":{"const":false}}})
}
/// The declaration joins the same provider registry at both API and credential-keeping composition roots.
/// Native support is an endpoint/model fact; service discovery never enables a service.
#[must_use]
pub fn declarations() -> Declarations {
    let mut declarations = Declarations::default();
    for (id, title, kind, fetch) in [
        (SEARCH, "DeepSeek Search", "search", false),
        (FETCH, "DeepSeek Fetch", "fetch", true),
    ] {
        declarations.interfaces.insert(
            id.into(),
            Interface {
                title: title.into(),
                kind: kind.into(),
                implementation: "http-json".into(),
                shape: api_binding(fetch),
                ..Interface::default()
            },
        );
    }
    for (protocol, paths, address) in [
        ("openai-responses", json!(["/"]), BASE),
        (
            "anthropic-messages",
            json!(["/anthropic", "/anthropic/"]),
            "https://api.deepseek.com/anthropic",
        ),
    ] {
        let when = endpoint_rule(protocol, paths);
        let mut endpoints = BTreeMap::new();
        let mut offerings = BTreeMap::new();
        for (id, capability, title) in [
            (SEARCH, "web_search", "DeepSeek Search"),
            (FETCH, "page_fetch", "DeepSeek Fetch"),
        ] {
            endpoints.insert(
                id.into(),
                Endpoint {
                    address: BASE.into(),
                    interface: id.into(),
                    credential_role: Some("model".into()),
                    ..Endpoint::default()
                },
            );
            offerings.insert(
                id.into(),
                Offering {
                    title: title.into(),
                    endpoint: id.into(),
                    kind: "service".into(),
                    enabled: false,
                    groups: vec!["web".into()],
                    prices: declarations.interfaces[id].shape.prices.clone(),
                    capabilities: BTreeMap::from([(
                        capability.into(),
                        Support {
                            enabled: true,
                            modes: vec!["service".into()],
                            ..Support::default()
                        },
                    )]),
                    ..Offering::default()
                },
            );
        }
        declarations.endpoint_rules.push(EndpointRule {
            when: when.clone(),
            native_tools: BTreeMap::from([
                ("web_search".into(), None),
                ("page_fetch".into(), None),
                ("image_generation".into(), None),
            ]),
            endpoints,
            offerings,
            ..EndpointRule::default()
        });
        if let Ok(base) = Url::parse(address)
            && let Some(binding) = native_binding(protocol, &base)
        {
            declarations.endpoint_rules.push(EndpointRule {when:json!({"allOf":[when,{"required":["model"],"properties":{"model":{"const":"deepseek-v4-pro"}}}]}),native_tools:BTreeMap::from([("web_search".into(),Some(binding))]),..EndpointRule::default()});
        }
    }
    declarations
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "a test fails by panicking")]
mod tests {
    use super::*;
    use orch_provider_contract::capabilities::{project_endpoint, project_result, render_request};
    #[test]
    fn native_pairs_and_api_services_join_the_common_registry_without_enabling_or_rewriting_models()
    {
        let declarations = declarations();
        let base = Url::parse(BASE).unwrap();
        let pro = project_endpoint(&declarations, "openai-responses", &base, "deepseek-v4-pro");
        assert!(pro.native_tools["web_search"].is_some());
        assert!(pro.native_tools["page_fetch"].is_none());
        assert_eq!(pro.offerings.len(), 2);
        assert!(pro.offerings.values().all(|offering| !offering.enabled));
        let flash = project_endpoint(&declarations, "openai-responses", &base, "deepseek-flash");
        assert!(flash.native_tools["web_search"].is_none());
        assert!(
            project_endpoint(
                &declarations,
                "openai-responses",
                &Url::parse("https://api.deepseek.com.example.invalid").unwrap(),
                "deepseek-v4-pro"
            )
            .native_tools
            .is_empty()
        );
        let binding = &declarations.interfaces[SEARCH].shape;
        let request = render_request(binding, &json!({"query":"docs site:example.com"})).unwrap();
        assert_eq!(
            request["body"],
            json!({"query":"docs site:example.com","count":10})
        );
        assert!(render_request(binding, &json!({"query":" ","count":1})).is_err());
        assert!(render_request(binding, &json!({"query":"q","count":51})).is_err());
        let empty = json!({"results":[]});
        assert_eq!(
            project_result(binding, &json!({"query":"q"}), &empty, 10).unwrap(),
            json!([])
        );
        assert_eq!(binding.counters["web_searches"].count(&empty), Some(0));
        assert_eq!(binding.counters["web_searches"].count(&json!({})), None);
        let actual = json!({"results":[{"title":"Actual page","url":"https://page.example.invalid","snippet":"Only recorded source text","content":"body","date":null}]});
        assert_eq!(binding.counters["web_searches"].count(&actual), Some(1));
        assert_eq!(binding.prices["search"].amount, "0.00075");
        assert_eq!(binding.prices["search"].currency, "USD");
        let page = &declarations.interfaces[FETCH].shape;
        let projected = project_result(
            page,
            &json!({"url":"https://page.example.invalid"}),
            &json!({"title":"Page","content":"# Text"}),
            1,
        )
        .unwrap();
        assert_eq!(projected["media_type"], "text/markdown");
        assert_eq!(
            page.counters["web_fetches"].count(&json!({"title":"","content":""})),
            Some(0)
        );
    }
}
