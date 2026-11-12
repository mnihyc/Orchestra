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
            json!({"type":"result","subtype":"success","is_error":false,"usage":{"input_tokens":10,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":2},"modelUsage":{"mock-large":{"inputTokens":21,"outputTokens":3}}}),
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
async fn a_turns_completion_waits_for_the_settler_up_to_the_budget_and_not_for_a_clock() {
    // The budget is half a minute and the transcript shows the request after a second and a half. A gate that a
    // clock opened (the second of slack of an earlier design) would let the completion go before the report; this one
    // goes after it, with the figure of the transcript, as soon as the transcript shows it. The budget is twenty
    // times the wait of the case, so that how fast the run is does not decide it.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(30_000)).await;
    actor.opened.as_ref().unwrap();
    actor.scripts.completed().await;
    tokio::time::sleep(Duration::from_millis(1_500)).await;
    let early = drained(&mut actor);
    assert!(
        !early
            .iter()
            .any(|row| row.name == "turn.completed" || subagent_request(row)),
        "nothing of the turn's end is out before the transcript shows the request: {:?}",
        names(&early)
    );
    put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
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
    assert!(
        requests[0].0 < rows.len() - 1,
        "the request is reported before the turn is"
    );
    assert_eq!(
        requests[0].1.body["request"]["tokens"]["output_tokens"], 5,
        "with the figure of its transcript, not the one the stream gave"
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
async fn a_program_that_is_lost_has_the_requests_of_its_windows_read_and_reported_at_once() {
    for written in [true, false] {
        let mut frames = handshake();
        frames.push(input());
        frames.extend(launch());
        frames.push(printed("agent-a", "ma1", 11));
        // The result names the request for its window; the program is lost with the turn's completion held behind it.
        frames.extend(ending());
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

#[tokio::test]
async fn a_request_whose_window_was_never_decided_when_the_program_is_lost_is_not_a_row() {
    // The program is lost while it writes a message of a sub-agent, before any result names it: the cost of a message
    // that ended after the last result is in no total that was reported, and the request is not reported as a row
    // (a closing line read after the last result cannot tell whether the message ended before it or after).
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    let after = frames.len();
    frames.push(Frame::Exit { code: Some(1) });
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(60_000)).await;
    actor.opened.as_ref().unwrap();
    gate.reached().await;
    put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
    gate.release();
    let mut rows = Vec::new();
    tokio::time::timeout(WAIT, async {
        while !rows.iter().any(|row: &Observation| row.name == "session.lost") {
            let event = actor.events.recv().await.expect("the event stream closed");
            rows.push(Observation::from_event(&event).unwrap());
        }
    })
    .await
    .expect("the loss was reported");
    tokio::time::sleep(Duration::from_millis(300)).await;
    rows.extend(drained(&mut actor));
    assert!(
        !rows.iter().any(subagent_request),
        "no row for a request that no window holds: {:?}",
        names(&rows)
    );
    actor.close().await;
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

// ---- more than one turn

/// The transcript of a sub-agent with several messages, each closed.
fn put_transcripts(home: &Path, agent: &str, messages: &[(&str, i64, i64)]) {
    let directory = home.join("projects/orch").join(SESSION).join("subagents");
    std::fs::create_dir_all(&directory).unwrap();
    let lines: String = messages
        .iter()
        .map(|(message, input, output)| {
            format!(
                "{}\n",
                json!({
                    "type":"assistant","isSidechain":true,"agentId":agent,
                    "message":{"id":message,"role":"assistant","stop_reason":"end_turn",
                        "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":output}}
                })
            )
        })
        .collect();
    std::fs::write(directory.join(format!("agent-{agent}.jsonl")), lines).unwrap();
}

/// The end of a turn: its own use (`usage`) and the running totals of the program (`modelUsage`), then what the
/// adapter asks for after it.
fn result_with(main: (i64, i64), totals: (i64, i64), name: &str) -> Vec<Frame> {
    vec![
        from_native(json!({"type":"result","subtype":"success","is_error":false,
            "usage":{"input_tokens":main.0,"cache_read_input_tokens":0,"cache_creation_input_tokens":0,"output_tokens":main.1},
            "modelUsage":{"mock-large":{"inputTokens":totals.0,"outputTokens":totals.1}}})),
        to_native(
            json!({"type":"control_request","request_id":{"$replay_capture":name},
            "request":{"subtype":"get_context_usage","detail":"summary"}}),
        ),
        from_native(
            json!({"type":"control_response","response":{"subtype":"error",
            "request_id":{"$replay_ref":name},"error":"fixture has no context breakdown"}}),
        ),
    ]
}

fn user_input(turn: orch_types::Id, text: &str) -> Frame {
    to_native(
        json!({"type":"user","uuid":turn,"message":{"role":"user","content":[{"type":"text","text":text}]}}),
    )
}

/// The use a turn was reported to have: `usage.updated` of the turn.
fn use_of(rows: &[Observation], turn: orch_types::Id) -> (i64, i64) {
    let row = rows
        .iter()
        .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap_or_else(|| panic!("the use of the turn was reported: {:?}", names(rows)));
    (
        row.body["turn"]["total_tokens"].as_i64().unwrap(),
        row.body["thread"]["total_tokens"].as_i64().unwrap(),
    )
}

/// The input and the output of the use a turn was reported to have.
fn classes_of(rows: &[Observation], turn: orch_types::Id) -> (i64, i64) {
    let row = rows
        .iter()
        .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap_or_else(|| panic!("the use of the turn was reported: {:?}", names(rows)));
    (
        row.body["turn"]["input_tokens"].as_i64().unwrap(),
        row.body["turn"]["output_tokens"].as_i64().unwrap(),
    )
}

/// What a use says it is carried by rows, apart from the use itself.
fn stated_of(rows: &[Observation], turn: orch_types::Id) -> (i64, i64) {
    let row = rows
        .iter()
        .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap_or_else(|| panic!("the use of the turn was reported: {:?}", names(rows)));
    (
        row.body["report"]["carried_by_rows"]["input_tokens"]
            .as_i64()
            .unwrap_or(0),
        row.body["report"]["carried_by_rows"]["output_tokens"]
            .as_i64()
            .unwrap_or(0),
    )
}

/// The parts of a use that are counted apart: the input, the part of it read from a cache, the part written to one,
/// and the output.
type Classes = (i64, i64, i64, i64);

fn classes_in(tokens: &serde_json::Value) -> Classes {
    let part = |name: &str| tokens[name].as_i64().unwrap_or(0);
    (
        part("input_tokens"),
        part("cached_input_tokens"),
        part("cache_write_tokens"),
        part("output_tokens"),
    )
}

/// What the engine counts over the turns, per class, from what was reported alone: each turn's use less the figure it
/// states its rows carry of it, and every row the engine would count (each request once, as its first report with
/// figures said it), whenever it came.
fn engine_counts(rows: &[Observation], turns: &[orch_types::Id]) -> Classes {
    let mut counted = (0, 0, 0, 0);
    let mut add = |more: Classes, less: Classes| {
        counted.0 += (more.0 - less.0).max(0);
        counted.1 += (more.1 - less.1).max(0);
        counted.2 += (more.2 - less.2).max(0);
        counted.3 += (more.3 - less.3).max(0);
    };
    for turn in turns {
        let row = rows
            .iter()
            .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
            .unwrap_or_else(|| panic!("the use of the turn was reported: {:?}", names(rows)));
        add(
            classes_in(&row.body["turn"]),
            classes_in(&row.body["report"]["carried_by_rows"]),
        );
    }
    let mut seen = std::collections::HashSet::new();
    for row in rows.iter().filter(|row| row.name == "model.request") {
        let tokens = &row.body["request"]["tokens"];
        let has_figures = ["input_tokens", "output_tokens"]
            .iter()
            .any(|name| !tokens[name].is_null());
        if has_figures && seen.insert(row.body["request"]["id"].to_string()) {
            add(classes_in(tokens), (0, 0, 0, 0));
        }
    }
    counted
}

#[tokio::test]
async fn a_turns_use_leaves_out_what_the_sub_agents_of_other_turns_added_while_it_ran() {
    // A sub-agent started in turn A makes a second request between A and the turn B that follows. The totals of the
    // program grow by it in B's window, so B's use holds it; A counts it by the request it owns. Left in B's use it
    // is counted twice. Run both ways: the request settled before B ended, and while B ended.
    for settled_first in [true, false] {
        let turn_b = orch_types::new_id();
        let mut frames = handshake();
        frames.push(input());
        frames.extend(launch());
        frames.push(printed("agent-a", "ma1", 70));
        // A used 10 and 2 itself and the sub-agent 70 and 10: the totals are 80 and 12.
        frames.extend(result_with((10, 2), (80, 12), "context-a"));
        let after = frames.len();
        frames.push(printed("agent-a", "ma2", 90));
        frames.push(user_input(turn_b, "Second input"));
        // B used 20 and 3 itself; the totals grew by the sub-agent's 90 and 10 as well: 190 and 25.
        frames.extend(result_with((20, 3), (190, 25), "context-b"));
        frames.extend(barrier());
        let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
        let mut actor =
            Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
        actor.opened.as_ref().unwrap();
        put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10), ("ma2", 90, 10)]);
        let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap();
        gate.reached().await;
        gate.release();
        if settled_first {
            // Its request is reported before B begins.
            tokio::time::sleep(Duration::from_millis(700)).await;
        }
        actor.next_turn(turn_b, "Second input").await.unwrap();
        rows.extend(
            collect_through(&mut actor.events, "turn.completed", WAIT)
                .await
                .unwrap(),
        );
        let (a_use, _) = use_of(&rows, crate::common::corpus::turn());
        let (b_use, thread) = use_of(&rows, turn_b);
        // Each part is conserved, not only the whole: the engine counts the input and the output apart.
        assert_eq!(
            classes_of(&rows, crate::common::corpus::turn()),
            (80, 12),
            "A's input and output: its own and the sub-agent's first request"
        );
        assert_eq!(
            classes_of(&rows, turn_b),
            (20, 3),
            "B's input and output are its own: the sub-agent's second request is left out of both"
        );
        assert_eq!(a_use, 92, "A's use: its own 12 and the sub-agent's 80");
        assert_eq!(
            b_use,
            23,
            "B's use: its own 23 and not the sub-agent's 100 that A owns (settled first: {settled_first}): {:?}",
            names(&rows)
        );
        assert_eq!(
            thread, 215,
            "the thread's total is the growth of the totals"
        );
        let requests: Vec<_> = rows.iter().filter(|row| subagent_request(row)).collect();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|row| row.body["turn_id"] == json!(crate::common::corpus::turn())),
            "both belong to the turn that started the sub-agent"
        );
        let a = crate::common::corpus::turn();
        // A's use is its own 10 / 2 (the row of its main conversation) and the sub-agent's first request, 70 / 10.
        assert_eq!(
            stated_of(&rows, a),
            (80, 12),
            "A states what its own rows carry of its use"
        );
        assert_eq!(
            stated_of(&rows, turn_b),
            (20, 3),
            "B states its own row of the main conversation, and not the sub-agent's request it left out, which is A's"
        );
        // What the engine counts over the two turns is what the program's totals grew by, per class, whichever way
        // the rows came.
        assert_eq!(
            engine_counts(&rows, &[a, turn_b]),
            (190, 0, 0, 25),
            "conserved (settled first: {settled_first})"
        );
        actor.close().await;
    }
}

#[tokio::test]
async fn gates_of_two_turns_stacked_open_in_their_order_and_each_waits_for_its_own_window() {
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 70));
    frames.extend(result_with((10, 2), (80, 12), "context-a"));
    let after = frames.len();
    frames.push(user_input(turn_b, "Second input"));
    frames.push(printed("agent-a", "ma2", 90));
    frames.extend(result_with((20, 3), (190, 25), "context-b"));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    // The transcript holds the first request only: it is reported at its first look, a tenth of a second after it was
    // printed, and the second at the end of the wait, two seconds after: a margin of twenty to one, so that what the
    // case shows (A's gate opens on its own request and does not wait for B's) does not rest on the speed of the run.
    // A's gate is shut when B begins and ends, and the second request, which A owns and B's window names, goes to the
    // host when it is ready, which is after A's completion and before B's use (B's gate waits for it).
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(2_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10)]);
    gate.reached().await;
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    let at = |name: &str, turn: orch_types::Id| {
        rows.iter()
            .position(|row| row.name == name && row.body["turn_id"] == json!(turn))
            .unwrap_or_else(|| panic!("{name} of the turn: {:?}", names(&rows)))
    };
    let requests: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| subagent_request(row))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(requests.len(), 2);
    let a = crate::common::corpus::turn();
    assert!(requests[0] < at("usage.updated", a), "{:?}", names(&rows));
    assert!(at("usage.updated", a) < at("turn.completed", a));
    assert!(
        at("turn.completed", a) < requests[1],
        "A's completion did not wait for what was printed after it: {:?}",
        names(&rows)
    );
    assert!(requests[1] < at("usage.updated", turn_b));
    assert!(at("usage.updated", turn_b) < at("turn.completed", turn_b));
    actor.close().await;
}

#[tokio::test]
async fn a_full_outbox_behind_a_shut_gate_stops_the_reading_of_the_program_and_loses_nothing() {
    // The next turn begins while the gate of the first is shut, and says more than the outbox holds.
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    let after = frames.len();
    frames.push(user_input(turn_b, "Second input"));
    for n in 0..100 {
        frames.push(from_native(json!({"type":"assistant","message":{"id":format!("filler-{n}"),"role":"assistant","content":[{"type":"text","text":"x"}]}})));
    }
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(2_000)).await;
    actor.opened.as_ref().unwrap();
    gate.reached().await;
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    // The gate is shut for the budget (no transcript), and the program is not read past the lane while it is: the
    // barrier at the end of the script is not answered.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert!(
        tokio::time::timeout(Duration::from_millis(500), actor.scripts.completed())
            .await
            .is_err(),
        "the program was read to its end although the outbox was full and the gate shut"
    );
    let rows = actor.collect().await;
    let completed = rows
        .iter()
        .position(|row| row.name == "turn.completed")
        .unwrap();
    let request = rows.iter().position(subagent_request).unwrap();
    assert!(request < completed);
    let fillers: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| {
            row.name == "item.started" && row.body["native_ref"].to_string().contains("filler")
        })
        .map(|(at, _)| at)
        .collect();
    assert_eq!(fillers.len(), 100, "nothing of it was lost: {:?}", names(&rows));
    assert!(
        fillers.iter().all(|at| *at > completed),
        "everything of the second turn follows the completion of the first"
    );
    actor.close().await;
}

#[tokio::test]
async fn a_read_that_hangs_cannot_hold_the_completion_past_the_budget_in_the_real_conversation() {
    let wait = Duration::from_millis(600);
    let released = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.extend(barrier());
    let mut actor = Actor::start_with_files(
        scripts(frames),
        Origin::New,
        false,
        false,
        settings(600),
        std::sync::Arc::new(crate::Hanging {
            released: std::sync::Arc::clone(&released),
        }),
    )
    .await;
    actor.opened.as_ref().unwrap();
    put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
    actor.scripts.completed().await;
    let began = std::time::Instant::now();
    let rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    let waited = began.elapsed();
    assert!(
        waited + Duration::from_millis(250) >= wait && waited < wait + Duration::from_secs(3),
        "the completion left at the budget, not before and not much after: {waited:?}"
    );
    let request = rows.iter().find(|row| subagent_request(row)).unwrap();
    assert_eq!(
        request.body["request"]["tokens"]["output_tokens"], 1,
        "as the stream said it: no read answered"
    );
    released.store(true, std::sync::atomic::Ordering::SeqCst);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !drained(&mut actor).iter().any(subagent_request),
        "the answer of a read that was left changes nothing"
    );
    actor.close().await;
}

/// What the agent asks a person with its question tool.
fn a_question() -> serde_json::Value {
    json!({"questions":[{"question":"Which value?","header":"Choice","multiSelect":false,
        "options":[{"label":"First","description":"First value"},{"label":"Second","description":"Second value"}]}]})
}

/// The program asks to use the question tool the call `tool` names: the question is the call's.
fn ask_a_question(request: &str, tool: &str) -> Frame {
    from_native(
        json!({"type":"control_request","request_id":request,"request":{"subtype":"can_use_tool",
        "tool_name":"AskUserQuestion","tool_use_id":tool,"input":a_question(),
        "description":"Fixture question"}}),
    )
}

#[tokio::test]
async fn a_question_asked_after_the_completion_is_shown_after_the_call_it_names() {
    // The next turn makes a call, and asks to use it, while the completion of the first waits for the reports. The
    // question goes out at once, and the start of the call it names with it, before it.
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    let after = frames.len();
    frames.push(user_input(turn_b, "Second input"));
    frames.push(from_native(
        json!({"type":"assistant","message":{"id":"main-2","role":"assistant",
            "content":[{"type":"tool_use","id":"tool-y","name":"AskUserQuestion","input":a_question()}]}}),
    ));
    frames.push(ask_a_question("ask-4", "tool-y"));
    frames.push(from_native(
        json!({"type":"control_cancel_request","request_id":"ask-4"}),
    ));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    gate.reached().await;
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    actor.scripts.completed().await;
    let rows = drained(&mut actor);
    let seen = names(&rows);
    assert!(!seen.contains(&"turn.completed"), "{seen:?}");
    let asked = rows
        .iter()
        .position(|row| row.name == "approval.requested")
        .unwrap_or_else(|| panic!("the question went out: {seen:?}"));
    let call = rows
        .iter()
        .position(|row| row.name == "item.started" && row.body["native_ref"] == "tool-y")
        .unwrap_or_else(|| panic!("the call the question names went out: {seen:?}"));
    assert!(
        call < asked,
        "the call is shown before its question: {seen:?}"
    );
    actor.close().await;
}

// ---- a sub-agent's question and the end of its parent's turn

/// The program asks, for a sub-agent, to use the tool its call names.
fn ask_by(request: &str, agent: &str) -> Frame {
    from_native(
        json!({"type":"control_request","request_id":request,"request":{"subtype":"can_use_tool",
        "tool_name":"FutureTool","tool_use_id":format!("future-{request}"),"agent_id":agent,
        "input":{"operation":"fixture"},"description":"Fixture permission"}}),
    )
}

#[tokio::test]
async fn the_question_of_a_sub_agent_is_closed_to_the_engine_with_its_turn_and_a_late_answer_finds_nothing() {
    // The order the real program showed when a person's answer came too late: the sub-agent asks, its parent's turn
    // ends first, the question is refused to the program ("the turn ended") and closed to the engine, and only then
    // does the answer arrive, to be told that there is no such request. The engine cancels what is pending when a turn
    // ends, so nobody could answer the question after the turn in any case: the answer is an undelivered decision.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.push(ask_by("ask-5", "agent-a"));
    frames.extend(ending());
    frames.push(to_native(
        json!({"type":"control_response","response":{"subtype":"success","request_id":"ask-5",
        "response":{"behavior":"deny","message":"the turn ended"}}}),
    ));
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    actor.scripts.completed().await;
    let rows = drained(&mut actor);
    let seen = names(&rows);
    let asked = seen.iter().position(|name| *name == "approval.requested");
    let closed = seen.iter().position(|name| *name == "approval.closed");
    assert!(
        asked.is_some() && closed.is_some() && asked < closed,
        "the question of the sub-agent is closed to the engine with the turn: {seen:?}"
    );
    let late = actor
        .handle
        .ask(|reply| orch_adapter_claude_code::conversation::Command::Resolve {
            request_ref: "ask-5".into(),
            decision: orch_agent_contract::session::ApprovalDecision::Option {
                option_id: "allow_once".into(),
                note: None,
            },
            reply,
        })
        .await
        .unwrap();
    assert_eq!(late.unwrap_err().code(), "not_found");
    actor.close().await;
}

// ---- one figure for a turn, windows that never repeat, a settler that is gone, a message the program has not ended

/// As [`result_with`], with the breakdown of the context given as the answer, so that the use is said again after
/// the turn's own.
fn result_answering(main: (i64, i64), totals: (i64, i64), name: &str) -> Vec<Frame> {
    let mut frames = result_with(main, totals, name);
    frames.pop();
    frames.push(from_native(json!({"type":"control_response","response":{"subtype":"success",
        "request_id":{"$replay_ref":name},
        "response":{"categories":[{"name":"Messages","tokens":100,"kind":"used"}],
            "totalTokens":100,"maxTokens":200000}}})));
    frames
}

/// The use that every report of a turn's use holds, as the rows say it: the total of the turn.
fn uses_of(rows: &[Observation], turn: orch_types::Id) -> Vec<i64> {
    rows.iter()
        .filter(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .map(|row| row.body["turn"]["total_tokens"].as_i64().unwrap())
        .collect()
}

#[tokio::test]
async fn a_report_of_use_said_again_after_the_turns_own_is_the_one_that_went_out() {
    // The sub-agent of A makes a request in B's window; B's use leaves it out. The breakdown of the context, which the
    // harness answers after the turn's result, is carried with the use of the turn it was taken after, and that use
    // must be the one that went out, not the one the mapper kept before the request was left out: the engine keeps
    // what it heard last.
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 70));
    frames.extend(result_with((10, 2), (80, 12), "context-a"));
    let after = frames.len();
    frames.push(printed("agent-a", "ma2", 90));
    frames.push(user_input(turn_b, "Second input"));
    frames.extend(result_answering((20, 3), (190, 25), "context-b"));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10), ("ma2", 90, 10)]);
    let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    gate.reached().await;
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    actor.scripts.completed().await;
    rows.extend(drained(&mut actor));
    let uses = uses_of(&rows, turn_b);
    assert!(
        uses.len() >= 2,
        "the use of B was said at its end and again with the breakdown: {:?}",
        names(&rows)
    );
    assert!(
        uses.iter().all(|use_| *use_ == 23),
        "every report of B's use is the one that went out, with the sub-agent's request left out: {uses:?}"
    );
    let stated: Vec<_> = rows
        .iter()
        .filter(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn_b))
        .map(|row| row.body["report"]["carried_by_rows"].clone())
        .collect();
    assert!(
        stated
            .iter()
            .all(|each| !each.is_null() && *each == stated[0]),
        "every report of B's use says, the same, what B's rows carry of it: {stated:?}"
    );
    let with_breakdown = rows
        .iter()
        .filter(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn_b))
        .any(|row| !row.body["report"]["context"].is_null());
    assert!(with_breakdown, "one of them carries the breakdown");
    actor.close().await;
}

#[tokio::test]
async fn a_settler_that_is_gone_leaves_the_accounting_conserved_through_the_real_conversation() {
    // The settler fails between the turns. What the sub-agent of A makes then is no row (nothing reports it), and
    // B's window holds it: B's use is B's own and the request, which the growth counted once, in B's remainder. The
    // ledger the settler kept is still used for A's statement.
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 70));
    frames.extend(result_with((10, 2), (80, 12), "context-a"));
    frames.extend(barrier());
    let after = frames.len();
    frames.push(printed("agent-a", "ma2", 90));
    frames.push(user_input(turn_b, "Second input"));
    frames.extend(result_with((20, 3), (190, 25), "context-b"));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10)]);
    let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    gate.reached().await;
    actor.settler.crash();
    tokio::time::timeout(WAIT, async {
        while !actor.settler.is_gone() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the settler is gone");
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    let a = crate::common::corpus::turn();
    let requests: Vec<_> = rows.iter().filter(|row| subagent_request(row)).collect();
    assert_eq!(
        requests.len(),
        1,
        "only the request of A's window is a row: {:?}",
        names(&rows)
    );
    assert_eq!(classes_of(&rows, a), (80, 12));
    assert_eq!(stated_of(&rows, a), (80, 12), "the ledger of the settler is still used");
    assert_eq!(
        classes_of(&rows, turn_b),
        (110, 13),
        "B's use holds the sub-agent's second request, which no row reports"
    );
    assert_eq!(
        stated_of(&rows, turn_b),
        (20, 3),
        "B's own row of the main conversation, and no row for the request no one reported"
    );
    assert_eq!(
        engine_counts(&rows, &[a, turn_b]),
        (190, 0, 0, 25),
        "conserved with the settler gone: the request is counted once, in B's remainder"
    );
    actor.close().await;
}

#[tokio::test]
async fn the_numbers_of_windows_do_not_repeat_when_the_session_is_opened_again_after_its_process_ended() {
    // A's process ends with a request of A's sub-agent made after A's result and not named by a later one. The
    // session is opened again (a new mapper) and a new process serves B and C: C's use is its own, not less by what
    // an entry of the old numbering matched.
    let turn_b = orch_types::new_id();
    let turn_c = orch_types::new_id();
    let mut first = handshake();
    first.push(input());
    first.extend(launch());
    first.push(printed("agent-a", "ma1", 70));
    first.extend(result_with((10, 2), (80, 12), "context-a"));
    first.push(printed("agent-a", "ma2", 90));
    first.extend(barrier());
    first.push(Frame::Exit { code: Some(0) });
    let mut second = handshake();
    second.push(user_input(turn_b, "Second input"));
    // A sub-agent of B goes on, so that the process is not restarted for the next turn.
    second.extend(launch());
    second.extend(result_with((5, 1), (5, 1), "context-b"));
    second.push(user_input(turn_c, "Third input"));
    second.extend(result_with((30, 5), (35, 6), "context-c"));
    second.extend(barrier());
    let scripts = Scripts::new(
        vec![first, second],
        orch_testkit::clock::manual(),
        Bindings::new(),
    );
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(1_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10), ("ma2", 90, 10)]);
    // ma2 was printed after A's result and the process ended with it: no result named it, so it is no row.
    let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    actor.scripts.completed().await;
    // The process has ended, and the conversation has seen it: it says it is no longer running.
    tokio::time::timeout(WAIT, async {
        loop {
            let listed = actor
                .handle
                .ask(|reply| orch_adapter_claude_code::conversation::Command::Background { reply })
                .await
                .unwrap();
            if listed.state == orch_agent_contract::background::BackgroundState::Offline {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the conversation saw the process end");
    actor.reconfigure().await;
    actor.next_turn(turn_b, "Second input").await.unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    actor.next_turn(turn_c, "Third input").await.unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    let (b_use, _) = use_of(&rows, turn_b);
    assert_eq!(b_use, 6, "B's use is its own: nothing of the old process is left out of it");
    let (c_use, _) = use_of(&rows, turn_c);
    assert_eq!(
        c_use, 35,
        "C's use is its own: nothing of the earlier numbering is left out of it: {:?}",
        names(&rows)
    );
    actor.close().await;
}

/// A block of a message of a sub-agent that is a call.
fn printed_call(agent: &str, message: &str, input: i64, call: &str) -> Frame {
    from_native(
        json!({"type":"assistant","parent_tool_use_id":"tool","agent_id":agent,"session_id":SESSION,
        "message":{"id":message,"model":"mock-large",
            "content":[{"type":"tool_use","id":call,"name":"Read","input":{}}],"stop_reason":null,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}}),
    )
}

#[tokio::test]
async fn a_message_the_program_has_not_ended_when_a_result_comes_is_counted_in_the_window_that_holds_its_cost() {
    // The order the pinned program showed (a sub-agent of two blocks, the first printed, then the parent's result,
    // then the second block and the end of the message): the cost of the message is added to the totals when the
    // message ends, which is after the result. The first result's totals do not hold it; the next result's do.
    // The message belongs to the window that holds its cost: it is named, and reported, when the result of that window
    // is read, which is after the turn that owns it is complete; the turn of the later window leaves it out.
    let turn_b = orch_types::new_id();
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    // The first block of the sub-agent's message, then A's result: its totals are A's own.
    frames.push(printed("agent-a", "ma1", 70));
    frames.extend(result_with((10, 2), (10, 2), "context-a"));
    let after = frames.len();
    // The second block, the end of the message, a message of the sub-agent that is whole, B, and the result of B whose
    // totals hold both messages: 70 and 90 of input, 10 and 10 of output.
    frames.push(printed_call("agent-a", "ma1", 70, "call-1"));
    frames.push(printed("agent-a", "ma2", 90));
    frames.push(user_input(turn_b, "Second input"));
    frames.extend(result_with((20, 3), (190, 25), "context-b"));
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcripts(actor.home(), "agent-a", &[("ma1", 70, 10), ("ma2", 90, 10)]);
    // A is complete at once: its gate waits for nothing, since the message is not in A's window; the budget is five
    // seconds and nothing but that would hold it.
    let began = std::time::Instant::now();
    let mut rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    assert!(
        began.elapsed() < Duration::from_secs(3),
        "A did not wait for a message its totals do not hold: {:?}",
        began.elapsed()
    );
    gate.reached().await;
    gate.release();
    actor.next_turn(turn_b, "Second input").await.unwrap();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    let a = crate::common::corpus::turn();
    let (a_use, _) = use_of(&rows, a);
    let (b_use, thread) = use_of(&rows, turn_b);
    assert_eq!(a_use, 12, "A's use is its own");
    assert_eq!(b_use, 23, "B's use is its own: both messages are left out of it");
    assert_eq!(thread, 215);
    let completed = rows
        .iter()
        .position(|row| row.name == "turn.completed")
        .unwrap();
    let requests: Vec<usize> = rows
        .iter()
        .enumerate()
        .filter(|(_, row)| subagent_request(row))
        .map(|(at, _)| at)
        .collect();
    assert_eq!(requests.len(), 2, "{:?}", names(&rows));
    assert!(
        requests.iter().all(|at| *at > completed),
        "both are reported after the completion of A, which owns them: {:?}",
        names(&rows)
    );
    assert_eq!(
        stated_of(&rows, a),
        (10, 2),
        "A's use holds only the row of its main conversation: the messages were not ended"
    );
    assert_eq!(
        stated_of(&rows, turn_b),
        (20, 3),
        "B's use holds the messages, which are A's, and states only the row of its own main conversation"
    );
    assert_eq!(
        engine_counts(&rows, &[a, turn_b]),
        (190, 0, 0, 25),
        "conserved: A's own, the sub-agent's two messages as rows, and B's own"
    );
    actor.close().await;
}

#[tokio::test]
async fn a_question_of_a_sub_agent_answered_while_its_parents_turn_is_open_reaches_the_program_and_the_sub_agent_goes_on()
 {
    // The flow the product supports for a sub-agent that needs a person: the parent's turn does not end while the
    // sub-agent's question is open (the parent waits for the sub-agent's first step), the person answers, the answer
    // reaches the program, and the sub-agent is still running when the parent's turn is complete. No question is
    // taken back, and nothing depends on which of the answer and the end of the turn comes first.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.push(ask_by("ask-7", "agent-a"));
    let after = frames.len();
    frames.push(to_native(
        json!({"type":"control_response","response":{"subtype":"success","request_id":"ask-7",
        "response":{"behavior":"allow","updatedInput":{"operation":"fixture"}}}}),
    ));
    frames.extend(ending());
    frames.extend(barrier());
    let (scripts, gate) = Scripts::paused(frames, after, orch_testkit::clock::manual());
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcript(actor.home(), "agent-a", "ma1", 11, 5);
    let mut rows = collect_through(&mut actor.events, "approval.requested", WAIT)
        .await
        .unwrap();
    gate.reached().await;
    actor
        .handle
        .ask(|reply| orch_adapter_claude_code::conversation::Command::Resolve {
            request_ref: "ask-7".into(),
            decision: orch_agent_contract::session::ApprovalDecision::Option {
                option_id: "allow_once".into(),
                note: None,
            },
            reply,
        })
        .await
        .unwrap()
        .expect("the question is open while the parent's turn is");
    gate.release();
    rows.extend(
        collect_through(&mut actor.events, "turn.completed", WAIT)
            .await
            .unwrap(),
    );
    actor.scripts.completed().await;
    rows.extend(drained(&mut actor));
    assert!(
        !rows.iter().any(|row| row.name == "approval.closed"),
        "no question was taken back: {:?}",
        names(&rows)
    );
    let listed = actor
        .handle
        .ask(|reply| orch_adapter_claude_code::conversation::Command::Background { reply })
        .await
        .unwrap();
    assert_eq!(
        listed.items.len(),
        1,
        "the sub-agent is still running when its parent's turn is complete"
    );
    actor.close().await;
}

// ---- the main conversation's own requests are rows too

/// A request of the main conversation as the program streams it: the start with the input and its cache parts, a
/// text, the stop reason with the output, the end. The program's input is counted without the parts read from and
/// written to the cache (they are added to it).
fn main_request(id: &str, input: i64, read: i64, write: i64, output: i64) -> Vec<Frame> {
    vec![
        from_native(
            json!({"type":"stream_event","event":{"type":"message_start","message":{"id":id,"model":"mock-large",
                "usage":{"input_tokens":input,"cache_read_input_tokens":read,"cache_creation_input_tokens":write,"output_tokens":0}}}}),
        ),
        from_native(
            json!({"type":"stream_event","event":{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}}),
        ),
        from_native(
            json!({"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Working."}}}),
        ),
        from_native(
            json!({"type":"stream_event","event":{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":output}}}),
        ),
        from_native(json!({"type":"stream_event","event":{"type":"message_stop"}})),
    ]
}

/// The end of a turn that used several models: its own use, and the running totals by model, with the parts of the
/// input read from and written to the cache apart, as the program gives them.
fn result_by_model(main: [i64; 4], totals: &[(&str, [i64; 4])], name: &str) -> Vec<Frame> {
    let models: serde_json::Map<String, serde_json::Value> = totals
        .iter()
        .map(|(model, each)| {
            (
                (*model).to_owned(),
                json!({"inputTokens":each[0],"cacheReadInputTokens":each[1],"cacheCreationInputTokens":each[2],"outputTokens":each[3]}),
            )
        })
        .collect();
    let mut frames = result_with((0, 0), (0, 0), name);
    frames[0] = from_native(json!({"type":"result","subtype":"success","is_error":false,
        "usage":{"input_tokens":main[0],"cache_read_input_tokens":main[1],"cache_creation_input_tokens":main[2],"output_tokens":main[3]},
        "modelUsage":models}));
    frames
}

#[tokio::test]
async fn a_plain_turn_states_what_its_own_requests_carry_and_the_engine_counts_the_charge_once() {
    // Two requests of the main conversation and no sub-agent. The totals of the program grew by what the two used,
    // and the two are reported as rows: the use the turn says has them in it, and the turn says so, or the engine
    // would count them in its remainder and again as rows.
    let mut frames = handshake();
    frames.push(input());
    // The input is 100 and 180 with the cache parts, the output 10 and 20.
    frames.extend(main_request("main-r1", 60, 30, 10, 10));
    frames.extend(main_request("main-r2", 100, 70, 10, 20));
    frames.extend(result_by_model(
        [160, 100, 20, 30],
        &[("mock-large", [160, 100, 20, 30])],
        "context-a",
    ));
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    let rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    let turn = crate::common::corpus::turn();
    let with_figures: Vec<_> = rows
        .iter()
        .filter(|row| row.name == "model.request" && !row.body["request"]["tokens"]["input_tokens"].is_null())
        .collect();
    assert_eq!(with_figures.len(), 2, "both requests are reported: {:?}", names(&rows));
    let used = rows
        .iter()
        .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap();
    assert_eq!(classes_in(&used.body["turn"]), (280, 100, 20, 30), "what the totals grew by");
    assert_eq!(
        classes_in(&used.body["report"]["carried_by_rows"]),
        (280, 100, 20, 30),
        "the statement equals the sum of the two requests, class by class"
    );
    assert_eq!(
        engine_counts(&rows, &[turn]),
        (280, 100, 20, 30),
        "the remainder is what the totals hold beyond the two requests (nothing), and the total is the charge"
    );
    actor.close().await;
}

/// The complete message of a sub-agent of a model, as the program prints it.
fn printed_by(agent: &str, message: &str, input: i64, model: &str) -> Frame {
    from_native(
        json!({"type":"assistant","parent_tool_use_id":"tool","agent_id":agent,"session_id":SESSION,
        "message":{"id":message,"model":model,"content":[{"type":"text","text":"a"}],"stop_reason":null,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}}),
    )
}

#[tokio::test]
async fn a_turn_with_a_request_of_its_own_one_placed_of_a_sub_agent_and_one_in_doubt_counts_the_charge_once() {
    // The main conversation's request (100 and 10), a sub-agent's message that ended in the window (70 and 10, of the
    // model the sums can be taken for), and a message of a second model whose growth holds something that is not it
    // (a side call of 40 and 5): the sums of that model name nothing, so its message is no row, and the growth that
    // holds it is counted once, in the turn's remainder. The statement holds the main request and the placed one.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.extend(main_request("main-r1", 100, 0, 0, 10));
    frames.push(printed_by("agent-a", "ma1", 70, "mock-large"));
    frames.push(printed_by("agent-b", "mb1", 55, "other-model"));
    frames.extend(result_by_model(
        [100, 0, 0, 10],
        &[("mock-large", [170, 0, 0, 20]), ("other-model", [40, 0, 0, 5])],
        "context-a",
    ));
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(5_000)).await;
    actor.opened.as_ref().unwrap();
    put_transcript(actor.home(), "agent-a", "ma1", 70, 10);
    let rows = collect_through(&mut actor.events, "turn.completed", WAIT)
        .await
        .unwrap();
    let turn = crate::common::corpus::turn();
    let used = rows
        .iter()
        .rfind(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap();
    assert_eq!(classes_in(&used.body["turn"]), (210, 0, 0, 25));
    assert_eq!(
        classes_in(&used.body["report"]["carried_by_rows"]),
        (170, 0, 0, 20),
        "the main request and the placed sub-agent request; not the one in doubt"
    );
    let sub: Vec<_> = rows.iter().filter(|row| subagent_request(row)).collect();
    assert_eq!(sub.len(), 1, "only the placed request is a row: {:?}", names(&rows));
    assert!(sub[0].body["request"]["id"].to_string().contains("ma1"));
    assert_eq!(
        engine_counts(&rows, &[turn]),
        (210, 0, 0, 25),
        "the charge: the turn's own 40 and 5 beyond the rows, and the two rows once"
    );
    actor.close().await;
}
