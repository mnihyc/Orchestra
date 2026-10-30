//! Database-backed component health.
//!
//! Each service writes, every few seconds, that it is alive and which
//! build it is. Health and the readiness of a deployment are read from
//! these rows, so that no service has to reach another to know.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Three fifteen-second beats: a missing worker or a silence longer than this makes readiness fail.
pub const WORKER_HEARTBEAT_MAX_AGE: chrono::Duration = chrono::Duration::seconds(45);

/// Whether the worker has reported within the same age limit shown on the system's Overview.
pub async fn worker_ready(conn: &mut Conn, now: DateTime<Utc>) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM component_heartbeats WHERE component='worker' AND seen_at >= $1)")
        .bind(now - WORKER_HEARTBEAT_MAX_AGE).fetch_one(conn).await?)
}

/// One service, as it last reported.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Heartbeat {
    /// `api`, `gateway` or `worker`.
    pub component: String,
    /// A value drawn when that process started: a new one means a restart.
    pub system: Id,
    /// Its version.
    pub version: String,
    /// When it started.
    pub started_at: DateTime<Utc>,
    /// When it last reported.
    pub seen_at: DateTime<Utc>,
}

/// Reports that `component`, the process `run` of `version` started at
/// `started_at`, is alive at `now`.
pub async fn beat(
    conn: &mut Conn,
    component: &str,
    run: Id,
    version: &str,
    started_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO component_heartbeats (component, system, version, started_at, seen_at) VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (component) DO UPDATE
            SET system = EXCLUDED.system, version = EXCLUDED.version, started_at = EXCLUDED.started_at, seen_at = EXCLUDED.seen_at",
    )
    .bind(component)
    .bind(run)
    .bind(version)
    .bind(started_at)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Every service that ever reported.
pub async fn all(conn: &mut Conn) -> Result<Vec<Heartbeat>, DbError> {
    Ok(sqlx::query_as("SELECT component, system, version, started_at, seen_at FROM component_heartbeats ORDER BY component")
        .fetch_all(conn)
        .await?)
}

/// How far the schema has come: the migrations recorded as applied, and the
/// newest of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SchemaState {
    /// How many migrations were applied.
    pub applied: i64,
    /// The version number of the newest, when any was.
    pub newest: Option<i64>,
}

/// The state of the schema; nothing applied before the first start.
pub async fn schema_state(conn: &mut Conn) -> Result<SchemaState, DbError> {
    let recorded: bool = sqlx::query_scalar("SELECT to_regclass('_sqlx_migrations') IS NOT NULL")
        .fetch_one(&mut *conn)
        .await?;
    if !recorded {
        return Ok(SchemaState {
            applied: 0,
            newest: None,
        });
    }
    let (applied, newest): (i64, Option<i64>) =
        sqlx::query_as("SELECT count(*), max(version) FROM _sqlx_migrations WHERE success")
            .fetch_one(conn)
            .await?;
    Ok(SchemaState { applied, newest })
}

/// The version of the database server, as it names itself.
pub async fn server_version(conn: &mut Conn) -> Result<String, DbError> {
    Ok(sqlx::query_scalar("SELECT version()")
        .fetch_one(conn)
        .await?)
}

/// A host, which is a machine that runs the node, as the system last saw it.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct HostSeen {
    /// What the host is called.
    pub name: String,
    /// Whether it has a session with a gateway now.
    pub online: bool,
    /// When it last had one.
    pub last_seen_at: Option<DateTime<Utc>>,
    /// The connector build it runs.
    pub connector_version: String,
}

/// Every host that is enrolled and not revoked or deleted, by name.
pub async fn hosts_seen(conn: &mut Conn) -> Result<Vec<HostSeen>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.name, m.online, m.last_seen_at, m.connector_version
           FROM hosts h JOIN machines m ON m.id = h.machine_id
          WHERE m.status = 'active' AND m.deleted_at IS NULL
          ORDER BY m.name",
    )
    .fetch_all(conn)
    .await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;
    use orch_testkit::{clock, database::TestDb};

    #[tokio::test]
    async fn a_service_is_listed_with_its_latest_report() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let (run, start) = (orch_types::new_id(), clock::start());
        beat(&mut conn, "worker", run, "0.1.0", start, start)
            .await
            .unwrap();
        beat(
            &mut conn,
            "worker",
            run,
            "0.1.0",
            start,
            start + Duration::seconds(10),
        )
        .await
        .unwrap();
        let seen = all(&mut conn).await.unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(
            (seen[0].component.as_str(), seen[0].seen_at),
            ("worker", start + Duration::seconds(10))
        );
    }
    #[tokio::test]
    async fn a_fresh_database_has_a_schema_and_no_host() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        let schema = schema_state(&mut conn).await.unwrap();
        assert!(schema.applied >= 1 && schema.newest.is_some());
        assert!(hosts_seen(&mut conn).await.unwrap().is_empty());
        assert!(
            server_version(&mut conn)
                .await
                .unwrap()
                .contains("PostgreSQL")
        );
    }

    #[tokio::test]
    async fn a_host_is_listed_with_whether_it_is_online_and_a_revoked_one_is_not() {
        let test = TestDb::fresh().await;
        let mut conn = test.pool.acquire().await.unwrap();
        for (name, status) in [("kept", "active"), ("gone", "revoked")] {
            let machine =
                crate::queries::resources::testing::insert_machine(&mut conn, name, "managed")
                    .await
                    .unwrap();
            sqlx::query("UPDATE machines SET status = $2, online = true WHERE id = $1")
                .bind(machine)
                .bind(status)
                .execute(&mut *conn)
                .await
                .unwrap();
            sqlx::query("INSERT INTO hosts (id, machine_id, name) VALUES ($1, $2, $3)")
                .bind(orch_types::new_id())
                .bind(machine)
                .bind(name)
                .execute(&mut *conn)
                .await
                .unwrap();
        }
        let seen = hosts_seen(&mut conn).await.unwrap();
        assert_eq!(
            seen.iter()
                .map(|host| (host.name.as_str(), host.online))
                .collect::<Vec<_>>(),
            [("kept", true)]
        );
    }
}
