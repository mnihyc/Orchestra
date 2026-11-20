//! Temporary provider preparation failures keep the original input and recover without another Send.
use super::*;
use orch_link_contract::commands::{CommandError, ErrorCode};
use orch_types::Clock;
use std::sync::Arc;

async fn observe_states(world: &World) {
    sqlx::raw_sql("CREATE TABLE observed_thread_states(thread_id uuid,state text,reason text,detail text);
        CREATE FUNCTION remember_thread_state() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
          INSERT INTO observed_thread_states VALUES(NEW.node_id,NEW.state,NEW.blocked_reason,NEW.blocked_detail);
          RETURN NEW;
        END $$;
        CREATE TRIGGER remember_thread_state AFTER UPDATE OF state ON threads
          FOR EACH ROW WHEN(OLD.state IS DISTINCT FROM NEW.state) EXECUTE FUNCTION remember_thread_state();")
        .execute(world.pool.pool()).await.unwrap();
}

fn busy() -> CommandError {
    CommandError::new(
        ErrorCode::Timeout,
        "the scripted keeper is temporarily busy",
    )
    .retryable()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_retryable_provider_preparation_is_retried_before_the_thread_is_blocked() {
    let world = World::on_a_key("scripted-recovery-key-not-a-model-credential").await;
    let thread = world.thread("A transient keeper failure").await;
    observe_states(&world).await;
    world.refuse(CommandKind::LeaseOpen, vec![busy()]);
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep my original words"),
        )
        .await
        .unwrap();
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.wait_state(thread, "running").await;
    let blocked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM observed_thread_states WHERE thread_id=$1 AND state='blocked'",
    )
    .bind(thread)
    .fetch_one(world.pool.pool())
    .await
    .unwrap();
    let delivered = world.input_state(input.input.id).await;
    let tries = world.commands.issued_of("lease.open");
    world.shutdown().await;
    assert!(
        tries >= 2,
        "the system must actually retry the native preparation"
    );
    assert_eq!(
        blocked, 0,
        "a transient preparation gets its fast retry before a blocking state is published"
    );
    assert_eq!(delivered.0, "delivered");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_recovered_keeper_resumes_the_original_input_after_restart_without_another_send() {
    recovered_keeper(CommandKind::LeaseOpen).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_keeper_bootstrap_recovers_without_replaying_its_failed_receipts_forever() {
    recovered_keeper(CommandKind::ProviderStart).await;
}

async fn recovered_keeper(kind: CommandKind) {
    let clock = Arc::new(orch_testkit::clock::manual());
    let world = World::from_setup(common::Setup {
        clock: Some(clock.clone()),
        key: Some("scripted-recovery-key-not-a-model-credential"),
        observed: true,
        ..common::Setup::default()
    })
    .await;
    let thread = world.thread("Recover my pending input").await;
    observe_states(&world).await;
    world.refuse(kind, vec![busy(), busy()]);
    let input = world
        .engine()
        .submit_input(
            &Caller::session(world.ada),
            thread,
            queued("Keep the same input"),
        )
        .await
        .unwrap();
    eventually("both fast native attempts have failed", || {
        let tried = world.commands.issued_of(kind.as_str()) >= 2;
        async move { tried }
    })
    .await;
    world.wait_blocked(thread, "provider_unavailable").await;
    assert_eq!(
        world.input_state(input.input.id).await.0,
        "pending",
        "a retryable provider failure must not exhaust the original input"
    );
    let retry: (i64, chrono::DateTime<chrono::Utc>) =
        sqlx::query_as("SELECT attempts,retry_at FROM thread_start_retries WHERE input_id=$1")
            .bind(input.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(retry.0, 2);
    assert!(retry.1 > clock.now());
    let detail: Option<String> =
        sqlx::query_scalar("SELECT blocked_detail FROM threads WHERE node_id=$1")
            .bind(thread)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert!(
        detail
            .as_deref()
            .unwrap_or_default()
            .contains("automatically"),
        "the state line must explain automatic recovery: {detail:?}"
    );
    world.restart_api().await;
    let still: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("SELECT retry_at FROM thread_start_retries WHERE input_id=$1")
            .bind(input.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    assert_eq!(
        still, retry.1,
        "restarting must not reset the retained retry schedule"
    );
    world.engine().preflight(thread).await.unwrap();
    assert_eq!(
        world.commands.issued_of(kind.as_str()),
        2,
        "the retained backoff must be honoured"
    );
    clock.advance(retry.1 - clock.now());
    world.engine().kick();
    let start = world.turn_start(thread, 0).await;
    world.say(thread, common::turn_began(start.turn_id)).await;
    world.wait_state(thread, "running").await;
    let delivered = world.input_state(input.input.id).await;
    let left: i64 =
        sqlx::query_scalar("SELECT count(*) FROM thread_start_retries WHERE input_id=$1")
            .bind(input.input.id)
            .fetch_one(world.pool.pool())
            .await
            .unwrap();
    let actual = world.commands.issued_of(kind.as_str());
    world.shutdown().await;
    assert_eq!(
        actual, 3,
        "only the due system-owned retry may open another lease"
    );
    assert_eq!(delivered.0, "delivered");
    assert_eq!(
        left, 0,
        "actual native start clears the retained preparation retry"
    );
}
