//! A message of any size crosses the link: the reply of a command, a command, an event. Whatever is larger than a
//! frame travels in parts and arrives whole, in order with the frames around it, without holding the session up;
//! whatever is larger than the session carries is refused where it is sent, with its size and the setting, and the
//! session stays up.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::commands::Progress;
use orch_link::framing::{
    Inbound, LinkMeter, PeerInfo, memory_pair, queues, read_loop, write_loop,
};
use orch_link::machine::LinkStatus;
use orch_link::spool::SpoolOptions;
use orch_link::streams::{PublishError, StreamSender};
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, GitDiff, GitDiffResult, ProcList,
    SerialKey, TurnInterrupt,
};
use orch_link_contract::events::{Event, SessionTitle};
use orch_link_contract::frames::Frame;
use orch_link_contract::session::Role;
use orch_link_contract::streams::{Ack, ResetReason, Stream};
use orch_link_contract::version::Limits;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Direction, Hub, Shape, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, ManualClock, Time, new_id};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

const MIB: usize = 1_048_576;

#[tokio::test]
async fn every_previously_unbounded_command_kind_crosses_in_parts_past_one_message() {
    use orch_testkit::doubles::link::large_messages::{KINDS, example};
    let limits = Limits::default();
    assert_eq!(
        CommandKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.is_paged())
            .collect::<Vec<_>>(),
        KINDS
    );
    for kind in KINDS {
        let frame = example(kind, limits.message_bytes() + 1);
        let expected = serde_json::to_vec(&frame).unwrap();
        assert!(expected.len() > limits.message_bytes(), "{kind}");
        let (out, queued) = queues(limits);
        let (sending, receiving) = memory_pair(PeerInfo::default(), PeerInfo::default());
        let (inbound, mut received) = mpsc::unbounded_channel();
        let end = CancellationToken::new();
        let writer = tokio::spawn(write_loop(
            sending.sink,
            queued,
            LinkMeter::new(),
            limits.frame_bytes(),
            end.clone(),
        ));
        let reader = tokio::spawn(read_loop(
            receiving.source,
            inbound,
            limits,
            LinkMeter::new(),
            Duration::from_secs(30),
        ));
        if matches!(&frame, Frame::Cmd(_)) {
            out.control(&frame).unwrap();
        } else {
            out.control_paged(&frame).unwrap();
        }
        let delivered = within(30, received.recv()).await.unwrap();
        let Inbound::Frame(actual) = delivered else {
            panic!("{kind}: {delivered:?}")
        };
        assert_eq!(
            serde_json::to_vec(actual.as_ref()).unwrap(),
            expected,
            "{kind}"
        );
        end.cancel();
        writer.await.unwrap().unwrap();
        reader.abort();
        let _ = reader.await;
    }
}

struct Rig {
    clock: ManualClock,
    hub: Hub,
    controller: ScriptedController,
    machine: ScriptedMachine,
    stream: Stream,
    name: String,
    environment: Id,
}

/// A machine and a controller joined by a hub whose sessions ping every second, so that a session that
/// stops hearing is given up within three.
fn rig(limits: Limits) -> Rig {
    rig_connecting(limits, true)
}

/// The same, with the machine let into the hub at once or not: one that is not spools what it is given until
/// the hub lets it in.
fn rig_connecting(limits: Limits, connect: bool) -> Rig {
    rig_with_heartbeat(limits, connect, 1)
}

fn rig_with_heartbeat(limits: Limits, connect: bool, heartbeat_secs: u32) -> Rig {
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let shared: Arc<dyn Clock> = Arc::new(clock.clone());
    let pki = TestPki::new(Arc::clone(&shared));
    let hub = Hub::with_limits(
        Arc::clone(&shared),
        pki.certificate(),
        limits,
        heartbeat_secs,
    );
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&shared),
        id,
        vec![Role::Agent, Role::Environment],
    );
    if !connect {
        hub.faults().refuse_connections(1_000_000);
    }
    let machine = ScriptedMachine::start(&hub, &shared, &identity, &MachineSpec::default());
    let stream = Stream::Session {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    let name = stream.to_string();
    Rig {
        clock,
        hub,
        controller,
        machine,
        stream,
        name,
        environment: new_id(),
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

/// A reading of the repository's differences: the path of the body says how many bytes the answer has.
fn answer_as_long_as_the_path_says(command: &Command) -> Result<CommandResult, CommandError> {
    let Command::GitDiff(body) = command else {
        return Err(CommandError::new(ErrorCode::Internal, "not a diff"));
    };
    let size = body
        .path
        .as_deref()
        .and_then(|path| path.parse::<usize>().ok())
        .unwrap_or(0);
    Ok(CommandResult::GitDiff(GitDiffResult {
        text: "d".repeat(size),
        truncated: false,
    }))
}

/// The same command, answered with how many bytes its path has.
fn answer_with_the_length_of_the_path(command: &Command) -> Result<CommandResult, CommandError> {
    let Command::GitDiff(body) = command else {
        return Err(CommandError::new(ErrorCode::Internal, "not a diff"));
    };
    Ok(CommandResult::GitDiff(GitDiffResult {
        text: body.path.as_deref().map_or(0, str::len).to_string(),
        truncated: false,
    }))
}

fn diff(rig: &Rig, path: String) -> GitDiff {
    GitDiff {
        environment_id: rig.environment,
        path: Some(path),
        staged: None,
    }
}

fn ask_for(rig: &Rig, key: &SerialKey, path: String) -> Id {
    rig.controller
        .issue(key, deadline(&rig.clock, 300), &diff(rig, path))
}

fn title(n: usize, padding: usize) -> Event {
    Event::SessionTitle(SessionTitle {
        title: format!("title {n:04}{}", "x".repeat(padding)),
    })
}

async fn publish_titles(rig: &Rig, range: std::ops::Range<usize>, padded: Option<usize>) {
    for n in range {
        let padding = if Some(n) == padded { 600_000 } else { 0 };
        rig.machine.publish(&rig.stream, &title(n, padding)).await;
    }
}

async fn text_of(rig: &Rig, id: Id) -> String {
    match within(60, rig.controller.wait_command(id)).await {
        Progress::Succeeded(body) => body["text"].as_str().unwrap().to_owned(),
        other => panic!("the command did not succeed: {other:?}"),
    }
}

fn assert_applied_in_order(rig: &Rig, count: usize) {
    let applied = rig.controller.applied(&rig.name);
    assert_eq!(applied.len(), count);
    for (place, event) in applied.iter().enumerate() {
        assert_eq!(event.seq, place as i64 + 1);
        assert!(
            event.body["title"]
                .as_str()
                .unwrap()
                .starts_with(&format!("title {place:04}")),
            "{place}"
        );
    }
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
}

#[tokio::test]
async fn a_command_reply_of_a_megabyte_arrives_whole_and_in_order_with_the_frames_around_it() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    publish_titles(&rig, 0..5, None).await;
    let id = ask_for(&rig, &SerialKey::None, MIB.to_string());
    publish_titles(&rig, 5..10, None).await;
    let text = text_of(&rig, id).await;
    assert_eq!(text.len(), MIB);
    assert!(text.bytes().all(|byte| byte == b'd'));
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&rig.name).len()
                >= 10)
            .await
    );
    assert_applied_in_order(&rig, 10);
}

#[tokio::test]
async fn two_large_replies_interleave_with_small_ones_and_each_arrives_whole() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    // At one Mbit/s neither large reply can finish within the small reply's
    // two-second wait. Serializing the large replies ahead of it must fail this case.
    rig.hub.faults().shape(
        Direction::Up,
        Some(Shape {
            delay: Duration::ZERO,
            bytes_per_second: 125_000,
            loss: 0.0,
        }),
    );
    let mut asked = vec![(
        ask_for(&rig, &SerialKey::None, "600000".to_owned()),
        600_000,
    )];
    for size in 1..=10 {
        asked.push((ask_for(&rig, &SerialKey::None, size.to_string()), size));
    }
    asked.push((
        ask_for(&rig, &SerialKey::None, "900000".to_owned()),
        900_000,
    ));
    for size in 11..=20 {
        asked.push((ask_for(&rig, &SerialKey::None, size.to_string()), size));
    }
    let Progress::Succeeded(small) = within(2, rig.controller.wait_command(asked[1].0)).await
    else {
        panic!("the small reply did not pass the large replies");
    };
    assert_eq!(small["text"].as_str().unwrap().len(), 1);
    assert!(!rig.controller.progress(asked[0].0).unwrap().is_final());
    assert!(!rig.controller.progress(asked[11].0).unwrap().is_final());
    for (id, size) in asked {
        assert_eq!(text_of(&rig, id).await.len(), size);
    }
}

#[tokio::test]
async fn a_command_of_a_megabyte_reaches_the_machine_whole() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    let id = ask_for(&rig, &SerialKey::None, "p".repeat(MIB));
    assert_eq!(text_of(&rig, id).await, MIB.to_string());
}

#[tokio::test]
async fn a_large_command_on_a_slow_line_succeeds_once_instead_of_failing_before_it_arrives() {
    // The liveness wait exceeds this transfer, independently of the shorter command ACK wait.
    let rig = rig_with_heartbeat(Limits::default(), false, 60);
    // 8,192 initial 4-KiB parts let a socket accept two twelve-MiB attempts ahead of the line.
    rig.hub.faults().buffer_frames(8_192);
    // Twelve MiB takes more than the two acknowledgement waits on a one-Mbit line,
    // while staying inside the command's five-minute deadline.
    rig.hub.faults().shape(
        Direction::Down,
        Some(Shape {
            delay: Duration::ZERO,
            bytes_per_second: 125_000,
            loss: 0.0,
        }),
    );
    rig.hub.faults().refuse_connections(0);
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    let id = ask_for(&rig, &SerialKey::None, "p".repeat(12 * MIB));
    for _ in 0..2 {
        tokio::time::sleep(Duration::from_secs(31)).await;
        rig.controller
            .link()
            .commands()
            .tick(tokio::time::Instant::now());
    }
    assert_eq!(rig.controller.progress(id), Some(Progress::Sent));
    assert_eq!(rig.machine.handler.runs_of(id), 0, "it is still arriving");
    let Progress::Succeeded(body) = within(120, rig.controller.wait_command(id)).await else {
        panic!("the command did not succeed after its transfer");
    };
    assert_eq!(body["text"], (12 * MIB).to_string());
    assert_eq!(rig.machine.handler.runs_of(id), 1);
    assert_eq!(rig.hub.faults().connections(), 1);
}

#[tokio::test]
async fn a_small_command_stays_behind_a_large_one_of_its_key_that_was_sent_before_it() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    let large = ask_for(&rig, &SerialKey::Machine, "p".repeat(MIB));
    let small = ask_for(&rig, &SerialKey::Machine, "p".to_owned());
    assert_eq!(text_of(&rig, large).await, MIB.to_string());
    assert_eq!(text_of(&rig, small).await, "1");
    let order: Vec<Id> = rig
        .machine
        .handler
        .executions()
        .into_iter()
        .map(|run| run.id)
        .collect();
    assert_eq!(
        order,
        [large, small],
        "the machine took them in the order they were sent"
    );
}

#[tokio::test]
async fn another_threads_interrupt_arrives_while_a_large_command_is_still_crossing_the_line() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    rig.hub.faults().shape(
        Direction::Down,
        Some(Shape {
            delay: Duration::ZERO,
            bytes_per_second: 125_000,
            loss: 0.0,
        }),
    );
    let large = ask_for(&rig, &SerialKey::Thread(new_id()), "p".repeat(4 * MIB));
    tokio::time::sleep(Duration::from_secs(1)).await;
    let thread = new_id();
    let interrupt = rig.controller.issue(
        &SerialKey::Thread(thread),
        deadline(&rig.clock, 30),
        &TurnInterrupt {
            thread_id: thread,
            turn_id: new_id(),
        },
    );
    assert!(matches!(
        within(5, rig.controller.wait_command(interrupt)).await,
        Progress::Succeeded(_)
    ));
    assert_eq!(
        rig.machine.handler.runs_of(large),
        0,
        "the large request is still arriving"
    );
    assert_eq!(text_of(&rig, large).await, (4 * MIB).to_string());
    assert_eq!(rig.machine.handler.runs_of(interrupt), 1);
}

#[tokio::test]
async fn a_large_event_arrives_whole_and_in_order_with_the_events_around_it() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    publish_titles(&rig, 0..8, Some(3)).await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&rig.name).len() >= 8)
            .await
    );
    assert_applied_in_order(&rig, 8);
    assert!(
        rig.controller.applied(&rig.name)[3].body["title"]
            .as_str()
            .unwrap()
            .len()
            > 600_000,
        "the large event is whole"
    );
}

#[tokio::test]
async fn a_reply_over_the_bound_fails_at_once_with_its_size_and_the_setting_and_the_session_stays_up()
 {
    let rig = rig(Limits {
        max_message_bytes: 512 * 1024,
        ..Limits::default()
    });
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    let session = rig.hub.session_of(rig.machine.machine_id);
    let id = ask_for(&rig, &SerialKey::None, MIB.to_string());
    let Progress::Failed(error) = within(10, rig.controller.wait_command(id)).await else {
        panic!("the answer was not refused");
    };
    assert_eq!(error.code, "too_large");
    assert!(
        error.detail.contains("max_message_bytes"),
        "{}",
        error.detail
    );
    assert!(error.detail.contains("524288"), "{}", error.detail);
    // The session stands, and a small answer goes through it.
    let small = ask_for(&rig, &SerialKey::None, "10".to_owned());
    assert_eq!(text_of(&rig, small).await.len(), 10);
    assert_eq!(rig.hub.session_of(rig.machine.machine_id), session);
    assert_eq!(rig.hub.faults().connections(), 1);
}

#[tokio::test]
async fn a_command_over_the_bound_fails_at_once_with_its_size_and_the_setting_and_never_reaches_the_machine()
 {
    let rig = rig(Limits {
        max_message_bytes: 512 * 1024,
        ..Limits::default()
    });
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_with_the_length_of_the_path);
    let id = ask_for(&rig, &SerialKey::None, "p".repeat(MIB));
    let Progress::Failed(error) = within(10, rig.controller.wait_command(id)).await else {
        panic!("the command was not refused");
    };
    assert_eq!(error.code, "too_large");
    assert!(
        error.detail.contains("max_message_bytes"),
        "{}",
        error.detail
    );
    assert_eq!(rig.machine.handler.runs_of(id), 0);
    // The session stands.
    let small = ask_for(&rig, &SerialKey::None, "p".to_owned());
    assert_eq!(text_of(&rig, small).await, "1");
    assert_eq!(rig.hub.faults().connections(), 1);
}

#[tokio::test]
async fn an_event_over_the_bound_is_refused_to_its_publisher_with_the_setting_and_the_events_after_it_arrive()
 {
    let rig = rig(Limits {
        max_message_bytes: 512 * 1024,
        ..Limits::default()
    });
    rig.machine.link.connected().await;
    publish_titles(&rig, 0..3, None).await;
    let refused = rig
        .machine
        .link
        .publish(&rig.stream, &title(3, 600_000))
        .await
        .unwrap_err();
    assert!(
        matches!(refused, PublishError::TooLarge { max, .. } if max == 512 * 1024),
        "{refused:?}"
    );
    assert!(
        refused.to_string().contains("max_message_bytes"),
        "{refused}"
    );
    publish_titles(&rig, 3..6, None).await;
    // The refused event took no number: the ones after it follow the ones before it.
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&rig.name).len() >= 6)
            .await
    );
    assert_eq!(rig.controller.applied(&rig.name).len(), 6);
    assert_eq!(rig.hub.faults().connections(), 1);
}

#[tokio::test]
async fn a_session_cut_in_the_middle_of_a_large_reply_gives_the_reply_once_on_the_next_session() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    // The line breaks when the fourth part of a message is to pass.
    rig.hub.faults().cut_after(Direction::Up, Some("part"), 3);
    let id = ask_for(&rig, &SerialKey::None, MIB.to_string());
    assert_eq!(text_of(&rig, id).await.len(), MIB);
    assert_eq!(rig.hub.faults().fired(), 1, "the line was cut once");
    assert!(
        rig.hub.faults().connections() >= 2,
        "and a new session was made"
    );
    assert_eq!(
        rig.machine.handler.runs_of(id),
        1,
        "the command ran once; its answer came again from what was written down"
    );
}

#[tokio::test]
async fn a_paged_reply_cut_midway_is_replayed_whole_without_running_the_command_twice() {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    let Frame::CmdRes(example) = orch_testkit::doubles::link::large_messages::example(
        CommandKind::ProcList,
        Limits::default().message_bytes() + 1,
    ) else {
        panic!("a reply")
    };
    let expected = Arc::new(example.outcome.unwrap());
    let reply = Arc::clone(&expected);
    rig.machine.handler.on(CommandKind::ProcList, move |_| {
        Ok(CommandResult::decode(CommandKind::ProcList, (*reply).clone()).unwrap())
    });
    rig.hub.faults().cut_after(Direction::Up, Some("part"), 3);
    let id = rig.controller.issue(
        &SerialKey::None,
        deadline(&rig.clock, 300),
        &ProcList {
            environment_id: rig.environment,
        },
    );
    let Progress::Succeeded(actual) = within(120, rig.controller.wait_command(id)).await else {
        panic!("the paged answer did not complete after reconnecting");
    };
    assert_eq!(&actual, expected.as_ref());
    assert_eq!(rig.hub.faults().fired(), 1);
    assert!(rig.hub.faults().connections() >= 2);
    assert_eq!(rig.machine.handler.runs_of(id), 1);
}

#[tokio::test]
async fn a_message_whose_parts_stop_coming_is_given_up_after_the_sessions_wait_and_made_up_for_on_the_next()
 {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    let faults = rig.hub.faults();
    // From the third part on, the line loses every part, and goes on carrying everything else. The machine is
    // let into the hub again only once the line is mended: on a session that met the same line no part of the
    // reply would reach the gateway, which cannot know that a message was begun, and nothing would end that
    // session or send the reply again.
    faults.drop_frames(Direction::Up, "part", 2, 1_000_000);
    faults.refuse_connections(1_000_000);
    let id = ask_for(&rig, &SerialKey::None, MIB.to_string());
    // The gateway's side waits three heartbeats for the rest, then ends the session, and the machine is told.
    let mut status = rig.machine.link.status();
    let session_ended = within(30, async {
        status
            .wait_for(|link| !matches!(link, LinkStatus::Connected { .. }))
            .await
            .is_ok()
    })
    .await;
    assert!(session_ended, "the link of the machine was shut down");
    assert!(
        !matches!(rig.controller.progress(id), Some(Progress::Succeeded(_))),
        "the reply never completed"
    );
    // The line is mended before the machine can make its next session, which then carries the reply.
    faults.clear();
    faults.refuse_connections(0);
    assert_eq!(text_of(&rig, id).await.len(), MIB);
    assert_eq!(
        faults.connections(),
        2,
        "the second session carried the reply, with no third"
    );
    assert_eq!(
        rig.machine.handler.runs_of(id),
        1,
        "the command ran once; its answer came again from what was written down"
    );
}

/// What frames carry, in the order they go: an event as its number, a reset as the number it goes on from and why.
fn numbers_and_resets(frames: &[Frame]) -> Vec<String> {
    frames
        .iter()
        .flat_map(|frame| match frame {
            Frame::Ev(event) => vec![event.seq.to_string()],
            Frame::Evb(batch) => batch
                .events
                .iter()
                .map(|event| event.seq.to_string())
                .collect(),
            Frame::Reset(reset) => vec![format!("reset {} {}", reset.from, reset.reason.as_str())],
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn an_event_the_session_cannot_carry_waits_for_the_events_before_it_and_is_then_given_up_with_a_reset()
 {
    let directory = tempfile::tempdir().unwrap();
    let options = SpoolOptions {
        max_bytes: 64 * MIB as u64,
        max_age: Duration::from_secs(86_400),
        segment_bytes: 8 * MIB as u64,
    };
    let stream = Stream::Session {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    let name = stream.to_string();
    let at: Time = "2026-10-13T08:00:00.000Z".parse().unwrap();
    let now = tokio::time::Instant::now();
    let (mut sender, _) =
        StreamSender::open_for_machine(directory.path(), options, vec![Role::Agent], now).unwrap();
    // Spooled before any session stated its limits, so under the shipped ones: the fourth is larger than the
    // session that comes carries.
    for n in 0..6 {
        let padding = if n == 3 { 300_000 } else { 0 };
        let title = format!("title {n}{}", "x".repeat(padding));
        sender
            .publish(
                &stream,
                at,
                "session.title",
                serde_json::json!({ "title": title }),
                now,
            )
            .unwrap();
    }
    let limits = Limits {
        max_frame_bytes: 64 * 1024,
        max_message_bytes: 256 * 1024,
        ..Limits::default()
    };
    assert!(
        sender
            .on_ready(&std::collections::BTreeMap::new(), limits, now)
            .unwrap()
            .is_empty()
    );
    // The events before it go, and the stream stops there: nothing after it is sent past a hole.
    let first = sender.pump(64 * 1024, now).unwrap();
    assert_eq!(numbers_and_resets(&first), ["1", "2", "3"]);
    assert!(
        numbers_and_resets(&sender.pump(64 * 1024, now).unwrap()).is_empty(),
        "it waits until the ones before it are acknowledged"
    );
    // The events before it were on the wire and no word came: they are sent again, not left behind it.
    sender.on_stall(Duration::ZERO, now);
    let again = sender.pump(64 * 1024, now).unwrap();
    assert_eq!(numbers_and_resets(&again), ["1", "2", "3"]);
    sender
        .on_ack(
            &Ack {
                stream: name.clone(),
                seq: 3,
            },
            now,
        )
        .unwrap();
    // Now it is the oldest the system has not taken: it is given up, the system is told, the stream goes on.
    let then = sender.pump(64 * 1024, now).unwrap();
    assert_eq!(
        numbers_and_resets(&then),
        ["reset 5 too_large", "5", "6"],
        "the reset comes before the events after the one given up"
    );
    assert!(sender.pump(64 * 1024, now).unwrap().is_empty());
    sender
        .on_ack(
            &Ack {
                stream: name,
                seq: 6,
            },
            now,
        )
        .unwrap();
    assert!(!sender.has_unacknowledged());
}

#[tokio::test]
async fn an_event_spooled_under_a_larger_limit_than_the_next_session_states_is_given_up_once_and_the_stream_goes_on()
 {
    let rig = rig_connecting(
        Limits {
            max_message_bytes: 512 * 1024,
            ..Limits::default()
        },
        false,
    );
    // While no session stands the limit is the shipped one, so every event is taken in.
    publish_titles(&rig, 0..8, Some(3)).await;
    rig.hub.faults().refuse_connections(0);
    rig.machine.link.connected().await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&rig.name).len() >= 7)
            .await,
        "the stream was held up: {} applied, resets {:?}",
        rig.controller.applied(&rig.name).len(),
        rig.controller.resets()
    );
    let applied = rig.controller.applied(&rig.name);
    let numbers: Vec<i64> = applied.iter().map(|event| event.seq).collect();
    assert_eq!(
        numbers,
        [1, 2, 3, 5, 6, 7, 8],
        "all but the one that cannot go"
    );
    let resets = rig.controller.resets();
    assert_eq!(resets.len(), 1, "the system is told once: {resets:?}");
    assert_eq!(resets[0].reason, ResetReason::TooLarge);
    assert_eq!(resets[0].from, 5);
    assert!(
        resets[0].resync_requested,
        "a session is repaired by being reported again"
    );
    // The stream goes on after it, and the session stood throughout.
    publish_titles(&rig, 8..10, None).await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&rig.name).len() >= 9)
            .await
    );
    assert_eq!(rig.controller.resets().len(), 1);
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
    assert_eq!(rig.hub.faults().connections(), 1);
}
