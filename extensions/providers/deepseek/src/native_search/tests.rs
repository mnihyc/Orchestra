#![allow(
    clippy::unwrap_used,
    reason = "recorded-frame controls fail by panicking"
)]
use super::*;

fn base(endpoint: Endpoint) -> Url {
    Url::parse(match endpoint {
        Endpoint::Responses => "https://api.deepseek.com",
        Endpoint::Anthropic => "https://api.deepseek.com/anthropic",
    })
    .unwrap()
}
fn protocol(endpoint: Endpoint) -> &'static str {
    match endpoint {
        Endpoint::Responses => "openai-responses",
        Endpoint::Anthropic => "anthropic-messages",
    }
}
fn stream(endpoint: Endpoint) -> Stream {
    Stream::new(protocol(endpoint), &base(endpoint), "deepseek-v4-pro").unwrap()
}
fn transcript(value: &str) -> Vec<Json> {
    value
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

#[test]
fn only_exact_vendor_endpoint_model_pairs_offer_live_and_discovery_never_enables_it() {
    for e in [Endpoint::Responses, Endpoint::Anthropic] {
        assert_eq!(endpoint(protocol(e), &base(e)), Some(e));
        assert_eq!(
            offered(protocol(e), &base(e), "deepseek-v4-pro")
                .unwrap()
                .web_search,
            Some(vec!["live".into()])
        );
        assert_eq!(
            offered(protocol(e), &base(e), "deepseek-flash")
                .unwrap()
                .web_search,
            Some(Vec::new())
        );
        assert!(declaration(protocol(e), &base(e), "deepseek-flash").is_none());
        assert!(Stream::new(protocol(e), &base(e), "deepseek-flash").is_none());
        assert!(offered(protocol(e), &base(e), "deepseek-v4-pro-guessed").is_none());
        let mut model = ModelSuggestion {
            model_name: "deepseek-v4-pro".into(),
            display_name: "DeepSeek Pro".into(),
            parameters: Default::default(),
            listed: true,
            origins: Default::default(),
        };
        enrich(protocol(e), &base(e), &mut model);
        assert!(model.parameters.web_search.is_none());
        assert!(model.parameters.default_web_search.is_none());
        model.parameters.web_search = Some(vec!["disabled".into()]);
        model.parameters.default_web_search = Some("disabled".into());
        model.parameters.offered.as_mut().unwrap().context_max = Some(1_000_000);
        enrich(protocol(e), &base(e), &mut model);
        assert_eq!(model.parameters.web_search, Some(vec!["disabled".into()]));
        assert_eq!(
            model.parameters.default_web_search.as_deref(),
            Some("disabled")
        );
        assert_eq!(
            model.parameters.offered.unwrap().context_max,
            Some(1_000_000)
        );
    }
    for url in [
        "http://api.deepseek.com",
        "https://api.deepseek.com.example.invalid",
        "https://api.deepseek.com:444",
        "https://user@api.deepseek.com",
        "https://api.deepseek.com?key=x",
        "https://api.deepseek.com/#other",
        "https://api.deepseek.com/v1",
        "https://api.deepseek.com//",
    ] {
        assert!(
            endpoint("openai-responses", &Url::parse(url).unwrap()).is_none(),
            "{url}"
        );
    }
    assert!(endpoint("anthropic-messages", &base(Endpoint::Responses)).is_none());
    assert!(endpoint("openai-responses", &base(Endpoint::Anthropic)).is_none());
    assert!(endpoint("openai-chat", &base(Endpoint::Responses)).is_none());
}

#[test]
fn declarations_select_the_native_tool_version_and_keep_per_call_prices_in_usd() {
    let r = declaration(
        "openai-responses",
        &base(Endpoint::Responses),
        "deepseek-v4-pro",
    )
    .unwrap();
    assert_eq!(r["tool"], json!({"type":"web_search"}));
    assert_eq!(r["paths"], json!(["/responses"]));
    assert_eq!(r["modes"], json!({"live":{}}));
    let a = declaration(
        "anthropic-messages",
        &base(Endpoint::Anthropic),
        "deepseek-v4-pro",
    )
    .unwrap();
    assert_eq!(
        a["tool"],
        json!({"type":"web_search_20250305","name":"web_search"})
    );
    assert_eq!(a["paths"], json!(["/v1/messages"]));
    assert_eq!(
        a["identities"],
        json!([
            {"type":"web_search_20250305","name":"web_search"},
            {"type":"web_search_20260209","name":"web_search"},
        ])
    );
    assert!(a["options_schema"]["properties"]["max_uses"].is_object());
    assert_eq!(
        prices(),
        json!({"unit":"USD","web_search_per_call":"0.00075","web_fetch_per_call":"0.00075"})
    );
}

#[test]
fn recorded_responses_lifecycle_yields_stable_search_and_fetch_items_and_aggregate_successes() {
    let mut stream = stream(Endpoint::Responses);
    let mut latest = BTreeMap::new();
    for (index, frame) in transcript(include_str!("responses.jsonl"))
        .into_iter()
        .enumerate()
    {
        for item in stream.frame(frame["event"].as_str().unwrap(), &frame["data"]) {
            latest.insert(item.id.clone(), item);
        }
        if index == 0 {
            assert_eq!(*stream.usage(), Usage::default());
        }
    }
    assert_eq!(
        latest.len(),
        2,
        "client-side tool discovery is not hosted search"
    );
    let search = &latest["ws_fixture"];
    assert_eq!(search.kind, "web_search");
    assert!(search.input.get("sources").is_none());
    assert_eq!(search.state, "completed");
    assert_eq!(
        search.presentation.as_ref().unwrap(),
        &json!({"kind":"web_search","query":"Python official website\nPython docs","results":[{"title":"Python","url":"https://www.python.org/"}]})
    );
    let fetch = &latest["fetch_fixture"];
    assert_eq!(fetch.kind, "web_fetch");
    assert_eq!(fetch.input["url"], "https://docs.python.org/");
    assert_eq!(stream.usage().searches, Some(3));
    assert_eq!(stream.usage().fetches, Some(1));
    assert!(stream.usage().attempted_searches.is_none());
    let terminal = transcript(include_str!("responses.jsonl")).pop().unwrap();
    stream.frame(terminal["event"].as_str().unwrap(), &terminal["data"]);
    assert_eq!(
        stream.usage().searches,
        Some(3),
        "a repeated cumulative report is not a delta"
    );
}

#[test]
fn recorded_messages_correlate_server_blocks_and_never_bill_attempts_or_failed_calls() {
    let mut stream = stream(Endpoint::Anthropic);
    let mut latest = BTreeMap::new();
    for (index, frame) in transcript(include_str!("anthropic.jsonl"))
        .into_iter()
        .enumerate()
    {
        for item in stream.frame(frame["event"].as_str().unwrap(), &frame["data"]) {
            latest.insert(item.id.clone(), item);
        }
        if index == 0 {
            assert_eq!(*stream.usage(), Usage::default());
        }
    }
    let search = &latest["srv_fixture"];
    assert_eq!(search.state, "completed");
    assert_eq!(
        search.presentation.as_ref().unwrap(),
        &json!({"kind":"web_search","query":"Python official website","results":[{"title":"Python","url":"https://www.python.org/"}]})
    );
    assert!(
        !search
            .presentation
            .as_ref()
            .unwrap()
            .to_string()
            .contains("opaque-fixture")
    );
    assert_eq!(latest["srv_failed_fixture"].state, "failed");
    assert_eq!(
        latest["srv_failed_fixture"].output.as_ref().unwrap()["error"],
        "max_uses_exceeded"
    );
    assert_eq!(stream.usage().searches, Some(1));
    assert_eq!(stream.usage().attempted_searches, Some(4));
    assert_eq!(
        stream.usage().fetches,
        None,
        "this endpoint has not told a page-read counter"
    );
}

#[test]
fn terminal_failed_or_incomplete_responses_keep_successes_without_estimating_from_items() {
    for event in ["response.failed", "response.incomplete"] {
        let mut stream = stream(Endpoint::Responses);
        stream.frame(event,&json!({"response":{"usage":{"input_tokens":7,"output_tokens":9,"server_tool_use":{"tool_search":2,"tool_fetch":3}}}}));
        assert_eq!(stream.usage().searches, Some(2));
        assert_eq!(stream.usage().fetches, Some(3));
    }
    assert_eq!(
        Usage::from_usage(Endpoint::Responses, &Json::Null),
        Usage::default()
    );
    assert_eq!(
        Usage::from_usage(Endpoint::Responses, &json!({})),
        Usage::default()
    );
    assert_eq!(
        Usage::from_usage(
            Endpoint::Responses,
            &json!({"input_tokens":0,"output_tokens":0})
        ),
        Usage {
            searches: Some(0),
            fetches: Some(0),
            attempted_searches: None
        }
    );
    for invalid in [
        json!(-1),
        json!(true),
        json!("3"),
        json!(1.5),
        json!(18446744073709551615_u64),
    ] {
        let usage = Usage::from_usage(
            Endpoint::Responses,
            &json!({"input_tokens":1,"output_tokens":1,"server_tool_use":{"tool_search":invalid,"tool_fetch":0}}),
        );
        assert_eq!(usage.searches, None);
        assert_eq!(usage.fetches, Some(0));
    }
    let only_attempts = Usage::from_usage(
        Endpoint::Anthropic,
        &json!({"output_tokens":1,"server_tool_use":{"web_search_requests":8}}),
    );
    assert_eq!(only_attempts.searches, None);
    assert_eq!(only_attempts.attempted_searches, Some(8));
}

#[test]
fn unassociated_result_blocks_and_client_tools_are_not_native_calls_and_partial_input_is_bounded() {
    let mut stream = stream(Endpoint::Anthropic);
    assert!(stream.frame("content_block_start",&json!({"index":0,"content_block":{"type":"tool_use","id":"client","name":"web_search","input":{"query":"not a server tool"}}})).is_empty());
    assert!(stream.frame("content_block_start",&json!({"index":1,"content_block":{"type":"web_search_tool_result","tool_use_id":"unrelated","content":[]}})).is_empty());
    stream.frame("content_block_start",&json!({"index":2,"content_block":{"type":"server_tool_use","id":"server","name":"web_search","input":{}}}));
    stream.frame("content_block_delta",&json!({"index":2,"delta":{"type":"input_json_delta","partial_json":"x".repeat(orch_provider_contract::limits::REPORT_MAX_BYTES+1)}}));
    assert!(
        stream
            .frame("content_block_stop", &json!({"index":2}))
            .is_empty()
    );
    assert_eq!(*stream.usage(), Usage::default());
}

#[test]
fn mismatched_event_types_and_reused_client_block_indices_cannot_relabel_server_items() {
    let mut s = stream(Endpoint::Anthropic);
    assert!(s.frame("content_block_start",&json!({"type":"message_stop","index":0,"content_block":{"type":"server_tool_use","id":"wrong","name":"web_search","input":{"query":"wrong"}}})).is_empty());
    s.frame("content_block_start",&json!({"index":1,"content_block":{"type":"server_tool_use","id":"real","name":"web_search","input":{"query":"original"}}}));
    s.frame("content_block_start",&json!({"index":1,"content_block":{"type":"tool_use","id":"client","name":"some_client_tool","input":{}}}));
    s.frame("content_block_delta",&json!({"index":1,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"client data\"}"}}));
    assert!(
        s.frame("content_block_stop", &json!({"index":1}))
            .is_empty()
    );
    let result=s.frame("content_block_start",&json!({"index":2,"content_block":{"type":"web_search_tool_result","tool_use_id":"real","content":[]}}));
    assert_eq!(result[0].input["query"], "original");
    let mut r = stream(Endpoint::Responses);
    for queries in [json!([]), json!([1]), json!(["real", false])] {
        let item=r.frame("response.output_item.done",&json!({"item":{"id":"partial","type":"web_search_call","status":"completed","action":{"type":"search","queries":queries}}}));
        assert!(
            item[0].presentation.is_none(),
            "invalid query arrays are not an invented query"
        );
    }
}

#[test]
fn nullable_and_zero_native_limits_are_accepted_without_inventing_a_context_dependent_cap() {
    let declared = declaration(
        "anthropic-messages",
        &base(Endpoint::Anthropic),
        "deepseek-v4-pro",
    )
    .unwrap();
    let schema = &declared["options_schema"];
    for value in [
        json!({}),
        json!({"max_uses":null}),
        json!({"max_uses":0}),
        json!({"max_uses":1}),
        json!({"max_uses":100}),
    ] {
        assert!(
            orch_types::schema::validate(schema, &value).is_ok(),
            "{value}"
        );
    }
    for value in [
        json!({"max_uses":-1}),
        json!({"max_uses":true}),
        json!({"max_uses":"1"}),
    ] {
        assert!(
            orch_types::schema::validate(schema, &value).is_err(),
            "{value}"
        );
    }
    assert!(
        declared["options_schema"]["properties"]["max_uses"]
            .get("default")
            .is_none()
    );
    assert!(
        declared["options_schema"]["properties"]["max_uses"]
            .get("maximum")
            .is_none()
    );
}
