#![allow(
    clippy::unwrap_used,
    reason = "recorded-frame controls fail by panicking"
)]
use super::*;
fn base() -> Url {
    Url::parse("https://openrouter.ai/api/v1").unwrap()
}
fn transcript(text: &str) -> Vec<Json> {
    text.lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn vendor_bases_and_native_offers_do_not_enable_paid_search_or_offer_fetch() {
    assert_eq!(endpoint("openai-chat", &base()), Some(Endpoint::Chat));
    assert_eq!(
        endpoint("openai-responses", &base()),
        Some(Endpoint::Responses)
    );
    assert_eq!(
        endpoint(
            "anthropic-messages",
            &Url::parse("https://openrouter.ai/api").unwrap()
        ),
        Some(Endpoint::Messages)
    );
    for url in [
        "http://openrouter.ai/api/v1",
        "https://openrouter.ai.example.invalid/api/v1",
        "https://openrouter.ai:444/api/v1",
        "https://user@openrouter.ai/api/v1",
        "https://openrouter.ai/api/v1?key=x",
        "https://openrouter.ai/api/v1#other",
        "https://openrouter.ai/",
        "https://openrouter.ai/api/v1//",
    ] {
        assert!(
            endpoint("openai-chat", &Url::parse(url).unwrap()).is_none(),
            "{url}"
        );
    }
    assert!(endpoint("openai-embeddings", &base()).is_none());
    let mut model = ModelSuggestion {
        model_name: "vendor/text-model".into(),
        display_name: "Text model".into(),
        parameters: Default::default(),
        listed: true,
        origins: Default::default(),
    };
    enrich("openai-chat", &base(), &mut model);
    assert_eq!(
        model.parameters.offered.unwrap().web_search,
        Some(vec!["live".into()])
    );
    assert!(model.parameters.web_search.is_none());
    assert!(model.parameters.default_web_search.is_none());
    for form in [Form::ServerTool, Form::Plugin] {
        assert_eq!(
            declaration("openai-chat", &base(), "vendor/text-model", form).unwrap()["supports_fetch"],
            false
        );
    }
}

#[test]
fn source_fragments_preserve_explicit_engine_options_and_separate_native_from_neutral_mode() {
    let options = json!({"engine":"parallel","engine_mode":"fast","max_results":7,"search_context_size":"low","max_uses":2});
    let patch = request_patch(Form::ServerTool, &options).unwrap();
    assert_eq!(patch["tools"][0]["type"], "openrouter:web_search");
    assert_eq!(patch["tools"][0]["parameters"]["mode"], "fast");
    assert_eq!(patch["tools"][0]["parameters"]["max_results"], 7);
    assert!(patch["tools"][0]["parameters"].get("engine_mode").is_none());
    assert_eq!(
        patch["tools"][0]["parameters"]["search_context_size"],
        "low"
    );
    let options = json!({"engine":"exa","engine_mode":"deep","max_results":12,"search_prompt":"Actual supplied prompt","search_context_size":"high","allowed_domains":["example.org"]});
    let patch = request_patch(Form::Plugin, &options).unwrap();
    assert_eq!(patch["plugins"][0]["id"], "web");
    assert_eq!(patch["plugins"][0]["mode"], "deep");
    assert_eq!(
        patch["plugins"][0]["search_prompt"],
        "Actual supplied prompt"
    );
    assert_eq!(
        patch["plugins"][0]["include_domains"],
        json!(["example.org"])
    );
    assert_eq!(patch["web_search_options"]["search_context_size"], "high");
    assert!(patch["plugins"][0].get("search_context_size").is_none());
    let default = request_patch(Form::Plugin, &json!({})).unwrap();
    assert!(
        default["plugins"][0].get("max_results").is_none(),
        "the vendor default is not a lock or an explicit request"
    );
    assert!(default["plugins"][0].get("engine").is_none());
    assert!(request_patch(Form::ServerTool, &json!({"mode":"live"})).is_err());
    assert!(
        request_patch(
            Form::Plugin,
            &json!({"engine":"perplexity","max_results":21})
        )
        .is_err()
    );
    assert!(request_patch(Form::Plugin, &json!({"max_results":true})).is_err());
}

#[test]
fn executable_aliases_and_both_containers_are_classified_without_renaming_a_model() {
    let mut body = json!({"model":"vendor/m","tools":[{"type":"function","function":{"name":"web_search"}},{"type":"openrouter:web_search","parameters":{"engine":"exa","mode":"fast"}}]});
    assert_eq!(selection(&body).unwrap().unwrap().engine, Engine::Exa);
    disable(&mut body).unwrap();
    assert_eq!(
        body["tools"],
        json!([{"type":"function","function":{"name":"web_search"}}])
    );
    assert!(selection(&body).unwrap().is_none());
    let mut plugin = json!({"model":"vendor/m","plugins":[{"id":"other","extra":true},{"id":"web","max_results":2}],"web_search_options":{"search_context_size":"medium"}});
    disable(&mut plugin).unwrap();
    assert_eq!(plugin["plugins"], json!([{"id":"other","extra":true}]));
    assert!(plugin.get("web_search_options").is_none());
    let mut online =
        json!({"model":"vendor/m:free:online","plugins":[{"id":"web","max_results":2}]});
    let before = online.clone();
    assert!(selection(&online).unwrap().unwrap().implicit_online);
    assert!(disable(&mut online).is_err());
    assert_eq!(online, before);
    assert!(
        selection(&json!({"tools":[{"type":"openrouter:web_search"}],"plugins":[{"id":"web"}]}))
            .is_err()
    );
    let mut root = json!({"model":"vendor/m","web_search_options":{"search_context_size":"high"}});
    assert!(selection(&root).unwrap().is_some());
    disable(&mut root).unwrap();
    assert!(selection(&root).unwrap().is_none());
}

#[test]
fn every_engine_price_retains_its_unit_mode_and_actual_result_terms() {
    for (engine, mode, amount) in [
        (Engine::Exa, "instant", "0.007"),
        (Engine::Exa, "fast", "0.007"),
        (Engine::Exa, "auto", "0.007"),
        (Engine::Exa, "deep-lite", "0.012"),
        (Engine::Exa, "deep", "0.012"),
        (Engine::Exa, "deep-reasoning", "0.015"),
        (Engine::Parallel, "turbo", "0.001"),
        (Engine::Parallel, "fast", "0.001"),
        (Engine::Parallel, "basic", "0.005"),
        (Engine::Parallel, "advanced", "0.005"),
    ] {
        let quote = price(engine, Some(mode)).unwrap();
        assert_eq!(quote["unit"], "USD");
        assert_eq!(quote["per_call"], amount);
        assert_eq!(quote["included_results_per_call"], 10);
        assert_eq!(quote["per_additional_result"], "0.001");
    }
    assert_eq!(
        price(Engine::Firecrawl, None).unwrap(),
        json!({"unit":"USD","per_call":"0.005","included_results_per_call":0,"per_additional_result":"0.0125"})
    );
    assert_eq!(
        price(Engine::Perplexity, None).unwrap()["per_call"],
        "0.005"
    );
    assert!(price(Engine::Native, None).is_none());
    assert!(price(Engine::Auto, None).is_none());
    assert!(price(Engine::Exa, Some("guessed")).is_none());
}

#[test]
fn recorded_server_frames_keep_citations_and_cumulative_counts_separate_from_total_tool_calls() {
    let mut stream = Stream::new("openai-chat", &base(), "vendor/m").unwrap();
    for frame in transcript(include_str!("server.jsonl")) {
        stream.frame(&frame);
    }
    assert_eq!(stream.results().len(), 2);
    assert_eq!(
        stream.results()[0]["content"],
        "First excerpt. [...] Another excerpt."
    );
    assert_eq!(stream.usage().search_requests, Some(2));
    assert_eq!(stream.usage().reported_total_usd, Some(json!(0.019)));
    let last = transcript(include_str!("server.jsonl")).pop().unwrap();
    stream.frame(&last);
    assert_eq!(stream.usage().search_requests, Some(2));
    assert_eq!(stream.results().len(), 2);
    assert_eq!(
        stream.presentation("actual service query")["kind"],
        "web_search"
    );
    assert_eq!(
        stream.presentation("actual service query")["query"],
        "actual service query"
    );
}

#[test]
fn recorded_plugin_response_retains_native_content_without_inventing_call_or_result_counts() {
    let mut stream = Stream::new("openai-chat", &base(), "vendor/m").unwrap();
    for frame in transcript(include_str!("plugin.jsonl")) {
        stream.frame(&frame);
    }
    assert_eq!(stream.results().len(), 1);
    assert_eq!(stream.results()[0]["date"], "2026-11-01");
    assert_eq!(
        stream.usage().search_requests,
        None,
        "citations and one configured plugin are not a successful-query report"
    );
    assert_eq!(stream.usage().reported_total_usd, Some(json!(0.008)));
}

#[test]
fn responses_annotations_are_structured_metadata_and_malformed_counters_do_not_become_zero() {
    let mut stream = Stream::new("openai-responses", &base(), "vendor/m").unwrap();
    stream.frame(&json!({"type":"response.output_text.annotation.added","annotation":{"type":"url_citation","url":"https://example.org/response","title":"Actual response citation"}}));
    stream.frame(&json!({"response":{"output":[{"type":"message","content":[{"type":"output_text","annotations":[{"type":"url_citation","url":"https://example.org/response","title":"Actual response citation","content":"Actual source content"}]}]}],"usage":{"server_tool_use_details":{"web_search_requests":3},"cost":"0.025"}}}));
    assert_eq!(stream.results().len(), 1);
    assert_eq!(stream.results()[0]["content"], "Actual source content");
    assert_eq!(stream.usage().search_requests, Some(3));
    assert_eq!(stream.usage().reported_total_usd, Some(json!("0.025")));
    let mut empty = Stream::default();
    empty.frame(&json!({"choices":[{"message":{"content":"{\"annotations\":[...]}"}}],"usage":{"server_tool_use_details":{"web_search_requests":-1,"tool_calls_executed":7},"cost":-1}}));
    assert!(empty.results().is_empty());
    assert_eq!(*empty.usage(), Usage::default());
}
