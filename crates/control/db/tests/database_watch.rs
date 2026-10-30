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
