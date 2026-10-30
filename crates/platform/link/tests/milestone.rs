//! A machine and a controller joined by the in-memory hub: the first thing the link has to get right.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::TimeZone;
use orch_link_contract::commands::{SerialKey, SessionClose};
use orch_link_contract::events::{Event, SessionTitle};
use orch_link_contract::session::Role;
use orch_link_contract::streams::Stream;
use orch_testkit::doubles::controller::ScriptedController;
use orch_testkit::doubles::link::{Hub, TestPki};
use orch_testkit::doubles::machine::{MachineSpec, ScriptedMachine};
use orch_types::{Clock, ManualClock, new_id};
use std::sync::Arc;
use std::time::Duration;

fn clock() -> Arc<dyn Clock> {
    Arc::new(ManualClock::new(
        chrono::Utc
            .with_ymd_and_hms(2026, 10, 8, 14, 0, 0)
            .single()
            .unwrap(),
    ))
}

#[tokio::test]
async fn a_thousand_events_published_while_the_connection_is_cut_three_times_arrive_exactly_once_and_in_order()
 {
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(
        &hub,
        Arc::clone(&clock),
        id,
        vec![Role::Agent, Role::Environment],
    );
    let machine = ScriptedMachine::start(&hub, &clock, &identity, &MachineSpec::default());
    machine.link.connected().await;

    let stream = Stream::Session {
        thread_id: new_id(),
        segment_id: new_id(),
    };
    for i in 0..1_000 {
        machine
            .publish(
                &stream,
                &Event::SessionTitle(SessionTitle {
                    title: format!("title {i}"),
                }),
            )
            .await;
        if matches!(i, 250 | 500 | 750) {
            hub.faults().sever_now();
        }
        if i % 50 == 49 {
            tokio::task::yield_now().await;
        }
    }
    let name = stream.to_string();
    assert!(
        controller
            .wait_until(Duration::from_secs(60), |c| c.applied(&name).len() >= 1_000)
            .await,
        "only {} of 1000 events arrived",
        controller.applied(&name).len()
    );
    let applied = controller.applied(&name);
    assert_eq!(applied.len(), 1_000, "no event twice");
    for (place, event) in applied.iter().enumerate() {
        assert_eq!(event.seq, place as i64 + 1, "in order, without a hole");
        assert_eq!(event.body["title"], format!("title {place}"));
    }
    assert!(
        controller.violations().is_empty(),
        "{:?}",
        controller.violations()
    );
    assert!(
        hub.faults().connections() >= 4,
        "the machine had to come back three times"
    );
}

#[tokio::test]
async fn a_command_issued_twice_with_one_identifier_runs_once_and_is_answered_twice() {
    let clock = clock();
    let pki = TestPki::new(Arc::clone(&clock));
    let hub = Hub::new(Arc::clone(&clock), pki.certificate());
    let id = new_id();
    let identity = pki.machine(id);
    let controller = ScriptedController::attach(&hub, Arc::clone(&clock), id, vec![Role::Agent]);
    let machine = ScriptedMachine::start(&hub, &clock, &identity, &MachineSpec::default());
    machine.link.connected().await;

    let thread = new_id();
    let command = new_id();
    let deadline = orch_types::Time::new(clock.now() + chrono::Duration::minutes(5));
    let body = SessionClose { thread_id: thread };
    controller.issue_with_id(command, &SerialKey::Thread(thread), deadline, &body);
    let first = tokio::time::timeout(Duration::from_secs(10), controller.wait_command(command))
        .await
        .unwrap();
    assert!(
        matches!(first, orch_link::commands::Progress::Succeeded(_)),
        "{first:?}"
    );

    controller.issue_with_id(command, &SerialKey::Thread(thread), deadline, &body);
    let second = tokio::time::timeout(Duration::from_secs(10), controller.wait_command(command))
        .await
        .unwrap();
    assert_eq!(first, second, "the same result comes back");
    assert_eq!(
        machine.handler.runs_of(command),
        1,
        "and the command ran once"
    );
}
