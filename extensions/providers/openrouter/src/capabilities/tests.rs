#![allow(clippy::unwrap_used, reason = "fixture assertions")]
use super::*;
use orch_model_contract::web_search::WebSearchDeclaration;
use orch_provider_contract::capabilities::{project_endpoint, project_result};

fn projected(protocol: &str) -> Binding {
    let address = if protocol == "anthropic-messages" {
        "https://openrouter.ai/api"
    } else {
        "https://openrouter.ai/api/v1"
    };
    project_endpoint(
        &declarations(),
        protocol,
        &Url::parse(address).unwrap(),
        "vendor/model",
    )
    .native_tools["web_search"]
        .clone()
        .unwrap()
}

#[test]
fn actual_engine_and_mode_select_public_terms_without_guessing_an_automatic_engine() {
    for protocol in ["openai-chat", "openai-responses", "anthropic-messages"] {
        let binding = projected(protocol);
        let bindings = BTreeMap::from([("web_search".into(), Some(binding.clone()))]);
        for (form, engine, mode, amount) in [
            (Form::Plugin, "exa", Some("deep"), "0.012"),
            (Form::ServerTool, "parallel", Some("fast"), "0.001"),
            (Form::Plugin, "exa", None, "0.007"),
            (Form::ServerTool, "perplexity", None, "0.005"),
            (Form::Plugin, "firecrawl", None, "0.005"),
        ] {
            let mut options = json!({"engine":engine});
            if let Some(mode) = mode {
                options["engine_mode"] = json!(mode);
            }
            let body = native_search::request_patch(form, &options).unwrap();
            let quotes = orch_model_contract::declared_usage::native_prices(&bindings, &body);
            let quote = quotes["web_search"].as_ref().unwrap();
            assert_eq!(quote.currency, "USD");
            assert_eq!(quote.terms[0].amount, amount);
            assert_eq!(quote.terms[0].counter, "web_searches");
            if engine == "perplexity" {
                assert_eq!(quote.terms.len(), 1);
            } else {
                assert_eq!(quote.terms[1].counter, "web_search_results");
            }
            assert!(
                !serde_json::to_string(&quotes)
                    .unwrap()
                    .contains("max_results")
            );
        }
        for options in [
            json!({}),
            json!({"engine":"auto"}),
            json!({"engine":"native"}),
        ] {
            let body = native_search::request_patch(Form::Plugin, &options).unwrap();
            assert_eq!(
                orch_model_contract::declared_usage::native_prices(&bindings, &body)["web_search"],
                None
            );
        }
        assert!(
            orch_model_contract::declared_usage::native_prices(
                &bindings,
                &json!({"tools":[{"type":"function","function":{"name":"web_search"}}]})
            )
            .is_empty()
        );
        let mut body =
            native_search::request_patch(Form::Plugin, &json!({"engine":"exa"})).unwrap();
        let declaration: WebSearchDeclaration = serde_json::from_value(binding.request).unwrap();
        declaration
            .apply(&declaration.paths[0], &mut body, &json!("disabled"))
            .unwrap();
        assert!(orch_model_contract::declared_usage::native_prices(&bindings, &body).is_empty());
    }
}
#[test]
fn source_registration_supplies_only_the_exact_native_endpoint_and_never_enables_a_service() {
    let sources =
        orch_provider_contract::builtins::BuiltinSources::new(vec![crate::builtin()]).unwrap();
    for protocol in ["openai-chat", "openai-responses", "anthropic-messages"] {
        let address = if protocol == "anthropic-messages" {
            "https://openrouter.ai/api"
        } else {
            "https://openrouter.ai/api/v1"
        };
        let projection =
            sources.endpoint_capabilities(protocol, &Url::parse(address).unwrap(), "vendor/model");
        let search = projection.native_tools["web_search"].as_ref().unwrap();
        assert_eq!(search.discovery["offered_modes"], json!(["live"]));
        assert!(projection.native_tools["page_fetch"].is_none());
        assert!(projection.offerings.is_empty() && projection.endpoints.is_empty());
        assert!(
            search.prices.is_empty(),
            "automatic/native is no fixed engine quote"
        );
        assert_eq!(
            search.discovery["native_prices"]["choices"]
                .as_array()
                .unwrap()
                .len(),
            12
        );
    }
    for address in [
        "https://openrouter.ai.example.invalid/api/v1",
        "http://openrouter.ai/api/v1",
        "https://openrouter.ai:444/api/v1",
        "https://user@openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1?key=not-a-credential",
        "https://openrouter.ai/api/v1#other",
        "https://openrouter.ai/",
    ] {
        assert!(
            sources
                .endpoint_capabilities("openai-chat", &Url::parse(address).unwrap(), "vendor/model")
                .native_tools
                .is_empty(),
            "{address}"
        );
    }
    assert!(
        sources
            .endpoint_capabilities(
                "openai-embeddings",
                &Url::parse("https://openrouter.ai/api/v1").unwrap(),
                "vendor/model"
            )
            .native_tools
            .is_empty()
    );
}

#[test]
fn the_shared_builder_preserves_plugins_functions_and_every_explicit_native_option() {
    for protocol in ["openai-chat", "openai-responses", "anthropic-messages"] {
        let binding = projected(protocol);
        let declaration: WebSearchDeclaration = serde_json::from_value(binding.request).unwrap();
        let path = declaration.paths[0].clone();
        let mut body = json!({"model":"vendor/model","tools":[{"type":"function","function":{"name":"keep_function"}}],"plugins":[{"id":"other","keep":true}]});
        declaration.apply(&path,&mut body,&json!({"mode":"live","engine":"exa","engine_mode":"deep","allowed_domains":["example.org"],"excluded_domains":["excluded.example"],"search_context_size":"high","search_prompt":"Actual supplied prompt"})).unwrap();
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert_eq!(body["plugins"][0], json!({"id":"other","keep":true}));
        assert_eq!(body["plugins"][1]["mode"], "deep");
        assert_eq!(
            body["plugins"][1]["include_domains"],
            json!(["example.org"])
        );
        assert_eq!(
            body["plugins"][1]["exclude_domains"],
            json!(["excluded.example"])
        );
        assert_eq!(body["web_search_options"]["search_context_size"], "high");
        assert_eq!(declaration.selection(&body).unwrap()["engine_mode"], "deep");
        declaration
            .apply(&path, &mut body, &json!("disabled"))
            .unwrap();
        assert_eq!(body["plugins"], json!([{"id":"other","keep":true}]));
        assert_eq!(body["tools"].as_array().unwrap().len(), 1);
        assert!(body.get("web_search_options").is_none());
        let mut server = json!({"model":"vendor/model","tools":[{"type":"openrouter:web_search"}]});
        declaration
            .apply(
                &path,
                &mut server,
                &json!({"mode":"live","engine":"parallel","engine_mode":"fast","max_uses":2}),
            )
            .unwrap();
        assert_eq!(server["tools"][0]["parameters"]["mode"], "fast");
        assert_eq!(server["tools"][0]["parameters"]["max_uses"], 2);
        assert!(server.get("plugins").is_none());
        let mut online = json!({"model":"vendor/model:free:online"});
        declaration
            .apply(&path, &mut online, &json!("live"))
            .unwrap();
        assert!(
            online.get("plugins").is_none(),
            "an implicit variant needs no duplicate plugin"
        );
        let before = online.clone();
        assert!(
            declaration
                .apply(&path, &mut online, &json!("disabled"))
                .is_err()
        );
        assert_eq!(
            online, before,
            "an executable model variant cannot be silently renamed"
        );
        let mut both = json!({"plugins":[{"id":"web"}],"tools":[{"type":"openrouter:web_search"}]});
        assert!(declaration.apply(&path, &mut both, &json!("live")).is_err());
        assert!(
            declaration
                .apply(
                    &path,
                    &mut json!({}),
                    &json!({"mode":"live","engine":"perplexity","max_results":21})
                )
                .is_err()
        );
        assert!(
            declaration
                .apply(
                    &path,
                    &mut json!({}),
                    &json!({"mode":"live","engine":"parallel","engine_mode":"deep"})
                )
                .is_err()
        );
        if let Some(compatible) = orch_provider_contract::search::declaration(protocol) {
            let mut compatible_body = json!({"model":"vendor/model","tools":[compatible["tool"].clone(),{"type":"function","function":{"name":"keep_function"}}]});
            assert!(
                declaration
                    .apply(&path, &mut compatible_body, &json!("live"))
                    .is_err(),
                "an unmapped compatible native tool cannot bypass the vendor declaration"
            );
            declaration
                .apply(&path, &mut compatible_body, &json!("disabled"))
                .unwrap();
            assert_eq!(
                compatible_body["tools"],
                json!([{"type":"function","function":{"name":"keep_function"}}])
            );
        }
    }
}

#[test]
fn actual_native_counts_and_original_cost_use_the_shared_observer_without_invented_plugin_counts() {
    let binding = projected("openai-chat");
    let mut observer = orch_model_contract::declared_usage::DeclaredUsage::default();
    observer.configure(&BTreeMap::from([(
        "web_search".into(),
        Some(binding.clone()),
    )]));
    let mut used = orch_model_contract::usage::ObservedUsage {
        web_searches: Some(99),
        ..Default::default()
    };
    observer.observe_usage(&json!({"cost":0.008}));
    observer.apply(&mut used);
    assert_eq!(
        used.web_searches, None,
        "one configured plugin or a citation is not a reported search count"
    );
    for _ in 0..2 {
        observer.observe_usage(&json!({"server_tool_use_details":{"web_search_requests":2}}));
    }
    observer.apply(&mut used);
    assert_eq!(used.web_searches, Some(2));
    observer.observe_usage(&json!({"output_tokens":7}));
    observer.apply(&mut used);
    assert_eq!(used.web_searches, Some(2));
    observer.observe_usage(&json!({"server_tool_use_details":{"web_search_requests":-1}}));
    observer.apply(&mut used);
    assert_eq!(used.web_searches, None);
    let (value, unit) = observer
        .reported_cost(r#"{"cost":0.01234567890123456789}"#)
        .unwrap();
    assert_eq!(value.get(), "0.01234567890123456789");
    assert_eq!(unit.as_deref(), Some("USD"));
    assert!(observer.reported_cost(r#"{"output_tokens":7}"#).is_none());
    assert!(binding.counters["reported_total"].reported_cost);
}

#[test]
fn common_result_projection_reads_structured_citations_without_client_argument_or_text_fallback() {
    let binding = projected("openai-chat");
    let actual = json!({"choices":[{"message":{"content":"ordinary answer", "annotations":[
        {"type":"url_citation","url_citation":{"title":"Native source","url":"https://source.example/page","content":"Actual excerpt"}},
        {"type":"url_citation","url":"https://source.example/other","title":"Second source"}]}}]});
    let found = project_result(
        &binding,
        &json!({"query":"actual service query"}),
        &actual,
        10,
    )
    .unwrap();
    assert_eq!(found.as_array().unwrap().len(), 2);
    assert_eq!(found[0]["url"], "https://source.example/page");
    assert_eq!(found[0]["content"], "Actual excerpt");
    assert_eq!(found[1]["title"], "Second source");
    let untrusted = json!({"choices":[{"message":{"content":"{\"annotations\":[...]}","tool_calls":[{"function":{"arguments":"{\"annotations\":[...]}"}}]}}]});
    assert_eq!(
        project_result(&binding, &json!({"query":"q"}), &untrusted, 10).unwrap(),
        json!([])
    );
}
