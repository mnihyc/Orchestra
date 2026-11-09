//! Stored grouping identities for the native reader's bounded selected resources.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_access_contract::permissions::ResourceKind;
use orch_types::Id;

/// Display provenance, separate from a resource's operational bindings and permission page.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    /// The real resource family.
    pub kind: String,
    /// The actual selected resource.
    pub id: Id,
    /// First owner of the already selected native resource.
    pub owner_id: Option<Id>,
    /// Its native current label, used only by the authorized writer/audit producer.
    pub name: String,
    /// The existing optimistic concurrency value.
    pub revision: i64,
    /// Its current settings and sharing home.
    pub home_id: Option<Id>,
    /// A retained manager choice, independent of later machine reports/imports.
    pub home_overridden_at: Option<DateTime<Utc>>,
    /// The existing native origin, when the resource belongs to a machine.
    pub origin_machine_id: Option<Id>,
    /// An owned MCP child follows this environment instead of being a flat resource.
    pub container_environment_id: Option<Id>,
}

/// Read only subjects already selected by their ordinary/native scope producer.
/// The API supplies protected references; this query grants no visibility or availability.
pub async fn among(conn: &mut Conn, subjects: &[(String, Id)]) -> Result<Vec<Row>, DbError> {
    if subjects.is_empty() {
        return Ok(Vec::new());
    }
    let (kinds, ids): (Vec<_>, Vec<_>) = subjects.iter().cloned().unzip();
    Ok(sqlx::query_as(include_str!("grouping.sql"))
        .bind(kinds)
        .bind(ids)
        .fetch_all(conn)
        .await?)
}

fn table(kind: ResourceKind) -> (&'static str, &'static str) {
    match kind {
        ResourceKind::Environment => ("environments", "home_node_id"),
        ResourceKind::Provider => ("providers", "home_id"),
        ResourceKind::Agent => ("agents", "home_id"),
        ResourceKind::Host => ("hosts", "home_id"),
        ResourceKind::Machine => ("machines", "home_id"),
        ResourceKind::Template => ("env_templates", "home_id"),
        ResourceKind::McpServer => ("mcp_servers", "home_id"),
        ResourceKind::Mount => ("mounts", "home_id"),
        ResourceKind::Skill => ("skills", "home_id"),
    }
}

/// Pin the actual native resource; a node's physical Machine precedes its Host facade.
pub async fn lock(conn: &mut Conn, kind: ResourceKind, id: Id) -> Result<bool, DbError> {
    if kind == ResourceKind::Host {
        let machine: Option<Id> = sqlx::query_scalar("SELECT machine_id FROM hosts WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        let Some(machine) = machine else {
            return Ok(false);
        };
        if !super::lock_resource(conn, "machine", machine).await? {
            return Ok(false);
        }
    }
    if !super::lock_resource(conn, kind.as_str(), id).await? {
        return Ok(false);
    }
    if kind == ResourceKind::Machine {
        sqlx::query("SELECT id FROM hosts WHERE machine_id=$1 FOR UPDATE")
            .bind(id)
            .fetch_all(&mut *conn)
            .await?;
    }
    Ok(true)
}

/// The manager's explicit home choice uses the existing row and keeps owner/runtime identity.
/// Call after native source/destination locks, authorization and the first-effect callback.
pub async fn set_home(
    conn: &mut Conn,
    kind: ResourceKind,
    id: Id,
    home: Id,
    org: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let (table, column) = table(kind);
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET {column}=$2,org_id=$3,home_overridden_at=$4,revision=revision+1,updated_at=$4 WHERE id=$1"
    )))
    .bind(id)
    .bind(home)
    .bind(org)
    .bind(at)
    .execute(&mut *conn)
    .await?;
    match kind {
        ResourceKind::Host => {
            sqlx::query("UPDATE machines SET home_id=$2,org_id=$3,home_overridden_at=$4,updated_at=$4 WHERE id=(SELECT machine_id FROM hosts WHERE id=$1)")
                .bind(id).bind(home).bind(org).bind(at).execute(conn).await?;
        }
        ResourceKind::Machine => {
            sqlx::query("UPDATE hosts SET home_id=$2,org_id=$3,home_overridden_at=$4,revision=revision+1,updated_at=$4 WHERE machine_id=$1")
                .bind(id).bind(home).bind(org).bind(at).execute(conn).await?;
        }
        _ => {}
    }
    Ok(())
}

/// Existing assignments in canonical order for comparison and the shared tag namespace check.
pub async fn tag_ids(conn: &mut Conn, kind: ResourceKind, id: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT tag_id FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2 ORDER BY tag_id")
        .bind(kind.as_str()).bind(id).fetch_all(conn).await?)
}

/// Replace associations atomically under the real native resource row lock.
pub async fn set_tags(
    conn: &mut Conn,
    kind: ResourceKind,
    id: Id,
    tags: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM resource_tags WHERE resource_kind=$1 AND resource_id=$2")
        .bind(kind.as_str())
        .bind(id)
        .execute(&mut *conn)
        .await?;
    sqlx::query("INSERT INTO resource_tags(resource_kind,resource_id,tag_id) SELECT $1,$2,assigned.tag_id FROM unnest($3::uuid[]) AS assigned(tag_id)")
        .bind(kind.as_str()).bind(id).bind(tags).execute(&mut *conn).await?;
    let (table, _) = table(kind);
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "UPDATE {table} SET revision=revision+1,updated_at=$2 WHERE id=$1"
    )))
    .bind(id)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(())
}
