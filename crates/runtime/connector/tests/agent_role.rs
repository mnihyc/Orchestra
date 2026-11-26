//! The agent role at work on the in-memory hub, against the scripted controller that stands for the system: what a harness does reaches the system once and in order, whatever happens to the link and to the connector, and what the machine's owner forbids is refused.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, Rig};
use orch_agent_contract::session::ApprovalDecision;
use orch_link::commands::Progress;
use orch_link_contract::commands::{
    ApprovalResolve, MachineProbe, MachineUpdateNow, OrchToolResult, ProviderStop, SerialKey,
    SessionClose, TurnInterrupt,
};
use orch_link_contract::session::Role;
use orch_types::{Id, new_id};
use serde_json::json;
use std::time::Duration;

async fn rig_with_session() -> (Rig, Id, Id) {
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    (rig, thread, segment)
}

#[tokio::test]
async fn cancelling_a_native_opening_retires_its_local_tool_key_and_its_late_loss_cannot_retire_the_next_one()
 {
    use orch_agent_contract::adapter::{
        AdapterError, AgentAdapter, AgentStart, AgentStarted, EventSink,
    };
    use orch_agent_contract::descriptor::AgentDescriptor;
    use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    struct Gate {
        inner: orch_adapter_fake::FakeAdapter,
        calls: Arc<AtomicUsize>,
        entered: Arc<tokio::sync::Notify>,
        specs: Arc<Mutex<Vec<(Id, String)>>>,
        sink: Arc<Mutex<Option<EventSink>>>,
    }
    #[async_trait::async_trait]
    impl AgentAdapter for Gate {
        fn kind(&self) -> &'static str {
            "fake"
        }
        async fn start(
            &mut self,
            start: AgentStart,
            sink: EventSink,
        ) -> Result<AgentStarted, AdapterError> {
            *self.sink.lock().unwrap() = Some(sink.clone());
            self.inner.start(start, sink).await
        }
        async fn stop(&mut self) -> Result<(), AdapterError> {
            self.inner.stop().await
        }
        async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
            self.inner.probe().await
        }
        async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
            self.specs
                .lock()
                .unwrap()
                .push((spec.opening_id, spec.session_key.clone()));
            if self.calls.fetch_add(1, Ordering::SeqCst) == 0 {
                self.entered.notify_one();
                std::future::pending::<()>().await;
            }
            self.inner.open_session(spec).await
        }
        async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
            self.inner.close_session(thread).await
        }
        async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
            self.inner.start_turn(turn).await
        }
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let entered = Arc::new(tokio::sync::Notify::new());
    let specs = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::new(Mutex::new(None));
    let mut registration = common::fake_registration();
    let (factory_calls, factory_entered, factory_specs, factory_sink) =
        (calls.clone(), entered.clone(), specs.clone(), sink.clone());
    registration.factory = Arc::new(move |clock| {
        Box::new(Gate {
            inner: orch_adapter_fake::FakeAdapter::new(clock),
            calls: factory_calls.clone(),
            entered: factory_entered.clone(),
            specs: factory_specs.clone(),
            sink: factory_sink.clone(),
        })
    });
    let mut rig = Rig::new();
    rig.adapters = orch_connector_core::harnesses::AdapterRegistry::new();
    rig.adapters.register(registration).unwrap();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    let body: orch_link_contract::commands::SessionOpen =
        serde_json::from_value(rig.open_body(thread, segment, "/tmp/work")).unwrap();
    let opening = rig
        .controller
        .issue(&SerialKey::Thread(thread), rig.deadline(), &body);
    tokio::time::timeout(PATIENCE, entered.notified())
        .await
        .unwrap();
    let loopback = rig.connector.as_ref().unwrap().loopback().clone();
    let (original, key) = specs.lock().unwrap()[0].clone();
    assert_eq!(
        original, opening,
        "the adapter's public opening comes from the real accepted command"
    );
    assert!(loopback.session(&key).is_some());
    rig.controller.link().commands().cancel(opening);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(opening))
            .await
            .unwrap(),
        Progress::Cancelled
    ));
    until("the cancelled opening's local tool key is retired", || {
        loopback.session(&key).is_none()
    })
    .await;
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    let (_, current_key) = specs.lock().unwrap()[1].clone();
    let source = sink.lock().unwrap().clone().unwrap();
    source
        .session(
            thread,
            segment,
            orch_agent_contract::events::SessionEvent::SessionLost(
                orch_agent_contract::events::SessionLost {
                    uncertain: false,
                    expected: false,
                    opening_id: original,
                    reason: "the cancelled opening's delayed report".into(),
                },
            ),
        )
        .await
        .unwrap();
    rig.wait_for(thread, segment, "session.lost", 1).await;
    assert!(
        loopback.session(&current_key).is_some(),
        "a stale physical report cannot unbind its replacement even on the same segment"
    );
    assert_eq!(
        rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap()
            .sessions_open(),
        1
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn retained_command_output_arrives_before_its_item_and_survives_a_cut_upload() {
    let (rig, thread, segment) = rig_with_session().await;
    assert_eq!(rig.roles, vec![Role::Agent]);
    let output = format!("{}tail", "b".repeat(40_000));
    let hash = orch_types::Hash::of(output.as_bytes());
    rig.blobs.cut_put(0);
    rig.start_turn(thread, &format!("#!fake\nrun cat => 0 {output}\nsay Done"))
        .await;
    rig.wait_for(thread, segment, "item.completed", 1).await;
    let applied = rig.applied(thread, segment);
    let completed = applied
        .iter()
        .find(|event| {
            event.kind == "item.completed" && event.body["payload"]["tool"]["kind"] == "command"
        })
        .expect("the command completed");
    assert_eq!(
        completed.body["payload"]["tool"]["output"]["blob"],
        hash.to_string()
    );
    assert_eq!(
        rig.blobs.content(&hash).as_deref(),
        Some(output.as_bytes()),
        "a published output reference already resolves to all of its original bytes"
    );
    let cached = rig
        .data
        .path()
        .join("harness")
        .join(rig.provider.to_string())
        .join(rig.agent.to_string())
        .join("outputs")
        .join(hash.to_string());
    assert_eq!(
        std::fs::read(cached).unwrap(),
        output.as_bytes(),
        "retained outputs are not removed like temporary exports"
    );
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert_eq!(
        rig.applied(thread, segment)
            .iter()
            .filter(|event| event.kind == "item.completed"
                && event.body["payload"]["tool"]["kind"] == "command")
            .count(),
        1
    );
    rig.connector.unwrap().stop().await;
}

fn request_ref(rig: &Rig, thread: Id, segment: Id) -> String {
    rig.applied(thread, segment)
        .into_iter()
        .find(|e| e.kind == "approval.requested")
        .map(|e| e.body["request_ref"].as_str().unwrap().to_owned())
        .expect("no request for a decision was made")
}

#[tokio::test]
async fn an_approval_waits_until_it_is_resolved_and_then_the_turn_goes_on() {
    let (rig, thread, segment) = rig_with_session().await;
    rig.start_turn(
        thread,
        "#!fake\nsay before\nask command rm -rf target\nsay after",
    )
    .await;
    rig.wait_for(thread, segment, "approval.requested", 1).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(
        !rig.kinds(thread, segment)
            .contains(&"turn.completed".to_owned()),
        "the turn waits for a person"
    );
    rig.succeed(
        &SerialKey::Thread(thread),
        &ApprovalResolve {
            thread_id: thread,
            request_ref: request_ref(&rig, thread, segment),
            decision: ApprovalDecision::Option {
                option_id: "once".to_owned(),
                note: None,
            },
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let applied = rig.applied(thread, segment);
    assert_eq!(applied.last().unwrap().body["status"], "completed");
    assert_eq!(rig.messages(thread, segment), ["before", "after"]);
    let ran = applied.iter().any(|e| {
        e.kind == "item.completed"
            && e.body["payload"]["tool"]["command"] == "rm -rf target"
            && e.body["status"] == "completed"
    });
    assert!(ran, "the command ran once it was allowed");
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_decision_that_denies_leaves_the_command_unrun() {
    let (rig, thread, segment) = rig_with_session().await;
    rig.start_turn(thread, "#!fake\nask command rm -rf target\nsay done")
        .await;
    rig.wait_for(thread, segment, "approval.requested", 1).await;
    rig.succeed(
        &SerialKey::Thread(thread),
        &ApprovalResolve {
            thread_id: thread,
            request_ref: request_ref(&rig, thread, segment),
            decision: ApprovalDecision::Option {
                option_id: "deny".to_owned(),
                note: None,
            },
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let cancelled = rig.applied(thread, segment).iter().any(|e| {
        e.kind == "item.completed"
            && e.body["payload"]["tool"]["command"] == "rm -rf target"
            && e.body["status"] == "cancelled"
    });
    assert!(cancelled);
    assert_eq!(rig.messages(thread, segment), ["done"]);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn an_idle_update_fence_holds_a_new_turn_out_and_a_running_approval_keeps_the_fence_out() {
    let (rig, thread, segment) = rig_with_session().await;
    let connector = rig.connector.as_ref().unwrap();
    let guard = connector
        .idle_for_update()
        .expect("an idle connector can fence its next turn");
    let turn: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
        "thread_id":thread,"turn_id":new_id(),"text":"#!fake\nask command echo accepted\nsay after",
        "images":[],"model":"fake-large","effort":"high","params":{},
        "approval_mode":"on_request","sandbox_mode":"workspace"
    }))
    .unwrap();
    let id = rig
        .controller
        .issue(&SerialKey::Thread(thread), rig.deadline(), &turn);
    // Command ACKs update the issuer's progress, not the event store's changed notification.
    tokio::time::timeout(PATIENCE, async {
        loop {
            let progress = rig.controller.progress(id);
            if matches!(
                progress,
                Some(Progress::Accepted | Progress::Running | Progress::Released)
            ) {
                break;
            }
            assert!(
                !progress.is_some_and(|progress| progress.is_final()),
                "the fenced command ended before admission"
            );
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the command was not durably accepted while its turn was fenced");
    assert_eq!(connector.turns_running(), 0);
    assert!(
        !rig.kinds(thread, segment)
            .contains(&"turn.started".to_owned())
    );
    drop(guard);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, rig.controller.wait_command(id))
            .await
            .expect("the accepted turn start did not finish after its fence opened"),
        Progress::Succeeded(_)
    ));
    rig.wait_for(thread, segment, "approval.requested", 1).await;
    assert!(
        connector.idle_for_update().is_none(),
        "waiting for a person is still a running turn"
    );
    rig.succeed(
        &SerialKey::Thread(thread),
        &ApprovalResolve {
            thread_id: thread,
            request_ref: request_ref(&rig, thread, segment),
            decision: ApprovalDecision::Option {
                option_id: "once".to_owned(),
                note: None,
            },
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert!(connector.idle_for_update().is_some());
    assert_eq!(rig.messages(thread, segment), ["after"]);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn switching_auto_off_while_a_build_is_staged_discards_it_without_installing_or_ending_the_link()
 {
    use orch_connector_core::{
        automatic,
        enrollment::{DataDir, EnrollmentRecord},
    };
    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };
    struct Stage {
        entered: tokio::sync::Notify,
        release: tokio::sync::Notify,
        prepared: Mutex<Option<automatic::Prepared>>,
        installed: AtomicUsize,
    }
    #[async_trait::async_trait]
    impl automatic::Updates for Stage {
        fn running_version(&self) -> &str {
            "0.0.0"
        }
        async fn prepare(
            &self,
        ) -> Result<Option<automatic::Prepared>, orch_connector_core::update::UpdateError> {
            self.entered.notify_one();
            self.release.notified().await;
            Ok(self.prepared.lock().unwrap().take())
        }
        fn install(
            &self,
            _: &automatic::Prepared,
        ) -> Result<(), orch_connector_core::update::UpdateError> {
            self.installed.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }
    // The machine's owner keeps automatic updates on.
    let mut rig = Rig::new();
    rig.config.update.auto = true;
    rig.start().await;
    rig.agent_start(json!({})).await;
    rig.open_session(rig.open_body(new_id(), new_id(), "/tmp/work"))
        .await;
    let data = DataDir::open(rig.data.path()).unwrap();
    std::fs::create_dir_all(data.identity_dir()).unwrap();
    std::fs::write(
        data.identity_dir().join("enrollment.json"),
        serde_json::to_vec(&EnrollmentRecord {
            gateway_measured_at: None,
            gateways: Vec::new(),
            gateway_measurements: Vec::new(),
            server_endpoints: Vec::new(),
            machine_id: rig.machine_id,
            name: "local".into(),
            system: orch_link_contract::enrollment::EnrollReplySystem {
                name: "test".into(),
                public_url: "https://orch.test".into(),
            },
            pin: "test".into(),
            protocol_version: orch_link_contract::version::PROTOCOL_VERSION,
            enrolled_at: rig.clock.time(),
            managed: false,
        })
        .unwrap(),
    )
    .unwrap();
    let staging = tempfile::tempdir().unwrap();
    let path = staging.path().join("verified-download");
    std::fs::write(&path, b"verified transport fixture").unwrap();
    let stage = Arc::new(Stage {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
        prepared: Mutex::new(Some(automatic::Prepared::new(
            orch_connector_core::update::Offer {
                version: "2.0.0".into(),
                download_path: "/fixture".into(),
                size: 26,
                sha256: "0".repeat(64),
            },
            path.clone(),
            staging,
        ))),
        installed: AtomicUsize::new(0),
    });
    let connector = rig.connector.as_ref().unwrap();
    assert!(connector.link().versions().is_some());
    // Automatic updates are on in the owner's own file, and the server grants this one (Update now): on a machine a
    // person connected only that drain grant permits an automatic installation.
    data.update_config(|config| config.update.auto = true)
        .unwrap();
    let updating = automatic::until_installed(connector, &data, Some(&*stage));
    tokio::pin!(updating);
    let grant = MachineUpdateNow {
        recommended: "2.0.0".into(),
        automatic: true,
    };
    let granted = rig.send(&SerialKey::Machine, &grant);
    tokio::pin!(granted);
    tokio::select! {
        biased;
        _=&mut updating=>panic!("installation cannot precede the blocked download"),
        progress=&mut granted=>assert!(matches!(progress, Progress::Succeeded(_)), "{progress:?}"),
    }
    tokio::select! {
        _=&mut updating=>panic!("installation cannot precede the blocked download"),
        result=tokio::time::timeout(PATIENCE, stage.entered.notified())=>result.expect("the granted update never began its download"),
    }
    data.update_config(|config| config.update.auto = false)
        .unwrap();
    stage.release.notify_one();
    let discarded = async {
        while path.exists() {
            tokio::task::yield_now().await;
        }
    };
    tokio::select! {
        _=&mut updating=>panic!("a disabled automatic update must not install"),
        result=tokio::time::timeout(PATIENCE,discarded)=>result.expect("the owned staged directory was not discarded"),
    }
    assert_eq!(stage.installed.load(Ordering::SeqCst), 0);
    assert!(matches!(
        &*connector.link().status().borrow(),
        orch_link::machine::LinkStatus::Connected { .. }
    ));
    // This scope drops the pinned future's storage, then the connector's normal Drop stops it.
}

#[tokio::test]
async fn an_interrupt_ends_the_turn_as_interrupted_with_what_was_said_so_far() {
    use orch_adapter_fake::events::{PIECE_CHARS, PIECE_MILLIS};
    let (rig, thread, segment) = rig_with_session().await;
    // The start acknowledgement, observed delta and interrupt may each take PATIENCE.
    // Natural completion must outlive those windows; the interrupt's own deadline stays unchanged.
    let pieces = usize::try_from(PATIENCE.as_millis() * 3 / u128::from(PIECE_MILLIS)).unwrap() + 1;
    let word = "word ";
    let text = word.repeat((pieces * PIECE_CHARS).div_ceil(word.len()));
    let turn = rig
        .start_turn(thread, &format!("#!fake\nsay {text}\nsay never said"))
        .await;
    rig.wait_for(thread, segment, "item.delta", 2).await;
    rig.succeed(
        &SerialKey::Thread(thread),
        &TurnInterrupt {
            thread_id: thread,
            turn_id: turn,
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let applied = rig.applied(thread, segment);
    // The turn's end; what follows it is the cleanup of the commands it left (`turn.command_cleanup`).
    let ended = applied
        .iter()
        .rfind(|e| e.kind == "turn.completed")
        .unwrap();
    assert_eq!(ended.body["status"], "interrupted");
    let message = applied
        .iter()
        .rfind(|e| e.kind == "item.completed")
        .unwrap();
    assert_eq!(message.body["status"], "cancelled");
    let said = message.body["payload"]["text"].as_str().unwrap();
    assert!(said.len() < text.len(), "it was cut: {said:?}");
    assert!(
        !rig.messages(thread, segment)
            .iter()
            .any(|m| m == "never said")
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_link_cut_in_the_middle_of_a_turn_loses_no_event_and_repeats_none() {
    let (rig, thread, segment) = rig_with_session().await;
    let text = "the quick brown fox jumps over the lazy dog. ".repeat(10);
    rig.start_turn(thread, &format!("#!fake\nsay {text}")).await;
    rig.wait_for(thread, segment, "item.delta", 3).await;
    rig.hub.faults().sever_now();
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let applied = rig.applied(thread, segment);
    for (place, event) in applied.iter().enumerate() {
        assert_eq!(
            event.seq,
            place as i64 + 1,
            "in order, without a hole, none twice"
        );
    }
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
    let pieces: String = applied
        .iter()
        .filter(|e| e.kind == "item.delta")
        .map(|e| e.body["data"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(pieces, text.trim());
    assert!(
        rig.hub.faults().connections() >= 2,
        "the machine had to come back"
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_connector_killed_in_the_middle_of_a_turn_says_on_its_return_what_became_of_it() {
    let (mut rig, thread, segment) = rig_with_session().await;
    let admitted = rig
        .start_turn(thread, "#!fake\nsay going\nsleep 600000\nsay never")
        .await;
    rig.wait_for(thread, segment, "item.completed", 1).await;
    rig.kill().await;
    // The process that was killed has nothing more to say; the one that takes its place does.
    tokio::time::sleep(Duration::from_millis(300)).await;
    rig.start().await;
    rig.wait_for(thread, segment, "turn.lost", 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let kinds = rig.kinds(thread, segment);
    assert_eq!(
        kinds.iter().filter(|k| *k == "turn.lost").count(),
        1,
        "{kinds:?}"
    );
    assert_eq!(
        kinds.last().unwrap(),
        "turn.lost",
        "nothing of the dead turn comes after: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"turn.completed".to_owned()),
        "the turn did not end by itself: {kinds:?}"
    );
    let lost = rig
        .applied(thread, segment)
        .into_iter()
        .find(|e| e.kind == "turn.lost")
        .unwrap();
    let opened = rig
        .applied(thread, segment)
        .into_iter()
        .find(|event| event.kind == "session.opened")
        .unwrap();
    assert_eq!(
        lost.body["opening_id"], opened.body["opening_id"],
        "the restarted connector reports the original physical session, never a current-map replacement"
    );
    assert!(
        lost.body["opening_id"]
            .as_str()
            .and_then(|text| text.parse::<Id>().ok())
            .is_some()
    );
    assert_eq!(
        lost.body["turn_id"],
        serde_json::json!(admitted),
        "the synthetic adapter cannot reattach the admitted native turn"
    );
    assert!(
        lost.body["reason"]
            .as_str()
            .unwrap()
            .contains("unavailable")
    );
    for (place, event) in rig.applied(thread, segment).iter().enumerate() {
        assert_eq!(event.seq, place as i64 + 1);
    }
    assert!(rig.controller.violations().is_empty());
    // The process came back, with no session open, and the system can open the session again.
    let status = rig
        .controller
        .applied(&format!("harness:{}:{}", rig.provider, rig.agent));
    let last = status.iter().rfind(|e| e.kind == "harness.status").unwrap();
    assert_eq!(last.body["status"], "online");
    assert_eq!(last.body["sessions_open"], 0);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn an_idle_conversation_retires_its_original_tool_opening_quietly_after_connector_restart() {
    let (mut rig, thread, segment) = rig_with_session().await;
    let original = rig
        .applied(thread, segment)
        .into_iter()
        .find(|event| event.kind == "session.opened")
        .unwrap();
    rig.kill().await;
    rig.start().await;
    rig.wait_for(thread, segment, "session.lost", 1).await;
    let lost = rig
        .applied(thread, segment)
        .into_iter()
        .find(|event| event.kind == "session.lost")
        .unwrap();
    assert_eq!(lost.body["opening_id"], original.body["opening_id"]);
    assert_eq!(lost.body["expected"], json!(true));
    assert!(
        rig.connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap()
            .session_key(thread)
            .is_none()
    );
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    let openings = rig
        .applied(thread, segment)
        .into_iter()
        .filter(|event| event.kind == "session.opened")
        .collect::<Vec<_>>();
    assert_eq!(openings.len(), 2);
    assert_ne!(openings[1].body["opening_id"], original.body["opening_id"]);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_command_sent_again_by_its_identifier_runs_once() {
    let (rig, thread, segment) = rig_with_session().await;
    let turn = new_id();
    let body: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
        "thread_id": thread, "turn_id": turn, "text": "hello", "images": [], "model": "m",
        "effort": null, "params": {}, "approval_mode": "ask", "sandbox_mode": "read_only"
    }))
    .unwrap();
    let command = new_id();
    for _ in 0..2 {
        rig.controller
            .issue_with_id(command, &SerialKey::Thread(thread), rig.deadline(), &body);
        let ended = tokio::time::timeout(PATIENCE, rig.controller.wait_command(command))
            .await
            .unwrap();
        assert!(matches!(ended, Progress::Succeeded(_)), "{ended:?}");
    }
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    tokio::time::sleep(Duration::from_millis(400)).await;
    let kinds = rig.kinds(thread, segment);
    assert_eq!(
        kinds.iter().filter(|k| *k == "turn.started").count(),
        1,
        "{kinds:?}"
    );
    assert_eq!(kinds.iter().filter(|k| *k == "turn.completed").count(), 1);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_session_that_asks_for_what_the_machines_owner_forbids_is_refused_with_the_policys_error()
{
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());

    let outside: orch_link_contract::commands::SessionOpen =
        serde_json::from_value(rig.open_body(thread, segment, "/etc/orch")).unwrap();
    assert_eq!(
        rig.fail(&SerialKey::Thread(thread), &outside).await,
        "policy.root_not_allowed"
    );
    assert!(
        rig.kinds(thread, segment).is_empty(),
        "nothing of a refused session reached the harness"
    );

    let credential_shared = {
        let mut body = rig.open_body(thread, segment, "/tmp/work");
        body["arrangement"] = json!("credential_shared");
        serde_json::from_value::<orch_link_contract::commands::SessionOpen>(body).unwrap()
    };
    assert_eq!(
        rig.fail(&SerialKey::Thread(thread), &credential_shared)
            .await,
        "policy.arrangement_not_allowed"
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_managed_runtime_never_attributes_its_template_sandbox_to_a_machine_policy() {
    let mut rig = Rig::new();
    rig.managed = true;
    rig.start().await;
    let (thread, segment) = (new_id(), new_id());
    let connector = rig.connector.as_ref().unwrap();
    assert!(connector.services().policy.policy().is_none());
    connector
        .services()
        .agents
        .get()
        .unwrap()
        .report_sandbox(
            (thread, segment),
            Some(new_id()),
            new_id(),
            orch_link_contract::commands::SandboxMode::Open,
            None,
            &[],
        )
        .await
        .unwrap();
    connector
        .services()
        .publisher
        .publish(
            orch_link_contract::streams::Stream::Session {
                thread_id: thread,
                segment_id: segment,
            },
            orch_link_contract::events::Event::SessionTitle(
                orch_link_contract::events::SessionTitle {
                    title: "The callback finished".into(),
                },
            ),
        )
        .await
        .unwrap();
    rig.wait_for(thread, segment, "session.title", 1).await;
    assert!(
        !rig.applied(thread, segment)
            .iter()
            .any(|event| event.kind == "session.sandbox")
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn the_applied_sandbox_stays_with_its_native_command_after_the_machine_policy_changes() {
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    let mut wide = rig.open_body(thread, segment, "/tmp/work");
    wide["sandbox_mode"] = json!("open");
    rig.open_session(wide).await;
    let turn: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
        "thread_id": thread, "turn_id": new_id(), "text": "#!fake\nmode", "images": [],
        "model": "m", "effort": null, "params": {}, "approval_mode": "on_request", "sandbox_mode": "open"
    }))
    .unwrap();
    rig.succeed(&SerialKey::Thread(thread), &turn).await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    assert_eq!(
        rig.messages(thread, segment),
        ["approval: on_request, sandbox: workspace"],
        "the harness never saw the wider mode"
    );
    rig.wait_for(thread, segment, "raw.frame", 2).await;
    let told: Vec<_> = rig
        .applied(thread, segment)
        .into_iter()
        .filter(|e| e.kind == "raw.frame")
        .collect();
    assert_eq!(told[0].body["direction"], "connector");
    assert_eq!(told[0].body["payload"]["policy"], "agent.max_sandbox");
    assert_eq!(told[0].body["payload"]["asked"], "open");
    assert_eq!(told[0].body["payload"]["used"], "workspace");
    let mut unchanged = turn.clone();
    unchanged.turn_id = new_id();
    unchanged.sandbox_mode = orch_link_contract::commands::SandboxMode::ReadOnly;
    rig.succeed(&SerialKey::Thread(thread), &unchanged).await;
    rig.wait_for(thread, segment, "turn.completed", 2).await;
    rig.wait_for(thread, segment, "session.sandbox", 3).await;
    rig.connector
        .as_ref()
        .unwrap()
        .services()
        .policy
        .set_here(
            orch_link_contract::policy::PolicyKey::AgentMaxSandbox,
            &json!("read_only"),
        )
        .unwrap();
    let decisions: Vec<_> = rig
        .applied(thread, segment)
        .into_iter()
        .filter(|event| event.kind == "session.sandbox")
        .collect();
    assert_eq!(decisions.len(), 3);
    assert!(decisions[0].body["turn_id"].is_null());
    assert_eq!(decisions[1].body["turn_id"], json!(turn.turn_id));
    for decision in &decisions[..2] {
        assert_eq!(decision.body["requested"], "open");
        assert_eq!(decision.body["applied"], "workspace");
        assert_eq!(decision.body["governing_policy"], "machine_policy");
        let command: Id = serde_json::from_value(decision.body["command_id"].clone()).unwrap();
        assert_ne!(command, Id::nil());
    }
    assert_eq!(decisions[2].body["turn_id"], json!(unchanged.turn_id));
    assert_eq!(decisions[2].body["requested"], "read_only");
    assert_eq!(decisions[2].body["applied"], "read_only");
    assert_ne!(
        decisions[0].body["command_id"],
        decisions[1].body["command_id"]
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_command_for_a_role_the_connector_does_not_run_or_has_not_built_is_answered_as_unsupported()
 {
    let mut rig = Rig::new();
    rig.roles = vec![Role::Agent, Role::Provider];
    rig.start().await;
    // The environment role is not played here at all: the link refuses.
    let attach: orch_link_contract::commands::EnvDetach = serde_json::from_value(json!({
        "environment_id": new_id()
    }))
    .unwrap();
    assert_eq!(
        rig.fail(&SerialKey::Environment(new_id()), &attach).await,
        "unsupported"
    );
    // The provider role is played and no handler for it is built yet: the dispatcher refuses.
    let provider = new_id();
    let stop = ProviderStop {
        provider_id: provider,
        purge: None,
    };
    let progress = rig.send(&SerialKey::Provider(provider), &stop).await;
    let Progress::Failed(error) = progress else {
        panic!("{progress:?}")
    };
    assert_eq!(error.code, "unsupported");
    assert!(
        error.detail.contains("holds no role that takes"),
        "{}",
        error.detail
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn two_sessions_of_one_harness_process_do_not_see_each_other() {
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (a, a_segment) = (new_id(), new_id());
    let (b, b_segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(a, a_segment, "/tmp/a"))
        .await;
    rig.open_session(rig.open_body(b, b_segment, "/tmp/b"))
        .await;
    let hosting = rig.connector.as_ref().unwrap().hosting().unwrap().clone();
    let process = hosting.process(rig.provider, rig.agent).unwrap();
    assert_eq!(process.sessions_open(), 2);
    assert_ne!(
        process.session_key(a),
        process.session_key(b),
        "each has a key of its own"
    );

    // Each says its own and the other's words are not in its stream.
    rig.start_turn(a, "#!fake\nsay only for A\nshared").await;
    rig.start_turn(b, "#!fake\nsay only for B").await;
    rig.wait_for(b, b_segment, "turn.completed", 1).await;
    rig.wait_for(a, a_segment, "orch.tool_call", 1).await;
    assert_eq!(rig.messages(a, a_segment), ["only for A"]);
    assert_eq!(rig.messages(b, b_segment), ["only for B"]);

    // A result for the call of A, sent to B's thread, finds no such call; sent to A's own, it does.
    let call_ref = rig
        .applied(a, a_segment)
        .into_iter()
        .find(|e| e.kind == "orch.tool_call")
        .map(|e| e.body["call_ref"].as_str().unwrap().to_owned())
        .unwrap();
    let wrong = OrchToolResult {
        thread_id: b,
        call_ref: call_ref.clone(),
        attempt: None,
        result: Some(json!({ "entries": [] })),
        error: None,
    };
    assert_eq!(rig.fail(&SerialKey::Thread(b), &wrong).await, "not_found");
    let right = OrchToolResult {
        thread_id: a,
        ..wrong
    };
    rig.succeed(&SerialKey::Thread(a), &right).await;
    rig.wait_for(a, a_segment, "turn.completed", 1).await;
    assert!(
        !rig.kinds(b, b_segment)
            .contains(&"orch.tool_call".to_owned())
    );
    // Closing one leaves the other open.
    rig.succeed(&SerialKey::Thread(a), &SessionClose { thread_id: a })
        .await;
    assert_eq!(process.sessions_open(), 1);
    assert!(process.session_key(b).is_some());
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_harness_that_dies_is_reported_and_started_again_and_serves_a_new_session() {
    let (rig, thread, segment) = rig_with_session().await;
    rig.start_turn(thread, "#!fake\nsay before the end\ncrash")
        .await;
    rig.wait_for(thread, segment, "session.lost", 1).await;
    let harness = format!("harness:{}:{}", rig.provider, rig.agent);
    let statuses = |rig: &Rig| -> Vec<String> {
        rig.controller
            .applied(&harness)
            .into_iter()
            .filter(|e| e.kind == "harness.status")
            .map(|e| e.body["status"].as_str().unwrap().to_owned())
            .collect()
    };
    let seen = rig
        .controller
        .wait_until(PATIENCE, |_| {
            statuses(&rig).last().map(String::as_str) == Some("online")
                && statuses(&rig).contains(&"error".to_owned())
        })
        .await;
    assert!(
        seen,
        "online, error, starting, online expected; saw {:?}",
        statuses(&rig)
    );
    let seen = statuses(&rig);
    let after_error = &seen[seen.iter().position(|s| s == "error").unwrap()..];
    assert_eq!(after_error.first().unwrap(), "error");
    assert_eq!(after_error.last().unwrap(), "online");
    let lost = rig
        .applied(thread, segment)
        .into_iter()
        .find(|e| e.kind == "session.lost")
        .unwrap();
    assert!(lost.body["reason"].as_str().unwrap().contains("died"));
    // A session opened on the process that came back works.
    let (again, again_segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(again, again_segment, "/tmp/work"))
        .await;
    rig.start_turn(again, "second life").await;
    rig.wait_for(again, again_segment, "turn.completed", 1)
        .await;
    assert_eq!(
        rig.messages(again, again_segment),
        ["You said: second life"]
    );
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_harness_that_dies_too_often_is_left_failed() {
    let mut rig = Rig::new();
    rig.config.adapter.harness_restarts = 1;
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    rig.start_turn(thread, "#!fake\ncrash").await;
    let harness = format!("harness:{}:{}", rig.provider, rig.agent);
    let online_again = rig
        .controller
        .wait_until(PATIENCE, |c| {
            c.applied(&harness)
                .iter()
                .filter(|e| e.kind == "harness.status" && e.body["status"] == "online")
                .count()
                >= 2
        })
        .await;
    assert!(online_again, "the first death is answered by a restart");
    let (second, second_segment) = (new_id(), new_id());
    rig.open_session(rig.open_body(second, second_segment, "/tmp/work"))
        .await;
    rig.start_turn(second, "#!fake\ncrash").await;
    let given_up = rig
        .controller
        .wait_until(PATIENCE, |c| {
            c.applied(&harness).iter().any(|e| {
                e.kind == "harness.status"
                    && e.body["detail"]
                        .as_str()
                        .is_some_and(|d| d.contains("stays failed"))
            })
        })
        .await;
    assert!(
        given_up,
        "the second within the window is not answered by another"
    );
    // Commands for it say so.
    let body: orch_link_contract::commands::SessionOpen =
        serde_json::from_value(rig.open_body(new_id(), new_id(), "/tmp/work")).unwrap();
    assert_eq!(
        rig.fail(&SerialKey::Thread(new_id()), &body).await,
        "harness_unavailable"
    );
    // The system starts it again, and it is whole.
    rig.agent_start(json!({})).await;
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn the_machine_answers_a_probe_with_what_it_offers() {
    let mut rig = Rig::new();
    rig.start().await;
    let facts = rig.succeed(&SerialKey::Machine, &MachineProbe {}).await;
    assert_eq!(
        facts["adapters"],
        json!([{ "kind": "fake", "adapter_version": 1, "builtin": true, "needs_binary": false,
                  "default_home_owned": true }])
    );
    assert_eq!(
        facts["policy"]["environment.roots"],
        json!(rig.config.environment.roots)
    );
    assert_eq!(facts["policy"]["agent.max_sandbox"], "workspace");
    assert!(facts["cpus"].as_u64().unwrap() >= 1);
    // The same facts were reported on the machine's stream when the connector started.
    let arrived = rig
        .controller
        .wait_until(PATIENCE, |c| {
            c.applied("machine")
                .iter()
                .any(|e| e.kind == "machine.facts")
        })
        .await;
    assert!(arrived);
    let reported = rig.controller.applied("machine");
    assert_eq!(reported[0].body["adapters"], facts["adapters"]);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn the_connector_counts_the_turns_that_run_in_what_it_hosts() {
    let (rig, thread, segment) = rig_with_session().await;
    let connector = rig.connector.as_ref().unwrap();
    assert_eq!(connector.turns_running(), 0);
    let turn = rig.start_turn(thread, "#!fake\nsleep 3600000").await;
    rig.wait_for(thread, segment, "turn.started", 1).await;
    assert_eq!(
        connector.turns_running(),
        1,
        "a connector that stops now would lose this turn"
    );
    rig.succeed(
        &SerialKey::Thread(thread),
        &TurnInterrupt {
            thread_id: thread,
            turn_id: turn,
        },
    )
    .await;
    rig.wait_for(thread, segment, "turn.completed", 1).await;
    let idle = tokio::time::timeout(PATIENCE, async {
        while connector.turns_running() > 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(
        idle.is_ok(),
        "the count goes back to nought when the turn has ended"
    );
    rig.connector.unwrap().stop().await;
}

// ---------------------------------------------------------------------------------------------------------
// The system gives a command up: what moves the history of a session stops, and leaves nothing behind

/// A line on which no request is answered for a minute: a transfer is under way for as long as a test needs.
fn stalled() -> orch_testkit::doubles::link::LinkShape {
    orch_testkit::doubles::link::LinkShape {
        round_trip: Duration::from_secs(60),
        up_bytes_per_second: 1 << 20,
        down_bytes_per_second: 1 << 20,
    }
}

/// The files the scripted adapter made as exports, wherever under the connector's data they lie.
fn exported_files(rig: &Rig) -> Vec<std::path::PathBuf> {
    fn walk(directory: &std::path::Path, found: &mut Vec<std::path::PathBuf>) {
        let Ok(entries) = std::fs::read_dir(directory) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                walk(&path, found);
            } else if path
                .extension()
                .is_some_and(|extension| extension == "json")
                && path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .is_some_and(|name| name == "exports")
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    walk(rig.data.path(), &mut found);
    found
}

/// The directory called `name` under `root`, if there is one.
fn directory_called(root: &std::path::Path, name: &str) -> Option<std::path::PathBuf> {
    for entry in std::fs::read_dir(root).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.file_name().is_some_and(|found| found == name) {
                return Some(path);
            }
            if let Some(inside) = directory_called(&path, name) {
                return Some(inside);
            }
        }
    }
    None
}

/// Waits until `condition` holds, as long as a test waits for anything.
async fn until(what: &str, mut condition: impl FnMut() -> bool) {
    let limit = tokio::time::Instant::now() + PATIENCE;
    while !condition() {
        assert!(
            tokio::time::Instant::now() < limit,
            "it never came about: {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn an_export_the_system_gave_up_stops_uploading_and_leaves_no_file_behind() {
    use orch_link_contract::commands::{ExportKind, SessionExport};
    let (rig, thread, _) = rig_with_session().await;
    rig.link.set_shape(Some(stalled()));
    let key = SerialKey::Thread(thread);
    let export = SessionExport {
        thread_id: thread,
        kind: ExportKind::Native,
        upto_turn_ordinal: None,
    };
    let first = rig.controller.issue(&key, rig.deadline(), &export);
    rig.blobs.open_command(first);
    // The archive is made and waits for the line.
    until("the archive was made", || !exported_files(&rig).is_empty()).await;
    // The system gives the command up.
    rig.controller.link().commands().cancel(first);
    until("the archive was removed", || {
        exported_files(&rig).is_empty()
    })
    .await;
    assert!(
        rig.blobs.requests().iter().all(|(kind, _)| kind != "put"),
        "no piece of it went up: {:?}",
        rig.blobs.requests()
    );
    // The thread's queue is free again: an export on a line that works goes through, and what it put up
    // is the whole archive.
    rig.link.set_shape(None);
    let second = rig.controller.issue(&key, rig.deadline(), &export);
    rig.blobs.open_command(second);
    let ended = tokio::time::timeout(PATIENCE, rig.controller.wait_command(second))
        .await
        .expect("the next export waited behind the one that was given up");
    let Progress::Succeeded(result) = ended else {
        panic!("the next export did not succeed: {ended:?}");
    };
    let hash: orch_types::Hash = serde_json::from_value(result["blob"].clone()).unwrap();
    let held = rig.blobs.content(&hash).expect("the archive is not held");
    assert_eq!(i64::try_from(held.len()).unwrap(), result["size"]);
    until("the second archive was removed", || {
        exported_files(&rig).is_empty()
    })
    .await;
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn an_opening_the_system_gave_up_stops_fetching_and_removes_what_it_fetched() {
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let content = bytes::Bytes::from(vec![7u8; 3 * 1024 * 1024]);
    let archive = rig.blobs.insert(content.clone());
    rig.link.set_shape(Some(stalled()));
    let (thread, segment) = (new_id(), new_id());
    let mut body = rig.open_body(thread, segment, "/tmp/work");
    body["mode"] = json!("import_native");
    body["archive"] = json!({"format": "fake-v1", "blob": archive, "size": content.len()});
    let body: orch_link_contract::commands::SessionOpen = serde_json::from_value(body).unwrap();
    let key = SerialKey::Thread(thread);
    let opening = rig.controller.issue(&key, rig.deadline(), &body);
    rig.blobs.open_command(opening);
    // The directory of the command is made before the first piece is asked for.
    until("the command's directory was made", || {
        directory_called(rig.data.path(), &opening.to_string()).is_some()
    })
    .await;
    rig.controller.link().commands().cancel(opening);
    until("the command's directory was removed", || {
        directory_called(rig.data.path(), &opening.to_string()).is_none()
    })
    .await;
    // At most the piece that was in flight was asked for, and the thread's queue is free again.
    assert!(
        rig.blobs
            .requests()
            .iter()
            .filter(|(kind, _)| kind == "get")
            .count()
            <= 1,
        "{:?}",
        rig.blobs.requests()
    );
    rig.link.set_shape(None);
    let other = rig.open_body(thread, new_id(), "/tmp/work");
    let other: orch_link_contract::commands::SessionOpen = serde_json::from_value(other).unwrap();
    let second = rig.controller.issue(&key, rig.deadline(), &other);
    let ended = tokio::time::timeout(PATIENCE, rig.controller.wait_command(second))
        .await
        .expect("the next opening waited behind the one that was given up");
    assert!(matches!(ended, Progress::Succeeded(_)), "{ended:?}");
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_delayed_close_keeps_its_original_physical_target_after_the_same_segment_reopens() {
    use orch_link_contract::commands::{Cmd, NativeTarget};
    for fenced in [false, true] {
        let (rig, thread, segment) = rig_with_session().await;
        rig.wait_for(thread, segment, "session.opened", 1).await;
        let hosting = rig.connector.as_ref().unwrap().hosting().unwrap();
        let process = hosting.process(rig.provider, rig.agent).unwrap();
        let original = process.session_target(thread).unwrap();
        let mut delayed = if fenced {
            Cmd::new(
                new_id(),
                &SerialKey::Thread(thread),
                orch_link_contract::commands::Cmd::work_budget(rig.clock.time(), rig.deadline()),
                &orch_link_contract::continuations::SessionCloseFenced {
                    thread_id: thread,
                    segment_id: segment,
                },
            )
            .unwrap()
        } else {
            Cmd::new(
                new_id(),
                &SerialKey::Thread(thread),
                orch_link_contract::commands::Cmd::work_budget(rig.clock.time(), rig.deadline()),
                &SessionClose { thread_id: thread },
            )
            .unwrap()
        };
        delayed.native_target = Some(NativeTarget::Session(original.clone()));
        let native = rig
            .applied(thread, segment)
            .into_iter()
            .find(|event| event.kind == "session.opened")
            .unwrap()
            .body["native_session_ref"]
            .clone();
        let mut reopened = rig.open_body(thread, segment, "/tmp/work");
        reopened["mode"] = json!("resume");
        reopened["native_session_ref"] = native;
        rig.open_session(reopened).await;
        rig.wait_for(thread, segment, "session.opened", 2).await;
        let current = process.session_target(thread).unwrap();
        assert_ne!(current.opening_id, original.opening_id);
        let mut handle = rig.controller.link().commands().issue(delayed);
        assert!(
            matches!(
                tokio::time::timeout(PATIENCE, handle.wait()).await.unwrap(),
                Progress::Succeeded(_)
            ),
            "the original opening is already gone; its close is a successful no-op"
        );
        assert_eq!(process.session_target(thread).unwrap(), current);
        assert_eq!(process.sessions_open(), 1);
        rig.start_turn(thread, "#!fake\nsay still current").await;
        rig.wait_for(thread, segment, "turn.completed", 1).await;
        assert_eq!(rig.messages(thread, segment), ["still current"]);
        rig.connector.unwrap().stop().await;
    }
}

#[tokio::test]
async fn a_close_accepted_while_absent_does_not_find_and_close_a_session_opened_later() {
    use orch_link_contract::commands::{Cmd, NativeTarget, SessionTarget};
    let mut rig = Rig::new();
    rig.start().await;
    rig.agent_start(json!({})).await;
    let (thread, segment) = (new_id(), new_id());
    let mut delayed = Cmd::new(
        new_id(),
        &SerialKey::Thread(thread),
        orch_link_contract::commands::Cmd::work_budget(rig.clock.time(), rig.deadline()),
        &SessionClose { thread_id: thread },
    )
    .unwrap();
    delayed.native_target = Some(NativeTarget::Session(SessionTarget::absent(thread)));
    rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
        .await;
    let hosting = rig.connector.as_ref().unwrap().hosting().unwrap();
    let process = hosting.process(rig.provider, rig.agent).unwrap();
    let current = process.session_target(thread).unwrap();
    let mut handle = rig.controller.link().commands().issue(delayed);
    assert!(matches!(
        tokio::time::timeout(PATIENCE, handle.wait()).await.unwrap(),
        Progress::Succeeded(_)
    ));
    assert_eq!(process.session_target(thread).unwrap(), current);
    assert_eq!(process.sessions_open(), 1);
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_session_call_waiting_for_its_adapter_cannot_act_after_its_physical_slot_ends() {
    use orch_link_contract::commands::NativeTarget;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    let (rig, thread, _) = rig_with_session().await;
    let process = rig
        .connector
        .as_ref()
        .unwrap()
        .hosting()
        .unwrap()
        .process(rig.provider, rig.agent)
        .unwrap();
    let target = process.session_target(thread).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let fences = Arc::new(orch_link::native_fences::NativeFences::new(
        directory.path().into(),
    ));
    fences
        .observe_session(
            &target,
            orch_link_contract::native_target::SessionAction::Open,
        )
        .unwrap();
    let context = orch_link::commands::CommandContext {
        authority_deadline: None,
        native_members: None,
        native_revision: None,

        native_fences: Some(fences),
        native_target: Some(NativeTarget::Session(target.clone())),
        id: new_id(),
        key: None,
        deadline: rig.deadline(),
        cancel: tokio_util::sync::CancellationToken::new(),
    };
    let entered = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let (held, waiting, finish) = (process.clone(), entered.clone(), release.clone());
    let holder = tokio::spawn(async move {
        held.call(move |_| {
            Box::pin(async move {
                waiting.notify_one();
                finish.notified().await;
                Ok(())
            })
        })
        .await
        .unwrap();
    });
    tokio::time::timeout(PATIENCE, entered.notified())
        .await
        .unwrap();
    let acted = Arc::new(AtomicBool::new(false));
    let marker = acted.clone();
    let pending = process.call_target(&context, move |_| {
        Box::pin(async move {
            marker.store(true, Ordering::SeqCst);
            Ok(())
        })
    });
    tokio::pin!(pending);
    tokio::select! {
        biased;
        result = &mut pending => panic!("the call must queue behind the held adapter: {result:?}"),
        () = tokio::task::yield_now() => {},
    }
    process.session_closed_if(thread, target.opening_id.unwrap());
    release.notify_one();
    holder.await.unwrap();
    let error = tokio::time::timeout(PATIENCE, &mut pending)
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        error.code,
        orch_link_contract::commands::ErrorCode::SessionNotFound.as_str()
    );
    assert!(!acted.load(Ordering::SeqCst));
    rig.connector.unwrap().stop().await;
}

#[tokio::test]
async fn a_queued_mcp_update_cannot_replace_its_successors_members_but_an_independent_model_change_does_not_block_it()
 {
    use orch_link_contract::commands::NativeTarget;
    use orch_link_contract::native_target::SessionAction;
    use orch_link_contract::tool_sessions::{ToolSessionMember, ToolSessionUpdate};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    for changed_mcp in [true, false] {
        let (rig, thread, _) = rig_with_session().await;
        let process = rig
            .connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap();
        let target = process.session_target(thread).unwrap();
        let directory = tempfile::tempdir().unwrap();
        let fences = Arc::new(orch_link::native_fences::NativeFences::new(
            directory.path().into(),
        ));
        fences
            .observe_session(&target, SessionAction::Open)
            .unwrap();
        let mcp = ToolSessionMember {
            member: format!("mcp:{}", new_id()),
            nonce: new_id(),
        };
        let model = ToolSessionMember {
            member: "llm".into(),
            nonce: new_id(),
        };
        let original = vec![mcp.clone(), model];
        let mut update = ToolSessionUpdate {
            machine_id: rig.machine_id,
            thread_id: thread,
            segment_id: target.segment_id.unwrap(),
            opening_id: target.opening_id,
            opening_epoch: target.opening_epoch.unwrap(),
            revision: 1,
            live: true,
            members: original.clone(),
        };
        fences.observe_tools(&update).unwrap();
        let loopback = rig.connector.as_ref().unwrap().loopback();
        loopback.native_fences(fences.clone());
        loopback.apply_tool_update(&update);
        let context = orch_link::commands::CommandContext {
            authority_deadline: None,
            native_target: Some(NativeTarget::Session(target)),
            native_fences: Some(fences.clone()),
            native_members: Some(original),
            native_revision: Some(1),
            id: new_id(),
            key: None,
            deadline: rig.deadline(),
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(tokio::sync::Notify::new());
        let (held, waiting, finish) = (process.clone(), entered.clone(), release.clone());
        let holder = tokio::spawn(async move {
            held.call(move |_| {
                Box::pin(async move {
                    waiting.notify_one();
                    finish.notified().await;
                    Ok(())
                })
            })
            .await
            .unwrap();
        });
        tokio::time::timeout(PATIENCE, entered.notified())
            .await
            .unwrap();
        let acted = Arc::new(AtomicBool::new(false));
        let marker = acted.clone();
        let pending = process.call_config(&context, SessionAction::Mcp, move |_| {
            Box::pin(async move {
                marker.store(true, Ordering::SeqCst);
                Ok(())
            })
        });
        tokio::pin!(pending);
        tokio::select! {biased;result=&mut pending=>panic!("configuration must wait behind the adapter: {result:?}"),()=tokio::task::yield_now()=>{}}
        update.revision = 2;
        if changed_mcp {
            update.members[0].nonce = new_id();
        } else {
            update.members[1].nonce = new_id();
        }
        fences.observe_tools(&update).unwrap();
        loopback.apply_tool_update(&update);
        release.notify_one();
        holder.await.unwrap();
        let result = tokio::time::timeout(PATIENCE, &mut pending).await.unwrap();
        if changed_mcp {
            assert_eq!(
                result.unwrap_err().code,
                orch_link_contract::commands::ErrorCode::Conflict.as_str()
            );
            assert!(!acted.load(Ordering::SeqCst));
        } else {
            result.unwrap();
            assert!(acted.load(Ordering::SeqCst));
        }
        rig.connector.unwrap().stop().await;
    }
}

#[tokio::test]
async fn an_earlier_turns_late_completion_keeps_the_replacement_turn_busy_in_memory_and_on_disk() {
    use orch_agent_contract::adapter::{
        AdapterError, AgentAdapter, AgentStart, AgentStarted, EventSink,
    };
    use orch_agent_contract::descriptor::AgentDescriptor;
    use orch_agent_contract::session::{SessionOpened, SessionSpec, ThreadId, TurnStart};
    use std::sync::{Arc, Mutex};
    struct Capture {
        inner: orch_adapter_fake::FakeAdapter,
        sink: Arc<Mutex<Option<EventSink>>>,
    }
    #[async_trait::async_trait]
    impl AgentAdapter for Capture {
        fn kind(&self) -> &'static str {
            "fake"
        }
        async fn start(
            &mut self,
            start: AgentStart,
            sink: EventSink,
        ) -> Result<AgentStarted, AdapterError> {
            *self.sink.lock().unwrap() = Some(sink.clone());
            self.inner.start(start, sink).await
        }
        async fn stop(&mut self) -> Result<(), AdapterError> {
            self.inner.stop().await
        }
        async fn probe(&mut self) -> Result<AgentDescriptor, AdapterError> {
            self.inner.probe().await
        }
        async fn open_session(&mut self, spec: SessionSpec) -> Result<SessionOpened, AdapterError> {
            self.inner.open_session(spec).await
        }
        async fn close_session(&mut self, thread: ThreadId) -> Result<(), AdapterError> {
            self.inner.close_session(thread).await
        }
        async fn start_turn(&mut self, turn: TurnStart) -> Result<(), AdapterError> {
            self.inner.start_turn(turn).await
        }
    }
    for retry_same_turn in [false, true] {
        let sink = Arc::new(Mutex::new(None));
        let source = sink.clone();
        let mut registration = common::fake_registration();
        registration.factory = Arc::new(move |clock| {
            Box::new(Capture {
                inner: orch_adapter_fake::FakeAdapter::new(clock),
                sink: source.clone(),
            })
        });
        let mut rig = Rig::new();
        rig.adapters = orch_connector_core::harnesses::AdapterRegistry::new();
        rig.adapters.register(registration).unwrap();
        rig.start().await;
        rig.agent_start(json!({})).await;
        let (thread, segment) = (new_id(), new_id());
        rig.open_session(rig.open_body(thread, segment, "/tmp/work"))
            .await;
        let process = rig
            .connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .process(rig.provider, rig.agent)
            .unwrap();
        let original = process.session_target(thread).unwrap();
        let old = rig.start_turn(thread, "#!fake\nsay old").await;
        rig.wait_for(thread, segment, "turn.completed", 1).await;
        let mut next = rig.open_body(thread, segment, "/tmp/work");
        next["mode"] = json!("resume");
        next["native_session_ref"] = json!(process.native_session_ref(thread));
        rig.open_session(next).await;
        let current = process.session_target(thread).unwrap();
        assert_ne!(original.opening_id, current.opening_id);
        let running = if retry_same_turn {
            let body: orch_link_contract::commands::TurnStart = serde_json::from_value(json!({
                "thread_id":thread,"turn_id":old,"text":"#!fake\nsleep 3600000","images":[],
                "model":"fake-large","effort":"high","params":{},
                "approval_mode":"on_request","sandbox_mode":"workspace"
            }))
            .unwrap();
            rig.succeed(
                &orch_link_contract::commands::SerialKey::Thread(thread),
                &body,
            )
            .await;
            old
        } else {
            rig.start_turn(thread, "#!fake\nsleep 3600000").await
        };
        rig.wait_for(thread, segment, "turn.started", 2).await;
        assert_eq!(process.running_turn(thread), Some(running));
        let source = sink.lock().unwrap().clone().unwrap();
        source.emit(orch_agent_contract::adapter::AdapterEvent::from_wire(
        "session.background_tasks",
            json!({"thread_id":thread,"segment_id":segment,"native_opening_id":current.opening_id,
            "self_waking":true,"native_idle":false,"native_wake":null,
            "tasks":[{"id":"background","run_id":"current","native_item_ref":"tool-current",
                "origin_turn":running,"kind":"command","label":"Current follow-up","state":"completed",
                "exit_code":0,"can_end":false}]}),
    ).unwrap()).await.unwrap();
        rig.wait_for(thread, segment, "session.background_tasks", 1)
            .await;
        assert!(process.background_running(thread));
        source
            .emit(
                orch_agent_contract::adapter::AdapterEvent::from_wire(
                    "turn.started",
                json!({"thread_id":thread,"segment_id":segment,"native_opening_id":original.opening_id,
                "turn_id":old,"native_turn_ref":"late-original-turn"}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        rig.wait_for(thread, segment, "turn.started", 3).await;
        assert_eq!(process.running_turn(thread), Some(running));
        assert!(process.background_running(thread));
        source
            .emit(
                orch_agent_contract::adapter::AdapterEvent::from_wire(
                    "session.background_tasks",
            json!({"thread_id":thread,"segment_id":segment,"native_opening_id":original.opening_id,
            "self_waking":false,"native_idle":true,"tasks":[],"native_wake":null}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        rig.wait_for(thread, segment, "session.background_tasks", 2)
            .await;
        assert!(
            process.background_running(thread),
            "an earlier native idle report cannot release the current wake"
        );
        source
            .emit(
                orch_agent_contract::adapter::AdapterEvent::from_wire(
                    "turn.completed",
                json!({"thread_id":thread,"segment_id":segment,"native_opening_id":original.opening_id,
                "turn_id":old,"status":"completed","error":null}),
                )
                .unwrap(),
            )
            .await
            .unwrap();
        rig.wait_for(thread, segment, "turn.completed", 2).await;
        assert_eq!(
            rig.applied(thread, segment)
                .iter()
                .rev()
                .find(|event| event.kind == "turn.completed")
                .unwrap()
                .native_opening_id,
            original.opening_id
        );
        assert_eq!(process.running_turn(thread), Some(running));
        let hosted = rig
            .connector
            .as_ref()
            .unwrap()
            .hosting()
            .unwrap()
            .ledger()
            .hosted();
        assert!(hosted.iter().flat_map(|harness| &harness.sessions).any(
            |session| session.thread_id == thread
                && Some(session.opening_id) == current.opening_id
                && session.running_turn == Some(running)
        ));
        rig.connector.unwrap().stop().await;
    }
}

#[path = "agent_role/automatic_install.rs"]
mod automatic_install;
#[path = "agent_role/stop_during_command.rs"]
mod stop_during_command;
