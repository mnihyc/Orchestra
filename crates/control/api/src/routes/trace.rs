//! Permanent audit rows projected with current content and restoration authority.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::Ctx,
};
use orch_access_contract::{
    audit::{AuditAction, AuditKind},
    permissions::{Permission, ResourceKind},
};
use orch_api_contract::nodes::{
    AuditActor, AuditEntry, AuditObject, AuditObjectState, NodeKind, NodeRef,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::{
    Conn,
    queries::{
        access, hierarchy,
        trace::{self, Row},
    },
};
use orch_types::{Id, Time};

/// The exact catalogue actions for a kind filter; unknown acts receive no guessed kind.
pub(crate) fn actions(kind: Option<AuditKind>) -> Option<Vec<String>> {
    kind.map(|kind| {
        AuditAction::ALL
            .iter()
            .filter(|a| a.kind() == kind)
            .map(|a| a.as_str().to_owned())
            .collect()
    })
}

/// The current authority that would restore this object, independent of audit permission.
pub(crate) async fn restore_allowed(
    conn: &mut Conn,
    principal: &Principal,
    kind: &str,
    id: Id,
) -> bool {
    if matches!(kind, "node" | "org" | "project" | "topic" | "thread") {
        let Ok(rows) = hierarchy::with_ancestors(conn, &[id]).await else {
            return false;
        };
        let Some(row) = rows.iter().find(|r| r.id == id) else {
            return false;
        };
        if crate::middleware::authorization::refusals::node_kind(&row.kind).is_none() {
            return false;
        }
        if !principal.caller().reaches(Some(&row.path)) {
            return false;
        }
        if let Some(parent) = row.parent_id {
            trace::read_scope(
                &[parent],
                authorization::require(
                    conn,
                    principal,
                    parent,
                    authorization::MANAGE,
                    Act::Delete,
                    Policy::default(),
                ),
            )
            .await
            .is_ok()
        } else {
            principal.caller().permits(Permission::AccessOwn)
                && (principal.is_system_admin()
                    || access::owns_page(conn, id, principal.user())
                        .await
                        .unwrap_or(false))
        }
    } else if kind == "document" {
        let Ok(Some(doc)) = orch_db::queries::documents::get(conn, id).await else {
            return false;
        };
        trace::read_scope(
            &[id, doc.node_id],
            orch_authz::documents::check_all(
                conn,
                principal,
                id,
                &[Permission::DocManage],
                Act::Delete,
                orch_authz::documents::State::AsItWas,
                Policy::default(),
            ),
        )
        .await
        .is_ok_and(|d| d.is_allowed())
    } else if kind == "environment" {
        orch_engine::jobs::retained::authority(conn, principal, id)
            .await
            .is_ok_and(|(manager, admin)| manager || admin)
    } else {
        false
    }
}

async fn opens(conn: &mut Conn, principal: &Principal, kind: &str, id: Id) -> bool {
    if matches!(kind, "node" | "org" | "project" | "topic" | "thread") {
        authorization::require(
            conn,
            principal,
            id,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await
        .is_ok()
    } else if kind == "document" {
        authorization::require_document(conn, principal, id, Permission::DocView, Policy::default())
            .await
            .is_ok()
    } else {
        let resource = match kind {
            "environment" => Some((ResourceKind::Environment, Permission::EnvironmentView)),
            "provider" => Some((ResourceKind::Provider, Permission::ProviderView)),
            "host" => Some((ResourceKind::Host, Permission::HostView)),
            "machine" => Some((ResourceKind::Machine, Permission::MachineView)),
            "agent" => Some((ResourceKind::Agent, Permission::AgentView)),
            "template" => Some((ResourceKind::Template, Permission::TemplateView)),
            "mount" => Some((ResourceKind::Mount, Permission::MountView)),
            "mcp" | "mcp_server" => Some((ResourceKind::McpServer, Permission::McpView)),
            "skill" => Some((ResourceKind::Skill, Permission::SkillView)),
            _ => None,
        };
        match resource {
            Some((kind, permission)) => authorization::require_resource(
                conn,
                principal,
                kind,
                id,
                permission,
                Policy::default(),
            )
            .await
            .is_ok(),
            None => false,
        }
    }
}

fn canonical(kind: &str) -> &str {
    match kind {
        "org" | "project" | "topic" | "thread" => "node",
        other => other,
    }
}

/// Projects the exact selected rows; audit authority supplies no object content right.
pub(crate) async fn entries(
    ctx: &Ctx,
    conn: &mut Conn,
    rows: &[Row],
) -> ApiResult<Vec<AuditEntry>> {
    let principal = authorization::principal(ctx, conn).await?;
    let mut lifecycle = std::collections::HashMap::new();
    for row in rows {
        if let Some(id) = row.base.object_id {
            let key = (canonical(&row.base.object_kind).to_owned(), id);
            if let std::collections::hash_map::Entry::Vacant(slot) = lifecycle.entry(key) {
                slot.insert(trace::life(conn, canonical(&row.base.object_kind), id).await?);
            }
        }
    }
    let mut ids: Vec<Id> = rows
        .iter()
        .flat_map(|r| [r.base.actor_id, r.for_user_id])
        .flatten()
        .collect();
    ids.extend(
        lifecycle
            .values()
            .flatten()
            .filter_map(|life| life.deleted_by),
    );
    let people = authorization::refusals::user_refs(conn, &ids).await?;
    let node_ids: Vec<Id> = rows
        .iter()
        .flat_map(|r| {
            [
                r.base.node_id,
                (r.base.actor_kind == "agent")
                    .then_some(r.base.actor_id)
                    .flatten(),
            ]
        })
        .flatten()
        .collect();
    let names =
        crate::routes::nodes::view::refs(conn, &principal, &node_ids, Policy::default()).await?;
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let base = &row.base;
        // A retained or newer writer's action must not hide the rest of the trail.
        // Preserve its key and facts, with no guessed catalogue classification.
        let action = base.action.parse::<AuditAction>().ok();
        let life = base.object_id.and_then(|id| {
            lifecycle
                .get(&(canonical(&base.object_kind).to_owned(), id))
                .cloned()
                .flatten()
        });
        let state = if life.as_ref().is_some_and(|l| l.purged_at.is_some()) {
            AuditObjectState::Purged
        } else if life.as_ref().is_some_and(|l| l.deleted_at.is_some()) {
            AuditObjectState::Deleted
        } else {
            AuditObjectState::Live
        };
        let known = base.object_id;
        let can_open = state == AuditObjectState::Live
            && match known {
                Some(id) => opens(conn, &principal, &base.object_kind, id).await,
                None => false,
            };
        let mut permitted = state == AuditObjectState::Deleted
            && match known {
                Some(id) => restore_allowed(conn, &principal, &base.object_kind, id).await,
                None => false,
            };
        let can_restore = if base.object_kind == "environment" && permitted {
            let promise = match known {
                Some(id) => orch_db::queries::environments::retained::removal(conn, id).await?,
                None => None,
            };
            permitted = promise
                .as_ref()
                .is_some_and(|r| r.removal_phase.is_some() && r.purged_at.is_none());
            promise.is_some_and(|r| r.removal_phase.as_deref() == Some("kept"))
        } else {
            let in_window = life.as_ref().is_some_and(|l| {
                l.purge_at
                    .or_else(|| {
                        l.deleted_at.map(|at| {
                            at + chrono::Duration::days(
                                i64::try_from(ctx.app.config.retention.soft_delete_days)
                                    .unwrap_or(i64::MAX / 86_400),
                            )
                        })
                    })
                    .is_some_and(|at| at > ctx.now)
            });
            let parent_live = match life.as_ref().and_then(|l| l.home_node_id) {
                Some(parent) => hierarchy::deleted_node(conn, parent).await?.is_none(),
                None => true,
            };
            permitted && in_window && parent_live
        };
        let actor = match (base.actor_kind.as_str(), base.actor_id) {
            ("user", Some(id)) => AuditActor::User {
                user: people.get(&id).cloned().ok_or_else(|| {
                    ApiError::internal("the audit actor's retained identity is unavailable")
                })?,
            },
            ("agent", Some(id)) => AuditActor::Agent {
                thread: names.get(&id).cloned().unwrap_or(NodeRef {
                    thread_display: None,
                    id,
                    kind: NodeKind::Thread,
                    name: None,
                    redacted: true,
                }),
                for_user: row.for_user_id.and_then(|id| people.get(&id).cloned()),
            },
            _ => AuditActor::System,
        };
        out.push(AuditEntry {
            as_admin: row.as_admin,
            id: base.id,
            at: Time::new(base.at),
            actor,
            action: base.action.clone(),
            kind: action.map(AuditAction::kind),
            request_id: row.request_id.clone(),
            event_deletion_id: base
                .details
                .get("deletion_id")
                .and_then(|v| v.as_str())
                .and_then(|v| v.parse().ok()),
            object: AuditObject {
                kind: base.object_kind.clone(),
                id: known,
                deletion_id: life.as_ref().and_then(|l| l.deletion_id),
                name: row.object_name.clone(),
                state,
                deleted_at: life.as_ref().and_then(|l| l.deleted_at).map(Time::new),
                deleted_by: life
                    .as_ref()
                    .and_then(|l| l.deleted_by)
                    .and_then(|id| people.get(&id).cloned()),
                purge_at: life.as_ref().and_then(|l| l.purge_at).map(Time::new),
                purged_at: life.as_ref().and_then(|l| l.purged_at).map(Time::new),
                can_open,
                can_inspect: permitted,
                can_restore,
            },
            history: base.history,
            r#override: base.r#override,
            node: base.node_id.and_then(|id| names.get(&id).cloned()),
            summary: base.summary.clone(),
            details: base.details.clone(),
            ip: base.ip.clone(),
        });
    }
    Ok(out)
}
