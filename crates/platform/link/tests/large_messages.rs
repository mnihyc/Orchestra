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
use orch_link::streams::PublishError;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, ErrorCode, GitDiff, GitDiffResult, SerialKey,
};
use orch_link_contract::events::{Event, SessionTitle};
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_link_contract::version::Limits;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Direction, Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, ManualClock, Time, new_id};
use std::sync::Arc;
use std::time::Duration;

const MIB: usize = 1_048_576;

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
    let clock = ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    );
    let shared: Arc<dyn Clock> = Arc::new(clock.clone());
    let pki = TestPki::new(Arc::clone(&shared));
    let hub = Hub::with_limits(Arc::clone(&shared), pki.certificate(), limits, 1);
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&shared),
        id,
        vec![Role::Agent, Role::Environment],
    );
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
async fn a_message_whose_parts_stop_coming_is_given_up_after_the_sessions_wait_and_made_up_for_on_the_next()
 {
    let rig = rig(Limits::default());
    rig.machine.link.connected().await;
    rig.machine
        .handler
        .on(CommandKind::GitDiff, answer_as_long_as_the_path_says);
    // From the third part on, the line loses every part, and goes on carrying everything else.
    rig.hub
        .faults()
        .drop_frames(Direction::Up, "part", 2, 1_000_000);
    let id = ask_for(&rig, &SerialKey::None, MIB.to_string());
    // The gateway's side waits three heartbeats for the rest, then ends the session, and the machine makes a new one.
    within(30, async {
        while rig.hub.faults().connections() < 2 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await;
    assert!(
        !matches!(rig.controller.progress(id), Some(Progress::Succeeded(_))),
        "the reply never completed"
    );
    rig.hub.faults().clear();
    assert_eq!(text_of(&rig, id).await.len(), MIB);
}
