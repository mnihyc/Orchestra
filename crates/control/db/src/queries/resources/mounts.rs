//! Mount resources, their machine locations and the settings that enable them.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// A currently placed environment's physical directory roots. No other configuration is selected.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct EnvironmentDataUse {
    /// Environment identifier for the batched visibility decision.
    pub id: Id,
    /// Its display name.
    pub name: String,
    /// Its current state.
    pub status: String,
    /// A borrowed workspace path, if any.
    pub workspace: Option<String>,
    /// A borrowed home path, if any.
    pub home: Option<String>,
}

/// One snapshot for either machine or host data-root page.
#[derive(Debug, sqlx::FromRow)]
pub struct EnvironmentDataSnapshot {
    /// The machine owner, if personal.
    pub owner_id: Option<Id>,
    /// A host served by this node, if any.
    pub host_id: Option<Id>,
    /// Its existing environment.roots policy.
    pub roots: Json,
    /// Facts of the local directory-running role.
    pub direct: Json,
    /// Driver facts as the host last reported them.
    pub drivers: Json,
    /// Uses are filtered by authorization by the caller.
    pub environments: sqlx::types::Json<Vec<EnvironmentDataUse>>,
}

/// Roots, actual driver facts and current usages are read together in one statement, with no scan/RPC.
pub async fn environment_data(
    conn: &mut Conn,
    machine: Id,
) -> Result<Option<EnvironmentDataSnapshot>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.owner_id, h.id AS host_id,
                COALESCE(m.facts->'policy'->'environment.roots','[]'::jsonb) AS roots,
                COALESCE(m.facts->'environment_descriptor'->'data_storage','null'::jsonb) AS direct,
                COALESCE(h.drivers,'{}'::jsonb) AS drivers,
                COALESCE(uses.environments,'[]'::jsonb) AS environments
           FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id
           LEFT JOIN LATERAL (
             SELECT jsonb_agg(to_jsonb(placed) ORDER BY placed.name,placed.id) AS environments FROM (
               SELECT e.id,e.name,e.status,e.workspace_root AS workspace,NULLIF(e.facts->>'home','') AS home
                 FROM environments e WHERE e.machine_id=m.id AND m.facts->'environment_descriptor'->>'lifecycle'='attach' AND e.deleted_at IS NULL
               UNION ALL
               SELECT e.id,e.name,e.status,r.volumes->'workspace'->'directory'->>'path' AS workspace,
                      r.volumes->'home'->'directory'->>'path' AS home
                 FROM runtimes r JOIN environments e ON e.id=r.environment_id AND e.deleted_at IS NULL
                WHERE r.host_id=h.id AND r.environment_id IS NOT NULL AND r.successor_of IS NULL
                  AND r.state NOT IN ('retired','removed')
             ) placed
           ) uses ON true
          WHERE m.id=$1 AND m.deleted_at IS NULL",
    ).bind(machine).fetch_optional(conn).await?)
}

/// A mount with its resource page's home.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MountRow {
    /// Its identifier.
    pub id: Id,
    /// Its name for agents and paths.
    pub name: String,
    /// Its description.
    pub description: String,
    /// Its root org.
    pub org_id: Option<Id>,
    /// Its home org.
    pub home_id: Option<Id>,
    /// Its first owner.
    pub owner_id: Option<Id>,
    /// Where a placed mount appears.
    pub target: String,
    /// Whether the underlying data is read-only.
    pub read_only: bool,
    /// Whether it may be attached.
    pub enabled: bool,
    /// What a plan preselects for it: `as_is`, `detach` or `copy`.
    pub default_fate: String,
    /// The revision a change compares.
    pub revision: i64,
}

/// Reads live mounts in one statement.
pub async fn mounts(conn: &mut Conn, ids: &[Id]) -> Result<Vec<MountRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id,name,description,org_id,home_id,owner_id,target,read_only,enabled,default_fate,revision
        FROM mounts WHERE id=ANY($1) AND deleted_at IS NULL ORDER BY id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// Locks a live mount before changing it or its locations.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<MountRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id,name,description,org_id,home_id,owner_id,target,read_only,enabled,default_fate,revision
        FROM mounts WHERE id=$1 AND deleted_at IS NULL FOR UPDATE",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}

/// Creates a mount and its home and first owner.
pub async fn insert(conn: &mut Conn, row: &MountRow, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("INSERT INTO mounts (id,name,description,org_id,home_id,owner_id,target,read_only,default_fate,created_at,updated_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$10)")
        .bind(row.id).bind(&row.name).bind(&row.description).bind(row.org_id).bind(row.home_id)
        .bind(row.owner_id).bind(&row.target).bind(row.read_only).bind(&row.default_fate).bind(at).execute(conn).await?;
    Ok(())
}

/// Changes a mount whose revision has been checked under its row lock.
pub async fn update(conn: &mut Conn, row: &MountRow, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE mounts SET name=$2,description=$3,target=$4,read_only=$5,
        enabled=$6,default_fate=$7,revision=revision+1,updated_at=$8 WHERE id=$1",
    )
    .bind(row.id)
    .bind(&row.name)
    .bind(&row.description)
    .bind(&row.target)
    .bind(row.read_only)
    .bind(row.enabled)
    .bind(&row.default_fate)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Removes a mount's page after its locations have been removed.
pub async fn delete(conn: &mut Conn, id: Id, at: DateTime<Utc>) -> Result<(), DbError> {
    sqlx::query("UPDATE mounts SET deleted_at=$2,updated_at=$2,revision=revision+1 WHERE id=$1")
        .bind(id)
        .bind(at)
        .execute(conn)
        .await?;
    Ok(())
}

/// A mount's data on one machine.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct LocationRow {
    /// The mount.
    pub mount_id: Id,
    /// The machine that holds the data.
    pub machine_id: Id,
    /// Its host, when it is a node.
    pub host_id: Option<Id>,
    /// The host's name.
    pub host_name: Option<String>,
    /// A path or a volume.
    pub kind: String,
    /// The directory or volume's name.
    pub source: String,
    /// The named storage for a volume.
    pub storage: Option<String>,
    /// What its data is, as the mount's managers say: `own`, `shared` or `synced`; none when nobody said.
    pub data_kind: Option<String>,
    /// Who added it.
    pub created_by: Id,
    /// When it was added.
    pub created_at: DateTime<Utc>,
}

/// Reads locations for all the mounts named.
pub async fn locations(conn: &mut Conn, ids: &[Id]) -> Result<Vec<LocationRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT l.mount_id,l.machine_id,h.id AS host_id,h.name AS host_name,
        l.kind,l.source,l.storage,l.data_kind,l.created_by,l.created_at FROM mount_locations l
        JOIN machines m ON m.id=l.machine_id AND m.deleted_at IS NULL
        LEFT JOIN hosts h ON h.machine_id=l.machine_id
        WHERE l.mount_id=ANY($1) ORDER BY l.mount_id,l.machine_id",
    )
    .bind(ids)
    .fetch_all(conn)
    .await?)
}

/// A safe lost-volume record, without any host path or an offered cleanup act.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct LostLocation {
    /// Its historical identity.
    pub id: Id,
    /// Safe machine name at loss.
    pub machine_name: String,
    /// Safe host name at loss.
    pub host_name: Option<String>,
    /// Named storage, when known.
    pub storage: Option<String>,
    /// The last measured bytes.
    pub total_bytes: Option<i64>,
    /// When that measurement was made.
    pub size_at: Option<DateTime<Utc>>,
    /// The confirmed loss or deletion time.
    pub lost_at: DateTime<Utc>,
}

/// Lost volume locations are metadata only, and never re-enter the active location resolver.
pub async fn lost_locations(conn: &mut Conn, mount: Id) -> Result<Vec<LostLocation>, DbError> {
    Ok(sqlx::query_as("SELECT id,machine_name,host_name,storage,total_bytes,size_at,lost_at FROM mount_lost_locations WHERE mount_id=$1 ORDER BY lost_at,id").bind(mount).fetch_all(conn).await?)
}

/// A candidate mount and its optional location on the selected machine.
#[derive(Debug, serde::Deserialize)]
pub struct MachineMountData {
    /// Its resource identifier.
    pub id: Id,
    /// Its agent-facing name.
    pub name: String,
    /// The common path inside environments.
    pub target: String,
    /// Present when this machine has a location.
    pub kind: Option<String>,
    /// The protected path or volume name.
    pub source: Option<String>,
    /// Its named storage, if any.
    pub storage: Option<String>,
    /// What its data is, as the mount's managers say.
    #[serde(default)]
    pub data_kind: Option<String>,
}

/// Last reported attachments of one current environment on the selected machine.
#[derive(Debug, serde::Deserialize)]
pub struct MachineMountEnvironment {
    /// Its resource identifier.
    pub id: Id,
    /// Its name.
    pub name: String,
    /// The last environment state.
    pub status: String,
    /// Attached mount facts, filtered by identifier and state in the projection's caller.
    pub mounts: Json,
}

/// One data read for the host-side mount section, independent of the number of mounts.
#[derive(Debug, sqlx::FromRow)]
pub struct MachineMountSnapshot {
    /// A directory service or driver explicitly offers making owned data here.
    pub makes_data: bool,
    /// A personal machine's owner.
    pub owner_id: Option<Id>,
    /// The host's resource, when the machine is a node.
    pub host_id: Option<Id>,
    /// Its reported storages; permissions are applied by the authorization layer.
    pub storages: Json,
    /// Last size reports, matched against the exact location before disclosure.
    pub sizes: Json,
    /// Mount candidates, including manageable mounts not located here yet.
    pub mounts: sqlx::types::Json<Vec<MachineMountData>>,
    /// Current environments and their actual attachment facts.
    pub environments: sqlx::types::Json<Vec<MachineMountEnvironment>>,
}

/// Reads location details, stored size reports and current attachments in one statement; never probes a node.
pub async fn on_machine(
    conn: &mut Conn,
    machine: Id,
) -> Result<Option<MachineMountSnapshot>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.owner_id,h.id AS host_id,COALESCE(h.storages,m.facts->'storages','[]'::jsonb) AS storages,
          (COALESCE(m.facts->'environment_descriptor'->'data_storage'->>'made_by_orch' IN ('volume','directory'),false)
           OR EXISTS (SELECT 1 FROM jsonb_each(COALESCE(h.drivers,'{}'::jsonb)) driver
                       WHERE driver.value->'data_storage'->>'made_by_orch' IN ('volume','directory'))) AS makes_data,
          COALESCE(m.facts->'mount_sizes','[]'::jsonb) AS sizes,
          COALESCE((SELECT jsonb_agg(to_jsonb(candidate) ORDER BY candidate.name,candidate.id)
            FROM (SELECT x.id,x.name,x.target,l.kind,l.source,l.storage,l.data_kind
              FROM mounts x LEFT JOIN mount_locations l ON l.mount_id=x.id AND l.machine_id=m.id
              WHERE x.deleted_at IS NULL) candidate),'[]'::jsonb) AS mounts,
          COALESCE((SELECT jsonb_agg(jsonb_build_object('id',e.id,'name',e.name,
              'status',e.status,'mounts',COALESCE(e.facts->'mounts','[]'::jsonb)) ORDER BY e.name,e.id)
            FROM environments e WHERE e.deleted_at IS NULL AND e.id IN
              (SELECT own.id FROM environments own WHERE own.machine_id=m.id AND own.deleted_at IS NULL
                 AND m.facts->'environment_descriptor'->>'lifecycle'='attach'
               UNION SELECT r.environment_id FROM runtimes r JOIN environments current_env
                 ON current_env.id=r.environment_id AND current_env.machine_id=r.machine_id
                 WHERE r.host_id=h.id AND r.environment_id IS NOT NULL AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed'))),
            '[]'::jsonb) AS environments
         FROM machines m LEFT JOIN hosts h ON h.machine_id=m.id
         WHERE m.id=$1 AND m.deleted_at IS NULL",
    ).bind(machine).fetch_optional(conn).await?)
}

/// Retains exact-location size reports from an admitted host report without adding a probe to a page read.
pub async fn record_sizes(conn: &mut Conn, host: Id, sizes: &Json) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE machines m SET facts=jsonb_set(m.facts,'{mount_sizes}',$2,true)
        FROM hosts h WHERE h.id=$1 AND h.machine_id=m.id",
    )
    .bind(host)
    .bind(sizes)
    .execute(conn)
    .await?;
    Ok(())
}

/// Saves a location after its authority and any volume have been checked. A changed location keeps the hint
/// its managers gave, which only they change.
pub async fn set_location(conn: &mut Conn, row: &LocationRow) -> Result<(), DbError> {
    sqlx::query("INSERT INTO mount_locations (mount_id,machine_id,kind,source,storage,data_kind,created_by,created_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8) ON CONFLICT (mount_id,machine_id) DO UPDATE SET
        kind=EXCLUDED.kind,source=EXCLUDED.source,storage=EXCLUDED.storage,
        created_by=EXCLUDED.created_by,created_at=EXCLUDED.created_at")
        .bind(row.mount_id).bind(row.machine_id).bind(&row.kind).bind(&row.source)
        .bind(&row.storage).bind(&row.data_kind).bind(row.created_by).bind(row.created_at).execute(conn).await?;
    Ok(())
}

/// Sets or takes away the hint of what a location's data is; false when the mount has no location there.
pub async fn set_data_kind(
    conn: &mut Conn,
    mount: Id,
    machine: Id,
    data_kind: Option<&str>,
) -> Result<bool, DbError> {
    Ok(
        sqlx::query("UPDATE mount_locations SET data_kind=$3 WHERE mount_id=$1 AND machine_id=$2")
            .bind(mount)
            .bind(machine)
            .bind(data_kind)
            .execute(conn)
            .await?
            .rows_affected()
            == 1,
    )
}

/// Forgets a location after any owned volume has been removed.
pub async fn remove_location(conn: &mut Conn, mount: Id, machine: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM mount_locations WHERE mount_id=$1 AND machine_id=$2")
        .bind(mount)
        .bind(machine)
        .execute(conn)
        .await?;
    Ok(())
}

/// The scopes that explicitly enable this mount, including hidden scopes.
pub async fn scopes(conn: &mut Conn, mount: Id) -> Result<Vec<(Id, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT s.node_id,s.value->$1 FROM settings s JOIN nodes n ON n.id=s.node_id
        WHERE s.key='env.mounts' AND s.value ? $1 AND s.value->$1 <> 'false'::jsonb
        AND n.deleted_at IS NULL ORDER BY s.node_id",
    )
    .bind(mount.to_string())
    .fetch_all(conn)
    .await?)
}

/// Environments that could enable this mount, for exact resolution afterwards.
pub async fn candidates(conn: &mut Conn, mount: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT e.id FROM environments e JOIN nodes n ON n.id=e.home_node_id
        WHERE e.deleted_at IS NULL AND (e.config->'env.mounts' ? $1 OR EXISTS
        (SELECT 1 FROM settings s WHERE s.key='env.mounts' AND s.value ? $1
         AND (s.node_id IS NULL OR s.node_id=ANY(n.path)))) ORDER BY e.id",
    )
    .bind(mount.to_string())
    .fetch_all(conn)
    .await?)
}

/// Invalidates connected attaches and marks placed units as needing updated mounts.
pub async fn changed(
    conn: &mut Conn,
    environments: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "UPDATE environments SET attached_session=NULL,updated_at=$2,env_seq=env_seq+1
        WHERE id=ANY($1) AND placement='connected' AND deleted_at IS NULL",
    )
    .bind(environments)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    sqlx::query("UPDATE runtimes SET state_detail='an update is available: mount attachments changed',updated_at=$2
        WHERE environment_id=ANY($1) AND successor_of IS NULL AND state NOT IN ('retired','removed')")
        .bind(environments).bind(at).execute(conn).await?;
    Ok(())
}

/// The machine, drivers and storages of a live host.
pub async fn volume_host(conn: &mut Conn, host: Id) -> Result<Option<(Id, Json, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT h.machine_id,h.drivers,h.storages FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.id=$1 AND m.deleted_at IS NULL",
    )
    .bind(host)
    .fetch_optional(conn)
    .await?)
}

/// Pick only a declared data maker; the directory service and node drivers use the same contract.
pub async fn data_driver(conn: &mut Conn, machine: Id) -> Result<Option<(String, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT name,storages FROM (
           SELECT m.facts->'environment_descriptor'->>'name' AS name, COALESCE(m.facts->'storages','[]'::jsonb) AS storages,0 AS priority
             FROM machines m WHERE m.id=$1 AND m.deleted_at IS NULL AND m.facts->'environment_descriptor'->'data_storage'->>'made_by_orch' IN ('volume','directory')
           UNION ALL
           SELECT driver.key AS name,h.storages,1 AS priority FROM hosts h JOIN machines m ON m.id=h.machine_id
             CROSS JOIN LATERAL jsonb_each(h.drivers) driver WHERE m.id=$1 AND m.deleted_at IS NULL
              AND driver.value->'data_storage'->>'made_by_orch' IN ('volume','directory')
         ) makers ORDER BY priority,name LIMIT 1",
    ).bind(machine).fetch_optional(conn).await?)
}

/// The drivers and storages reported by a host's node machine.
pub async fn capabilities(conn: &mut Conn, machine: Id) -> Result<Option<(Json, Json)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT h.drivers,h.storages FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.machine_id=$1 AND m.deleted_at IS NULL",
    )
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

/// The last complete live set acknowledged for this generation's runtime.
pub async fn applied(conn: &mut Conn, runtime: Id) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT body->'mounts' FROM commands WHERE kind='node.mounts.apply'
        AND body->>'runtime_id'=$1 AND state='succeeded' AND result->>'applied'='true'
        ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(runtime.to_string())
    .fetch_optional(conn)
    .await?)
}

/// The latest approved request may still be retained by the node even if it did not apply.
pub async fn requested(conn: &mut Conn, runtime: Id) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT body->'mounts' FROM commands WHERE kind='node.mounts.apply'
        AND body->>'runtime_id'=$1 ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(runtime.to_string())
    .fetch_optional(conn)
    .await?)
}

/// Private layers created live remain owned environment data after detachment and unit retirement.
pub async fn retain_layers(conn: &mut Conn, runtime: Id, layers: &Json) -> Result<(), DbError> {
    sqlx::query("UPDATE runtimes SET volumes=volumes || $2::jsonb WHERE id=$1")
        .bind(runtime)
        .bind(layers)
        .execute(conn)
        .await?;
    Ok(())
}

/// Keeps a failure visible without hiding an unrelated state detail; the accepted set lives in the command row.
pub async fn live_result(
    conn: &mut Conn,
    runtime: Id,
    reason: Option<&str>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let detail = reason.map(|reason| format!("mount changes need recreate: {reason}"));
    sqlx::query("UPDATE runtimes SET state_detail=CASE WHEN $2::text IS NOT NULL THEN $2
        WHEN state_detail LIKE 'mount changes need recreate:%' OR state_detail LIKE 'mount changes pending:%' OR state_detail='an update is available: mount attachments changed' THEN NULL ELSE state_detail END,updated_at=$3 WHERE id=$1")
        .bind(runtime).bind(detail).bind(at).execute(conn).await?;
    Ok(())
}

/// The last requested attach preserves the tree's chosen indexing mode and explicit ignore patterns.
pub async fn last_attach(
    conn: &mut Conn,
    environment: Id,
    machine: Id,
) -> Result<Option<Json>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT body FROM commands WHERE kind='env.attach' AND machine_id=$2
        AND body->>'environment_id'=$1 ORDER BY created_at DESC,id DESC LIMIT 1",
    )
    .bind(environment.to_string())
    .bind(machine)
    .fetch_optional(conn)
    .await?)
}

/// Mount setting rows and their last writers along a home chain.
pub async fn settings(
    conn: &mut Conn,
    chain: &[Id],
) -> Result<Vec<(Id, Json, Option<Id>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT node_id,value,updated_by FROM settings
        WHERE key='env.mounts' AND node_id=ANY($1)",
    )
    .bind(chain)
    .fetch_all(conn)
    .await?)
}

/// The last person who replaced an environment's own configuration, when it has been edited.
pub async fn configuration_writer(conn: &mut Conn, environment: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT actor_id FROM audit_log WHERE object_kind='environment' AND object_id=$1
        AND actor_kind='user' AND action='environment.updated' AND details->'config'='true'::jsonb
        AND actor_id IS NOT NULL ORDER BY at DESC,id DESC LIMIT 1",
    )
    .bind(environment)
    .fetch_optional(conn)
    .await?)
}

/// Environments on a machine that hold a mount there: one whose unit or own report has it attached, or whose own
/// configuration names it. A copy may land in that machine's location only while none does.
pub async fn holders_on(conn: &mut Conn, mount: Id, machine: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT e.id FROM environments e LEFT JOIN runtimes r ON r.environment_id=e.id AND r.state<>'removed'
        LEFT JOIN hosts h ON h.id=r.host_id WHERE (r.id IS NOT NULL OR (e.deleted_at IS NULL AND e.status IN ('running','starting','offline','failed')))
        AND (e.machine_id=$2 OR h.machine_id=$2) AND (EXISTS (SELECT 1 FROM jsonb_array_elements(COALESCE(e.facts->'mounts','[]')) f
        WHERE f->>'mount_id'=$1::text AND f->>'state'='attached') OR e.config->'env.mounts' ? $1::text) ORDER BY e.id",
    )
    .bind(mount)
    .bind(machine)
    .fetch_all(conn)
    .await?)
}

/// Data a copy displaced from a machine's location of a mount, kept where it is until somebody purges it.
#[derive(Debug, Clone, sqlx::FromRow, serde::Serialize, serde::Deserialize)]
pub struct KeptRow {
    /// Its identifier.
    pub id: Id,
    /// The mount.
    pub mount_id: Id,
    /// The machine it is on.
    pub machine_id: Id,
    /// `path` or `volume`.
    pub kind: String,
    /// The directory or the volume's name.
    pub source: String,
    /// A volume's storage.
    pub storage: Option<String>,
    /// The hint its location had.
    pub data_kind: Option<String>,
    /// How many files it held when it was kept, where measured.
    pub file_count: Option<i64>,
    /// How many bytes.
    pub total_bytes: Option<i64>,
    /// The copy that displaced it.
    pub job_id: Option<Id>,
    /// Who confirmed that copy.
    pub kept_by: Id,
    /// When it was kept.
    pub kept_at: DateTime<Utc>,
}

/// The kept data of the mounts named, oldest first.
pub async fn kept(conn: &mut Conn, mounts: &[Id]) -> Result<Vec<KeptRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id,mount_id,machine_id,kind,source,storage,data_kind,file_count,total_bytes,job_id,kept_by,kept_at
        FROM mount_kept_data WHERE mount_id=ANY($1) AND lost_at IS NULL ORDER BY kept_at,id",
    )
    .bind(mounts)
    .fetch_all(conn)
    .await?)
}

/// Records data as kept; the same data is recorded once.
pub async fn keep(conn: &mut Conn, row: &KeptRow) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO mount_kept_data (id,mount_id,machine_id,kind,source,storage,data_kind,file_count,total_bytes,job_id,kept_by,kept_at)
        VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12) ON CONFLICT (machine_id,kind,source) DO NOTHING",
    )
    .bind(row.id)
    .bind(row.mount_id)
    .bind(row.machine_id)
    .bind(&row.kind)
    .bind(&row.source)
    .bind(&row.storage)
    .bind(&row.data_kind)
    .bind(row.file_count)
    .bind(row.total_bytes)
    .bind(row.job_id)
    .bind(row.kept_by)
    .bind(row.kept_at)
    .execute(conn)
    .await?;
    Ok(())
}

/// Forgets kept data after its volume, if it is one, has been removed.
pub async fn forget_kept(conn: &mut Conn, id: Id) -> Result<Option<KeptRow>, DbError> {
    Ok(sqlx::query_as(
        "DELETE FROM mount_kept_data WHERE id=$1
        RETURNING id,mount_id,machine_id,kind,source,storage,data_kind,file_count,total_bytes,job_id,kept_by,kept_at",
    )
    .bind(id)
    .fetch_optional(conn)
    .await?)
}
