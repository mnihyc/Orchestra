//! Stop ends the current turn's terminals, and a vendor refusal ends the turn on its first notice.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use common::fake::{FakeLauncher, FakeState};
use common::{plain_spec, session_events};
use futures::StreamExt;
use orch_adapter_codex::CodexAdapter;
use orch_agent_contract::adapter::{
    AdapterEvent, AgentAdapter, AgentStart, EventSink, EventStream,
};
use orch_agent_contract::descriptor::{ApprovalMode, SandboxMode};
use orch_agent_contract::events::{
    ItemContent, ItemStatus, SessionEvent, ToolPayload, TurnOutcome,
};
use orch_agent_contract::facts::AgentProfile;
use orch_agent_contract::loopback::Loopback;
use orch_agent_contract::session::{SessionMode, TurnStart};
use orch_plugin_contract::Secret;
use orch_types::{Id, Json, new_id};
use serde_json::json;

const WAIT: Duration = Duration::from_secs(10);

#[tokio::test]
async fn native_end_names_one_reviewed_command_and_keeps_another_turns_work_running() {
    use orch_agent_contract::background::BackgroundEndState;
    let mut rig = Scripted::new(true).await;
    let older = rig.begin().await;
    rig.running_command("retained", "11").await;
    rig.fake.state.notify(
        "turn/completed",
        json!({"threadId":"thread","turn":{"id":"turn-1","status":"completed"}}),
    );
    rig.until(|event| matches!(event,SessionEvent::TurnCompleted(end) if end.turn_id==older))
        .await;
    let current = rig.begin().await;
    rig.running_command("current", "22").await;
    let roster = rig.adapter.background_commands(rig.thread).await.unwrap();
    assert_eq!(roster.total, Some(2));
    assert!(roster.items.iter().all(|command| command.since.is_none()
        && command.cpu_percent.is_none()
        && command.memory_bytes.is_none()));
    let old = roster
        .items
        .iter()
        .find(|command| command.native_item_ref.as_deref() == Some("retained"))
        .unwrap();
    assert_eq!(old.turn_id, Some(older));
    let old_id = old.id.clone();
    assert_eq!(
        rig.adapter
            .end_background_command(rig.thread, &old_id)
            .await
            .unwrap()
            .state,
        BackgroundEndState::Ended
    );
    assert_eq!(
        rig.adapter
            .end_background_command(rig.thread, &old_id)
            .await
            .unwrap()
            .state,
        BackgroundEndState::AlreadyEnded
    );
    assert_eq!(rig.alive.lock().unwrap().len(), 1);
    assert_eq!(rig.alive.lock().unwrap()[0]["itemId"], "current");
    assert_eq!(
        rig.fake.state.calls("turn/start").len(),
        2,
        "a native read and End never send input to the model"
    );
    assert_eq!(
        rig.adapter
            .end_background_command(rig.thread, "22")
            .await
            .unwrap()
            .state,
        BackgroundEndState::Unknown,
        "an operating-system or native process number alone is not authority"
    );
    assert!(
        rig.adapter.background_commands(new_id()).await.is_err(),
        "another thread never resolves to this session"
    );
    let now = rig.adapter.background_commands(rig.thread).await.unwrap();
    assert_eq!(now.items[0].turn_id, Some(current));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_declined_exact_end_stays_unknown_and_a_reused_process_number_cannot_replace_the_reviewed_item()
 {
    use orch_agent_contract::background::BackgroundEndState;
    let mut rig = Scripted::new(true).await;
    rig.begin().await;
    rig.running_command("old-item", "22").await;
    let roster = rig.adapter.background_commands(rig.thread).await.unwrap();
    let id = roster.items[0].id.clone();
    *rig.decline_terminal.lock().unwrap() = Some("22".into());
    assert_eq!(
        rig.adapter
            .end_background_command(rig.thread, &id)
            .await
            .unwrap()
            .state,
        BackgroundEndState::Unknown
    );
    assert_eq!(rig.alive.lock().unwrap().len(), 1);
    rig.alive.lock().unwrap()[0]["itemId"] = json!("replacement-item");
    let before = rig
        .fake
        .state
        .calls("thread/backgroundTerminals/terminate")
        .len();
    assert_eq!(
        rig.adapter
            .end_background_command(rig.thread, &id)
            .await
            .unwrap()
            .state,
        BackgroundEndState::Unknown
    );
    assert_eq!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/terminate")
            .len(),
        before,
        "a replaced item is never ended by an old reviewed id"
    );
    rig.adapter.stop().await.unwrap();
}

struct Scripted {
    adapter: CodexAdapter,
    events: EventStream,
    fake: Arc<FakeLauncher>,
    _home: tempfile::TempDir,
    thread: Id,
    turns: u32,
    alive: Arc<Mutex<Vec<Json>>>,
    decline_terminal: Arc<Mutex<Option<String>>>,
}

fn command(item: &str, process: &str, status: &str) -> Json {
    json!({ "id": item, "type": "commandExecution", "command": "sleep 300",
        "cwd": "/workspace", "processId": process, "status": status,
        "exitCode": (status == "failed").then_some(-1), "aggregatedOutput": "" })
}

impl Scripted {
    async fn new(terminals_supported: bool) -> Self {
        let state = Arc::new(OnceLock::<Arc<FakeState>>::new());
        let handler_state = Arc::clone(&state);
        let alive = Arc::new(Mutex::new(Vec::<Json>::new()));
        let handler_alive = Arc::clone(&alive);
        let decline_terminal = Arc::new(Mutex::new(None::<String>));
        let handler_decline = Arc::clone(&decline_terminal);
        let counter = Arc::new(Mutex::new(0));
        let fake = Arc::new(FakeLauncher::new(move |method, params| {
            let state = handler_state.get().unwrap();
            match method {
                "thread/start" => Ok(json!({ "thread": { "id": "thread", "path": null } })),
                "turn/start" => {
                    let mut n = counter.lock().unwrap();
                    *n += 1;
                    let turn = format!("turn-{n}");
                    state.notify("turn/started", json!({ "threadId": "thread", "turn": { "id": turn, "status": "inProgress" } }));
                    Ok(json!({ "turn": { "id": turn, "status": "inProgress" } }))
                }
                "turn/interrupt" => {
                    // The real app server ends the turn but leaves the process alive until terminate.
                    state.notify("turn/completed", json!({ "threadId": "thread", "turn": { "id": params["turnId"], "status": "interrupted" } }));
                    Ok(json!({}))
                }
                "thread/backgroundTerminals/list" if !terminals_supported => {
                    Err((-32601, "method not found".into()))
                }
                "thread/backgroundTerminals/list" => {
                    let live = handler_alive.lock().unwrap();
                    // A one-element page exercises the native cursor contract, including earlier turns.
                    let index = params["cursor"]
                        .as_str()
                        .and_then(|s| s.parse::<usize>().ok())
                        .unwrap_or(0);
                    let data: Vec<_> = live.get(index).cloned().into_iter().collect();
                    let next = (index + 1 < live.len()).then(|| (index + 1).to_string());
                    Ok(json!({ "data": data, "nextCursor": next }))
                }
                "thread/backgroundTerminals/terminate" => {
                    if handler_decline.lock().unwrap().as_deref() == params["processId"].as_str() {
                        return Ok(json!({ "terminated": false }));
                    }
                    let mut live = handler_alive.lock().unwrap();
                    let index = live
                        .iter()
                        .position(|item| item["processId"] == params["processId"])
                        .unwrap();
                    let terminal = live.remove(index);
                    state.notify("item/completed", json!({ "threadId": "thread", "turnId": terminal["turnId"],
                        "item": command(terminal["itemId"].as_str().unwrap(), terminal["processId"].as_str().unwrap(), "failed") }));
                    Ok(json!({ "terminated": true }))
                }
                _ => Ok(json!({})),
            }
        }));
        state.set(Arc::clone(&fake.state)).ok().unwrap();
        let (sink, events) = EventSink::channel();
        let home = tempfile::tempdir().unwrap();
        let mut adapter = CodexAdapter::with_launcher(fake.clone());
        adapter
            .start(
                AgentStart {
                    provider_id: new_id(),
                    agent_id: new_id(),
                    harness_kind: "codex".into(),
                    profile: AgentProfile::default(),
                    home: Some(home.path().display().to_string()),
                    provider_config: json!({}),
                    loopback: Loopback::new(41999, Secret::new("scripted")),
                    binary: None,
                },
                sink,
            )
            .await
            .unwrap();
        let thread = new_id();
        adapter
            .open_session(plain_spec(
                "/workspace",
                new_id(),
                new_id(),
                new_id(),
                thread,
                SessionMode::New,
                None,
            ))
            .await
            .unwrap();
        Self {
            adapter,
            events,
            fake,
            _home: home,
            thread,
            turns: 0,
            alive,
            decline_terminal,
        }
    }

    async fn until(&mut self, done: impl Fn(&SessionEvent) -> bool) -> Vec<SessionEvent> {
        tokio::time::timeout(WAIT, async {
            let mut events = Vec::new();
            while let Some(AdapterEvent::Session { event, .. }) = self.next_session().await {
                let finished = done(&event);
                events.push(event);
                if finished {
                    return events;
                }
            }
            panic!("event stream ended")
        })
        .await
        .expect("the scripted harness responds promptly")
    }

    async fn next_session(&mut self) -> Option<AdapterEvent> {
        loop {
            match self.events.next().await {
                Some(event @ AdapterEvent::Session { .. }) => return Some(event),
                Some(_) => {}
                None => return None,
            }
        }
    }

    async fn begin(&mut self) -> Id {
        let id = new_id();
        self.turns += 1;
        self.adapter
            .start_turn(TurnStart {
                plan: false,
                thread_id: self.thread,
                turn_id: id,
                text: "run".into(),
                images: vec![],
                model: "mock-large".into(),
                effort: None,
                params: json!({}),
                approval_mode: ApprovalMode::Bypass,
                sandbox_mode: SandboxMode::Open,
            })
            .await
            .unwrap();
        self.until(
            |event| matches!(event, SessionEvent::TurnStarted(start) if start.turn_id == id),
        )
        .await;
        id
    }

    async fn running_command(&mut self, item: &str, process: &str) {
        let native = format!("turn-{}", self.turns);
        self.alive
            .lock()
            .unwrap()
            .push(json!({ "itemId": item, "processId": process,
            "command": "sleep 300", "cwd": "/workspace", "turnId": native }));
        let mut started = command(item, process, "inProgress");
        // Foreground commands initially have no process id in the real item/started frame.
        started["processId"] = Json::Null;
        self.fake.state.notify(
            "item/started",
            json!({ "threadId": "thread", "turnId": native, "item": started }),
        );
        self.until(
            |event| matches!(event, SessionEvent::ItemStarted(start) if start.native_ref == item),
        )
        .await;
    }
}

#[tokio::test]
async fn stop_ends_a_command_that_the_app_server_left_alive_and_reports_its_real_exit() {
    let mut rig = Scripted::new(true).await;
    let turn = rig.begin().await;
    rig.running_command("current", "22").await;
    rig.adapter.interrupt(rig.thread, turn).await.unwrap();
    let events = rig.until(|event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "current")).await;
    assert!(events.iter().any(|event| matches!(event, SessionEvent::TurnCompleted(end) if end.status == TurnOutcome::Interrupted)));
    let SessionEvent::ItemCompleted(done) = events.last().unwrap() else {
        panic!("a command ended")
    };
    assert_eq!(done.status, ItemStatus::Failed);
    assert_eq!(done.payload["tool"]["exit_code"], -1);
    assert!(rig.alive.lock().unwrap().is_empty());
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn stop_tries_every_command_even_if_one_terminal_cannot_be_ended() {
    let mut rig = Scripted::new(true).await;
    let turn = rig.begin().await;
    rig.running_command("unconfirmed", "11").await;
    rig.running_command("ended", "22").await;
    *rig.decline_terminal.lock().unwrap() = Some("11".to_owned());
    rig.adapter.interrupt(rig.thread, turn).await.unwrap();
    let mut events = rig.until(|event| matches!(event, SessionEvent::ItemUpdated(update)
        if update.native_ref == "unconfirmed" && update.payload["title"].as_str().is_some_and(|title| title.contains("may still be running")))).await;
    if !events.iter().any(
        |event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "ended"),
    ) {
        events.extend(rig.until(|event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "ended")).await);
    }
    assert_eq!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/terminate")
            .len(),
        2
    );
    assert_eq!(rig.alive.lock().unwrap().len(), 1);
    assert!(!events.iter().any(|event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "unconfirmed")));
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn stop_leaves_a_command_started_by_an_earlier_turn_running() {
    let mut rig = Scripted::new(true).await;
    rig.begin().await;
    rig.running_command("earlier", "11").await;
    rig.fake.state.notify(
        "turn/completed",
        json!({ "threadId": "thread", "turn": { "id": "turn-1", "status": "completed" } }),
    );
    rig.until(|event| matches!(event, SessionEvent::TurnCompleted(_)))
        .await;
    let turn = rig.begin().await;
    rig.running_command("current", "22").await;
    rig.adapter.interrupt(rig.thread, turn).await.unwrap();
    rig.until(
        |event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "current"),
    )
    .await;
    assert_eq!(
        rig.alive.lock().unwrap().as_slice(),
        [
            json!({ "itemId": "earlier", "processId": "11", "command": "sleep 300", "cwd": "/workspace", "turnId": "turn-1" })
        ]
    );
    assert_eq!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/terminate")
            .len(),
        1
    );
    assert_eq!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/list")
            .len(),
        2
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn stop_leaves_an_earlier_turns_command_alone_when_its_start_notice_arrives_late() {
    let mut rig = Scripted::new(true).await;
    rig.begin().await;
    rig.fake.state.notify(
        "turn/completed",
        json!({ "threadId": "thread", "turn": { "id": "turn-1", "status": "completed" } }),
    );
    rig.until(|event| matches!(event, SessionEvent::TurnCompleted(_)))
        .await;
    let turn = rig.begin().await;
    rig.running_command("current", "22").await;
    rig.alive
        .lock()
        .unwrap()
        .push(json!({ "itemId": "late-earlier", "processId": "11",
        "command": "sleep 300", "cwd": "/workspace", "turnId": "turn-1" }));
    rig.fake.state.notify("item/started", json!({ "threadId": "thread", "turnId": "turn-1", "item": command("late-earlier", "11", "inProgress") }));
    rig.until(|event| matches!(event, SessionEvent::ItemStarted(start) if start.native_ref == "late-earlier")).await;
    rig.adapter.interrupt(rig.thread, turn).await.unwrap();
    rig.until(
        |event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "current"),
    )
    .await;
    assert_eq!(rig.alive.lock().unwrap().len(), 1);
    assert_eq!(rig.alive.lock().unwrap()[0]["itemId"], "late-earlier");
    assert_eq!(
        rig.fake
            .state
            .calls("thread/backgroundTerminals/terminate")
            .len(),
        1
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn a_harness_without_terminal_cleanup_says_the_command_may_still_run() {
    let mut rig = Scripted::new(false).await;
    let turn = rig.begin().await;
    rig.running_command("current", "22").await;
    rig.adapter.interrupt(rig.thread, turn).await.unwrap();
    let events = rig.until(|event| matches!(event, SessionEvent::ItemUpdated(update) if update.payload["title"].as_str().is_some_and(|title| title.contains("may still be running")))).await;
    assert!(!events.iter().any(
        |event| matches!(event, SessionEvent::ItemCompleted(done) if done.native_ref == "current")
    ));
    assert_eq!(rig.alive.lock().unwrap().len(), 1);
    rig.adapter.stop().await.unwrap();
}

#[tokio::test]
async fn the_first_recorded_unauthorized_retry_ends_the_turn_as_a_provider_refusal() {
    for (info, message, details, retries) in [
        (
            json!({ "responseStreamDisconnected": { "httpStatusCode": null } }),
            "Reconnecting... 1/5",
            "workspace routing discovery unauthorized (401)",
            true,
        ),
        (
            json!("other"),
            "workspace routing discovery unauthorized (401)",
            "",
            false,
        ),
    ] {
        let mut rig = Scripted::new(true).await;
        let turn = rig.begin().await;
        rig.fake.state.notify("error", json!({ "threadId": "thread", "turnId": "turn-1", "willRetry": retries,
            "error": { "codexErrorInfo": info, "message": message, "additionalDetails": details } }));
        let events = rig
            .until(|event| matches!(event, SessionEvent::TurnCompleted(_)))
            .await;
        let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
            panic!("ended")
        };
        assert_eq!(end.turn_id, turn);
        assert_eq!(end.status, TurnOutcome::Failed);
        let failure = end.error.as_ref().unwrap();
        assert_eq!(failure.code, "provider_unauthorized");
        assert_eq!(events.iter().filter(|event| matches!(event, SessionEvent::ItemStarted(start)
            if matches!(&start.content, ItemContent::Error(error) if error.code == "provider_unauthorized"))).count(), 1);
        assert!(!failure.retryable);
        assert!(
            failure
                .said
                .as_ref()
                .unwrap()
                .contains("unauthorized (401)")
        );
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn an_unauthorized_warning_is_already_a_refusal_even_without_a_thread_target() {
    for target in [json!("thread"), Json::Null] {
        let mut rig = Scripted::new(true).await;
        rig.begin().await;
        rig.fake.state.notify("warning", json!({ "threadId": target,
            "message": "Falling back from WebSockets to HTTPS transport. workspace routing discovery unauthorized (401)" }));
        let events = rig
            .until(|event| matches!(event, SessionEvent::TurnCompleted(_)))
            .await;
        let SessionEvent::TurnCompleted(end) = events.last().unwrap() else {
            panic!("ended")
        };
        assert_eq!(end.error.as_ref().unwrap().code, "provider_unauthorized");
        assert!(!end.error.as_ref().unwrap().retryable);
        rig.adapter.stop().await.unwrap();
    }
}

#[tokio::test]
async fn a_disconnect_without_a_refusal_is_announced_and_the_harness_can_retry() {
    let mut rig = Scripted::new(true).await;
    let turn = rig.begin().await;
    rig.fake.state.notify(
        "error",
        json!({ "threadId": "thread", "turnId": "turn-1", "willRetry": true,
        "error": { "codexErrorInfo": { "responseStreamDisconnected": { "httpStatusCode": null } },
            "message": "Reconnecting... 2/5", "additionalDetails": "connection dropped" } }),
    );
    let notice = rig
        .until(|event| matches!(event, SessionEvent::ItemCompleted(_)))
        .await;
    assert!(
        matches!(&notice[0], SessionEvent::ItemStarted(start) if matches!(&start.content, ItemContent::Error(error) if error.message.contains("2/5") && error.message.contains("connection dropped")))
    );
    assert!(rig.fake.state.calls("turn/interrupt").is_empty());
    rig.fake.state.notify(
        "turn/completed",
        json!({ "threadId": "thread", "turn": { "id": "turn-1", "status": "completed" } }),
    );
    let ended = rig
        .until(|event| matches!(event, SessionEvent::TurnCompleted(_)))
        .await;
    assert!(
        matches!(&ended[0], SessionEvent::TurnCompleted(end) if end.turn_id == turn && end.status == TurnOutcome::Completed)
    );
    rig.adapter.stop().await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_of_the_real_program_leaves_no_sleep_process_within_ten_seconds() {
    let mut rig = common::Rig::start("real-stop")
        .await
        .expect("the real harness image is required");
    rig.boot().await;
    let thread = new_id();
    rig.open_new(thread).await;
    let turn = rig.turn(
        thread,
        &common::script(
            "Run a command",
            &[
                "tool exec_command {\"cmd\":\"sleep 300\",\"yield_time_ms\":30000}",
                "then",
                "say done",
            ],
        ),
    );
    let id = turn.turn_id;
    rig.adapter.start_turn(turn).await.unwrap();
    let began = rig.until(|event| matches!(event, AdapterEvent::Session { event: SessionEvent::ItemStarted(start), .. }
        if matches!(&start.content, ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::Command { command, .. } if command == "sleep 300")))).await;
    let item = session_events(&began).into_iter().find_map(|event| match event {
        SessionEvent::ItemStarted(start) if matches!(&start.content, ItemContent::ToolCall(call) if matches!(&call.tool, ToolPayload::Command { .. })) => Some(start.native_ref), _ => None,
    }).unwrap();
    let container = rig
        .launcher
        .containers
        .lock()
        .unwrap()
        .last()
        .unwrap()
        .clone();
    let process_probe = r#"const fs=require('fs'); let n=0; for(const id of fs.readdirSync('/proc')) { if(!/^\d+$/.test(id))continue; try {const a=fs.readFileSync('/proc/'+id+'/cmdline').toString().split('\0').filter(Boolean); if(a.length===2 && (a[0]==='sleep'||a[0]==='/usr/bin/sleep') && a[1]==='300')n++;}catch{}} console.log(n);"#;
    let count = || async {
        let output = tokio::process::Command::new("docker")
            .args(["exec", &container, "node", "-e", process_probe])
            .output()
            .await
            .unwrap();
        assert!(output.status.success());
        String::from_utf8(output.stdout)
            .unwrap()
            .trim()
            .parse::<u32>()
            .unwrap()
    };
    tokio::time::timeout(WAIT, async {
        while count().await == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the sleep command really started");
    rig.adapter.interrupt(thread, id).await.unwrap();
    let stopped = tokio::time::timeout(
        WAIT,
        rig.until(|event| {
            matches!(event, AdapterEvent::Session {
        event: SessionEvent::ItemCompleted(done), .. } if done.native_ref == item)
        }),
    )
    .await
    .expect("Stop reports the command's end within ten seconds");
    let end = session_events(&stopped)
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::ItemCompleted(done) if done.native_ref == item => Some(done),
            _ => None,
        })
        .unwrap();
    assert_eq!(end.status, ItemStatus::Failed);
    assert_eq!(end.payload["tool"]["exit_code"], -1);
    // The harness tells the command's end when it has ended it; the kernel has the process gone a moment after.
    tokio::time::timeout(WAIT, async {
        while count().await != 0 {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        }
    })
    .await
    .expect("Stop killed the process, as well as ending the turn");
    rig.adapter.stop().await.unwrap();
}
