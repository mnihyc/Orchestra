//! The generic adapter against an agent that speaks the protocol from a pipe: sessions, turns, requests, interrupts and what the plugin adds.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{
    Action, Agent, Opened, Rig, Seen, message, names, permission, program, tool_call, tool_done,
};
use orch_agent_contract::adapter::AgentAdapter;
use orch_agent_contract::descriptor::HarnessSupport;
use orch_agent_contract::session::{ApprovalDecision, ElicitationAction, SessionMode};
use serde_json::{Value, json};

fn position(events: &[Seen], name: &str) -> usize {
    events
        .iter()
        .position(|event| event.is(name))
        .unwrap_or_else(|| panic!("no {name} in {:?}", names(events)))
}

#[tokio::test]
async fn starting_asks_the_program_its_version_and_what_it_says_and_declares_only_that() {
    let agent = Agent::new();
    let rig = Rig::start(program(json!({})), agent.clone()).await;
    assert_eq!(rig.started.harness_version, "0.2.1-rc.2");
    let declared = &rig.started.descriptor;
    assert_eq!(declared.check(), Ok(()));
    assert_eq!(declared.harness.status, HarnessSupport::Supported);
    assert!(
        declared.capabilities.resume
            && declared.capabilities.input_images
            && declared.capabilities.interrupt
    );
    assert!(!declared.capabilities.steer && !declared.capabilities.usage_tokens);
    assert_eq!(declared.forwarded_protocols, ["openai-chat"]);
    let launches = agent.record.lock().unwrap().launches.clone();
    assert_eq!(launches[0].args, ["--version"]);
    assert_eq!(launches[1].args, ["--acp"]);
    assert!(
        !launches[1].env.contains_key("HOME"),
        "a variable of the connector's own is not passed unless the manifest names it"
    );
    assert_eq!(launches[1].env["DEMO_TOKEN"], "t".repeat(64));
}

#[tokio::test]
async fn a_turn_streams_what_the_agent_thinks_and_says_and_reports_no_count_it_was_not_given() {
    let agent = Agent::new();
    agent.script(vec![
        message("agent_thought_chunk", "m1", "hmm "),
        message("agent_thought_chunk", "m1", "yes"),
        message("agent_message_chunk", "m1", "Done."),
        Action::Update(json!({ "sessionUpdate": "usage_update", "used": 5702, "size": 128000 })),
    ]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let events = rig.say(&session, "hello").await;
    let all: Vec<&str> = names(&events)
        .into_iter()
        .filter(|name| *name != "harness.status")
        .collect();
    assert_eq!(all.first(), Some(&"turn.started"));
    assert_eq!(all.last(), Some(&"turn.completed"));
    assert!(position(&events, "item.started") < position(&events, "usage.updated"));
    let request = events
        .iter()
        .find(|event| event.is("model.request"))
        .expect("a request");
    assert_eq!(
        request.body["request"]["tokens"]["input_tokens"],
        Value::Null
    );
    assert_eq!(request.body["request"]["outcome"]["status"], "done");
    let usage = events
        .iter()
        .find(|event| event.is("usage.updated"))
        .unwrap();
    assert_eq!(usage.body["context_used"], 5702);
    assert_eq!(usage.body["context_window"], 128000);
    let completed: Vec<&Seen> = events
        .iter()
        .filter(|event| event.is("item.completed"))
        .collect();
    assert_eq!(completed[0].body["payload"]["summary"], "hmm yes");
    assert_eq!(completed[1].body["payload"]["text"], "Done.");
    assert_eq!(completed[1].body["payload"]["phase"], "final");
    assert_eq!(events.last().unwrap().body["status"], "completed");
}

#[tokio::test]
async fn a_tool_that_asks_waits_for_a_person_and_goes_on_with_the_answer() {
    let agent = Agent::new();
    agent.script(vec![
        message("agent_message_chunk", "m1", "Let me check."),
        tool_call(
            "c1",
            "bash",
            json!({ "command": "rm -rf build", "description": "clean", "justification": "stale" }),
        ),
        permission("c1", "perm"),
        tool_done("c1", "completed", "gone\n[exit code: 0]"),
        message("agent_message_chunk", "m2", "Cleaned."),
    ]);
    let mut rig = Rig::start(
        program(json!({ "command_exit_marker": "[exit code:" })),
        agent.clone(),
    )
    .await;
    let session = rig.session().await;
    rig.begin(&session, "clean it").await;
    let asked = rig.until(|event| event.is("approval.requested")).await;
    let request = asked.last().unwrap();
    assert_eq!(request.body["request"]["request_kind"], "command");
    assert_eq!(request.body["request"]["detail"]["command"], "rm -rf build");
    assert_eq!(request.body["request"]["detail"]["reason"], "stale");
    assert_eq!(request.body["request"]["options"][0]["id"], "allow-once");
    let reference = request.body["request_ref"].as_str().unwrap().to_owned();
    rig.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Option {
                option_id: "allow-once".into(),
                note: None,
            },
        )
        .await
        .unwrap();
    let rest = rig.until(|event| event.is("turn.completed")).await;
    assert_eq!(
        agent.answer("perm").unwrap(),
        json!({ "outcome": { "outcome": "selected", "optionId": "allow-once" } })
    );
    let call = rest
        .iter()
        .find(|event| event.is("item.completed") && event.body["native_ref"] == "tool:c1")
        .expect("the call ended");
    assert_eq!(call.body["status"], "completed");
    assert_eq!(call.body["payload"]["tool"]["exit_code"], 0);
    assert_eq!(call.body["payload"]["title"], "clean");
}

#[tokio::test]
async fn an_answer_that_is_none_of_the_options_is_refused_and_the_request_still_waits() {
    let agent = Agent::new();
    agent.script(vec![
        tool_call("c1", "bash", json!({ "command": "ls" })),
        permission("c1", "perm"),
        tool_done("c1", "completed", ""),
    ]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    rig.begin(&session, "go").await;
    let asked = rig.until(|event| event.is("approval.requested")).await;
    let reference = asked.last().unwrap().body["request_ref"]
        .as_str()
        .unwrap()
        .to_owned();
    let refused = rig
        .adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Option {
                option_id: "allow-forever".into(),
                note: None,
            },
        )
        .await;
    assert!(refused.is_err());
    rig.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Option {
                option_id: "reject-once".into(),
                note: Some("no".into()),
            },
        )
        .await
        .unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    assert_eq!(
        agent.answer("perm").unwrap()["outcome"]["optionId"],
        "reject-once"
    );
    assert_eq!(
        agent.answer("perm").unwrap()["outcome"]["_meta"]["orch"]["note"],
        "no"
    );
}

#[tokio::test]
async fn an_interrupt_asks_the_agent_to_stop_withdraws_what_waits_and_ends_the_turn_interrupted() {
    let agent = Agent::new();
    agent.script(vec![
        tool_call("c1", "bash", json!({ "command": "sleep 100" })),
        permission("c1", "perm"),
        Action::UntilCancelled,
    ]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let turn = rig.begin(&session, "go").await;
    rig.until(|event| event.is("approval.requested")).await;
    rig.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = rig.until(|event| event.is("turn.completed")).await;
    assert!(rest.iter().any(|event| event.is("approval.closed")));
    assert_eq!(rest.last().unwrap().body["status"], "interrupted");
    assert_eq!(agent.frames("session/cancel").len(), 1);
    assert_eq!(
        agent.answer("perm").unwrap(),
        json!({ "outcome": { "outcome": "cancelled" } })
    );
    let call = rest
        .iter()
        .find(|event| event.is("item.completed") && event.body["native_ref"] == "tool:c1")
        .unwrap();
    assert_eq!(call.body["status"], "cancelled");
}

#[tokio::test]
async fn an_agent_that_does_not_stop_when_asked_is_ended_after_its_grace_and_the_session_is_lost() {
    let agent = Agent::new();
    agent.script(vec![
        message("agent_message_chunk", "m1", "working"),
        Action::Hang,
    ]);
    let mut rig = Rig::start(program(json!({ "cancel_grace_ms": 200 })), agent.clone()).await;
    let session = rig.session().await;
    let turn = rig.begin(&session, "go").await;
    rig.until(|event| event.is("item.delta")).await;
    rig.adapter.interrupt(session.thread, turn).await.unwrap();
    let rest = rig.until(|event| event.is("session.lost")).await;
    let ended = rest
        .iter()
        .find(|event| event.is("turn.completed"))
        .expect("the turn ended");
    assert_eq!(ended.body["status"], "interrupted");
    assert!(
        rest.last().unwrap().body["reason"]
            .as_str()
            .unwrap()
            .contains("did not stop")
    );
    assert!(
        rig.adapter
            .start_turn(rig.turn_of(&session, "again"))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_process_that_ends_in_the_middle_of_a_turn_fails_the_turn_and_loses_the_session() {
    let agent = Agent::new();
    agent.script(vec![
        message("agent_message_chunk", "m1", "half"),
        Action::Die,
    ]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let events = rig.say(&session, "go").await;
    let rest = if events.last().unwrap().is("session.lost") {
        events
    } else {
        let mut all = events;
        all.extend(rig.until(|event| event.is("session.lost")).await);
        all
    };
    let ended = rest
        .iter()
        .find(|event| event.is("turn.completed"))
        .unwrap();
    assert_eq!(ended.body["status"], "failed");
    assert_eq!(ended.body["error"]["code"], "harness_error");
    assert!(rest.iter().any(|event| event.is("session.lost")));
}

#[tokio::test]
async fn the_model_the_effort_and_the_mode_are_set_before_the_prompt_only_where_they_differ() {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().modes = Some(json!({
        "currentModeId": "default",
        "availableModes": [ { "id": "default", "name": "Default" }, { "id": "plan", "name": "Plan" } ]
    }));
    let mut rig = Rig::start(
        program(json!({ "plan_modes": { "plan": "plan", "default": "default" } })),
        agent.clone(),
    )
    .await;
    let session = rig.session().await;
    let mut turn = rig.turn_of(&session, "plan it");
    turn.model = "second".into();
    turn.effort = Some("high".into());
    turn.plan = true;
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    let options = agent.frames("session/set_config_option");
    assert_eq!(options.len(), 2);
    assert_eq!(options[0]["params"]["configId"], "model");
    assert_eq!(options[0]["params"]["value"], "[\"orch\",\"second\"]");
    assert_eq!(options[1]["params"]["configId"], "reasoning_effort");
    assert_eq!(options[1]["params"]["value"], "high");
    assert_eq!(
        agent.frames("session/set_mode")[0]["params"]["modeId"],
        "plan"
    );
    let mut again = rig.turn_of(&session, "now do it");
    again.model = "second".into();
    again.effort = Some("high".into());
    rig.adapter.start_turn(again).await.unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    assert_eq!(
        agent.frames("session/set_config_option").len(),
        2,
        "nothing differed"
    );
    let modes = agent.frames("session/set_mode");
    assert_eq!(modes.len(), 2);
    assert_eq!(
        modes[1]["params"]["modeId"], "default",
        "the turn that is no plan leaves plan mode"
    );
}

#[tokio::test]
async fn a_model_the_program_does_not_offer_is_refused_before_any_turn_begins() {
    let agent = Agent::new();
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let mut turn = rig.turn_of(&session, "go");
    turn.model = "not-offered".into();
    let refused = rig.adapter.start_turn(turn).await.unwrap_err();
    assert_eq!(refused.code(), "model_unavailable");
    assert!(
        rig.queued().iter().all(|event| !event.is("turn.started")),
        "no turn began"
    );
    assert!(agent.frames("session/prompt").is_empty());
}

#[tokio::test]
async fn a_question_of_the_agent_is_put_to_a_person_and_the_answers_go_back_as_the_values_of_the_form()
 {
    let agent = Agent::new();
    agent.script(vec![Action::Ask {
        method: "elicitation/create",
        params: json!({ "sessionId": "$SESSION", "mode": "form", "message": "Which database?",
            "requestedSchema": { "type": "object", "properties": {
                "db": { "type": "string", "title": "Database", "oneOf": [ { "const": "pg", "title": "Postgres" }, { "const": "my", "title": "MySQL" } ] } } } }),
        tag: "form",
    }]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    rig.begin(&session, "ask").await;
    let asked = rig.until(|event| event.is("approval.requested")).await;
    let request = &asked.last().unwrap().body;
    assert_eq!(request["request"]["request_kind"], "user_input");
    assert_eq!(
        request["request"]["detail"]["questions"][0]["options"][0]["label"],
        "Postgres"
    );
    let reference = request["request_ref"].as_str().unwrap().to_owned();
    rig.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Answers {
                answers: [("db".to_owned(), vec!["Postgres".to_owned()])].into(),
                note: None,
            },
        )
        .await
        .unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    assert_eq!(
        agent.answer("form").unwrap(),
        json!({ "action": "accept", "content": { "db": "pg" } })
    );
    let _ = ElicitationAction::Accept;
}

#[tokio::test]
async fn a_session_is_resumed_in_a_new_process_by_the_id_the_agent_gave_it() {
    let agent = Agent::new();
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let first = rig.session().await;
    rig.say(&first, "one").await;
    rig.adapter.close_session(first.thread).await.unwrap();
    let mut spec = rig.spec(SessionMode::Resume);
    spec.settings.native_session_ref = Some(first.native.clone());
    let resumed = rig.adapter.open_session(spec).await.unwrap();
    assert!(resumed.resumed);
    assert_eq!(resumed.native_session_ref, first.native);
    let resumes = agent.frames("session/resume");
    assert_eq!(resumes.len(), 1);
    assert_eq!(resumes[0]["params"]["sessionId"], first.native.as_str());
    assert_eq!(
        agent.processes(),
        3,
        "the probe, the session, and the session again"
    );
}

#[tokio::test]
async fn orchs_tools_and_the_environments_servers_are_given_by_address_with_the_token_and_the_plugin_is_told_the_session()
 {
    let agent = Agent::new();
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let mut spec = rig.spec(SessionMode::New);
    spec.mcp = vec!["tmux".into()];
    let key = spec.session_key.clone();
    rig.open(spec).await;
    let new = &agent.frames("session/new")[0]["params"];
    let servers = new["mcpServers"].as_array().unwrap();
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0]["name"], "orch");
    assert_eq!(servers[0]["type"], "http");
    assert_eq!(
        servers[0]["url"],
        format!("http://127.0.0.1:41999/mcp/{key}/orch")
    );
    assert_eq!(
        servers[0]["headers"][0]["value"],
        format!("Bearer {}", "t".repeat(64))
    );
    assert_eq!(
        servers[1]["url"],
        format!("http://127.0.0.1:41999/mcp/{key}/tmux")
    );
    assert_eq!(new["_meta"]["orch"]["version"], 1);
    assert_eq!(new["_meta"]["orch"]["session_key"], key.as_str());
    assert_eq!(new["cwd"], rig.work.path().display().to_string());
}

#[tokio::test]
async fn an_image_of_a_turn_goes_to_the_agent_as_a_block_of_base64() {
    let agent = Agent::new();
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let path = rig.work.path().join("shot.png");
    std::fs::write(&path, [137_u8, 80, 78, 71]).unwrap();
    let mut turn = rig.turn_of(&session, "look");
    turn.images = vec![orch_agent_contract::session::ImageInput {
        blob: orch_types::Hash::of(b"x"),
        media_type: "image/png".into(),
        name: "shot.png".into(),
        path: Some(path.display().to_string()),
    }];
    rig.adapter.start_turn(turn).await.unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    let prompt = &agent.frames("session/prompt")[0]["params"]["prompt"];
    assert_eq!(prompt[1]["type"], "image");
    assert_eq!(prompt[1]["mimeType"], "image/png");
    assert_eq!(prompt[1]["data"], "iVBORw==");
}

#[tokio::test]
async fn a_refusal_ends_the_turn_as_the_models_refusal_and_an_error_of_the_agent_as_its_failure() {
    let agent = Agent::new();
    agent.script(vec![Action::Stop("refusal")]);
    agent.script(vec![Action::Fail(-32603, "Internal error")]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let refused = rig.say(&session, "one").await;
    assert_eq!(
        refused.last().unwrap().body["error"]["code"],
        "model_refused"
    );
    let failed = rig.say(&session, "two").await;
    let ended = failed.last().unwrap();
    assert_eq!(
        (
            ended.body["status"].as_str(),
            ended.body["error"]["code"].as_str()
        ),
        (Some("failed"), Some("harness_error"))
    );
}

#[tokio::test]
async fn raw_frames_are_kept_when_asked_and_no_credential_is_in_them() {
    let agent = Agent::new();
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let mut spec = rig.spec(SessionMode::New);
    spec.settings.raw_events = true;
    let session = rig.open(spec).await;
    let events = rig.say(&session, "hi").await;
    assert!(events.iter().any(|event| event.is("raw.frame")));
    let text = serde_json::to_string(&names(&events)).unwrap();
    assert!(!text.is_empty());
    let all: String = events.iter().map(|event| event.body.to_string()).collect();
    assert!(
        !all.contains(&"t".repeat(64)),
        "the token is hidden in what is kept"
    );
}

fn hello(extras: Value) -> Value {
    json!({
        "protocol": 1,
        "plugin": { "name": "orch-demo-bridge", "version": "0.1.0" },
        "host": { "name": "demo", "version": "0.2.1-rc.2", "tested": true },
        "extras": extras,
        "methods": []
    })
}

fn on() -> Value {
    json!({ "status": "on" })
}

fn plugin_manifest(extras: &[&str]) -> orch_adapter_acp::program::AcpProgram {
    program(json!({
        "companion": { "file": "orch-demo-bridge.mjs", "extras": extras, "hello_wait_ms": 2000 }
    }))
}

#[tokio::test]
async fn a_plugins_hello_turns_its_extras_on_steering_reaches_it_and_its_model_calls_stand_for_the_envelope()
 {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello = Some(hello(json!({
        "steering": on(), "usage": on(), "teleport": on(),
        "questions": { "status": "off", "reason": "host_api_missing", "detail": "no ask tool" }
    })));
    agent.script(vec![
        message("agent_message_chunk", "m1", "working"),
        Action::Notify("_orch/usage", json!({ "sessionId": "$SESSION", "turn": 1, "step": 0, "attempt": "a1",
            "provider": "orch", "model": "scripted",
            "usage": { "inputTokens": 36, "cacheReadTokens": 64, "outputTokens": 20, "reasoningTokens": 5 } })),
        Action::Sleep(300),
    ]);
    let manifest = plugin_manifest(&["steering", "usage", "questions"]);
    let mut rig = Rig::start_plugin(manifest, agent.clone()).await;
    let declared = &rig.started.descriptor;
    assert!(declared.capabilities.steer && declared.capabilities.usage_tokens);
    assert!(
        !declared
            .capabilities
            .approvals
            .contains(&orch_agent_contract::events::HarnessRequestKind::UserInput)
    );
    let note = &declared.extensions.get("x_acp_plugin").unwrap();
    assert_eq!(note["extras"], json!(["steering", "usage"]));
    let off: Vec<(&str, &str)> = note["off"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["extra"].as_str().unwrap(),
                entry["reason"].as_str().unwrap(),
            )
        })
        .collect();
    assert!(off.contains(&("questions", "host_api_missing")), "{off:?}");
    assert!(
        off.iter()
            .any(|(name, why)| *name == "teleport" && why.contains("manifest")),
        "{off:?}"
    );
    let session = rig.session().await;
    let turn = rig.begin(&session, "go").await;
    let reported = rig.until(|event| event.is("model.request")).await;
    let request = &reported.last().unwrap().body["request"];
    assert_eq!(request["id"], "a1");
    assert_eq!(
        request["tokens"]["input_tokens"], 100,
        "the cache is a part of the input"
    );
    assert_eq!(request["tokens"]["cached_input_tokens"], 64);
    assert_eq!(request["tokens"]["output_tokens"], 20);
    rig.adapter
        .steer(orch_agent_contract::session::TurnSteer {
            thread_id: session.thread,
            turn_id: turn,
            text: "also this".into(),
            images: vec![],
        })
        .await
        .unwrap();
    let rest = rig.until(|event| event.is("turn.completed")).await;
    let steers = agent.frames("_orch/input/steer");
    assert_eq!(steers.len(), 1);
    assert_eq!(steers[0]["params"]["content"][0]["text"], "also this");
    assert!(
        steers[0]["params"]["messageId"]
            .as_str()
            .is_some_and(|id| !id.is_empty())
    );
    assert!(
        !rest.iter().any(|event| event.is("model.request")),
        "the plugin's call stands for the envelope"
    );
}

#[tokio::test]
async fn a_plugin_that_is_asked_for_its_hello_and_answers_is_heard_as_one_that_says_it() {
    let agent = Agent::new();
    {
        let mut behavior = agent.behavior.lock().unwrap();
        behavior.hello = Some(hello(json!({ "steering": on() })));
        behavior.hello_on_request = true;
    }
    let rig = Rig::start_plugin(plugin_manifest(&["steering"]), agent.clone()).await;
    assert!(rig.started.descriptor.capabilities.steer);
    assert_eq!(
        agent.frames("_orch/hello").len(),
        1,
        "the probe asked once; no session was opened"
    );
}

#[tokio::test]
async fn a_program_without_its_plugin_file_is_plain_protocol_and_is_not_waited_for() {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello = Some(hello(json!({ "steering": on() })));
    let started = std::time::Instant::now();
    let rig = Rig::start(plugin_manifest(&["steering"]), agent.clone()).await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(1500),
        "no wait for a plugin that is not there"
    );
    assert!(!rig.started.descriptor.capabilities.steer);
    assert!(agent.frames("_orch/hello").is_empty());
}

fn streaming_rig_script(agent: &std::sync::Arc<Agent>) {
    agent.behavior.lock().unwrap().hello = Some(hello(json!({ "streaming": on() })));
    let frames = |seq: u64, frames: Value| {
        Action::Notify(
            "_orch/stream",
            json!({ "sessionId": "$SESSION", "turn": 1, "step": 0, "attempt": "a1", "seq": seq, "frames": frames }),
        )
    };
    agent.script(vec![
        frames(0, json!([ { "kind": "start" }, { "kind": "text", "index": 0, "delta": "Hel" } ])),
        frames(2, json!([ { "kind": "text", "index": 0, "delta": "lo" }, { "kind": "end", "outcome": "committed" } ])),
        Action::Update(json!({ "sessionUpdate": "agent_message_chunk", "messageId": "m1",
            "content": { "type": "text", "text": "Hello" }, "_meta": { "orch": { "attempt": "a1", "turn": 1, "step": 0 } } })),
    ]);
}

#[tokio::test]
async fn a_plugin_that_streams_a_call_shows_it_growing_and_the_committed_message_ends_that_item_without_a_second()
 {
    let agent = Agent::new();
    streaming_rig_script(&agent);
    let mut rig = Rig::start_plugin(plugin_manifest(&["streaming"]), agent.clone()).await;
    let session = rig.session().await;
    let events = rig.say(&session, "greet").await;
    let kinds = names(&events);
    assert_eq!(
        kinds.iter().filter(|name| **name == "item.started").count(),
        1,
        "{kinds:?}"
    );
    let deltas: Vec<&str> = events
        .iter()
        .filter(|event| event.is("item.delta"))
        .map(|event| event.body["data"].as_str().unwrap())
        .collect();
    assert_eq!(deltas, ["Hel", "lo"]);
    let done = events
        .iter()
        .find(|event| event.is("item.completed"))
        .unwrap();
    assert_eq!(done.body["payload"]["text"], "Hello");
    assert_eq!(done.body["payload"]["phase"], "final");
}

#[tokio::test]
async fn a_plugins_questions_and_plan_are_cards_and_what_a_person_answers_goes_back_in_its_words() {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello =
        Some(hello(json!({ "questions": on(), "plan_mode": on() })));
    agent.script(vec![
        Action::Ask {
            method: "_orch/question/ask",
            params: json!({ "sessionId": "$SESSION", "callId": "q1", "questions": [
                { "id": "db", "question": "Which database?", "options": [ { "label": "Postgres" }, { "label": "MySQL" } ] } ] }),
            tag: "questions",
        },
        Action::Ask {
            method: "_orch/plan/review",
            params: json!({ "sessionId": "$SESSION", "callId": "p1", "plan": "# Move the tests\n1. a" }),
            tag: "plan",
        },
    ]);
    let mut rig =
        Rig::start_plugin(plugin_manifest(&["questions", "plan_mode"]), agent.clone()).await;
    assert!(rig.started.descriptor.capabilities.plan_mode);
    let session = rig.session().await;
    let mut turn = rig.turn_of(&session, "plan it");
    turn.plan = true;
    rig.adapter.start_turn(turn).await.unwrap();
    assert_eq!(
        agent.frames("_orch/plan/set")[0]["params"]["active"],
        true,
        "the plugin is told before the prompt"
    );
    let asked = rig.until(|event| event.is("approval.requested")).await;
    let question = &asked.last().unwrap().body;
    assert_eq!(question["request"]["request_kind"], "user_input");
    let reference = question["request_ref"].as_str().unwrap().to_owned();
    rig.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Answers {
                answers: [("db".to_owned(), vec!["MySQL".to_owned()])].into(),
                note: None,
            },
        )
        .await
        .unwrap();
    let plan = rig.until(|event| event.is("approval.requested")).await;
    let card = &plan.last().unwrap().body;
    assert_eq!(card["request"]["request_kind"], "plan");
    assert_eq!(card["request"]["options"][1]["label"], "Keep planning");
    let reference = card["request_ref"].as_str().unwrap().to_owned();
    rig.adapter
        .resolve_approval(
            session.thread,
            &reference,
            ApprovalDecision::Option {
                option_id: "keep_planning".into(),
                note: Some("use sqlite".into()),
            },
        )
        .await
        .unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    assert_eq!(
        agent.answer("questions").unwrap(),
        json!({ "answers": [ { "id": "db", "selected": ["MySQL"] } ] })
    );
    assert_eq!(
        agent.answer("plan").unwrap(),
        json!({ "decision": "keep_planning", "feedback": "use sqlite" })
    );
    // The next turn is no plan: the plugin is told so, once.
    rig.say(&session, "go on").await;
    let sets = agent.frames("_orch/plan/set");
    assert_eq!(sets.len(), 2);
    assert_eq!(sets[1]["params"]["active"], false);
}

#[tokio::test]
async fn a_plugin_that_asks_before_changes_is_told_how_each_session_is_to_treat_them_and_only_when_it_changes()
 {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello = Some(hello(json!({ "approvals": on() })));
    let mut rig = Rig::start_plugin(plugin_manifest(&["approvals"]), agent.clone()).await;
    let session = rig.session().await;
    rig.say(&session, "one").await;
    rig.say(&session, "two").await;
    let mut third = rig.turn_of(&session, "three");
    third.approval_mode = orch_agent_contract::descriptor::ApprovalMode::Bypass;
    rig.adapter.start_turn(third).await.unwrap();
    rig.until(|event| event.is("turn.completed")).await;
    let modes: Vec<String> = agent
        .frames("_orch/approval/set")
        .iter()
        .map(|frame| frame["params"]["mode"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(modes, ["ask", "bypass"]);
}

#[tokio::test]
async fn a_neutral_history_is_written_into_the_program_and_the_session_it_names_is_resumed() {
    for through in [None, Some(3)] {
        let agent = Agent::new();
        agent.behavior.lock().unwrap().hello = Some(hello(json!({ "native_carry": on() })));
        let mut rig = Rig::start_plugin(plugin_manifest(&["native_carry"]), agent.clone()).await;
        assert!(rig.started.descriptor.capabilities.import_neutral);
        let mut spec = rig.spec(SessionMode::ImportNeutral);
        spec.history = Some(orch_agent_contract::session::LocalFile {
            path: "/tmp/history.jsonl".into(),
            size: 10,
        });
        spec.settings.upto_turn_ordinal = through;
        let opened = rig.adapter.open_session(spec).await.unwrap();
        assert!(
            !opened.resumed,
            "an import begins a conversation of its own"
        );
        assert_eq!(opened.native_session_ref, "imported-1");
        assert_eq!(opened.import_report.unwrap().items_total, 4);
        let import = &agent.frames("_orch/history/import")[0]["params"];
        assert_eq!(import["format"], "orch-neutral-jsonl-v1");
        assert_eq!(import["path"], "/tmp/history.jsonl");
        if let Some(through) = through {
            assert_eq!(import["throughTurn"], through);
        } else {
            assert!(
                import.get("throughTurn").is_none(),
                "a whole history omits its turn limit"
            );
        }
        assert_eq!(import["provider"], "orch");
        assert_eq!(
            agent.frames("session/resume")[0]["params"]["sessionId"],
            "imported-1"
        );
    }
}

#[tokio::test]
async fn a_history_larger_than_the_program_takes_in_one_go_is_refused_with_the_way_out_before_the_plugin_is_asked()
 {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello = Some(hello(json!({ "native_carry": on() })));
    let mut rig = Rig::start_plugin(plugin_manifest(&["native_carry"]), agent.clone()).await;
    let mut spec = rig.spec(SessionMode::ImportNeutral);
    spec.history = Some(orch_agent_contract::session::LocalFile {
        path: "/tmp/history.jsonl".into(),
        size: 100 * 1024 * 1024,
    });
    let refused = rig.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(refused.code(), "unsupported");
    let said = refused.to_string();
    assert!(said.contains("summary or as a file"), "{said}");
    assert!(
        agent.frames("_orch/history/import").is_empty(),
        "the plugin was not asked to read it"
    );
}

#[tokio::test]
async fn a_plugin_revises_a_plan_and_proceeds_without_replacing_the_composers_approval_mode() {
    use orch_agent_contract::descriptor::ApprovalMode;
    for (mode, expected) in [(ApprovalMode::Ask, "ask"), (ApprovalMode::Bypass, "bypass")] {
        let agent = Agent::new();
        agent.behavior.lock().unwrap().hello =
            Some(hello(json!({ "approvals": on(), "plan_mode": on() })));
        agent.script(vec![
            Action::Ask {
                method: "_orch/plan/review",
                params: json!({ "sessionId": "$SESSION", "callId": "first", "plan": "Read, then change" }),
                tag: "first-plan",
            },
            Action::Ask {
                method: "_orch/plan/review",
                params: json!({ "sessionId": "$SESSION", "callId": "revised", "plan": "Read, explain rollback, then change" }),
                tag: "revised-plan",
            },
        ]);
        let mut rig =
            Rig::start_plugin(plugin_manifest(&["approvals", "plan_mode"]), agent.clone()).await;
        let session = rig.session().await;
        let mut turn = rig.turn_of(&session, "Plan the change");
        turn.plan = true;
        turn.approval_mode = mode;
        rig.adapter.start_turn(turn).await.unwrap();
        let events = rig.until(|event| event.is("approval.requested")).await;
        let first = &events.last().unwrap().body;
        assert_eq!(first["request"]["request_kind"], "plan");
        let reference = first["request_ref"].as_str().unwrap().to_owned();
        rig.adapter
            .resolve_approval(
                session.thread,
                &reference,
                ApprovalDecision::Option {
                    option_id: "keep_planning".into(),
                    note: Some("Explain rollback first. 请先读配置。".into()),
                },
            )
            .await
            .unwrap();
        let events = rig.until(|event| event.is("approval.requested")).await;
        let revised = &events.last().unwrap().body;
        assert_eq!(revised["request"]["request_kind"], "plan");
        assert_eq!(
            revised["request"]["detail"]["plan"],
            "Read, explain rollback, then change"
        );
        assert_ne!(revised["request_ref"].as_str().unwrap(), reference);
        assert_eq!(
            agent.frames("_orch/plan/set").len(),
            1,
            "keeping a plan leaves planning active"
        );
        rig.adapter
            .resolve_approval(
                session.thread,
                revised["request_ref"].as_str().unwrap(),
                ApprovalDecision::Option {
                    option_id: "proceed".into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        rig.until(|event| event.is("turn.completed")).await;
        assert_eq!(
            agent.answer("first-plan").unwrap(),
            json!({
                "decision": "keep_planning", "feedback": "Explain rollback first. 请先读配置。"
            })
        );
        assert_eq!(
            agent.answer("revised-plan").unwrap(),
            json!({ "decision": "proceed" })
        );
        let mut ordinary = rig.turn_of(&session, "Implement the approved plan");
        ordinary.approval_mode = mode;
        rig.adapter.start_turn(ordinary).await.unwrap();
        rig.until(|event| event.is("turn.completed")).await;
        let plans = agent.frames("_orch/plan/set");
        assert_eq!(plans.len(), 2);
        assert_eq!(plans[0]["params"]["active"], true);
        assert_eq!(plans[1]["params"]["active"], false);
        let modes = agent.frames("_orch/approval/set");
        assert_eq!(
            modes.len(),
            1,
            "plan choices cannot replace the composer's approval mode"
        );
        assert_eq!(modes[0]["params"]["mode"], expected);
    }
}

#[tokio::test]
async fn an_import_refusal_names_the_failed_history_and_recovery_instead_of_an_unavailable_program()
{
    let agent = Agent::new();
    {
        let mut behavior = agent.behavior.lock().unwrap();
        behavior.hello = Some(hello(json!({ "native_carry": on() })));
        behavior.fails_import = Some((-32602, "the history file is not valid JSON".into()));
    }
    let mut rig = Rig::start_plugin(plugin_manifest(&["native_carry"]), agent.clone()).await;
    let mut spec = rig.spec(SessionMode::ImportNeutral);
    spec.history = Some(orch_agent_contract::session::LocalFile {
        path: "/tmp/history.jsonl".into(),
        size: 10,
    });
    let refused = rig.adapter.open_session(spec).await.unwrap_err();
    assert_eq!(refused.code(), "history_import_failed");
    assert!(!refused.retryable());
    let detail = refused.detail();
    assert!(detail.contains("Summary or None"), "{detail}");
    assert!(
        detail.contains("the history file is not valid JSON"),
        "{detail}"
    );
    assert!(!detail.contains("register it"), "{detail}");
    assert!(
        agent.frames("session/resume").is_empty(),
        "a refused import cannot resume another session"
    );
}

/// The program's process has ended and the thread's next message opens its session again, by the id the agent gave
/// it, in a new process of the program.
async fn start_again(rig: &mut Rig, session: &Opened) {
    rig.adapter.close_session(session.thread).await.unwrap();
    let mut spec = rig.spec(SessionMode::Resume);
    spec.settings.thread_id = session.thread;
    spec.settings.segment_id = session.segment;
    spec.settings.native_session_ref = Some(session.native.clone());
    rig.adapter.open_session(spec).await.unwrap();
    rig.until(|event| event.is("session.opened")).await;
}

/// Whether Orch would show a request: it keeps one approval for each reference a thread was ever given, and takes a
/// second request with a reference it has seen for the first one again, which it does not show.
fn is_new_to_orch(known: &mut std::collections::HashSet<String>, reference: &str) -> bool {
    known.insert(reference.to_owned())
}

#[tokio::test]
async fn the_first_question_of_a_program_that_was_started_again_is_a_request_orch_has_not_seen() {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().hello = Some(hello(json!({ "questions": on() })));
    let ask = |tag: &'static str| Action::Ask {
        method: "_orch/question/ask",
        params: json!({ "sessionId": "$SESSION", "callId": "q1", "questions": [
            { "id": "colour", "question": "Which colour?", "options": [ { "label": "Blue" }, { "label": "Green" } ] } ] }),
        tag,
    };
    agent.script(vec![ask("first")]);
    agent.script(vec![ask("second")]);
    let mut rig = Rig::start_plugin(plugin_manifest(&["questions"]), agent.clone()).await;
    let session = rig.session().await;
    let mut known = std::collections::HashSet::new();
    for (round, (tag, choice)) in [("first", "Blue"), ("second", "Green")]
        .into_iter()
        .enumerate()
    {
        if round == 1 {
            start_again(&mut rig, &session).await;
        }
        rig.begin(&session, "ask me").await;
        let asked = rig.until(|event| event.is("approval.requested")).await;
        let reference = asked.last().unwrap().body["request_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            is_new_to_orch(&mut known, &reference),
            "{reference} was the reference of an earlier request of this thread, and Orch would show no card for it"
        );
        rig.adapter
            .resolve_approval(
                session.thread,
                &reference,
                ApprovalDecision::Answers {
                    answers: [("colour".to_owned(), vec![choice.to_owned()])].into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        rig.until(|event| event.is("turn.completed")).await;
        assert_eq!(
            agent.answer(tag).unwrap(),
            json!({ "answers": [ { "id": "colour", "selected": [choice] } ] }),
            "the answer of round {round} reaches the program"
        );
    }
    assert_eq!(
        agent.processes(),
        3,
        "the probe, the run, and the run after it was started again"
    );
}

#[tokio::test]
async fn a_request_for_a_tool_call_the_program_names_again_after_it_was_started_again_is_not_one_orch_has_seen()
 {
    let agent = Agent::new();
    // A program that counts its calls from one in each process names the first call of the new process as before.
    for _ in 0..2 {
        agent.script(vec![
            tool_call(
                "call_1",
                "bash",
                json!({ "command": "rm -rf build", "description": "clean" }),
            ),
            permission("call_1", "perm"),
            tool_done("call_1", "completed", "gone"),
        ]);
    }
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let mut known = std::collections::HashSet::new();
    for round in 0..2 {
        if round == 1 {
            start_again(&mut rig, &session).await;
        }
        rig.begin(&session, "clean it").await;
        let asked = rig.until(|event| event.is("approval.requested")).await;
        let reference = asked.last().unwrap().body["request_ref"]
            .as_str()
            .unwrap()
            .to_owned();
        assert!(
            is_new_to_orch(&mut known, &reference),
            "{reference} was the reference of an earlier request of this thread, and Orch would show no card for it"
        );
        rig.adapter
            .resolve_approval(
                session.thread,
                &reference,
                ApprovalDecision::Option {
                    option_id: "allow-once".into(),
                    note: None,
                },
            )
            .await
            .unwrap();
        rig.until(|event| event.is("turn.completed")).await;
    }
}

#[tokio::test]
async fn a_program_that_ends_between_turns_is_lost_by_how_it_ended_and_not_by_a_note_it_wrote_as_it_started()
 {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().startup_note = Some(
        "orch-dsh-bridge: executor_backends is off: the arrangement is not separated".to_owned(),
    );
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    rig.say(&session, "one").await;
    agent.end_process();
    let lost = rig
        .until(|event| event.is("session.lost"))
        .await
        .pop()
        .unwrap();
    let reason = lost.body["reason"].as_str().unwrap();
    assert!(
        reason.contains("the program exited with code 0"),
        "the reason names how the program ended: {reason}"
    );
    assert!(
        !reason.contains("executor_backends"),
        "a note from the start of the program is not why it ended: {reason}"
    );
}

#[tokio::test]
async fn a_program_that_dies_in_a_turn_is_lost_by_how_it_ended_and_what_it_wrote_as_it_died() {
    let agent = Agent::new();
    agent.behavior.lock().unwrap().startup_note = Some("a part of the program is off".to_owned());
    agent.script(vec![
        message("agent_message_chunk", "m1", "half"),
        Action::Stderr("panicked at agent.rs:7: no room"),
        Action::Sleep(100),
        Action::Die,
    ]);
    let mut rig = Rig::start(program(json!({})), agent.clone()).await;
    let session = rig.session().await;
    let mut events = rig.say(&session, "go").await;
    if !events.iter().any(|event| event.is("session.lost")) {
        events.extend(rig.until(|event| event.is("session.lost")).await);
    }
    let failed = events
        .iter()
        .find(|event| event.is("turn.completed"))
        .unwrap();
    let lost = events
        .iter()
        .find(|event| event.is("session.lost"))
        .unwrap();
    for (what, text) in [
        (
            "the failure of the turn",
            failed.body["error"]["message"].as_str().unwrap(),
        ),
        (
            "the loss of the session",
            lost.body["reason"].as_str().unwrap(),
        ),
    ] {
        assert!(
            text.contains("the program exited with code 0"),
            "{what} names how the program ended: {text}"
        );
        assert!(
            text.contains("panicked at agent.rs:7"),
            "{what} shows what the program wrote as it died: {text}"
        );
        assert!(
            !text.contains("a part of the program is off"),
            "{what} does not show what the program wrote as it started: {text}"
        );
    }
}
