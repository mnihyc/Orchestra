//! The conversation's own glue around the settler of a sub-agent's requests, driven as it runs, on the scripted
//! program of the corpus: the outbox, the gate that holds a turn's completion behind the reports of its
//! sub-agents, the flush when the conversation is closed, and what is done when the program is lost.

use std::path::Path;
use std::time::Duration;

use crate::common::corpus::{
    Actor, SESSION, Scripts, WAIT, barrier, from_native, handshake, input, to_native,
};
use orch_adapter_claude_code::config::ProfileConfig;
use orch_adapter_claude_code::sessions::Origin;
use orch_testkit::harness_corpus::{Bindings, Frame, Observation, collect_through};
use serde_json::json;

fn settings(wait_ms: u64) -> ProfileConfig {
    ProfileConfig::from_json(&json!({ "child_usage_wait_ms": wait_ms }))
}

/// The transcript of a sub-agent, where the program keeps it.
fn put_transcript(home: &Path, agent: &str, message: &str, input: i64, output: i64) {
    let directory = home.join("projects/orch").join(SESSION).join("subagents");
    std::fs::create_dir_all(&directory).unwrap();
    let line = json!({
        "type":"assistant","isSidechain":true,"agentId":agent,
        "message":{"id":message,"role":"assistant","stop_reason":"end_turn",
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":output}}
    });
    std::fs::write(
        directory.join(format!("agent-{agent}.jsonl")),
        format!("{line}\n"),
    )
    .unwrap();
}

/// A turn that starts a background sub-agent.
fn launch() -> Vec<Frame> {
    vec![
        from_native(
            json!({"type":"assistant","message":{"id":"main-1","content":[{"type":"tool_use","id":"tool","name":"Agent","input":{"description":"Review","prompt":"Read the code","run_in_background":true,"subagent_type":"general-purpose"}}]}}),
        ),
        from_native(
            json!({"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"tool","content":"launched"}]},"tool_use_result":{"status":"async_launched","agentId":"task"}}),
        ),
        from_native(
            json!({"type":"system","subtype":"task_started","session_id":SESSION,"task_id":"task","run_id":"run-1","tool_use_id":"tool","task_type":"local_agent","is_backgrounded":true}),
        ),
    ]
}

/// The complete message of the sub-agent as the program prints it: before the model's closing usage.
fn printed(agent: &str, message: &str, input: i64) -> Frame {
    from_native(
        json!({"type":"assistant","parent_tool_use_id":"tool","agent_id":agent,"session_id":SESSION,
        "message":{"id":message,"model":"mock-large","content":[{"type":"text","text":"a"}],"stop_reason":null,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}}),
    )
}

/// The end of the turn, while the sub-agent goes on.
fn ending() -> Vec<Frame> {
    vec![
        from_native(
            json!({"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"output_tokens":2},"modelUsage":{"mock-large":{"inputTokens":10,"outputTokens":2}}}),
        ),
        to_native(
            json!({"type":"control_request","request_id":{"$replay_capture":"context"},"request":{"subtype":"get_context_usage","detail":"summary"}}),
        ),
        from_native(
            json!({"type":"control_response","response":{"subtype":"error","request_id":{"$replay_ref":"context"},"error":"fixture has no context breakdown"}}),
        ),
    ]
}

fn scripts(frames: Vec<Frame>) -> std::sync::Arc<Scripts> {
    Scripts::new(vec![frames], orch_testkit::clock::manual(), Bindings::new())
}

fn subagent_request(row: &Observation) -> bool {
    row.name == "model.request" && row.body["request"]["subagent"] == true
}

/// Reads until the sub-agent's request and the loss of the session have both been reported.
async fn until_request_and_loss(actor: &mut Actor) -> Vec<Observation> {
    let mut rows = Vec::new();
    tokio::time::timeout(WAIT, async {
        while !(rows.iter().any(subagent_request)
            && rows.iter().any(|row| row.name == "session.lost"))
        {
            let event = actor.events.recv().await.expect("the event stream closed");
            rows.push(Observation::from_event(&event).unwrap());
        }
    })
    .await
    .expect("the request and the loss were reported");
    rows
}

fn drained(actor: &mut Actor) -> Vec<Observation> {
    let mut rows = Vec::new();
    while let Ok(event) = actor.events.try_recv() {
        rows.push(Observation::from_event(&event).unwrap());
    }
    rows
}

#[tokio::test]
async fn a_turns_completion_goes_out_after_the_reports_of_its_sub_agents_though_the_consumer_is_slow()
 {
    let mut frames = handshake();
    frames.push(input());
    // Enough events to fill the lane to the host, so that the conversation waits for its consumer.
    for n in 0..80 {
        frames.push(from_native(json!({"type":"assistant","message":{"id":format!("filler-{n}"),"role":"assistant","content":[{"type":"text","text":"x"}]}})));
    }
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(200)).await;
    actor.opened.as_ref().unwrap();
    put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
    // Nothing is read for longer than the budget and its slack together.
    tokio::time::sleep(Duration::from_millis(1500)).await;
    let rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    let requests: Vec<_> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| subagent_request(row))
        .collect();
    assert_eq!(
        requests.len(),
        1,
        "the sub-agent's request is reported once"
    );
    let completed = rows
        .iter()
        .position(|row| row.name == "turn.completed")
        .unwrap();
    assert!(
        requests[0].0 < completed,
        "the request is reported before the turn is: {} before {completed}",
        requests[0].0
    );
    assert_eq!(
        requests[0].1.body["request"]["tokens"]["output_tokens"], 5,
        "with the figure of its transcript"
    );
    actor.close().await;
    assert!(
        !drained(&mut actor).iter().any(subagent_request),
        "nothing is reported for it after the turn"
    );
}

#[tokio::test]
async fn closing_the_conversation_sends_what_waits_behind_a_gate_before_it_answers() {
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.extend(barrier());
    // The session has no transcripts: the request is reported when its time has run out, and the completion of
    // the turn waits for that.
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    actor.scripts.completed().await;
    let mut rows = drained(&mut actor);
    assert!(
        !rows.iter().any(|row| row.name == "turn.completed"),
        "the completion waits for the reports: {:?}",
        rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>()
    );
    actor.close().await;
    rows.extend(drained(&mut actor));
    let completed = rows
        .iter()
        .position(|row| row.name == "turn.completed")
        .expect("Close answers after the completion has gone out");
    let request = rows
        .iter()
        .position(subagent_request)
        .expect("and so has the report");
    assert!(request < completed);
}

#[tokio::test]
async fn a_program_that_is_lost_has_its_sub_agents_requests_read_and_reported_at_once() {
    for written in [true, false] {
        let mut frames = handshake();
        frames.push(input());
        frames.extend(launch());
        frames.push(printed("agent-a", "ma1", 11));
        let after = frames.len();
        frames.push(Frame::Exit { code: Some(1) });
        let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
        // The budget is longer than the test waits: nothing waits for a program that is gone.
        let mut actor =
            Actor::start_configured(scripts, Origin::New, false, false, settings(60_000)).await;
        actor.opened.as_ref().unwrap();
        gate.reached().await;
        if written {
            put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
        }
        gate.release();
        let rows = until_request_and_loss(&mut actor).await;
        let request = rows.iter().position(subagent_request).unwrap();
        assert_eq!(
            rows[request].body["request"]["tokens"]["output_tokens"],
            if written { 5 } else { 1 },
            "from the file of a program that has exited, or as the stream said it"
        );
        actor.close().await;
    }
}

/// The program asks to use a tool.
fn ask(request: &str) -> Frame {
    from_native(
        json!({"type":"control_request","request_id":request,"request":{"subtype":"can_use_tool",
        "tool_name":"FutureTool","tool_use_id":format!("future-{request}"),"input":{"operation":"fixture"},
        "description":"Fixture permission"}}),
    )
}

fn names(rows: &[Observation]) -> Vec<&str> {
    rows.iter().map(|row| row.name.as_str()).collect()
}

#[tokio::test]
async fn a_question_taken_back_when_its_turn_ends_reaches_the_engine_though_the_completion_waits() {
    // A question is open when the turn ends: the adapter lets it go with the turn and the engine must hear that at
    // once, or it answers what the adapter no longer holds ("there is no request to answer").
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.push(ask("ask-1"));
    frames.extend(ending());
    frames.push(to_native(
        json!({"type":"control_response","response":{"subtype":"success","request_id":"ask-1"}}),
    ));
    frames.extend(barrier());
    // The session has no transcripts, so the completion waits behind its gate for the budget.
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    actor.scripts.completed().await;
    let mut rows = drained(&mut actor);
    let seen = names(&rows);
    assert!(
        !seen.contains(&"turn.completed"),
        "the completion waits for the reports: {seen:?}"
    );
    let asked = seen.iter().position(|name| *name == "approval.requested");
    let closed = seen.iter().position(|name| *name == "approval.closed");
    assert!(
        asked.is_some() && closed.is_some() && asked < closed,
        "the question and its being taken back are not held behind the gate: {seen:?}"
    );
    actor.close().await;
    rows.extend(drained(&mut actor));
    assert!(rows.iter().any(|row| row.name == "turn.completed"));
}

#[tokio::test]
async fn a_question_asked_while_the_completion_waits_reaches_the_engine_at_once() {
    // A sub-agent that goes on asks after the turn has ended, while the completion waits for the reports.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.push(ask("ask-2"));
    frames.push(from_native(
        json!({"type":"control_cancel_request","request_id":"ask-2"}),
    ));
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    actor.scripts.completed().await;
    let rows = drained(&mut actor);
    let seen = names(&rows);
    assert!(
        !seen.contains(&"turn.completed"),
        "the completion waits for the reports: {seen:?}"
    );
    let asked = seen.iter().position(|name| *name == "approval.requested");
    let closed = seen.iter().position(|name| *name == "approval.closed");
    assert!(
        asked.is_some() && closed.is_some() && asked < closed,
        "a question does not wait behind the gate: {seen:?}"
    );
    actor.close().await;
}
