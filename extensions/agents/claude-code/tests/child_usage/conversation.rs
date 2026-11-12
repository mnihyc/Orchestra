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
async fn a_turns_completion_waits_for_the_settler_up_to_the_budget_and_not_for_a_clock() {
    // The budget is three seconds and the transcript shows the request after one and a half. A gate that a clock
    // opened (the second of slack of an earlier design, or half the budget) would let the completion go before the
    // report; this one goes after it, with the figure of the transcript.
    let mut frames = handshake();
    frames.push(input());
    frames.extend(launch());
    frames.push(printed("agent-a", "ma1", 11));
    frames.extend(ending());
    frames.extend(barrier());
    let mut actor =
        Actor::start_configured(scripts(frames), Origin::New, false, false, settings(3_000)).await;
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
            "usage":{"input_tokens":main.0,"output_tokens":main.1},
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
        .find(|row| row.name == "usage.updated" && row.body["turn_id"] == json!(turn))
        .unwrap_or_else(|| panic!("the use of the turn was reported: {:?}", names(rows)));
    (
        row.body["turn"]["total_tokens"].as_i64().unwrap(),
        row.body["thread"]["total_tokens"].as_i64().unwrap(),
    )
}

/// What the engine counts for a turn: its requests, and what its use holds beyond the requests it had when it
/// ended (the rule of its close).
fn engine_counts(requests_before: i64, requests_after: i64, usage: i64) -> i64 {
    requests_before + requests_after + (usage - requests_before).max(0)
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
        let completed = rows
            .iter()
            .position(|row| row.name == "turn.completed")
            .unwrap();
        let requests: Vec<_> = rows
            .iter()
            .enumerate()
            .filter(|(_, row)| subagent_request(row))
            .collect();
        assert_eq!(requests.len(), 2);
        assert!(
            requests
                .iter()
                .all(|(_, row)| row.body["turn_id"] == json!(crate::common::corpus::turn())),
            "both belong to the turn that started the sub-agent"
        );
        let before: i64 = requests
            .iter()
            .filter(|(at, _)| *at < completed)
            .map(|(_, row)| {
                row.body["request"]["tokens"]["input_tokens"]
                    .as_i64()
                    .unwrap()
                    + row.body["request"]["tokens"]["output_tokens"]
                        .as_i64()
                        .unwrap()
            })
            .sum();
        let after_a: i64 = requests
            .iter()
            .filter(|(at, _)| *at > completed)
            .map(|(_, row)| {
                row.body["request"]["tokens"]["input_tokens"]
                    .as_i64()
                    .unwrap()
                    + row.body["request"]["tokens"]["output_tokens"]
                        .as_i64()
                        .unwrap()
            })
            .sum();
        assert_eq!((before, after_a), (80, 100));
        // What the engine counts over the two turns is what the program's totals grew by.
        assert_eq!(
            engine_counts(before, after_a, a_use) + engine_counts(0, 0, b_use),
            190 + 25,
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
    // The transcript holds the first request only: it is reported at its first look, and the second at the end of the
    // wait, a good while after (and after A's gate has opened, which is shut when B begins and ends).
    let mut actor =
        Actor::start_configured(scripts, Origin::New, false, false, settings(800)).await;
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
    assert_eq!(
        fillers.len(),
        100,
        "nothing of it was lost: {:?}",
        names(&rows)
    );
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
async fn the_question_of_a_sub_agent_is_closed_to_the_engine_with_its_turn_and_a_late_answer_finds_nothing()
 {
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
        .ask(
            |reply| orch_adapter_claude_code::conversation::Command::Resolve {
                request_ref: "ask-5".into(),
                decision: orch_agent_contract::session::ApprovalDecision::Option {
                    option_id: "allow_once".into(),
                    note: None,
                },
                reply,
            },
        )
        .await
        .unwrap();
    assert_eq!(late.unwrap_err().code(), "not_found");
    actor.close().await;
}
