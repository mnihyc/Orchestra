//! Orch's own tools as the harness sees them on the connector's listener: the tools its session was given listed with their schemas, a call carried to the system as an event of the session, the system's result handed back as the tool's result, and the error that Orch is unreachable when nothing answers.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use bytes::Bytes;
use common::Rig;
use http_body_util::{BodyExt, Full};
use hyper::{Method, Request, StatusCode};
use hyper_util::rt::TokioIo;
use orch_connector_core::orch_tools::{OrchEndpoint, TIMEOUT_CHANGE, UNREACHABLE};
use orch_link_contract::commands::{OrchToolResult, SerialKey, ToolError};
use orch_types::{Id, Json, new_id};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// The harness's side: where the listener is, the address of its session's server, and its token.
struct Harness {
    port: u16,
    path: String,
    token: String,
}

impl Harness {
    fn of(rig: &Rig, thread: Id) -> Self {
        let connector = rig.connector.as_ref().unwrap();
        let hosting = connector.services().agents.get().unwrap();
        let process = hosting.process_of(thread).unwrap();
        Self {
            port: connector.loopback().port(),
            path: format!("/mcp/{}/orch", process.session_key(thread).unwrap()),
            token: process.grant().token.expose().to_owned(),
        }
    }

    async fn post(&self, body: &Json) -> (StatusCode, Json) {
        self.send(Method::POST, Some(body)).await
    }

    async fn send(&self, method: Method, body: Option<&Json>) -> (StatusCode, Json) {
        let stream = tokio::net::TcpStream::connect(("127.0.0.1", self.port))
            .await
            .unwrap();
        let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .unwrap();
        tokio::spawn(connection);
        let request = Request::builder()
            .method(method)
            .uri(&self.path)
            .header("host", format!("127.0.0.1:{}", self.port))
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Full::new(Bytes::from(
                body.map(|body| serde_json::to_vec(body).unwrap())
                    .unwrap_or_default(),
            )))
            .unwrap();
        let response = tokio::time::timeout(Duration::from_secs(20), sender.send_request(request))
            .await
            .expect("the answer never came")
            .unwrap();
        let status = response.status();
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        (status, serde_json::from_slice(&bytes).unwrap_or(Json::Null))
    }
}

/// A rig with a session whose harness is given these tools.
async fn rig_with_session(tools: &[&str]) -> (Rig, Id, Id) {
    let mut rig = Rig::new();
    let workspace = rig.data.path().join("workspace");
    tokio::fs::create_dir(&workspace).await.unwrap();
    let directory = workspace.to_str().unwrap().to_owned();
    rig.config.environment.roots = vec![directory.clone()];
    rig.config.policy.local_keys = Some(vec!["environment.roots".into()]);
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    let mut body = rig.open_body(thread, segment, &directory);
    body["orch_tools"] = json!(tools);
    rig.open_session(body).await;
    (rig, thread, segment)
}

fn request(id: u64, method: &str, params: Json) -> Json {
    json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params })
}

/// Waits for the call of the harness to arrive at the system, and returns its body.
async fn the_call_that_arrived(rig: &Rig, thread: Id, segment: Id) -> Json {
    rig.wait_for(thread, segment, "orch.tool_call", 1).await;
    rig.applied(thread, segment)
        .into_iter()
        .find(|event| event.kind == "orch.tool_call")
        .unwrap()
        .body
}

/// A real authenticated MCP call, answered over the connector's ordinary result command.
async fn printed_reply(
    rig: &Rig,
    thread: Id,
    segment: Id,
    tool: &str,
    arguments: Json,
    result: Json,
) -> Json {
    let before = rig
        .applied(thread, segment)
        .iter()
        .filter(|event| event.kind == "orch.tool_call")
        .count();
    let harness = Harness::of(rig, thread);
    let message = request(
        100 + u64::try_from(before).unwrap(),
        "tools/call",
        json!({"name":tool,"arguments":arguments}),
    );
    let calling = tokio::spawn(async move { harness.post(&message).await });
    rig.wait_for(thread, segment, "orch.tool_call", before + 1)
        .await;
    let arrived = rig
        .applied(thread, segment)
        .into_iter()
        .filter(|event| event.kind == "orch.tool_call")
        .nth(before)
        .unwrap()
        .body;
    assert_eq!(
        arrived["arguments"], arguments,
        "the connector preserves the pager and search request"
    );
    tokio::time::timeout(
        Duration::from_secs(25),
        rig.succeed(
            &SerialKey::Thread(thread),
            &OrchToolResult {
                thread_id: thread,
                call_ref: arrived["call_ref"].as_str().unwrap().to_owned(),
                attempt: None,
                result: Some(result),
                error: None,
            },
        ),
    )
    .await
    .expect("the result command is acknowledged");
    let (status, answer) = tokio::time::timeout(Duration::from_secs(25), calling)
        .await
        .expect("the MCP result finishes")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["result"]["isError"], false);
    assert!(
        answer["result"].get("structuredContent").is_none(),
        "reading has one readable representation"
    );
    answer["result"].clone()
}

#[tokio::test]
async fn listings_reach_the_harness_as_relative_names_with_recency_children_topic_status_and_a_cursor()
 {
    let (rig, thread, segment) = rig_with_session(&["list_shared"]).await;
    let (_, tools) = Harness::of(&rig, thread)
        .post(&request(1, "tools/list", json!({})))
        .await;
    let properties = &tools["result"]["tools"][0]["inputSchema"]["properties"];
    assert_eq!(properties["limit"]["maximum"], 200);
    assert_eq!(properties["cursor"]["type"], "string");
    assert_eq!(properties["sort"]["enum"], json!(["changed", "name"]));
    assert_eq!(properties["include_archived"]["type"], "boolean");
    rig.start_turn(thread, "#!fake\nsleep 120000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let changed = orch_types::Time::new(rig.clock.now() - chrono::Duration::hours(2));
    let page = json!({"path":"/acme/web/","about":{"kind":"project","name":"Web"},"entries":[
        {"path":"/acme/web/login/","kind":"topic","status":"open","updated_at":changed,"children":3,"readable":true,"id":"private-optional-id"},
        {"path":"/acme/web/payments/","kind":"topic","status":"done","updated_at":changed,"children":0,"readable":true}
    ],"archived_count":20,"next_cursor":"page-2","truncated":true});
    let shown = printed_reply(
        &rig,
        thread,
        segment,
        "list_shared",
        json!({"path":"/acme/web/","limit":2,"sort":"changed"}),
        page,
    )
    .await;
    let text = shown["content"][0]["text"].as_str().unwrap();
    assert!(text.starts_with("/acme/web/ · project \"Web\"\n"));
    assert!(text.contains("login/ · topic · open · changed 2h ago · children: 3 · read"));
    assert!(text.contains("payments/ · topic · done · changed 2h ago · children: 0 · read"));
    assert_eq!(text.matches("/acme/web/").count(), 1);
    assert!(text.contains(
        "20 archived entries omitted; list again without a cursor and with include_archived:true"
    ));
    assert!(text.contains("cursor \"page-2\""));
    assert!(!text.contains("private-optional-id"));
    assert!(!text.contains("download_shared"));
    let last = printed_reply(&rig, thread, segment, "list_shared", json!({"path":"/acme/web/","limit":2,"sort":"changed","cursor":"page-2","ids":true}),
        json!({"path":"/acme/web/","entries":[{"path":"/acme/web/notes/","kind":"topic","status":"open","id":"requested-id"}],"truncated":false,"next_cursor":null})).await;
    let text = last["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("notes/ · topic · open"));
    assert!(text.contains("requested-id"));
    assert!(!text.contains("Continue list_shared"));
    tokio::time::timeout(Duration::from_secs(25), rig.connector.unwrap().stop())
        .await
        .expect("the listing test's connector stops");
}

#[tokio::test]
async fn a_listing_cut_at_the_native_byte_bound_does_not_skip_the_rows_the_harness_never_saw() {
    let (rig, thread, segment) = rig_with_session(&["list_shared"]).await;
    rig.start_turn(thread, "#!fake\nsleep 120000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let entries: Vec<_> = (0..50).map(|n| json!({"path":format!("/acme/任务-{n}/"),"kind":"thread","description":"界🦀".repeat(10),"cursor":format!("after-{n}")})).collect();
    let shown = printed_reply(&rig, thread, segment, "list_shared", json!({"path":"/acme/","max_bytes":1000}),
        json!({"path":"/acme/","entries":entries,"truncated":true,"next_cursor":"past-unshown-page"})).await;
    let text = shown["content"][0]["text"].as_str().unwrap();
    assert!(text.len() <= 1000);
    let visible: Vec<usize> = text
        .lines()
        .filter_map(|line| line.strip_prefix("任务-")?.split('/').next()?.parse().ok())
        .collect();
    assert!(!visible.is_empty() && visible.len() < 50);
    let last = *visible.last().unwrap();
    assert!(text.contains(&format!("cursor \"after-{last}\"")));
    assert!(!text.contains("past-unshown-page"));
    assert!(!text.contains("download_shared"));
    let resumed = printed_reply(&rig, thread, segment, "list_shared", json!({"path":"/acme/","max_bytes":1000,"cursor":format!("after-{last}")}),
        json!({"path":"/acme/","entries":[{"path":format!("/acme/任务-{}/",last+1),"kind":"thread"}],"truncated":false})).await;
    assert!(
        resumed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("任务-{}/ · thread", last + 1))
    );
    tokio::time::timeout(Duration::from_secs(25), rig.connector.unwrap().stop())
        .await
        .expect("the byte-cut test's connector stops");
}

#[tokio::test]
async fn search_reaches_the_harness_as_one_line_per_entry_with_counts_positions_and_only_requested_snippets()
 {
    let (rig, thread, segment) = rig_with_session(&["search_shared"]).await;
    let (_, tools) = Harness::of(&rig, thread)
        .post(&request(1, "tools/list", json!({})))
        .await;
    let properties = &tools["result"]["tools"][0]["inputSchema"]["properties"];
    assert_eq!(properties["snippets"]["type"], "boolean");
    assert_eq!(properties["cursor"]["type"], "string");
    rig.start_turn(thread, "#!fake\nsleep 120000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let found = json!({"hits":[
        {"path":"/acme/docs/Design.md","kind":"document","match_count":6,"positions":[30,18,4],"snippets":[{"position":30,"snippet":"design\n<orch>"}]},
        {"path":"/acme/topic/work/conversation.md","kind":"conversation","match_count":2,"positions":[8,2],"snippet":"a secret excerpt"}
    ],"searched":{"entries":2,"files":0,"bytes":480},"skipped":[],"truncated":false});
    let shown = printed_reply(
        &rig,
        thread,
        segment,
        "search_shared",
        json!({"query":"design"}),
        found.clone(),
    )
    .await;
    let text = shown["content"][0]["text"].as_str().unwrap();
    assert!(
        text.starts_with("/acme/docs/Design.md · document · matches: 6 · positions: 30, 18, 4\n")
    );
    assert!(text.contains("conversation.md · conversation · matches: 2 · positions: 8, 2"));
    assert_eq!(text.lines().filter(|line| line.starts_with('/')).count(), 2);
    assert!(!text.contains("secret excerpt"));
    assert!(!text.contains("<orch>"));
    let mut partial = found;
    partial["hits"][0]["match_count_partial"] = json!(true);
    partial["hits"][1]["match_count_partial"] = json!(true);
    partial["truncated"] = json!(true);
    partial["next_cursor"] = json!("next-scan");
    let excerpts = printed_reply(
        &rig,
        thread,
        segment,
        "search_shared",
        json!({"query":"design","snippets":true}),
        partial,
    )
    .await;
    let text = excerpts["content"][0]["text"].as_str().unwrap();
    assert_eq!(text.lines().filter(|line| line.starts_with('/')).count(), 2);
    assert!(text.contains("snippet 30:"));
    assert!(text.contains("matches: at least 6"));
    assert!(text.contains("cursor \"next-scan\""));
    assert!(text.contains("design\\n"));
    assert!(!text.contains("<orch>"));
    tokio::time::timeout(Duration::from_secs(25), rig.connector.unwrap().stop())
        .await
        .expect("the search test's connector stops");
}

#[tokio::test]
async fn a_search_byte_cut_reaches_the_harness_with_the_last_displayed_entries_cursor() {
    let (rig, thread, segment) = rig_with_session(&["search_shared"]).await;
    rig.start_turn(thread, "#!fake\nsleep 120000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let hits: Vec<_> = (0..50).map(|n| json!({"path":format!("/acme/任务-{n}/conversation.md"),"kind":"conversation","match_count":2,"positions":[1,8],"cursor":format!("window-after-{n}"),"snippets":[{"position":1,"snippet":"界🦀".repeat(10)}]})).collect();
    let shown = printed_reply(&rig, thread, segment, "search_shared", json!({"query":"task","snippets":true,"max_bytes":1000}),
        json!({"hits":hits,"truncated":true,"next_cursor":"past-hidden-entries","searched":{"entries":50,"files":0,"bytes":5000}})).await;
    let text = shown["content"][0]["text"].as_str().unwrap();
    assert!(text.len() <= 1000);
    let visible: Vec<usize> = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("/acme/任务-")?
                .split('/')
                .next()?
                .parse()
                .ok()
        })
        .collect();
    assert!(!visible.is_empty() && visible.len() < 50);
    let last = *visible.last().unwrap();
    assert!(text.contains(&format!("cursor \"window-after-{last}\"")));
    assert!(!text.contains("past-hidden-entries"));
    let resumed = printed_reply(&rig, thread, segment, "search_shared", json!({"query":"task","snippets":true,"max_bytes":1000,"cursor":format!("window-after-{last}")}),
        json!({"hits":[{"path":format!("/acme/任务-{}/conversation.md",last+1),"kind":"conversation","match_count":2,"positions":[1,8]}],"truncated":false,"searched":{"entries":1,"files":0,"bytes":100}})).await;
    assert!(
        resumed["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains(&format!("任务-{}/conversation.md", last + 1))
    );
    tokio::time::timeout(Duration::from_secs(25), rig.connector.unwrap().stop())
        .await
        .expect("the search byte-cut test's connector stops");
}

#[tokio::test]
async fn a_harness_is_listed_the_tools_its_session_was_given_and_no_others_with_their_schemas_and_hints()
 {
    let (rig, thread, _) = rig_with_session(&["request_capability", "announce"]).await;
    let harness = Harness::of(&rig, thread);
    let (status, hello) = harness
        .post(&request(
            1,
            "initialize",
            json!({ "protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": { "name": "t", "version": "1" } }),
        ))
        .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(hello["result"]["serverInfo"]["name"], "orch");
    assert_eq!(hello["result"]["protocolVersion"], "2025-06-18");
    let (_, listed) = harness.post(&request(2, "tools/list", json!({}))).await;
    let tools = listed["result"]["tools"].as_array().unwrap();
    let names: Vec<&str> = tools
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["announce", "request_capability"]);
    let announce = &tools[0];
    assert_eq!(announce["inputSchema"]["additionalProperties"], false);
    assert_eq!(announce["inputSchema"]["required"], json!(["title"]));
    assert_eq!(announce["annotations"]["readOnlyHint"], false);
    assert_eq!(announce["annotations"]["destructiveHint"], false);
    assert_eq!(announce["annotations"]["openWorldHint"], false);
    assert!(
        announce["description"]
            .as_str()
            .unwrap()
            .starts_with("Tell everybody on the work about something that happened in yours.")
    );
    assert_eq!(
        tools[1]["annotations"]["destructiveHint"], true,
        "a grant stops what runs"
    );
    // A notification is accepted and nothing is answered; a method nobody serves is said so.
    let (status, nothing) = harness
        .post(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }))
        .await;
    assert_eq!((status, nothing), (StatusCode::ACCEPTED, Json::Null));
    let (_, unknown) = harness.post(&request(3, "resources/list", json!({}))).await;
    assert_eq!(unknown["error"]["code"], -32601);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_tool_the_session_was_not_given_is_unknown_to_its_harness() {
    let (rig, thread, _) = rig_with_session(&["announce"]).await;
    let harness = Harness::of(&rig, thread);
    let (_, answer) = harness
        .post(&request(
            1,
            "tools/call",
            json!({ "name": "request_capability", "arguments": { "capability": "kvm", "reason": "x" } }),
        ))
        .await;
    assert_eq!(answer["error"]["code"], -32602);
    assert!(
        answer["error"]["message"]
            .as_str()
            .unwrap()
            .contains("unknown tool")
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_call_becomes_an_event_of_the_session_and_the_systems_result_is_the_tools_result() {
    let (rig, thread, segment) = rig_with_session(&["announce"]).await;
    let turn = rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let harness = Arc::new(Harness::of(&rig, thread));
    let calling = {
        let harness = Arc::clone(&harness);
        tokio::spawn(async move {
            harness
                .post(&request(
                    9,
                    "tools/call",
                    json!({ "name": "announce", "arguments": { "title": "Build is green" } }),
                ))
                .await
        })
    };
    let arrived = the_call_that_arrived(&rig, thread, segment).await;
    assert_eq!(arrived["tool"], "announce");
    assert_eq!(arrived["arguments"], json!({ "title": "Build is green" }));
    assert_eq!(arrived["turn_id"], json!(turn));
    let call_ref = arrived["call_ref"].as_str().unwrap().to_owned();
    assert!(call_ref.starts_with("mcp:"));
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref,
            attempt: None,
            result: Some(json!({ "announcement_id": "a1", "audience": "topic" })),
            error: None,
        },
    )
    .await;
    let (status, answer) = calling.await.unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["id"], 9);
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(
        answer["result"]["structuredContent"],
        json!({ "audience": "topic" })
    );
    assert!(
        answer["result"]["structuredContent"]
            .get("announcement_id")
            .is_none(),
        "optional ids are delivered only when requested"
    );
    assert_eq!(answer["result"]["content"], json!([]));
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_refusal_reaches_the_harness_as_a_result_marked_as_an_error_that_begins_with_its_code() {
    let (rig, thread, segment) = rig_with_session(&["announce"]).await;
    rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let harness = Arc::new(Harness::of(&rig, thread));
    let calling = {
        let harness = Arc::clone(&harness);
        tokio::spawn(async move {
            harness
                .post(&request(
                    4,
                    "tools/call",
                    json!({ "name": "announce", "arguments": {} }),
                ))
                .await
        })
    };
    let arrived = the_call_that_arrived(&rig, thread, segment).await;
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref: arrived["call_ref"].as_str().unwrap().to_owned(),
            attempt: None,
            result: None,
            error: Some(ToolError {
                code: "invalid".to_owned(),
                message: "The argument \"title\" is required.".to_owned(),
            }),
        },
    )
    .await;
    let (_, answer) = calling.await.unwrap();
    assert_eq!(answer["result"]["isError"], true);
    assert_eq!(
        answer["result"]["content"][0]["text"],
        "invalid: The argument \"title\" is required."
    );
    assert!(
        answer.get("error").is_none(),
        "a refusal is no error of the protocol"
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_call_nobody_answers_in_time_keeps_read_and_change_timeouts_distinct() {
    let (rig, thread, segment) = rig_with_session(&["announce", "list_shared"]).await;
    // The same server, with a system that has a moment to answer, in place of the one that has two minutes.
    let connector = rig.connector.as_ref().unwrap();
    connector.loopback().register(
        orch_connector_core::loopback::EndpointKind::Mcp("orch".to_owned()),
        Arc::new(OrchEndpoint::with_wait(
            Arc::clone(connector.services().agents.get().unwrap()),
            Duration::from_millis(300),
        )),
    );
    rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let harness = Harness::of(&rig, thread);
    for (id, tool, arguments, text) in [
        (
            5,
            "announce",
            json!({ "title": "Anyone there?" }),
            TIMEOUT_CHANGE,
        ),
        (6, "list_shared", json!({}), UNREACHABLE),
    ] {
        let (status, answer) = harness
            .post(&request(
                id,
                "tools/call",
                json!({ "name": tool, "arguments": arguments }),
            ))
            .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(answer["result"]["isError"], true);
        assert_eq!(
            answer["result"]["content"],
            json!([{ "type": "text", "text": format!("unavailable: {text}") }])
        );
        // The system's answer, if it comes after all, is taken without a failure: the call was ours.
        rig.wait_for(
            thread,
            segment,
            "orch.tool_call",
            if tool == "announce" { 1 } else { 2 },
        )
        .await;
        let arrived = rig
            .applied(thread, segment)
            .into_iter()
            .find(|event| event.kind == "orch.tool_call" && event.body["tool"] == tool)
            .unwrap()
            .body;
        rig.succeed(
            &SerialKey::Thread(thread),
            &OrchToolResult {
                thread_id: thread,
                call_ref: arrived["call_ref"].as_str().unwrap().to_owned(),
                attempt: None,
                result: Some(json!({})),
                error: None,
            },
        )
        .await;
    }
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_call_while_no_turn_runs_is_told_so_and_reaches_the_system_not_at_all() {
    let (rig, thread, segment) = rig_with_session(&["announce"]).await;
    let harness = Harness::of(&rig, thread);
    let (_, answer) = harness
        .post(&request(
            6,
            "tools/call",
            json!({ "name": "announce", "arguments": { "title": "Hello" } }),
        ))
        .await;
    assert_eq!(answer["result"]["isError"], true);
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .contains("No turn of yours is running")
    );
    assert!(
        !rig.kinds(thread, segment)
            .contains(&"orch.tool_call".to_owned())
    );
    rig.connector.unwrap().stop().await;
}

/// These cases select actual connector lanes/turns, not reconstructed model history.
/// The engine's source-effect cases separately drive the guarded continuation job.
async fn source_retry_isolation(replace_lane: bool) {
    use orch_link_contract::commands::{SessionClose, TurnInterrupt};
    let (rig, thread, original_segment) = rig_with_session(&["announce"]).await;
    let connector = rig.connector.as_ref().unwrap();
    let hosting = Arc::clone(connector.services().agents.get().unwrap());
    connector.loopback().register(
        orch_connector_core::loopback::EndpointKind::Mcp("orch".into()),
        Arc::new(OrchEndpoint::with_wait(
            hosting.clone(),
            Duration::from_millis(300),
        )),
    );
    let original_turn = rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, original_segment, "turn.started", 1)
        .await;
    let old = Harness::of(&rig, thread);
    let arguments = json!({"title":"Identical arguments from separate native sources","ids":true});
    let (_, timeout) = old
        .post(&request(
            71,
            "tools/call",
            json!({"name":"announce","arguments":arguments}),
        ))
        .await;
    assert_eq!(timeout["result"]["isError"], true);
    assert_eq!(
        timeout["result"]["content"],
        json!([{"type":"text","text":format!("unavailable: {TIMEOUT_CHANGE}")}])
    );
    let arrived = the_call_that_arrived(&rig, thread, original_segment).await;
    let original_ref = arrived["call_ref"].as_str().unwrap().to_owned();
    let original_result = json!({"announcement_id":"source-A-owned-answer","audience":"topic"});
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref: original_ref.clone(),
            attempt: Some(new_id()),
            result: Some(original_result.clone()),
            error: None,
        },
    )
    .await;
    let (_, retry) = old
        .post(&request(
            72,
            "tools/call",
            json!({"name":"announce","arguments":arguments}),
        ))
        .await;
    assert_eq!(retry["result"]["structuredContent"], original_result);
    assert_eq!(
        rig.applied(thread, original_segment)
            .iter()
            .filter(|event| event.kind == "orch.tool_call")
            .count(),
        1,
        "a timed-out call's same-source retry returns the actual retained answer without another effect"
    );
    let recorded = orch_connector_core::ledger::Ledger::open(rig.data.path())
        .unwrap()
        .retained_calls();
    let retained = recorded
        .iter()
        .find(|call| call.call_ref == original_ref)
        .unwrap();
    assert_eq!(retained.source.unwrap().segment_id, original_segment);
    assert_eq!(retained.source.unwrap().turn_id, original_turn);
    assert_eq!(retained.answer, Some(Ok(original_result.clone())));
    rig.succeed(
        &SerialKey::Thread(thread),
        &TurnInterrupt {
            thread_id: thread,
            turn_id: original_turn,
        },
    )
    .await;
    rig.wait_for(thread, original_segment, "turn.completed", 1)
        .await;
    let segment = if replace_lane {
        rig.succeed(
            &SerialKey::Thread(thread),
            &SessionClose { thread_id: thread },
        )
        .await;
        let directory = rig.data.path().join("workspace");
        let segment = new_id();
        let mut body = rig.open_body(thread, segment, directory.to_str().unwrap());
        body["orch_tools"] = json!(["announce"]);
        rig.open_session(body).await;
        segment
    } else {
        original_segment
    };
    // Only the synthetic first timeout is shortened. The actual B exchange keeps the
    // normal listener budget and existing 20/25s fixture watchdogs.
    connector.loopback().register(
        orch_connector_core::loopback::EndpointKind::Mcp("orch".into()),
        Arc::new(OrchEndpoint::new(hosting)),
    );
    let selected_turn = rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(
        thread,
        segment,
        "turn.started",
        if replace_lane { 1 } else { 2 },
    )
    .await;
    let current = Harness::of(&rig, thread);
    let request_body = request(
        73,
        "tools/call",
        json!({"name":"announce","arguments":arguments}),
    );
    let calling = tokio::spawn(async move { current.post(&request_body).await });
    rig.wait_for(
        thread,
        segment,
        "orch.tool_call",
        if replace_lane { 1 } else { 2 },
    )
    .await;
    let fresh = rig
        .applied(thread, segment)
        .into_iter()
        .filter(|event| event.kind == "orch.tool_call")
        .find(|event| event.body["turn_id"] == json!(selected_turn))
        .unwrap()
        .body;
    let selected_ref = fresh["call_ref"].as_str().unwrap().to_owned();
    assert_ne!(
        selected_ref, original_ref,
        "identical arguments cannot alias the old logical source reference"
    );
    assert_eq!(fresh["arguments"], arguments);
    // A late duplicate A result, with a different transport attempt, must be consumed
    // by A's retained ref rather than resolving B or falling through to B's adapter.
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref: original_ref.clone(),
            attempt: Some(new_id()),
            result: Some(original_result.clone()),
            error: None,
        },
    )
    .await;
    let selected_result = json!({"announcement_id":"source-B-fresh-answer","audience":"topic"});
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref: selected_ref,
            attempt: serde_json::from_value(fresh["attempt"].clone()).unwrap(),
            result: Some(selected_result.clone()),
            error: None,
        },
    )
    .await;
    let (status, answer) = tokio::time::timeout(Duration::from_secs(25), calling)
        .await
        .expect("the scoped native reply never came")
        .unwrap();
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["result"]["isError"], false);
    assert_eq!(answer["result"]["structuredContent"], selected_result);
    let original_calls = rig
        .applied(thread, original_segment)
        .iter()
        .filter(|event| {
            event.kind == "orch.tool_call" && event.body["turn_id"] == json!(original_turn)
        })
        .count();
    assert_eq!(
        original_calls, 1,
        "neither B's same arguments nor A's replay executes another A effect"
    );
    let recorded = orch_connector_core::ledger::Ledger::open(rig.data.path())
        .unwrap()
        .retained_calls();
    let retained = recorded
        .iter()
        .find(|call| call.call_ref == original_ref)
        .unwrap();
    assert_eq!(retained.source.unwrap().turn_id, original_turn);
    assert_eq!(retained.answer, Some(Ok(original_result)));
    if replace_lane {
        let (status, _) = old
            .post(&request(
                74,
                "tools/call",
                json!({"name":"announce","arguments":arguments}),
            ))
            .await;
        assert_eq!(
            status,
            StatusCode::NOT_FOUND,
            "a retired listener key cannot borrow the selected lane's turn"
        );
    }
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_new_native_turn_cannot_reuse_a_timed_out_prior_turns_answer_or_pending_reference() {
    source_retry_isolation(false).await;
}

#[tokio::test]
async fn a_selected_native_lane_cannot_reuse_the_previous_lanes_listener_answer_or_reference() {
    source_retry_isolation(true).await;
}

#[tokio::test]
async fn delayed_mcp_request_bytes_cannot_borrow_the_next_turn_on_the_same_native_listener() {
    use orch_link_contract::commands::TurnInterrupt;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let (rig, thread, segment) = rig_with_session(&["announce"]).await;
    let original = rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    let harness = Harness::of(&rig, thread);
    let arguments = json!({"title":"Delayed source-bound request","ids":true});
    let body = serde_json::to_vec(&request(
        81,
        "tools/call",
        json!({"name":"announce","arguments":arguments}),
    ))
    .unwrap();
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", harness.port))
        .await
        .unwrap();
    let headers = format!(
        "POST {} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Bearer {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n",
        harness.path,
        harness.port,
        harness.token,
        body.len()
    );
    stream.write_all(headers.as_bytes()).await.unwrap();
    // HTTP100 is emitted when the endpoint first polls Incoming, after capturing
    // its actual source. No guessed delay or test-only production hook is needed.
    let interim = tokio::time::timeout(Duration::from_secs(20), async {
        let mut bytes = Vec::new();
        while !bytes.ends_with(b"\r\n\r\n") {
            let byte = stream.read_u8().await.unwrap();
            bytes.push(byte);
            assert!(
                bytes.len() <= 1024,
                "the controlled interim response must stay bounded"
            );
        }
        bytes
    })
    .await
    .expect("the authenticated request never reached its first body poll");
    assert!(
        interim.starts_with(b"HTTP/1.1 100 "),
        "the actual server body-poll phase was not observed"
    );
    rig.succeed(
        &SerialKey::Thread(thread),
        &TurnInterrupt {
            thread_id: thread,
            turn_id: original,
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let selected = rig.start_turn(thread, "#!fake\nsleep 20000").await;
    rig.wait_for(thread, segment, "turn.started", 2).await;
    stream.write_all(&body).await.unwrap();
    let response = tokio::time::timeout(Duration::from_secs(20), async {
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).await.unwrap();
        bytes
    })
    .await
    .expect("the delayed source refusal never arrived");
    let split = response
        .windows(4)
        .position(|bytes| bytes == b"\r\n\r\n")
        .expect("a complete native HTTP response");
    assert!(response.starts_with(b"HTTP/1.1 200 "));
    let answer: Json = serde_json::from_slice(&response[split + 4..]).unwrap();
    assert_eq!(answer["result"]["isError"], true);
    assert!(
        answer["result"]["content"][0]["text"]
            .as_str()
            .unwrap()
            .starts_with("changed: The native turn that admitted this request")
    );
    assert_eq!(
        rig.applied(thread, segment)
            .iter()
            .filter(|event| event.kind == "orch.tool_call")
            .count(),
        0,
        "the old HTTP request cannot publish work attributed to either the completed or newly running turn"
    );
    let current = Harness::of(&rig, thread);
    let calling = tokio::spawn(async move {
        current
            .post(&request(
                82,
                "tools/call",
                json!({"name":"announce","arguments":arguments}),
            ))
            .await
    });
    let fresh = the_call_that_arrived(&rig, thread, segment).await;
    assert_eq!(fresh["turn_id"], json!(selected));
    rig.succeed(
        &SerialKey::Thread(thread),
        &OrchToolResult {
            thread_id: thread,
            call_ref: fresh["call_ref"].as_str().unwrap().into(),
            attempt: serde_json::from_value(fresh["attempt"].clone()).unwrap(),
            result: Some(json!({"announcement_id":"current-source"})),
            error: None,
        },
    )
    .await;
    let (_, reply) = tokio::time::timeout(Duration::from_secs(25), calling)
        .await
        .expect("the legitimate current-source call never completed")
        .unwrap();
    assert_eq!(
        reply["result"]["structuredContent"]["announcement_id"],
        "current-source"
    );
    rig.connector.unwrap().stop().await;
}
