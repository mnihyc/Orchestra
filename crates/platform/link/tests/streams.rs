//! Events from a machine to the system: exactly once, in order, whatever the link does to them.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link::spool::SpoolOptions;
use orch_link_contract::commands::CommandKind;
use orch_link_contract::events::{Event, ItemCompleted, ItemDelta, RawFrame, SessionTitle};
use orch_link_contract::session::Role;
use orch_link_contract::streams::{ResetReason, Stream};
use orch_link_contract::version::Limits;
use orch_testkit::doubles::controller::{CommitMode, ScriptedController};
use orch_testkit::doubles::link::{Direction, Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, Id, ManualClock, new_id};
use std::sync::Arc;
use std::time::Duration;

struct Rig {
    hub: Hub,
    controller: ScriptedController,
    machine: ScriptedMachine,
    stream: Stream,
    thread_id: Id,
    name: String,
}

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ))
}

/// A machine and a controller joined by a hub whose sessions ping every second, so that a stream that
/// stands still is sent again within a second.
fn rig(spec: MachineSpec, connect: bool) -> Rig {
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::with_limits(Arc::clone(&clock), pki.certificate(), Limits::default(), 1);
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(&hub, Arc::clone(&clock), id, spec.roles.clone());
    if !connect {
        hub.faults().refuse_connections(1_000_000);
    }
    let machine = ScriptedMachine::start(&hub, &clock, &identity, &spec);
    let thread_id = new_id();
    let stream = Stream::Session {
        thread_id,
        segment_id: new_id(),
    };
    let name = stream.to_string();
    Rig {
        hub,
        controller,
        machine,
        stream,
        thread_id,
        name,
    }
}

fn title(n: usize) -> Event {
    Event::SessionTitle(SessionTitle {
        title: format!("title {n:04}"),
    })
}

async fn publish_titles(rig: &Rig, range: std::ops::Range<usize>) {
    for n in range {
        rig.machine.publish(&rig.stream, &title(n)).await;
        if n % 7 == 6 {
            tokio::task::yield_now().await;
        }
    }
}

fn assert_exactly_once_in_order(rig: &Rig, count: usize) {
    let applied = rig.controller.applied(&rig.name);
    assert_eq!(applied.len(), count);
    for (place, event) in applied.iter().enumerate() {
        assert_eq!(event.seq, place as i64 + 1);
        assert_eq!(event.body["title"], format!("title {place:04}"));
    }
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
}

#[tokio::test]
async fn events_whose_frames_are_dropped_repeated_and_swapped_are_each_applied_once_in_order() {
    let rig = rig(MachineSpec::default(), true);
    rig.machine.link.connected().await;
    let faults = rig.hub.faults();
    faults.drop_frames(Direction::Up, "ev", 2, 4);
    faults.drop_frames(Direction::Up, "evb", 1, 3);
    faults.duplicate_frames(Direction::Up, "evb", 5, 4);
    faults.swap_frames(Direction::Up, "evb", 9, 3);
    faults.drop_frames(Direction::Down, "ack", 4, 4);
    publish_titles(&rig, 0..700).await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&rig.name).len()
                >= 700)
            .await,
        "{} of 700",
        rig.controller.applied(&rig.name).len()
    );
    assert_exactly_once_in_order(&rig, 700);
    assert!(
        faults.fired() >= 8,
        "the faults did fire: {}",
        faults.fired()
    );
}

#[tokio::test]
async fn a_session_lost_in_the_middle_of_a_batch_is_made_up_for_without_a_second_effect() {
    let rig = rig(MachineSpec::default(), true);
    rig.machine.link.connected().await;
    rig.hub.faults().cut_after(Direction::Up, None, 40);
    rig.hub.faults().cut_after(Direction::Up, None, 90);
    // Paced, so that the events go in many small batches and there are frames to cut in the middle of.
    for n in 0..400 {
        rig.machine.publish(&rig.stream, &title(n)).await;
        if n % 4 == 3 {
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    }
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&rig.name).len()
                >= 400)
            .await
    );
    assert_exactly_once_in_order(&rig, 400);
    assert!(rig.hub.faults().connections() >= 3);
}

#[tokio::test]
async fn an_api_that_restarts_before_it_acknowledges_receives_the_events_again_and_applies_each_once()
 {
    let rig = rig(MachineSpec::default(), true);
    rig.machine.link.connected().await;
    rig.controller.set_commit_mode(CommitMode::Held);
    publish_titles(&rig, 0..120).await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(10), |c| c.held_events() > 0)
            .await,
        "nothing reached the system"
    );
    // The system stumbles: what it had in hand is lost with it, uncommitted and unacknowledged.
    rig.controller.forget_held();
    rig.controller.restart();
    rig.controller.set_commit_mode(CommitMode::Immediate);
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&rig.name).len()
                >= 120)
            .await,
        "only {} of 120 events were applied after the restart",
        rig.controller.applied(&rig.name).len()
    );
    assert_exactly_once_in_order(&rig, 120);
}

#[tokio::test]
async fn a_commit_that_fails_leads_to_the_events_being_sent_again() {
    let rig = rig(MachineSpec::default(), true);
    rig.machine.link.connected().await;
    rig.controller.fail_next_commits(3);
    publish_titles(&rig, 0..60).await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&rig.name).len()
                >= 60)
            .await
    );
    assert_exactly_once_in_order(&rig, 60);
}

#[tokio::test]
async fn a_system_that_has_fallen_behind_is_sent_no_more_than_a_window() {
    let rig = rig(MachineSpec::default(), true);
    rig.machine.link.connected().await;
    rig.controller.set_commit_mode(CommitMode::Held);
    publish_titles(&rig, 0..2_500).await;
    tokio::time::sleep(Duration::from_millis(800)).await;
    let held = rig.controller.held_events();
    assert!(held > 0);
    assert!(
        held <= 1_000,
        "{held} events are on their way without an acknowledgement; the window is 1000"
    );
    rig.controller.set_commit_mode(CommitMode::Immediate);
    rig.controller.release_commits();
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&rig.name).len()
                >= 2_500)
            .await
    );
    assert_exactly_once_in_order(&rig, 2_500);
}

fn small_spool(max_bytes: u64) -> MachineSpec {
    MachineSpec {
        spool: SpoolOptions {
            max_bytes,
            segment_bytes: 2_048,
            max_age: Duration::from_secs(7 * 86_400),
        },
        ..MachineSpec::default()
    }
}

fn raw(n: usize) -> Event {
    Event::RawFrame(RawFrame {
        n: 0,
        direction: "in".to_owned(),
        payload: serde_json::json!({ "n": n, "text": "x".repeat(150) }),
    })
}

fn delta(item: &str, n: usize) -> Event {
    Event::ItemDelta(ItemDelta {
        native_ref: item.to_owned(),
        field: orch_link_contract::events::DeltaField::Text,
        offset: n as i64,
        len: 1,
        data: "x".to_owned(),
    })
}

fn completed(item: &str) -> Event {
    Event::ItemCompleted(ItemCompleted {
        native_ref: item.to_owned(),
        status: serde_json::from_value(serde_json::json!("completed")).unwrap(),
        payload: serde_json::from_value(
            serde_json::json!({ "text": "xxxxxxxxxxxxxxxxxxxx", "text_len": 20, "phase": null }),
        )
        .unwrap(),
        output_blob: None,
    })
}

#[tokio::test]
async fn a_full_spool_gives_up_diagnostics_and_then_the_text_a_completed_item_repeats_and_nothing_else()
 {
    let rig = rig(small_spool(7_000), false);
    // Offline: everything is spooled and nothing is sent.
    for n in 0..30 {
        rig.machine.publish(&rig.stream, &raw(n)).await;
    }
    for n in 0..40 {
        rig.machine.publish(&rig.stream, &delta("a", n)).await;
    }
    rig.machine.publish(&rig.stream, &completed("a")).await;
    publish_titles(&rig, 0..60).await;
    rig.hub.faults().refuse_connections(0);
    rig.machine.link.connected().await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| {
                c.applied(&rig.name)
                    .iter()
                    .filter(|e| e.kind == "session.title")
                    .count()
                    >= 60
            })
            .await
    );
    let applied = rig.controller.applied(&rig.name);
    let titles = applied.iter().filter(|e| e.kind == "session.title").count();
    assert_eq!(titles, 60, "no event that matters was given up");
    assert!(
        applied.iter().any(|e| e.kind == "item.completed"),
        "the completed item holds the whole text"
    );
    let deltas = applied.iter().filter(|e| e.kind == "item.delta").count();
    assert!(
        deltas < 40,
        "deltas of a completed item were left out: {deltas} of 40 arrived"
    );
    assert!(
        rig.controller.resets().is_empty(),
        "nothing was lost, so nobody is told: {:?}",
        rig.controller.resets()
    );
    assert!(rig.controller.violations().is_empty());
}

#[tokio::test]
async fn a_spool_that_must_give_up_its_oldest_events_says_so_with_a_reset_and_the_system_asks_for_the_session_again()
 {
    let rig = rig(small_spool(6_000), false);
    for n in 0..20 {
        rig.machine.publish(&rig.stream, &raw(n)).await;
    }
    publish_titles(&rig, 0..300).await;
    rig.hub.faults().refuse_connections(0);
    rig.machine.link.connected().await;
    assert!(
        rig.controller
            .wait_until(Duration::from_secs(30), |c| {
                c.applied(&rig.name)
                    .iter()
                    .any(|e| e.body["title"] == "title 0299")
            })
            .await,
        "the newest event must arrive; applied {} (first {:?}, last {:?}), resets {:?}, violations {:?}",
        rig.controller.applied(&rig.name).len(),
        rig.controller.applied(&rig.name).first().map(|e| e.seq),
        rig.controller.applied(&rig.name).last().map(|e| e.seq),
        rig.controller.resets(),
        rig.controller.violations()
    );
    let resets = rig.controller.resets();
    assert_eq!(resets.len(), 1, "{resets:?}");
    assert_eq!(resets[0].reason, ResetReason::SpoolOverflow);
    assert!(
        resets[0].resync_requested,
        "a session stream is repaired by asking for the session again"
    );
    let applied = rig.controller.applied(&rig.name);
    assert!(
        applied.iter().all(|e| e.kind == "session.title"),
        "diagnostics went first"
    );
    assert!(applied.len() < 300, "the oldest went");
    // The numbers continue from where the reset says the stream goes on.
    assert_eq!(applied[0].seq, resets[0].from);
    for pair in applied.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq + 1);
    }
    assert!(
        rig.controller.violations().is_empty(),
        "{:?}",
        rig.controller.violations()
    );
    // The machine was asked, once, for the session again.
    let asked = async {
        while !rig
            .machine
            .handler
            .executions()
            .iter()
            .any(|run| run.kind == CommandKind::SessionResync)
        {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), asked)
        .await
        .expect("the machine was never asked for the session");
    let resyncs = rig
        .machine
        .handler
        .executions()
        .iter()
        .filter(|run| run.kind == CommandKind::SessionResync)
        .count();
    assert_eq!(resyncs, 1);
    let _ = rig.thread_id;
}

#[tokio::test]
async fn events_that_were_spooled_before_a_stop_are_sent_when_the_machine_starts_again() {
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::with_limits(Arc::clone(&clock), pki.certificate(), Limits::default(), 1);
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(&hub, Arc::clone(&clock), id, vec![Role::Agent]);
    let dir = tempfile::tempdir().unwrap();
    let stream = Stream::Session {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    let name = stream.to_string();
    let spec = MachineSpec {
        roles: vec![Role::Agent],
        ..MachineSpec::default()
    };
    hub.faults().refuse_connections(1_000_000);
    {
        let machine = ScriptedMachine::start_in(&hub, &clock, &identity, &spec, dir.path());
        for n in 0..25 {
            machine.publish(&stream, &title(n)).await;
        }
        machine.link.shutdown().await;
    }
    hub.faults().refuse_connections(0);
    let machine = ScriptedMachine::start_in(&hub, &clock, &identity, &spec, dir.path());
    machine.link.connected().await;
    assert!(
        controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&name).len() >= 25)
            .await
    );
    let applied = controller.applied(&name);
    assert_eq!(
        applied.iter().map(|e| e.seq).collect::<Vec<_>>(),
        (1..=25).collect::<Vec<_>>()
    );
    // New events go on after the old ones.
    machine.publish(&stream, &title(25)).await;
    assert!(
        controller
            .wait_until(Duration::from_secs(30), |c| c.applied(&name).len() >= 26)
            .await
    );
    assert_eq!(controller.applied(&name)[25].seq, 26);
    assert!(controller.violations().is_empty());
}
