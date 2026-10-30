//! The generic adapter against a real DeepSeek Harness (`dsh --profile acp`) WITH ITS COMPANION PLUGIN loaded, and
//! the scripted model as its only provider, over Chat Completions: what each extra of the plugin gives the thread.
//!
//! One case for each thing the plugin adds that a person sees: the plugin's announcement and what the descriptor
//! claims of it; text as it is written; the token counts of a model call with its cache; a question put to the
//! person; a command that asks before it runs, and one that only reads and does not; plan mode that refuses a
//! change and shows the plan to review; a message added to a turn that runs; a stop. Every case also checks that
//! nothing but model requests left. The plugin's own cases (against a stand-in for Orch) are in
//! `dsh-bridge/tests`; these are the same things through the adapter.
//!
//! Which dsh: `ORCH_DSH_BIN` names the native executable of the runtime wheel, as in `dsh_real.rs`. Which plugin:
//! the committed bundle `dsh-bridge/dist/orch-dsh-bridge.mjs`, which the adapter embeds and writes into each
//! session's home. Without the program every case is skipped with a line that says so. A cold start extracts some 80 MB of the program into its cache, so the first
//! case takes a while.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use common::{agent_start, spec_in, turn_for};
use futures::StreamExt as _;
use orch_adapter_acp::AcpAdapter;
use orch_agent_contract::adapter::{AdapterEvent, AgentAdapter, EventSink, EventStream};
use orch_agent_contract::events::{
    ApprovalRequested, HarnessRequestDetail, HarnessRequestKind, OptionScope, OptionStyle,
    SessionEvent,
};
use orch_agent_contract::session::{ApprovalDecision, SessionMode, SessionSpec, TurnSteer};
use orch_testkit::doubles::model::{MockConfig, MockLlm};
use orch_types::{Json, new_id};

/// How long a case waits for the next event: a cold start of the program is slow, and so is a first answer.
const WAIT: Duration = Duration::from_secs(180);

fn dsh_binary() -> Option<String> {
    match std::env::var("ORCH_DSH_BIN") {
        Ok(path) if Path::new(&path).is_file() => Some(path),
        _ => {
            eprintln!("skipped: ORCH_DSH_BIN does not name the native executable of the dsh wheel");
            None
        }
    }
}

struct Rig {
    adapter: AcpAdapter,
    events: EventStream,
    mock: MockLlm,
    spec: SessionSpec,
    /// What the adapter declared of the program, as JSON.
    descriptor: Json,
    work: tempfile::TempDir,
    _home: tempfile::TempDir,
}

async fn rig(binary: &str) -> Rig {
    let mock = MockLlm::start(MockConfig::default()).await.unwrap();
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (provider, profile) = (new_id(), new_id());
    let mut start = agent_start(provider, profile, home.path());
    start.binary = Some(binary.to_owned());
    start.loopback = serde_json::from_value(serde_json::json!({
        "base_url": mock.url(), "token": "s".repeat(64)
    }))
    .unwrap();
    let mut adapter = AcpAdapter::new(orch_adapter_acp::programs::dsh().unwrap());
    let (sink, events) = EventSink::channel();
    let started = adapter.start(start, sink).await.unwrap();
    let mut spec = spec_in(work.path(), profile, provider, SessionMode::New);
    spec.settings.model = "scripted".to_owned();
    spec.settings.available_models = vec!["scripted".to_owned()];
    spec.settings.orch_tools = Vec::new();
    Rig {
        adapter,
        events,
        mock,
        spec,
        descriptor: serde_json::to_value(&started.descriptor).unwrap(),
        work,
        _home: home,
    }
}

/// The events up to and including the first that `done` accepts.
async fn until(
    events: &mut EventStream,
    done: impl Fn(&AdapterEvent) -> bool,
) -> Vec<AdapterEvent> {
    let mut seen: Vec<AdapterEvent> = Vec::new();
    loop {
        let event = tokio::time::timeout(WAIT, events.next())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "nothing came for {WAIT:?}; so far {:?}",
                    seen.iter().map(AdapterEvent::name).collect::<Vec<_>>()
                )
            })
            .expect("the adapter's stream closed");
        let end = done(&event);
        seen.push(event);
        if end {
            return seen;
        }
    }
}

fn is_named(event: &AdapterEvent, name: &str) -> bool {
    event.name() == name
}

fn turn_ended(event: &AdapterEvent) -> bool {
    is_named(event, "turn.completed")
}

fn asked_of(event: &AdapterEvent) -> Option<&ApprovalRequested> {
    match event {
        AdapterEvent::Session {
            event: SessionEvent::ApprovalRequested(asked),
            ..
        } => Some(asked),
        _ => None,
    }
}

fn is_request_for_person(event: &AdapterEvent) -> bool {
    turn_ended(event) || asked_of(event).is_some()
}

/// The status a `turn.completed` reports.
fn status_of(seen: &[AdapterEvent]) -> String {
    let last = seen.last().expect("an event");
    assert!(
        turn_ended(last),
        "the last event is not the end of the turn: {:?}",
        last.name()
    );
    last.to_wire().unwrap().1["status"]
        .as_str()
        .unwrap_or_default()
        .to_owned()
}

/// The text of the last finished message of the assistant.
fn final_text(seen: &[AdapterEvent]) -> String {
    seen.iter()
        .filter_map(|event| {
            let (kind, body) = event.to_wire().ok()?;
            (kind == "item.completed" && body["payload"]["phase"] == "final").then(|| {
                body["payload"]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned()
            })
        })
        .next_back()
        .expect("a finished message")
}

/// The reports of the model's calls in the events.
fn model_calls(seen: &[AdapterEvent]) -> Vec<Json> {
    seen.iter()
        .filter(|event| is_named(event, "model.request"))
        .map(|event| event.to_wire().unwrap().1)
        .collect()
}

/// Answers the request the person was asked with the option of this style (and scope, where one is wanted).
async fn answer_with(rig: &mut Rig, asked: &ApprovalRequested, style: OptionStyle, once: bool) {
    let option = asked
        .request
        .options
        .iter()
        .find(|option| option.style == style && (!once || option.scope == Some(OptionScope::Once)))
        .or_else(|| {
            asked
                .request
                .options
                .iter()
                .find(|option| option.style == style)
        })
        .unwrap_or_else(|| {
            panic!(
                "no option of the style {style:?} in {:?}",
                asked.request.options
            )
        });
    rig.adapter
        .resolve_approval(
            rig.spec.settings.thread_id,
            &asked.request_ref,
            ApprovalDecision::Option {
                option_id: option.id.clone(),
                note: None,
            },
        )
        .await
        .unwrap();
}

/// Only model requests left, and none that names the client or sends what its vendor is sent.
async fn only_model_requests_left(rig: &Rig) {
    let requests = rig.mock.settled(1).await;
    assert!(!requests.is_empty());
    for recorded in &requests {
        assert!(
            recorded.path.ends_with("/chat/completions") || recorded.path.ends_with("/models"),
            "only model requests leave: {}",
            recorded.path
        );
        assert!(
            recorded
                .headers
                .keys()
                .all(|name| !name.starts_with("x-deepseek-harness")),
            "a header that names the client left: {:?}",
            recorded.headers.keys().collect::<Vec<_>>()
        );
        let body = recorded.body.to_string();
        for member in ["dsh_session_log", "dsh_plugin_packages"] {
            assert!(!body.contains(member), "{member} left");
        }
    }
}

/// The text of the last tool result a request holds: what the model was told a call came to.
fn last_tool_result(body: &Json) -> String {
    body["messages"]
        .as_array()
        .and_then(|messages| {
            messages
                .iter()
                .rev()
                .find(|message| message["role"] == "tool")
        })
        .map(|message| message["content"].to_string())
        .expect("a tool result in the request")
}

/// The chat requests the model received, in order.
fn chats(rig: &Rig) -> Vec<Json> {
    rig.mock
        .requests()
        .into_iter()
        .filter(|recorded| recorded.path.ends_with("/chat/completions"))
        .map(|recorded| recorded.body)
        .collect()
}

#[tokio::test]
async fn the_plugin_announces_its_extras_and_the_descriptor_claims_what_they_give() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    let note = &rig.descriptor["x_acp_plugin"];
    assert_eq!(
        note["missing"], false,
        "the plugin was not found, or it did not speak: {note}"
    );
    assert_eq!(note["plugin"]["name"], "orch-dsh-bridge");
    let on: Vec<&str> = note["extras"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(Json::as_str)
        .collect();
    for extra in [
        "streaming",
        "steering",
        "questions",
        "plan_mode",
        "approvals",
        "usage",
        "compaction",
        "skills",
        "fork",
        "native_carry",
    ] {
        assert!(on.contains(&extra), "{extra} is not on: {note}");
    }
    assert!(
        note["off"]
            .as_array()
            .unwrap()
            .iter()
            .any(|each| each["extra"] == "executor_backends"),
        "without a launch for a separated arrangement the plugin's backends are off, and say so: {note}"
    );
    let caps = &rig.descriptor["capabilities"];
    for claim in ["steer", "plan_mode", "usage_tokens", "interrupt", "resume"] {
        assert_eq!(caps[claim], true, "{claim} is not claimed: {caps}");
    }
    let arrangements = rig.descriptor["arrangements"].as_array().unwrap();
    assert!(
        arrangements
            .iter()
            .all(|entry| entry["type"] != "separated"),
        "separated is offered only where the plugin's backends are on: {arrangements:?}"
    );
    // The session opens, and the plugin's answer to a prompt carries nothing the adapter does not read.
    let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    assert!(!opened.native_session_ref.is_empty());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn text_arrives_as_it_is_written_and_the_counts_of_the_call_keep_their_cache() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    let words = "The quick brown fox jumps over the lazy dog.";
    rig.mock
        .queue(&(format!("usage 1000 20 cached=400\nchunk 4\ndelay 120\nsay {words}\n")))
        .unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, turn_ended).await;
    assert_eq!(
        status_of(&seen),
        "completed",
        "{:?}",
        seen.iter().map(AdapterEvent::name).collect::<Vec<_>>()
    );
    let deltas = seen
        .iter()
        .filter(|event| is_named(event, "item.delta"))
        .count();
    assert!(
        deltas >= 2,
        "the text came in pieces while it was written, not at the end: {deltas} deltas"
    );
    assert_eq!(
        final_text(&seen),
        words,
        "the pieces and the finished message are the same text"
    );
    // The counts the plugin reports are disjoint (fresh input, cache read); the ledger's input has both.
    let counted = model_calls(&seen)
        .into_iter()
        .find(|call| call["request"]["tokens"]["output_tokens"] == 20);
    let counted = counted
        .unwrap_or_else(|| panic!("no model call with its counts in {:?}", model_calls(&seen)));
    assert_eq!(counted["request"]["tokens"]["input_tokens"], 1000);
    assert_eq!(counted["request"]["tokens"]["cached_input_tokens"], 400);
    assert_eq!(counted["request"]["outcome"]["status"], "done");
    only_model_requests_left(&rig).await;
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_question_a_command_and_a_plan_reach_the_person_and_their_answers_reach_the_model() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    rig.adapter.open_session(rig.spec.clone()).await.unwrap();

    // A question of the model is a card with its options; the choice goes back to the model.
    rig.mock.queue(&("tool ask_user_question {\"questions\":[{\"id\":\"choice\",\"header\":\"Choice\",\"question\":\"Which value?\",\"options\":[{\"label\":\"First\",\"description\":\"First value\"},{\"label\":\"Second\",\"description\":\"Second value\"}]}]}\nthen\nsay question finished\n".to_owned())).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, is_request_for_person).await;
    let asked = asked_of(seen.last().unwrap())
        .expect("the question reached the person")
        .clone();
    assert_eq!(asked.request.request_kind, HarnessRequestKind::UserInput);
    let HarnessRequestDetail::UserInput { questions, .. } = &asked.request.detail else {
        panic!("the question lost its shape: {:?}", asked.request.detail);
    };
    assert_eq!(questions[0].id, "choice");
    assert_eq!(questions[0].options.as_ref().map(Vec::len), Some(2));
    rig.adapter
        .resolve_approval(
            rig.spec.settings.thread_id,
            &asked.request_ref,
            ApprovalDecision::Answers {
                answers: BTreeMap::from([("choice".to_owned(), vec!["Second".to_owned()])]),
                note: None,
            },
        )
        .await
        .unwrap();
    let ended = until(&mut rig.events, turn_ended).await;
    assert_eq!(status_of(&ended), "completed");
    let told = last_tool_result(chats(&rig).last().unwrap());
    assert!(
        told.contains("Second") && !told.contains("First"),
        "the model was given the person's choice and no other: {told}"
    );

    // A command that changes something asks first, and a refusal keeps it from running.
    let refused = rig.work.path().join("refused.txt");
    rig.mock.queue(&(format!(
        "tool bash {}\nthen\nsay denied\n",
        serde_json::json!({"command": format!("touch {}", refused.display()), "description": "creates a file"})
    ))).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, is_request_for_person).await;
    let asked = asked_of(seen.last().unwrap())
        .expect("the command asked before it ran")
        .clone();
    assert_eq!(asked.request.request_kind, HarnessRequestKind::Command);
    assert!(
        !refused.exists(),
        "a command waiting for approval had its effect"
    );
    answer_with(&mut rig, &asked, OptionStyle::Deny, true).await;
    let ended = until(&mut rig.events, turn_ended).await;
    assert_eq!(status_of(&ended), "completed");
    assert!(!refused.exists(), "a refused command wrote its file");

    // The same command, allowed once, runs.
    let allowed = rig.work.path().join("allowed.txt");
    rig.mock.queue(&(format!(
        "tool bash {}\nthen\nsay done\n",
        serde_json::json!({"command": format!("touch {}", allowed.display()), "description": "creates a file"})
    ))).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, is_request_for_person).await;
    let asked = asked_of(seen.last().unwrap())
        .expect("the command asked before it ran")
        .clone();
    answer_with(&mut rig, &asked, OptionStyle::Allow, true).await;
    let ended = until(&mut rig.events, turn_ended).await;
    assert_eq!(status_of(&ended), "completed");
    assert!(allowed.exists(), "an allowed command did not run");

    // A command that only reads does not ask.
    rig.mock.queue(&("tool bash {\"command\":\"ls\",\"description\":\"lists the directory\"}\nthen\nsay listed\n".to_owned())).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, is_request_for_person).await;
    assert!(
        asked_of(seen.last().unwrap()).is_none(),
        "a command that only reads asked: {:?}",
        seen.iter().map(AdapterEvent::name).collect::<Vec<_>>()
    );
    assert_eq!(status_of(&seen), "completed");

    // Plan mode refuses a change; a revision stays a plan and Proceed restores Ask.
    let planned = rig.work.path().join("planned.txt");
    let after_plan = rig.work.path().join("after-plan.txt");
    assert_eq!(
        rig.spec.settings.approval_mode,
        orch_agent_contract::descriptor::ApprovalMode::Ask
    );
    rig.mock.queue(&(format!(
        "tool bash {}\nthen\ntool exit_plan_mode {}\nthen\ntool exit_plan_mode {}\nthen\ntool bash {}\nthen\nsay planned\n",
        serde_json::json!({"command": format!("touch {}", planned.display()), "description": "creates a file"}),
        serde_json::json!({"plan": "# Create the file\n\n1. touch it\n"}),
        serde_json::json!({"plan": "# Revised file change\n\n1. Read\n2. Explain rollback\n3. touch it\n"}),
        serde_json::json!({"command": format!("touch {}", after_plan.display()), "description": "creates a file after the plan"})
    ))).unwrap();
    let mut turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    turn.plan = true;
    rig.adapter.start_turn(turn).await.unwrap();
    let seen = until(&mut rig.events, is_request_for_person).await;
    let asked = asked_of(seen.last().unwrap())
        .expect("the plan reached the person")
        .clone();
    assert_eq!(asked.request.request_kind, HarnessRequestKind::Plan);
    let HarnessRequestDetail::Plan { plan } = &asked.request.detail else {
        panic!("the plan lost its shape: {:?}", asked.request.detail);
    };
    assert!(plan.starts_with("# Create the file"), "{plan}");
    assert!(!planned.exists(), "a change was made while planning");
    let ids: Vec<&str> = asked
        .request
        .options
        .iter()
        .map(|option| option.id.as_str())
        .collect();
    assert_eq!(ids, ["proceed", "keep_planning"]);
    let feedback = "Explain rollback first. 请先读配置。";
    rig.adapter
        .resolve_approval(
            rig.spec.settings.thread_id,
            &asked.request_ref,
            ApprovalDecision::Option {
                option_id: "keep_planning".into(),
                note: Some(feedback.into()),
            },
        )
        .await
        .unwrap();
    let revision = until(&mut rig.events, is_request_for_person).await;
    let revised = asked_of(revision.last().unwrap())
        .expect("a revised plan reaches the person")
        .clone();
    assert_eq!(revised.request.request_kind, HarnessRequestKind::Plan);
    assert_ne!(revised.request_ref, asked.request_ref);
    let HarnessRequestDetail::Plan { plan } = &revised.request.detail else {
        panic!("a revised plan")
    };
    assert!(plan.starts_with("# Revised file change"), "{plan}");
    let told = last_tool_result(chats(&rig).last().unwrap());
    assert!(
        told.contains(feedback),
        "the model received the exact revision words: {told}"
    );
    assert!(!planned.exists(), "keeping a plan permitted an action");
    assert!(!after_plan.exists());
    let asked = revised;
    rig.adapter
        .resolve_approval(
            rig.spec.settings.thread_id,
            &asked.request_ref,
            ApprovalDecision::Option {
                option_id: "proceed".to_owned(),
                note: None,
            },
        )
        .await
        .unwrap();
    let continuation = until(&mut rig.events, is_request_for_person).await;
    let action = asked_of(continuation.last().unwrap())
        .expect("Ask remains in force after Proceed")
        .clone();
    assert_eq!(action.request.request_kind, HarnessRequestKind::Command);
    assert!(
        !after_plan.exists(),
        "Proceed cannot bypass the composer's Ask mode"
    );
    answer_with(&mut rig, &action, OptionStyle::Deny, true).await;
    let ended = until(&mut rig.events, turn_ended).await;
    assert_eq!(status_of(&ended), "completed");
    assert!(!planned.exists(), "the refused change ran after all");
    assert!(!after_plan.exists(), "the denied action after Proceed ran");

    only_model_requests_left(&rig).await;
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_message_added_to_a_running_turn_reaches_the_model_and_a_stop_ends_the_turn_as_stopped() {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    rig.adapter.open_session(rig.spec.clone()).await.unwrap();

    // The first round writes slowly and ends in a call that only reads; what is added arrives before the next round.
    rig.mock.queue(&("chunk 4\ndelay 150\nsay Working on it, one moment please.\ntool bash {\"command\":\"ls\",\"description\":\"lists\"}\nthen\nsay finished\n".to_owned())).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    let turn_id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let began = until(&mut rig.events, |event| is_named(event, "item.delta")).await;
    assert!(!began.is_empty());
    rig.adapter
        .steer(TurnSteer {
            thread_id: rig.spec.settings.thread_id,
            turn_id,
            text: "Also mention the colour purple".to_owned(),
            images: Vec::new(),
        })
        .await
        .unwrap();
    let ended = until(&mut rig.events, turn_ended).await;
    assert_eq!(status_of(&ended), "completed");
    let rounds = chats(&rig);
    assert!(
        rounds
            .iter()
            .skip(1)
            .any(|body| body.to_string().contains("Also mention the colour purple")),
        "no later request of the model holds what was added: {} requests",
        rounds.len()
    );

    // A turn that is stopped while the model writes ends as stopped, and its call is reported as cut short.
    rig.mock.queue(&("chunk 3\ndelay 400\nsay Once upon a time there was a very long sentence that goes on and on and on.\n".to_owned())).unwrap();
    let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
    let turn_id = turn.turn_id;
    let before = rig.mock.requests().len();
    rig.adapter.start_turn(turn).await.unwrap();
    let deadline = std::time::Instant::now() + WAIT;
    while rig.mock.requests().len() == before {
        assert!(
            std::time::Instant::now() < deadline,
            "the program never asked the model"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    rig.adapter
        .interrupt(rig.spec.settings.thread_id, turn_id)
        .await
        .unwrap();
    let seen = until(&mut rig.events, turn_ended).await;
    assert_eq!(
        status_of(&seen),
        "interrupted",
        "{:?}",
        seen.iter().map(AdapterEvent::name).collect::<Vec<_>>()
    );

    only_model_requests_left(&rig).await;
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn whole_and_bounded_histories_are_imported_through_the_adapter_and_bridge() {
    let Some(binary) = dsh_binary() else { return };
    for through in [None, Some(1)] {
        let mut rig = rig(&binary).await;
        let history = [
            serde_json::json!({ "kind": "user_message", "status": "completed", "stream": "main", "turn_id": "t1", "payload": { "parts": [{ "type": "text", "text": "Remember the cedar helper" }] } }),
            serde_json::json!({ "kind": "agent_message", "status": "completed", "stream": "main", "turn_id": "t1", "payload": { "text": "The cedar helper was renamed" } }),
            serde_json::json!({ "kind": "user_message", "status": "completed", "stream": "main", "turn_id": "t2", "payload": { "parts": [{ "type": "text", "text": "Remember the later spruce tests" }] } }),
            serde_json::json!({ "kind": "agent_message", "status": "completed", "stream": "main", "turn_id": "t2", "payload": { "text": "The later spruce tests passed" } }),
        ].iter().map(Json::to_string).collect::<Vec<_>>().join("\n") + "\n";
        let file = rig.work.path().join("carried.jsonl");
        std::fs::write(&file, &history).unwrap();
        rig.spec.settings.mode = SessionMode::ImportNeutral;
        rig.spec.settings.upto_turn_ordinal = through;
        rig.spec.history = Some(orch_agent_contract::session::LocalFile {
            path: file.display().to_string(),
            size: i64::try_from(history.len()).unwrap(),
        });
        let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
        assert!(
            !opened.resumed,
            "a carried history creates a session of its own"
        );
        let report = opened
            .import_report
            .expect("the bridge reports what it carried");
        assert_eq!(report.items_total, if through.is_some() { 2 } else { 4 });
        rig.mock.queue("say carried on\n").unwrap();
        rig.adapter
            .start_turn(turn_for(&rig.spec, "Continue with the carried history"))
            .await
            .unwrap();
        let ended = until(&mut rig.events, turn_ended).await;
        assert_eq!(status_of(&ended), "completed");
        assert!(final_text(&ended).contains("carried on"));
        let requests = chats(&rig);
        let messages = requests.last().unwrap()["messages"].to_string();
        assert!(messages.contains("Remember the cedar helper"), "{messages}");
        assert!(
            messages.contains("The cedar helper was renamed"),
            "{messages}"
        );
        assert_eq!(
            messages.contains("Remember the later spruce tests"),
            through.is_none(),
            "{messages}"
        );
        assert_eq!(
            messages.contains("The later spruce tests passed"),
            through.is_none(),
            "{messages}"
        );
        only_model_requests_left(&rig).await;
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn the_first_question_of_a_program_started_again_is_a_new_card_and_its_answer_reaches_the_model()
 {
    let Some(binary) = dsh_binary() else { return };
    let mut rig = rig(&binary).await;
    let opened = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
    // Orch keeps one approval for each reference a thread was ever given, and takes a second request with a reference
    // it has seen for the first again, which it does not show.
    let mut known = std::collections::HashSet::new();
    for (round, choice) in ["First", "Second"].into_iter().enumerate() {
        if round == 1 {
            // The program's process ends, and the thread's next message opens its session again in a new process.
            rig.adapter
                .close_session(rig.spec.settings.thread_id)
                .await
                .unwrap();
            rig.spec.settings.mode = SessionMode::Resume;
            rig.spec.settings.native_session_ref = Some(opened.native_session_ref.clone());
            let again = rig.adapter.open_session(rig.spec.clone()).await.unwrap();
            assert!(again.resumed, "the session was opened again, not made anew");
        }
        rig.mock.queue(&("tool ask_user_question {\"questions\":[{\"id\":\"choice\",\"header\":\"Choice\",\"question\":\"Which value?\",\"options\":[{\"label\":\"First\",\"description\":\"First value\"},{\"label\":\"Second\",\"description\":\"Second value\"}]}]}\nthen\nsay question finished\n".to_owned())).unwrap();
        let turn = turn_for(&rig.spec, "Run the scripted conformance case.");
        rig.adapter.start_turn(turn).await.unwrap();
        let seen = until(&mut rig.events, is_request_for_person).await;
        let asked = asked_of(seen.last().unwrap())
            .unwrap_or_else(|| panic!("the question of round {round} never reached the person"))
            .clone();
        assert_eq!(asked.request.request_kind, HarnessRequestKind::UserInput);
        assert!(
            known.insert(asked.request_ref.clone()),
            "{} was the reference of an earlier request of this thread, and Orch would show no card for it",
            asked.request_ref
        );
        rig.adapter
            .resolve_approval(
                rig.spec.settings.thread_id,
                &asked.request_ref,
                ApprovalDecision::Answers {
                    answers: BTreeMap::from([("choice".to_owned(), vec![choice.to_owned()])]),
                    note: None,
                },
            )
            .await
            .unwrap();
        let ended = until(&mut rig.events, turn_ended).await;
        assert_eq!(status_of(&ended), "completed");
        let told = last_tool_result(chats(&rig).last().unwrap());
        assert!(
            told.contains(choice),
            "the model was given the choice of round {round}: {told}"
        );
    }
    only_model_requests_left(&rig).await;
    rig.adapter.stop().await.unwrap();
}
