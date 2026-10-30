//! The thread engine's commands go through the link's dispatcher and come back in the engine's words.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_api_core::backplane::Link;
use orch_api_core::thread_engine::{DispatchCommands, start};
use orch_config::ServerConfig;
use orch_db::Db;
use orch_db::queries::resources::testing::insert_machine;
use orch_engine::commands::{Commands, CommandsError, Outcome, Outgoing, Stage};
use orch_engine::views::ReadError;
use orch_link_contract::commands::{SerialKey, SessionClose};
use orch_testkit::clock;
use orch_testkit::database::TestDb;
use orch_types::{Clock, Id, ManualClock, new_id};
use std::sync::Arc;

struct Fixture {
    _test: TestDb,
    db: Db,
    handle: Arc<dyn Clock>,
    config: ServerConfig,
    link: Link,
    machine: Id,
    commands: DispatchCommands,
}

async fn fixture() -> Fixture {
    let test = TestDb::fresh().await;
    let db = Db::from_pool(test.pool.clone());
    let handle: Arc<dyn Clock> = Arc::new(ManualClock::new(clock::start()));
    let config = ServerConfig::default();
    let link = Link::detached(db.clone(), Arc::clone(&handle), Arc::new(config.clone())).unwrap();
    let machine = {
        let mut conn = db.acquire().await.unwrap();
        insert_machine(&mut conn, "m", "connected").await.unwrap()
    };
    let commands = DispatchCommands::new(link.dispatcher());
    Fixture {
        _test: test,
        db,
        handle,
        config,
        link,
        machine,
        commands,
    }
}

fn close(machine: Id) -> Outgoing {
    let thread = new_id();
    Outgoing::new(
        new_id(),
        machine,
        SerialKey::Thread(thread),
        &SessionClose { thread_id: thread },
    )
    .unwrap()
}

#[tokio::test]
async fn a_command_stands_queued_until_a_machine_takes_it_and_asking_twice_makes_one() {
    let fixture = fixture().await;
    let command = close(fixture.machine);
    let id = command.id;
    fixture.commands.issue(command.clone()).await.unwrap();
    fixture.commands.issue(command).await.unwrap();

    let info = fixture.commands.status(id).await.unwrap().unwrap();
    assert_eq!(info.stage, Stage::Queued, "no machine has a session here");
    assert_eq!(info.issued_at, fixture.handle.now());
    assert!(info.outcome.is_none());
    let mut conn = fixture.db.acquire().await.unwrap();
    let rows: i64 = sqlx::query_scalar("SELECT count(*) FROM commands WHERE id = $1")
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .unwrap();
    assert_eq!(
        rows, 1,
        "a command asked for twice under one identifier is made once"
    );
}

#[tokio::test]
async fn a_command_that_is_cancelled_ends_as_cancelled_and_waiting_for_it_says_so() {
    let fixture = fixture().await;
    let command = close(fixture.machine);
    let id = command.id;
    fixture.commands.issue(command).await.unwrap();

    assert!(
        fixture.commands.cancel(id).await.unwrap(),
        "it was not over"
    );
    assert_eq!(fixture.commands.wait(id).await.unwrap(), Outcome::Cancelled);
    let info = fixture.commands.status(id).await.unwrap().unwrap();
    assert_eq!(
        (info.stage, info.outcome),
        (Stage::Over, Some(Outcome::Cancelled))
    );
    assert!(!fixture.commands.cancel(id).await.unwrap(), "and now it is");
}

#[tokio::test]
async fn a_command_nobody_knows_is_nothing_and_cancelling_it_cancels_nothing() {
    let fixture = fixture().await;
    let unknown = new_id();
    assert!(fixture.commands.status(unknown).await.unwrap().is_none());
    assert!(!fixture.commands.cancel(unknown).await.unwrap());
    assert!(fixture.commands.wait(unknown).await.is_err());
}

#[tokio::test]
async fn a_command_for_a_machine_the_system_does_not_know_is_refused() {
    let fixture = fixture().await;
    let refused = fixture.commands.issue(close(new_id())).await;
    assert!(
        matches!(refused, Err(CommandsError::UnknownMachine)),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_engine_of_the_process_is_built_over_the_link_and_answers_requests() {
    let fixture = fixture().await;
    let engine = start(&fixture.db, &fixture.handle, &fixture.config, &fixture.link);
    let missing = engine.sync(new_id(), None, None, 10).await;
    assert!(matches!(missing, Err(ReadError::NotFound)), "{missing:?}");
    engine.stop();
}
