//! Concurrent service startup prepares every required partition without racing in the catalogues.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use chrono::{TimeZone, Utc};
use orch_db::migrations::ensure_partitions;
use orch_testkit::database::TestDb;
use std::time::Duration;

#[tokio::test]
async fn two_services_can_prepare_the_same_fresh_partitions_together() {
    let test = TestDb::fresh().await;
    // Separate, nonoverlapping windows repeat the first-start condition rather than
    // merely finding the partitions the previous iteration already created.
    for round in 0..32 {
        let year = 2100 + round * 3;
        let now = Utc.with_ymd_and_hms(year, 1, 15, 0, 0, 0).unwrap();
        let mut first = test.pool.acquire().await.unwrap();
        let mut second = test.pool.acquire().await.unwrap();
        let first_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *first)
            .await
            .unwrap();
        let second_pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
            .fetch_one(&mut *second)
            .await
            .unwrap();
        let mut blocked = test.pool.begin().await.unwrap();
        sqlx::query("LOCK TABLE raw_events IN ACCESS EXCLUSIVE MODE")
            .execute(&mut *blocked)
            .await
            .unwrap();
        let one = tokio::spawn(async move { ensure_partitions(&mut first, now).await });
        let two = tokio::spawn(async move { ensure_partitions(&mut second, now).await });
        // Before serialization both requests wait on the parent; afterwards one
        // waits there and the other on the advisory lock. Neither relies on a sleep.
        tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                let waiting: i64 = sqlx::query_scalar(
                    "SELECT count(*) FROM pg_stat_activity WHERE pid=ANY($1) AND wait_event_type='Lock'",
                ).bind([first_pid, second_pid].as_slice())
                    .fetch_one(&test.pool).await.unwrap();
                if waiting == 2 { break; }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        }).await.expect("both startup requests reach their database locks");
        blocked.commit().await.unwrap();
        let (one, two) =
            tokio::time::timeout(Duration::from_secs(10), async { tokio::join!(one, two) })
                .await
                .expect("both services finish startup");
        let (one, two) = (one.unwrap(), two.unwrap());
        assert!(
            one.is_ok() && two.is_ok(),
            "fresh window {round}: {one:?}; {two:?}"
        );
        let mut names = Vec::new();
        for offset in 0..3 {
            names.push(format!("raw_events_p{year}{:02}", 1 + offset));
            names.push(format!("metric_samples_p{year}01{:02}", 15 + offset));
            names.push(format!("metric_hours_p{}", year + offset));
        }
        let created: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM pg_class WHERE relname=ANY($1) AND relkind='r'",
        )
        .bind(&names)
        .fetch_one(&test.pool)
        .await
        .unwrap();
        assert_eq!(
            created, 9,
            "both services leave every required partition ready"
        );
    }
}
