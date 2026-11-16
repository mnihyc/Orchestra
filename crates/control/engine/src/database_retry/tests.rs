#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a regression case fails by panicking"
)]
use super::*;
use crate::test_common::{World, eventually, queued};
use orch_authz::Caller;
use serde_json::json;

#[test]
fn a_deterministic_refusal_cannot_be_retried_by_a_timer_or_another_caller() {
    let mut episode = Policy {
        attempts: 2,
        initial: Duration::ZERO,
        recovery: Duration::ZERO,
    }
    .episode();
    assert!(episode.ready());
    episode.reject();
    for _ in 0..10 {
        episode.failed();
        assert!(!episode.ready());
    }
    episode.recovered();
    assert!(episode.ready());
}

#[test]
fn transient_failures_have_a_bounded_burst_and_only_counted_interval_reports() {
    let policy = Policy {
        attempts: 2,
        initial: Duration::from_millis(10),
        recovery: Duration::from_secs(60),
    };
    let mut episode = policy.episode();
    let now = Instant::now();
    assert_eq!(episode.failed_at(now), (Duration::from_millis(10), Some(1)));
    assert_eq!(episode.failed_at(now), (Duration::from_millis(20), None));
    assert_eq!(episode.failed_at(now), (Duration::from_secs(60), None));
    assert_eq!(
        episode.failed_at(now + Duration::from_secs(60)),
        (Duration::from_millis(10), Some(3))
    );
    episode.recovered();
    assert_eq!(episode.failed_at(now + Duration::from_secs(60)).1, Some(1));
    let mut no_repeat = Policy {
        attempts: 0,
        ..policy
    }
    .episode();
    assert_eq!(no_repeat.failed_at(now).0, Duration::from_secs(60));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_to_retry_thread_work_releases_its_thread_row_and_does_not_repeat_immediately() {
    let world = World::new().await;
    world
        .set_system("database.retry_initial_ms", json!(30_000))
        .await;
    let thread = world.thread("Recovery without a held row").await;
    sqlx::query("UPDATE provider_models SET enabled=false WHERE provider_id=$1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world
        .engine()
        .submit_input(&Caller::session(world.ada), thread, queued("Begin"))
        .await
        .unwrap();
    world.wait_blocked(thread, "model_not_allowed").await;
    sqlx::raw_sql("CREATE SEQUENCE failed_turn_writes;
        CREATE FUNCTION delay_turn_write() RETURNS trigger LANGUAGE plpgsql AS $$ BEGIN
          PERFORM nextval('failed_turn_writes');
          RAISE EXCEPTION 'synthetic transient write failure' USING ERRCODE='40001';
        END $$;
        CREATE TRIGGER delay_turn_write BEFORE INSERT ON turns FOR EACH ROW EXECUTE FUNCTION delay_turn_write();")
        .execute(world.pool.pool()).await.unwrap();
    sqlx::query("UPDATE provider_models SET enabled=true WHERE provider_id=$1")
        .bind(world.provider)
        .execute(world.pool.pool())
        .await
        .unwrap();
    world.engine().wake(thread);
    eventually("the first turn write to fail", || async {
        sqlx::query_scalar::<_, bool>("SELECT is_called FROM failed_turn_writes")
            .fetch_one(world.pool.pool())
            .await
            .unwrap()
    })
    .await;
    let mut tx = world.pool.begin().await.unwrap();
    // This operation has its own short deadline, far inside the configured recovery wait.
    tokio::time::timeout(
        Duration::from_secs(3),
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR UPDATE")
            .bind(thread)
            .execute(&mut *tx),
    )
    .await
    .expect("a waiting retry releases the row")
    .unwrap();
    tx.rollback().await.unwrap();
    let attempts: i64 = sqlx::query_scalar("SELECT last_value FROM failed_turn_writes")
        .fetch_one(world.pool.pool())
        .await
        .unwrap();
    assert_eq!(
        attempts, 1,
        "a sweep or a wake does not bypass the retry pause"
    );
    world.shutdown().await;
}
