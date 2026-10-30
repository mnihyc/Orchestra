//! Commands are rows first: they wait for their machine, survive a restart of the system, run once, and carry no secret.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_api_core::dispatch::{CommandOutcome, DispatchError};
use orch_db::queries::settings::put_secret;
use orch_db::secrets::MasterKey;
use orch_link_contract::commands::{
    Command, CommandError, CommandKind, CommandResult, Empty, ErrorCode, ProviderStart, Secret,
    SerialKey, SessionClose,
};
use orch_link_contract::enrollment::Trust;
use orch_link_contract::session::Role;
use orch_pki::sealing::{info_for_command, open};
use orch_types::{Time, new_id};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn later(seconds: i64) -> Time {
    Time::new(chrono::Utc::now() + chrono::Duration::seconds(seconds))
}

async fn state_of(world: &World, command: orch_types::Id) -> String {
    world
        .api
        .link
        .dispatcher()
        .status(command)
        .await
        .unwrap()
        .unwrap()
        .state
}

#[tokio::test]
async fn a_command_given_while_its_machine_is_away_is_delivered_when_it_returns_and_runs_once() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let id = enrolled.identity.machine_id;
    let (thread, command) = (new_id(), new_id());
    world
        .api
        .link
        .dispatcher()
        .issue_as(
            command,
            id,
            &SerialKey::Thread(thread),
            later(300),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    assert_eq!(
        state_of(&world, command).await,
        "queued",
        "a row, with nobody to send it to"
    );

    // The system restarts while the command waits: it was a row, not a memory.
    world.restart_api().await;
    assert_eq!(state_of(&world, command).await, "queued");

    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    let outcome = tokio::time::timeout(PATIENCE, world.api.link.dispatcher().wait(command))
        .await
        .expect("the command to be over")
        .unwrap();
    assert_eq!(outcome, CommandOutcome::Succeeded(serde_json::json!({})));
    assert_eq!(machine.handler.runs_of(command), 1, "it ran once");

    let row = world
        .api
        .link
        .dispatcher()
        .status(command)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.state, "succeeded");
    assert_eq!(
        row.result,
        Some(serde_json::json!({})),
        "its result is stored"
    );
    assert!(row.attempts >= 1, "it was handed over at least once");
    world.stop().await;
}

#[tokio::test]
async fn a_command_the_machine_has_taken_is_not_run_again_when_the_system_restarts_meanwhile() {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    machine.handler.hold(CommandKind::SessionClose);

    let (thread, command) = (new_id(), new_id());
    world
        .api
        .link
        .dispatcher()
        .issue_as(
            command,
            machine.id,
            &SerialKey::Thread(thread),
            later(300),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    tokio::time::timeout(PATIENCE, machine.handler.started(command))
        .await
        .unwrap();
    eventually("the command to be written as taken", || {
        let state = state_of(&world, command);
        async move { state.await == "accepted" }
    })
    .await;

    world.restart_api().await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    machine.handler.release();

    let outcome = tokio::time::timeout(PATIENCE, world.api.link.dispatcher().wait(command))
        .await
        .expect("the command to be over")
        .unwrap();
    assert_eq!(outcome, CommandOutcome::Succeeded(serde_json::json!({})));
    assert_eq!(
        machine.handler.runs_of(command),
        1,
        "it ran once, however often it was sent"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_secret_is_stored_as_a_reference_and_reaches_only_the_machine_it_is_for() {
    let mut world = World::start().await;
    let enrolled = world.enroll("provider-box").await;
    let other = world.enroll("somebody-else").await;

    // A provider's key, stored sealed under the system's key, as the settings store them.
    let provider = new_id();
    let binding = format!("secrets/provider/{provider}/api_key").into_bytes();
    let sealed = MasterKey::new(world.master)
        .seal(b"sk-live-123", &binding)
        .unwrap();
    let secret = {
        let mut conn = world.db.acquire().await.unwrap();
        put_secret(
            &mut conn,
            "provider",
            provider,
            "api_key",
            &sealed,
            None,
            chrono::Utc::now(),
        )
        .await
        .unwrap()
    };

    let machine = world.start_machine_as(
        &enrolled,
        enrolled.endpoints(Trust::Pinned),
        vec![Role::Provider],
    );
    let received: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&received);
    machine
        .handler
        .on(CommandKind::ProviderStart, move |command| {
            if let Command::ProviderStart(body) = command
                && let Some(Secret::Sealed(text)) = &body.credential
            {
                *sink.lock().unwrap() = Some(text.clone());
            }
            Ok(CommandResult::ProviderStart(Empty {}))
        });
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();

    let command = new_id();
    world
        .api
        .link
        .dispatcher()
        .issue_as(
            command,
            machine.id,
            &SerialKey::Provider(provider),
            later(300),
            &ProviderStart {
                provider_id: provider,
                protocols: Vec::new(),
                upstreams: Vec::new(),
                credential: Some(Secret::Ref(secret.id)),
                secrets: BTreeMap::new(),
                presenter: None,
            },
        )
        .await
        .unwrap();
    let stored = world
        .api
        .link
        .dispatcher()
        .status(command)
        .await
        .unwrap()
        .unwrap();
    let text = stored.body.to_string();
    assert!(
        text.contains(&secret.id.to_string()),
        "the row refers to the secret: {text}"
    );
    assert!(!text.contains("sk-live"), "and holds none of it: {text}");

    let outcome = tokio::time::timeout(PATIENCE, world.api.link.dispatcher().wait(command))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, CommandOutcome::Succeeded(serde_json::json!({})));

    let sealed_for_it = received
        .lock()
        .unwrap()
        .clone()
        .expect("the machine was sent the value, sealed");
    assert!(!sealed_for_it.contains("sk-live"));
    let info = info_for_command(command);
    let opened = open(&enrolled.identity.key, &info, &sealed_for_it).unwrap();
    assert_eq!(
        opened, b"sk-live-123",
        "the machine it was sealed to reads it"
    );
    assert!(
        open(&other.identity.key, &info, &sealed_for_it).is_err(),
        "no other machine can"
    );
    assert!(
        open(
            &enrolled.identity.key,
            &info_for_command(new_id()),
            &sealed_for_it
        )
        .is_err(),
        "and it opens for no other command"
    );
    world.stop().await;
}

#[tokio::test]
async fn a_command_no_machine_took_before_its_deadline_expires_and_one_that_was_cancelled_never_runs()
 {
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let dispatcher = world.api.link.dispatcher();
    let thread = new_id();
    let (late, cancelled) = (new_id(), new_id());
    dispatcher
        .issue_as(
            late,
            enrolled.identity.machine_id,
            &SerialKey::Thread(thread),
            later(1),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    dispatcher
        .issue_as(
            cancelled,
            enrolled.identity.machine_id,
            &SerialKey::Thread(thread),
            later(300),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    assert!(dispatcher.cancel(cancelled).await.unwrap());
    assert!(!dispatcher.cancel(cancelled).await.unwrap(), "only once");

    let outcome = tokio::time::timeout(PATIENCE, dispatcher.wait(late))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(outcome, CommandOutcome::Expired);
    assert_eq!(
        dispatcher.wait(cancelled).await.unwrap(),
        CommandOutcome::Cancelled
    );

    // The machine comes back, and neither is run.
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    assert_eq!(machine.handler.runs_of(late), 0);
    assert_eq!(machine.handler.runs_of(cancelled), 0);
    world.stop().await;
}

#[tokio::test]
async fn a_machine_that_fails_a_command_has_its_reason_stored_and_a_stranger_is_not_given_commands()
{
    let mut world = World::start().await;
    let enrolled = world.enroll("alice-laptop").await;
    let machine = world.start_machine(&enrolled, enrolled.endpoints(Trust::Pinned));
    machine.handler.on(CommandKind::SessionClose, |_| {
        Err(CommandError::new(ErrorCode::SessionNotFound, "the harness lost it").retryable())
    });
    tokio::time::timeout(PATIENCE, machine.link.connected())
        .await
        .unwrap();

    let thread = new_id();
    let dispatcher = world.api.link.dispatcher();
    let issued = dispatcher
        .issue(
            machine.id,
            &SerialKey::Thread(thread),
            &SessionClose { thread_id: thread },
        )
        .await
        .unwrap();
    let outcome = tokio::time::timeout(PATIENCE, dispatcher.wait(issued.id))
        .await
        .unwrap()
        .unwrap();
    let CommandOutcome::Failed(error) = outcome else {
        panic!("the command should have failed");
    };
    assert_eq!(error.code, "session_not_found");
    assert!(error.retryable);
    let row = dispatcher.status(issued.id).await.unwrap().unwrap();
    assert_eq!(row.state, "failed");
    assert_eq!(row.error.unwrap()["detail"], "the harness lost it");

    let nobody = dispatcher
        .issue(
            new_id(),
            &SerialKey::Thread(thread),
            &SessionClose { thread_id: thread },
        )
        .await;
    assert!(matches!(nobody, Err(DispatchError::UnknownMachine)));
    world.stop().await;
}
