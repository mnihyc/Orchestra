//! One durable update/drain fence per physical program, shared with turn admission.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// The latest update's progress; the running version still comes from the authenticated hello.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UpdateRow {
    /// Physical machine, including a host's node machine.
    pub machine_id: Id,
    /// Server recommendation at the time of the request.
    pub recommended_version: String,
    /// queued/updating hold admission; final states release it.
    pub state: String,
    /// The machine's explanation, when one exists.
    pub detail: Option<String>,
    /// Idempotent command identity, persisted before dispatch.
    pub command_id: Option<Id>,
}

/// Tries only this physical machine. Turn admission takes the same locks in UUID order.
/// Contention aborts retryably instead of holding a transaction while another task owns the fence.
pub async fn lock(conn: &mut Conn, machine: Id) -> Result<(), DbError> {
    let acquired: bool = sqlx::query_scalar("SELECT pg_try_advisory_xact_lock(hashtextextended('orch.component-update:' || ($1::uuid)::text, 0))")
        .bind(machine).fetch_one(conn).await?;
    if acquired {
        Ok(())
    } else {
        Err(DbError::Retry)
    }
}

/// Locks this physical machine shared for turn admission: the admissions of every thread on
/// the machine go on together, and only an update's drain and grant ([`lock`]) excludes them.
pub async fn lock_shared(conn: &mut Conn, machine: Id) -> Result<(), DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock_shared(hashtextextended('orch.component-update:' || ($1::uuid)::text, 0))")
        .bind(machine).execute(conn).await?;
    Ok(())
}

/// Latest progress for a batch of visible machine/host rows.
pub async fn among(conn: &mut Conn, machines: &[Id]) -> Result<Vec<UpdateRow>, DbError> {
    Ok(sqlx::query_as("SELECT machine_id,recommended_version,state,detail,command_id FROM component_updates WHERE machine_id=ANY($1)")
        .bind(machines).fetch_all(conn).await?)
}

/// Physical dependencies of a binding: environment, runtime, credential keeper and node hosting either unit.
pub async fn dependencies(
    conn: &mut Conn,
    environment: Id,
    provider: Id,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("WITH direct AS (
        SELECT machine_id AS id FROM environments WHERE id=$1
        UNION SELECT machine_id FROM providers WHERE id=$2
        UNION SELECT key_machine_id FROM providers WHERE id=$2
        UNION SELECT r.machine_id FROM runtimes r WHERE (r.environment_id=$1 OR r.provider_id=$2) AND r.state NOT IN ('removed','retired')
    ), physical AS (
        SELECT id FROM direct WHERE id IS NOT NULL
        UNION SELECT h.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id
          WHERE (r.environment_id=$1 OR r.provider_id=$2) AND r.state NOT IN ('removed','retired')
        UNION SELECT h.machine_id FROM direct d JOIN machines m ON m.id=d.id JOIN hosts h ON h.id=m.host_id
    ) SELECT DISTINCT id FROM physical ORDER BY id")
        .bind(environment).bind(provider).fetch_all(conn).await?)
}

/// Checks under the same transaction locks as request/dispatch; no start can slip between drain and grant.
pub async fn blocks_binding(
    conn: &mut Conn,
    environment: Id,
    provider: Id,
) -> Result<bool, DbError> {
    let machines = dependencies(conn, environment, provider).await?;
    for machine in &machines {
        lock_shared(conn, *machine).await?;
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM component_updates WHERE machine_id=ANY($1) AND state IN ('queued','updating'))")
        .bind(&machines).fetch_one(conn).await?)
}

/// A running/starting turn uses a physical machine if any dependency or its hosting node is there.
pub async fn busy(conn: &mut Conn, machine: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("WITH hosted AS (
        SELECT $1::uuid AS id UNION SELECT m.id FROM machines m JOIN hosts h ON h.id=m.host_id WHERE h.machine_id=$1
        UNION SELECT r.machine_id FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE h.machine_id=$1 AND r.state NOT IN ('removed','retired')
    ), busy_turns AS (
        SELECT t.environment_id,t.provider_id,s.machine_id FROM threads t
          JOIN turns u ON u.id=t.current_turn_id AND u.state IN ('starting','running')
          LEFT JOIN segments s ON s.id=u.segment_id
        UNION ALL
        SELECT s.environment_id,s.provider_id,s.machine_id FROM turns u
          JOIN segments s ON s.id=u.segment_id
          WHERE u.state='interrupted' AND u.error->>'code'='forced' AND NOT u.native_end_confirmed
    ) SELECT EXISTS(SELECT 1 FROM busy_turns b
        LEFT JOIN environments e ON e.id=b.environment_id LEFT JOIN providers p ON p.id=b.provider_id
        WHERE (
          e.machine_id IN (SELECT id FROM hosted) OR p.machine_id IN (SELECT id FROM hosted)
          OR p.key_machine_id IN (SELECT id FROM hosted) OR b.machine_id IN (SELECT id FROM hosted)
          OR EXISTS(SELECT 1 FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE h.machine_id=$1
             AND (r.environment_id=b.environment_id OR r.provider_id=b.provider_id) AND r.state NOT IN ('removed','retired'))))")
        .bind(machine).fetch_one(conn).await?)
}

/// Files a new request without resetting an in-flight command on repeated clicks or hourly reports.
pub async fn request(
    conn: &mut Conn,
    machine: Id,
    recommended: &str,
    person: Option<Id>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    lock(conn, machine).await?;
    let changed = sqlx::query("INSERT INTO component_updates(machine_id,recommended_version,state,requested_by,requested_at,updated_at)
        VALUES($1,$2,'queued',$3,$4,$4) ON CONFLICT(machine_id) DO UPDATE SET
          recommended_version=excluded.recommended_version,state='queued',detail=NULL,command_id=NULL,
          requested_by=excluded.requested_by,requested_at=excluded.requested_at,updated_at=excluded.updated_at
        WHERE component_updates.state NOT IN ('queued','updating')")
        .bind(machine).bind(recommended).bind(person).bind(now).execute(conn).await?.rows_affected() > 0;
    Ok(changed)
}

/// Records the grant before issuing its idempotent command; a restarted coordinator can resume it.
pub async fn grant(
    conn: &mut Conn,
    machine: Id,
    command: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE component_updates SET command_id=$2,state='updating',updated_at=$3 WHERE machine_id=$1 AND state='queued'")
        .bind(machine).bind(command).bind(now).execute(conn).await?;
    Ok(())
}

/// The authenticated minimum-version refusal can carry the idle grant without opening a session.
pub async fn grant_minimum(
    conn: &mut Conn,
    machine: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE component_updates SET state='updating',command_id=NULL,updated_at=$2 WHERE machine_id=$1 AND state IN ('queued','updating')")
        .bind(machine).bind(now).execute(conn).await?;
    Ok(())
}

/// An old-version reconnect retries an acknowledged request without releasing its admission fence.
pub async fn reconnected(conn: &mut Conn, machine: Id, now: DateTime<Utc>) -> Result<(), DbError> {
    lock(conn, machine).await?;
    sqlx::query("UPDATE component_updates SET state='queued',command_id=NULL,updated_at=$2 WHERE machine_id=$1 AND state='updating'")
        .bind(machine).bind(now).execute(conn).await?;
    Ok(())
}

/// Final progress releases admission. The details stay until the next explicit request.
pub async fn finish(
    conn: &mut Conn,
    machine: Id,
    state: &str,
    detail: Option<&str>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    lock(conn, machine).await?;
    let changed = sqlx::query("UPDATE component_updates SET state=$2,detail=$3,updated_at=$4 WHERE machine_id=$1 AND state IN ('queued','updating')")
        .bind(machine).bind(state).bind(detail).bind(now).execute(conn).await?.rows_affected() > 0;
    Ok(changed)
}

/// Threads waiting on a physical update, including every unit hosted by a node.
pub async fn waiting(conn: &mut Conn, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT t.node_id FROM threads t
        LEFT JOIN environments e ON e.id=t.environment_id LEFT JOIN providers p ON p.id=t.provider_id
        WHERE t.state='blocked' AND t.blocked_reason='agent_offline' AND (
          e.machine_id=$1 OR p.machine_id=$1 OR p.key_machine_id=$1
          OR EXISTS(SELECT 1 FROM runtimes r JOIN hosts h ON h.id=r.host_id WHERE h.machine_id=$1
            AND (r.environment_id=t.environment_id OR r.provider_id=t.provider_id) AND r.state NOT IN ('removed','retired')))
        ORDER BY t.node_id")
        .bind(machine).fetch_all(conn).await?)
}

/// Work is bounded by actual outstanding physical requests, not by the number of threads.
pub async fn pending(conn: &mut Conn) -> Result<Vec<UpdateRow>, DbError> {
    Ok(sqlx::query_as("SELECT machine_id,recommended_version,state,detail,command_id FROM component_updates WHERE state IN ('queued','updating') ORDER BY machine_id")
        .fetch_all(conn).await?)
}
