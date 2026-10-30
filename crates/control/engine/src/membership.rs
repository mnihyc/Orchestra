//! Ending a membership: what the person owned alone passed to the first owner of the org it lies in, what they owned with others left to the others, and what is on their machines unregistered; a member's move to another domain; what waits while a thread's first owner is suspended or may not use its provider.
//!
//! The ownership layer supplies the transfer plan. Runtime cleanup and pending work are settled
//! beside it in the caller's transaction; a personal machine is never removed with its registration.

use crate::{Engine, EngineError};
use chrono::{DateTime, Utc};
use orch_api_contract::access::ObjectRef;
use orch_api_contract::ownership::HoldingsResource;
use orch_db::queries::resources::{self, AccessFacts};
use orch_db::{Conn, DbError};
use orch_types::{Id, Json};
use std::collections::{HashMap, HashSet};

mod actions;
pub use actions::{EndError, Leaving};

/// Pending work cancelled by an ending; running turns are left to finish.
#[derive(Debug, Default, serde::Serialize)]
pub struct Cancelled {
    /// Inputs still waiting for their author.
    pub inputs: usize,
    /// Requests and approvals still pending for that author.
    pub approvals: usize,
}

impl Engine {
    /// Cancels the leaver's pending work in this exact tree after the action's guard accepted it.
    pub async fn cancel_member_work_in(
        &self,
        conn: &mut Conn,
        root: Id,
        user: Id,
        by: Id,
        at: orch_types::Time,
    ) -> Result<Cancelled, EngineError> {
        let facts = resources::resources_in_root(conn, root).await?;
        let ids: Vec<_> = facts.iter().map(|fact| fact.id).collect();
        let approvals: Vec<Id>=sqlx::query_scalar("SELECT a.id FROM approvals a WHERE a.state='pending'
            AND (a.requester_user_id=$2 OR (a.kind='harness' AND EXISTS(SELECT 1 FROM turns u JOIN thread_inputs i ON i.id=u.input_id
                WHERE u.id=a.turn_id AND i.author_id=$2)))
            AND (EXISTS(SELECT 1 FROM nodes n WHERE n.id=COALESCE(a.node_id,a.thread_id,a.approver_node_id) AND n.org_id=$1)
                OR a.object_id=ANY($3)) ORDER BY a.id")
            .bind(root).bind(user).bind(ids).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        let inputs: Vec<(Id, Id)> = sqlx::query_as(
            "SELECT i.thread_id,i.id FROM thread_inputs i JOIN nodes n ON n.id=i.thread_id
            WHERE n.org_id=$1 AND i.author_id=$2 AND i.state='pending' ORDER BY i.thread_id,i.id",
        )
        .bind(root)
        .bind(user)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        let mut touched: Vec<_> = inputs.iter().map(|(thread, _)| *thread).collect();
        for id in &approvals {
            if let Some(row) = orch_db::queries::approvals::approval(conn, *id).await? {
                touched.extend(row.thread_id);
            }
        }
        touched.sort_unstable();
        touched.dedup();
        orch_db::queries::threads::tools::lock_threads(conn, &touched).await?;
        let mut unit = crate::unit::Unit::new(conn, at.get());
        let mut result = Cancelled::default();
        for (thread, id) in inputs {
            let seq = unit.stamp(thread).await?;
            if orch_db::queries::threads::change_input(
                &mut *unit.conn,
                id,
                &["pending"],
                &orch_db::queries::threads::InputChange {
                    state: "cancelled",
                    detail: Some("The author's membership ended."),
                    cancelled_by: Some(by),
                    ..orch_db::queries::threads::InputChange::default()
                },
                seq,
            )
            .await?
            {
                result.inputs += 1;
                unit.wake(thread);
            }
        }
        for id in approvals {
            let Some(row) = orch_db::queries::approvals::approval(&mut *unit.conn, id).await?
            else {
                continue;
            };
            if !orch_db::queries::approvals::cancel_pending(&mut *unit.conn, id, at.get()).await? {
                continue;
            }
            result.approvals += 1;
            crate::notifications::approval_settled(&mut unit, id).await?;
            if let Some(thread) = row.thread_id {
                if let Some(item) = row.item_id {
                    let seq = unit.stamp(thread).await?;
                    orch_db::queries::threads::set_item_status(
                        &mut *unit.conn,
                        item,
                        &["pending"],
                        "cancelled",
                        at.get(),
                        seq,
                    )
                    .await?;
                }
                if row.kind == "harness" {
                    if let (Some(request_ref), Some(decision), Some(machine)) = (
                        row.native_ref.clone(),
                        crate::locks::refusal_with_note(&row, "The requester's membership ended."),
                        self.harness_machine_for(&mut *unit.conn, thread).await?,
                    ) {
                        let body = orch_link_contract::commands::ApprovalResolve {
                            thread_id: thread,
                            request_ref,
                            decision: serde_json::from_value(decision)?,
                        };
                        crate::archive::persist(
                            &mut *unit.conn,
                            at.get(),
                            crate::commands::Outgoing::new(
                                crate::ids::approval_resolve(id, 0),
                                machine,
                                orch_link_contract::commands::SerialKey::Thread(thread),
                                &body,
                            )?,
                        )
                        .await?;
                    }
                    self.after_approval_left_pending(&mut unit, thread).await?;
                }
                unit.wake(thread);
            }
        }
        unit.finish().await?;
        Ok(result)
    }
}

/// A registration the ending will forget, without changing its machine or its files.
pub struct Registration {
    /// The named resource.
    pub object: ObjectRef,
    /// The person's machine.
    pub machine: Id,
    /// Bound threads, including hidden ones.
    pub threads: i64,
}

/// The resource and pending-work portion of a membership-ending preview.
pub struct Holdings {
    /// Managed resources owned alone.
    pub resources: Vec<HoldingsResource>,
    /// What must be unregistered, even if it has another owner.
    pub registrations: Vec<Registration>,
    /// Managed resources left with their other owners.
    pub shared_resources: i64,
    /// Pending requests and approvals in this tree.
    pub pending: i64,
}

/// Resources and work affected by ending this exact membership.
pub async fn holdings(
    conn: &mut Conn,
    root: Id,
    user: Id,
    at: DateTime<Utc>,
) -> Result<Holdings, EngineError> {
    let owned = resources::owned_by(conn, root, user, at).await?;
    let facts = resources::resources_in_root(conn, root).await?;
    let registered = resources::registrations_of_member(conn, root, user).await?;
    let mut ids: Vec<_> = owned
        .iter()
        .filter(|row| !matches!(row.object_kind.as_str(), "node" | "document"))
        .map(|row| row.object_id)
        .collect();
    ids.extend(registered.iter().map(|row| row.id));
    ids.sort_unstable();
    ids.dedup();
    let counts = usage_counts(conn, &ids, user, at, &facts).await?;
    let mut resources = Vec::new();
    let mut shared_resources = 0;
    for row in owned
        .iter()
        .filter(|row| !matches!(row.object_kind.as_str(), "node" | "document"))
    {
        if registered
            .iter()
            .any(|registered| registered.id == row.object_id)
        {
            continue;
        }
        if row.others > 0 {
            shared_resources += 1;
            continue;
        }
        let Some(fact) = facts.iter().find(|fact| fact.id == row.object_id) else {
            continue;
        };
        let (threads, shared_with) = counts.get(&row.object_id).copied().unwrap_or_default();
        resources.push(HoldingsResource {
            object: named(fact),
            threads,
            shared_with,
            personal_sign_in: fact.subscription && fact.owner_id == Some(user),
        });
    }
    let registrations = registered
        .iter()
        .filter_map(|fact| {
            fact.machine_id.map(|machine| Registration {
                object: named(fact),
                machine,
                threads: counts.get(&fact.id).map_or(0, |counts| counts.0),
            })
        })
        .collect();
    let resource_ids: Vec<_> = facts.iter().map(|fact| fact.id).collect();
    let pending: i64=sqlx::query_scalar("SELECT count(*) FROM approvals a WHERE a.state='pending'
        AND (a.requester_user_id=$2 OR (a.kind='harness' AND EXISTS(SELECT 1 FROM turns u JOIN thread_inputs i ON i.id=u.input_id
            WHERE u.id=a.turn_id AND i.author_id=$2)))
        AND (EXISTS(SELECT 1 FROM nodes n WHERE n.id=COALESCE(a.node_id,a.thread_id,a.approver_node_id) AND n.org_id=$1)
            OR a.object_id=ANY($3))")
        .bind(root).bind(user).bind(resource_ids).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    Ok(Holdings {
        resources,
        registrations,
        shared_resources,
        pending,
    })
}

fn named(fact: &AccessFacts) -> ObjectRef {
    ObjectRef {
        kind: fact.kind.clone(),
        id: fact.id,
        name: fact.name.clone(),
    }
}

async fn usage_counts(
    conn: &mut Conn,
    ids: &[Id],
    user: Id,
    at: DateTime<Utc>,
    facts: &[AccessFacts],
) -> Result<HashMap<Id, (i64, i64)>, EngineError> {
    let rows: Vec<(Id,i64,i64)>=sqlx::query_as("SELECT resource.id,
        (SELECT count(DISTINCT t.node_id) FROM threads t JOIN nodes n ON n.id=t.node_id
            LEFT JOIN environments e ON e.id=t.environment_id
            WHERE n.deleted_at IS NULL AND (t.provider_id=resource.id OR t.agent_id=resource.id OR t.environment_id=resource.id
                OR e.template_id=resource.id
                OR EXISTS(SELECT 1 FROM runtimes r WHERE r.successor_of IS NULL AND r.state NOT IN ('retired','removed')
                    AND (r.environment_id=t.environment_id OR r.provider_id=t.provider_id) AND (r.host_id=resource.id OR r.template_id=resource.id)))) AS threads,
        (SELECT count(DISTINCT (p.holder_kind,p.holder_id,p.holder_role)) FROM permission_entries p WHERE p.object_id=resource.id
            AND p.removed_at IS NULL AND (p.expires_at IS NULL OR p.expires_at>$3)
            AND p.permission<>'access.own' AND NOT (p.holder_kind='user' AND p.holder_id=$2)) AS shared_with
        FROM unnest($1::uuid[]) resource(id)")
        .bind(ids).bind(user).bind(at).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let mut counts: HashMap<_, _> = rows
        .into_iter()
        .map(|(id, threads, shared)| (id, (threads, shared)))
        .collect();
    let skills: Vec<Id> = facts
        .iter()
        .filter(|fact| ids.contains(&fact.id) && fact.kind == "skill")
        .map(|fact| fact.id)
        .collect();
    if !skills.is_empty() {
        // A skill reaches a thread through settings and its starter, never through the binding:
        // the threads counted are those whose turns were given one of its versions.
        let given: Vec<(Id, i64)> = sqlx::query_as(
            "SELECT v.skill_id,count(DISTINCT t.thread_id)
               FROM turns t CROSS JOIN LATERAL unnest(t.skill_versions) given(version)
               JOIN skill_versions v ON v.id=given.version JOIN nodes n ON n.id=t.thread_id
              WHERE t.skill_versions IS NOT NULL AND v.skill_id=ANY($1) AND n.deleted_at IS NULL
              GROUP BY v.skill_id",
        )
        .bind(&skills)
        .fetch_all(&mut *conn)
        .await
        .map_err(DbError::from)?;
        for (skill, threads) in given {
            if let Some(count) = counts.get_mut(&skill) {
                count.0 = threads;
            }
        }
    }
    let scoped: HashSet<_> = facts
        .iter()
        .filter(|fact| {
            ids.contains(&fact.id) && matches!(fact.kind.as_str(), "mount" | "mcp_server")
        })
        .map(|fact| fact.id)
        .collect();
    if scoped.is_empty() {
        return Ok(counts);
    }
    // These resources are selected by scopes rather than the binding's three direct references.
    // Resolve the same map/lock rules over one hierarchy/settings read, and count each bound
    // thread once even if both a template and a scope select the server.
    let bindings: Vec<(Id,Json,Option<Json>,i64)>=sqlx::query_as("SELECT e.home_node_id,e.config,t.spec,count(DISTINCT th.node_id)
        FROM environments e JOIN threads th ON th.environment_id=e.id JOIN nodes n ON n.id=th.node_id
        LEFT JOIN env_templates t ON t.id=e.template_id WHERE e.deleted_at IS NULL AND n.deleted_at IS NULL GROUP BY e.id,t.spec")
        .fetch_all(&mut *conn).await.map_err(DbError::from)?;
    let homes: Vec<_> = bindings.iter().map(|(home, _, _, _)| *home).collect();
    let nodes = orch_db::queries::hierarchy::with_ancestors(conn, &homes).await?;
    let node_ids: Vec<_> = nodes.iter().map(|node| node.id).collect();
    let keys = [
        orch_settings_contract::registry::keys::ENV_MOUNTS,
        orch_settings_contract::registry::keys::MCP_ENABLED,
    ];
    let settings = orch_settings::merge::Rows::new(
        orch_db::queries::settings::rows_on_chain(
            conn,
            &node_ids,
            Some(&keys.iter().map(|key| (*key).to_owned()).collect::<Vec<_>>()),
        )
        .await?,
    );
    for (home, config, template, threads) in bindings {
        let Some(home) = nodes
            .iter()
            .find(|node| node.id == home && node.deleted_at.is_none())
        else {
            continue;
        };
        let chain: Result<Vec<_>, EngineError> = home
            .path
            .iter()
            .map(|id| {
                let node = nodes
                    .iter()
                    .find(|node| node.id == *id && node.deleted_at.is_none())
                    .ok_or_else(|| {
                        EngineError::unreadable("a binding's settings home is unavailable")
                    })?;
                let kind: orch_access_contract::roles::NodeKind = node
                    .kind
                    .parse()
                    .map_err(|_| EngineError::unreadable("a binding's home has an unknown kind"))?;
                Ok(orch_settings::merge::Link {
                    id: *id,
                    scope: kind.into(),
                })
            })
            .collect();
        let Ok(chain) = chain else {
            continue;
        };
        let own = orch_settings::merge::OwnValues {
            scope: orch_settings_contract::registry::Scope::Environment,
            values: config,
            revision: 0,
        };
        let stack = orch_settings::merge::Stack {
            rows: &settings,
            chain: &chain,
            own: Some(&own),
        };
        let mut used: HashSet<Id> = template
            .as_ref()
            .and_then(|template| template.get("mcp_server_ids"))
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|id| id.as_str().and_then(|id| id.parse().ok()))
            .collect();
        for key in keys {
            let def = orch_settings_contract::registry::get(key).ok_or_else(|| {
                EngineError::unreadable("a resource selection setting is missing")
            })?;
            if let Some(value) = orch_settings::merge::resolve_key(def, &stack, None).value {
                for (id, value) in value.as_object().into_iter().flatten() {
                    if let Ok(id) = id.parse::<Id>() {
                        if value == &Json::Bool(false) {
                            used.remove(&id);
                        } else {
                            used.insert(id);
                        }
                    }
                }
            }
        }
        for id in used.intersection(&scoped) {
            if let Some(count) = counts.get_mut(id) {
                count.0 += threads;
            }
        }
    }
    Ok(counts)
}
