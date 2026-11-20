//! Connections hosted by the connector of an existing environment.

use super::resources::routes::EnvironmentRefRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// The current own connector of an environment, independently of whether it is online.
pub async fn current_machine(conn: &mut Conn, environment: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT r.machine_id FROM environments e JOIN runtimes r ON r.environment_id=e.id AND r.kind='environment' AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed') JOIN machines m ON m.id=r.machine_id AND m.status='active' AND m.deleted_at IS NULL WHERE e.id=$1 AND e.deleted_at IS NULL")
        .bind(environment).fetch_optional(conn).await?)
}

/// Current own connectors for a set of checked environments, in one metadata query.
pub async fn current_machines(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as("SELECT r.environment_id,r.machine_id FROM runtimes r JOIN machines m ON m.id=r.machine_id AND m.status='active' AND m.deleted_at IS NULL WHERE r.environment_id=ANY($1) AND r.kind='environment' AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')")
        .bind(environments).fetch_all(conn).await?)
}

/// The selected logical environment; its machine can change without changing the provider.
pub async fn environment(conn: &mut Conn, provider: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar::<_, Option<Id>>(
        "SELECT connection_environment_id FROM providers WHERE id=$1",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?
    .flatten())
}

/// No value means another location kind. Environment availability includes the current own connector.
pub async fn available(conn: &mut Conn, provider: Id) -> Result<Option<bool>, DbError> {
    Ok(sqlx::query_scalar::<_,Option<bool>>("SELECT CASE WHEN p.connection_environment_id IS NULL THEN NULL ELSE EXISTS(SELECT 1 FROM environments e JOIN runtimes r ON r.environment_id=e.id AND r.kind='environment' AND r.successor_of IS NULL AND r.state='running' JOIN machines m ON m.id=r.machine_id AND m.online AND m.status='active' AND m.deleted_at IS NULL WHERE e.id=p.connection_environment_id AND e.deleted_at IS NULL AND e.status='running' AND r.machine_id=p.machine_id) END FROM providers p WHERE p.id=$1")
        .bind(provider).fetch_optional(conn).await?.flatten())
}

/// Record a checked environment location in the provider creation transaction.
pub async fn bind(conn: &mut Conn, provider: Id, environment: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE providers SET connection_environment_id=$2 WHERE id=$1")
        .bind(provider)
        .bind(environment)
        .execute(conn)
        .await?;
    Ok(())
}

/// Environment references remain factual after removal; the provider does not silently relocate.
pub async fn references(
    conn: &mut Conn,
    providers: &[Id],
) -> Result<Vec<(Id, EnvironmentRefRow)>, DbError> {
    #[derive(sqlx::FromRow)]
    struct Row {
        provider: Id,
        id: Id,
        name: String,
        owner_id: Id,
        placement: String,
        status: String,
        machine_id: Option<Id>,
    }
    let rows:Vec<Row>=sqlx::query_as("SELECT p.id AS provider,e.id,e.name,e.owner_id,e.placement,CASE WHEN e.deleted_at IS NOT NULL THEN 'removed' ELSE e.status END AS status,e.machine_id FROM providers p JOIN environments e ON e.id=p.connection_environment_id WHERE p.id=ANY($1)")
        .bind(providers).fetch_all(conn).await?;
    Ok(rows
        .into_iter()
        .map(|row| {
            (
                row.provider,
                EnvironmentRefRow {
                    id: row.id,
                    name: row.name,
                    owner_id: row.owner_id,
                    placement: row.placement,
                    status: row.status,
                    machine_id: row.machine_id,
                },
            )
        })
        .collect())
}

/// Readable collection candidates can be narrowed to the connection's logical environment.
pub async fn among(conn: &mut Conn, providers: &[Id], environment: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM providers WHERE id=ANY($1) AND connection_environment_id=$2",
    )
    .bind(providers)
    .bind(environment)
    .fetch_all(conn)
    .await?)
}

/// Rebind a bounded page after a move or rebuild. Processed rows no longer match the next page.
/// Native reads are made due so their ordinary coalescing and credential installation restore service.
pub async fn synchronize(
    conn: &mut Conn,
    now: DateTime<Utc>,
    batch: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("WITH selected AS (
        SELECT p.id,p.agent_id,p.connection_environment_id,p.machine_id AS old_machine,r.machine_id
        FROM providers p JOIN environments e ON e.id=p.connection_environment_id
        LEFT JOIN LATERAL (SELECT r.machine_id FROM runtimes r JOIN machines m ON m.id=r.machine_id AND m.status='active' AND m.deleted_at IS NULL WHERE r.environment_id=e.id AND e.deleted_at IS NULL AND r.kind='environment' AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed') LIMIT 1) r ON true
        WHERE p.deleted_at IS NULL AND p.machine_id IS DISTINCT FROM r.machine_id ORDER BY p.id LIMIT $2 FOR NO KEY UPDATE OF p SKIP LOCKED
      ), profiles AS (
        UPDATE agents a SET machine_id=s.machine_id,revision=a.revision+1,updated_at=$1 FROM selected s,machines old,runtimes old_runtime
        WHERE a.id=s.agent_id AND s.machine_id IS NOT NULL AND a.machine_id=old.id AND old.runtime_id=old_runtime.id AND old_runtime.environment_id=s.connection_environment_id AND a.machine_id IS DISTINCT FROM s.machine_id RETURNING a.id
      )
      UPDATE providers p SET machine_id=s.machine_id,
        key_machine_id=CASE WHEN p.kind='subscription' AND (p.key_machine_id IS NULL OR EXISTS(SELECT 1 FROM machines old JOIN runtimes old_runtime ON old_runtime.id=old.runtime_id WHERE old.id=p.key_machine_id AND old_runtime.environment_id=s.connection_environment_id)) THEN s.machine_id ELSE p.key_machine_id END,
        facts_requested_at=NULL,revision=p.revision+1,updated_at=$1
      FROM selected s WHERE p.id=s.id RETURNING p.id")
        .bind(now).bind(batch).fetch_all(conn).await?)
}

/// A shared connection's status follows its environment without overwriting credential failures.
pub async fn synchronize_presence(conn: &mut Conn, now: DateTime<Utc>) -> Result<Vec<Id>, DbError> {
    sqlx::query("UPDATE providers p SET status='offline',status_detail=NULL,revision=p.revision+1,updated_at=$1 WHERE p.connection_environment_id IS NOT NULL AND p.kind='api' AND (p.key_machine_id IS NULL OR p.key_machine_id=p.machine_id) AND p.deleted_at IS NULL AND p.status IN ('ready','exhausted') AND NOT EXISTS(SELECT 1 FROM environments e JOIN machines m ON m.id=p.machine_id WHERE e.id=p.connection_environment_id AND e.deleted_at IS NULL AND e.status='running' AND m.online AND m.status='active' AND m.deleted_at IS NULL)")
        .bind(now).execute(&mut *conn).await?;
    Ok(sqlx::query_scalar("UPDATE providers p SET status='ready',status_detail=NULL,facts_requested_at=NULL,revision=p.revision+1,updated_at=$1 WHERE p.connection_environment_id IS NOT NULL AND p.kind='api' AND (p.key_machine_id IS NULL OR p.key_machine_id=p.machine_id) AND p.deleted_at IS NULL AND p.status='offline' AND EXISTS(SELECT 1 FROM environments e JOIN machines m ON m.id=p.machine_id WHERE e.id=p.connection_environment_id AND e.deleted_at IS NULL AND e.status='running' AND m.online AND m.status='active' AND m.deleted_at IS NULL) RETURNING p.id")
        .bind(now).fetch_all(conn).await?)
}
