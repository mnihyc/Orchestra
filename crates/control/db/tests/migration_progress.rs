//! Real migrations keep their transaction and ledger while reporting the active step and completion.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use orch_testkit::database::TestDb;
use std::path::Path;
use std::time::Duration;

#[tokio::test]
async fn a_running_migration_is_observed_without_replacing_the_standard_migrator() {
    let db = TestDb::fresh().await;
    let directory = tempfile::tempdir().unwrap();
    let shipped = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../migrations");
    for entry in std::fs::read_dir(shipped).unwrap() {
        let entry = entry.unwrap();
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "sql")
        {
            std::fs::copy(entry.path(), directory.path().join(entry.file_name())).unwrap();
        }
    }
    std::fs::write(directory.path().join("9000_observed_update.sql"),
        "LOCK TABLE _sqlx_migrations IN ACCESS EXCLUSIVE MODE; CREATE TABLE observed_update(id bigint); INSERT INTO observed_update SELECT generate_series(1,3); SELECT pg_sleep(2);").unwrap();
    let mut observations = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        orch_db::migrations::apply_reporting(
            &orch_db::Db::from_pool(db.pool.clone()),
            directory.path(),
            &orch_types::SystemClock,
            Duration::from_secs(1),
            |progress| observations.push(progress.clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(
        observations
            .iter()
            .any(|progress| progress.step == "schema" && progress.migration == Some(9000))
    );
    assert!(
        observations
            .iter()
            .any(|progress| progress.elapsed_seconds >= 1)
    );
    assert_eq!(observations.last().unwrap().step, "complete");
    let applied_count: i64 = sqlx::query_scalar("SELECT count(*) FROM _sqlx_migrations")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(observations.last().unwrap().migrations_total, applied_count);
    assert_eq!(
        observations.last().unwrap().migrations_done,
        Some(applied_count)
    );
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM observed_update")
        .fetch_one(&db.pool)
        .await
        .unwrap();
    assert_eq!(count, 3);
    let applied: bool =
        sqlx::query_scalar("SELECT success FROM _sqlx_migrations WHERE version=9000")
            .fetch_one(&db.pool)
            .await
            .unwrap();
    assert!(applied);
}
