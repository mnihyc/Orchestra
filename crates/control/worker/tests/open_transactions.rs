//! The worker measures how long the oldest transaction of the services has been open, and measures none once it ends.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::TestDb;

fn gauge(readings: &orch_telemetry::Readings, name: &str) -> f64 {
    readings
        .render()
        .lines()
        .find_map(|line| {
            let (key, value) = line.split_once(' ')?;
            key.starts_with(name).then(|| value.parse::<f64>().unwrap())
        })
        .expect("the series is exported")
}

#[tokio::test]
async fn a_transaction_left_open_is_measured_from_its_start_and_none_is_once_it_ends() {
    let readings = orch_telemetry::install_metrics().unwrap();
    let test = TestDb::fresh().await;
    let mut conn = test.pool.acquire().await.unwrap();
    assert_eq!(
        orch_worker_core::gauges::transactions(&mut conn)
            .await
            .unwrap(),
        None
    );
    assert_eq!(gauge(&readings, "orch_db_oldest_transaction_seconds"), 0.0);

    let mut held = test.pool.begin().await.unwrap();
    sqlx::query("SELECT 1").execute(&mut *held).await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(1_100)).await;
    let age = orch_worker_core::gauges::transactions(&mut conn)
        .await
        .unwrap()
        .expect("the transaction is open");
    assert!(
        age >= 1.0,
        "measured from the transaction's start, not from its last statement: {age}"
    );
    assert!(gauge(&readings, "orch_db_oldest_transaction_seconds") >= 1.0);
    assert!(age < orch_worker_core::gauges::TRANSACTION_WARN_AFTER_SECS);

    held.rollback().await.unwrap();
    assert_eq!(
        orch_worker_core::gauges::transactions(&mut conn)
            .await
            .unwrap(),
        None
    );
    assert_eq!(gauge(&readings, "orch_db_oldest_transaction_seconds"), 0.0);
}
