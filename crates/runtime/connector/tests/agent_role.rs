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
            if matches!(progress, Some(Progress::Accepted)) {
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
            server_endpoints: Vec::new(),
            machine_id: rig.machine_id,
            name: "local".into(),
            system: orch_link_contract::enrollment::EnrollReplySystem {
                name: "test".into(),
                public_url: "https://orch.test".into(),
            },
            pin: "test".into(),
            protocol_version: 1,
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
    rig.start_turn(thread, "#!fake\nsay going\nsleep 600000\nsay never")
        .await;
    rig.wait_for(thread, segment, "item.completed", 1).await;
    rig.kill().await;
    // The process that was killed has nothing more to say; the one that takes its place does.
    tokio::time::sleep(Duration::from_millis(300)).await;
    rig.start().await;
    rig.wait_for(thread, segment, "session.lost", 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let kinds = rig.kinds(thread, segment);
    assert_eq!(
        kinds.iter().filter(|k| *k == "session.lost").count(),
        1,
        "{kinds:?}"
    );
    assert_eq!(
        kinds.last().unwrap(),
        "session.lost",
        "nothing of the dead turn comes after: {kinds:?}"
    );
    assert!(
        !kinds.contains(&"turn.completed".to_owned()),
        "the turn did not end by itself: {kinds:?}"
    );
    let lost = rig
        .applied(thread, segment)
        .into_iter()
        .find(|e| e.kind == "session.lost")
        .unwrap();
    assert!(
        lost.body["reason"]
            .as_str()
            .unwrap()
            .contains("turn was running")
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
async fn a_sandbox_wider_than_the_ceiling_reaches_the_harness_lowered_and_the_system_is_told() {
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
