//! The forwarder, through a connector on the in-memory hub with the scripted controller, and a scripted model server that wants a key the harness never has: a turn of the scripted adapter calls the model through it; a call outside the lease, after it, over its allowance or on a path the protocol does not let through is refused in the protocol's own shape; a stream is passed on while it arrives, and an upstream's error or a cut stream reaches the harness as it is.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::model::{
    MESSAGES, MODEL, ProviderRig, RESPONSES, files_holding, messages_request, responses_request,
};
use orch_link::commands::Progress;
use orch_testkit::doubles::model::{Outcome, Verdict};
use orch_types::new_id;
use serde_json::json;

/// What the model is asked, with a script for the scripted server after it.
fn scripted(text: &str, directives: &[&str]) -> String {
    format!("{text}\n#!mock\n{}\n", directives.join("\n"))
}

#[tokio::test]
async fn an_upstream_error_cannot_echo_the_providers_key_in_the_harness_response() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    p.mock
        .queue(&format!(
            "header x-echo {}\nerror authentication The gateway rejected {}",
            p.key, p.key
        ))
        .unwrap();
    let reply = p
        .harness(p.thread)
        .post("/v1/responses", &responses_request(MODEL, "hello"))
        .await;
    assert_eq!(reply.status, 401);
    assert!(reply.text().contains("The gateway rejected"));
    assert!(!reply.text().contains(&p.key));
    assert!(
        reply
            .text()
            .contains(orch_plugin_contract::secret::REDACTED)
    );
    let echo = reply
        .header("x-echo")
        .expect("the upstream returned its header");
    assert!(!echo.contains(&p.key));
    assert!(echo.contains(orch_plugin_contract::secret::REDACTED));
}

#[tokio::test]
async fn an_encoded_upstream_response_is_refused_before_it_can_hide_a_credential() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    p.mock
        .queue("header content-encoding gzip\nsay hello")
        .unwrap();
    let reply = p
        .harness(p.thread)
        .post("/v1/responses", &responses_request(MODEL, "hello"))
        .await;
    assert_eq!(reply.status, 502);
    assert_eq!(reply.json()["error"]["code"], "upstream_unreachable");
    assert!(!reply.text().contains(&p.key));
}

#[tokio::test]
async fn a_turn_of_the_scripted_adapter_on_the_forwarded_path_calls_the_model_through_the_forwarder_which_adds_the_key()
 {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.start_harness().await;
    p.open_forwarded_session(RESPONSES, MODEL).await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    p.start_turn_for("hello", MODEL).await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;

    // The model's answer is what the turn said.
    assert_eq!(p.rig.messages(p.thread, p.segment), ["You said: hello"]);
    let calls = p.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].status, 200);

    // The server was asked once, with the key, and the key was the only credential it saw.
    let requests = p.model_requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].path.ends_with("/responses"),
        "{}",
        requests[0].path
    );
    assert_eq!(requests[0].credential.verdict, Verdict::Accepted);
    assert_eq!(requests[0].credential.presented_by.len(), 1);
    assert_eq!(
        requests[0].headers["authorization"],
        format!(
            "Bearer {}",
            orch_testkit::doubles::model::fingerprint(&p.key)
        ),
        "the key arrived, masked by the server as every secret is"
    );

    // The harness never had the key: not in what the connector gave the adapter, which holds the token of
    // the listener and nothing else.
    let given = p.given.lock().unwrap().join("\n");
    assert!(
        given.contains("\"token\""),
        "the adapter was given a token: {given}"
    );
    assert!(!given.contains(&p.key), "the key reached the adapter");

    // What it used was reported, for the ledger, and counted under the lease.
    let usage = p.wait_provider_event("lease.usage", 1).await;
    assert_eq!(usage[0].body["lease_id"], json!(lease));
    assert_eq!(usage[0].body["model"], MODEL);
    assert!(usage[0].body["usage"]["total_tokens"].as_i64().unwrap() > 0);
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 1);
    assert_eq!(
        closed["usage"]["total_tokens"],
        usage[0].body["usage"]["total_tokens"]
    );
}

#[tokio::test]
async fn the_same_turn_on_the_messages_format_is_forwarded_the_same_way() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.start_harness().await;
    p.open_forwarded_session(MESSAGES, MODEL).await;
    p.open_lease(p.thread, &[MODEL], Some(MESSAGES), None).await;
    p.start_turn_for("hello", MODEL).await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    assert_eq!(p.rig.messages(p.thread, p.segment), ["You said: hello"]);
    let requests = p.model_requests();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].path.ends_with("/messages"),
        "{}",
        requests[0].path
    );
    assert_eq!(requests[0].credential.verdict, Verdict::Accepted);
}

#[tokio::test]
async fn a_call_for_a_model_the_lease_does_not_name_is_refused_in_the_protocols_shape_and_the_system_is_told()
 {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let lease = p.open_lease(p.thread, &["mock-small"], None, None).await;
    let harness = p.harness(p.thread);

    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.json()["error"]["code"], "model_not_allowed");
    assert!(reply.json()["error"]["message"].is_string());

    let reply = harness
        .client()
        .send(
            harness
                .request("POST", "/v1/messages", Some(&messages_request(MODEL, "hi")))
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 403);
    assert_eq!(reply.json()["type"], "error");
    assert_eq!(reply.json()["error"]["type"], "permission_error");
    assert_eq!(reply.json()["error"]["code"], "model_not_allowed");

    let reply = harness
        .post("/v1/responses", &json!({ "input": "no model named" }))
        .await;
    assert_eq!(
        reply.status, 403,
        "a request that names no model is refused too"
    );

    let denied = p.wait_provider_event("lease.denied", 3).await;
    assert_eq!(denied[0].body["lease_id"], json!(lease));
    assert_eq!(denied[0].body["model"], MODEL);
    assert_eq!(denied[0].body["reason"], "model_not_allowed");
    assert!(
        p.model_requests().is_empty(),
        "nothing went upstream: {:?}",
        p.model_requests()
    );
}

#[tokio::test]
async fn a_model_is_compared_as_the_string_it_is() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    for variant in ["Mock-Large", "mock-large ", "mock-large:free", "mock"] {
        let reply = harness
            .post("/v1/responses", &responses_request(variant, "hi"))
            .await;
        assert_eq!(reply.status, 403, "{variant:?}");
    }
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn a_call_after_its_lease_is_closed_or_with_none_ever_opened_is_refused() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let harness = p.harness(p.thread);
    let before = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(before.status, 403, "no lease is open yet");
    assert_eq!(before.json()["error"]["code"], "lease_closed");

    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let during = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(during.status, 200);
    p.close_lease(lease).await;
    let after = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(after.status, 403);
    assert_eq!(after.json()["error"]["code"], "lease_closed");
    assert_eq!(
        p.model_requests().len(),
        1,
        "only the call under the open lease went upstream"
    );

    let messages = harness
        .client()
        .send(
            harness
                .request("POST", "/v1/messages", Some(&messages_request(MODEL, "hi")))
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(messages.status, 403);
    assert_eq!(
        messages.json()["error"]["code"],
        "lease_closed",
        "the refusal is in the shape of the format the call was in"
    );
}

#[tokio::test]
async fn a_lease_of_another_thread_is_no_leave_for_this_one() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(new_id(), &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.json()["error"]["code"], "lease_closed");
}

#[tokio::test]
async fn the_next_call_is_refused_once_the_allowance_in_tokens_is_reached_and_the_system_is_told() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    let lease = p
        .open_lease(
            p.thread,
            &[MODEL],
            Some(RESPONSES),
            Some(json!({ "tokens": 150 })),
        )
        .await;
    let harness = p.harness(p.thread);
    let ask = || responses_request(MODEL, &scripted("go", &["say ok", "usage 100 60"]));
    let first = harness.post("/v1/responses", &ask()).await;
    assert_eq!(first.status, 200);
    let second = harness.post("/v1/responses", &ask()).await;
    assert_eq!(
        second.status, 403,
        "160 tokens used of 150 allowed: no room for another"
    );
    assert_eq!(second.json()["error"]["code"], "cap_reached");
    assert_eq!(p.model_requests().len(), 1);
    let denied = p.wait_provider_event("lease.denied", 1).await;
    assert_eq!(denied[0].body["reason"], "cap_reached");
    assert_eq!(denied[0].body["lease_id"], json!(lease));
}

#[tokio::test]
async fn an_allowance_in_credits_is_held_at_the_rate_it_came_with_while_the_link_to_the_system_is_down()
 {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    // 1 000 input tokens at 2 credits for a million and 100 of output at 10: 0.003 credits a request.
    p.open_lease(
        p.thread,
        &[MODEL],
        Some(RESPONSES),
        Some(json!({ "credits": { "amount": "0.005", "rate": {
            "input_per_mtok": "2", "cached_input_per_mtok": "0.2", "output_per_mtok": "10" } } })),
    )
    .await;
    // The system is out of reach: the forwarder holds the cap by itself.
    p.rig.hub.faults().sever_now();
    p.rig.hub.faults().refuse_connections(1000);
    let harness = p.harness(p.thread);
    let ask = || responses_request(MODEL, &scripted("go", &["say ok", "usage 1000 100"]));
    assert_eq!(harness.post("/v1/responses", &ask()).await.status, 200);
    assert_eq!(
        harness.post("/v1/responses", &ask()).await.status,
        200,
        "0.003 of 0.005 is room"
    );
    let third = harness.post("/v1/responses", &ask()).await;
    assert_eq!(third.status, 403, "0.006 of 0.005 is none");
    assert_eq!(third.json()["error"]["code"], "cap_reached");
}

#[tokio::test]
async fn a_path_the_protocol_does_not_let_through_is_refused_whatever_the_model() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    for (method, rest) in [
        ("POST", "/v1/chat/completions/stored"),
        ("POST", "/v1/responses/resp_1/cancel"),
        ("GET", "/v1/responses/resp_1"),
        ("DELETE", "/v1/responses/resp_1"),
        ("POST", "/v1/files"),
        ("POST", "/v1/responses/"),
        ("POST", "/v1/v1/responses"),
        ("POST", "/v1/messages/batches"),
    ] {
        let body = responses_request(MODEL, "hi");
        let reply = harness
            .client()
            .send(harness.request(method, rest, Some(&body)))
            .await
            .unwrap();
        assert_eq!(reply.status, 404, "{method} {rest}");
        let code = reply.json()["error"]["code"].clone();
        assert!(
            code == "path_not_allowed" || reply.text() == "no such address",
            "{method} {rest}: {}",
            reply.text()
        );
    }
    assert!(p.model_requests().is_empty());
}

#[tokio::test]
async fn a_harness_that_lists_models_is_told_the_leases_models_and_no_others() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    let listed = harness.get("/v1/models").await;
    assert_eq!(listed.status, 200);
    let names: Vec<_> = listed.json()["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|model| model["id"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        names,
        [MODEL],
        "the server lists mock-small too; the lease does not"
    );
    assert!(
        p.model_requests().is_empty(),
        "the list is the lease's and is not asked of the upstream"
    );
}

#[tokio::test]
async fn a_stream_is_passed_on_while_it_arrives() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    let harness = p.harness(p.thread);
    // Eight characters at a time, a pause of 400 ms before each: the answer takes seconds to finish.
    let text = scripted(
        "go",
        &[
            "chunk 8",
            "delay 400",
            "say one two three four five six seven eight nine ten",
        ],
    );
    let mut body = responses_request(MODEL, &text);
    body["stream"] = json!(true);
    let started = tokio::time::Instant::now();
    let mut reply = harness
        .client()
        .open(harness.request("POST", "/v1/responses", Some(&body)))
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    let mut first = None;
    let mut pieces = 0;
    while let Some(piece) = reply.next_chunk().await.unwrap() {
        if first.is_none() && String::from_utf8_lossy(&piece).contains("response.output_text.delta")
        {
            first = Some(started.elapsed());
        }
        pieces += 1;
    }
    let total = started.elapsed();
    let first = first.expect("a text delta arrived");
    assert!(
        first + Duration::from_millis(800) < total,
        "the first text reached the harness at {first:?}, the answer ended at {total:?}: it was held back"
    );
    assert!(pieces > 3, "the answer arrived in {pieces} pieces");
}

#[tokio::test]
async fn an_error_of_the_upstream_reaches_the_harness_as_it_is() {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let rate_limited = harness
        .post(
            "/v1/responses",
            &responses_request(MODEL, &scripted("go", &["error rate_limit retry-after=7"])),
        )
        .await;
    assert_eq!(rate_limited.status, 429);
    assert_eq!(rate_limited.header("retry-after"), Some("7"));
    assert_eq!(rate_limited.json()["error"]["type"], "rate_limit_error");

    let overloaded = harness
        .client()
        .send(
            harness
                .request(
                    "POST",
                    "/v1/messages",
                    Some(&messages_request(
                        MODEL,
                        &scripted("go", &["error overloaded"]),
                    )),
                )
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(overloaded.status, 529);
    assert_eq!(overloaded.json()["error"]["type"], "overloaded_error");
}

#[tokio::test]
async fn a_stream_the_upstream_cuts_reaches_the_harness_as_a_cut_stream_and_what_was_used_before_the_cut_is_counted()
 {
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    let lease = p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let mut body = messages_request(
        MODEL,
        &scripted("go", &["usage 40 3", "say a b c d e f g h", "cut"]),
    );
    body["stream"] = json!(true);
    let reply = harness
        .client()
        .send(
            harness
                .request("POST", "/v1/messages", Some(&body))
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 200);
    assert!(
        reply.broke.is_some(),
        "the harness must see the break: {:?}",
        reply.text()
    );
    assert!(!reply.text().contains("message_stop"));
    let usage = p.wait_provider_event("lease.usage", 1).await;
    assert_eq!(
        usage[0].body["usage"]["input_tokens"], 40,
        "the prompt the upstream counted before it cut is counted"
    );
    let closed = p.close_lease(lease).await;
    assert_eq!(closed["requests"], 1);
    let records = p.mock.settled(1).await;
    assert!(
        matches!(
            records[0].outcome,
            Outcome::Cut | Outcome::ClientGone | Outcome::Error
        ),
        "{:?}",
        records[0].outcome
    );
}

#[tokio::test]
async fn a_call_to_an_upstream_nobody_listens_at_is_a_bad_gateway_the_harness_is_told_to_try_again()
{
    let dead = {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
        listener.local_addr().unwrap().port()
    };
    let base = format!("http://127.0.0.1:{dead}/v1");
    let listed = base.clone();
    let p = ProviderRig::with(move |rig, _| rig.config.provider.upstreams.push(listed)).await;
    let progress = p.start_provider(&[(RESPONSES, base, json!({}))]).await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let reply = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    assert_eq!(reply.status, 502);
    assert_eq!(reply.json()["error"]["code"], "upstream_unreachable");
    assert!(
        reply.header("retry-after").is_some(),
        "a harness is told when to try again"
    );
}

#[tokio::test]
async fn a_key_is_never_written_to_a_file_outside_the_sealed_store_nor_to_a_log() {
    let log = common::model::capture_log();
    tracing::info!("the log is captured");
    let p = ProviderRig::new().await;
    p.start_both_providers().await;
    p.start_harness().await;
    p.open_forwarded_session(RESPONSES, MODEL).await;
    let lease = p
        .open_lease(p.thread, &[MODEL], Some(RESPONSES), None)
        .await;
    p.start_turn_for("hello", MODEL).await;
    p.rig
        .wait_for(p.thread, p.segment, "turn.completed", 1)
        .await;
    let harness = p.harness(p.thread);
    // A refusal and a failure too, since errors are where a key is most likely to be printed.
    let _ = harness
        .post("/v1/responses", &responses_request("nope", "hi"))
        .await;
    p.mock
        .set_credential(Some("another-key-the-provider-does-not-have"));
    let _ = harness
        .post("/v1/responses", &responses_request(MODEL, "hi"))
        .await;
    p.close_lease(lease).await;
    p.wait_provider_event("lease.usage", 1).await;
    tokio::time::sleep(Duration::from_millis(300)).await;

    // On disk the key is nowhere in clear, the sealed store included. (The search finds a key where there is
    // one: a file with the key in it, planted for the check, is found.)
    let data = p.rig.data.path().to_owned();
    let found = files_holding(&data, &p.key);
    assert!(found.is_empty(), "the key is in {found:?}");
    let planted = data.join("planted-for-the-search.txt");
    std::fs::write(&planted, format!("a line with {} in it", p.key)).unwrap();
    assert_eq!(files_holding(&data, &p.key), std::slice::from_ref(&planted));
    std::fs::remove_file(&planted).unwrap();
    assert!(
        data.join("secrets").is_dir(),
        "the provider is kept in the sealed store"
    );
    // In what the system was told.
    for stream in p.rig.controller.reports() {
        assert!(!stream.contains(&p.key));
    }
    let events: String = p
        .provider_events()
        .iter()
        .map(|event| event.body.to_string())
        .collect();
    assert!(!events.contains(&p.key), "an event holds the key");
    // In what the scripted adapter, standing for a harness, was given.
    assert!(!p.given.lock().unwrap().join("").contains(&p.key));
    // In the log, at the finest level.
    let logged = log.contents();
    assert!(
        logged.contains("the log is captured"),
        "the log was captured"
    );
    assert!(!logged.contains(&p.key), "the log holds the key");
    // Nor is it in the process's own environment or arguments.
    assert!(!std::env::args().any(|argument| argument.contains(&p.key)));
    assert!(!std::env::vars().any(|(_, value)| value.contains(&p.key)));
}

#[tokio::test]
async fn the_scripted_server_is_asked_in_the_way_each_endpoint_says_with_its_own_headers_and_query()
{
    let p = ProviderRig::new().await;
    let id = new_id();
    let secret = "gateway-secret-value";
    let body = json!({
        "provider_id": p.rig.provider,
        "protocols": [RESPONSES, MESSAGES],
        "upstreams": [
            { "protocol": RESPONSES, "base_url": p.mock.responses_base_url(), "options": {
                "headers": { "x-route": "team-a", "x-forwarded-host": "manager.example" }, "query": { "api-version": "2025-01-01" },
                "secret_headers": ["X-Gateway-Key"] } },
            { "protocol": MESSAGES, "base_url": p.mock.messages_base_url(), "options": { "auth": "x-api-key" } }
        ],
        "credential": p.sealed_key_for(id, &p.key),
        "secrets": { "x-gateway-key": p.sealed_key_for(id, secret) }
    });
    let body: orch_link_contract::commands::ProviderStart = serde_json::from_value(body).unwrap();
    let progress = p.command_with_id(id, &p.provider_key(), &body).await;
    assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let mut responses_call = harness.request(
        "POST",
        "/v1/responses?api%2Dversion=wrong&prompt-cache-key=ok",
        Some(&responses_request(MODEL, "hi")),
    );
    responses_call = responses_call
        .header("x-route", "from-the-harness")
        .header("cookie", "session=1")
        .header("x-api-key", "harness-key")
        .header("x-forwarded-host", "attacker.example")
        .header("forwarded", "host=attacker.example")
        .header("connection", "x-internal-hop")
        .header("x-internal-hop", "unwanted")
        .header("x-original-url", "/attacker")
        .header("x-rewrite-url", "/attacker")
        .header("x-codex-turn-metadata", "{}");
    assert_eq!(
        harness.client().send(responses_call).await.unwrap().status,
        200
    );
    let messages_call = harness
        .request("POST", "/v1/messages", Some(&messages_request(MODEL, "hi")))
        .messages_version()
        .header("x-forwarded-host", "attacker.example")
        .header("anthropic-beta", "tools-2024");
    assert_eq!(
        harness.client().send(messages_call).await.unwrap().status,
        200
    );

    let requests = p.model_requests();
    assert_eq!(requests.len(), 2);
    let (responses, messages) = (&requests[0], &requests[1]);
    // The Responses endpoint: bearer, its own static header over the harness's, its secret header, its query.
    assert_eq!(
        responses.credential.presented_by,
        [orch_testkit::doubles::model::Presentation::Bearer]
    );
    assert_eq!(responses.headers["x-route"], "team-a");
    assert_eq!(responses.headers["x-forwarded-host"], "manager.example");
    assert!(!responses.headers.contains_key("forwarded"));
    assert!(!responses.headers.contains_key("x-internal-hop"));
    assert!(!responses.headers.contains_key("x-original-url"));
    assert!(!responses.headers.contains_key("x-rewrite-url"));
    assert_eq!(
        responses.headers["x-gateway-key"],
        orch_testkit::doubles::model::fingerprint(secret)
    );
    assert_eq!(
        responses.query.as_deref(),
        Some("prompt-cache-key=ok&api-version=2025-01-01")
    );
    assert!(
        !responses.headers.contains_key("cookie"),
        "the harness's cookie goes no further"
    );
    assert!(
        !responses.headers.contains_key("x-api-key"),
        "the harness's own key goes no further"
    );
    assert_eq!(
        responses.headers["x-codex-turn-metadata"], "{}",
        "what is not named is passed on"
    );
    // The Messages endpoint: the key in x-api-key, as its options say, and the harness's headers kept.
    assert_eq!(
        messages.credential.presented_by,
        [orch_testkit::doubles::model::Presentation::XApiKey]
    );
    assert_eq!(messages.headers["anthropic-beta"], "tools-2024");
    assert!(!messages.headers.contains_key("x-forwarded-host"));
    assert!(
        !messages.headers.contains_key("x-gateway-key"),
        "a secret of one endpoint is for that endpoint"
    );
    assert!(!messages.headers.contains_key("x-route"));
    assert_eq!(messages.query, None);
    // Neither saw what the harness presented to the listener.
    let all = serde_json::to_string(
        &requests
            .iter()
            .map(|r| r.headers.clone())
            .collect::<Vec<_>>(),
    )
    .unwrap();
    assert!(!all.contains("harness-key"));
}

#[tokio::test]
async fn a_call_in_a_protocol_no_endpoint_speaks_is_refused_when_no_translator_is_registered() {
    let p = ProviderRig::new().await;
    p.start_responses_provider().await;
    p.open_lease(p.thread, &[MODEL], None, None).await;
    let harness = p.harness(p.thread);
    let reply = harness
        .client()
        .send(
            harness
                .request("POST", "/v1/messages", Some(&messages_request(MODEL, "hi")))
                .messages_version(),
        )
        .await
        .unwrap();
    assert_eq!(reply.status, 404);
    assert_eq!(reply.json()["error"]["code"], "protocol_not_served");
    assert!(p.model_requests().is_empty());
}

#[tokio::test]
async fn a_machine_the_owner_limits_to_listed_upstreams_refuses_to_start_a_provider_for_another() {
    let p = ProviderRig::with(|rig, _| rig.config.provider.upstreams.clear()).await;
    let progress = p
        .start_provider(&[(RESPONSES, p.mock.responses_base_url(), json!({}))])
        .await;
    assert!(
        matches!(progress, Progress::Failed(ref error) if error.code == "policy.upstream_not_allowed"),
        "{progress:?}"
    );
}

#[tokio::test]
async fn a_header_the_forwarder_sets_itself_cannot_be_an_option_of_an_endpoint() {
    let p = ProviderRig::with(|rig, _| {
        rig.config
            .provider
            .upstreams
            .push("https://acme.example/v1".to_owned());
    })
    .await;
    let progress = p
        .start_provider(&[(
            RESPONSES,
            p.mock.responses_base_url(),
            json!({ "headers": { "Host": "elsewhere" } }),
        )])
        .await;
    assert!(
        matches!(progress, Progress::Failed(ref error) if error.code == "unsupported"),
        "{progress:?}"
    );
    let unknown = p
        .start_provider(&[(
            RESPONSES,
            p.mock.responses_base_url(),
            json!({ "timeout": 5 }),
        )])
        .await;
    assert!(
        matches!(unknown, Progress::Failed(ref error) if error.code == "unsupported"),
        "{unknown:?}"
    );
    let alien = p
        .start_provider(&[("acme-chat", "https://acme.example/v1".to_owned(), json!({}))])
        .await;
    assert!(
        matches!(alien, Progress::Failed(ref error) if error.code == "unsupported"),
        "{alien:?}"
    );
}

#[tokio::test]
async fn the_key_is_presented_the_way_each_endpoint_says_on_each_format() {
    use orch_testkit::doubles::model::Presentation;
    let p = ProviderRig::new().await;
    let cases = [
        ("bearer", json!({}), Presentation::Bearer),
        ("bearer", json!({ "auth": "bearer" }), Presentation::Bearer),
        (
            "x-api-key",
            json!({ "auth": "x-api-key" }),
            Presentation::XApiKey,
        ),
        (
            "header",
            json!({ "auth": "header", "auth_header": "api-key" }),
            Presentation::Header("api-key".to_owned()),
        ),
    ];
    for protocol in [RESPONSES, MESSAGES] {
        for (name, options, wanted) in &cases {
            p.mock.set_accepted(std::slice::from_ref(wanted));
            let base = if protocol == RESPONSES {
                p.mock.responses_base_url()
            } else {
                p.mock.messages_base_url()
            };
            let progress = p.start_provider(&[(protocol, base, options.clone())]).await;
            assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}");
            let thread = new_id();
            p.open_lease(thread, &[MODEL], Some(protocol), None).await;
            let harness = p.harness(thread);
            let reply = if protocol == RESPONSES {
                harness
                    .post("/v1/responses", &responses_request(MODEL, "hi"))
                    .await
            } else {
                harness
                    .client()
                    .send(
                        harness
                            .request("POST", "/v1/messages", Some(&messages_request(MODEL, "hi")))
                            .messages_version(),
                    )
                    .await
                    .unwrap()
            };
            assert_eq!(
                reply.status,
                200,
                "{protocol} with {name}: {}",
                reply.text()
            );
            let seen = p.model_requests().pop().unwrap();
            assert_eq!(
                seen.credential.presented_by,
                std::slice::from_ref(wanted),
                "{protocol} with {name}"
            );
            assert_eq!(seen.credential.verdict, Verdict::Accepted);
        }
    }
}
