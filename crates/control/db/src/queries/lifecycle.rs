//! Thread settlement, committed close commands and retained environment removal references.

use crate::queries::link::CommandRow;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Locks thread rows before their node flags, matching the turn engine's write order.
pub async fn lock_threads_under(conn: &mut Conn, node: Id) -> Result<(), DbError> {
    sqlx::query(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id
                 WHERE $1=ANY(n.path) ORDER BY t.node_id FOR UPDATE OF t",
    )
    .bind(node)
    .fetch_all(conn)
    .await?;
    Ok(())
}

/// Live threads below a node, including its side lanes and the node itself.
pub async fn threads_under(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
         WHERE $1 = ANY(n.path) AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// Work left by an older archive or an interrupted lifecycle that must still be settled.
pub async fn archive_work_pending(conn: &mut Conn, node: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id
        WHERE $1=ANY(n.path) AND n.deleted_at IS NULL AND (t.side_of IS NOT NULL
        OR EXISTS(SELECT 1 FROM turns WHERE thread_id=t.node_id AND state IN ('starting','running'))
        OR EXISTS(SELECT 1 FROM thread_inputs WHERE thread_id=t.node_id AND state IN ('pending','delivering'))
        OR EXISTS(SELECT 1 FROM approvals WHERE thread_id=t.node_id AND state='pending')
        OR EXISTS(SELECT 1 FROM leases WHERE thread_id=t.node_id AND closed_at IS NULL)
        OR EXISTS(SELECT 1 FROM segments WHERE id=t.current_segment_id AND ended_at IS NULL)))")
        .bind(node).fetch_one(conn).await?)
}

/// Live bindings to an environment, including threads of other homes.
pub async fn bindings(conn: &mut Conn, environment: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
         WHERE t.environment_id = $1 AND n.deleted_at IS NULL ORDER BY t.node_id",
    )
    .bind(environment)
    .fetch_all(conn)
    .await?)
}

/// Clears the current binding without changing historical segments.
pub async fn detach(conn: &mut Conn, thread: Id, environment: Id) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE threads SET environment_id = NULL WHERE node_id = $1 AND environment_id = $2",
    )
    .bind(thread)
    .bind(environment)
    .execute(conn)
    .await?;
    Ok(())
}

/// Cancels accepted-but-unfinished session work before committing its close.
pub async fn cancel_delivery(
    conn: &mut Conn,
    thread: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE commands SET state = 'cancelled', updated_at = $2
         WHERE body->>'thread_id' = $1::uuid::text AND state IN ('queued','sent','accepted')
           AND kind IN ('session.open','session.open.fenced','session.prefix.open','session.continuation.resume',
             'turn.start','turn.start.fenced','turn.input','approval.resolve')",
    )
    .bind(thread)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Pending lifecycle controls, with an interrupt before a close made by the same act.
pub async fn controls(conn: &mut Conn, limit: i64) -> Result<Vec<CommandRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id,machine_id,kind,body,serial_key,state,result,error,attempts,deadline_at,created_at,updated_at
         FROM commands c WHERE state IN ('queued','sent','accepted')
          AND (kind IN ('turn.interrupt','lease.close','session.close','session.close.fenced','session.continuation.close','node.runtime.stop','provider.stop','env.detach','mcp.forget')
            OR (kind='approval.resolve' AND EXISTS(SELECT 1 FROM approvals a WHERE a.state='cancelled'
                AND a.thread_id::text=c.body->>'thread_id' AND a.native_ref=c.body->>'request_ref'))
            )
         ORDER BY created_at, CASE kind WHEN 'turn.interrupt' THEN 0 WHEN 'lease.close' THEN 1 ELSE 2 END, id
         LIMIT $1",
    ).bind(limit).fetch_all(conn).await?)
}

/// Finished local-key purges whose original machine is online and which have not been retried.
pub async fn provider_purges_due(
    conn: &mut Conn,
    ready_before: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<CommandRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT c.id,c.machine_id,c.kind,c.body,c.serial_key,c.state,c.result,c.error,c.attempts,
                c.deadline_at,c.created_at,c.updated_at
           FROM commands c JOIN machines m ON m.id=c.machine_id
          WHERE c.kind='provider.stop' AND c.body->>'purge'='true'
            AND c.state IN ('failed','expired','cancelled') AND c.updated_at <= $1
            AND c.error#>>'{details,purge_retry}' IS NULL
            AND m.online AND m.status='active' AND m.deleted_at IS NULL
          ORDER BY c.updated_at,c.id LIMIT $2",
    )
    .bind(ready_before)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Claims one purge retry; the caller journals its successor in the same transaction.
pub async fn claim_provider_purge_retry(
    conn: &mut Conn,
    previous: Id,
    next: Id,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE commands c SET error=jsonb_set(
            CASE WHEN jsonb_typeof(c.error)='object' THEN c.error ELSE '{}'::jsonb END,
            '{details}',
            (CASE WHEN jsonb_typeof(c.error->'details')='object' THEN c.error->'details' ELSE '{}'::jsonb END)
              || jsonb_build_object('purge_retry',$2::uuid::text), true)
          WHERE c.id=$1 AND c.kind='provider.stop' AND c.body->>'purge'='true'
            AND c.state IN ('failed','expired','cancelled')
            AND c.error#>>'{details,purge_retry}' IS NULL
            AND EXISTS(SELECT 1 FROM machines m WHERE m.id=c.machine_id AND m.online
                       AND m.status='active' AND m.deleted_at IS NULL)",
    )
    .bind(previous)
    .bind(next)
    .execute(conn)
    .await?
    .rows_affected() == 1)
}

/// Retryable detach refusals belonging to a registration that remains withdrawn.
/// A job owns retries for active environments; this covers committed one-way cleanup controls.
pub async fn busy_registration_detaches(
    conn: &mut Conn,
    ready_before: DateTime<Utc>,
    now: DateTime<Utc>,
    limit: i64,
) -> Result<Vec<CommandRow>, DbError> {
    Ok(sqlx::query_as("SELECT c.id,c.machine_id,c.kind,c.body,c.serial_key,c.state,c.result,c.error,c.attempts,
        c.deadline_at,c.created_at,c.updated_at FROM commands c JOIN environments e ON e.id::text=c.body->>'environment_id'
        WHERE c.kind='env.detach' AND c.state='failed' AND c.error @> '{\"code\":\"busy\",\"retryable\":true}'::jsonb
        AND c.error#>>'{details,retry_command}' IS NULL AND c.updated_at<=$1 AND c.deadline_at>$2
        AND e.deleted_at IS NOT NULL AND e.machine_id=c.machine_id ORDER BY c.updated_at,c.id LIMIT $3")
        .bind(ready_before).bind(now).bind(limit).fetch_all(conn).await?)
}

/// Claim a cleanup's next attempt in the same transaction that inserts it.
pub async fn claim_busy_registration_detach(
    conn: &mut Conn,
    previous: Id,
    next: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE commands c SET error=jsonb_set(c.error,'{details}',
        (CASE WHEN jsonb_typeof(c.error->'details')='object' THEN c.error->'details' ELSE '{}'::jsonb END)
        ||jsonb_build_object('retry_command',$2::uuid::text),true)
        WHERE c.id=$1 AND c.kind='env.detach' AND c.state='failed'
        AND c.error @> '{\"code\":\"busy\",\"retryable\":true}'::jsonb AND c.error#>>'{details,retry_command}' IS NULL
        AND c.deadline_at>$3 AND EXISTS(SELECT 1 FROM environments e WHERE e.id::text=c.body->>'environment_id'
            AND e.deleted_at IS NOT NULL AND e.machine_id=c.machine_id)")
        .bind(previous).bind(next).bind(now).execute(conn).await?.rows_affected() == 1)
}

/// Whether a committed close still prevents reopening the same native session.
pub async fn closing(conn: &mut Conn, thread: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM commands WHERE kind IN ('session.close','session.close.fenced','session.continuation.close')
          AND body->>'thread_id' = $1::uuid::text AND state IN ('queued','sent','accepted'))",
    )
    .bind(thread)
    .fetch_one(conn)
    .await?)
}

/// Environments whose own home lies in the deleted subtree.
pub async fn environments_under(conn: &mut Conn, node: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT e.id FROM environments e JOIN nodes n ON n.id = e.home_node_id
         WHERE $1 = ANY(n.path) AND e.deleted_at IS NULL ORDER BY e.id",
    )
    .bind(node)
    .fetch_all(conn)
    .await?)
}

/// Keeps environments explicitly handed to the surviving parent of a deleted department.
pub async fn rehome(
    conn: &mut Conn,
    environment: Id,
    parent: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE environments SET home_node_id=$2,revision=revision+1,updated_at=$3
                 WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(environment)
    .bind(parent)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Records that the removed unit's exact runtime keeps its volumes until the recorded deadline.
pub async fn retain(
    conn: &mut Conn,
    job: Id,
    runtime: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE jobs SET plan = plan || jsonb_build_object('retained_runtime', $2::uuid::text,
                          'volumes_purge_at', to_jsonb($3::timestamptz)) WHERE id = $1",
    )
    .bind(job)
    .bind(runtime)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Whether this retired runtime's unit was already removed while its volumes were retained.
pub async fn retained_runtime(conn: &mut Conn, runtime: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM jobs WHERE kind = 'environment_remove'
          AND plan->>'retained_runtime' = $1::uuid::text AND plan ? 'committed')",
    )
    .bind(runtime)
    .fetch_one(conn)
    .await?)
}

/// Saves the frozen cleanup plan on an existing queued removal.
pub async fn removal_plan(conn: &mut Conn, job: Id, plan: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE jobs SET plan = plan || $2 WHERE id = $1 AND state = 'queued'")
        .bind(job)
        .bind(plan)
        .execute(conn)
        .await?;
    Ok(())
}

/// Retires a physically removed unit until its owned volumes become eligible for cleanup.
pub async fn retain_runtime(
    conn: &mut Conn,
    runtime: Id,
    due: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE runtimes SET state='retired', state_detail='Unit removed; owned volumes retained',
                 remove_at=$2, updated_at=$3 WHERE id=$1 AND state <> 'removed'",
    )
    .bind(runtime)
    .bind(due)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Cancels queued scheduled removal before any host work begins.
pub async fn withdraw_removal(
    conn: &mut Conn,
    environment: Id,
    by: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE jobs SET state='cancelled',finished_at=$3,updated_at=$3,
                 plan=plan || jsonb_build_object('withdrawn',true,'cancel',jsonb_build_object('by',$2::uuid::text,'at',to_jsonb($3::timestamptz)))
                 WHERE environment_id=$1 AND kind='environment_remove' AND state='queued'
                   AND (plan->>'runtime') IS NULL")
        .bind(environment).bind(by).bind(now).execute(conn).await?;
    Ok(())
}

/// The latest accepted removal promise, retained across failures unless Keep it withdrew it.
pub async fn removal_promise(conn: &mut Conn, environment: Id) -> Result<Option<Json>, DbError> {
    let plan: Option<Json> = sqlx::query_scalar(
        "SELECT plan FROM jobs WHERE environment_id=$1
        AND kind='environment_remove' AND (plan->>'runtime') IS NULL
        ORDER BY env_seq DESC NULLS LAST,created_at DESC,id DESC LIMIT 1",
    )
    .bind(environment)
    .fetch_optional(conn)
    .await?;
    Ok(plan.filter(|plan| plan.get("withdrawn").and_then(Json::as_bool) != Some(true)))
}

/// The exact nodes newly deleted by the current deletion, including under a held clock.
pub async fn deletion_nodes(
    conn: &mut Conn,
    node: Id,
    at: DateTime<Utc>,
) -> Result<Option<Vec<Id>>, DbError> {
    let ids: Option<Json> = sqlx::query_scalar(
        "SELECT details->'deleted_nodes' FROM audit_log
        WHERE action='node.deleted' AND object_id=$1 AND at=$2 AND details ? 'deleted_nodes'
        ORDER BY id DESC LIMIT 1",
    )
    .bind(node)
    .bind(at)
    .fetch_optional(conn)
    .await?;
    ids.map(|ids| {
        serde_json::from_value(ids)
            .map_err(|error| DbError::Other(sqlx::Error::Decode(Box::new(error))))
    })
    .transpose()
}

/// Restores only the frozen deletion members, leaving earlier or later tombstones unchanged.
pub async fn restore_nodes(
    conn: &mut Conn,
    node: Id,
    ids: &[Id],
    deleted: DateTime<Utc>,
    by: Id,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE nodes SET deleted_at=NULL,deleted_by=NULL,updated_at=$5,
        archived_at=CASE WHEN id=$1 THEN COALESCE(archived_at,$5) ELSE archived_at END,
        archived_by=CASE WHEN id=$1 THEN COALESCE(archived_by,$4) ELSE archived_by END
        WHERE id=ANY($2) AND $1=ANY(path) AND deleted_at=$3",
    )
    .bind(node)
    .bind(ids)
    .bind(deleted)
    .bind(by)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}
