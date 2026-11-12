//! A sub-agent's requests are settled from the sub-agent's own transcript by a task beside the conversation, and
//! reported once, final, with the usage the model closed them with (the program prints the messages of a
//! sub-agent before that usage is known).
//!
//! The first cases replay a recording of the pinned program (`child_usage.json`): a turn that starts a background
//! sub-agent making two requests, with the sub-agent's transcript as the program wrote it. The other cases are
//! constructed from the same frame shapes. The mapper only hears the messages and what a result's totals name; the
//! `Run` below does what the conversation does around it: it hands what the mapper heard to the settler, and a
//! turn that ends to the settler with a gate, and states what a turn's own rows carry of its use.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use orch_adapter_claude_code::child_usage::{
    self, Done, Files, Heard, Located, Placement, Reader, Settling, Transcripts,
};
use orch_adapter_claude_code::events::{EventMapper, MapperSettings};
use orch_adapter_claude_code::stream::Frame;
use orch_agent_contract::adapter::{AdapterEvent, EventSink, EventStream};
use orch_agent_contract::events::SessionEvent;
use orch_agent_contract::requests::{ModelRequestReport, RequestOutcome, UsageRequest};
use orch_types::{Id, Json, ManualClock, new_id};
use serde_json::json;
use tokio::sync::{mpsc, watch};

mod common;
#[path = "child_usage/conversation.rs"]
mod conversation;

const DOCUMENT: &str = include_str!("fixtures/native/2.1.319/default/child_usage.json");
const TRANSCRIPT: &str =
    include_str!("fixtures/native/2.1.319/default/child_usage.transcript.jsonl");

/// The recording, and where its frames that matter stand.
struct Capture {
    frames: Vec<Json>,
    session: String,
    agent: String,
    result: usize,
    second_echo: usize,
    notification: usize,
}

fn position(frames: &[Json], nth: usize, test: impl Fn(&Json) -> bool) -> usize {
    frames
        .iter()
        .enumerate()
        .filter(|(_, frame)| test(frame))
        .nth(nth)
        .map(|(at, _)| at)
        .unwrap()
}

impl Capture {
    fn load() -> Self {
        let document: Json = serde_json::from_str(DOCUMENT).unwrap();
        assert_eq!(document["harness"], "claude-code");
        assert_eq!(document["version"], "2.1.319");
        assert_eq!(document["case"], "child_usage");
        let frames: Vec<Json> = document["frames"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| {
                assert_eq!(row["direction"], "from_harness");
                row["payload"].clone()
            })
            .collect();
        assert_eq!(
            frames.len(),
            usize::try_from(document["raw_frame_count"].as_u64().unwrap()).unwrap()
        );
        let echo =
            |frame: &Json| frame["type"] == "assistant" && frame["parent_tool_use_id"].is_string();
        let first_echo = position(&frames, 0, echo);
        let second_echo = position(&frames, 1, echo);
        let result = position(&frames, 0, |frame| frame["type"] == "result");
        let notification = position(&frames, 0, |frame| frame["subtype"] == "task_notification");
        assert!(first_echo < result && result < second_echo && second_echo < notification);
        let session = frames[first_echo]["session_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let agent = frames[first_echo]["agent_id"].as_str().unwrap().to_owned();
        // The recorded printed messages carry the count the messages started with.
        for at in [first_echo, second_echo] {
            assert_eq!(frames[at]["message"]["usage"]["output_tokens"], 1);
            assert!(frames[at]["message"]["stop_reason"].is_null());
        }
        Self {
            frames,
            session,
            agent,
            result,
            second_echo,
            notification,
        }
    }
}

/// The transcript of an agent, as the program lays it out: `projects/orch/<session>/subagents/[group/]agent-<id>.jsonl`.
fn put_transcript(
    home: &Path,
    session: &str,
    group: Option<&str>,
    agent: &str,
    text: &str,
) -> PathBuf {
    let mut directory = home.join("projects/orch").join(session).join("subagents");
    if let Some(group) = group {
        directory = directory.join(group);
    }
    std::fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("agent-{agent}.jsonl"));
    std::fs::write(&path, text).unwrap();
    path
}

/// A line of a transcript: the message of an assistant, with the stop reason it has once the model closed it.
fn transcript_line(
    agent: &str,
    message: &str,
    input: i64,
    output: i64,
    stop: Option<&str>,
) -> String {
    json!({
        "type":"assistant","isSidechain":true,"agentId":agent,
        "message":{"id":message,"role":"assistant","stop_reason":stop,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":output}}
    })
    .to_string()
}

/// A line of a transcript for one block of a message: a line written before the message ended carries no stop reason
/// and the count the message had then; one written after carries both its stop reason and the closing usage.
fn transcript_block(
    agent: &str,
    message: &str,
    content: Json,
    input: i64,
    output: i64,
    stop: Option<&str>,
) -> String {
    json!({
        "type":"assistant","isSidechain":true,"agentId":agent,
        "message":{"id":message,"role":"assistant","content":content,"stop_reason":stop,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":output}}
    })
    .to_string()
}

/// Stands in for the files: every look waits until it is released, and then reads the files as they are.
struct Hanging {
    released: Arc<AtomicBool>,
}

impl Transcripts for Hanging {
    fn look(&self, home: &Path, session: &str, agent: &str, reader: Reader, last: bool) -> Done {
        while !self.released.load(Ordering::SeqCst) {
            std::thread::sleep(Duration::from_millis(10));
        }
        Files.look(home, session, agent, reader, last)
    }
}

/// What the conversation does around the mapper and the settler.
struct Run {
    mapper: EventMapper,
    turn: Id,
    segment: Id,
    thread: Id,
    home: tempfile::TempDir,
    settling: Settling,
    gates: mpsc::UnboundedReceiver<u64>,
    stream: EventStream,
    reported: Vec<ModelRequestReport>,
    next_gate: u64,
    /// The running totals of the program's model, as its last result said (input, output).
    totals: (i64, i64),
    /// The window the last result closed, where a turn ended.
    window: Option<u64>,
}

/// What a turn's use comes to when it goes out: the use less what the others' requests added, and the figure its own
/// requests carry of it.
#[derive(Debug, Clone, Copy)]
struct Closed {
    used: orch_agent_contract::events::TokenUsage,
    stated: orch_agent_contract::events::TokenUsage,
}

impl Run {
    fn new(wait: Duration) -> Self {
        Self::with_files(wait, Arc::new(Files))
    }

    fn with_files(wait: Duration, files: Arc<dyn Transcripts>) -> Self {
        let mut mapper = EventMapper::new(
            MapperSettings {
                directory: "/workspace/case".into(),
                cwd: "/workspace/case".into(),
                raw_events: false,
            },
            Arc::new(ManualClock::new("2026-11-11T00:00:00Z".parse().unwrap())),
        );
        mapper.process_started(true);
        let turn = new_id();
        mapper.begin_turn(turn);
        let home = tempfile::tempdir().unwrap();
        let thread = new_id();
        let (sink, stream) = EventSink::channel();
        let (settling, gates) = Settling::spawn_with(
            sink,
            thread,
            Some(home.path().to_path_buf()),
            wait,
            files,
        );
        Self {
            mapper,
            turn,
            segment: new_id(),
            thread,
            home,
            settling,
            gates,
            stream,
            reported: Vec::new(),
            next_gate: 0,
            totals: (0, 0),
            window: None,
        }
    }

    /// One frame of the program. A turn that ends while requests are owed to its window is handed to the settler with
    /// a gate, whose number is returned.
    fn frame(&mut self, frame: &Json) -> Option<u64> {
        let events = self.mapper.on_frame(&Frame::from_value(frame.clone()));
        self.hear();
        let _ = self.mapper.take_requests();
        let window = self.mapper.take_usage_window();
        let mut gate = None;
        for event in events {
            if let SessionEvent::TurnCompleted(_) = event {
                self.window = window;
                if let Some(window) = window.filter(|_| self.settling.owes()) {
                    let number = self.next_gate;
                    self.next_gate += 1;
                    self.settling.gate(window, number);
                    gate = Some(number);
                }
            }
        }
        gate
    }

    /// Hands what the mapper heard to the settler.
    fn hear(&mut self) {
        for heard in self.mapper.take_child_heard() {
            match heard {
                Heard::Printed { turn, printed } => {
                    assert_eq!(
                        turn, self.turn,
                        "a request belongs to the turn that started its sub-agent"
                    );
                    self.settling.printed(self.segment, turn, *printed);
                }
                Heard::Placed(placement) => self.settling.placed(placement),
                Heard::Dropped(keys) => self.settling.dropped(keys),
            }
        }
    }

    fn frames(&mut self, frames: &[Json]) {
        for frame in frames {
            assert!(self.frame(frame).is_none());
        }
    }

    /// A result that closes a window of the totals whose growth holds `ended` of input beyond the parent's own use
    /// (a thousand input and ten output a turn): the messages printed so far that add up to it are the window's.
    fn ends(&mut self, ended: i64) -> Option<u64> {
        self.ends_with((ended, 0))
    }

    /// As [`Run::ends`], the messages that ended having used `ended` (input, output) between them.
    fn ends_with(&mut self, ended: (i64, i64)) -> Option<u64> {
        self.totals.0 += 1_000 + ended.0;
        self.totals.1 += 10 + ended.1;
        self.mapper.begin_turn(self.turn);
        self.frame(&result_of((1_000, 10), self.totals))
    }

    /// Waits for the gate, as the conversation's outbox does; whether it opened in time.
    async fn gate(&mut self, gate: u64, within: Duration) -> bool {
        match tokio::time::timeout(within, self.gates.recv()).await {
            Ok(Some(opened)) => {
                assert_eq!(opened, gate);
                true
            }
            _ => false,
        }
    }

    /// Takes in the reports that have gone to the host.
    async fn drain(&mut self) {
        while let Ok(event) = self.stream.try_recv() {
            if let AdapterEvent::ModelRequest(report) = event {
                self.reported.push(*report);
            }
        }
    }

    /// Waits for the settler to owe nothing: every request named for a window has been reported.
    async fn quiet(&self) {
        for _ in 0..100 {
            if !self.settling.owes() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("the settler still owes requests");
    }

    /// Waits until `count` reports have gone to the host, or the time is out.
    async fn until(&mut self, count: usize, within: Duration) {
        let deadline = tokio::time::Instant::now() + within;
        self.drain().await;
        while self.reported.len() < count {
            match tokio::time::timeout_at(deadline, self.stream.recv()).await {
                Ok(Some(AdapterEvent::ModelRequest(report))) => self.reported.push(*report),
                Ok(Some(_)) => {}
                Ok(None) | Err(_) => return,
            }
        }
    }

    /// What the use of the turn whose result closed the last window says when it goes out: the figure its own rows
    /// carry of it, and what the others' requests added taken out. Call it once the gate is open.
    fn closes(&self, used: orch_agent_contract::events::TokenUsage) -> Closed {
        let window = self.window.expect("a turn ended");
        let carried = self.settling.take_window(window, self.turn);
        let mut update = orch_agent_contract::events::UsageUpdated {
            turn_id: self.turn,
            turn: used,
            thread: used,
            context_window: None,
            context_used: None,
            report: None,
        };
        child_usage::leave_out(&mut update, &carried.others);
        Closed {
            used: update.turn,
            stated: carried.own,
        }
    }
}

fn figures(report: &ModelRequestReport) -> (Option<i64>, Option<i64>) {
    (
        report.request.tokens.input_tokens,
        report.request.tokens.output_tokens,
    )
}

fn done(request: &UsageRequest) -> bool {
    request.subagent && request.outcome.as_ref().map(RequestOutcome::code) == Some("done")
}

const LONG: Duration = Duration::from_secs(30);
const SOON: Duration = Duration::from_secs(5);

// ---- the recorded sequence

#[tokio::test]
async fn the_recorded_sub_agents_requests_are_reported_once_final_with_the_figures_of_its_transcript()
 {
    let capture = Capture::load();
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        &capture.session,
        None,
        &capture.agent,
        TRANSCRIPT,
    );
    for (at, frame) in capture.frames.iter().enumerate() {
        let gate = run.frame(frame);
        if at == capture.result {
            // The request is held when the turn ends, unless a look at its transcript came first.
            if let Some(gate) = gate {
                assert!(
                    run.gate(gate, SOON).await,
                    "the turn's completion goes out as soon as what the sub-agent made so far is reported"
                );
            }
            run.until(1, SOON).await;
            assert_eq!(run.reported.len(), 1);
            assert_eq!(figures(&run.reported[0]), (Some(70), Some(10)));
        } else {
            assert!(gate.is_none());
        }
    }
    // The next turn is the wake: its result names the sub-agent's second request, which its totals hold.
    run.mapper.begin_turn(new_id());
    let gate = run.frame(&result_of_model("mock-large", (1_200, 4), (22_758, 127))).unwrap();
    assert!(run.gate(gate, SOON).await);
    run.until(2, SOON).await;
    run.until(3, Duration::from_millis(500)).await;
    assert_eq!(run.reported.len(), 2, "each request is reported once");
    assert_eq!(figures(&run.reported[1]), (Some(90), Some(10)));
    assert!(run.reported.iter().all(|report| done(&report.request)));
    for report in &run.reported {
        assert_eq!(report.turn_id, run.turn);
        assert_eq!(report.segment_id, run.segment);
        assert_eq!(report.thread_id, run.thread);
    }
    assert_ne!(
        run.reported[0].request.id, run.reported[1].request.id,
        "two requests"
    );
    let total: i64 = run
        .reported
        .iter()
        .map(|report| {
            report.request.tokens.input_tokens.unwrap()
                + report.request.tokens.output_tokens.unwrap()
        })
        .sum();
    assert_eq!(
        total, 180,
        "both requests count with the model's closing usage"
    );
}

#[tokio::test]
async fn a_request_is_looked_for_on_a_schedule_of_its_own_until_its_transcript_shows_it() {
    let capture = Capture::load();
    let mut run = Run::new(LONG);
    run.frames(&capture.frames[..capture.result]);
    // The result names the request: from then on it is looked for.
    let _ = run.frame(&capture.frames[capture.result]);
    run.until(1, Duration::from_millis(300)).await;
    assert!(
        run.reported.is_empty(),
        "nothing is reported for the sub-agent before its transcript shows the request"
    );
    put_transcript(
        run.home.path(),
        &capture.session,
        None,
        &capture.agent,
        TRANSCRIPT,
    );
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(70), Some(10)));
    run.frames(&capture.frames[capture.result + 1..=capture.notification]);
    run.mapper.begin_turn(new_id());
    let _ = run.frame(&result_of_model("mock-large", (1_200, 4), (22_758, 127)));
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    assert_eq!(figures(&run.reported[1]), (Some(90), Some(10)));
}

// ---- when there is no transcript

// The clock is the runtime's and is paused, so that what is measured is what the settler waits for and not how fast
// the run is: tokio does not move a paused clock while a blocking read runs.
#[tokio::test(start_paused = true)]
async fn a_session_without_transcripts_waits_once_for_the_bound_and_is_not_waited_for_again() {
    let capture = Capture::load();
    let wait = Duration::from_millis(400);
    let mut run = Run::new(wait);
    run.frames(&capture.frames[..capture.result]);
    let began = tokio::time::Instant::now();
    let gate = run.frame(&capture.frames[capture.result]).unwrap();
    assert!(run.gate(gate, SOON).await);
    let waited = began.elapsed();
    assert!(
        waited >= wait,
        "the turn waited for the request's transcript: {waited:?}"
    );
    assert!(waited < LONG, "and for the bound only: {waited:?}");
    run.drain().await;
    assert_eq!(
        run.reported.len(),
        1,
        "a request is never lost for want of a transcript"
    );
    assert_eq!(
        figures(&run.reported[0]),
        (Some(70), Some(1)),
        "it goes as the stream said it"
    );
    assert!(done(&run.reported[0].request));
    // The session was found to have no transcripts: its next request is reported at once.
    let began = tokio::time::Instant::now();
    run.frames(&capture.frames[capture.result + 1..=capture.second_echo]);
    run.mapper.begin_turn(new_id());
    let _ = run.frame(&result_of_model("mock-large", (1_200, 4), (22_758, 127)));
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    assert_eq!(figures(&run.reported[1]), (Some(90), Some(1)));
    assert!(
        began.elapsed() < wait,
        "nothing waits for a session that has none: {:?}",
        began.elapsed()
    );
}

#[tokio::test]
async fn a_message_that_names_no_agent_or_session_or_an_unusable_one_is_reported_at_once() {
    for (key, value) in [
        ("agent_id", None),
        ("session_id", None),
        ("agent_id", Some("../outside")),
        ("agent_id", Some("")),
    ] {
        let mut run = Run::new(LONG);
        let mut message = printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        );
        match value {
            Some(value) => message[key] = json!(value),
            None => {
                message.as_object_mut().unwrap().remove(key);
            }
        }
        run.frames(&[agent_call("tool-a"), started("task-a", "tool-a")]);
        let began = Instant::now();
        run.frames(&[message]);
        run.ends(11);
        run.until(1, SOON).await;
        assert_eq!(run.reported.len(), 1, "{key} {value:?}");
        assert_eq!(figures(&run.reported[0]), (Some(11), Some(1)));
        assert!(
            began.elapsed() < Duration::from_secs(2),
            "the sub-agent is running and nothing is waited for: {:?}",
            began.elapsed()
        );
        run.quiet().await;
    }
}

#[tokio::test(start_paused = true)]
async fn the_bound_is_one_for_all_the_requests_of_a_turn_and_not_one_for_each() {
    let wait = Duration::from_millis(300);
    let mut run = Run::new(wait);
    // The directory is there and the files are not: every request waits for its file.
    std::fs::create_dir_all(run.home.path().join("projects/orch/session/subagents")).unwrap();
    let text = json!([{"type":"text","text":"t"}]);
    run.frames(&[
        agent_call("tool-a"),
        agent_call("tool-b"),
        started("task-a", "tool-a"),
        started("task-b", "tool-b"),
        printed("tool-a", "agent-a", "ma1", 11, text.clone()),
        printed("tool-b", "agent-b", "mb1", 22, text.clone()),
        printed("tool-a", "agent-a", "ma2", 12, text),
    ]);
    let began = tokio::time::Instant::now();
    let gate = run.ends(11 + 22 + 12).unwrap();
    assert!(run.gate(gate, SOON).await);
    let waited = began.elapsed();
    assert!(waited >= wait, "{waited:?}");
    assert!(
        waited < wait * 2,
        "three requests together wait for the bound once: {waited:?}"
    );
    run.drain().await;
    let mut input: Vec<_> = run
        .reported
        .iter()
        .map(|report| figures(report).0)
        .collect();
    input.sort();
    assert_eq!(input, [Some(11), Some(12), Some(22)]);
    assert!(
        run.reported
            .iter()
            .all(|report| figures(report).1 == Some(1))
    );
}

#[tokio::test(start_paused = true)]
async fn the_bound_runs_on_the_runtime_clock_so_that_paused_time_does_not_spin() {
    let wait = Duration::from_secs(2);
    let mut run = Run::new(wait);
    std::fs::create_dir_all(run.home.path().join("projects/orch/session/subagents")).unwrap();
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    let began = tokio::time::Instant::now();
    let gate = run.ends(11).unwrap();
    assert!(run.gate(gate, Duration::from_secs(60)).await);
    let waited = began.elapsed();
    assert!(waited >= wait, "{waited:?}");
    assert!(waited < wait + Duration::from_secs(1), "{waited:?}");
    run.drain().await;
    assert_eq!(run.reported.len(), 1);
}

// ---- reading

#[tokio::test]
async fn a_transcript_is_read_forward_in_bounded_looks_and_a_line_too_long_is_skipped() {
    let mut run = Run::new(LONG);
    // A line longer than any message, then more than one look reads of lines that say nothing, then the request.
    let mut text = "x".repeat(2 << 20);
    text.push('\n');
    let filler = format!("{}\n", "y".repeat(64 * 1024));
    for _ in 0..(11 << 20) / filler.len() {
        text.push_str(&filler);
    }
    text.push_str(&transcript_line(
        "agent-big",
        "mbig",
        5,
        9,
        Some("end_turn"),
    ));
    text.push('\n');
    assert!(text.len() > 12 << 20);
    put_transcript(run.home.path(), "session", None, "agent-big", &text);
    run.frames(&[
        agent_call("tool-big"),
        started("task-big", "tool-big"),
        printed(
            "tool-big",
            "agent-big",
            "mbig",
            5,
            json!([{"type":"text","text":"b"}]),
        ),
    ]);
    run.ends(5);
    run.until(1, Duration::from_secs(20)).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(5), Some(9)));
}

#[tokio::test]
async fn a_request_that_never_settles_does_not_delay_the_others() {
    let mut run = Run::new(LONG);
    // The first message of the agent never got a stop reason; the second did.
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &format!(
            "{}\n{}\n",
            transcript_line("agent-a", "mx", 7, 1, None),
            transcript_line("agent-a", "my", 8, 4, Some("end_turn"))
        ),
    );
    let text = json!([{"type":"text","text":"t"}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "mx", 7, text.clone()),
        printed("tool-a", "agent-a", "my", 8, text),
    ]);
    run.ends(7 + 8);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(
        run.reported[0].request.provider_request_id.as_deref(),
        Some("my")
    );
    assert_eq!(figures(&run.reported[0]), (Some(8), Some(4)));
    assert!(run.settling.owes(), "the other waits");
    // The program is gone: the last look finds nothing for it, and it goes as the stream said it, once.
    run.settling.lost(None);
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    assert_eq!(figures(&run.reported[1]), (Some(7), Some(1)));
    run.until(3, Duration::from_millis(300)).await;
    assert_eq!(run.reported.len(), 2);
    run.quiet().await;
}

#[tokio::test]
async fn a_program_that_is_going_is_waited_for_and_its_files_are_read_to_the_end() {
    let mut run = Run::new(LONG);
    let text = json!([{"type":"text","text":"t"}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 11, text),
    ]);
    run.ends(11);
    let (exited, watching) = watch::channel(false);
    run.settling.lost(Some(watching));
    tokio::time::sleep(Duration::from_millis(300)).await;
    run.drain().await;
    assert!(
        run.reported.is_empty(),
        "nothing is read while the program may still write"
    );
    // The last of the file is written as the program ends.
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 11, 5, Some("end_turn")) + "\n"),
    );
    exited.send(true).unwrap();
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(11), Some(5)));
}

#[cfg(unix)]
#[tokio::test]
async fn a_link_in_the_session_directory_is_not_followed() {
    let wait = Duration::from_millis(200);
    let mut run = Run::new(wait);
    let outside = tempfile::tempdir().unwrap();
    let target = outside.path().join("elsewhere.jsonl");
    std::fs::write(
        &target,
        transcript_line("agent-a", "ma1", 11, 5, Some("end_turn")) + "\n",
    )
    .unwrap();
    let directory = run.home.path().join("projects/orch/session/subagents");
    std::fs::create_dir_all(&directory).unwrap();
    std::os::unix::fs::symlink(&target, directory.join("agent-agent-a.jsonl")).unwrap();
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    run.ends(11);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(
        figures(&run.reported[0]),
        (Some(11), Some(1)),
        "a link out of the directory is not read: the request goes as the stream said it"
    );
}

#[tokio::test]
async fn a_transcript_line_written_before_the_closing_usage_does_not_count() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("agent-a.jsonl");
    // A message of two blocks has a line for each; the first was written while the usage was the starting one.
    let lines = [
        transcript_line("a", "m1", 70, 1, None),
        transcript_line("a", "m1", 70, 10, Some("tool_use")),
        transcript_line("a", "m2", 90, 1, None),
        "{ not json".to_owned(),
    ];
    std::fs::write(&path, lines.join("\n") + "\n").unwrap();
    let closing = child_usage::closing_usage(&path, "m1").unwrap().unwrap();
    assert_eq!(closing["output_tokens"], 10);
    assert!(child_usage::closing_usage(&path, "m2").unwrap().is_none());
    assert!(child_usage::closing_usage(&path, "m3").unwrap().is_none());
    assert!(child_usage::closing_usage(&directory.path().join("absent"), "m1").is_err());
}

#[test]
fn names_that_could_leave_the_transcripts_directory_find_nothing() {
    let home = tempfile::tempdir().unwrap();
    let path = put_transcript(home.path(), "session", None, "a", "{}\n");
    assert_eq!(
        child_usage::locate(home.path(), "session", "agent-a", None),
        Located::NoFile
    );
    assert_eq!(
        child_usage::locate(home.path(), "session", "a", None),
        Located::File(path)
    );
    for (session, agent) in [
        ("..", "a"),
        ("session", "../a"),
        ("session/x", "a"),
        ("", "a"),
        ("session", ""),
    ] {
        assert_eq!(
            child_usage::locate(home.path(), session, agent, None),
            Located::NoDirectory,
            "{session:?} {agent:?}"
        );
    }
    assert_eq!(
        child_usage::locate(home.path(), "other", "a", None),
        Located::NoDirectory
    );
}

// ---- who owns a request, and what is reported twice

#[tokio::test]
async fn a_message_that_belongs_to_no_turn_of_the_session_is_dropped_at_once() {
    let mut run = Run::new(LONG);
    run.frames(&[printed(
        "tool-unknown",
        "agent-x",
        "mx",
        5,
        json!([{"type":"text","text":"x"}]),
    )]);
    run.ends(0);
    assert!(!run.settling.owes(), "nothing is owed for it");
    run.until(1, Duration::from_millis(300)).await;
    assert!(run.reported.is_empty());
}

#[tokio::test]
async fn another_block_of_a_held_message_adds_its_calls_and_a_reprint_after_the_report_is_not_reported()
 {
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 11, 5, Some("tool_use")) + "\n"),
    );
    let call = |id: &str| json!([{"type":"tool_use","id":id,"name":"Read","input":{}}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 11, call("call-1")),
        printed("tool-a", "agent-a", "ma1", 11, call("call-2")),
    ]);
    run.ends(11);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(
        run.reported[0].request.tool_refs.as_deref(),
        Some(&["call-1".to_owned(), "call-2".to_owned()][..]),
        "the request names the calls of both blocks"
    );
    run.frames(&[printed("tool-a", "agent-a", "ma1", 11, call("call-3"))]);
    run.until(2, Duration::from_millis(400)).await;
    assert_eq!(run.reported.len(), 1, "a request is reported once");
}

// ---- constructed from the recorded shapes

fn agent_call(tool: &str) -> Json {
    json!({"type":"assistant","session_id":"session","message":{"id":format!("main-{tool}"),"content":[
        {"type":"tool_use","id":tool,"name":"Agent","input":{"description":tool,"prompt":"p","subagent_type":"general-purpose","run_in_background":true}}]}})
}

fn started(task: &str, tool: &str) -> Json {
    json!({"type":"system","subtype":"task_started","session_id":"session","uuid":format!("start-{task}"),
        "task_id":task,"run_id":format!("run-{task}"),"tool_use_id":tool,"task_type":"local_agent",
        "description":task,"is_backgrounded":true})
}

fn finished(task: &str, tool: &str) -> Json {
    json!({"type":"system","subtype":"task_notification","session_id":"session","uuid":format!("end-{task}"),
        "task_id":task,"run_id":format!("run-{task}"),"tool_use_id":tool,"status":"completed",
        "output_file":"/workspace/out","summary":"done"})
}

/// The printed message of a sub-agent: complete, with the count it started with, before the closing usage.
fn printed(tool: &str, agent: &str, message: &str, input: i64, content: Json) -> Json {
    json!({"type":"assistant","parent_tool_use_id":tool,"agent_id":agent,"session_id":"session",
        "message":{"id":message,"model":"m","content":content,"stop_reason":null,
            "usage":{"input_tokens":input,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,"output_tokens":1}}})
}

#[tokio::test]
async fn two_sub_agents_at_once_are_settled_each_from_its_own_transcript_and_each_at_its_own_end() {
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 11, 5, Some("end_turn")) + "\n"),
    );
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-b",
        &(transcript_line("agent-b", "mb1", 22, 6, Some("end_turn")) + "\n"),
    );
    run.frames(&[
        agent_call("tool-a"),
        agent_call("tool-b"),
        started("task-a", "tool-a"),
        started("task-b", "tool-b"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
        printed(
            "tool-b",
            "agent-b",
            "mb1",
            22,
            json!([{"type":"text","text":"b"}]),
        ),
    ]);
    run.ends(11 + 22);
    run.until(2, SOON).await;
    let mut seen: Vec<_> = run.reported.iter().map(figures).collect();
    seen.sort();
    assert_eq!(
        seen,
        [(Some(11), Some(5)), (Some(22), Some(6))],
        "each request has the figures of its own agent's transcript"
    );
    run.frames(&[finished("task-a", "tool-a"), finished("task-b", "tool-b")]);
    run.until(3, Duration::from_millis(300)).await;
    assert_eq!(run.reported.len(), 2, "the ends add nothing");
    run.quiet().await;
}

#[tokio::test]
async fn a_sub_agent_of_a_sub_agent_is_found_where_the_program_groups_its_transcript() {
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-p",
        &(transcript_line("agent-p", "mp1", 30, 7, Some("tool_use")) + "\n"),
    );
    put_transcript(
        run.home.path(),
        "session",
        Some("group"),
        "agent-g",
        &(transcript_line("agent-g", "mg1", 40, 8, Some("end_turn")) + "\n"),
    );
    run.frames(&[
        agent_call("tool-p"),
        started("task-p", "tool-p"),
        printed(
            "tool-p",
            "agent-p",
            "mp1",
            30,
            json!([{"type":"tool_use","id":"tool-g","name":"Agent","input":{"description":"g","prompt":"p","subagent_type":"general-purpose"}}]),
        ),
        printed(
            "tool-g",
            "agent-g",
            "mg1",
            40,
            json!([{"type":"text","text":"g"}]),
        ),
    ]);
    run.ends(30 + 40);
    run.until(2, SOON).await;
    let mut seen: Vec<_> = run.reported.iter().map(figures).collect();
    seen.sort();
    assert_eq!(seen, [(Some(30), Some(7)), (Some(40), Some(8))]);
}

// ---- the budget: nothing the settler waits for can hold a turn's gate

#[tokio::test]
async fn a_read_that_does_not_answer_cannot_hold_a_turns_gate_past_the_budget() {
    let wait = Duration::from_millis(300);
    let released = Arc::new(AtomicBool::new(false));
    let mut run = Run::with_files(
        wait,
        Arc::new(Hanging {
            released: Arc::clone(&released),
        }),
    );
    // The file shows the request, if only a read answered.
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 11, 5, Some("end_turn")) + "\n"),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    let began = Instant::now();
    let gate = run.ends(11).unwrap();
    assert!(
        run.gate(gate, SOON).await,
        "the gate opens although no read answers"
    );
    let waited = began.elapsed();
    assert!(waited >= wait, "{waited:?}");
    assert!(waited < wait + Duration::from_secs(2), "{waited:?}");
    run.drain().await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(
        figures(&run.reported[0]),
        (Some(11), Some(1)),
        "it goes as the stream said it"
    );
    released.store(true, Ordering::SeqCst);
    run.until(2, Duration::from_millis(500)).await;
    assert_eq!(
        run.reported.len(),
        1,
        "the answer of a read that was left is dropped"
    );
    run.quiet().await;
}

#[tokio::test]
async fn waiting_for_the_exit_of_a_program_does_not_hold_up_other_requests() {
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-b",
        &(transcript_line("agent-b", "mb1", 22, 6, Some("end_turn")) + "\n"),
    );
    let text = json!([{"type":"text","text":"t"}]);
    run.frames(&[
        agent_call("tool-a"),
        agent_call("tool-b"),
        started("task-a", "tool-a"),
        started("task-b", "tool-b"),
        printed("tool-a", "agent-a", "ma1", 11, text.clone()),
    ]);
    run.ends(11);
    let (exited, watching) = watch::channel(false);
    run.settling.lost(Some(watching));
    // A request of the program that comes next is settled while the exit of the last is awaited.
    run.frames(&[printed("tool-b", "agent-b", "mb1", 22, text)]);
    run.ends(22);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(22), Some(6)));
    exited.send(true).unwrap();
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    assert_eq!(
        figures(&run.reported[1]),
        (Some(11), Some(1)),
        "its file was read to the end after the exit, and showed nothing"
    );
    run.quiet().await;
}

#[tokio::test]
async fn a_request_whose_file_is_absent_is_reported_at_its_deadline_and_not_at_the_end_of_its_sub_agent()
 {
    let wait = Duration::from_millis(300);
    let mut run = Run::new(wait);
    // The session has transcripts, and this agent has none.
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-other",
        &(transcript_line("agent-other", "mo1", 1, 1, Some("end_turn")) + "\n"),
    );
    run.frames(&[agent_call("tool-a"), started("task-a", "tool-a")]);
    run.frames(&[printed(
        "tool-a",
        "agent-x",
        "mx1",
        15,
        json!([{"type":"text","text":"x"}]),
    )]);
    let began = Instant::now();
    run.ends(15);
    run.until(1, SOON).await;
    let waited = began.elapsed();
    assert_eq!(run.reported.len(), 1, "the sub-agent has not ended");
    assert!(waited >= wait, "{waited:?}");
    assert!(waited < Duration::from_secs(3), "{waited:?}");
    assert_eq!(figures(&run.reported[0]), (Some(15), Some(1)));
    run.quiet().await;
}

// ---- reading

#[tokio::test]
async fn a_message_of_several_blocks_is_settled_by_the_line_that_carries_its_stop_reason() {
    // Constructed from the shape of the program's transcripts: a line for each block.
    let mut run = Run::new(LONG);
    let path = put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_block(
            "agent-a",
            "ma1",
            json!([{"type":"text","text":"Looking."}]),
            40,
            1,
            None,
        ) + "\n"),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            40,
            json!([{"type":"text","text":"Looking."}]),
        ),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            40,
            json!([{"type":"tool_use","id":"call-9","name":"Read","input":{}}]),
        ),
    ]);
    run.ends(40);
    run.until(1, Duration::from_millis(400)).await;
    assert!(
        run.reported.is_empty(),
        "the line of the first block has the count the message started with and settles nothing"
    );
    let mut file = std::fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap();
    std::io::Write::write_all(
        &mut file,
        (transcript_block(
            "agent-a",
            "ma1",
            json!([{"type":"tool_use","id":"call-9","name":"Read","input":{}}]),
            40,
            12,
            Some("tool_use"),
        ) + "\n")
            .as_bytes(),
    )
    .unwrap();
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(40), Some(12)));
    assert_eq!(
        run.reported[0].request.tool_refs.as_deref(),
        Some(&["call-9".to_owned()][..])
    );
}

#[tokio::test]
async fn the_calls_of_a_message_are_those_of_its_transcript_whenever_its_blocks_were_heard() {
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &format!(
            "{}\n{}\n",
            transcript_block(
                "agent-a",
                "ma1",
                json!([{"type":"tool_use","id":"call-1","name":"Read","input":{}}]),
                40,
                1,
                None,
            ),
            transcript_block(
                "agent-a",
                "ma1",
                json!([{"type":"tool_use","id":"call-2","name":"Read","input":{}}]),
                40,
                12,
                Some("tool_use"),
            )
        ),
    );
    let call = |id: &str| json!([{"type":"tool_use","id":id,"name":"Read","input":{}}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 40, call("call-1")),
    ]);
    run.ends(40);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    // The second block is heard after the report.
    run.frames(&[printed("tool-a", "agent-a", "ma1", 40, call("call-2"))]);
    run.until(2, Duration::from_millis(400)).await;
    assert_eq!(run.reported.len(), 1, "a request is reported once");
    assert_eq!(
        run.reported[0].request.tool_refs.as_deref(),
        Some(&["call-1".to_owned(), "call-2".to_owned()][..]),
        "the report names the calls of the whole message"
    );
}

/// A result whose use is `main` and whose running totals are `totals`.
fn result_of(main: (i64, i64), totals: (i64, i64)) -> Json {
    result_of_model("m", main, totals)
}

/// As [`result_of`], for the model `model`.
fn result_of_model(model: &str, main: (i64, i64), totals: (i64, i64)) -> Json {
    json!({"type":"result","subtype":"success","session_id":"session","is_error":false,
        "terminal_reason":"completed",
        "usage":{"input_tokens":main.0,"output_tokens":main.1},
        "modelUsage":{model:{"inputTokens":totals.0,"outputTokens":totals.1}}})
}

fn placed(mapper: &mut EventMapper) -> Vec<Placement> {
    mapper
        .take_child_heard()
        .into_iter()
        .filter_map(|heard| match heard {
            Heard::Placed(placement) => Some(placement),
            _ => None,
        })
        .collect()
}

fn a_mapper() -> EventMapper {
    let mut mapper = EventMapper::new(
        MapperSettings {
            directory: "/workspace/case".into(),
            cwd: "/workspace/case".into(),
            raw_events: false,
        },
        Arc::new(ManualClock::new("2026-11-11T00:00:00Z".parse().unwrap())),
    );
    mapper.process_started(true);
    mapper.begin_turn(new_id());
    mapper
}

fn key(tool: &str, message: &str) -> (String, String) {
    (tool.to_owned(), message.to_owned())
}

#[test]
fn a_result_names_the_messages_its_totals_hold_and_leaves_the_others_waiting_for_a_later_one() {
    // The order the pinned program showed: the first block of a sub-agent's message is printed, the parent's result
    // comes, and the message ends (its cost is added to the totals) after it. The totals of the first result are the
    // parent's own; the next result's hold both messages of the sub-agent.
    let mut mapper = a_mapper();
    let text = json!([{"type":"text","text":"a"}]);
    for frame in [
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 70, text.clone()),
        result_of((10, 2), (10, 2)),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    let first = placed(&mut mapper);
    assert!(
        first.is_empty(),
        "the message was printed, and its cost is not in the totals of the result: it is not ended, and waits: {first:?}"
    );
    mapper.begin_turn(new_id());
    for frame in [
        printed("tool-a", "agent-a", "ma2", 90, text),
        // 20 and 3 of its own, 70 and 10 of ma1, 90 and 10 of ma2.
        result_of((20, 3), (190, 25)),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    let second = placed(&mut mapper);
    assert_eq!(second.len(), 1);
    assert!(second[0].base_known);
    assert_eq!(
        second[0].named,
        vec![key("tool-a", "ma1"), key("tool-a", "ma2")],
        "both messages are in the totals of the second result, and it names them"
    );
    assert!(second[0].unknown.is_empty() && second[0].dropped.is_empty());
}

/// What the engine counts of a turn that states `stated`: its use less that figure, and the rows it has, per class.
fn engine_counts(
    uses: &[(orch_agent_contract::events::TokenUsage, orch_agent_contract::events::TokenUsage)],
    rows: &[(Option<i64>, Option<i64>)],
) -> (i64, i64) {
    let remainders = uses.iter().fold((0, 0), |sum, (used, stated)| {
        (
            sum.0 + (used.input_tokens - stated.input_tokens).max(0),
            sum.1 + (used.output_tokens - stated.output_tokens).max(0),
        )
    });
    rows.iter().fold(remainders, |sum, (input, output)| {
        (sum.0 + input.unwrap_or(0), sum.1 + output.unwrap_or(0))
    })
}

fn use_of(input: i64, output: i64) -> orch_agent_contract::events::TokenUsage {
    usage_of(input, output)
}

#[tokio::test]
async fn a_message_that_ended_in_a_window_with_cost_nobody_printed_is_never_a_row_and_the_turn_counts_it_once() {
    // A window whose growth holds something no printed message explains (a side call of the same model, a
    // shortening of the conversation): the sums name nothing, the request is no row, and the turn's own use holds
    // it, once. The message ends before the result and its transcript is written; its line is read late.
    let mut run = Run::new(LONG);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 70, 10, Some("end_turn")) + "\n"),
    );
    let text = json!([{"type":"text","text":"a"}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 70, text),
    ]);
    // The parent's 1 000 and 10, the message's 70 and 10, and 500 and 40 that nothing explains.
    run.mapper.begin_turn(run.turn);
    let gate = run.frame(&result_of((1_000, 10), (1_570, 60)));
    assert!(gate.is_none(), "nothing is owed to the window: nothing was named");
    run.until(1, Duration::from_millis(400)).await;
    assert!(run.reported.is_empty(), "no row is sent for a request whose window could not be decided");
    let closed = run.closes(use_of(1_570, 60));
    assert_eq!(
        (closed.stated.input_tokens, closed.stated.output_tokens),
        (0, 0),
        "the turn states that its rows carry none of its use"
    );
    assert_eq!(
        engine_counts(&[(closed.used, closed.stated)], &[]),
        (1_570, 60),
        "the growth is counted once, as the turn's own"
    );
    // The message is done with once its sub-agent prints another and a window that could be told does not hold it.
    run.frames(&[printed(
        "tool-a",
        "agent-a",
        "ma2",
        90,
        json!([{"type":"text","text":"b"}]),
    )]);
    // The next result holds the parent's own use alone: the sums name nothing but the empty set, which they do.
    run.mapper.begin_turn(run.turn);
    let gate = run.frame(&result_of((1_000, 10), (2_570, 70)));
    assert!(gate.is_none());
    run.until(1, Duration::from_millis(400)).await;
    assert!(
        run.reported.is_empty(),
        "the first message is done with, and no row is sent for it; the second may be written still"
    );
    assert!(!run.settling.owes());
}

#[tokio::test]
async fn more_agents_than_the_old_bound_are_named_by_the_exact_sums_and_every_row_is_conserved() {
    // Twelve sub-agents each print a message that ends in the window: the sums name them all (nothing bounds the
    // search but its steps), and the turn states exactly what its twelve rows carry.
    let mut run = Run::new(LONG);
    let text = json!([{"type":"text","text":"t"}]);
    let mut frames = Vec::new();
    for n in 0..12 {
        let (tool, agent) = (format!("tool-{n}"), format!("agent-{n}"));
        frames.push(agent_call(&tool));
        frames.push(started(&format!("task-{n}"), &tool));
        frames.push(printed(&tool, &agent, &format!("m{n}"), 100 + n, text.clone()));
        put_transcript(
            run.home.path(),
            "session",
            None,
            &agent,
            &(transcript_line(&agent, &format!("m{n}"), 100 + n, 5, Some("end_turn")) + "\n"),
        );
    }
    run.frames(&frames);
    let ended: i64 = (0..12).map(|n| 100 + n).sum();
    let gate = run.ends_with((ended, 12 * 5)).unwrap();
    assert!(run.gate(gate, SOON).await);
    run.drain().await;
    assert_eq!(run.reported.len(), 12);
    let used = use_of(1_000 + ended, 10 + 12 * 5);
    let closed = run.closes(used);
    assert_eq!(closed.stated.input_tokens, ended);
    assert_eq!(closed.stated.output_tokens, 12 * 5);
    let rows: Vec<_> = run.reported.iter().map(figures).collect();
    assert_eq!(
        engine_counts(&[(closed.used, closed.stated)], &rows),
        (1_000 + ended, 10 + 12 * 5),
        "the turn's own 1 000 / 10 and each row once"
    );
}

#[test]
fn a_main_conversation_that_does_not_fit_under_the_growth_names_nothing() {
    // The result says the parent used more than the model's totals grew by: the growth cannot be taken for the
    // window's, and nothing is named for room that may not be there (the message has not certainly ended).
    let mut mapper = a_mapper();
    let text = json!([{"type":"text","text":"a"}]);
    for frame in [
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 70, text),
        result_of((500, 20), (400, 20)),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    assert!(placed(&mut mapper).is_empty());
}

#[test]
fn a_window_of_unknown_base_names_only_the_messages_that_have_certainly_ended() {
    // A process that resumed a session has totals the harness restored: the first result's growth is not known. A
    // message of a sub-agent that went on to print another has ended; the latest of each agent may still be written.
    let mut mapper = a_mapper();
    mapper.process_started(false);
    mapper.begin_turn(new_id());
    let text = json!([{"type":"text","text":"a"}]);
    for frame in [
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 70, text.clone()),
        printed("tool-a", "agent-a", "ma2", 90, text),
        result_of((100, 10), (50_000, 4_000)),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    let heard = placed(&mut mapper);
    assert_eq!(heard.len(), 1);
    assert!(!heard[0].base_known);
    assert_eq!(heard[0].named, vec![key("tool-a", "ma1")]);
    assert!(heard[0].dropped.is_empty());
}

#[tokio::test]
async fn a_message_whose_every_line_carries_the_closing_figures_is_settled_by_them() {
    // The shape the pinned program writes for a message that finished within one flush: a line for each block, and
    // every one of them with the stop reason and the usage the model closed the message with.
    let mut run = Run::new(LONG);
    let text = json!([{"type":"text","text":"Looking."}]);
    let call = json!([{"type":"tool_use","id":"call-9","name":"Read","input":{}}]);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &format!(
            "{}\n{}\n",
            transcript_block("agent-a", "ma1", text.clone(), 40, 12, Some("tool_use")),
            transcript_block("agent-a", "ma1", call.clone(), 40, 12, Some("tool_use")),
        ),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 40, text),
        printed("tool-a", "agent-a", "ma1", 40, call),
    ]);
    run.ends(40);
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1, "the message is reported once");
    assert_eq!(figures(&run.reported[0]), (Some(40), Some(12)));
    assert_eq!(
        run.reported[0].request.tool_refs.as_deref(),
        Some(&["call-9".to_owned()][..])
    );
}

#[tokio::test]
async fn of_the_lines_with_a_stop_reason_the_one_with_the_most_output_is_the_closing_one() {
    // Counts only grow while a message is written: a line that carries a stop reason and less output than another is
    // not the closing one, whichever comes first in the file.
    let mut run = Run::new(LONG);
    let text = json!([{"type":"text","text":"t"}]);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &format!(
            "{}\n{}\n{}\n{}\n",
            transcript_block("agent-a", "ma1", text.clone(), 40, 3, Some("tool_use")),
            transcript_block("agent-a", "ma1", text.clone(), 40, 12, Some("tool_use")),
            transcript_block("agent-a", "ma2", text.clone(), 50, 9, Some("end_turn")),
            transcript_block("agent-a", "ma2", text.clone(), 50, 2, Some("end_turn")),
        ),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 40, text.clone()),
        printed("tool-a", "agent-a", "ma2", 50, text),
    ]);
    run.ends(40 + 50);
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    let mut figures_of: Vec<_> = run.reported.iter().map(figures).collect();
    figures_of.sort();
    assert_eq!(figures_of, vec![(Some(40), Some(12)), (Some(50), Some(9))]);
}

#[tokio::test]
async fn a_transcript_that_is_rewritten_at_a_larger_size_is_read_from_its_start() {
    let mut run = Run::new(LONG);
    let path = put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "m1", 11, 5, Some("end_turn")) + "\n"),
    );
    let text = json!([{"type":"text","text":"t"}]);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "m1", 11, text.clone()),
    ]);
    run.ends(11);
    run.until(1, SOON).await;
    assert_eq!(figures(&run.reported[0]), (Some(11), Some(5)));
    // The file is written anew, larger, and starts differently.
    std::fs::write(
        &path,
        format!(
            "{}\n{}\n{}\n",
            r#"{"type":"user","note":"a file written anew"}"#,
            "x".repeat(300),
            transcript_line("agent-a", "m2", 30, 7, Some("end_turn"))
        ),
    )
    .unwrap();
    run.frames(&[printed("tool-a", "agent-a", "m2", 30, text)]);
    run.ends(30);
    run.until(2, SOON).await;
    assert_eq!(run.reported.len(), 2);
    assert_eq!(figures(&run.reported[1]), (Some(30), Some(7)));
}

// ---- what a turn's use leaves out, and a settler that fails

fn usage_of(input: i64, output: i64) -> orch_agent_contract::events::TokenUsage {
    orch_agent_contract::events::TokenUsage {
        input_tokens: input,
        output_tokens: output,
        total_tokens: input + output,
        ..Default::default()
    }
}

fn use_of_turn(models: &[(&str, i64, i64)]) -> orch_agent_contract::events::UsageUpdated {
    let tokens = |input: i64, output: i64| usage_of(input, output);
    orch_agent_contract::events::UsageUpdated {
        turn_id: new_id(),
        turn: tokens(
            models.iter().map(|model| model.1).sum(),
            models.iter().map(|model| model.2).sum(),
        ),
        thread: tokens(1_000, 100),
        context_window: None,
        context_used: None,
        report: Some(orch_agent_contract::events::TurnUsageReport {
            models: models
                .iter()
                .map(
                    |(name, input, output)| orch_agent_contract::events::ModelTokens {
                        model: (*name).to_owned(),
                        tokens: tokens(*input, *output),
                        tokens_reported: true,
                        counts: Default::default(),
                    },
                )
                .collect(),
            ..Default::default()
        }),
    }
}

fn contribution(model: Option<&str>, input: i64, output: i64) -> child_usage::Contribution {
    child_usage::Contribution {
        window: 4,
        owner: new_id(),
        model: model.map(str::to_owned),
        held: usage_of(input, output),
    }
}

#[test]
fn what_other_turns_requests_added_is_left_out_of_a_turns_use_by_model_and_never_below_nothing() {
    let mut usage = use_of_turn(&[("mock-large", 110, 13), ("mock-small[1m]", 40, 5)]);
    child_usage::leave_out(
        &mut usage,
        &[
            contribution(Some("mock-large"), 90, 10),
            // The model is named without the suffix of its larger window.
            contribution(Some("mock-small"), 10, 1),
            // A model the turn did not use: taken from the one it used most (now the second).
            contribution(Some("other"), 5, 1),
        ],
    );
    let models = &usage.report.as_ref().unwrap().models;
    assert_eq!(models[0].tokens.total_tokens, 23);
    assert_eq!(models[1].tokens.total_tokens, 28);
    assert_eq!(usage.turn.total_tokens, 168 - 100 - 11 - 6);
    assert_eq!(
        usage.thread.total_tokens, 1_100,
        "the thread's total is the growth of the totals, and stays"
    );
    let mut small = use_of_turn(&[("m", 5, 1)]);
    child_usage::leave_out(&mut small, &[contribution(Some("m"), 100, 100)]);
    assert_eq!(small.turn.total_tokens, 0, "never below nothing");
    assert_eq!(
        small.report.as_ref().unwrap().models[0].tokens.total_tokens,
        0
    );
}

#[test]
fn the_reasoning_a_row_does_not_carry_is_not_taken_out_and_is_clamped_to_the_output_that_is_left() {
    // Reasoning is a part of the output. A request whose row carries none leaves none of it out of a turn's use, and
    // what the output left cannot hold is dropped (as the engine does for any report): it is the request's own, which
    // no row could carry, and the total is conserved whatever the parts.
    let mut usage = use_of_turn(&[("m", 110, 13)]);
    usage.turn.reasoning_tokens = 12;
    usage.report.as_mut().unwrap().models[0].tokens.reasoning_tokens = 12;
    child_usage::leave_out(&mut usage, &[contribution(Some("m"), 90, 10)]);
    assert_eq!(usage.turn.total_tokens, 123 - 100);
    assert_eq!(usage.turn.output_tokens, 3);
    assert_eq!(usage.turn.reasoning_tokens, 3, "never more than the output left");
    assert!(usage.turn.reasoning_tokens <= usage.turn.output_tokens);
}

/// The request a printed message of a sub-agent describes, as the mapper makes it: ended (`done`), with the figures the
/// message started with. The engine refuses figures without an end (`UsageRequest::check`).
fn a_request(n: usize) -> UsageRequest {
    let mut request = UsageRequest {
        id: format!("tool-a:m{n}"),
        subagent: true,
        model: Some("m".to_owned()),
        tokens: orch_agent_contract::requests::RequestTokens {
            input_tokens: Some(2),
            output_tokens: Some(1),
            ..Default::default()
        },
        ..UsageRequest::default()
    };
    request.finish("done", None, None);
    request
}

fn a_printed(n: usize) -> orch_adapter_claude_code::performance::Printed {
    orch_adapter_claude_code::performance::Printed {
        request: a_request(n),
        parent: "tool-a".to_owned(),
        agent_id: Some("agent-a".to_owned()),
        session_id: Some("session".to_owned()),
        message_id: format!("m{n}"),
    }
}

#[tokio::test]
async fn however_many_requests_a_window_holds_the_ledger_loses_none() {
    // A sub-agent that makes thousands of requests between two results: the ledger sums them by window, owner and
    // model, and the turn that closes the window leaves out all of the others' and states its own.
    let (sink, mut stream) = EventSink::channel();
    let (settling, _gates) = Settling::spawn_with(
        sink,
        new_id(),
        None,
        Duration::from_millis(0),
        Arc::new(Files),
    );
    let drained = tokio::spawn(async move { while stream.recv().await.is_some() {} });
    let (segment, owner) = (new_id(), new_id());
    for (window, from) in [(7, 0), (8, 4_500)] {
        let mut named = Vec::new();
        for n in from..from + 4_500 {
            settling.printed(segment, owner, a_printed(n));
            named.push(("tool-a".to_owned(), format!("m{n}")));
        }
        settling.placed(Placement {
            window,
            base_known: true,
            named,
            ..Placement::default()
        });
    }
    for _ in 0..500 {
        if !settling.owes() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!settling.owes(), "every request named for a window was reported");
    // The turn of window 7 does not own them: it leaves them all out, summed.
    let others = settling.take_window(7, new_id());
    assert_eq!(others.own.total_tokens, 0);
    let left_out: i64 = others.others.iter().map(|each| each.held.total_tokens).sum();
    assert_eq!(left_out, 4_500 * 3);
    assert!(others.others.len() < 10, "summed by owner and model: {}", others.others.len());
    assert!(
        settling.take_window(7, new_id()).others.is_empty(),
        "what was taken for a window is forgotten"
    );
    // The turn of window 8 owns them: it states them.
    let own = settling.take_window(8, owner);
    assert_eq!(own.own.total_tokens, 4_500 * 3);
    assert!(own.others.is_empty());
    drained.abort();
}

#[tokio::test]
async fn a_request_the_engine_would_refuse_is_no_row_of_the_ledger_and_its_cost_stays_in_the_use() {
    // Figures without an end are refused by `UsageRequest::check`, the check the engine asks of every report it is given
    // (the settler calls that very method): such a request is sent, as the engine will refuse it, but the window's use
    // does not say that a row carries it, nor is it left out of another turn's use.
    let (sink, mut stream) = EventSink::channel();
    let (settling, _gates) = Settling::spawn_with(
        sink,
        new_id(),
        None,
        Duration::from_millis(0),
        Arc::new(Files),
    );
    let drained = tokio::spawn(async move { while stream.recv().await.is_some() {} });
    let (segment, owner) = (new_id(), new_id());
    let mut refused = a_printed(1);
    refused.request.outcome = None;
    assert!(refused.request.check().is_err(), "this shape is one the engine refuses");
    assert!(a_printed(2).request.check().is_ok(), "and this one it accepts");
    settling.printed(segment, owner, refused);
    settling.printed(segment, owner, a_printed(2));
    settling.placed(Placement {
        window: 5,
        base_known: true,
        named: vec![
            ("tool-a".to_owned(), "m1".to_owned()),
            ("tool-a".to_owned(), "m2".to_owned()),
        ],
        ..Placement::default()
    });
    for _ in 0..500 {
        if !settling.owes() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(!settling.owes());
    let taken = settling.take_window(5, owner);
    assert_eq!(
        taken.own.total_tokens, 3,
        "only the request the engine accepts is carried: 2 and 1"
    );
    drained.abort();
}

#[tokio::test]
async fn a_settler_that_fails_reports_what_it_decided_as_the_stream_said_and_opens_the_gates() {
    let mut run = Run::new(LONG);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    let gate = run.ends(11).unwrap();
    run.settling.crash();
    assert!(
        run.gate(gate, SOON).await,
        "the gate opens although the settler failed"
    );
    run.until(1, SOON).await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(11), Some(1)));
    for _ in 0..100 {
        if run.settling.is_gone() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(run.settling.is_gone());
    assert!(!run.settling.owes(), "nothing is owed to a window by a settler that is gone");
    // The ledger it kept is still used for the turn's statement.
    let closed = run.closes(use_of(1_011, 10));
    assert_eq!(closed.stated.input_tokens, 11);
    // Nothing waits for a settler that is gone, and what is printed after is not reported: the turn that holds it
    // counted it once, in its remainder.
    run.settling.printed(run.segment, run.turn, a_printed(9));
    run.until(2, Duration::from_millis(300)).await;
    assert_eq!(run.reported.len(), 1);
}

#[tokio::test]
async fn a_wait_of_nothing_holds_nothing_and_the_request_goes_as_the_stream_said_it() {
    // `0` is no hold at all: the transcript is there and is not looked at.
    let mut run = Run::new(Duration::ZERO);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 11, 5, Some("end_turn")) + "\n"),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    let gate = run.ends(11).unwrap();
    assert!(run.gate(gate, SOON).await);
    run.drain().await;
    assert_eq!(run.reported.len(), 1);
    assert_eq!(figures(&run.reported[0]), (Some(11), Some(1)));
}

#[tokio::test]
async fn a_request_named_for_a_window_is_owed_before_the_settler_has_taken_the_note_up() {
    // A turn that ends right after the result is read decides on a gate from what it has just heard, not from what
    // the settler has taken up: on this runtime the settler cannot run until the case awaits.
    let (sink, _stream) = EventSink::channel();
    let home = tempfile::tempdir().unwrap();
    let (settling, _gates) = Settling::spawn_with(
        sink,
        new_id(),
        Some(home.path().to_path_buf()),
        LONG,
        Arc::new(Files),
    );
    assert!(!settling.owes());
    settling.printed(new_id(), new_id(), a_printed(1));
    assert!(!settling.owes(), "a request that is printed is not owed until a window names it");
    settling.placed(Placement {
        window: 3,
        base_known: true,
        named: vec![("tool-a".to_owned(), "m1".to_owned())],
        ..Placement::default()
    });
    assert!(settling.owes());
}

#[tokio::test]
async fn a_message_streaming_at_the_first_result_of_a_resumed_process_is_named_by_the_next_window_and_the_total_is_conserved() {
    // Review E: the first result of a resumed process has totals of unknown base; the sub-agent's message is still
    // being written and its cost enters the next window.
    let mut run = Run::new(LONG);
    run.mapper.process_started(false);
    run.mapper.begin_turn(run.turn);
    put_transcript(
        run.home.path(),
        "session",
        None,
        "agent-a",
        &(transcript_line("agent-a", "ma1", 70, 10, Some("end_turn")) + "\n"),
    );
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            70,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    let first = run.ends(0);
    assert!(first.is_none(), "nothing is named for the window of unknown base");
    let one = run.closes(use_of(1_000, 10));
    assert_eq!((one.stated.input_tokens, one.stated.output_tokens), (0, 0));
    run.until(1, Duration::from_millis(300)).await;
    assert!(run.reported.is_empty(), "no row while its window is not decided");
    let gate = run.ends_with((70, 10)).unwrap();
    assert!(run.gate(gate, SOON).await);
    run.drain().await;
    assert_eq!(run.reported.len(), 1);
    let two = run.closes(use_of(1_070, 20));
    assert_eq!((two.stated.input_tokens, two.stated.output_tokens), (70, 10));
    let rows: Vec<_> = run.reported.iter().map(figures).collect();
    assert_eq!(
        engine_counts(&[(one.used, one.stated), (two.used, two.stated)], &rows),
        (1_000 + 1_070, 10 + 20),
        "each window's own use, and the one row, once: what the program's totals grew by after the first result"
    );
}

#[test]
fn messages_the_sums_cannot_tell_apart_are_named_by_no_window() {
    // Two sub-agents each print a message of the same size and only one of them ended in the window: which is not told by
    // the sums, so neither is named (a cost is never guessed into a window).
    let mut mapper = a_mapper();
    let text = json!([{"type":"text","text":"a"}]);
    for frame in [
        agent_call("tool-a"),
        agent_call("tool-b"),
        started("task-a", "tool-a"),
        started("task-b", "tool-b"),
        printed("tool-a", "agent-a", "ma1", 50, text.clone()),
        printed("tool-b", "agent-b", "mb1", 50, text),
        result_of((10, 2), (60, 7)),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    assert!(placed(&mut mapper).is_empty());
}

#[test]
fn a_side_call_of_another_model_does_not_stop_the_sums_of_the_models_the_messages_name() {
    // The growth of another model (a side call) is no room for the sub-agent's messages and no reason to doubt them.
    let mut mapper = a_mapper();
    let text = json!([{"type":"text","text":"a"}]);
    for frame in [
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed("tool-a", "agent-a", "ma1", 70, text),
        json!({"type":"result","subtype":"success","session_id":"session","is_error":false,
            "terminal_reason":"completed","usage":{"input_tokens":1000,"output_tokens":10},
            "modelUsage":{"m":{"inputTokens":1070,"outputTokens":20},"other":{"inputTokens":300,"outputTokens":5}}}),
    ] {
        mapper.on_frame(&Frame::from_value(frame));
    }
    let heard = placed(&mut mapper);
    assert_eq!(heard.len(), 1);
    assert_eq!(heard[0].named, vec![key("tool-a", "ma1")]);
}

#[tokio::test]
async fn what_is_not_decided_when_the_program_is_lost_is_not_a_row() {
    // A message the program was still writing when it was lost: its window was never decided, and the growth that
    // holds it, if any, counted it once.
    let mut run = Run::new(LONG);
    run.frames(&[
        agent_call("tool-a"),
        started("task-a", "tool-a"),
        printed(
            "tool-a",
            "agent-a",
            "ma1",
            11,
            json!([{"type":"text","text":"a"}]),
        ),
    ]);
    run.settling.lost(None);
    run.until(1, Duration::from_millis(400)).await;
    assert!(run.reported.is_empty());
    assert!(!run.settling.owes());
}
