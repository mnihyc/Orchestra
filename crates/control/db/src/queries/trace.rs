//! Permanent event-time audit scope, captured names and minimal deletion lifecycle facts.

use super::hierarchy::{AuditCursor, AuditRow};
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

tokio::task_local! { static RETAINED_READ: Vec<Id>; }

/// Lets existing stored-content readers build only explicitly authorized retained objects.
///
/// A scope opened inside another adds to it: what the request was admitted to read (a document and the
/// node it is beside, say) stays admitted for every part of its work, which may admit more for a part.
pub async fn read_scope<T>(ids: &[Id], work: impl std::future::Future<Output = T>) -> T {
    let mut admitted = RETAINED_READ.try_with(Clone::clone).unwrap_or_default();
    for id in ids {
        if !admitted.contains(id) {
            admitted.push(*id);
        }
    }
    RETAINED_READ.scope(admitted, work).await
}

/// Whether this exact object was admitted for a read-only retained-content request.
#[must_use]
pub fn retained_read(id: Id) -> bool {
    RETAINED_READ
        .try_with(|ids| ids.contains(&id))
        .unwrap_or(false)
}

/// Facts that may be captured without retaining content or credentials.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// The name at this event, where still available.
    pub name: Option<String>,
    /// The hierarchy node at the act.
    pub node: Option<Id>,
    /// The event-time scope, never reconstructed from an old sentence.
    pub scope: Vec<Id>,
    /// The root org of the actual event-time place.
    pub org: Option<Id>,
}

/// Captures names and scope while the action's transaction still owns the object.
pub async fn facts(
    conn: &mut Conn,
    kind: &str,
    id: Option<Id>,
    node: Option<Id>,
    details: &Json,
) -> Result<Facts, DbError> {
    let mut found = Facts {
        node,
        ..Facts::default()
    };
    if let Some(id) = id {
        let row: Option<(String, Option<Id>)> = match kind {
            "node" | "org" | "project" | "topic" | "thread" => {
                sqlx::query_as("SELECT name,id FROM nodes WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "document" => {
                sqlx::query_as("SELECT title,node_id FROM documents WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "document_container" => {
                sqlx::query_as("SELECT name,node_id FROM document_containers WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "environment" => {
                sqlx::query_as("SELECT name,home_node_id FROM environments WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "provider" => {
                sqlx::query_as("SELECT name,home_id FROM providers WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "agent" => {
                sqlx::query_as("SELECT name,home_id FROM agents WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "template" => {
                sqlx::query_as("SELECT name,home_id FROM env_templates WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "mount" => {
                sqlx::query_as("SELECT name,home_id FROM mounts WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "host" => {
                sqlx::query_as("SELECT name,home_id FROM hosts WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "mcp" | "mcp_server" => {
                sqlx::query_as("SELECT name,home_id FROM mcp_servers WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "skill" => {
                sqlx::query_as("SELECT name,home_id FROM skills WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            "user" => {
                sqlx::query_as("SELECT display_name,NULL::uuid FROM users WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            _ => None,
        };
        if let Some((name, place)) = row {
            found.name = Some(name);
            if place.is_some() {
                found.node = place;
            }
        }
    }
    if found.name.is_none() {
        found.name = details
            .get("name")
            .and_then(Json::as_str)
            .map(str::to_owned);
    }
    let mut nodes: Vec<Id> = found.node.into_iter().collect();
    for key in [
        "from",
        "from_node",
        "source_node",
        "old_node",
        "from_topic",
        "source_topic",
    ] {
        if let Some(value) = details.get(key) {
            let value = value
                .as_str()
                .or_else(|| value.get("id").and_then(Json::as_str));
            if let Some(id) = value.and_then(|v| v.parse::<Id>().ok()) {
                nodes.push(id);
            }
        }
    }
    found.org = match found.node {
        Some(id) => sqlx::query_scalar::<_, Option<Id>>("SELECT org_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten(),
        None => None,
    };
    let paths: Vec<Vec<Id>> = sqlx::query_scalar("SELECT path FROM nodes WHERE id=ANY($1)")
        .bind(&nodes)
        .fetch_all(conn)
        .await?;
    found.scope = paths.into_iter().flatten().collect();
    found.scope.sort_unstable();
    found.scope.dedup();
    Ok(found)
}

/// Minimal permanent lifecycle metadata; no foreign key to the disappearing object.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Life {
    /// The current deletion identity, never inferred from a clock time.
    pub deletion_id: Option<Id>,
    /// The last known name.
    pub last_name: String,
    /// The current deletion time.
    pub deleted_at: Option<DateTime<Utc>>,
    /// Who deleted it.
    pub deleted_by: Option<Id>,
    /// Scheduled purge day, where known.
    pub purge_at: Option<DateTime<Utc>>,
    /// Confirmed purge time.
    pub purged_at: Option<DateTime<Utc>>,
    /// Its retained or current home.
    pub home_node_id: Option<Id>,
}

/// One permanent lifecycle record.
pub async fn life(conn: &mut Conn, kind: &str, id: Id) -> Result<Option<Life>, DbError> {
    Ok(sqlx::query_as("SELECT deletion_id,last_name,deleted_at,deleted_by,purge_at,purged_at,home_node_id FROM audit_object_state WHERE object_kind=$1 AND object_id=$2")
        .bind(kind).bind(id).fetch_optional(conn).await?)
}

/// Updates only the tiny lifecycle record alongside the action that changed the thing.
pub async fn changed(
    conn: &mut Conn,
    entry: &crate::audit::Entry<'_>,
    facts: &Facts,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let Some(id) = entry.object_id else {
        return Ok(());
    };
    let canonical = match entry.object_kind {
        "org" | "project" | "topic" | "thread" => "node",
        other => other,
    };
    let tracked = matches!(
        canonical,
        "node"
            | "document"
            | "document_container"
            | "environment"
            | "provider"
            | "agent"
            | "host"
            | "mount"
            | "template"
            | "mcp"
            | "skill"
    );
    let Some(name) = facts.name.as_deref().filter(|_| tracked) else {
        return Ok(());
    };
    let home = if canonical == "node" {
        sqlx::query_scalar::<_, Option<Id>>("SELECT parent_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?
            .flatten()
    } else {
        facts.node
    };
    sqlx::query("INSERT INTO audit_object_state(object_kind,object_id,last_name,home_node_id) VALUES($1,$2,$3,$4) ON CONFLICT(object_kind,object_id) DO UPDATE SET last_name=EXCLUDED.last_name,home_node_id=EXCLUDED.home_node_id")
        .bind(canonical).bind(id).bind(name).bind(home).execute(&mut *conn).await?;
    let deleting = matches!(
        entry.action,
        "node.deleted"
            | "document.deleted"
            | "environment.deleted"
            | "provider.deleted"
            | "agent.deleted"
            | "host.deleted"
            | "mount.deleted"
            | "template.deleted"
            | "mcp.deleted"
            | "skill.deleted"
    );
    let actor = match entry.actor {
        crate::audit::Actor::User(id) => Some(id),
        _ => None,
    };
    if deleting {
        let group = entry
            .details
            .get("deletion_id")
            .and_then(Json::as_str)
            .and_then(|v| v.parse::<Id>().ok());
        let purge = entry
            .details
            .get("purge_at")
            .and_then(Json::as_str)
            .and_then(|v| v.parse::<DateTime<Utc>>().ok());
        sqlx::query("UPDATE audit_object_state SET deleted_at=$3,deleted_by=$4,purge_at=$5,purged_at=NULL,purged_by=NULL,deletion_id=$6 WHERE object_kind=$1 AND object_id=$2")
            .bind(canonical).bind(id).bind(now).bind(actor).bind(purge).bind(group).execute(&mut *conn).await?;
        if canonical == "node" {
            sqlx::query("INSERT INTO audit_object_state(object_kind,object_id,last_name,deletion_id,deleted_at,deleted_by,purge_at,home_node_id) SELECT 'node',id,name,deletion_id,deleted_at,deleted_by,$3,parent_id FROM nodes WHERE $1=ANY(path) AND deletion_id=$2 ON CONFLICT(object_kind,object_id) DO UPDATE SET last_name=EXCLUDED.last_name,deletion_id=EXCLUDED.deletion_id,deleted_at=EXCLUDED.deleted_at,deleted_by=EXCLUDED.deleted_by,purge_at=EXCLUDED.purge_at,purged_at=NULL,purged_by=NULL")
                .bind(id).bind(group).bind(purge).execute(&mut *conn).await?;
        }
    } else if matches!(
        entry.action,
        "node.restored" | "document.restored" | "environment.restored"
    ) {
        sqlx::query("UPDATE audit_object_state SET deletion_id=NULL,deleted_at=NULL,deleted_by=NULL,purge_at=NULL,purged_at=NULL,purged_by=NULL WHERE object_kind=$1 AND object_id=$2")
            .bind(canonical).bind(id).execute(&mut *conn).await?;
        if canonical == "node" {
            sqlx::query("UPDATE audit_object_state s SET deletion_id=NULL,deleted_at=NULL,deleted_by=NULL,purge_at=NULL FROM nodes n WHERE s.object_kind='node' AND s.object_id=n.id AND $1=ANY(n.path) AND n.deleted_at IS NULL").bind(id).execute(&mut *conn).await?;
        }
    } else if entry.action == "environment.purged" {
        mark_purged(conn, canonical, &[id], actor, now).await?;
    }
    Ok(())
}

/// Records actual purge before the owning full rows can be collected.
pub async fn mark_purged(
    conn: &mut Conn,
    kind: &str,
    ids: &[Id],
    by: Option<Id>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE audit_object_state SET purged_at=COALESCE(purged_at,$4),purged_by=COALESCE(purged_by,$3) WHERE object_kind=$1 AND object_id=ANY($2)")
        .bind(kind).bind(ids).bind(by).bind(now).execute(conn).await?;
    Ok(())
}

/// Captures a legacy object's last name before its full row is successfully collected.
pub async fn before_purge(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    let kind = match table {
        "nodes" => "node",
        "documents" => "document",
        "document_containers" => "document_container",
        "environments" => "environment",
        "providers" => "provider",
        "agents" => "agent",
        "env_templates" => "template",
        "mounts" => "mount",
        "mcp_servers" => "mcp",
        "skills" => "skill",
        _ => return Ok(()),
    };
    for id in ids {
        let facts = facts(conn, kind, Some(*id), None, &Json::Null).await?;
        if let Some(name) = facts.name {
            sqlx::query("INSERT INTO audit_object_state(object_kind,object_id,last_name,home_node_id) VALUES($1,$2,$3,$4) ON CONFLICT(object_kind,object_id) DO UPDATE SET last_name=EXCLUDED.last_name")
                .bind(kind).bind(id).bind(name).bind(facts.node).execute(&mut *conn).await?;
        }
    }
    mark_purged(conn, kind, ids, None, now).await
}

/// Only active references block collection; completed private records are removed in its savepoint.
pub(crate) fn live_reference_condition(table: &str) -> &'static str {
    match table {
        "approvals" => " AND x.state='pending'",
        "leases" => " AND x.closed_at IS NULL",
        "jobs" => {
            " AND (x.state IN ('queued','running') OR x.plan->>'restore_recovery_required'='true' OR EXISTS(SELECT 1 FROM environments scratch WHERE scratch.id::text=x.plan->>'scratch_cleanup' AND scratch.removal_phase IS DISTINCT FROM 'purged'))"
        }
        "preflights" => " AND x.expires_at>$3",
        "runtimes" => " AND x.state<>'removed'",
        "terminal_sessions" | "browser_sessions" | "display_sessions" => " AND x.state<>'closed'",
        _ => "",
    }
}

/// Removes completed references and their pins only in the savepoint that collects their owner.
pub(crate) async fn collect_history(
    conn: &mut Conn,
    table: &str,
    ids: &[Id],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    if table == "nodes" {
        crate::queries::storage::release_input_pins(conn, ids, now).await?;
        crate::queries::storage::release_schedule_pins(conn, ids, now).await?;
        sqlx::query("DELETE FROM thread_approval_rules WHERE thread_id=ANY($1)")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM approvals WHERE state<>'pending' AND (node_id=ANY($1) OR thread_id=ANY($1) OR requester_thread_id=ANY($1) OR approver_node_id=ANY($1) OR turn_id IN(SELECT id FROM turns WHERE thread_id=ANY($1)) OR item_id IN(SELECT id FROM items WHERE thread_id=ANY($1)) OR tool_item_id IN(SELECT id FROM items WHERE thread_id=ANY($1)))")
            .bind(ids).execute(&mut *conn).await?;
        sqlx::query("DELETE FROM leases WHERE closed_at IS NOT NULL AND thread_id=ANY($1)")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        sqlx::query("UPDATE providers SET credential_trial_turn_id=NULL WHERE credential_trial_turn_id IN(SELECT id FROM turns WHERE thread_id=ANY($1) AND completed_at IS NOT NULL)")
            .bind(ids).execute(&mut *conn).await?;
        let snapshots: Vec<Id> =
            sqlx::query_scalar("SELECT id FROM snapshots WHERE thread_id=ANY($1) FOR UPDATE")
                .bind(ids)
                .fetch_all(&mut *conn)
                .await?;
        crate::queries::storage::release_snapshot_pins(conn, &snapshots, now).await?;
        let ends:Vec<Vec<u8>>=sqlx::query_scalar("SELECT end_manifest_blob FROM turns WHERE thread_id=ANY($1) AND end_manifest_blob IS NOT NULL FOR UPDATE")
            .bind(ids).fetch_all(&mut *conn).await?;
        release_history_blobs(conn, ends, now).await?;
    }
    if table == "environments" {
        sqlx::query("DELETE FROM browser_sessions WHERE environment_id=ANY($1) AND state='closed'")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM display_sessions WHERE environment_id=ANY($1) AND state='closed'")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM runtimes WHERE environment_id=ANY($1) AND state='removed'")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        let manifests: Vec<Vec<u8>> = sqlx::query_scalar(
            "SELECT manifest_blob FROM environments WHERE id=ANY($1) AND manifest_blob IS NOT NULL",
        )
        .bind(ids)
        .fetch_all(&mut *conn)
        .await?;
        release_history_blobs(conn, manifests, now).await?;
    }
    if table == "providers" {
        sqlx::query("DELETE FROM leases WHERE provider_id=ANY($1) AND closed_at IS NOT NULL")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        sqlx::query("DELETE FROM runtimes WHERE provider_id=ANY($1) AND state='removed'")
            .bind(ids)
            .execute(&mut *conn)
            .await?;
    }
    if table == "machines" {
        sqlx::query(
            "UPDATE runtimes SET machine_id=NULL WHERE state='removed' AND machine_id=ANY($1)",
        )
        .bind(ids)
        .execute(&mut *conn)
        .await?;
        sqlx::query(
            "DELETE FROM leases WHERE closed_at IS NOT NULL AND recipient_machine_id=ANY($1)",
        )
        .bind(ids)
        .execute(&mut *conn)
        .await?;
    }
    let target = match table {
        "nodes" => Some("node_id"),
        "environments" => Some("environment_id"),
        "providers" => Some("provider_id"),
        _ => None,
    };
    if let Some(column) = target {
        let statement = format!(
            "DELETE FROM jobs x WHERE x.\"{column}\"=ANY($1) AND x.state NOT IN ('queued','running') AND COALESCE(x.plan->>'restore_recovery_required','false')<>'true' AND NOT EXISTS(SELECT 1 FROM environments scratch WHERE scratch.id::text=x.plan->>'scratch_cleanup' AND scratch.removal_phase IS DISTINCT FROM 'purged')"
        );
        sqlx::query(sqlx::AssertSqlSafe(statement))
            .bind(ids)
            .execute(&mut *conn)
            .await?;
        if table != "providers" {
            let statement = format!(
                "DELETE FROM preflights WHERE \"{column}\"=ANY($1) AND expires_at<=$2 RETURNING plan"
            );
            let plans: Vec<Json> = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
                .bind(ids)
                .bind(now)
                .fetch_all(&mut *conn)
                .await?;
            for plan in plans {
                if let Some(blob) = plan.get("plan_blob") {
                    let hash = serde_json::from_value::<orch_types::Hash>(blob.clone())
                        .map_err(|error| DbError::Other(sqlx::Error::Decode(error.into())))?;
                    crate::queries::storage::unpin_blobs(conn, &[hash], now).await?;
                }
            }
        }
    }
    if matches!(table, "nodes" | "environments") {
        let column = if table == "nodes" {
            "thread_id"
        } else {
            "environment_id"
        };
        let statement = format!(
            "DELETE FROM terminal_sessions WHERE \"{column}\"=ANY($1) AND state='closed' RETURNING recording_blob"
        );
        let recordings: Vec<Option<Vec<u8>>> = sqlx::query_scalar(sqlx::AssertSqlSafe(statement))
            .bind(ids)
            .fetch_all(&mut *conn)
            .await?;
        release_history_blobs(conn, recordings.into_iter().flatten().collect(), now).await?;
    }
    let secrets = match table {
        "nodes" => Some("setting"),
        "environments" => Some("environment"),
        "providers" => Some("provider"),
        "mcp_servers" => Some("mcp_server"),
        _ => None,
    };
    if let Some(kind) = secrets {
        sqlx::query("DELETE FROM secrets WHERE owner_kind=$1 AND owner_id=ANY($2)")
            .bind(kind)
            .bind(ids)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

async fn release_history_blobs(
    conn: &mut Conn,
    raw: Vec<Vec<u8>>,
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    for bytes in raw {
        let hash = orch_types::Hash::from_slice(&bytes)
            .map_err(|error| DbError::Other(sqlx::Error::Decode(error.to_string().into())))?;
        crate::queries::storage::unpin_blobs(conn, &[hash], now).await?;
    }
    Ok(())
}

/// The current deletion identity of a node, or unknown legacy group.
pub async fn node_deletion(conn: &mut Conn, id: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar::<_, Option<Id>>("SELECT deletion_id FROM nodes WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?
            .flatten(),
    )
}

/// Captures a new delete event's group without borrowing the current group of an older event.
pub async fn deletion_details(
    conn: &mut Conn,
    entry: &mut crate::audit::Entry<'_>,
) -> Result<(), DbError> {
    if !matches!(
        entry.action,
        "node.deleted" | "document.deleted" | "environment.deleted"
    ) {
        return Ok(());
    }
    let Some(id) = entry.object_id else {
        return Ok(());
    };
    let group = if entry.action == "node.deleted" {
        node_deletion(conn, id).await?
    } else {
        Some(orch_types::new_id())
    };
    entry.details["deletion_id"] =
        serde_json::to_value(group).map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
    Ok(())
}

/// A permanent row enriched without changing older audit entries.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct Row {
    /// This human act used its browser session's explicit As admin switch.
    pub as_admin: bool,
    /// The original permanent row.
    #[sqlx(flatten)]
    pub base: AuditRow,
    /// Captured event name.
    pub object_name: Option<String>,
    /// The person for whom an agent acted, when recorded.
    pub for_user_id: Option<Id>,
    /// Common request identity.
    pub request_id: Option<String>,
}

/// Intersected server-side trail filters, applied before paging.
#[derive(Debug, Clone, Default)]
pub struct Filter {
    /// Direct actor person.
    pub actor: Option<Id>,
    /// Also select an agent's frozen responsibility for that person.
    pub include_agents: bool,
    /// Exact action.
    pub action: Option<String>,
    /// Exact catalogue actions belonging to the chosen kind.
    pub actions: Option<Vec<String>>,
    /// Event-time place.
    pub node: Option<Id>,
    /// Inclusive start.
    pub from: Option<DateTime<Utc>>,
    /// Inclusive end.
    pub to: Option<DateTime<Utc>>,
}

/// The same permanent reader for list and both export formats.
pub async fn page(
    conn: &mut Conn,
    filter: &Filter,
    after: Option<AuditCursor>,
    limit: i64,
    newest: bool,
) -> Result<Vec<Row>, DbError> {
    let order = if newest { "DESC" } else { "ASC" };
    let comparison = if newest { "<" } else { ">" };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!("SELECT a.as_admin,a.id,a.at,a.actor_kind,a.actor_id,a.action,a.history,a.override,a.object_kind,a.object_id,a.node_id,a.summary,a.details,host(a.ip) AS ip,a.object_name,a.for_user_id,a.request_id FROM audit_log a WHERE ($1::uuid IS NULL OR (a.actor_kind='user' AND a.actor_id=$1) OR ($2 AND a.actor_kind='agent' AND a.for_user_id=$1)) AND ($3::text IS NULL OR a.action=$3) AND ($4::text[] IS NULL OR a.action=ANY($4)) AND ($5::uuid IS NULL OR $5=ANY(a.scope_node_ids)) AND ($6::timestamptz IS NULL OR a.at >= $6) AND ($7::timestamptz IS NULL OR a.at <= $7) AND ($8::uuid IS NULL OR (a.at,a.id) {comparison} ($9,$8)) ORDER BY a.at {order},a.id {order} LIMIT $10")))
        .bind(filter.actor).bind(filter.include_agents).bind(filter.action.as_deref()).bind(filter.actions.as_deref()).bind(filter.node).bind(filter.from).bind(filter.to).bind(after.map(|a|a.id)).bind(after.map(|a|a.at)).bind(limit).fetch_all(conn).await?)
}

fn named_id(value: &Json) -> Option<Id> {
    value
        .as_str()
        .or_else(|| value.get("id").and_then(Json::as_str))
        .and_then(|value| value.parse().ok())
}

/// Captures only display names of exact referenced choices and holders, never their values or content.
pub async fn snapshot_names(
    conn: &mut Conn,
    entry: &mut crate::audit::Entry<'_>,
) -> Result<(), DbError> {
    for (key, kind) in [
        ("provider", "provider"),
        ("provider_id", "provider"),
        ("from_provider", "provider"),
        ("to_provider", "provider"),
        ("agent", "agent"),
        ("agent_id", "agent"),
        ("environment", "environment"),
        ("environment_id", "environment"),
        ("from_environment", "environment"),
        ("to_environment", "environment"),
        ("owner", "user"),
        ("user", "user"),
        ("user_id", "user"),
    ] {
        if let Some(id) = entry.details.get(key).and_then(named_id) {
            let known = facts(conn, kind, Some(id), None, &Json::Null).await?;
            if let Some(name) = known.name {
                entry.details[format!("{}_name", key.trim_end_matches("_id"))] = Json::String(name);
            }
        }
    }
    if let Some(holder) = entry.details.get("holder") {
        let kind = holder.get("kind").and_then(Json::as_str);
        let id = named_id(holder);
        let name = match (kind, id) {
            (Some("user"), Some(id)) => {
                sqlx::query_scalar::<_, String>("SELECT display_name FROM users WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            (Some("group"), Some(id)) => {
                sqlx::query_scalar::<_, String>("SELECT name FROM groups WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            (Some("node" | "agents" | "members"), Some(id)) => {
                sqlx::query_scalar::<_, String>("SELECT name FROM nodes WHERE id=$1")
                    .bind(id)
                    .fetch_optional(&mut *conn)
                    .await?
            }
            (Some("role"), _) => holder.get("role").and_then(Json::as_str).map(str::to_owned),
            _ => None,
        };
        if let Some(name) = name {
            entry.details["holder_name"] = Json::String(name);
        }
    }
    if entry.action == "approval.decided"
        && let Some(id) = entry.details.get("approval").and_then(named_id)
        && let Some((request, decision)) = sqlx::query_as::<_, (Json, Option<Json>)>(
            "SELECT request,decision FROM approvals WHERE id=$1",
        )
        .bind(id)
        .fetch_optional(conn)
        .await?
        && let Some(option) = decision
            .as_ref()
            .and_then(|value| value.get("option_id"))
            .and_then(Json::as_str)
    {
        entry.details["option_id"] = Json::String(option.to_owned());
        let name = request
            .get("options")
            .and_then(Json::as_array)
            .and_then(|options| {
                options
                    .iter()
                    .find(|value| value.get("id").and_then(Json::as_str) == Some(option))
            })
            .and_then(|option| option.get("label"))
            .and_then(Json::as_str);
        if let Some(name) = name {
            entry.details["option_name"] = Json::String(name.to_owned());
        }
    }
    Ok(())
}
