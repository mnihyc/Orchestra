//! MCP preset and environment-owned resources and their enablement.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// An MCP resource whose imported definition, when any, stays on its machine.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct McpRow {
    /// Whether its native definition is supplied by machine offerings, including when none remains.
    pub imported: bool,
    /// Its identifier.
    pub id: Id,
    /// Its name for a harness.
    pub name: String,
    /// What it is for.
    pub description: String,
    /// Its root org.
    pub org_id: Option<Id>,
    /// Its home org.
    pub home_id: Option<Id>,
    /// Its first owner.
    pub owner_id: Option<Id>,
    /// The machine that owns its imported copy, if any.
    pub machine_id: Option<Id>,
    /// The environment whose own server it is.
    pub environment_id: Option<Id>,
    /// Its public spec or safe import metadata, never secret values.
    pub spec: Json,
    /// Whether it can be enabled.
    pub enabled: bool,
    /// Its revision.
    pub revision: i64,
}

/// The live server's real permission subject, before loading its public definition.
#[derive(Debug, Clone, Copy, sqlx::FromRow)]
pub struct ServerSubject {
    /// The outward server identity.
    pub id: Id,
    /// The owning permission page when this is one environment's own server.
    pub environment_id: Option<Id>,
}

/// Identity only; the ordinary guard and full projection remain with their existing readers.
pub async fn subject(conn: &mut Conn, id: Id) -> Result<Option<ServerSubject>, DbError> {
    Ok(subjects(conn, &[id]).await?.pop())
}

/// The same live native subjects in one typed batch, without loading definitions.
pub async fn subjects(conn: &mut Conn, ids: &[Id]) -> Result<Vec<ServerSubject>, DbError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as(
        "SELECT id,environment_id FROM mcp_servers WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A recorded identity remains occupied after soft deletion, including for native-copy cleanup.
pub async fn recorded(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM mcp_servers WHERE id=$1)")
            .bind(id)
            .fetch_one(conn)
            .await?,
    )
}

/// Reads live servers by identifier.
pub async fn servers(conn: &mut Conn, ids: &[Id]) -> Result<Vec<McpRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,name,description,org_id,home_id,owner_id,machine_id,environment_id,spec,enabled,revision,imported
        FROM mcp_servers WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id")
        .bind(ids).fetch_all(conn).await?)
}

/// Locks a live server before changing its definition or import.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<McpRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,name,description,org_id,home_id,owner_id,machine_id,environment_id,spec,enabled,revision,imported
        FROM mcp_servers WHERE id=$1 AND deleted_at IS NULL FOR UPDATE")
        .bind(id).fetch_optional(conn).await?)
}

/// Complete native parent, server and preset-name serialization before a checked change.
pub async fn lock_update(
    conn: &mut Conn,
    id: Id,
    name: Option<&str>,
) -> Result<Option<McpRow>, DbError> {
    let Some(selected) = subject(conn, id).await? else {
        return Ok(None);
    };
    lock_owned_parent(conn, selected.environment_id).await?;
    let Some(row) = lock(conn, id).await? else {
        return Ok(None);
    };
    // A different current parent needs a new native preparation, not the old parent's lock.
    if row.environment_id != selected.environment_id {
        return Ok(None);
    }
    if !row.imported && row.environment_id.is_none() {
        lock_preset_name(conn, row.org_id, None, name.unwrap_or(&row.name)).await?;
    }
    Ok(Some(row))
}

/// The environment's own servers, including ones switched off.
pub async fn own(conn: &mut Conn, environment: Id) -> Result<Vec<McpRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,name,description,org_id,home_id,owner_id,machine_id,environment_id,spec,enabled,revision,imported
        FROM mcp_servers WHERE environment_id=$1 AND deleted_at IS NULL ORDER BY id")
        .bind(environment).fetch_all(conn).await?)
}

/// Serializes the exact native non-imported preset uniqueness namespace.
/// Imported offerings and environment-owned acts retain their own native parent footprint.
pub async fn lock_preset_name(
    conn: &mut Conn,
    org: Option<Id>,
    environment: Option<Id>,
    name: &str,
) -> Result<(), DbError> {
    let namespace = serde_json::json!([org, environment, name]).to_string();
    crate::queries::chats::lock_named(conn, &format!("mcp.preset:{namespace}")).await
}

/// Creates a resource without ever persisting an imported command or secret.
pub async fn insert(conn: &mut Conn, row: &McpRow, at: DateTime<Utc>) -> Result<(), DbError> {
    lock_owned_parent(conn, row.environment_id).await?;
    if !row.imported && row.environment_id.is_none() {
        lock_preset_name(conn, row.org_id, row.environment_id, &row.name).await?;
    }
    sqlx::query("INSERT INTO mcp_servers (id,name,description,org_id,home_id,owner_id,machine_id,environment_id,spec,enabled,created_at,updated_at,imported)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$11,$12)")
        .bind(row.id).bind(&row.name).bind(&row.description).bind(row.org_id).bind(row.home_id).bind(row.owner_id)
        .bind(row.machine_id).bind(row.environment_id).bind(&row.spec).bind(row.enabled).bind(at).bind(row.imported).execute(&mut *conn).await?;
    if row.imported
        && let Some(machine) = row.machine_id
    {
        offer(
            conn,
            row.id,
            machine,
            None,
            row.spec.get("imported").unwrap_or(&Json::Null),
            at,
        )
        .await?;
    }
    Ok(())
}

/// An import offered by one machine; definitions and credentials remain on that machine.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Offering {
    /// The retained resource.
    pub server_id: Id,
    /// A machine currently supplying or preparing its native copy.
    pub machine_id: Id,
    /// The environment used to read the native definition.
    pub environment_id: Option<Id>,
    /// Pending, ready or failed; lost offerings are not current assignments.
    pub state: String,
    /// Safe discovery/import origin metadata.
    pub source: Json,
    /// A live discovery differing from the already imported protected copy.
    pub proposal: Option<Json>,
    /// When its native copy became available.
    pub offered_at: Option<DateTime<Utc>>,
    /// When it was last reported.
    pub seen_at: DateTime<Utc>,
    /// A safe reason for an unavailable copy.
    pub reason: Option<String>,
}

/// Reads every current offering without treating a missing machine as a deleted resource.
pub async fn offerings(conn: &mut Conn, ids: &[Id]) -> Result<Vec<Offering>, DbError> {
    Ok(sqlx::query_as("SELECT o.server_id,o.machine_id,o.environment_id,o.state,o.source,o.proposal,o.offered_at,o.seen_at,o.reason FROM mcp_server_offerings o JOIN machines m ON m.id=o.machine_id WHERE o.server_id=ANY($1) AND o.state<>'lost' AND m.status='active' AND m.deleted_at IS NULL AND m.lost_at IS NULL ORDER BY o.server_id,o.offered_at,o.machine_id")
        .bind(ids).fetch_all(conn).await?)
}

/// Records a verified native copy while preserving its resource's definition and permission page.
pub async fn offer(
    conn: &mut Conn,
    server: Id,
    machine: Id,
    environment: Option<Id>,
    source: &Json,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO mcp_server_offerings(server_id,machine_id,environment_id,state,source,offered_at,seen_at) VALUES($1,$2,$3,'ready',$4,$5,$5) ON CONFLICT(server_id,machine_id) DO UPDATE SET environment_id=COALESCE(EXCLUDED.environment_id,mcp_server_offerings.environment_id),state='ready',source=EXCLUDED.source,proposal=NULL,job_id=NULL,offered_at=EXCLUDED.offered_at,seen_at=EXCLUDED.seen_at,lost_at=NULL,reason=NULL")
        .bind(server).bind(machine).bind(environment).bind(source).bind(at).execute(&mut *conn).await?;
    sqlx::query("UPDATE mcp_servers SET imported=true,machine_id=COALESCE(machine_id,$2),revision=revision+CASE WHEN machine_id IS DISTINCT FROM $2 THEN 1 ELSE 0 END,updated_at=$3 WHERE id=$1")
        .bind(server).bind(machine).bind(at).execute(conn).await?;
    Ok(())
}

/// Marks one attempted offering unavailable without deleting or disabling its resource.
pub async fn offer_failed(
    conn: &mut Conn,
    server: Id,
    machine: Id,
    job: Id,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let changed = sqlx::query("UPDATE mcp_server_offerings SET state='failed',reason=$3,seen_at=$4 WHERE server_id=$1 AND machine_id=$2 AND state='pending' AND job_id=$5")
        .bind(server).bind(machine).bind(reason).bind(at).bind(job).execute(&mut *conn).await?.rows_affected();
    if changed > 0 {
        sqlx::query("UPDATE mcp_servers SET revision=revision+1,updated_at=$2 WHERE id=$1")
            .bind(server)
            .bind(at)
            .execute(conn)
            .await?;
    }
    Ok(())
}

/// Changes a server after its revision has been checked under its row lock.
pub async fn update(conn: &mut Conn, row: &McpRow, at: DateTime<Utc>) -> Result<(), DbError> {
    lock_owned_parent(conn, row.environment_id).await?;
    if !row.imported && row.environment_id.is_none() {
        lock_preset_name(conn, row.org_id, None, &row.name).await?;
    }
    sqlx::query("UPDATE mcp_servers SET name=$2,description=$3,spec=$4,enabled=$5,revision=revision+1,updated_at=$6 WHERE id=$1")
        .bind(row.id).bind(&row.name).bind(&row.description).bind(&row.spec).bind(row.enabled).bind(at).execute(conn).await?;
    Ok(())
}

/// Deletes a resource and the sealed values it alone owns.
pub async fn delete(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    let environment: Option<Option<Id>> =
        sqlx::query_scalar("SELECT environment_id FROM mcp_servers WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
    lock_owned_parent(conn, environment.flatten()).await?;
    sqlx::query("DELETE FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1")
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query(
        "UPDATE mcp_servers SET deleted_at=$2,updated_at=$2,revision=revision+1 WHERE id=$1",
    )
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Owned names and their enablement share the real parent row. Native transaction
/// callers hold it before insert/rename/delete, so creation has no late name-index wait.
async fn lock_owned_parent(conn: &mut Conn, environment: Option<Id>) -> Result<(), DbError> {
    if let Some(environment) = environment {
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
            .bind(environment)
            .fetch_optional(conn)
            .await?;
    }
    Ok(())
}

/// Enables a resource in one environment's own configuration, within the caller's transaction.
pub async fn enable(
    conn: &mut Conn,
    environment: Id,
    server: Id,
    enabled: bool,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE environments SET config=jsonb_set(config,'{mcp.enabled}',
        COALESCE(config->'mcp.enabled','{}'::jsonb)||jsonb_build_object($2::text,$3::boolean)),
        revision=revision+1,env_seq=env_seq+1,updated_at=$4 WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(environment)
    .bind(server.to_string())
    .bind(enabled)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Every environment of one machine that can be used to reimport a local definition.
pub async fn on_machine(conn: &mut Conn, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM environments WHERE machine_id=$1 AND deleted_at IS NULL ORDER BY id",
    )
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// Safe discoveries reported by this machine's environments, without reading a definition.
pub async fn found_on_machine(conn: &mut Conn, machine: Id) -> Result<Vec<Json>, DbError> {
    Ok(found_on_machines(conn, &[machine])
        .await?
        .into_iter()
        .map(|(_, found)| found)
        .collect())
}

/// The same safe discoveries in one batch, retaining their actual physical source.
pub async fn found_on_machines(
    conn: &mut Conn,
    machines: &[Id],
) -> Result<Vec<(Id, Json)>, DbError> {
    if machines.is_empty() {
        return Ok(Vec::new());
    }
    Ok(sqlx::query_as("SELECT e.machine_id,items.found FROM environments e
        CROSS JOIN LATERAL jsonb_array_elements(COALESCE(e.facts->'mcp_found','[]'::jsonb)) AS items(found)
        WHERE e.machine_id=ANY($1) AND e.deleted_at IS NULL")
        .bind(machines).fetch_all(conn).await?)
}

/// Sealed secret references for a server, for the command writer to seal to its target machine.
pub async fn secret_ids(conn: &mut Conn, server: Id) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT name,id FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 ORDER BY name",
    )
    .bind(server)
    .fetch_all(conn)
    .await?)
}

/// The last writers of MCP settings down the home chain.
pub async fn settings(conn: &mut Conn, chain: &[Id]) -> Result<Vec<(Id, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id,updated_by FROM settings WHERE key='mcp.enabled' AND node_id=ANY($1)",
    )
    .bind(chain)
    .fetch_all(conn)
    .await?)
}

/// The latest command's observed start result for every named server of this environment.
pub async fn states(conn: &mut Conn, environment: Id) -> Result<Vec<(String, String)>, DbError> {
    Ok(sqlx::query_as("SELECT DISTINCT ON (body->>'name') body->>'name',state FROM commands
        WHERE kind='mcp.start' AND body->>'environment_id'=$1 ORDER BY body->>'name',created_at DESC,id DESC")
        .bind(environment.to_string()).fetch_all(conn).await?)
}

/// Whether every currently bound harness can take a different MCP set between turns.
pub async fn reload_available(conn: &mut Conn, environment: Id) -> Result<bool, DbError> {
    let descriptors: Vec<Json> = sqlx::query_scalar(
        "SELECT DISTINCT a.descriptor FROM threads t
        JOIN nodes n ON n.id=t.node_id JOIN agents a ON a.id=t.agent_id
        WHERE t.environment_id=$1 AND n.deleted_at IS NULL AND a.deleted_at IS NULL",
    )
    .bind(environment)
    .fetch_all(conn)
    .await?;
    Ok(!descriptors.is_empty()
        && descriptors.iter().all(|descriptor| {
            descriptor
                .pointer("/capabilities/mcp/reload")
                .and_then(Json::as_bool)
                == Some(true)
        }))
}

/// Retains a different discovered definition as a candidate, without making it usable.
pub async fn found(
    conn: &mut Conn,
    server: Id,
    machine: Id,
    source: &Json,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE mcp_server_offerings SET state='found',source=$3,reason=$4,seen_at=$5 WHERE server_id=$1 AND machine_id=$2 AND state<>'lost'").bind(server).bind(machine).bind(source).bind(reason).bind(at).execute(&mut *conn).await?;
    sqlx::query("UPDATE mcp_servers SET revision=revision+1,updated_at=$2 WHERE id=$1")
        .bind(server)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// A manager's selected definition becomes canonical; other different definitions remain found.
pub async fn select_definition(
    conn: &mut Conn,
    row: &McpRow,
    machine: Id,
    environment: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    update(conn, row, at).await?;
    sqlx::query("UPDATE mcp_server_offerings SET state='found',reason='different_definition' WHERE server_id=$1 AND machine_id<>$2 AND state='ready' AND source->>'definition_fingerprint' IS DISTINCT FROM $3")
        .bind(row.id).bind(machine).bind(row.spec.pointer("/imported/definition_fingerprint").and_then(Json::as_str)).execute(&mut *conn).await?;
    sqlx::query("UPDATE mcp_servers SET machine_id=$2 WHERE id=$1")
        .bind(row.id)
        .bind(machine)
        .execute(&mut *conn)
        .await?;
    let mut source = row.spec["imported"].clone();
    source["transport"] = row.spec["transport"].clone();
    offer(conn, row.id, machine, Some(environment), &source, at).await
}

/// Native copy and canonical-definition acts serialize without holding a resource row across a machine call.
pub async fn lock_definition(conn: &mut Conn, id: Id) -> Result<(), DbError> {
    lock_definition_for_command(conn, id, None).await
}

/// The same definition fence also observes its actual outstanding native copy.
/// Only that exact privately checked command may prepare its own delivery/completion.
pub async fn lock_definition_for_command(
    conn: &mut Conn,
    id: Id,
    own: Option<Id>,
) -> Result<(), DbError> {
    sqlx::query(
        "SELECT pg_advisory_xact_lock(hashtextextended('mcp.definition:'||$1::uuid::text,0))",
    )
    .bind(id)
    .execute(&mut *conn)
    .await?;
    let pending:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM commands WHERE kind='mcp.import' AND state IN ('queued','sent','accepted') AND body->>'server_id'=$1 AND ($2::uuid IS NULL OR id<>$2))")
        .bind(id.to_string()).bind(own).fetch_one(conn).await?;
    if pending {
        return Err(DbError::Retry);
    }
    Ok(())
}
