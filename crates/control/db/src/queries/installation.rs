//! The installation's two small operational records and the snapshot inventory of an existing database backup.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Reads one record; accepted keys are constrained by the schema.
pub async fn get(conn: &mut Conn, key: &str) -> Result<Option<Json>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT value FROM installation_records WHERE key=$1")
            .bind(key)
            .fetch_optional(conn)
            .await?,
    )
}

/// Replaces one metadata record in the caller's transaction, never a dump or an updater command.
pub async fn put(
    conn: &mut Conn,
    key: &str,
    value: &Json,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO installation_records(key,value,updated_at) VALUES($1,$2,$3)
        ON CONFLICT(key) DO UPDATE SET value=excluded.value,updated_at=excluded.updated_at",
    )
    .bind(key)
    .bind(value)
    .bind(now)
    .execute(conn)
    .await?;
    Ok(())
}

/// Active human system admins, for the once-per-offered-version local notice.
pub async fn administrators(conn: &mut Conn) -> Result<Vec<(Id, String)>, DbError> {
    Ok(sqlx::query_as("SELECT id,locale FROM users WHERE is_system_admin AND kind='human' AND status='active' ORDER BY id")
        .fetch_all(conn).await?)
}

/// One exported read-only database point, kept alive by the caller's transaction during pg_dump.
#[derive(Debug)]
pub struct Snapshot {
    /// PostgreSQL's opaque snapshot identifier, given to pg_dump without another inventory point.
    pub id: String,
    /// Database time of this point.
    pub at: DateTime<Utc>,
    /// Last successful schema migration at this point.
    pub schema: i64,
    /// Recorded external locations, always excluded from this database-only command.
    pub excluded: Json,
}

/// Starts and inventories the same snapshot pg_dump will use. No remote machine or private file is read.
pub async fn export_snapshot(conn: &mut Conn) -> Result<Snapshot, DbError> {
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ, READ ONLY")
        .execute(&mut *conn)
        .await?;
    let (id, at, schema): (String, DateTime<Utc>, i64) = sqlx::query_as(
        "SELECT pg_export_snapshot(), transaction_timestamp(), COALESCE((SELECT max(version) FROM _sqlx_migrations WHERE success),0)")
        .fetch_one(&mut *conn).await?;
    let excluded = sqlx::query_scalar(r#"
        WITH envs AS (
          SELECT e.id,e.placement,e.workspace_root,e.machine_id,r.host_id,r.driver,r.volumes
          FROM environments e LEFT JOIN LATERAL (
            SELECT r.host_id,r.driver,r.volumes FROM runtimes r WHERE r.environment_id=e.id
            ORDER BY (r.state NOT IN ('retired','removed')) DESC,r.generation DESC,r.id DESC LIMIT 1
          ) r ON true WHERE e.purged_at IS NULL
        ), places AS (
          SELECT 'workspaces'::text AS category,e.id,e.host_id,e.driver,
            CASE WHEN e.placement='connected' THEN e.workspace_root ELSE e.volumes->'workspace'->>'name' END AS location,
            e.volumes->'workspace'->>'storage' AS storage FROM envs e
          UNION ALL SELECT 'homes',e.id,e.host_id,e.driver,e.volumes->'home'->>'name',e.volumes->'home'->>'storage' FROM envs e
          UNION ALL SELECT 'provider_states',r.provider_id,r.host_id,r.driver,r.volumes->'state'->>'name',r.volumes->'state'->>'storage'
            FROM runtimes r WHERE r.kind='provider' AND r.state<>'removed'
        ), grouped AS (
          SELECT category,jsonb_build_object('count',count(*),'included',false,'reason','host file backup required',
            'entries',jsonb_agg(jsonb_build_object('id',id,'host_id',host_id,'driver',driver,'storage',storage,
              'location',location,'location_status',CASE WHEN location IS NULL OR location='' THEN 'unknown' ELSE 'known' END,
              'availability','unknown') ORDER BY id)) AS inventory FROM places GROUP BY category
        ) SELECT jsonb_build_object(
          'workspaces',jsonb_build_object('count',0,'included',false,'entries','[]'::jsonb,'reason','no registered workspaces'),
          'homes',jsonb_build_object('count',0,'included',false,'entries','[]'::jsonb,'reason','no registered homes'),
          'provider_states',jsonb_build_object('count',0,'included',false,'entries','[]'::jsonb,'reason','no registered provider runtimes'))
          || COALESCE((SELECT jsonb_object_agg(category,inventory) FROM grouped),'{}'::jsonb) || jsonb_build_object(
          'blobs',jsonb_build_object('count',(SELECT count(*) FROM blobs),'included',false,'location','runtime/data/blobs',
            'location_status','known','reason','database references are not immutable blob bytes'),
          'browser_profiles',jsonb_build_object('count',null,'environments_requiring_inspection',(SELECT count(*) FROM envs),
            'included',false,'location_status','unknown','reason','no complete profile inventory exists; inspect the environment home/workspace'),
          'connected_directories',jsonb_build_object('count',(SELECT count(*) FROM envs WHERE placement='connected'),'included',false,
            'entries',COALESCE((SELECT jsonb_agg(jsonb_build_object('id',id,'machine_id',machine_id,'location',workspace_root,
              'location_status','known','availability','unknown') ORDER BY id) FROM envs WHERE placement='connected'),'[]'::jsonb),
            'reason','the directory stays on its owner machine'),
          'mounts',jsonb_build_object('count',(SELECT count(*) FROM mounts WHERE deleted_at IS NULL),'included',false,
            'entries',COALESCE((SELECT jsonb_agg(jsonb_build_object('id',m.id,'machine_id',l.machine_id,'driver',null,
              'storage',l.storage,'location',l.source,'location_status',CASE WHEN l.source IS NULL THEN 'unknown' ELSE 'known' END,
              'availability','unknown') ORDER BY m.id,l.machine_id)
              FROM mounts m LEFT JOIN mount_locations l ON l.mount_id=m.id WHERE m.deleted_at IS NULL),'[]'::jsonb),
            'reason','mount sources and private overlays need separate host copies'),
          'private_overlays',jsonb_build_object('count',null,'included',false,'location_status','unknown',
            'entries',COALESCE((SELECT jsonb_agg(jsonb_build_object('runtime_id',r.id,'host_id',r.host_id,'driver',r.driver,
              'mount',v.key,'location',v.value->>'name','storage',v.value->>'storage','location_status','known','availability','unknown'))
              FROM runtimes r CROSS JOIN LATERAL jsonb_each(r.volumes) v WHERE v.key LIKE 'over:%'),'[]'::jsonb),
            'reason','a private view is not part of the database dump'),
          'edge_volumes',jsonb_build_object('count',null,'included',false,'location_status','unknown',
            'reason','optional edge certificate/config volumes require the operator host inventory'))
        "#).fetch_one(conn).await?;
    Ok(Snapshot {
        id,
        at,
        schema,
        excluded,
    })
}
