//! Commands from the system to a machine: run once, one at a time by key, never late, and with secrets that only
//! their machine reads.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::commands::{CommandIssuer, Progress};
use orch_link::machine::{MachineConfig, MachineLink};
use orch_link_contract::backplane::DownFrame;
use orch_link_contract::commands::{
    Cmd, CommandKind, ErrorCode, Reference, SerialKey, SessionClose, resolve_references,
};
use orch_link_contract::session::{Arch, Os, Platform, Role};
use orch_link_contract::version::Limits;
use orch_pki::sealing::{RecipientKey, info_for_command, open, seal};
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Direction, Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedHandler, ScriptedMachine};
use orch_types::{Clock, ManualClock, Time, new_id};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;

struct Rig {
    clock: ManualClock,
    shared: Arc<dyn Clock>,
    pki: TestPki,
    hub: Hub,
    controller: ScriptedController,
    machine: ScriptedMachine,
}

fn rig() -> Rig {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let shared: Arc<dyn Clock> = Arc::new(clock.clone());
    let pki = TestPki::new(Arc::clone(&shared));
    let hub = Hub::with_limits(Arc::clone(&shared), pki.certificate(), Limits::default(), 1);
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&shared),
        id,
        vec![Role::Agent, Role::Environment],
    );
    let machine = ScriptedMachine::start(&hub, &shared, &identity, &MachineSpec::default());
    Rig {
        clock,
        shared,
        pki,
        hub,
        controller,
        machine,
    }
}

fn deadline(clock: &ManualClock, seconds: i64) -> Time {
    Time::new(clock.now() + chrono::Duration::seconds(seconds))
}

async fn within<T>(seconds: u64, work: impl std::future::Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(seconds), work)
        .await
        .expect("took too long")
}

#[tokio::test]
async fn a_command_that_is_sent_again_after_a_reconnect_runs_once_and_gets_one_result() {
    let rig = rig();
    rig.machine.link.connected().await;
    rig.machine.handler.hold(CommandKind::SessionClose);
    let thread = new_id();
    let id = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 300),
        &SessionClose { thread_id: thread },
    );
    within(10, rig.machine.handler.started(id)).await;
    // The link breaks while the command runs; the system sends it again on the next session.
    rig.hub.faults().sever_now();
    tokio::time::sleep(Duration::from_millis(1_800)).await;
    rig.machine.handler.release();
    let ended = within(30, rig.controller.wait_command(id)).await;
    assert!(matches!(ended, Progress::Succeeded(_)), "{ended:?}");
    assert_eq!(rig.machine.handler.runs_of(id), 1);
}

#[tokio::test]
async fn a_command_that_is_sent_again_after_the_system_restarts_returns_what_was_written_down_and_does_not_run_again()
 {
    let rig = rig();
    rig.machine.link.connected().await;
    // The result is lost on its way to the system, which then restarts and asks again.
    rig.hub.faults().drop_frames(Direction::Up, "cmd_res", 0, 1);
    let thread = new_id();
    let id = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 300),
        &SessionClose { thread_id: thread },
    );
    within(10, rig.machine.handler.started(id)).await;
    // This case restarts after the journaled result was lost, rather than while it
    // is still being written. A fixed sleep does not establish that ordering.
    within(10, async {
        while rig.hub.faults().fired() == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(
        rig.hub.faults().fired(),
        1,
        "the sole result-drop rule fired"
    );
    assert!(
        !matches!(rig.controller.progress(id), Some(Progress::Succeeded(_))),
        "the result was lost"
    );
    rig.controller.restart();
    let ended = within(30, rig.controller.wait_command(id)).await;
    assert!(matches!(ended, Progress::Succeeded(_)), "{ended:?}");
    assert_eq!(
        rig.machine.handler.runs_of(id),
        1,
        "the answer came from the journal"
    );
}

#[tokio::test]
async fn what_a_machine_wrote_down_survives_its_own_restart() {
    let mut rig = rig();
    rig.machine.link.connected().await;
    let thread = new_id();
    let id = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 300),
        &SessionClose { thread_id: thread },
    );
    let first = within(10, rig.controller.wait_command(id)).await;
    assert!(matches!(first, Progress::Succeeded(_)));
    // The connector is stopped and started again in the same directory.
    let _kept = rig.machine.keep_dir();
    let dir = rig.machine.data_dir().to_owned();
    rig.machine.link.shutdown().await;
    let identity = rig.pki.machine(rig.machine.machine_id);
    let reborn = ScriptedMachine::start_in(
        &rig.hub,
        &rig.shared,
        &identity,
        &MachineSpec::default(),
        &dir,
    );
    reborn.link.connected().await;
    rig.controller.issue_with_id(
        id,
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 300),
        &SessionClose { thread_id: thread },
    );
    let again = within(30, rig.controller.wait_command(id)).await;
    assert_eq!(first, again);
    assert_eq!(reborn.handler.runs_of(id), 0, "the new run did not run it");
}

#[tokio::test]
async fn commands_with_one_key_run_one_at_a_time_in_the_order_they_came_and_others_run_beside_them()
{
    let rig = rig();
    rig.machine.link.connected().await;
    rig.machine.handler.hold(CommandKind::SessionClose);
    let (thread_a, thread_b) = (new_id(), new_id());
    let first = rig.controller.issue(
        &SerialKey::Thread(thread_a),
        deadline(&rig.clock, 300),
        &SessionClose {
            thread_id: thread_a,
        },
    );
    let second = rig.controller.issue(
        &SerialKey::Thread(thread_a),
        deadline(&rig.clock, 300),
        &SessionClose {
            thread_id: thread_a,
        },
    );
    let other = rig.controller.issue(
        &SerialKey::Thread(thread_b),
        deadline(&rig.clock, 300),
        &SessionClose {
            thread_id: thread_b,
        },
    );
    within(10, rig.machine.handler.started(first)).await;
    within(10, rig.machine.handler.started(other)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        rig.machine.handler.runs_of(second),
        0,
        "it waits for the one before it with the same key"
    );
    rig.machine.handler.release();
    within(10, rig.controller.wait_command(second)).await;
    let order: Vec<_> = rig
        .machine
        .handler
        .executions()
        .iter()
        .map(|run| run.id)
        .collect();
    let place = |id| order.iter().position(|run| *run == id).unwrap();
    assert!(place(first) < place(second), "arrival order within a key");
}

#[tokio::test]
async fn a_command_whose_deadline_passed_before_its_turn_is_not_started() {
    let rig = rig();
    rig.machine.link.connected().await;
    rig.machine.handler.hold(CommandKind::SessionClose);
    let thread = new_id();
    let blocker = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 600),
        &SessionClose { thread_id: thread },
    );
    let late = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 30),
        &SessionClose { thread_id: thread },
    );
    within(10, rig.machine.handler.started(blocker)).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    rig.clock.advance(chrono::Duration::seconds(60));
    rig.machine.handler.release();
    let ended = within(10, rig.controller.wait_command(late)).await;
    let Progress::Failed(error) = ended else {
        panic!("{ended:?}")
    };
    assert_eq!(error.code, ErrorCode::Timeout.as_str());
    assert_eq!(rig.machine.handler.runs_of(late), 0);
}

#[tokio::test]
async fn a_command_that_reaches_a_machine_before_it_has_said_what_carries_commands_out_waits_for_it()
 {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let shared: Arc<dyn Clock> = Arc::new(clock.clone());
    let pki = TestPki::new(Arc::clone(&shared));
    let hub = Hub::with_limits(Arc::clone(&shared), pki.certificate(), Limits::default(), 1);
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&shared),
        id,
        vec![Role::Agent, Role::Environment],
    );
    // A connector opens its link first and builds its roles with the link in hand: its session stands, and
    // the system can send commands, before it has said what carries them out.
    let spec = MachineSpec::default();
    let dir = tempfile::tempdir().unwrap();
    let link = MachineLink::start(
        MachineConfig {
            hosted_endpoints: None,
            follow_system_endpoints: false,
            machine_id: id,
            connector_version: "0.1.0".to_owned(),
            platform: Platform {
                os: Os::Linux,
                arch: Arch::X86_64,
            },
            roles: spec.roles.clone(),
            proof: None,
            endpoints: vec![Hub::endpoint()],
            data_dir: dir.path().to_owned(),
            spool: spec.spool,
            journal_keep: Duration::from_secs(24 * 3_600),
            connected: spec.connected,
            tunnels: spec.tunnels,
            tunnel_gauge: spec.gauge.clone(),
        },
        hub.dialer(Some(identity.certificate.clone())),
        Arc::clone(&shared),
    )
    .unwrap();
    link.connected().await;
    let thread = new_id();
    let command = controller.issue(
        &SerialKey::Thread(thread),
        deadline(&clock, 300),
        &SessionClose { thread_id: thread },
    );
    // The machine has written it down and said so; the next moments are its chance to fail it.
    within(10, async {
        while !matches!(controller.progress(command), Some(Progress::Accepted)) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(
        controller.progress(command),
        Some(Progress::Accepted),
        "it is in hand and waits"
    );
    let handler = Arc::new(ScriptedHandler::default());
    link.serve(handler.clone());
    let ended = within(10, controller.wait_command(command)).await;
    assert!(matches!(ended, Progress::Succeeded(_)), "{ended:?}");
    assert_eq!(handler.runs_of(command), 1);
}

#[tokio::test]
async fn a_command_for_a_role_the_machine_does_not_have_is_refused_as_unsupported() {
    let rig = rig();
    rig.machine.link.connected().await;
    let id = rig.controller.issue(
        &SerialKey::Machine,
        deadline(&rig.clock, 300),
        &orch_link_contract::commands::NodeProbe {},
    );
    let ended = within(10, rig.controller.wait_command(id)).await;
    let Progress::Failed(error) = ended else {
        panic!("{ended:?}")
    };
    assert_eq!(error.code, ErrorCode::Unsupported.as_str());
    assert_eq!(rig.machine.handler.runs_of(id), 0);
}

fn issuer() -> (
    CommandIssuer,
    mpsc::UnboundedReceiver<DownFrame>,
    ManualClock,
) {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let (down, frames) = mpsc::unbounded_channel();
    (
        CommandIssuer::new(down, Arc::new(clock.clone())),
        frames,
        clock,
    )
}

#[tokio::test]
async fn a_durable_terminal_reconciliation_neither_sends_nor_resurrects_its_original_command() {
    let (issuer, mut frames, clock) = issuer();
    issuer.session_ready();
    let original = command(&clock, 600);
    let handle = issuer.issue(original.clone());
    assert_eq!(frames.recv().await, Some(DownFrame::Cmd(original.clone())));
    assert!(issuer.reconcile_terminal(original.id, Progress::Cancelled));
    assert_eq!(handle.progress(), Progress::Cancelled);
    assert!(
        frames.try_recv().is_err(),
        "local durable reconciliation emits no cancel, replacement or command frame"
    );
    let replay = issuer.adopt(original.clone(), Progress::Queued);
    assert_eq!(replay.progress(), Progress::Cancelled);
    issuer.tick(tokio::time::Instant::now());
    assert!(
        frames.try_recv().is_err(),
        "same-ID adoption cannot reissue a durable terminal command"
    );
    assert_eq!(issuer.progress(original.id), Some(Progress::Cancelled));
}

#[tokio::test]
async fn durable_reconciliation_refuses_nonterminal_input_and_preserves_the_first_terminal_outcome()
{
    let (issuer, mut frames, clock) = issuer();
    issuer.session_ready();
    let original = command(&clock, 600);
    let handle = issuer.issue(original.clone());
    assert_eq!(frames.recv().await, Some(DownFrame::Cmd(original.clone())));
    assert!(!issuer.reconcile_terminal(original.id, Progress::Queued));
    assert_eq!(handle.progress(), Progress::Sent);
    let first = json!({"actual_first_result":true});
    issuer.on_cmd_res(&orch_link_contract::commands::CmdRes {
        id: original.id,
        outcome: Ok(first.clone()),
    });
    assert!(!issuer.reconcile_terminal(original.id, Progress::Cancelled));
    assert_eq!(handle.progress(), Progress::Succeeded(first));
    assert!(frames.try_recv().is_err());
}

fn command(clock: &ManualClock, lasts: i64) -> Cmd {
    Cmd::new(
        new_id(),
        &SerialKey::Machine,
        deadline(clock, lasts),
        &SessionClose {
            thread_id: new_id(),
        },
    )
    .unwrap()
}

#[tokio::test(start_paused = true)]
async fn a_machine_that_does_not_acknowledge_a_command_is_asked_once_more_and_then_the_command_fails_as_retryable()
 {
    let (issuer, mut frames, clock) = issuer();
    issuer.session_ready();
    let handle = issuer.issue(command(&clock, 3_600));
    assert!(matches!(frames.recv().await, Some(DownFrame::Cmd(_))));
    assert_eq!(handle.progress(), Progress::Sent);
    tokio::time::advance(Duration::from_secs(31)).await;
    issuer.tick(tokio::time::Instant::now());
    assert!(
        matches!(frames.recv().await, Some(DownFrame::Cmd(_))),
        "sent once more"
    );
    tokio::time::advance(Duration::from_secs(31)).await;
    issuer.tick(tokio::time::Instant::now());
    let Progress::Failed(error) = handle.progress() else {
        panic!("{:?}", handle.progress())
    };
    assert!(error.retryable);
    assert_eq!(error.code, ErrorCode::Timeout.as_str());
}

#[tokio::test(start_paused = true)]
async fn a_command_no_machine_took_expires_at_its_deadline_and_one_a_machine_took_does_not() {
    let (issuer, mut frames, clock) = issuer();
    // Nobody is connected: it waits, and its time runs out.
    let waiting = issuer.issue(command(&clock, 60));
    assert_eq!(waiting.progress(), Progress::Queued);
    // Another is taken by a machine that writes it down.
    issuer.session_ready();
    let taken = issuer.issue(command(&clock, 60));
    assert!(
        matches!(frames.recv().await, Some(DownFrame::Cmd(_))),
        "the one that waited is sent first"
    );
    assert!(matches!(frames.recv().await, Some(DownFrame::Cmd(_))));
    issuer.on_cmd_ack(&orch_link_contract::commands::CmdAck { id: taken.id() });
    issuer.session_down();
    clock.advance(chrono::Duration::seconds(90));
    issuer.tick(tokio::time::Instant::now());
    assert_eq!(waiting.progress(), Progress::Expired);
    assert_eq!(
        taken.progress(),
        Progress::Accepted,
        "a machine has it, so it is not expired"
    );
}

#[test]
fn a_secret_in_a_stored_command_is_replaced_by_a_value_only_its_machine_opens() {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ));
    let pki = TestPki::new(Arc::clone(&clock));
    let (mine, other) = (pki.machine(new_id()), pki.machine(new_id()));
    let secret_id = new_id();
    let command_id = new_id();
    let mut body = json!({ "api_key": { "$secret": secret_id.to_string() }, "name": "plain" });
    assert!(orch_link_contract::commands::has_references(&body));
    let recipient = RecipientKey::from_certificate(&mine.certificate);
    resolve_references(&mut body, &mut |reference| match reference {
        Reference::Secret(id) => {
            assert_eq!(id, secret_id);
            seal(&recipient, &info_for_command(command_id), b"sk-live-123")
                .map_err(|e| e.to_string())
        }
        Reference::Ticket(_) => Err("no ticket here".to_owned()),
    })
    .unwrap();
    assert!(
        !orch_link_contract::commands::has_references(&body),
        "the frame can be sent now"
    );
    let sealed = body["api_key"]["$sealed"].as_str().unwrap().to_owned();
    assert_eq!(
        open(&mine.key, &info_for_command(command_id), &sealed).unwrap(),
        b"sk-live-123"
    );
    assert!(
        open(&other.key, &info_for_command(command_id), &sealed).is_err(),
        "no other machine reads it"
    );
    assert!(!serde_json::to_string(&body).unwrap().contains("sk-live"));
}

#[tokio::test]
async fn cancellation_received_before_command_admission_prevents_any_handler_work() {
    use orch_link::commands::{CommandServer, ServerOptions};
    use orch_link::journal::{Journal, Step};
    use orch_link_contract::commands::CmdCancel;
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(orch_testkit::clock::start()));
    let directory = tempfile::tempdir().unwrap();
    let journal = Journal::open(directory.path(), Duration::from_secs(3600), clock.time()).unwrap();
    let server = CommandServer::new(
        journal,
        clock.clone(),
        ServerOptions {
            roles: vec![Role::Agent],
            connected: true,
        },
    );
    let handler = Arc::new(ScriptedHandler::default());
    server.serve(handler.clone());
    let id = new_id();
    let command = Cmd {
        id,
        kind: CommandKind::SessionClose.as_str().into(),
        key: None,
        deadline: Time::new(clock.now() + chrono::Duration::minutes(1)),
        body: json!({"thread_id":new_id()}),
    };
    server.on_cancel(CmdCancel { id }).await;
    server.on_cmd(command).await;
    within(10, async {
        loop {
            if server
                .entry(id)
                .is_some_and(|entry| matches!(entry.step, Step::Done { .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(
        handler.runs_of(id),
        0,
        "a cancellation crossed admission and started new work"
    );
}

#[tokio::test]
async fn a_stop_after_restart_waits_for_interrupted_external_work_to_be_cleaned_up() {
    use async_trait::async_trait;
    use orch_link::commands::{CommandContext, CommandHandler, CommandServer, ServerOptions};
    use orch_link::journal::{Journal, Step};
    use orch_link_contract::commands::{CmdCancel, Command, CommandError, CommandResult};
    struct Cleanup {
        entered: tokio::sync::Semaphore,
        released: tokio::sync::Semaphore,
        runs: std::sync::atomic::AtomicUsize,
    }
    #[async_trait]
    impl CommandHandler for Cleanup {
        async fn interrupted(&self, _: orch_types::Id, _: &Command) -> Result<(), CommandError> {
            self.entered.add_permits(1);
            self.released.acquire().await.unwrap().forget();
            Ok(())
        }
        async fn handle(
            &self,
            _: CommandContext,
            _: Command,
        ) -> Result<CommandResult, CommandError> {
            self.runs.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            panic!("a started journal entry must not rerun the original operation")
        }
    }
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(orch_testkit::clock::start()));
    let directory = tempfile::tempdir().unwrap();
    let id = new_id();
    let command = Cmd {
        id,
        kind: CommandKind::SessionClose.as_str().into(),
        key: None,
        deadline: Time::new(clock.now() + chrono::Duration::minutes(1)),
        body: json!({"thread_id":new_id()}),
    };
    let mut journal =
        Journal::open(directory.path(), Duration::from_secs(3600), clock.time()).unwrap();
    journal.accept(&command, clock.time()).unwrap();
    journal.started(id).unwrap();
    let server = CommandServer::new(
        journal,
        clock,
        ServerOptions {
            roles: vec![Role::Agent],
            connected: true,
        },
    );
    let cleanup = Arc::new(Cleanup {
        entered: tokio::sync::Semaphore::new(0),
        released: tokio::sync::Semaphore::new(0),
        runs: std::sync::atomic::AtomicUsize::new(0),
    });
    server.serve(cleanup.clone());
    server.on_cancel(CmdCancel { id }).await;
    within(10, cleanup.entered.acquire())
        .await
        .unwrap()
        .forget();
    assert!(
        matches!(server.entry(id).unwrap().step, Step::Started),
        "stop acknowledged before external cleanup finished"
    );
    cleanup.released.add_permits(1);
    within(10, async {
        loop {
            if server
                .entry(id)
                .is_some_and(|entry| matches!(entry.step, Step::Done { .. }))
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert_eq!(cleanup.runs.load(std::sync::atomic::Ordering::SeqCst), 0);
}
