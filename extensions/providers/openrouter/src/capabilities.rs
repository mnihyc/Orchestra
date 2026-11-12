//! Endpoint-owned search grammar, results, actual counters and original-unit quotes in the shared registry.
use crate::native_search::{self, Engine, Form};
use orch_provider_contract::capabilities::{Binding, Counter, Declarations, EndpointRule};
use orch_types::Json;
use serde_json::json;
use std::collections::BTreeMap;
use url::Url;

fn options(form: Form) -> Json {
    let mut schema = native_search::options_schema(form, Engine::Auto);
    schema["allOf"] =
        json!(
        [Engine::Exa, Engine::Parallel, Engine::Perplexity].into_iter().map(|engine| {
            let specific = native_search::options_schema(form, engine);
            json!({"if":{"required":["engine"],"properties":{"engine":{"const":engine.word()}}},
                "then":{"properties":specific["properties"]}})
        }).collect::<Vec<_>>()
    );
    schema
}
fn quotes() -> Json {
    let mut rows = Vec::new();
    for (engine, modes) in [
        (
            Engine::Exa,
            vec![
                "instant",
                "fast",
                "auto",
                "deep-lite",
                "deep",
                "deep-reasoning",
            ],
        ),
        (Engine::Parallel, vec!["turbo", "fast", "basic", "advanced"]),
        (Engine::Firecrawl, vec![""]),
        (Engine::Perplexity, vec![""]),
    ] {
        for mode in modes {
            if let Some(quote) = native_search::price(engine, (!mode.is_empty()).then_some(mode)) {
                rows.push(json!({"engine":engine.word(),"engine_mode":(!mode.is_empty()).then_some(mode),"quote":quote}));
            }
        }
    }
    json!({"documentation":native_search::PRICING_DOCUMENTATION,"choices":rows,
        "automatic":"provider_dependent","native":"provider_dependent","quantity":"actual_reported_requests_and_results_only"})
}
fn price_rules() -> Json {
    let quoted = quotes();
    let rules = quoted["choices"].as_array().into_iter().flatten().map(|row| {
        let mode = &row["engine_mode"];
        let default = match row["engine"].as_str() {
            Some("exa") => Some("auto"),
            Some("parallel") => Some("basic"),
            _ => None,
        };
        let mode_when = if mode.is_null() {
            json!({"not":{"required":["engine_mode"]}})
        } else if mode.as_str() == default {
            json!({"anyOf":[{"not":{"required":["engine_mode"]}},{"required":["engine_mode"],"properties":{"engine_mode":{"const":mode}}}]})
        } else {
            json!({"required":["engine_mode"],"properties":{"engine_mode":{"const":mode}}})
        };
        let mut terms = vec![json!({"amount":row["quote"]["per_call"],"counter":"web_searches"})];
        if row["quote"]["per_additional_result"].as_str() != Some("0") {
            // No documented per-request result counter is supplied here. Missing actual result
            // quantities keep a complete fee unknown; citations and requested maxima are not units.
            terms.push(json!({"amount":row["quote"]["per_additional_result"],"counter":"web_search_results",
                "included":{"counter":"web_searches","each":row["quote"]["included_results_per_call"]}}));
        }
        json!({"when":{"required":["engine"],"properties":{"engine":{"const":row["engine"]}},"allOf":[mode_when]},
            "quote":{"currency":"USD","terms":terms}})
    }).collect::<Vec<_>>();
    json!(rules)
}
fn binding(protocol: &str, base: &Url) -> Option<Binding> {
    // The plugin works without inventing tool-calling support. Explicit server tools keep their own form.
    let mut request =
        native_search::declaration(protocol, base, "declared-generation-model", Form::Plugin)?;
    request["options_schema"] = options(Form::Plugin);
    let mut server = native_search::declaration(
        protocol,
        base,
        "declared-generation-model",
        Form::ServerTool,
    )?;
    server["options_schema"] = options(Form::ServerTool);
    let mut forms = vec![server];
    if let Some(mut compatible) = orch_provider_contract::search::declaration(protocol) {
        // A compatible protocol's hosted tool is not an additional vendor mapping. Recognize it
        // for the off ceiling, and refuse live execution unless the vendor declares that form.
        compatible["modes"] = json!({});
        forms.push(compatible);
    }
    request["forms"] = json!(forms);
    let item = json!({"title":{"from_result":["/url_citation/title","/title","/url_citation/url","/url"]},
        "url":{"from_result":["/url_citation/url","/url"]},
        "content":{"from_result":["/url_citation/content","/content"],"optional":true},
        "snippet":{"from_result":["/url_citation/snippet","/snippet"],"optional":true},
        "date":{"from_result":["/url_citation/date","/date"],"optional":true}});
    let when = json!({"type":"object","required":["type"],"properties":{"type":{"const":"url_citation"}},
        "anyOf":[{"required":["url"],"properties":{"url":{"type":"string"}}},
            {"required":["url_citation"],"properties":{"url_citation":{"type":"object","required":["url"],"properties":{"url":{"type":"string"}}}}}]});
    let pointers = [
        "/annotations/*",
        "/message/annotations/*",
        "/choices/*/message/annotations/*",
        "/choices/*/delta/annotations/*",
        "/output/*/content/*/annotations/*",
        "/content/*/annotations/*",
    ];
    Some(Binding {
        input_schema: json!({"type":"object","required":["query"],"properties":{"query":{"type":"string","minLength":1,"pattern":"\\S"},
            "count":{"type":"integer","minimum":1}},"additionalProperties":false}),
        request,
        result: json!({"unique_by":"/url","collections":pointers.into_iter().map(|pointer|
            json!({"pointer":pointer,"optional":true,"when":when,"item":item})).collect::<Vec<_>>()}),
        counters: BTreeMap::from([
            (
                "web_searches".into(),
                Counter {
                    pointer: "/usage/server_tool_use_details/web_search_requests".into(),
                    unit: "search".into(),
                    cumulative: true,
                    ..Counter::default()
                },
            ),
            (
                "reported_total".into(),
                Counter {
                    pointer: "/usage/cost".into(),
                    unit: "USD".into(),
                    reported_cost: true,
                    optional: true,
                    ..Counter::default()
                },
            ),
        ]),
        eligible_when: json!({"required":["mode"],"properties":{"mode":{"enum":["live","service"]}}}),
        // A conditional quote is not one universally applicable per-search price.
        prices: BTreeMap::new(),
        discovery: json!({"offered_modes":["live"],"supports_fetch":false,"native_prices":quotes()}),
        extensions: BTreeMap::from([
            ("price_rules".into(), price_rules()),
            (
                "auxiliary".into(),
                json!({"prompt":{"from_input":"/query"},"options":{"web_search":"live"}}),
            ),
        ]),
    })
}

/// Registered by the same built-in source at the API and credential-keeping composition roots.
#[must_use]
pub fn declarations() -> Declarations {
    let mut result = Declarations::default();
    for (protocol, paths, address) in [
        (
            "openai-chat",
            json!(["/api/v1", "/api/v1/"]),
            "https://openrouter.ai/api/v1",
        ),
        (
            "openai-responses",
            json!(["/api/v1", "/api/v1/"]),
            "https://openrouter.ai/api/v1",
        ),
        (
            "anthropic-messages",
            json!(["/api", "/api/"]),
            "https://openrouter.ai/api",
        ),
    ] {
        if let Ok(base) = Url::parse(address)
            && let Some(binding) = binding(protocol, &base)
        {
            result.endpoint_rules.push(EndpointRule {
                when: json!({"type":"object","required":["protocol","scheme","host","port","path","has_credentials","has_query","has_fragment","model"],
                    "properties":{"protocol":{"const":protocol},"scheme":{"const":"https"},"host":{"const":"openrouter.ai"},"port":{"const":null},"path":{"enum":paths},
                        "has_credentials":{"const":false},"has_query":{"const":false},"has_fragment":{"const":false},"model":{"type":"string","minLength":1}}}),
                native_tools: BTreeMap::from([("web_search".into(),Some(binding)),("page_fetch".into(),None)]),
                ..EndpointRule::default()
            });
        }
    }
    result
}

#[cfg(test)]
mod tests;
