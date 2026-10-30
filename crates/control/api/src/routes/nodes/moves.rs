//! Moving a project, or an org that lies in another, under another org of its own tree, and the plan that says what the move would change before anything does.
//!
//! A move changes the parent of what is moved and nothing else of it: its
//! owners, its page and everything inside it stay, and so do its members.
//! What changes is what it inherits, which the plan lists: the settings that
//! resolve differently there, how many people start or stop to hold a role
//! in it from above, and whose address changes. Who may move is the engine's
//! to say, at both ends; what is checked here is the shape of the tree: the
//! tree stays one, an org or a project never goes into itself or below
//! itself, the depth the system allows of orgs and of projects one inside
//! another, and the tree not being flat. A project goes under an org or
//! under another project, a topic to a project or a folder, with the rows
//! of the ledger and of both rollups that name it following it, and not
//! while a fork, a switch or a move holds one of its threads.

use super::{NodeKind, Placed, actor_name, before_name, placed, record, shown, stale, view};
use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::PagePreview;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::PlanBlocker;
use orch_api_contract::nodes::{
    MoveAddresses, MovePreviewBody, MoveRoles, MoveSetting, Node, NodeMoveBody, NodeMovePlan,
    NodeOrSystem, NodeRef, Org, OrgMove, OrgMovePreview, Project, ProjectMove, ProjectMovePreview,
    Topic, TopicMove, TopicMovePreview,
};
use orch_authz::{Act, Policy, Principal, epoch, orgs};
use orch_db::Conn;
use orch_db::queries::hierarchy::{self, NodeRow};
use orch_settings::{Place, resolve_all};
use orch_types::Id;
use serde_json::json;
use std::collections::HashMap;

mod review;
pub mod thread;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    thread::mount(registry);
    registry
        .on(ProjectMovePreview, project_preview)
        .on(ProjectMove, project_move)
        .on(OrgMovePreview, org_preview)
        .on(OrgMove, org_move)
        .on(TopicMovePreview, topic_preview)
        .on(TopicMove, topic_move);
}

/// What stops a move for the shape of the tree.
#[derive(Debug, Clone)]
struct Blocker {
    code: ErrorCode,
    message: String,
    parent: Option<Id>,
    limit: Option<u32>,
    /// A thread that is held, and what it is doing.
    held: Option<(Id, String, String)>,
}

/// A move as it was found to be: both ends read, and what stops it.
struct Plan {
    moved: NodeRow,
    from: Option<NodeRow>,
    to: NodeRow,
    blockers: Vec<Blocker>,
}

/// What taking a thing of this kind away takes: Manage of it.
fn permission_to_move(kind: NodeKind) -> Permission {
    match kind {
        NodeKind::Org => Permission::OrgManage,
        NodeKind::Project => Permission::ProjectManage,
        NodeKind::Topic => Permission::TopicManage,
        NodeKind::Thread => Permission::ThreadManage,
    }
}

/// What making a thing of this kind where it goes takes, for a place of the kind `target`; nothing for a
/// place it could not go into, which the plan says.
fn permission_to_receive(kind: NodeKind, target: Option<&str>) -> Option<Permission> {
    match (kind, target?) {
        (NodeKind::Org, "org") => Some(Permission::OrgManage),
        (NodeKind::Project, "org") => Some(Permission::OrgInteract),
        (NodeKind::Project, "project") => Some(Permission::ProjectInteract),
        (NodeKind::Topic, "project") => Some(Permission::ProjectCreateTopics),
        (NodeKind::Thread, "topic") => Some(Permission::TopicStartThreads),
        _ => None,
    }
}

/// The kinds of place a thing of this kind may be moved under, and the words for them.
fn places_for(kind: NodeKind) -> (&'static [&'static str], &'static str) {
    match kind {
        NodeKind::Org => (&["org"], "it can only be moved under an org"),
        NodeKind::Project => (
            &["org", "project"],
            "it can only be moved under an org or a project",
        ),
        NodeKind::Topic => (
            &["project"],
            "it can only be moved to a project or a folder",
        ),
        NodeKind::Thread => (
            &["topic"],
            "a main thread moves only to another topic of its tree",
        ),
    }
}

/// Asks the engine for both ends of the move.
async fn may_move(
    conn: &mut Conn,
    principal: &Principal,
    kind: NodeKind,
    node: Id,
    target: Id,
) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        node,
        exact(permission_to_move(kind)),
        Act::Use,
        Policy::default(),
    )
    .await?;
    // Where it goes is asked about only if it is a place the thing could go
    // into: nothing is asked about another, which the plan says.
    let target_kind = hierarchy::with_ancestors(conn, &[target])
        .await?
        .into_iter()
        .find(|row| row.id == target)
        .map(|row| row.kind);
    let Some(permission) = permission_to_receive(kind, target_kind.as_deref()) else {
        return Ok(());
    };
    authorization::require(
        conn,
        principal,
        target,
        exact(permission),
        Act::Use,
        Policy::default(),
    )
    .await
}

/// Reads both ends and finds what stops the move.
async fn plan_of(
    ctx: &Ctx,
    conn: &mut Conn,
    kind: NodeKind,
    node: Id,
    target: Id,
) -> ApiResult<Plan> {
    let rows = hierarchy::with_ancestors(conn, &[node, target]).await?;
    let find = |id: Id| {
        rows.iter()
            .find(|row| row.id == id && row.deleted_at.is_none())
            .cloned()
            .ok_or_else(ApiError::not_found)
    };
    let moved = find(node)?;
    let to = find(target)?;
    let from = moved
        .parent_id
        .and_then(|parent| rows.iter().find(|row| row.id == parent).cloned());
    let mut blockers = Vec::new();
    let mut wrong = |why: &str| {
        blockers.push(Blocker {
            code: ErrorCode::NodeWrongParent,
            message: why.to_owned(),
            parent: Some(to.id),
            limit: None,
            held: None,
        });
    };
    let (places, words) = places_for(kind);
    if !places.contains(&to.kind.as_str()) {
        wrong(words);
    }
    if moved.parent_id.is_none() {
        wrong("a root org does not become a department");
    }
    if to.org_id != moved.org_id {
        wrong("it can only be moved within its own tree");
    }
    if to.path.contains(&moved.id) {
        wrong("it cannot be moved into itself or below itself");
    }
    if kind == NodeKind::Org && blockers.is_empty() {
        let inside = hierarchy::descendants(conn, moved.id).await?;
        let kinds: HashMap<Id, &str> = inside
            .iter()
            .chain(std::iter::once(&moved))
            .map(|row| (row.id, row.kind.as_str()))
            .collect();
        let above = to
            .path
            .iter()
            .filter(|place| {
                rows.iter()
                    .any(|row| row.id == **place && row.kind == "org")
            })
            .count();
        // The deepest org of what is moved, counted from the moved org itself.
        let deepest = inside
            .iter()
            .filter(|row| row.kind == "org")
            .map(|row| {
                row.path
                    .iter()
                    .skip_while(|place| **place != moved.id)
                    .filter(|place| kinds.get(*place) == Some(&"org"))
                    .count()
            })
            .max()
            .unwrap_or(1)
            .max(1);
        let limit = ctx.app.config.limits.org_depth;
        if !orgs::fits_depth(above + deepest, limit) {
            blockers.push(Blocker {
                code: ErrorCode::OrgTooDeep,
                message: format!("orgs may lie in one another {limit} deep at most"),
                parent: None,
                limit: Some(limit),
                held: None,
            });
        }
        if orgs::limits(conn, moved.org_id)
            .await
            .map_err(ApiError::internal)?
            .is_some_and(|limits| limits.flat)
        {
            blockers.push(Blocker {
                code: ErrorCode::OrgFlat,
                message: "this tree holds no org inside it".to_owned(),
                parent: None,
                limit: None,
                held: None,
            });
        }
    }
    // An address names exactly one org: an org is not moved where another org holds its short name.
    if kind == NodeKind::Org
        && blockers.is_empty()
        && moved.parent_id != Some(to.id)
        && let Some(own) = hierarchy::org(conn, moved.id).await?
        && let Some(sibling) = hierarchy::org_with_slug(conn, Some(to.id), &own.slug).await?
        && sibling.id != moved.id
    {
        blockers.push(Blocker {
            code: ErrorCode::ConflictExists,
            message: format!(
                "{} holds the short name {} there already, and two orgs of one place cannot share an address",
                sibling.name, own.slug
            ),
            parent: Some(to.id),
            limit: None,
            held: None,
        });
    }
    if kind == NodeKind::Project && blockers.is_empty() {
        // The projects on the way down to the new place, and the most that lie one inside another
        // within what is moved, counted from the moved project itself.
        let above = hierarchy::projects_on_the_way(conn, to.id).await?;
        let within = hierarchy::deepest_chain_of_projects(conn, moved.id).await?;
        let limit = ctx.app.config.limits.project_depth;
        if !orgs::fits_project_depth(above + within, limit) {
            blockers.push(Blocker {
                code: ErrorCode::ProjectTooDeep,
                message: format!("projects may lie in one another {limit} deep at most"),
                parent: None,
                limit: Some(limit),
                held: None,
            });
        }
    }
    if kind == NodeKind::Topic
        && blockers.is_empty()
        && let Some(held) = hierarchy::threads_held_in_topic(conn, moved.id)
            .await?
            .into_iter()
            .next()
    {
        blockers.push(Blocker {
            code: ErrorCode::ThreadNotIdle,
            message: "a fork, a switch or a move holds a thread of this topic".to_owned(),
            parent: None,
            limit: None,
            held: Some((held.thread, held.state, held.job)),
        });
    }
    Ok(Plan {
        moved,
        from,
        to,
        blockers,
    })
}

/// The refusal of a move for what stops it.
async fn refuse(conn: &mut Conn, blocker: &Blocker) -> ApiError {
    let error = ApiError::new(blocker.code, blocker.message.clone());
    let error = match blocker.parent {
        Some(parent) => match refusals::named_node(conn, parent).await {
            Ok(parent) => error.with("parent", parent),
            Err(failed) => return failed,
        },
        None => error,
    };
    let error = match blocker.limit {
        Some(limit) => error.with("limit", limit),
        None => error,
    };
    match &blocker.held {
        Some((thread, state, job)) => error
            .with("thread", thread)
            .with("state", state)
            .with("job", job),
        None => error,
    }
}

/// What moving a node would change.
async fn plan_reply(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &Plan,
) -> ApiResult<NodeMovePlan> {
    let from = plan
        .moved
        .parent_id
        .ok_or_else(|| ApiError::internal("a node that is moved has a parent"))?;
    let refs = view::refs(
        conn,
        principal,
        &[plan.moved.id, from, plan.to.id],
        Policy::default(),
    )
    .await?;
    let reference = |id: Id| -> ApiResult<NodeRef> {
        refs.get(&id)
            .cloned()
            .ok_or_else(|| ApiError::internal("a node of a plan was not read"))
    };
    let mut blockers: Vec<PlanBlocker> = plan
        .blockers
        .iter()
        .map(|blocker| PlanBlocker {
            code: blocker.code.as_str().to_owned(),
            message: blocker.message.clone(),
            missing: None,
        })
        .collect();
    let (settings, roles) = if plan.blockers.is_empty() {
        (
            if plan.moved.kind == "thread" || from == plan.to.id {
                Vec::new()
            } else {
                changed_settings(ctx, conn, principal, plan).await?
            },
            roles_changed(conn, plan, from, ctx.now).await?,
        )
    } else {
        (
            Vec::new(),
            MoveRoles {
                gaining: 0,
                losing: 0,
            },
        )
    };
    let members = if plan.moved.kind == "org" {
        hierarchy::org_facts(conn, &[plan.moved.id], ctx.now)
            .await?
            .first()
            .map_or(0, |facts| facts.member_count)
    } else {
        0
    };
    let pages = hierarchy::pages_following_defaults(conn, plan.moved.id).await?;
    if blockers.is_empty() && plan.moved.kind == "org" && ctx.app.config.limits.org_depth == 0 {
        blockers.push(PlanBlocker {
            code: "org.too_deep".to_owned(),
            message: "no org may lie in another".to_owned(),
            missing: None,
        });
    }
    Ok(NodeMovePlan {
        preflight_id: Id::nil(),
        stamp: String::new(),
        revision: plan.moved.revision,
        expires_at: orch_types::Time::new(ctx.now),
        node: reference(plan.moved.id)?,
        from: reference(from)?,
        to: reference(plan.to.id)?,
        access: if blockers.is_empty() && from != plan.to.id && plan.moved.kind != "thread" {
            review::effects(ctx, conn, principal, plan.moved.id, plan.to.id).await?
        } else {
            PagePreview {
                pages,
                orgs: None,
                gaining: Vec::new(),
                losing: Vec::new(),
                more: 0,
            }
        },
        settings,
        roles,
        addresses: MoveAddresses {
            members,
            resources: 0,
        },
        blockers,
    })
}

/// Actual resolved values and inherited locks before and after a rolled-back move, including own values overridden by a new lock.
async fn changed_settings(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    plan: &Plan,
) -> ApiResult<Vec<MoveSetting>> {
    let before: HashMap<_, _> = resolve_all(conn, Place::Node(plan.moved.id))
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|view| {
            (
                view.key(),
                (
                    view.value().cloned(),
                    view.lock_above().map(|lock| lock.node),
                ),
            )
        })
        .collect();
    sqlx::query("SAVEPOINT move_settings_preview")
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    hierarchy::move_under(conn, plan.moved.id, plan.to.id, ctx.now).await?;
    let after = resolve_all(conn, Place::Node(plan.moved.id)).await;
    sqlx::query("ROLLBACK TO SAVEPOINT move_settings_preview")
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    sqlx::query("RELEASE SAVEPOINT move_settings_preview")
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    let after = after.map_err(ApiError::internal)?;
    let mut changed = Vec::new();
    let mut named: Vec<Id> = Vec::new();
    for view in &after {
        let (was, old_lock) = before.get(view.key()).cloned().unwrap_or_default();
        if was.as_ref() != view.value() || old_lock != view.lock_above().map(|lock| lock.node) {
            if let Some(lock) = view.lock_above()
                && let Some(node) = lock.node
            {
                named.push(node);
            }
            changed.push((
                view.key().to_owned(),
                was,
                view.value().cloned(),
                view.lock_above().map(|lock| lock.node),
            ));
        }
    }
    let names = view::refs(conn, principal, &named, Policy::default()).await?;
    Ok(changed
        .into_iter()
        .map(|(key, from, to, lock)| MoveSetting {
            key,
            from,
            to,
            locked_by: lock.map(|node| match node {
                None => NodeOrSystem::SYSTEM,
                Some(node) => names
                    .get(&node)
                    .cloned()
                    .map_or(NodeOrSystem::SYSTEM, NodeOrSystem::Node),
            }),
        })
        .collect())
}

/// How many people hold a role in the moved node from places that it gains,
/// and from places that it loses.
async fn roles_changed(
    conn: &mut Conn,
    plan: &Plan,
    from: Id,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<MoveRoles> {
    let rows = hierarchy::with_ancestors(conn, &[from, plan.to.id]).await?;
    let path_of = |id: Id| {
        rows.iter()
            .find(|row| row.id == id)
            .map(|row| row.path.clone())
            .unwrap_or_default()
    };
    let (old, new) = (path_of(from), path_of(plan.to.id));
    let before = hierarchy::people_holding_roles(conn, &old, now).await?;
    let after = hierarchy::people_holding_roles(conn, &new, now).await?;
    let count = |of: &[Id], not_in: &[Id]| {
        i64::try_from(of.iter().filter(|person| !not_in.contains(person)).count())
            .unwrap_or(i64::MAX)
    };
    let (gaining, losing) = (count(&after, &before), count(&before, &after));
    Ok(MoveRoles { gaining, losing })
}

async fn preview(ctx: &Ctx, kind: NodeKind, body: &MovePreviewBody) -> ApiResult<NodeMovePlan> {
    let node = ctx.id("id")?;
    let (current, mut conn, principal) = super::action(ctx).await?;
    let ctx = &current;
    let (_, found) = placed(&mut conn, node).await?;
    if found != kind {
        return Err(ApiError::not_found());
    }
    may_move(&mut conn, &principal, kind, node, body.parent_id).await?;
    let plan = plan_of(ctx, &mut conn, kind, node, body.parent_id).await?;
    let reply = plan_reply(ctx, &mut conn, &principal, &plan).await?;
    let reply = review::keep(ctx, &mut conn, &principal, reply).await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    Ok(reply)
}

/// Shows what moving a project would change.
async fn project_preview(ctx: Ctx, (): (), body: MovePreviewBody) -> ApiResult<NodeMovePlan> {
    preview(&ctx, NodeKind::Project, &body).await
}

/// Shows what moving an org that lies in another would change.
async fn org_preview(ctx: Ctx, (): (), body: MovePreviewBody) -> ApiResult<NodeMovePlan> {
    preview(&ctx, NodeKind::Org, &body).await
}

async fn carry_out(ctx: &Ctx, kind: NodeKind, body: &NodeMoveBody) -> ApiResult<Node> {
    let node = ctx.id("id")?;
    let (current, mut tx, principal) = super::action(ctx).await?;
    let ctx = &current;
    let (place, found) = placed(&mut tx, node).await?;
    if found != kind {
        return Err(ApiError::not_found());
    }
    may_move(&mut tx, &principal, kind, node, body.parent_id).await?;
    // Moves of one tree are made one after the other, so that none is planned on a tree that
    // another is changing.
    hierarchy::lock_tree_for_moves(&mut tx, place.org).await?;
    let plan = plan_of(ctx, &mut tx, kind, node, body.parent_id).await?;
    if let Some(blocker) = plan.blockers.first() {
        return Err(refuse(&mut tx, blocker).await);
    }
    let before = shown(ctx, &mut tx, &principal, node).await?;
    if plan.moved.revision != body.expected_revision {
        return Err(stale(&before));
    }
    if let Some(preflight) = body.preflight_id {
        let current = plan_reply(ctx, &mut tx, &principal, &plan).await?;
        if let Some(current) = review::changed(ctx, &mut tx, &principal, preflight, current).await?
        {
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Err(ApiError::new(ErrorCode::ConflictStaleRevision, "The move's subtree or consequences changed. Nothing moved; review the current plan.").with("current", current));
        }
    }
    let from = plan
        .moved
        .parent_id
        .ok_or_else(|| ApiError::internal("a node that is moved has a parent"))?;
    if from != plan.to.id {
        hierarchy::lock(&mut tx, node).await?;
        hierarchy::move_under(&mut tx, node, plan.to.id, ctx.now).await?;
        epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
        let who = actor_name(ctx)?;
        let summary = format!(
            "{who} moved {} from {} to {}",
            before_name(&before),
            plan.from
                .as_ref()
                .map_or("its old place", |row| row.name.as_str()),
            plan.to.name
        );
        let mut details = json!({ "from": from, "to": plan.to.id });
        if kind == NodeKind::Topic {
            // Its use goes with it: the rows of the ledger and of both rollups that name it name
            // the project it lies in now, in this transaction, and no figure changes by that.
            let rows = hierarchy::repoint_usage(&mut tx, node, plan.to.id).await?;
            details["usage_rows"] = json!({
                "ledger": rows.ledger,
                "daily": rows.daily,
                "monthly": rows.monthly,
            });
            // A topic has a feed of its own besides its tree's.
            hierarchy::stamp(&mut tx, node).await?;
        }
        record(
            ctx,
            &mut tx,
            orch_db::audit::action!(AuditAction::NodeMoved),
            place,
            &summary,
            details.clone(),
        )
        .await?;
        // The old place learns of it too: a line in its Activity.
        record(
            ctx,
            &mut tx,
            AuditAction::NodeMoved,
            Placed {
                id: from,
                org: place.org,
            },
            &summary,
            details,
        )
        .await?;
        hierarchy::stamp_subtree(&mut tx, node).await?;
    }
    let moved = shown(ctx, &mut tx, &principal, node).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(moved)
}

/// Moves a project under another org, or into another project, of its root org.
async fn project_move(ctx: Ctx, (): (), body: NodeMoveBody) -> ApiResult<Project> {
    match carry_out(&ctx, NodeKind::Project, &body).await? {
        Node::Project(project) => Ok(project),
        _ => Err(ApiError::internal("a project was moved and is not one")),
    }
}

/// Moves an org that lies in another under another org of the same tree.
async fn org_move(ctx: Ctx, (): (), body: NodeMoveBody) -> ApiResult<Org> {
    match carry_out(&ctx, NodeKind::Org, &body).await? {
        Node::Org(org) => Ok(org),
        _ => Err(ApiError::internal("an org was moved and is not one")),
    }
}

/// Shows what moving a topic to another project or folder would change.
async fn topic_preview(ctx: Ctx, (): (), body: MovePreviewBody) -> ApiResult<NodeMovePlan> {
    preview(&ctx, NodeKind::Topic, &body).await
}

/// Moves a topic, with its threads, to another project or folder of its root org.
async fn topic_move(ctx: Ctx, (): (), body: NodeMoveBody) -> ApiResult<Topic> {
    match carry_out(&ctx, NodeKind::Topic, &body).await? {
        Node::Topic(topic) => Ok(topic),
        _ => Err(ApiError::internal("a topic was moved and is not one")),
    }
}
