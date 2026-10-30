//! Frozen owned volume references and revisioned removal, restoration and confirmed purge.
//!
//! A removed unit and its files have different lifetimes. A deletion reservation
//! holds the environment row across acceptance; the same revision decides whether
//! restoration or purge may claim the files. Host failure never discards a reference.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use serde::{Deserialize, Serialize};

/// One exact volume owned by a committed environment on one host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Volume {
    /// The runtime that recorded the volume.
    pub runtime_id: Id,
    /// The environment that owns it.
    pub environment_id: Id,
    /// The host keeping it.
    pub host_id: Id,
    /// The driver keeping it.
    pub driver: String,
    /// Its logical role, including a private view.
    pub role: String,
    /// Its driver's exact volume name.
    pub name: String,
    /// Its storage, where named.
    pub storage: Option<String>,
}

/// The durable deletion promise and the phase that currently reserves it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, Serialize, Deserialize)]
pub struct Removal {
    /// The environment.
    pub id: Id,
    /// Its accepted removal actor.
    pub deleted_by: Option<Id>,
    /// Actual unit removal time.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Frozen days.
    pub removed_keep_days: Option<i32>,
    /// The actual purge deadline.
    pub volumes_purge_at: Option<DateTime<Utc>>,
    /// Frozen owned references.
    pub retained_volumes: Json,
    /// Measured bytes, or unknown.
    pub retained_bytes: Option<i64>,
    /// When the size was measured.
    pub retained_size_at: Option<DateTime<Utc>>,
    /// The durable phase.
    pub removal_phase: Option<String>,
    /// The compare-and-set revision for restore and purge.
    pub removal_revision: i64,
    /// Confirmed physical removal time.
    pub purged_at: Option<DateTime<Utc>>,
}

const COLUMNS: &str = "id, deleted_by, deleted_at, removed_keep_days, volumes_purge_at, retained_volumes, retained_bytes, retained_size_at, removal_phase, removal_revision, purged_at";

/// Retained metadata grouped by actual host, without exposing physical volume references.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct HostData {
    /// The host keeping these files.
    pub host_id: Id,
    /// Their environment.
    pub environment_id: Id,
    /// The environment's safe name, before reader filtering.
    pub name: String,
    /// Its home.
    pub home_id: Id,
    /// Distinct named or default storages on this host.
    pub storages: Vec<Option<String>>,
}

/// Includes volume-only retention and unresolved restore/purge work, never a published live successor.
pub async fn on_hosts(conn: &mut Conn, hosts: &[Id]) -> Result<Vec<HostData>, DbError> {
    if hosts.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as("SELECT (v.value->>'host_id')::uuid AS host_id,e.id AS environment_id,e.name,e.home_node_id AS home_id,array_agg(DISTINCT v.value->>'storage') AS storages
        FROM environments e CROSS JOIN LATERAL jsonb_array_elements(e.retained_volumes) v(value)
        WHERE e.removal_phase IN ('kept','restoring','purging') AND (v.value->>'host_id')::uuid=ANY($1)
        GROUP BY e.id,(v.value->>'host_id')::uuid ORDER BY host_id,e.volumes_purge_at NULLS LAST,e.id")
        .bind(hosts).fetch_all(conn).await?)
}

/// Reads the promise, including after confirmed purge.
pub async fn removal(conn: &mut Conn, id: Id) -> Result<Option<Removal>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {COLUMNS} FROM environments WHERE id = $1"
    )))
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Captures every owned role from the current runtime, never the shared mounts beside it.
pub async fn references(conn: &mut Conn, environment: Id) -> Result<Vec<Volume>, DbError> {
    Ok(sqlx::query_as::<_, (Id, Id, String, String, String, Option<String>)>(
        "SELECT r.id, r.host_id, r.driver, v.key, v.value->>'name', v.value->>'storage'
         FROM runtimes r CROSS JOIN LATERAL jsonb_each(r.volumes) v
         WHERE r.environment_id=$1 AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')
           AND v.value->>'name' IS NOT NULL ORDER BY r.id, v.key"
    ).bind(environment).fetch_all(conn).await?.into_iter().map(|(runtime_id,host_id,driver,role,name,storage)|
        Volume { runtime_id, environment_id:environment, host_id, driver, role, name, storage }).collect())
}

/// A failed fork records its unpublished destination before rollback schedules cleanup.
pub async fn is_scratch(conn: &mut Conn, environment: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM jobs WHERE kind='thread_fork' AND plan->>'scratch_cleanup'=$1::text AND NOT (plan ? 'committed'))")
        .bind(environment).fetch_one(conn).await?)
}

/// Forgets unpublished scratch only after the old rollback removed its unit and files.
pub async fn discard_scratch(conn: &mut Conn, id: Id, now: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE environments SET deleted_at=$2,purged_at=$2,removal_phase='purged',retained_volumes='[]',remove_at=NULL,machine_id=NULL,status='removed',status_detail=NULL,updated_at=$2,revision=revision+1 WHERE id=$1 AND removal_phase='removing'")
        .bind(id).bind(now).execute(conn).await?;
    Ok(())
}

/// Freezes the removal promise once; repeating acceptance never changes its window or references.
pub async fn freeze(
    conn: &mut Conn,
    id: Id,
    by: Option<Id>,
    days: i32,
    refs: &[Volume],
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE environments SET deleted_by=$2,removed_keep_days=$3,retained_volumes=$4,removal_phase='scheduled',removal_revision=removal_revision+1,updated_at=$5 WHERE id=$1 AND deleted_at IS NULL AND removal_phase IS NULL")
        .bind(id).bind(by).bind(days).bind(serde_json::to_value(refs).map_err(|error| sqlx::Error::Protocol(error.to_string()))?).bind(at).execute(conn).await?.rows_affected()>0)
}

/// Records that unit removal began, so Keep it no longer withdraws the accepted work.
pub async fn begin_remove(conn: &mut Conn, id: Id, revision: i64) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE environments SET removal_phase='removing' WHERE id=$1 AND removal_revision=$2 AND removal_phase IN ('scheduled','removing') AND NOT EXISTS(SELECT 1 FROM threads t JOIN nodes n ON n.id=t.node_id WHERE t.environment_id=$1 AND n.deleted_at IS NULL)")
        .bind(id).bind(revision).execute(conn).await?.rows_affected()>0)
}

/// Records actual unit removal and starts the complete frozen window at that moment.
pub async fn keep_removed(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query("UPDATE environments SET deleted_at=$3,machine_id=CASE WHEN placement='managed' THEN NULL ELSE machine_id END,status='removed',status_detail=NULL,removal_phase='kept',volumes_purge_at=$3 + make_interval(days=>removed_keep_days),remove_at=NULL,updated_at=$3,revision=revision+1 WHERE id=$1 AND removal_revision=$2 AND removal_phase='removing' AND deleted_at IS NULL")
        .bind(id).bind(revision).bind(now).execute(&mut *conn).await?.rows_affected()>0;
    if changed {
        sqlx::query(
            "DELETE FROM settings WHERE key='env.default_id' AND value=to_jsonb($1::uuid::text)",
        )
        .bind(id)
        .execute(&mut *conn)
        .await?;
        sqlx::query("UPDATE environments SET attached_session=NULL WHERE id=$1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        crate::queries::jobs::stamp_environment(conn, id, now).await?;
    }
    Ok(changed)
}

/// Claims kept volumes for one restore or purge under the caller's environment lock.
pub async fn reserve(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    phase: &str,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query("UPDATE environments SET removal_phase=$3,updated_at=$4 WHERE id=$1 AND removal_revision=$2 AND removal_phase='kept' AND purged_at IS NULL")
        .bind(id).bind(revision).bind(phase).bind(now).execute(conn).await?.rows_affected()>0)
}

/// Returns an unsuccessful reservation to its original promise.
pub async fn release(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    phase: &str,
    detail: &str,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE environments SET removal_phase='kept',status='removed',status_detail=$4,updated_at=$5 WHERE id=$1 AND removal_revision=$2 AND removal_phase=$3 AND purged_at IS NULL")
        .bind(id).bind(revision).bind(phase).bind(detail).bind(now).execute(conn).await?;
    Ok(())
}

/// Keeps a partially attempted purge unavailable for restore and dates its retry.
pub async fn retry_purge(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    detail: &str,
    again: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE environments SET status_detail=$3,remove_at=$4,updated_at=$5 WHERE id=$1 AND removal_revision=$2 AND removal_phase='purging' AND purged_at IS NULL")
        .bind(id).bind(revision).bind(detail).bind(again).bind(now).execute(conn).await?;
    Ok(())
}

/// Commits a verified stopped runtime on the kept files and clears the old deletion promise.
pub async fn restored(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    runtime: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query("UPDATE environments e SET deleted_at=NULL,deleted_by=NULL,volumes_purge_at=NULL,removed_keep_days=NULL,retained_volumes='[]',retained_bytes=NULL,retained_size_at=NULL,removal_phase=NULL,purged_at=NULL,remove_at=NULL,status='stopped',status_detail=NULL,machine_id=r.machine_id,generation=r.generation,template_id=r.template_id,updated_at=$4,revision=e.revision+1,removal_revision=removal_revision+1 FROM runtimes r WHERE e.id=$1 AND e.removal_revision=$2 AND e.removal_phase='restoring' AND r.id=$3 AND r.environment_id=e.id AND r.state='stopped'")
        .bind(id).bind(revision).bind(runtime).bind(now).execute(&mut *conn).await?.rows_affected()>0;
    if changed {
        crate::queries::jobs::stamp_environment(conn, id, now).await?;
    }
    Ok(changed)
}

/// Restores only a verified registration in its original machine session, without touching its files.
pub async fn restored_registration(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    machine: Id,
    session: Option<DateTime<Utc>>,
    generation: i32,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed = sqlx::query("UPDATE environments e SET deleted_at=NULL,deleted_by=NULL,volumes_purge_at=NULL,removed_keep_days=NULL,retained_volumes='[]',retained_bytes=NULL,retained_size_at=NULL,removal_phase=NULL,purged_at=NULL,remove_at=NULL,status='stopped',status_detail=NULL,attached_session=$4,generation=$5,updated_at=$6,revision=e.revision+1,removal_revision=removal_revision+1 FROM machines m WHERE e.id=$1 AND e.removal_revision=$2 AND e.removal_phase='restoring' AND e.placement='connected' AND e.machine_id=$3 AND m.id=$3 AND m.online AND m.status='active' AND m.deleted_at IS NULL AND COALESCE(m.session_started_at,m.last_seen_at) IS NOT DISTINCT FROM $4")
        .bind(id).bind(revision).bind(machine).bind(session).bind(generation).bind(now)
        .execute(&mut *conn).await?.rows_affected()>0;
    if changed {
        crate::queries::jobs::stamp_environment(conn, id, now).await?;
    }
    Ok(changed)
}

/// Commits physical absence without removing permanent names or historical environment identities.
pub async fn purged(
    conn: &mut Conn,
    id: Id,
    revision: i64,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let changed=sqlx::query("UPDATE environments SET removal_phase='purged',purged_at=$3,retained_volumes='[]',config='{}',facts='{}',manifest_blob=NULL,host_entry_id=NULL,template_entry_id=NULL,status_detail=NULL,updated_at=$3,removal_revision=removal_revision+1 WHERE id=$1 AND removal_revision=$2 AND removal_phase='purging'")
        .bind(id).bind(revision).bind(now).execute(&mut *conn).await?.rows_affected()>0;
    if changed {
        // The metadata and manifest remain useful after the environment-owned store is gone.
        // Server-backed snapshots have their own lifetime and pins.
        sqlx::query("UPDATE snapshots SET state='unavailable',unavailable_reason='environment_purged' WHERE environment_id=$1 AND storage='environment' AND state<>'expired'")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM secrets WHERE owner_kind='environment' AND owner_id=$1")
            .bind(id)
            .execute(&mut *conn)
            .await?;
        crate::queries::jobs::stamp_environment(conn, id, now).await?;
    }
    Ok(changed)
}

/// Due kept environments; a restore reservation is never eligible.
pub async fn due(conn: &mut Conn, now: DateTime<Utc>, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM environments WHERE (removal_phase='kept' AND volumes_purge_at <= $1) OR (removal_phase='purging' AND remove_at <= $1) ORDER BY volumes_purge_at,id LIMIT $2")
        .bind(now).bind(limit).fetch_all(conn).await?)
}

/// Kept disk declarations and measurements for a host, without content or tenant names.
pub async fn on_host(conn: &mut Conn, host: Id) -> Result<(i64, i64, i64, i64), DbError> {
    Ok(sqlx::query_as("SELECT count(*),COALESCE(sum(retained_bytes),0)::bigint,count(*) FILTER(WHERE retained_bytes IS NULL),COALESCE(sum(COALESCE((e.config->'env.needs'->>'disk_gb')::bigint,(t.spec->'needs'->>'disk_gb')::bigint,(t.spec->'resources'->>'disk_gb')::bigint,0)),0)::bigint FROM environments e LEFT JOIN env_templates t ON t.id=e.template_id WHERE e.removal_phase IN ('kept','restoring','purging') AND EXISTS(SELECT 1 FROM jsonb_array_elements(e.retained_volumes) v WHERE (v->>'host_id')::uuid=$1)")
        .bind(host).fetch_one(conn).await?)
}

/// Rechecks a queued early purge's browser identity without keeping a credential value.
pub async fn active_session(
    conn: &mut Conn,
    session: Id,
    person: Id,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM sessions WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND expires_at>$3)").bind(session).bind(person).bind(now).fetch_one(conn).await?)
}

/// Rechecks a queued early purge's current narrowed token.
pub async fn active_token(
    conn: &mut Conn,
    token: Id,
    person: Id,
    now: DateTime<Utc>,
) -> Result<Option<(Vec<String>, Option<Id>)>, DbError> {
    Ok(sqlx::query_as("SELECT scopes,node_id FROM api_tokens WHERE id=$1 AND user_id=$2 AND revoked_at IS NULL AND (expires_at IS NULL OR expires_at>$3)").bind(token).bind(person).bind(now).fetch_optional(conn).await?)
}

/// The last committed runtime definition, excluding uncommitted failed candidates.
pub async fn last_runtime(conn: &mut Conn, environment: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT id FROM runtimes WHERE environment_id=$1 AND state='removed' AND successor_of IS NULL ORDER BY created_at DESC,id DESC LIMIT 1").bind(environment).fetch_optional(conn).await?)
}

/// A restoring candidate enrolls with a new machine rather than a revoked predecessor's identity.
pub async fn new_machine(conn: &mut Conn, runtime: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE runtimes SET machine_id=NULL WHERE id=$1")
        .bind(runtime)
        .execute(conn)
        .await?;
    Ok(())
}

/// Publishes only a verified stopped candidate in the caller's restoration commit.
pub async fn publish(conn: &mut Conn, runtime: Id) -> Result<(), DbError> {
    sqlx::query("UPDATE runtimes SET successor_of=NULL WHERE id=$1 AND state='stopped'")
        .bind(runtime)
        .execute(conn)
        .await?;
    Ok(())
}

/// The original stored command definition, retaining secret references rather than old wire ciphertext.
pub async fn definition(conn: &mut Conn, runtime: Id) -> Result<Option<Json>, DbError> {
    let frozen = sqlx::query_scalar("SELECT plan->'retained_spec' FROM jobs WHERE kind='environment_remove' AND plan->>'retained_runtime'=$1::text AND plan->'retained_spec' IS NOT NULL AND plan->'retained_spec'<>'null'::jsonb ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(runtime).fetch_optional(&mut *conn).await?;
    if frozen.is_some() {
        return Ok(frozen);
    }
    let mut saved: Option<Json> = sqlx::query_scalar("SELECT plan->'created_spec' FROM jobs WHERE plan->>'runtime'=$1::text AND plan->'created_spec' IS NOT NULL ORDER BY created_at DESC,id DESC LIMIT 1")
        .bind(runtime).fetch_optional(&mut *conn).await?;
    if let Some(spec) = saved.as_mut()
        && let Some(mounts) = crate::queries::resources::mounts::applied(conn, runtime).await?
    {
        spec["mounts"] = mounts;
    }
    Ok(saved)
}

#[derive(sqlx::FromRow)]
struct RemovalOrigin {
    ip: Option<String>,
    user_agent: Option<String>,
    request_id: Option<String>,
    as_admin: Option<Id>,
}

/// Safe origin of this exact accepted removal, without borrowing a later request.
pub async fn removal_origin(
    conn: &mut Conn,
    environment: Id,
    revision: i64,
) -> Result<crate::audit::Origin, DbError> {
    let row: Option<RemovalOrigin> = sqlx::query_as(
        "SELECT ip,user_agent,request_id,CASE WHEN as_admin AND actor_kind='user' THEN actor_id END AS as_admin FROM audit_log WHERE action='environment.removal_scheduled' AND object_id=$1 AND details->>'removal_revision'=$2::text ORDER BY at,id LIMIT 1",
    )
    .bind(environment)
    .bind(revision)
    .fetch_optional(conn)
    .await?;
    Ok(
        row.map_or_else(crate::audit::Origin::default, |row| crate::audit::Origin {
            as_admin: row.as_admin,
            ip: row.ip.and_then(|ip| ip.parse().ok()),
            user_agent: row.user_agent,
            request_id: row.request_id,
        }),
    )
}

/// Preserves the original requester once a purge has possibly changed physical files.
pub async fn started_purge(
    conn: &mut Conn,
    environment: Id,
    revision: i64,
) -> Result<Option<(Option<Id>, Json)>, DbError> {
    Ok(sqlx::query_as("SELECT created_by,plan FROM jobs WHERE
        (environment_id=$1 AND kind='environment_purge' AND plan->>'removal_revision'=$2::text AND plan->>'purge_started'='true')
        OR (kind='storage_cleanup' AND plan->'cleanup_purges' @> jsonb_build_object($1::uuid::text,jsonb_build_object('revision',$2::bigint,'started',true)))
        ORDER BY created_at,id LIMIT 1")
        .bind(environment).bind(revision).fetch_optional(conn).await?)
}
