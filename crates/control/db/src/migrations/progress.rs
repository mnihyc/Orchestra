//! Migration step observations without changing the migrator's locking, checksums or transaction boundaries.

use crate::Db;
use orch_types::{Clock, Time};
use serde::Serialize;
use sqlx::migrate::Migrator;
use std::time::{Duration, Instant};

/// Public installation activity; SQL text, table names and database credentials never leave the process.
#[derive(Debug, Clone, Serialize)]
pub struct MigrationProgress {
    /// Waiting, schema, repair, complete, or failed.
    pub step: String,
    /// The running migration's public version, when observed.
    pub migration: Option<i64>,
    /// The running migration's public description, when observed.
    pub description: Option<String>,
    /// Completed migration files, when the migration ledger exists.
    pub migrations_done: Option<i64>,
    /// Migration files shipped by this release.
    pub migrations_total: i64,
    /// Rows completed where PostgreSQL reports a row count; otherwise unknown.
    pub rows_done: Option<i64>,
    /// Rows expected where PostgreSQL reports them; otherwise unknown.
    pub rows_total: Option<i64>,
    /// A database-provided phase of an index operation, without its object name.
    pub detail: Option<String>,
    /// Wall-clock observation time from the caller's clock.
    pub updated_at: Time,
    /// Monotonic seconds since migration work began.
    pub elapsed_seconds: u64,
    /// How often the independently served waiting page may refresh this file.
    pub retry_after_seconds: u64,
}

/// Preserves the standard migrator while observing its connection from a second connection.
pub(super) async fn run(
    db: &Db,
    conn: &mut crate::Conn,
    migrator: &Migrator,
    clock: &dyn Clock,
    interval: Duration,
    report: &mut (impl FnMut(&MigrationProgress) + Send),
) -> Result<(), sqlx::migrate::MigrateError> {
    let started = Instant::now();
    let mut status = MigrationProgress {
        step: "waiting".into(),
        migration: None,
        description: None,
        migrations_done: None,
        migrations_total: i64::try_from(migrator.iter().count()).unwrap_or(i64::MAX),
        rows_done: None,
        rows_total: None,
        detail: None,
        updated_at: clock.time(),
        elapsed_seconds: 0,
        retry_after_seconds: interval.as_secs().max(1),
    };
    report(&status);
    let pid: i32 = sqlx::query_scalar("SELECT pg_backend_pid()")
        .fetch_one(&mut *conn)
        .await?;
    let work = migrator.run(&mut *conn);
    tokio::pin!(work);
    // An observation may itself wait on a relation changed by the migration. Poll the migrator
    // beside the complete observer future, so that it can commit and release that relation.
    let result = {
        let observing = async {
            let mut observer = db.acquire().await.ok();
            let mut tick = tokio::time::interval(interval);
            loop {
                tick.tick().await;
                status.updated_at = clock.time();
                status.elapsed_seconds = started.elapsed().as_secs();
                if let Some(observing) = observer.as_mut() {
                    if let Ok(Some(query)) = sqlx::query_scalar::<_, String>(
                        "SELECT query FROM pg_stat_activity WHERE pid=$1",
                    )
                    .bind(pid)
                    .fetch_optional(&mut **observing)
                    .await
                    {
                        let query = query.trim();
                        if let Some(migration) = migrator.iter().find(|migration| {
                            let sql = migration.sql.as_str().trim();
                            !query.is_empty() && (sql.starts_with(query) || query.starts_with(sql))
                        }) {
                            status.step = "schema".into();
                            status.migration = Some(migration.version);
                            status.migrations_done = Some(
                                i64::try_from(
                                    migrator
                                        .iter()
                                        .take_while(|file| file.version != migration.version)
                                        .count(),
                                )
                                .unwrap_or(i64::MAX),
                            );
                            status.description = Some(migration.description.to_string());
                        }
                    }
                    let rows: Result<Option<(i64, i64, String)>, _> = sqlx::query_as("SELECT tuples_done,tuples_total,phase FROM pg_stat_progress_create_index WHERE pid=$1")
                        .bind(pid).fetch_optional(&mut **observing).await;
                    if let Ok(Some((done, total, phase))) = rows {
                        status.rows_done = Some(done);
                        status.rows_total = (total > 0).then_some(total);
                        status.detail = Some(phase);
                    } else {
                        status.rows_done = None;
                        status.rows_total = None;
                        status.detail = None;
                    }
                }
                report(&status);
            }
        };
        tokio::pin!(observing);
        tokio::select! {
            result = &mut work => result,
            () = &mut observing => (&mut work).await,
        }
    };
    status.step = if result.is_ok() { "repair" } else { "failed" }.into();
    status.updated_at = clock.time();
    status.elapsed_seconds = started.elapsed().as_secs();
    if result.is_ok() {
        status.migrations_done = Some(status.migrations_total);
    }
    report(&status);
    result
}

/// Records whether the idempotent data repair completed after the schema.
pub(super) fn finished(
    clock: &dyn Clock,
    elapsed: u64,
    interval: Duration,
    migrations_total: i64,
    rows: Option<u64>,
) -> MigrationProgress {
    MigrationProgress {
        step: if rows.is_some() { "complete" } else { "failed" }.into(),
        migration: None,
        description: None,
        migrations_done: Some(migrations_total),
        migrations_total,
        rows_done: rows.map(|rows| i64::try_from(rows).unwrap_or(i64::MAX)),
        rows_total: None,
        detail: None,
        updated_at: clock.time(),
        elapsed_seconds: elapsed,
        retry_after_seconds: interval.as_secs().max(1),
    }
}
