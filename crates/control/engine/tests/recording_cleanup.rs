//! A committed registration withdrawal retries Busy durably without creating a retry for an active place.
mod common;
mod hosting;

use common::{PATIENCE, World, eventually};
use orch_db::queries::{environments, lifecycle, link};
use orch_link_contract::commands::{CommandKind, EnvDetach};
use orch_types::{Clock, Id, new_id};
use serde_json::json;
use std::sync::Arc;

type TestResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;

async fn command_reaches(world: &World, id: Id, state: &str) -> TestResult {
    tokio::time::timeout(PATIENCE, async {
        loop {
            let reached = {
                let mut conn = world.pool.acquire().await?;
                link::command(&mut conn, id)
                    .await?
                    .is_some_and(|row| row.state == state)
            };
            if reached {
                return Ok::<(), orch_db::DbError>(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await??;
    Ok(())
}

#[tokio::test]
async fn a_withdrawn_registration_retries_its_committed_busy_detach_and_checkpoints_the_next_command()
-> TestResult {
    use orch_link_contract::commands::{CommandError, ErrorCode};
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let connector = hosting::primary(&world);
    let environment =
        hosting::directory(&world, connector.id, "Withdrawn registration", "/workspace").await;
    connector.refuse_detaches(
        1,
        CommandError::new(ErrorCode::Busy, "recording publication owns the store").retryable(),
    );
    let original = new_id();
    let at = clock.now();
    let deadline = at + chrono::Duration::seconds(30);
    let body = serde_json::to_value(EnvDetach {
        environment_id: environment,
    })?;
    let mut tx = world.pool.begin().await?;
    environments::routes::remove_registration(&mut tx, environment, at).await?;
    link::insert_command(
        &mut tx,
        &link::NewCommand {
            id: original,
            machine_id: connector.id,
            kind: "env.detach",
            body: &body,
            serial_key: Some(&environment.to_string()),
            deadline_at: deadline,
        },
        at,
    )
    .await?;
    tx.commit().await?;
    world.engine().kick();
    eventually("the committed detach reaches the machine", || async {
        connector.count(CommandKind::EnvDetach) == 1
    })
    .await;
    command_reaches(&world, original, "failed").await?;
    clock.advance(chrono::Duration::seconds(1));
    world.engine().kick();
    eventually("the withdrawal retries and succeeds", || async {
        connector.count(CommandKind::EnvDetach) == 2
    })
    .await;
    let (child, next) = {
        let mut conn = world.pool.acquire().await?;
        let parent = link::command(&mut conn, original)
            .await?
            .ok_or("original command missing")?;
        let child: Id = parent
            .error
            .as_ref()
            .and_then(|error| error.pointer("/details/retry_command"))
            .and_then(serde_json::Value::as_str)
            .ok_or("retry checkpoint missing")?
            .parse()?;
        let next = link::command(&mut conn, child)
            .await?
            .ok_or("retry command missing")?;
        (child, next)
    };
    command_reaches(&world, child, "succeeded").await?;
    assert_eq!(next.body, body);
    assert_eq!(next.machine_id, connector.id);
    assert_eq!(next.serial_key, Some(environment.to_string()));
    assert_eq!(next.deadline_at, deadline);
    assert_eq!(
        child,
        orch_engine::ids::derived("command.busy-retry", &[original.as_bytes()])
    );
    Ok(())
}

#[tokio::test]
async fn active_or_expired_registrations_cannot_be_claimed_for_background_busy_retry() -> TestResult
{
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::with_clock(clock.clone()).await;
    let connector = hosting::primary(&world);
    let environment =
        hosting::directory(&world, connector.id, "Active registration", "/workspace").await;
    let original = new_id();
    let at = clock.now();
    let deadline = at + chrono::Duration::seconds(30);
    let body = json!({"environment_id":environment});
    let error = json!({"code":"busy","detail":"publication owns the store","retryable":true});
    let mut conn = world.pool.acquire().await?;
    link::insert_command(
        &mut conn,
        &link::NewCommand {
            id: original,
            machine_id: connector.id,
            kind: "env.detach",
            body: &body,
            serial_key: Some(&environment.to_string()),
            deadline_at: deadline,
        },
        at,
    )
    .await?;
    link::finish_command(&mut conn, original, &link::Ending::Failed(&error), at).await?;
    assert!(
        lifecycle::busy_registration_detaches(
            &mut conn,
            at + chrono::Duration::seconds(1),
            at,
            100
        )
        .await?
        .is_empty()
    );
    assert!(!lifecycle::claim_busy_registration_detach(&mut conn, original, new_id(), at).await?);
    environments::routes::remove_registration(&mut conn, environment, at).await?;
    assert!(
        lifecycle::busy_registration_detaches(
            &mut conn,
            deadline + chrono::Duration::seconds(1),
            deadline + chrono::Duration::seconds(1),
            100
        )
        .await?
        .is_empty()
    );
    assert!(
        !lifecycle::claim_busy_registration_detach(&mut conn, original, new_id(), deadline).await?
    );
    Ok(())
}
