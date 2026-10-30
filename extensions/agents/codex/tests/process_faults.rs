//! A harness process that is killed, prints half a line, goes silent, or waits for the network without end: the turn ends with what was last seen, the next one gets a harness that takes the session up, and nothing waits.
//!
//! The real app-server cannot be made to do these things on purpose (it waits for the network for ever
//! against an address nobody listens on, and has no deadline for the first byte of an answer), so the harness
//! is played in the test, over pipes in the process. A harness that refuses to start is in `tests/fake.rs`.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::fake::FakeLauncher;
use common::{kinds, plain_spec};
use futures::StreamExt;
use orch_adapter_codex::CodexAdapter;
use orch_adapter_codex::failure::Patience;
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{ApprovalRequested, SessionEvent, TurnOutcome};
use orch_agent_contract::facts::{AgentProfile, HarnessState};
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{ApprovalDecision, SessionMode, TurnStart};
use orch_plugin_contract::Secret;
use orch_types::{Id, Json, new_id};
use serde_json::json;

struct Setup {
    adapter: CodexAdapter,
    events: EventStream,
    sink: EventSink,
    fake: Arc<FakeLauncher>,
    home: tempfile::TempDir,
}

fn setup(patience: Patience) -> Setup {
    let fake = Arc::new(FakeLauncher::new(|method, _| match method {
        "thread/start" | "thread/resume" => {
            Ok(json!({ "thread": { "id": "thr_1", "path": null } }))
        }
        "turn/start" => Ok(json!({ "turn": { "id": "turn_1", "status": "inProgress" } })),
        _ => Ok(json!({})),
    }));
    let adapter = CodexAdapter::with_launcher(
        Arc::clone(&fake) as Arc<dyn orch_adapter_codex::process::Launcher>
    )
    .with_patience(patience);
    let (sink, events) = EventSink::channel();
    Setup {
        adapter,
        events,
        sink,
        fake,
        home: tempfile::tempdir().unwrap(),
    }
}

/// Patience that is out in a second, for what is waited for.
fn impatient() -> Patience {
    Patience {
        silence: Some(Duration::from_secs(1)),
        retrying: Some(Duration::from_secs(1)),
    }
}

impl Setup {
    async fn boot(&mut self) {
        let start = AgentStart {
            provider_id: new_id(),
            agent_id: new_id(),
            harness_kind: "codex".into(),
            profile: AgentProfile::default(),
            home: Some(self.home.path().to_string_lossy().into_owned()),
            provider_config: json!({}),
            loopback: Loopback::new(41999, Secret::new("secret-token")),
            binary: None,
        };
        self.adapter.start(start, self.sink.clone()).await.unwrap();
    }

    async fn next(&mut self) -> AdapterEvent {
        tokio::time::timeout(Duration::from_secs(30), self.events.next())
            .await
            .expect("an event")
            .expect("open")
    }

    async fn until(&mut self, done: impl Fn(&AdapterEvent) -> bool) -> Vec<AdapterEvent> {
        let mut taken = Vec::new();
        loop {
            let event = self.next().await;
            let finished = done(&event);
            taken.push(event);
            if finished {
                return taken;
            }
        }
    }

    /// Opens a session and starts a turn that the harness has taken in.
    async fn running_turn(&mut self, mode: SessionMode, native: Option<&str>) -> (Id, TurnStart) {
        let thread = new_id();
        let turn = self.running_turn_of(thread, mode, native).await;
        (thread, turn)
    }

    /// Opens the session of `thread` and starts a turn that the harness has taken in.
    async fn running_turn_of(
        &mut self,
        thread: Id,
        mode: SessionMode,
        native: Option<&str>,
    ) -> TurnStart {
        let spec = plain_spec(
            "/workspace/web",
            new_id(),
            new_id(),
            new_id(),
            thread,
            mode,
            native,
        );
        self.adapter.open_session(spec).await.unwrap();
        let turn = TurnStart {
            plan: false,
            thread_id: thread,
            turn_id: new_id(),
            text: "go".into(),
            images: Vec::new(),
            model: "mock-large".into(),
            effort: None,
            params: json!({}),
            approval_mode: ApprovalMode::Ask,
            sandbox_mode: SandboxMode::Workspace,
        };
        self.adapter.start_turn(turn.clone()).await.unwrap();
        self.fake.state.notify(
            "turn/started",
            json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "inProgress" } }),
        );
        self.until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnStarted(_),
                    ..
                }
            )
        })
        .await;
        turn
    }

    /// The harness asks to run `command`, as its request number `id`; what the adapter reports of it.
    async fn asked_to_run(&mut self, id: i64, item: &str, command: &str) -> ApprovalRequested {
        self.fake.state.ask(
            id,
            "item/commandExecution/requestApproval",
            json!({ "threadId": "thr_1", "turnId": "turn_1", "itemId": item, "command": command,
                    "cwd": "/workspace/web", "availableDecisions": ["accept", "cancel"] }),
        );
        self.reported_request().await
    }

    /// The harness puts a question to the person, as its request number `id`; what the adapter reports of it.
    async fn asked_a_question(&mut self, id: i64, item: &str) -> ApprovalRequested {
        self.fake.state.ask(
            id,
            "item/tool/requestUserInput",
            json!({ "threadId": "thr_1", "turnId": "turn_1", "itemId": item, "isBlocking": true,
                    "autoResolutionMs": null,
                    "questions": [ { "id": "db", "header": "Database", "question": "Which database should it use?",
                                     "isOther": true, "isSecret": false,
                                     "options": [ { "label": "Postgres", "description": "The default" },
                                                  { "label": "SQLite", "description": "A file" } ] } ] }),
        );
        self.reported_request().await
    }

    async fn reported_request(&mut self) -> ApprovalRequested {
        let events = self
            .until(|event| {
                matches!(
                    event,
                    AdapterEvent::Session {
                        event: SessionEvent::ApprovalRequested(_),
                        ..
                    }
                )
            })
            .await;
        let Some(SessionEvent::ApprovalRequested(request)) = session(&events).pop() else {
            panic!("a request");
        };
        request
    }

    /// The answer the adapter gave the harness to its request number `id`, once it has.
    async fn answer_to(&self, id: i64) -> Json {
        for _ in 0..100 {
            if let Some(answer) = self.fake.state.answer_to(id) {
                return answer;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("no answer to request {id}");
    }
}

fn once() -> ApprovalDecision {
    ApprovalDecision::Option {
        option_id: "once".into(),
        note: None,
    }
}

fn ended(events: &[AdapterEvent]) -> Option<&orch_agent_contract::events::TurnCompleted> {
    events.iter().find_map(|event| match event {
        AdapterEvent::Session {
            event: SessionEvent::TurnCompleted(ended),
            ..
        } => Some(ended),
        _ => None,
    })
}

#[tokio::test]
async fn a_process_exit_cannot_be_hidden_by_open_pipes_and_both_session_and_provider_keep_its_reason()
 {
    let mut setup = setup(Patience::default());
    setup.fake.state.set_stderr("fatal-managed-server-reason\n");
    setup.boot().await;
    setup.running_turn(SessionMode::New, None).await;
    setup.fake.state.notify("item/completed",json!({"threadId":"thr_1","turnId":"turn_1","item":{"type":"agentMessage","id":"last-message","text":"Last words before exit","phase":null}}));
    setup.fake.state.exit_with_open_pipes();
    let events = tokio::time::timeout(Duration::from_secs(3), setup.until(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Error))).await.unwrap();
    assert!(session(&events).iter().any(|event| matches!(event, SessionEvent::SessionLost(lost) if lost.reason.contains("fatal-managed-server-reason"))));
    assert!(events.iter().any(|event| matches!(event, AdapterEvent::Status(status) if status.detail.as_deref().is_some_and(|reason| reason.contains("fatal-managed-server-reason")))));
    assert!(session(&events).iter().any(|event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref=="last-message" && item.payload["text"]=="Last words before exit")), "the last readable frame is retained before the loss");
    setup.adapter.stop().await.unwrap();
}

fn session(events: &[AdapterEvent]) -> Vec<SessionEvent> {
    common::session_events(events)
}

#[tokio::test]
async fn a_process_that_is_killed_in_the_middle_of_a_turn_loses_the_session_and_a_restart_takes_it_up()
 {
    let mut setup = setup(Patience::default());
    setup.boot().await;
    let (thread, _) = setup.running_turn(SessionMode::New, None).await;
    setup.fake.state.notify(
        "item/started",
        json!({ "threadId": "thr_1", "turnId": "turn_1",
                "item": { "type": "agentMessage", "id": "m1", "text": "I was say", "phase": null } }),
    );
    setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::ItemStarted(_),
                    ..
                }
            )
        })
        .await;
    setup.fake.state.die();
    let events = setup
        .until(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Error))
        .await;
    let seen = session(&events);
    assert!(
        seen.iter()
            .any(|event| matches!(event, SessionEvent::SessionLost(_))),
        "{:?}",
        kinds(&seen)
    );
    assert!(
        seen.iter().any(
            |event| matches!(event, SessionEvent::ItemCompleted(item) if item.native_ref == "m1")
        ),
        "what was open ends: {:?}",
        kinds(&seen)
    );
    // Started again, the harness serves the session it had, by its id.
    setup.boot().await;
    assert_eq!(
        setup
            .fake
            .state
            .launches
            .load(std::sync::atomic::Ordering::SeqCst),
        2
    );
    let spec = plain_spec(
        "/workspace/web",
        new_id(),
        new_id(),
        new_id(),
        thread,
        SessionMode::Resume,
        Some("thr_1"),
    );
    assert!(setup.adapter.open_session(spec).await.unwrap().resumed);
}

/// The app-server counts the requests it makes of the client from 0 in each process; Orch keeps one approval for
/// each reference of a thread for the thread's whole life and drops a request that comes under a reference it has
/// seen. So a reference that was only that number hid the first requests of every process started after the first
/// one in a thread: nobody was asked, and the turn waited for an answer nobody could give.
#[tokio::test]
async fn the_requests_of_a_harness_started_again_are_new_ones_though_it_numbers_them_from_0_again()
{
    let mut setup = setup(Patience::default());
    setup.boot().await;
    let (thread, _) = setup.running_turn(SessionMode::New, None).await;
    // The first process asks for a command (its request 0) and puts a question (its request 1).
    let command_before = setup.asked_to_run(0, "c_before", "ls").await;
    let question_before = setup.asked_a_question(1, "q_before").await;
    setup.fake.state.die();
    setup
        .until(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Error))
        .await;

    // Started again, the harness takes the thread up and asks the same two things, numbered 0 and 1 again.
    setup.boot().await;
    setup
        .running_turn_of(thread, SessionMode::Resume, Some("thr_1"))
        .await;
    let command = setup.asked_to_run(0, "c_after", "pwd").await;
    let question = setup.asked_a_question(1, "q_after").await;
    let references = [
        &command_before.request_ref,
        &question_before.request_ref,
        &command.request_ref,
        &question.request_ref,
    ];
    for (at, reference) in references.iter().enumerate() {
        for other in &references[at + 1..] {
            assert_ne!(
                reference, other,
                "two requests of one thread share a reference"
            );
        }
    }
    for (at, reference) in references.iter().enumerate() {
        assert!(
            reference.ends_with(if at % 2 == 0 { ":0" } else { ":1" }),
            "the reference names the harness's own number: {reference}"
        );
    }

    // The person's answer to the card of the process that is gone has nowhere to go; it must not reach the request
    // that has the same number in this process.
    let stale = setup
        .adapter
        .resolve_approval(thread, &command_before.request_ref, once())
        .await
        .unwrap_err();
    assert_eq!(stale.code(), "not_found");
    assert!(setup.fake.state.answer_to(0).is_none());
    assert!(setup.fake.state.answer_to(1).is_none());

    // The requests of this process are answered, each as the request number the harness gave it.
    setup
        .adapter
        .resolve_approval(
            thread,
            &question.request_ref,
            ApprovalDecision::Answers {
                answers: [("db".to_owned(), vec!["Postgres".to_owned()])]
                    .into_iter()
                    .collect(),
                note: None,
            },
        )
        .await
        .unwrap();
    setup
        .adapter
        .resolve_approval(thread, &command.request_ref, once())
        .await
        .unwrap();
    assert_eq!(
        setup.answer_to(1).await["result"],
        json!({ "answers": { "db": { "answers": ["Postgres"] } } })
    );
    assert_eq!(
        setup.answer_to(0).await["result"],
        json!({ "decision": "accept" })
    );
    setup.adapter.stop().await.unwrap();
}

/// Two harness processes that serve one thread at the same moment (a thread taken over by another connector while
/// the first still answers) both make a request number 0: there are two requests, and each answer goes to the
/// process that asked.
#[tokio::test]
async fn two_processes_that_both_make_request_0_for_one_thread_make_two_requests_each_answered_in_its_own()
 {
    let mut first = setup(Patience::default());
    let mut second = setup(Patience::default());
    first.boot().await;
    second.boot().await;
    let thread = new_id();
    first.running_turn_of(thread, SessionMode::New, None).await;
    second.running_turn_of(thread, SessionMode::New, None).await;
    let ls = first.asked_to_run(0, "c_ls", "ls").await;
    let pwd = second.asked_to_run(0, "c_pwd", "pwd").await;
    assert_ne!(ls.request_ref, pwd.request_ref);

    // A reference of the other process names nothing here.
    let wrong = first
        .adapter
        .resolve_approval(thread, &pwd.request_ref, once())
        .await
        .unwrap_err();
    assert_eq!(wrong.code(), "not_found");
    assert!(first.fake.state.answer_to(0).is_none());
    assert!(second.fake.state.answer_to(0).is_none());

    first
        .adapter
        .resolve_approval(thread, &ls.request_ref, once())
        .await
        .unwrap();
    second
        .adapter
        .resolve_approval(
            thread,
            &pwd.request_ref,
            ApprovalDecision::Option {
                option_id: "deny".into(),
                note: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        first.answer_to(0).await["result"],
        json!({ "decision": "accept" })
    );
    assert_eq!(
        second.answer_to(0).await["result"],
        json!({ "decision": "decline" })
    );
    first.adapter.stop().await.unwrap();
    second.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn half_a_line_and_the_end_of_the_process_is_a_lost_session_and_the_half_line_is_nothing() {
    let mut setup = setup(Patience::default());
    setup.boot().await;
    setup.running_turn(SessionMode::New, None).await;
    setup
        .fake
        .state
        .say_raw(r#"{"method":"item/started","params":{"threadId":"thr_1","item":{"ty"#);
    // The half line has left before the process ends.
    tokio::time::sleep(Duration::from_millis(200)).await;
    setup.fake.state.die();
    let events = setup
        .until(|event| matches!(event, AdapterEvent::Status(status) if status.status == HarnessState::Error))
        .await;
    let seen = session(&events);
    assert!(
        seen.iter()
            .any(|event| matches!(event, SessionEvent::SessionLost(_)))
    );
    assert!(
        seen.iter()
            .all(|event| !matches!(event, SessionEvent::ItemStarted(_))),
        "nothing is made of half a line: {:?}",
        kinds(&seen)
    );
}

#[tokio::test]
async fn a_harness_that_says_nothing_while_the_model_is_awaited_has_its_turn_ended_and_is_asked_to_stop_it()
 {
    let mut setup = setup(impatient());
    setup.boot().await;
    let (thread, _) = setup.running_turn(SessionMode::New, None).await;
    let began = Instant::now();
    let events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnCompleted(_),
                    ..
                }
            )
        })
        .await;
    assert!(
        began.elapsed() < Duration::from_secs(20),
        "nothing waits for ever"
    );
    let ended = ended(&events).unwrap();
    assert_eq!(ended.status, TurnOutcome::Failed);
    let error = ended.error.as_ref().unwrap();
    assert_eq!(error.code, "harness_error");
    assert!(error.retryable);
    assert!(
        error
            .said
            .as_deref()
            .unwrap()
            .contains("no word from the harness"),
        "{error:?}"
    );
    // The harness is asked to stop the turn, and its own end of it is not reported twice.
    let calls = setup.fake.state.calls("turn/interrupt");
    assert_eq!(calls.len(), 1, "{calls:?}");
    assert_eq!(
        calls[0]["params"],
        json!({ "threadId": "thr_1", "turnId": "turn_1" })
    );
    setup.fake.state.notify(
        "turn/completed",
        json!({ "threadId": "thr_1", "turn": { "id": "turn_1", "status": "interrupted" } }),
    );
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut late = Vec::new();
    while let Ok(event) = setup.events.try_recv() {
        late.push(event);
    }
    assert!(
        ended_in(&late).is_none(),
        "the end the harness gives later is dropped"
    );
    // The same harness takes the next turn.
    setup.fake.state.notify(
        "turn/started",
        json!({ "threadId": "thr_1", "turn": { "id": "turn_2", "status": "inProgress" } }),
    );
    let next = TurnStart {
        plan: false,
        thread_id: thread,
        turn_id: new_id(),
        text: "again".into(),
        images: Vec::new(),
        model: "mock-large".into(),
        effort: None,
        params: json!({}),
        approval_mode: ApprovalMode::Ask,
        sandbox_mode: SandboxMode::Workspace,
    };
    setup.adapter.start_turn(next).await.unwrap();
}

fn ended_in(events: &[AdapterEvent]) -> Option<&orch_agent_contract::events::TurnCompleted> {
    ended(events)
}

#[tokio::test]
async fn a_harness_that_waits_for_the_network_without_end_is_stopped_when_its_patience_is_out() {
    let mut setup = setup(Patience {
        silence: None,
        retrying: Some(Duration::from_secs(1)),
    });
    setup.boot().await;
    setup.running_turn(SessionMode::New, None).await;
    // It is heard every few moments, so only the patience for trying again can end this.
    let fake = Arc::clone(&setup.fake);
    let talker = tokio::spawn(async move {
        for n in 1..=40 {
            fake.state.notify(
                "error",
                json!({ "threadId": "thr_1", "turnId": "turn_1", "willRetry": true,
                        "error": { "message": "Reconnecting... waiting for network",
                                   "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": null } },
                                   "additionalDetails": format!("Connection failed: error sending request ({n})") } }),
            );
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    });
    let events = setup
        .until(|event| {
            matches!(
                event,
                AdapterEvent::Session {
                    event: SessionEvent::TurnCompleted(_),
                    ..
                }
            )
        })
        .await;
    talker.abort();
    let ended = ended(&events).unwrap();
    assert_eq!(ended.status, TurnOutcome::Failed);
    let error = ended.error.as_ref().unwrap();
    assert_eq!(error.code, "provider_unreachable");
    assert!(error.retryable);
    let said = error.said.as_deref().unwrap();
    assert!(
        said.contains("Connection failed: error sending request"),
        "{said}"
    );
    assert!(said.contains("tries over"), "{said}");
}

#[tokio::test]
async fn a_command_that_runs_for_long_is_not_the_silence_of_the_model() {
    let mut setup = setup(Patience {
        silence: Some(Duration::from_secs(1)),
        retrying: None,
    });
    setup.boot().await;
    setup.running_turn(SessionMode::New, None).await;
    setup.fake.state.notify(
        "item/started",
        json!({ "threadId": "thr_1", "turnId": "turn_1",
                "item": { "type": "commandExecution", "id": "c1", "command": "sleep 100", "cwd": "/workspace/web",
                          "processId": null, "source": "agent", "status": "inProgress", "commandActions": [],
                          "aggregatedOutput": null, "exitCode": null, "durationMs": null } }),
    );
    tokio::time::sleep(Duration::from_millis(3200)).await;
    let mut seen = Vec::new();
    while let Ok(event) = setup.events.try_recv() {
        seen.push(event);
    }
    assert!(ended_in(&seen).is_none(), "{:?}", kinds(&session(&seen)));
}
