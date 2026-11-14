//! Slow database work reaches the operator's counters and logs without exposing bound values.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_db::Db;
use orch_telemetry::{
    Readings,
    database::{SLOW_ACQUIRES, SLOW_STATEMENTS},
};
use orch_testkit::database::TestDb;
use std::{
    future::Future as _,
    io::Write,
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};
use tracing::instrument::WithSubscriber as _;
use tracing_subscriber::{Layer as _, layer::SubscriberExt};

// PostgreSQL enforces the minimum delay; there is no assertion that a loaded runner is fast.
const WARNING_AFTER: Duration = Duration::from_millis(10);
const DATABASE_DELAY_SECS: f64 = 0.025;
const BOUND_TEXT: &str = "database-watch-private-bound-text";

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl Write for Captured {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

fn count(readings: &Readings, name: &str) -> f64 {
    readings
        .render()
        .lines()
        .find_map(|line| {
            let (metric, value) = line.split_once(' ')?;
            (metric == name).then(|| value.parse::<f64>().unwrap())
        })
        .expect("the zero-valued database watch is exported before its first slow operation")
}

#[tokio::test]
async fn a_slow_statement_and_pool_wait_are_counted_without_logging_the_bound_value() {
    let test = TestDb::fresh().await;
    let readings = orch_telemetry::install_metrics().unwrap();
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::registry()
        .with(orch_telemetry::database::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(tracing_subscriber::EnvFilter::new("warn")),
        );
    let db = async {
        let db = Db::connect_with_slow_thresholds(&test.url, 1, WARNING_AFTER, WARNING_AFTER)
            .await
            .unwrap();
        let statements = count(&readings, SLOW_STATEMENTS);
        let acquires = count(&readings, SLOW_ACQUIRES);
        {
            let mut held = db.acquire().await.unwrap();
            let waiting = db.acquire();
            tokio::pin!(waiting);
            std::future::poll_fn(|context| {
                assert!(
                    matches!(waiting.as_mut().poll(context), Poll::Pending),
                    "the sole connection is held"
                );
                Poll::Ready(())
            })
            .await;
            sqlx::query("SELECT pg_sleep($1), $2::text")
                .bind(DATABASE_DELAY_SECS)
                .bind(BOUND_TEXT)
                .execute(&mut *held)
                .await
                .unwrap();
            drop(held);
            drop(waiting.await.unwrap());
        }
        let seen = || String::from_utf8_lossy(&captured.0.lock().unwrap()).into_owned();
        assert!(
            count(&readings, SLOW_STATEMENTS) > statements,
            "{}\n{}",
            readings.render(),
            seen()
        );
        assert!(
            count(&readings, SLOW_ACQUIRES) > acquires,
            "{}\n{}",
            readings.render(),
            seen()
        );
        db
    }
    .with_subscriber(subscriber)
    .await;
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        logged.contains("pg_sleep($1)") && logged.contains("$2::text"),
        "{logged}"
    );
    assert!(logged.contains("sqlx::pool::acquire"), "{logged}");
    assert!(!logged.contains(BOUND_TEXT));
    assert!(!readings.render().contains(BOUND_TEXT));

    // Suppressing printed warnings does not turn off the operator's counters.
    let statements = count(&readings, SLOW_STATEMENTS);
    let subscriber = tracing_subscriber::registry()
        .with(orch_telemetry::database::layer())
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(std::io::sink)
                .with_filter(tracing_subscriber::EnvFilter::new("error")),
        );
    async {
        sqlx::query("SELECT pg_sleep($1)")
            .bind(DATABASE_DELAY_SECS)
            .execute(db.pool())
            .await
            .unwrap();
    }
    .with_subscriber(subscriber)
    .await;
    assert!(count(&readings, SLOW_STATEMENTS) > statements);
    db.pool().close().await;
}

#[tokio::test]
async fn a_rejected_deferred_check_rolls_back_once_and_leaves_the_connection_ready() {
    let test = TestDb::fresh().await;
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let db = Db::connect_with_slow_thresholds(&test.url, 1, WARNING_AFTER, WARNING_AFTER)
            .await.unwrap();
        sqlx::raw_sql("CREATE TABLE commit_notice_case (id int PRIMARY KEY);
            CREATE FUNCTION refuse_commit_notice_case() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN RAISE EXCEPTION 'the deferred check refused the change'
                USING ERRCODE='40001', CONSTRAINT='notice_case_refusal'; END $$;
            CREATE CONSTRAINT TRIGGER refuse_commit_notice_case AFTER INSERT ON commit_notice_case
                DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION refuse_commit_notice_case();")
            .execute(db.pool()).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        sqlx::query("INSERT INTO commit_notice_case VALUES (1)")
            .execute(&mut *tx).await.unwrap();
        let error = tx.commit().await.unwrap_err();
        assert_eq!(error.as_database_error().and_then(|error| error.code()).as_deref(), Some("40001"));
        let mut tx = db.begin().await.unwrap();
        let count: i64 = sqlx::query_scalar("SELECT count(*) FROM commit_notice_case")
            .fetch_one(&mut *tx).await.unwrap();
        assert_eq!(count, 0, "the refused change was rolled back");
        tx.commit().await.unwrap();
        db.pool().close().await;
    }.with_subscriber(subscriber).await;
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        !logged.contains("there is no transaction in progress"),
        "{logged}"
    );
    assert!(
        !logged.contains("there is already a transaction in progress"),
        "{logged}"
    );
}

#[tokio::test]
async fn dropping_a_commit_waiter_keeps_one_owner_until_the_database_finishes() {
    let test = TestDb::fresh().await;
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    async {
        let db = Db::connect_with_slow_thresholds(&test.url, 1, WARNING_AFTER, WARNING_AFTER)
            .await.unwrap();
        // An exact advisory latch holds the deferred check. No delay decides
        // whether the commit was sent before its caller disappears.
        sqlx::raw_sql("CREATE TABLE commit_waiter_case (id int PRIMARY KEY);
            CREATE FUNCTION pause_commit_waiter_case() RETURNS trigger LANGUAGE plpgsql AS $$
            BEGIN PERFORM pg_advisory_xact_lock(1701,1); RETURN NEW; END $$;
            CREATE CONSTRAINT TRIGGER pause_commit_waiter_case AFTER INSERT ON commit_waiter_case
                DEFERRABLE INITIALLY DEFERRED FOR EACH ROW EXECUTE FUNCTION pause_commit_waiter_case();")
            .execute(db.pool()).await.unwrap();
        let mut latch = test.pool.begin().await.unwrap();
        sqlx::query("SELECT pg_advisory_xact_lock(1701,1)")
            .execute(&mut *latch).await.unwrap();
        let mut tx = db.begin().await.unwrap();
        let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *tx).await.unwrap();
        sqlx::query("INSERT INTO commit_waiter_case VALUES (1)")
            .execute(&mut *tx).await.unwrap();
        let mut commit = Box::pin(tx.commit());
        let blocked = async {
            loop {
                let blocked: bool = sqlx::query_scalar("SELECT cardinality(pg_blocking_pids($1)) > 0")
                    .bind(pid).fetch_one(&test.pool).await.unwrap();
                if blocked { break; }
                tokio::task::yield_now().await;
            }
        };
        tokio::select! {
            result = &mut commit => panic!("the held commit finished early: {result:?}"),
            () = async { tokio::time::timeout(Duration::from_secs(30), blocked).await.unwrap() } => {}
        }
        drop(commit);
        latch.rollback().await.unwrap();
        let count: i64 = tokio::time::timeout(Duration::from_secs(30),
            sqlx::query_scalar("SELECT count(*) FROM commit_waiter_case").fetch_one(db.pool()))
            .await.unwrap().unwrap();
        assert_eq!(count, 1, "the commit already begun is finished by its owner");
        db.begin().await.unwrap().commit().await.unwrap();
        db.pool().close().await;
    }.with_subscriber(subscriber).await;
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    assert!(
        !logged.contains("there is no transaction in progress"),
        "{logged}"
    );
    assert!(
        !logged.contains("there is already a transaction in progress"),
        "{logged}"
    );
}
