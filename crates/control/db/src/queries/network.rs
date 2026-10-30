//! What is stored of runtimes' networks: the state their host's node last reported, the restriction retained on an old unit, and the network their template asks for.
//!
//! A runtime's network is reported by the registered node of its host and kept whole, as it said it, in
//! `runtimes.network_state`; the restriction a requested change asked for is kept in `runtimes.network_required`
//! on the old unit before it is stopped, so that no later start reopens it. What the template the runtime was made
//! from asks for is read beside them, for a runtime whose host has not reported yet.

use orch_types::{Id, Json};

use crate::{Conn, DbError};

/// Runtime identifiers on this host, including cold units and replacement candidates.
pub async fn on_host(conn: &mut Conn, host: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM runtimes WHERE host_id = $1 AND state NOT IN ('retired','removed')",
    )
    .bind(host)
    .fetch_all(conn)
    .await?)
}

/// Online hosts with live restricted runtimes, including a retained restriction on an old open one.
pub async fn active_hosts(conn: &mut Conn) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT DISTINCT h.id FROM hosts h JOIN machines m ON m.id = h.machine_id
           JOIN runtimes r ON r.host_id = h.id JOIN env_templates t ON t.id = r.template_id
          WHERE m.online AND m.status = 'active' AND m.deleted_at IS NULL
            AND NOT h.disabled AND r.state NOT IN ('retired','removed')
            AND (r.network_required = 'restricted' OR r.network_state ->> 'mode' = 'restricted'
                 OR t.spec -> 'network' ->> 'mode' = 'none' OR t.spec -> 'network' ->> 'egress' = 'restricted')")
        .fetch_all(conn).await?)
}

/// Current runtimes used by a thread's environment or provider, without requiring a unit enrollment.
pub async fn for_thread(conn: &mut Conn, thread: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT r.id FROM runtimes r JOIN threads t ON t.node_id = $1
          WHERE (r.environment_id = t.environment_id OR r.provider_id = t.provider_id)
            AND r.successor_of IS NULL AND r.state NOT IN ('retired','removed')",
    )
    .bind(thread)
    .fetch_all(conn)
    .await?)
}

/// A runtime's network as stored.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RuntimeNetworkRow {
    /// The runtime.
    pub runtime_id: Id,
    /// The environment, for an environment's runtime.
    pub environment_id: Option<Id>,
    /// The provider, for a provider's runtime.
    pub provider_id: Option<Id>,
    /// What its host's node last reported of its network; none before it reported.
    pub network_state: Option<Json>,
    /// The mode a requested change retained on it (`open` or `restricted`); none when nothing was.
    pub network_required: Option<String>,
    /// The `network` member of the spec of the template it was made from.
    pub template_network: Option<Json>,
}

/// The networks of the runtimes `runtimes`.
pub async fn of_runtimes(
    conn: &mut Conn,
    runtimes: &[Id],
) -> Result<Vec<RuntimeNetworkRow>, DbError> {
    let mut found: Vec<RuntimeNetworkRow> = sqlx::query_as(
        "SELECT r.id AS runtime_id, r.environment_id, r.provider_id, r.network_state, r.network_required,
                t.spec -> 'network' AS template_network
           FROM runtimes r JOIN env_templates t ON t.id = r.template_id
          WHERE r.id = ANY($1)",
    )
    .bind(runtimes)
    .fetch_all(&mut *conn)
    .await?;
    frozen_networks(conn, &mut found).await?;
    Ok(found)
}

/// The networks of the runtimes the environments `environments` stand on now: for each, its runtime that is
/// neither retired nor removed nor a candidate, the newest where there are several.
pub async fn of_environments(
    conn: &mut Conn,
    environments: &[Id],
) -> Result<Vec<RuntimeNetworkRow>, DbError> {
    let mut found: Vec<RuntimeNetworkRow> = sqlx::query_as(
        "SELECT DISTINCT ON (r.environment_id)
                r.id AS runtime_id, r.environment_id, r.provider_id, r.network_state, r.network_required,
                t.spec -> 'network' AS template_network
           FROM runtimes r JOIN env_templates t ON t.id = r.template_id
          WHERE r.environment_id = ANY($1) AND r.state NOT IN ('retired', 'removed')
            AND r.successor_of IS NULL
          ORDER BY r.environment_id, r.created_at DESC, r.id",
    )
    .bind(environments)
    .fetch_all(&mut *conn)
    .await?;
    frozen_networks(conn, &mut found).await?;
    Ok(found)
}

async fn frozen_networks(conn: &mut Conn, rows: &mut [RuntimeNetworkRow]) -> Result<(), DbError> {
    let ids: Vec<Id> = rows.iter().map(|row| row.runtime_id).collect();
    for (id, definition) in crate::queries::jobs::changes::definitions(conn, &ids).await? {
        if let Some(network) = definition.and_then(|definition| {
            definition
                .get("spec")
                .and_then(|spec| spec.get("network"))
                .cloned()
        }) && let Some(row) = rows.iter_mut().find(|row| row.runtime_id == id)
        {
            row.template_network = Some(network);
        }
    }
    Ok(())
}

/// Keeps a changed host report and stamps its resource projection; false when absent or unchanged.
pub async fn store(
    conn: &mut Conn,
    runtime: Id,
    state: &Json,
    now: chrono::DateTime<chrono::Utc>,
) -> Result<bool, DbError> {
    let changed: Option<(Option<Id>, Option<Id>)> = sqlx::query_as("UPDATE runtimes SET network_state = $2, updated_at = GREATEST(updated_at, $3) WHERE id = $1 AND network_state IS DISTINCT FROM $2 RETURNING environment_id, provider_id")
        .bind(runtime)
        .bind(state)
        .bind(now)
        .fetch_optional(&mut *conn)
        .await?;
    let Some((environment, provider)) = changed else {
        return Ok(false);
    };
    if let Some(environment) = environment {
        crate::queries::jobs::stamp_environment(conn, environment, now).await?;
    }
    if let Some(provider) = provider {
        sqlx::query("UPDATE providers SET updated_at = GREATEST(updated_at, $2) WHERE id = $1")
            .bind(provider)
            .bind(now)
            .execute(conn)
            .await?;
    }
    Ok(true)
}

/// Retains on a runtime the mode a requested change asked for, so that no start reopens it while the change has
/// not been verified; `None` clears it, which only a verified replacement or a plan that chose open does.
pub async fn require(conn: &mut Conn, runtime: Id, mode: Option<&str>) -> Result<(), DbError> {
    sqlx::query("UPDATE runtimes SET network_required = $2 WHERE id = $1")
        .bind(runtime)
        .bind(mode)
        .execute(conn)
        .await?;
    Ok(())
}

/// The mode retained on a runtime, when one is.
pub async fn required(conn: &mut Conn, runtime: Id) -> Result<Option<String>, DbError> {
    let found: Option<Option<String>> = sqlx::query_scalar::<_, Option<String>>(
        "SELECT network_required FROM runtimes WHERE id = $1",
    )
    .bind(runtime)
    .fetch_optional(conn)
    .await?;
    Ok(found.flatten())
}
