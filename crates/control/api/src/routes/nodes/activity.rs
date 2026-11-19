//! A place's Activity and its audit trail.
//!
//! The Activity of an org, a project or a topic is its history: the audit
//! rows that are lines of it, about the place and about what lies directly
//! inside it, newest first, minus the lines about anything the caller may not
//! view. A project's takes in its folders: what lies directly inside each of
//! them too. The audit trail of a node is every row about it and about what lies
//! below it, for whoever holds the audit permission there. Both page by the
//! moment and the identifier of the last row they showed.

use super::{NodeKind, invalid, placed, view};
use crate::middleware::authorization::{self, refusals::user_refs};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::access::{HistoryActor, HistoryAgent, HistoryWording};
use orch_api_contract::common::{Page, PageQuery};
use orch_api_contract::nodes::{
    ActivityDepth, ActivityEntry, ActivityKind, ActivityObject, AuditEntry, NodeActivityList,
    NodeActivityListQuery, NodeAuditList, NodeAuditListQuery, NodeRef,
};
use orch_authz::{Act, Policy};
use orch_db::queries::activity;
use orch_db::queries::hierarchy::{AuditCursor, AuditRow};
use orch_types::{Id, Time};
use std::collections::HashSet;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_raw(orch_api_contract::nodes::NodeAuditExport, audit_export);
    registry
        .on(NodeActivityList, activity_list)
        .on(NodeAuditList, audit_list);
}

async fn audit_export(
    ctx: Ctx,
    query: orch_api_contract::nodes::NodeAuditExportQuery,
    _body: axum::body::Body,
) -> ApiResult<axum::response::Response> {
    let node = ctx.id("id")?;
    crate::routes::admin::audit::export_scope(
        ctx,
        orch_api_contract::admin::AdminAuditExportQuery {
            actor: query.actor,
            include_agents: query.include_agents,
            action: query.action,
            kind: query.kind,
            node_id: Some(node),
            from: query.from,
            to: query.to,
            format: query.format,
        },
        true,
    )
    .await
}

fn cursor_text(row: &AuditRow) -> String {
    format!("{}|{}", row.at.timestamp_micros(), row.id)
}

fn cursor_of(text: Option<&str>) -> ApiResult<Option<AuditCursor>> {
    let Some(text) = text else {
        return Ok(None);
    };
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    Ok(Some(AuditCursor {
        at: micros
            .parse::<i64>()
            .ok()
            .and_then(DateTime::<Utc>::from_timestamp_micros)
            .ok_or_else(bad)?,
        id: id.parse().map_err(|_| bad())?,
    }))
}

/// What a line of Activity or of the audit trail needs to name: the people
/// who did things, and the nodes and threads they are about.
struct Names {
    people: std::collections::HashMap<Id, orch_api_contract::users::UserRef>,
    nodes: std::collections::HashMap<Id, orch_api_contract::nodes::NodeRef>,
    objects: std::collections::HashMap<Id, Option<String>>,
}

async fn names(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    rows: &[&AuditRow],
) -> ApiResult<Names> {
    let people: Vec<Id> = rows
        .iter()
        .filter(|row| row.actor_kind == "user")
        .filter_map(|row| row.actor_id)
        .collect();
    let mut nodes: Vec<Id> = rows.iter().filter_map(|row| row.node_id).collect();
    nodes.extend(
        rows.iter()
            .filter(|row| is_node_kind(&row.object_kind))
            .filter_map(|row| row.object_id),
    );
    nodes.extend(
        rows.iter()
            .filter(|row| row.actor_kind == "agent")
            .filter_map(|row| row.actor_id),
    );
    nodes.sort_unstable();
    nodes.dedup();
    let mut objects = std::collections::HashMap::new();
    let docs: Vec<Id> = rows
        .iter()
        .filter(|row| row.object_kind == "document")
        .filter_map(|row| row.object_id)
        .collect();
    for each in orch_authz::documents::listed(conn, principal, &docs, Policy::default())
        .await
        .map_err(ApiError::internal)?
    {
        if each.holds(Permission::DocView) {
            objects.insert(
                each.document.id,
                (!each.document.deleted).then_some(each.document.title),
            );
        }
    }
    for (kind, permission) in [
        ("provider", Permission::ProviderView),
        ("agent", Permission::AgentView),
        ("environment", Permission::EnvironmentView),
        ("host", Permission::HostView),
        ("template", Permission::TemplateView),
        ("mcp", Permission::McpView),
        ("mcp_server", Permission::McpView),
        ("mount", Permission::MountView),
        ("skill", Permission::SkillView),
    ] {
        let ids: Vec<Id> = rows
            .iter()
            .filter(|row| row.object_kind == kind)
            .filter_map(|row| row.object_id)
            .collect();
        if ids.is_empty() {
            continue;
        }
        for each in
            orch_authz::resources::holding(conn, principal, &ids, permission, Policy::default())
                .await
                .map_err(ApiError::internal)?
        {
            objects.insert(each.id, (!each.deleted).then_some(each.name));
        }
    }
    Ok(Names {
        people: user_refs(conn, &people).await?,
        nodes: view::refs(conn, principal, &nodes, Policy::default()).await?,
        objects,
    })
}

/// The lines of a place's Activity.
async fn activity_list(
    ctx: Ctx,
    query: NodeActivityListQuery,
    (): (),
) -> ApiResult<Page<ActivityEntry>> {
    let id = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let mut after = cursor_of(query.cursor.as_deref())?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (_, kind) = placed(&mut conn, id).await?;
    if kind == NodeKind::Thread {
        return Err(invalid(
            "id",
            "a thread has no Activity; its history is in its timeline",
        ));
    }
    let summary = query.depth.unwrap_or_default() == ActivityDepth::Summary;
    let actions: Vec<String> = AuditAction::ALL
        .iter()
        .copied()
        .filter(|action| !summary || meaningful(*action))
        .filter(|action| query.kind.is_empty() || query.kind.contains(&activity_kind(*action)))
        .map(|action| action.as_str().to_owned())
        .collect();
    let batch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let since = query.since.map(Time::get);
    let mut kept: Vec<activity::Row> = Vec::new();
    loop {
        let rows = activity::read(&mut conn, id, since, after, batch, summary, &actions).await?;
        let exhausted = i64::try_from(rows.len()).is_ok_and(|count| count < batch);
        let Some(last) = rows.last() else {
            break;
        };
        after = Some(AuditCursor {
            at: last.base.at,
            id: last.base.id,
        });
        let about: Vec<Id> = rows
            .iter()
            .flat_map(|row| {
                [
                    row.base.node_id,
                    row.node_removed.then_some(row.parent_id).flatten(),
                ]
            })
            .flatten()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        let viewable: HashSet<Id> = orch_authz::nodes::holding(
            &mut conn,
            &principal,
            &about,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|each| each.id)
        .collect();
        let named = names(
            &mut conn,
            &principal,
            &rows.iter().map(|row| &row.base).collect::<Vec<_>>(),
        )
        .await?;
        kept.extend(rows.into_iter().filter(|row| {
            let place_readable = row
                .base
                .node_id
                .is_some_and(|node| viewable.contains(&node))
                || (row.node_removed
                    && row
                        .parent_id
                        .is_some_and(|parent| viewable.contains(&parent)));
            place_readable
                && (!is_separate_object(&row.base.object_kind)
                    || row
                        .base
                        .object_id
                        .is_some_and(|object| named.objects.contains_key(&object)))
        }));
        if kept.len() > limit || exhausted {
            break;
        }
    }
    let more = kept.len() > limit;
    kept.truncate(limit);
    let named = names(
        &mut conn,
        &principal,
        &kept.iter().map(|row| &row.base).collect::<Vec<_>>(),
    )
    .await?;
    let items = kept
        .iter()
        .filter_map(|each| {
            let row = &each.base;
            let node = if each.node_removed {
                NodeRef {
                    thread_display: None,
                    id: row.node_id?,
                    kind: crate::middleware::authorization::refusals::node_kind(&each.node_kind)
                        .unwrap_or(NodeKind::Thread),
                    name: None,
                    redacted: true,
                }
            } else {
                named.nodes.get(&row.node_id?)?.clone()
            };
            let action: AuditAction = row.action.parse().ok()?;
            let object = activity_object(row, &node, &named);
            Some(ActivityEntry {
                at: Time::new(row.at),
                actor: history_actor(row, &named),
                title: object.name.clone().unwrap_or_default(),
                object: Some(object),
                wording: Some(activity_wording(row, action, &node)),
                count: each.count,
                node: node.clone(),
                kind: activity_kind(action),
                action: Some(row.action.clone()),
                thread: (activity_kind(action) != ActivityKind::History
                    && node.kind == NodeKind::Thread)
                    .then_some(node),
                item_id: row
                    .details
                    .get("item_id")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|id| id.parse().ok()),
            })
        })
        .collect();
    Ok(Page {
        next_cursor: more
            .then(|| kept.last().map(|row| cursor_text(&row.base)))
            .flatten(),
        items,
    })
}

fn is_node_kind(kind: &str) -> bool {
    matches!(kind, "node" | "org" | "project" | "topic" | "thread")
}

fn is_separate_object(kind: &str) -> bool {
    matches!(
        kind,
        "document"
            | "provider"
            | "agent"
            | "environment"
            | "host"
            | "template"
            | "mcp"
            | "mcp_server"
            | "mount"
            | "skill"
    )
}

fn activity_kind(action: AuditAction) -> ActivityKind {
    match action {
        AuditAction::AnnouncementSent => ActivityKind::Announcement,
        AuditAction::ApprovalDecided => ActivityKind::ApprovalDecided,
        _ => ActivityKind::History,
    }
}

pub(crate) fn meaningful(action: AuditAction) -> bool {
    // Reads and transport housekeeping stay in Every change; the person's acts and work remain.
    (action.is_history_line()
        && !matches!(
            action,
            AuditAction::TerminalRecordingRead
                | AuditAction::DocumentExported
                | AuditAction::InputScheduleCannotStart
                | AuditAction::NetworkPolicyApplied
                | AuditAction::NetworkFailedClosed
                | AuditAction::UnitHookRan
        ))
        || matches!(
            action,
            AuditAction::AnnouncementSent
                | AuditAction::ApprovalDecided
                | AuditAction::ChatPostedByAgent
                | AuditAction::ThreadSentByAgent
        )
}

/// Name facts come from the same authorized batch as the visible Activity row, never its old text.
fn activity_object(row: &AuditRow, node: &NodeRef, named: &Names) -> ActivityObject {
    if is_separate_object(&row.object_kind) {
        let id = row.object_id.unwrap_or(node.id);
        let name = named.objects.get(&id).cloned().flatten();
        return ActivityObject {
            id,
            kind: if row.object_kind == "mcp" {
                "mcp_server".into()
            } else {
                row.object_kind.clone()
            },
            redacted: name.is_none(),
            name,
        };
    }
    let object = row
        .object_id
        .and_then(|id| named.nodes.get(&id))
        .unwrap_or(node);
    ActivityObject {
        id: object.id,
        kind: match object.kind {
            NodeKind::Org => "org",
            NodeKind::Project => "project",
            NodeKind::Topic => "topic",
            NodeKind::Thread => "thread",
        }
        .to_owned(),
        name: object.name.clone(),
        redacted: object.redacted,
    }
}

/// Every catalogued action has a semantic verb; English audit text is reserved for export.
fn activity_wording(row: &AuditRow, action: AuditAction, node: &NodeRef) -> HistoryWording {
    let special = match row.action.as_str() {
        "topic.status_changed" => Some("status"),
        "role.given" | "role.changed" | "role.removed" => Some("roles"),
        "announcement.sent" | "chat.posted_by_agent" | "thread.sent_by_agent" => Some("said"),
        _ => None,
    };
    if special.is_none()
        && let Some(words) =
            orch_engine::history::wording_of(&row.action, &row.details, Some(node.clone()))
    {
        return words;
    }
    HistoryWording {
        version: 1,
        verb: special
            .unwrap_or(match action.kind() {
                orch_access_contract::audit::AuditKind::Made => "created",
                orch_access_contract::audit::AuditKind::Deleted => "deleted",
                orch_access_contract::audit::AuditKind::Access => "access",
                orch_access_contract::audit::AuditKind::Work => "work",
                orch_access_contract::audit::AuditKind::SignIns => "signIn",
                _ => "changed",
            })
            .to_owned(),
        target: Some(node.clone()),
    }
}

fn history_actor(row: &AuditRow, named: &Names) -> Option<HistoryActor> {
    match (row.actor_kind.as_str(), row.actor_id) {
        ("user", Some(user)) => named.people.get(&user).cloned().map(HistoryActor::User),
        ("agent", Some(thread)) => named
            .nodes
            .get(&thread)
            .cloned()
            .map(|agent| HistoryActor::Agent(HistoryAgent { agent })),
        _ => None,
    }
}

/// The audit trail of a place.
async fn audit_list(ctx: Ctx, query: NodeAuditListQuery, (): ()) -> ApiResult<Page<AuditEntry>> {
    let id = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let after = cursor_of(query.cursor.as_deref())?;
    let mut conn = ctx.app.db.acquire().await?;
    let filter = orch_db::queries::trace::Filter {
        actor: query.actor,
        include_agents: query.include_agents.unwrap_or(false),
        action: query.action,
        actions: crate::routes::trace::actions(query.kind),
        node: Some(id),
        from: query.from.map(Time::get),
        to: query.to.map(Time::get),
    };
    let mut rows = orch_db::queries::trace::page(
        &mut conn,
        &filter,
        after,
        i64::try_from(limit + 1).unwrap_or(i64::MAX),
        true,
    )
    .await?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    Ok(Page {
        next_cursor: more
            .then(|| rows.last().map(|row| cursor_text(&row.base)))
            .flatten(),
        items: crate::routes::trace::entries(&ctx, &mut conn, &rows).await?,
    })
}
