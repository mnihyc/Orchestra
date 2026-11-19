//! Tests of the whole scripted model server, through its own client.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use super::*;

async fn start() -> MockLlm {
    MockLlm::start(MockConfig::default()).await.unwrap()
}

fn responses_request(input: &str, stream: bool) -> ClientRequest {
    ClientRequest::post_json(
        "/v1/responses",
        &json!({ "model": "mock-large", "input": input, "stream": stream }),
    )
}

fn messages_request(text: &str, stream: bool) -> ClientRequest {
    ClientRequest::post_json(
        "/v1/messages",
        &json!({ "model": "mock-large", "max_tokens": 64, "stream": stream,
                 "messages": [{ "role": "user", "content": text }] }),
    )
    .messages_version()
}

fn event_names(reply: &ClientReply) -> Vec<String> {
    reply
        .events()
        .into_iter()
        .filter_map(|event| event.event)
        .collect()
}

fn data_of(reply: &ClientReply, name: &str) -> Vec<Value> {
    reply
        .events()
        .into_iter()
        .filter(|event| event.event.as_deref() == Some(name))
        .map(|event| serde_json::from_str(&event.data).unwrap())
        .collect()
}

#[tokio::test]
async fn a_directive_in_the_message_streams_a_response_of_the_responses_format() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(responses_request(
            "#!mock\nchunk 5\nsay Hello there\nusage 20 6",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(
        reply.header("content-type"),
        Some("text/event-stream; charset=utf-8")
    );
    let names = event_names(&reply);
    assert_eq!(names.first().map(String::as_str), Some("response.created"));
    assert_eq!(names.last().map(String::as_str), Some("response.completed"));
    let deltas: Vec<_> = data_of(&reply, "response.output_text.delta")
        .iter()
        .map(|d| d["delta"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(deltas, ["Hello", " ther", "e"]);
    let completed = &data_of(&reply, "response.completed")[0]["response"];
    assert_eq!(completed["usage"]["input_tokens"], 20);
    assert_eq!(completed["usage"]["output_tokens"], 6);
    assert_eq!(completed["model"], "mock-large");
}

#[tokio::test]
async fn a_directive_in_the_message_streams_a_message_of_the_messages_format() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(messages_request(
            "#!mock\nthink hmm\nsay Hi\nusage 30 9 reasoning=3",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    let names = event_names(&reply);
    assert_eq!(names.first().map(String::as_str), Some("message_start"));
    assert_eq!(names.last().map(String::as_str), Some("message_stop"));
    assert!(names.contains(&"ping".to_owned()));
    let delta = &data_of(&reply, "message_delta")[0];
    assert_eq!(delta["usage"]["output_tokens"], 9);
    assert_eq!(
        delta["usage"]["output_tokens_details"]["thinking_tokens"],
        3
    );
    assert_eq!(
        data_of(&reply, "message_start")[0]["message"]["usage"]["input_tokens"],
        30
    );
}

#[tokio::test]
async fn a_plain_request_gets_one_document_that_agrees_with_the_stream() {
    let mock = start().await;
    let script = "#!mock\nsay Hello\ntool t {\"a\":1}\nusage 12 7";
    let plain = mock
        .client()
        .send(responses_request(script, false))
        .await
        .unwrap();
    assert_eq!(plain.header("content-type"), Some("application/json"));
    let document = plain.json();
    assert_eq!(document["object"], "response");
    assert_eq!(document["usage"]["total_tokens"], 19);
    let streamed = mock
        .client()
        .send(responses_request(script, true))
        .await
        .unwrap();
    let completed = &data_of(&streamed, "response.completed")[0]["response"];
    let content = |output: &Value| -> Vec<(Value, Value, Value)> {
        output
            .as_array()
            .unwrap()
            .iter()
            .map(|item| {
                (
                    item["type"].clone(),
                    item["content"].clone(),
                    item["arguments"].clone(),
                )
            })
            .collect()
    };
    assert_eq!(content(&completed["output"]), content(&document["output"]));
    let plain = mock
        .client()
        .send(messages_request(script, false))
        .await
        .unwrap()
        .json();
    assert_eq!(plain["type"], "message");
    assert_eq!(plain["stop_reason"], "tool_use");
    assert_eq!(plain["content"][1]["input"], json!({ "a": 1 }));
}

#[tokio::test]
async fn without_a_script_the_server_says_what_it_was_told() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(responses_request("hello", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["output"][0]["content"][0]["text"], "You said: hello");
    let reply = mock
        .client()
        .send(messages_request("hello", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["content"][0]["text"], "You said: hello");
    let requests = mock.settled(2).await;
    assert!(
        requests
            .iter()
            .all(|r| r.script == Some(ScriptSource::Echo))
    );
}

#[tokio::test]
async fn tool_calls_are_continued_from_the_results_in_the_next_request_in_the_messages_format() {
    let mock = start().await;
    let script = "#!mock\nthink I will look.\ntool read_file {\"path\":\"a.txt\"}\nthen\nsay The file says: {{result}}";
    let first = mock
        .client()
        .send(messages_request(script, true))
        .await
        .unwrap();
    let tool = data_of(&first, "content_block_start")
        .into_iter()
        .find(|d| d["content_block"]["type"] == "tool_use")
        .unwrap();
    let id = tool["content_block"]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        data_of(&first, "message_delta")[0]["delta"]["stop_reason"],
        "tool_use"
    );
    let second = ClientRequest::post_json(
        "/v1/messages",
        &json!({ "model": "mock-large", "max_tokens": 64, "stream": true, "messages": [
            { "role": "user", "content": script },
            { "role": "assistant", "content": [{ "type": "tool_use", "id": id, "name": "read_file", "input": { "path": "a.txt" } }] },
            { "role": "user", "content": [{ "type": "tool_result", "tool_use_id": id, "content": "hello" }] }
        ] }),
    )
    .messages_version();
    let second = mock.client().send(second).await.unwrap();
    let text: String = data_of(&second, "content_block_delta")
        .iter()
        .map(|d| d["delta"]["text"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(text, "The file says: hello");
    assert_eq!(
        data_of(&second, "message_delta")[0]["delta"]["stop_reason"],
        "end_turn"
    );
    let requests = mock.settled(2).await;
    assert_eq!((requests[0].round, requests[1].round), (0, 1));
}

#[tokio::test]
async fn tool_calls_are_continued_from_the_results_in_the_next_request_in_the_responses_format() {
    let mock = start().await;
    let script = "#!mock\ntool a {}\ntool b {}\nthen\nsay got {{result}}\nthen\nsay never";
    let first = mock
        .client()
        .send(responses_request(script, false))
        .await
        .unwrap()
        .json();
    let calls: Vec<_> = first["output"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| i["call_id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(calls.len(), 2);
    let second = ClientRequest::post_json(
        "/v1/responses",
        &json!({ "model": "mock-large", "input": [
            { "type": "message", "role": "user", "content": [{ "type": "input_text", "text": script }] },
            { "type": "function_call", "call_id": calls[0], "name": "a", "arguments": "{}" },
            { "type": "function_call", "call_id": calls[1], "name": "b", "arguments": "{}" },
            { "type": "function_call_output", "call_id": calls[0], "output": "A" },
            { "type": "function_call_output", "call_id": calls[1], "output": "B" }
        ] }),
    );
    let second = mock.client().send(second).await.unwrap().json();
    assert_eq!(second["output"][0]["content"][0]["text"], "got A\nB");
}

#[tokio::test]
async fn a_request_that_names_the_response_it_continues_needs_no_history() {
    let mock = start().await;
    let script = "#!mock\ntool a {}\nthen\nsay done after {{result}}";
    let first = mock
        .client()
        .send(responses_request(script, false))
        .await
        .unwrap()
        .json();
    let id = first["id"].as_str().unwrap().to_owned();
    let call = first["output"][0]["call_id"].as_str().unwrap().to_owned();
    let second = ClientRequest::post_json(
        "/v1/responses",
        &json!({ "model": "mock-large", "previous_response_id": id,
                 "input": [{ "type": "function_call_output", "call_id": call, "output": "ok" }] }),
    );
    let second = mock.client().send(second).await.unwrap().json();
    assert_eq!(second["output"][0]["content"][0]["text"], "done after ok");
}

#[tokio::test]
async fn a_script_that_has_no_response_left_for_a_round_ends_the_turn_with_a_plain_answer() {
    let mock = start().await;
    let request = ClientRequest::post_json(
        "/v1/responses",
        &json!({ "model": "mock-large", "input": [
            { "role": "user", "content": "#!mock\ntool a {}" },
            { "type": "function_call", "call_id": "c1", "name": "a", "arguments": "{}" },
            { "type": "function_call_output", "call_id": "c1", "output": "x" }
        ] }),
    );
    let reply = mock.client().send(request).await.unwrap().json();
    assert_eq!(reply["output"][0]["content"][0]["text"], "Done.");
}

#[tokio::test]
async fn a_script_set_ahead_answers_what_has_no_directive_and_a_directive_wins_over_it() {
    let mock = start().await;
    mock.queue("say from the queue").unwrap();
    let directive = mock
        .client()
        .send(responses_request("#!mock\nsay from the message", false))
        .await
        .unwrap()
        .json();
    assert_eq!(
        directive["output"][0]["content"][0]["text"],
        "from the message"
    );
    let queued = mock
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap()
        .json();
    assert_eq!(queued["output"][0]["content"][0]["text"], "from the queue");
    let echoed = mock
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap()
        .json();
    assert_eq!(
        echoed["output"][0]["content"][0]["text"], "You said: hi",
        "the queue answered once"
    );
    let sources: Vec<_> = mock.settled(3).await.iter().map(|r| r.script).collect();
    assert_eq!(
        sources,
        [
            Some(ScriptSource::Directive),
            Some(ScriptSource::Queue),
            Some(ScriptSource::Echo)
        ]
    );
}

#[tokio::test]
async fn responses_set_ahead_answer_the_next_requests_in_order_and_can_repeat() {
    let mock = start().await;
    mock.queue("say one\nthen\nsay two").unwrap();
    mock.queue_for("say again", &QueueFilter::default(), 2)
        .unwrap();
    let mut said = Vec::new();
    for _ in 0..4 {
        let reply = mock
            .client()
            .send(messages_request("x", false))
            .await
            .unwrap()
            .json();
        said.push(reply["content"][0]["text"].as_str().unwrap().to_owned());
    }
    assert_eq!(said, ["one", "two", "again", "again"]);
}

#[tokio::test]
async fn a_response_set_ahead_for_a_model_is_not_stolen_by_another_model() {
    let mock = start().await;
    let filter = QueueFilter {
        model: Some("claude-haiku*".into()),
        format: Some(Format::Messages),
    };
    mock.queue_for("say for haiku", &filter, 1).unwrap();
    let other = ClientRequest::post_json(
        "/v1/messages",
        &json!({ "model": "claude-opus-5", "max_tokens": 8, "messages": [{ "role": "user", "content": "x" }] }),
    )
    .messages_version();
    let reply = mock.client().send(other).await.unwrap().json();
    assert_eq!(reply["content"][0]["text"], "You said: x");
    let haiku = ClientRequest::post_json(
        "/v1/messages",
        &json!({ "model": "claude-haiku-5.5", "max_tokens": 8, "messages": [{ "role": "user", "content": "x" }] }),
    )
    .messages_version();
    let reply = mock.client().send(haiku).await.unwrap().json();
    assert_eq!(reply["content"][0]["text"], "for haiku");
    assert_eq!(
        reply["model"], "claude-haiku-5.5",
        "the model the request names is the one reported"
    );
}

#[tokio::test]
async fn the_default_script_answers_when_nothing_else_does() {
    let mock = start().await;
    mock.set_default("say by default").unwrap();
    let reply = mock
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["output"][0]["content"][0]["text"], "by default");
    assert_eq!(mock.settled(1).await[0].script, Some(ScriptSource::Default));
    let configured = MockLlm::start(MockConfig::default().with_default_script("say configured"))
        .await
        .unwrap();
    let reply = configured
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["output"][0]["content"][0]["text"], "configured");
}

#[tokio::test]
async fn a_malformed_script_is_refused_with_its_line() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(responses_request("#!mock\nsay fine\nsya typo", false))
        .await
        .unwrap();
    assert_eq!(reply.status, 400);
    let message = reply.json()["error"]["message"]
        .as_str()
        .unwrap()
        .to_owned();
    assert!(
        message.contains("line 3") && message.contains("unknown directive"),
        "{message}"
    );
    let reply = mock
        .client()
        .send(messages_request("#!mock\nusage lots", false))
        .await
        .unwrap();
    assert_eq!(reply.status, 400);
    assert_eq!(reply.json()["error"]["type"], "invalid_request_error");
}

#[tokio::test]
async fn each_kind_of_error_is_the_http_answer_of_the_real_interface() {
    let mock = start().await;
    let ask = |text: String, messages: bool| {
        let client = mock.client();
        async move {
            let request = if messages {
                messages_request(&text, true)
            } else {
                responses_request(&text, true)
            };
            client.send(request).await.unwrap()
        }
    };
    let reply = ask(
        "#!mock\nsay never\nerror rate_limit retry-after=7".into(),
        false,
    )
    .await;
    assert_eq!(reply.status, 429);
    assert_eq!(reply.header("retry-after"), Some("7"));
    assert_eq!(
        reply.header("content-type"),
        Some("application/json"),
        "an error is a document and no stream"
    );
    assert_eq!(reply.json()["error"]["type"], "rate_limit_error");
    let reply = ask("#!mock\nerror rate_limit".into(), true).await;
    assert_eq!(
        (reply.status.as_u16(), reply.header("retry-after")),
        (429, Some("2"))
    );
    assert_eq!(reply.json()["error"]["type"], "rate_limit_error");
    assert_eq!(reply.json()["type"], "error");
    let reply = ask("#!mock\nerror overloaded".into(), false).await;
    assert_eq!(
        (
            reply.status.as_u16(),
            reply.json()["error"]["code"].as_str()
        ),
        (503, Some("server_is_overloaded"))
    );
    let reply = ask("#!mock\nerror overloaded retry-after=none".into(), true).await;
    assert_eq!(
        (reply.status.as_u16(), reply.header("retry-after")),
        (529, None)
    );
    assert_eq!(reply.json()["error"]["type"], "overloaded_error");
    let reply = ask(
        "#!mock\nerror invalid_request model: not usable".into(),
        true,
    )
    .await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.json()["error"]["message"], "model: not usable");
    let reply = ask("#!mock\nerror authentication".into(), false).await;
    assert_eq!(
        (
            reply.status.as_u16(),
            reply.json()["error"]["code"].as_str()
        ),
        (401, Some("invalid_api_key"))
    );
    let reply = ask("#!mock\nerror server status=502".into(), false).await;
    assert_eq!(reply.status, 502);
    let requests = mock.settled(7).await;
    assert!(
        requests
            .iter()
            .all(|r| r.outcome == Outcome::Error && r.status.is_some_and(|s| s >= 400))
    );
}

#[tokio::test]
async fn a_limit_of_a_plan_is_answered_in_the_headers_and_the_document_each_vendor_uses() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(messages_request(
            "#!mock\nerror plan_limit window=weekly resets=1792000000",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 429);
    assert_eq!(reply.header("retry-after"), None);
    assert_eq!(
        reply.header("anthropic-ratelimit-unified-status"),
        Some("rejected")
    );
    assert_eq!(
        reply.header("anthropic-ratelimit-unified-representative-claim"),
        Some("seven_day")
    );
    assert_eq!(
        reply.header("anthropic-ratelimit-unified-reset"),
        Some("1792000000")
    );
    assert_eq!(reply.json()["error"]["type"], "rate_limit_error");
    let reply = mock
        .client()
        .send(responses_request(
            "#!mock\nerror plan_limit window=session resets=1792000000 plan=plus",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 429);
    assert_eq!(reply.json()["error"]["type"], "usage_limit_reached");
    assert_eq!(reply.json()["error"]["plan_type"], "plus");
    assert_eq!(reply.json()["error"]["resets_at"], 1_792_000_000);
    assert_eq!(reply.header("x-codex-primary-used-percent"), Some("100"));
    assert_eq!(reply.header("x-codex-primary-reset-at"), Some("1792000000"));
    let after = mock
        .client()
        .send(responses_request(
            "#!mock\nerror plan_limit resets=+3600",
            false,
        ))
        .await
        .unwrap();
    let ends = after.json()["error"]["resets_at"].as_i64().unwrap();
    let now = after.json()["error"]["resets_in_seconds"].as_i64().unwrap();
    assert_eq!(
        now, 3600,
        "a limit that ends in an hour says so, by the server's clock"
    );
    assert!(ends > 1_700_000_000);
}

#[tokio::test]
async fn a_prompt_that_is_too_long_and_a_page_from_a_gateway_are_answered_as_the_real_ones_are() {
    let mock = start().await;
    let long = mock
        .client()
        .send(messages_request("#!mock\nerror context_length", true))
        .await
        .unwrap();
    assert_eq!(long.status, 400);
    assert!(
        long.json()["error"]["message"]
            .as_str()
            .is_some_and(|text| text.starts_with("prompt is too long"))
    );
    let long = mock
        .client()
        .send(responses_request("#!mock\nerror context_length", true))
        .await
        .unwrap();
    assert_eq!(long.json()["error"]["code"], "context_length_exceeded");
    let page = mock
        .client()
        .send(responses_request(
            "#!mock\nheader x-gateway proxy-7\nbody 502 <html><body>502 Bad Gateway</body></html>",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(page.status, 502);
    assert_eq!(
        page.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        page.header("x-gateway"),
        Some("proxy-7"),
        "a header is on whatever the answer is"
    );
    assert_eq!(page.text(), "<html><body>502 Bad Gateway</body></html>");
    assert!(serde_json::from_str::<Value>(&page.text()).is_err());
    assert!(
        mock.settled(3)
            .await
            .iter()
            .all(|r| r.outcome == Outcome::Error)
    );
}

#[tokio::test]
async fn an_event_nobody_knows_and_a_line_that_is_no_json_come_inside_the_stream() {
    let mock = start().await;
    for request in [
        responses_request(
            "#!mock\nsay before\nevent mystery.event {\"n\": 1}\ngarbage this is {not json\nsay after",
            true,
        ),
        messages_request(
            "#!mock\nsay before\nevent mystery_event {\"n\": 1}\ngarbage this is {not json\nsay after",
            true,
        ),
    ] {
        let reply = mock.client().send(request).await.unwrap();
        assert_eq!(reply.status, 200);
        let text = reply.text();
        assert!(text.contains("data: this is {not json\n\n"), "{text}");
        let unknown = reply
            .events()
            .into_iter()
            .find(|event| {
                event
                    .event
                    .as_deref()
                    .is_some_and(|name| name.starts_with("mystery"))
            })
            .expect("the unknown event is in the stream");
        assert_eq!(unknown.data, "{\"n\": 1}");
        assert!(
            text.contains("response.completed") || text.contains("message_stop"),
            "the stream goes on after both"
        );
    }
}

#[tokio::test]
async fn a_stream_cut_after_some_characters_breaks_inside_the_sentence_and_inside_the_call() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(messages_request(
            "#!mock\nchunk 4\nsay The answer is that the file\ncut_after 10",
            true,
        ))
        .await
        .unwrap();
    assert!(reply.broke.is_some());
    let said: String = data_of(&reply, "content_block_delta")
        .iter()
        .filter_map(|delta| delta["delta"]["text"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(said, "The answer");
    let reply = mock
        .client()
        .send(responses_request(
            "#!mock\ntool Bash {\"command\": \"ls -la /tmp\"}\ncut_after 14",
            true,
        ))
        .await
        .unwrap();
    assert!(reply.broke.is_some());
    let arguments: String = data_of(&reply, "response.function_call_arguments.delta")
        .iter()
        .filter_map(|delta| delta["delta"].as_str().map(str::to_owned))
        .collect();
    assert_eq!(arguments, "{\"command\":\"ls");
    assert!(
        serde_json::from_str::<Value>(&arguments).is_err(),
        "the call is cut inside its arguments"
    );
}

#[tokio::test]
async fn an_answer_can_be_held_back_before_its_first_byte_and_can_carry_a_header_of_its_own() {
    let mock = start().await;
    let started = Instant::now();
    let reply = mock
        .client()
        .send(responses_request(
            "#!mock\nstall 300\nheader x-gateway proxy-7\nsay late",
            true,
        ))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(300));
    assert_eq!(reply.status, 200);
    assert_eq!(reply.header("x-gateway"), Some("proxy-7"));
    let error = mock
        .client()
        .send(messages_request(
            "#!mock\nheader retry-after-ms 20\nerror overloaded",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(error.status, 529);
    assert_eq!(error.header("retry-after-ms"), Some("20"));
}

#[tokio::test]
async fn an_error_inside_the_stream_comes_after_what_was_sent_and_ends_it() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(messages_request(
            "#!mock\nsay partial\nstream_error overloaded",
            true,
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 200, "the status was sent before the error");
    let names = event_names(&reply);
    assert_eq!(names.last().map(String::as_str), Some("error"));
    assert!(!names.contains(&"message_stop".to_owned()));
    assert_eq!(
        data_of(&reply, "error")[0]["error"]["type"],
        "overloaded_error"
    );
    let reply = mock
        .client()
        .send(responses_request(
            "#!mock\nsay partial\nfail rate_limit",
            true,
        ))
        .await
        .unwrap();
    let failed = &data_of(&reply, "response.failed")[0]["response"];
    assert_eq!(failed["error"]["code"], "rate_limit_exceeded");
    assert_eq!(mock.settled(2).await[1].outcome, Outcome::Error);
}

#[tokio::test]
async fn a_cut_stream_breaks_the_body_after_what_came_before_it() {
    let mock = start().await;
    for request in [
        responses_request("#!mock\nchunk 3\nsay abcdefghi\ncut", true),
        messages_request("#!mock\nchunk 3\nsay abcdefghi\ncut", true),
    ] {
        let reply = mock.client().send(request).await.unwrap();
        assert_eq!(reply.status, 200);
        assert!(
            reply.broke.is_some(),
            "the client sees the connection break"
        );
        let names = event_names(&reply);
        assert!(names.iter().any(|n| n.ends_with("delta")), "{names:?}");
        assert!(
            !names
                .iter()
                .any(|n| n == "response.completed" || n == "message_stop")
        );
    }
    let requests = mock.settled(2).await;
    assert!(requests.iter().all(|r| r.outcome == Outcome::Cut));
}

#[tokio::test]
async fn a_truncated_stream_ends_cleanly_without_its_closing_events() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(responses_request("#!mock\nsay abcdefghi\ntruncate", true))
        .await
        .unwrap();
    assert!(reply.broke.is_none(), "the body ended as a body ends");
    assert!(!event_names(&reply).contains(&"response.completed".to_owned()));
    assert_eq!(mock.settled(1).await[0].outcome, Outcome::Truncated);
}

#[tokio::test]
async fn a_plain_answer_can_be_cut_in_half_too() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(messages_request("#!mock\nsay hello\ncut", false))
        .await
        .unwrap();
    assert!(reply.broke.is_some());
    assert!(
        serde_json::from_slice::<Value>(&reply.body()).is_err(),
        "half a document is none"
    );
}

#[tokio::test]
async fn a_slow_stream_is_slow_and_its_first_piece_comes_before_its_last() {
    let mock = start().await;
    let started = Instant::now();
    let mut open = mock
        .client()
        .open(responses_request(
            "#!mock\nchunk 2\ndelay 100\nsay abcdef",
            true,
        ))
        .await
        .unwrap();
    let mut first = None;
    let mut count = 0;
    while let Some(chunk) = open.next_chunk().await.unwrap() {
        count += 1;
        if first.is_none() && String::from_utf8_lossy(&chunk).contains("output_text.delta") {
            first = Some(started.elapsed());
        }
    }
    let total = started.elapsed();
    assert!(
        total >= Duration::from_millis(290),
        "three pieces, a wait before each: {total:?}"
    );
    assert!(
        first.unwrap() < total - Duration::from_millis(150),
        "the first piece was not held back"
    );
    assert!(count > 3);
}

#[tokio::test]
async fn a_sleep_holds_back_a_plain_answer_as_well() {
    let mock = start().await;
    let started = Instant::now();
    mock.client()
        .send(messages_request("#!mock\nsleep 120\nsay late", false))
        .await
        .unwrap();
    assert!(started.elapsed() >= Duration::from_millis(110));
}

#[tokio::test]
async fn a_client_that_goes_away_is_noted() {
    let mock = start().await;
    let mut open = mock
        .client()
        .open(responses_request(
            "#!mock\nchunk 1\ndelay 80\nsay a long answer that is never read",
            true,
        ))
        .await
        .unwrap();
    open.next_chunk().await.unwrap();
    drop(open);
    let requests = mock.settled(1).await;
    assert_eq!(requests[0].outcome, Outcome::ClientGone);
}

#[tokio::test]
async fn a_server_with_a_credential_refuses_a_request_that_does_not_present_it() {
    let mock = MockLlm::start(MockConfig::default().with_credential("sk-test"))
        .await
        .unwrap();
    let missing = mock
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap();
    assert_eq!(missing.status, 401);
    assert_eq!(missing.json()["error"]["code"], "invalid_api_key");
    let wrong = mock
        .client()
        .send(messages_request("hi", false).bearer("sk-other"))
        .await
        .unwrap();
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.json()["error"]["type"], "authentication_error");
    let bearer = mock
        .client()
        .send(responses_request("hi", false).bearer("sk-test"))
        .await
        .unwrap();
    assert_eq!(bearer.status, 200);
    let header = mock
        .client()
        .send(messages_request("hi", false).x_api_key("sk-test"))
        .await
        .unwrap();
    assert_eq!(header.status, 200);
    let checks: Vec<_> = mock
        .settled(4)
        .await
        .iter()
        .map(|r| r.credential.verdict)
        .collect();
    assert_eq!(
        checks,
        [
            Verdict::Missing,
            Verdict::Wrong,
            Verdict::Accepted,
            Verdict::Accepted
        ]
    );
}

#[tokio::test]
async fn a_server_can_insist_on_one_way_of_presenting_the_credential() {
    let config = MockConfig::default()
        .with_credential("sk-test")
        .accepting(&[Presentation::Header("api-key".into())]);
    let mock = MockLlm::start(config).await.unwrap();
    let wrong_way = mock
        .client()
        .send(responses_request("hi", false).bearer("sk-test"))
        .await
        .unwrap();
    assert_eq!(
        wrong_way.status, 401,
        "the right key the wrong way is refused"
    );
    let right_way = mock
        .client()
        .send(responses_request("hi", false).header("api-key", "sk-test"))
        .await
        .unwrap();
    assert_eq!(right_way.status, 200);
    let requests = mock.settled(2).await;
    assert_eq!(requests[0].credential.presented_by, [Presentation::Bearer]);
    assert_eq!(
        requests[1].credential.presented_by,
        [Presentation::Header("api-key".into())]
    );
}

#[tokio::test]
async fn a_server_without_a_credential_takes_a_request_that_presents_none() {
    let mock = start().await;
    let reply = mock
        .client()
        .send(responses_request("hi", false))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert_eq!(
        mock.settled(1).await[0].credential.verdict,
        Verdict::NotChecked
    );
}

#[tokio::test]
async fn the_credential_can_be_rotated_while_the_server_runs() {
    let mock = MockLlm::start(MockConfig::default().with_credential("old"))
        .await
        .unwrap();
    assert_eq!(
        mock.client()
            .send(responses_request("x", false).bearer("old"))
            .await
            .unwrap()
            .status,
        200
    );
    mock.set_credential(Some("new"));
    assert_eq!(
        mock.client()
            .send(responses_request("x", false).bearer("old"))
            .await
            .unwrap()
            .status,
        401
    );
    assert_eq!(
        mock.client()
            .send(responses_request("x", false).bearer("new"))
            .await
            .unwrap()
            .status,
        200
    );
    let rotated = mock
        .client()
        .send(
            ClientRequest::new(http::Method::PUT, "/_mock/credential")
                .header("content-type", "application/json")
                .with_body(br#"{"credential": "newer", "accept": ["x-api-key"]}"#),
        )
        .await
        .unwrap();
    assert_eq!(rotated.status, 200);
    assert_eq!(
        mock.client()
            .send(responses_request("x", false).bearer("newer"))
            .await
            .unwrap()
            .status,
        401
    );
    assert_eq!(
        mock.client()
            .send(responses_request("x", false).x_api_key("newer"))
            .await
            .unwrap()
            .status,
        200
    );
}

#[tokio::test]
async fn what_was_received_is_remembered_with_every_secret_masked() {
    let config = MockConfig::default()
        .with_credential("sk-secret-key")
        .with_secret_value("gw-secret");
    let mock = MockLlm::start(config).await.unwrap();
    let request = ClientRequest::post_json(
        "/v1/messages?api-version=2025-01-01&token=gw-secret",
        &json!({ "model": "mock-large", "max_tokens": 8, "messages": [{ "role": "user", "content": "keep gw-secret out" }] }),
    )
    .bearer("sk-secret-key")
    .header("x-gateway-key", "gw-secret")
    .header("x-route", "team-a")
    .header("anthropic-version", "2023-06-01");
    mock.client().send(request).await.unwrap();
    let seen = &mock.settled(1).await[0];
    assert_eq!(seen.method, "POST");
    assert_eq!(seen.path, "/v1/messages");
    assert_eq!(seen.model.as_deref(), Some("mock-large"));
    assert_eq!(seen.format, Some(Format::Messages));
    assert_eq!(seen.header("x-route"), Some("team-a"));
    assert_eq!(seen.header("anthropic-version"), Some("2023-06-01"));
    assert_eq!(
        seen.header("x-gateway-key"),
        Some(fingerprint("gw-secret").as_str())
    );
    assert_eq!(
        seen.header("Authorization"),
        Some(format!("Bearer {}", fingerprint("sk-secret-key")).as_str())
    );
    assert_eq!(
        seen.query.as_deref(),
        Some(format!("api-version=2025-01-01&token={}", fingerprint("gw-secret")).as_str())
    );
    let whole = serde_json::to_string(seen).unwrap();
    assert!(
        !whole.contains("sk-secret-key") && !whole.contains("gw-secret"),
        "{whole}"
    );
    assert_eq!(
        seen.body["messages"][0]["content"],
        format!("keep {} out", fingerprint("gw-secret"))
    );
}

#[tokio::test]
async fn the_models_are_listed_in_the_shape_of_the_format_that_asks() {
    let mock = start().await;
    let responses = mock
        .client()
        .send(ClientRequest::get("/v1/models"))
        .await
        .unwrap()
        .json();
    assert_eq!(responses["object"], "list");
    assert_eq!(responses["data"][1]["id"], "mock-large");
    let messages = mock
        .client()
        .send(ClientRequest::get("/v1/models").messages_version())
        .await
        .unwrap()
        .json();
    assert_eq!(messages["data"][1]["type"], "model");
    assert_eq!(messages["has_more"], false);
    mock.set_models(vec![MockModel::named("only-one")]);
    let again = mock
        .client()
        .send(ClientRequest::get("/models"))
        .await
        .unwrap()
        .json();
    assert_eq!(again["data"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn any_base_address_in_front_of_the_paths_works_and_an_unknown_path_is_not_found_in_the_shape_of_its_format()
 {
    let mock = start().await;
    let reply = mock
        .client()
        .send(ClientRequest::post_json(
            "/gateway/openai/responses",
            &json!({ "input": "x" }),
        ))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    let unknown = mock
        .client()
        .send(ClientRequest::get("/v1/files"))
        .await
        .unwrap();
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["error"]["code"], "model_not_found");
    let unknown = mock
        .client()
        .send(ClientRequest::get("/v1/messages/batches").messages_version())
        .await
        .unwrap();
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.json()["error"]["type"], "not_found_error");
    let wrong_method = mock
        .client()
        .send(ClientRequest::get("/v1/responses"))
        .await
        .unwrap();
    assert_eq!(wrong_method.status, 404);
}

#[tokio::test]
async fn a_body_that_is_no_json_is_an_invalid_request() {
    let mock = start().await;
    let request = ClientRequest::new(http::Method::POST, "/v1/responses").with_body(b"not json");
    let reply = mock.client().send(request).await.unwrap();
    assert_eq!(reply.status, 400);
    assert_eq!(
        mock.settled(1).await[0].body,
        json!("not json"),
        "the text is kept when it is no JSON"
    );
}

#[tokio::test]
async fn tokens_can_be_counted_and_a_conversation_compacted() {
    let mock = start().await;
    let counted = mock
        .client()
        .send(
            ClientRequest::post_json(
                "/v1/messages/count_tokens",
                &json!({ "model": "mock-large", "messages": [{ "role": "user", "content": "0123456789abcdef" }] }),
            )
            .messages_version(),
        )
        .await
        .unwrap()
        .json();
    assert!(counted["input_tokens"].as_i64().unwrap() >= 4);
    assert!(counted.get("usage").is_none());
    let compacted = mock
        .client()
        .send(ClientRequest::post_json(
            "/v1/responses/compact",
            &json!({ "model": "mock-large", "input": [
                { "type": "message", "role": "user", "content": "keep me" },
                { "type": "message", "role": "assistant", "content": [{ "type": "output_text", "text": "dropped" }] }
            ] }),
        ))
        .await
        .unwrap()
        .json();
    assert_eq!(compacted["object"], "response.compaction");
    assert_eq!(compacted["output"][0]["content"], "keep me");
    assert_eq!(compacted["output"][1]["type"], "compaction");
    assert!(compacted["usage"]["total_tokens"].as_i64().unwrap() > 0);
}

#[tokio::test]
async fn the_same_script_gives_the_same_bytes_on_two_servers() {
    let script = "#!mock\nthink hmm\nsay Hello, world\ntool t {\"a\":[1,2]}\nusage 50 20 cached=10";
    let mut bodies = Vec::new();
    for _ in 0..2 {
        let mock = start().await;
        let mut both = Vec::new();
        for request in [
            responses_request(script, true),
            messages_request(script, true),
            responses_request(script, false),
        ] {
            both.push(mock.client().send(request).await.unwrap().body());
        }
        bodies.push(both);
    }
    assert_eq!(bodies[0], bodies[1]);
    assert!(!bodies[0][0].is_empty());
}

#[tokio::test]
async fn the_control_endpoint_reads_what_was_received_and_sets_responses_ahead() {
    let mock = start().await;
    let client = mock.client();
    let health = client
        .send(ClientRequest::get("/_mock/health"))
        .await
        .unwrap()
        .json();
    assert_eq!(health, json!({ "ok": true, "requests": 0 }));
    let queued = client
        .send(
            ClientRequest::post_json(
                "/_mock/queue",
                &json!({ "script": "say one\nthen\nsay two", "model": "mock-*", "format": "responses" }),
            ),
        )
        .await
        .unwrap();
    assert_eq!(queued.json(), json!({ "queued": 2 }));
    let listed = client
        .send(ClientRequest::get("/_mock/queue"))
        .await
        .unwrap()
        .json();
    assert_eq!(listed["queue"].as_array().unwrap().len(), 2);
    let first = client
        .send(responses_request("x", false))
        .await
        .unwrap()
        .json();
    assert_eq!(first["output"][0]["content"][0]["text"], "one");
    let seen = client
        .send(ClientRequest::get("/_mock/requests"))
        .await
        .unwrap()
        .json();
    assert_eq!(seen.as_array().unwrap().len(), 1);
    assert_eq!(seen[0]["path"], "/v1/responses");
    assert_eq!(seen[0]["credential"]["verdict"], "not_checked");
    let after = client
        .send(ClientRequest::get("/_mock/requests?since=1"))
        .await
        .unwrap()
        .json();
    assert_eq!(after, json!([]));
    let cleared = client
        .send(ClientRequest::new(http::Method::DELETE, "/_mock/requests"))
        .await
        .unwrap()
        .json();
    assert_eq!(cleared, json!({ "cleared": 1 }));
    let bad = client
        .send(ClientRequest::post_json(
            "/_mock/queue",
            &json!({ "script": "oops" }),
        ))
        .await
        .unwrap();
    assert_eq!(bad.status, 400);
    assert!(bad.json()["error"].as_str().unwrap().contains("line 1"));
    let reset = client
        .send(ClientRequest::post_json("/_mock/reset", &json!({})))
        .await
        .unwrap();
    assert_eq!(reset.status, 200);
    let again = client
        .send(ClientRequest::get("/_mock/health"))
        .await
        .unwrap()
        .json();
    assert_eq!(again["requests"], 0);
    assert_eq!(
        client
            .send(ClientRequest::get("/_mock/nothing"))
            .await
            .unwrap()
            .status,
        404
    );
}

#[tokio::test]
async fn the_control_endpoint_sets_the_default_script_and_the_models() {
    let mock = start().await;
    let client = mock.client();
    let set = client
        .send(
            ClientRequest::new(http::Method::PUT, "/_mock/default")
                .with_body(br#"{"script":"say by default"}"#),
        )
        .await
        .unwrap();
    assert_eq!(set.status, 200);
    let reply = client
        .send(messages_request("x", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["content"][0]["text"], "by default");
    client
        .send(ClientRequest::new(http::Method::DELETE, "/_mock/default"))
        .await
        .unwrap();
    let reply = client
        .send(messages_request("x", false))
        .await
        .unwrap()
        .json();
    assert_eq!(reply["content"][0]["text"], "You said: x");
    let models = client
        .send(ClientRequest::new(http::Method::PUT, "/_mock/models").with_body(
            br#"[{"id":"vendor/m:free","display_name":"Vendor: M","context_length":1000,"pricing":{"prompt":"0","completion":"0"}}]"#,
        ))
        .await
        .unwrap();
    assert_eq!(models.status, 200);
    let list = client
        .send(ClientRequest::get("/v1/models"))
        .await
        .unwrap()
        .json();
    assert_eq!(list["data"][0]["name"], "Vendor: M");
    assert_eq!(list["data"][0]["pricing"]["prompt"], "0");
}

#[tokio::test]
async fn a_message_with_tool_results_does_not_drive_the_reply_when_the_harness_adds_a_line_to_it() {
    let mock = start().await;
    let script = "#!mock\ntool t\nthen\nsay finished";
    let request = ClientRequest::post_json(
        "/v1/messages",
        &json!({ "model": "mock-large", "max_tokens": 8, "messages": [
            { "role": "user", "content": [{ "type": "text", "text": script }] },
            { "role": "assistant", "content": [{ "type": "tool_use", "id": "t1", "name": "t", "input": {} }] },
            { "role": "user", "content": [
                { "type": "tool_result", "tool_use_id": "t1", "content": "r" },
                { "type": "text", "text": "<system-reminder>keep going</system-reminder>" }] }
        ] }),
    )
    .messages_version();
    let reply = mock.client().send(request).await.unwrap().json();
    assert_eq!(reply["content"][0]["text"], "finished");
}

#[tokio::test]
async fn the_start_of_a_server_on_a_taken_address_fails_with_the_address() {
    let mock = start().await;
    let error = MockLlm::start_on(mock.addr(), MockConfig::default())
        .await
        .unwrap_err();
    assert!(error.to_string().contains(&mock.addr().to_string()));
}
