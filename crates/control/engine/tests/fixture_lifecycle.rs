//! Fixture shutdown runs while the current-thread runtime can still release its owned work.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use common::{PATIENCE, World, eventually};
use orch_engine::commands::{CommandsError, Outgoing};
use orch_link::commands::Progress;
use orch_link_contract::commands::{CommandKind, SerialKey, SessionClose, SessionResync};
use orch_types::new_id;

#[tokio::test(flavor = "current_thread")]
async fn successive_worlds_join_retired_appliers_and_release_all_engine_generations_before_removal()
{
    for _ in 0..3 {
        let world = World::new().await;
        let thread = world.thread("Owned fixture lifecycle").await;
        world
            .running_turn(thread, "One actual admitted input before shutdown")
            .await;
        world.restart_api().await;
        let application = world.pool.pool().clone();
        let database = world.db.pool.clone();
        tokio::time::timeout(PATIENCE, world.shutdown())
            .await
            .expect("fixture shutdown stays within the existing proof budget");
        assert!(application.is_closed());
        assert!(database.is_closed());
        assert_eq!(application.size(), 0);
        let issued = world.commands.kinds().len();
        let commands = world.commands.as_engine();
        let command = Outgoing::new(
            new_id(),
            world.machine_id,
            SerialKey::Thread(thread),
            &SessionClose { thread_id: thread },
        )
        .unwrap();
        assert!(matches!(
            commands.issue(command).await,
            Err(CommandsError::Stopping)
        ));
        assert_eq!(
            world.commands.kinds().len(),
            issued,
            "a retired fixture cannot send new work"
        );
        drop(commands);
        tokio::time::timeout(PATIENCE, world.shutdown())
            .await
            .expect("shutdown is idempotent");
        drop(world);
        assert!(
            matches!(application.acquire().await, Err(sqlx::Error::PoolClosed)),
            "the former application pool never reconnects to a removed database"
        );
    }
}

#[tokio::test(flavor = "current_thread")]
async fn a_held_observation_releases_its_process_owner_on_shutdown_without_waiting_out_its_command_deadline()
 {
    let world = World::new().await;
    let thread = world.thread("Held native observation").await;
    world.machine.handler.hold(CommandKind::SessionResync);
    let commands = world.commands.as_engine();
    let id = new_id();
    let command = Outgoing::new(
        id,
        world.machine_id,
        SerialKey::Thread(thread),
        &SessionResync { thread_id: thread },
    )
    .unwrap();
    let mut observing = tokio::spawn(async move { commands.observe(command).await });
    eventually(
        "the actual native observation is admitted before fixture shutdown",
        || {
            let admitted = world
                .machine
                .handler
                .executions()
                .iter()
                .any(|run| run.kind == CommandKind::SessionResync);
            async move { admitted }
        },
    )
    .await;
    // Keep one real engine-generation owner until the held native command has itself ended.
    // Stopping a fixture must release the observation waiter without cancelling/replaying that command.
    let generation = world.engine();
    let shutting = world.shutdown();
    tokio::pin!(shutting);
    tokio::time::timeout(PATIENCE, async {
        let result = tokio::select! {
            result = &mut observing => result.unwrap(),
            () = &mut shutting => panic!("shutdown cannot finish while an engine generation is retained"),
        };
        assert!(matches!(result, Err(CommandsError::Stopping)));
        assert!(!world.commands.native_progress(world.machine_id, id).unwrap().is_final(),
            "the native hold was not lifted to release the observation waiter");
        world.machine.handler.release();
        eventually("the original held native command is durably answered before link shutdown", || {
            let complete = matches!(world.commands.native_progress(world.machine_id, id), Some(Progress::Succeeded(_)));
            async move { complete }
        }).await;
        drop(generation);
        shutting.await;
    }).await.expect("the held process releases ownership within the unchanged proof budget");
    assert_eq!(
        world.machine.handler.runs_of(id),
        1,
        "shutdown neither replays nor cancels native work"
    );
    assert!(world.pool.pool().is_closed());
    assert!(world.db.pool.is_closed());
    assert_eq!(world.commands.issued_of("turn.interrupt"), 0);
}
