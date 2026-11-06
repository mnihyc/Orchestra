//! Deleting a node with everything in it, bringing it back until it is purged, and handing a node to another owner.
//!
//! A node that is not archived is archived first, in the same transaction;
//! then it and everything under it is marked deleted by one moment, and
//! bringing it back undoes exactly that: what was deleted before stays
//! deleted. An org that lies in another gives its members to the org above
//! first, who keep the role they hold there or are given the one the
//! request names. Nothing is removed until the retention days have passed.
//! The first owner of a node is passed on by the page change that does it.

use super::{Placed, actor_name, invalid, placed, record, view};
use crate::middleware::authorization::{self, MANAGE, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::sign_in::origin;
use chrono::{DateTime, Duration, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::pages as stored;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{ErrorCode, Page, PageQuery};
use orch_api_contract::nodes::{Node, NodeKind, NodeOrSystem};
use orch_api_contract::ownership::{
    DeletedNode, DeletedOrg, NodeDelete, NodeDeleteBody, NodeDeleteCounts, NodeDeletePlan,
    NodeDeletePreview, NodeDeleted, NodeDeletedList, NodeRestore, NodeTransfer, OrgDeletedList,
    RestoreBy, TransferBody,
};
use orch_authz::pages::{self, Target};
use orch_authz::{Act, Policy, epoch};
use orch_db::queries::access;
use orch_db::queries::hierarchy::{self, AuditCursor, DeletedRow};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use serde_json::json;

mod restoring;
mod review;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            NodeDeletePreview,
            crate::key_operations::native::effects::prepare::<review::Preview>,
            node_delete_preview,
        )
        .on(NodeDelete, node_delete)
        .on(NodeDeletedList, deleted_list)
        .on_with_operation(
            NodeRestore,
            crate::key_operations::native::effects::prepare::<restoring::Restore>,
            node_restore,
        )
        .on_snapshot_read_with_optional_operation(
            OrgDeletedList,
            crate::key_operations::native::selected::<super::deleted_orgs::Orgs>,
            org_deleted_list,
        )
        .on_with_operation(
            NodeTransfer,
            crate::routes::transfer::operation::node,
            node_transfer,
        );
}

/// When what was deleted at `at` is gone for good.
pub(super) fn purge_at(at: DateTime<Utc>, days: u64) -> DateTime<Utc> {
    at + Duration::days(i64::try_from(days).unwrap_or(i64::MAX / 86_400))
}

/// The moment before which a deletion has been purged by `now`.
pub(super) fn purged_before(now: DateTime<Utc>, days: u64) -> DateTime<Utc> {
    now - Duration::days(i64::try_from(days).unwrap_or(i64::MAX / 86_400))
}

/// The kind of a stored deletion plan.
const DELETE_PLAN_KIND: &str = "node_delete";
/// For how long a deletion plan may be confirmed: the life of every stored change plan.
const DELETE_PLAN_MINUTES: i64 = 15;

/// What a deletion plan keeps beside what it answered, for the deletion that names it.
#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct KeptPlan {
    /// The node's revision then.
    revision: i64,
    /// The hash of every affected node and document with its revision.
    fingerprint: String,
    /// When what it deleted would have been purged.
    purge_at: DateTime<Utc>,
    /// Original accepted review capability for its later confirmation.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    origins: Vec<orch_authz::CapturedOperation>,
}

/// The hash of a deletion set: changed, added or replaced rows give another, whatever the totals.
fn fingerprint_of(set: &[(String, Id, i64)]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (kind, id, revision) in set {
        hasher.update(format!("{kind}:{id}:{revision}\n").as_bytes());
    }
    hex::encode(hasher.finalize())
}

/// How many of each kind a deletion set holds.
pub(super) fn counts_of(set: &[(String, Id, i64)]) -> NodeDeleteCounts {
    let of = |kind: &str| {
        i64::try_from(set.iter().filter(|(each, _, _)| each == kind).count()).unwrap_or(i64::MAX)
    };
    NodeDeleteCounts {
        orgs: of("org"),
        projects: of("project"),
        topics: of("topic"),
        threads: of("thread"),
        documents: of("document"),
    }
}

/// Who may bring `node` back once it is deleted, as restoring asks: an owner of a root org (or a
/// system admin), else whoever manages its parent.
async fn restore_by(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    node: &hierarchy::NodeRow,
) -> ApiResult<RestoreBy> {
    let Some(parent) = node.parent_id else {
        let refs = view::refs(conn, principal, &[node.id], Policy::default()).await?;
        return Ok(RestoreBy {
            permission: "access.own".to_owned(),
            place: refs
                .get(&node.id)
                .cloned()
                .map_or(NodeOrSystem::SYSTEM, NodeOrSystem::Node),
        });
    };
    let (_, kind) = placed(conn, parent).await?;
    let refs = view::refs(conn, principal, &[parent], Policy::default()).await?;
    Ok(RestoreBy {
        permission: format!("{}.manage", kind_word(kind)),
        place: refs
            .get(&parent)
            .cloned()
            .map_or(NodeOrSystem::SYSTEM, NodeOrSystem::Node),
    })
}

/// The immutable native preparation of the existing deletion review.
struct DeleteReview {
    node: hierarchy::NodeRow,
    set: Vec<(String, Id, i64)>,
    purge_at: DateTime<Utc>,
}

async fn prepare_delete_review(
    ctx: &Ctx,
    conn: &mut Conn,
    id: Id,
    set: &[(String, Id, i64)],
) -> ApiResult<DeleteReview> {
    let rows = hierarchy::with_ancestors(conn, &[id]).await?;
    let node = rows
        .into_iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?;
    Ok(DeleteReview {
        node,
        set: set.to_vec(),
        purge_at: purge_at(ctx.now, ctx.app.config.retention.soft_delete_days),
    })
}

struct SavedReview {
    preflight: orch_db::queries::jobs::forks::PreflightRow,
    review: std::sync::Arc<DeleteReview>,
}

async fn save_delete_review(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    review: std::sync::Arc<DeleteReview>,
    before: impl FnOnce() -> ApiResult<()> + Send,
    inherited: &[orch_authz::CapturedOperation],
) -> ApiResult<SavedReview> {
    let kept = KeptPlan {
        revision: review.node.revision,
        fingerprint: fingerprint_of(&review.set),
        purge_at: review.purge_at,
        origins: {
            let mut origins = inherited.to_vec();
            if crate::key_operations::native::modern(principal.caller()) {
                let current = orch_authz::CapturedOperation::capture(principal.caller())
                    .map_err(ApiError::internal)?;
                if !origins.contains(&current) {
                    origins.push(current);
                }
            }
            origins
        },
    };
    let preflight = orch_db::queries::jobs::forks::PreflightRow {
        id: orch_types::new_id(),
        kind: DELETE_PLAN_KIND.to_owned(),
        node_id: Some(review.node.id),
        environment_id: None,
        created_by: principal.user(),
        plan: serde_json::to_value(&kept).map_err(ApiError::internal)?,
        expires_at: ctx.now + Duration::minutes(DELETE_PLAN_MINUTES),
        created_at: ctx.now,
    };
    before()?;
    orch_db::queries::jobs::forks::save_preflight(conn, &preflight).await?;
    Ok(SavedReview { preflight, review })
}

async fn render_delete_review(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    saved: &SavedReview,
) -> ApiResult<NodeDeletePlan> {
    let review = &saved.review;
    let refs = view::refs(conn, principal, &[review.node.id], Policy::default()).await?;
    Ok(NodeDeletePlan {
        preflight_id: saved.preflight.id,
        expires_at: Time::new(saved.preflight.expires_at),
        node: refs
            .get(&review.node.id)
            .cloned()
            .ok_or_else(|| ApiError::internal("a node to delete has no reference"))?,
        revision: review.node.revision,
        counts: counts_of(&review.set),
        purge_at: Time::new(review.purge_at),
        restore_by: restore_by(conn, principal, &review.node).await?,
    })
}

/// The same native planner remains shared with stale deletion confirmations.
async fn delete_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
    set: &[(String, Id, i64)],
    origins: &[orch_authz::CapturedOperation],
) -> ApiResult<NodeDeletePlan> {
    let review = std::sync::Arc::new(prepare_delete_review(ctx, conn, id, set).await?);
    let saved = save_delete_review(ctx, conn, principal, review, || Ok(()), origins).await?;
    render_delete_review(conn, principal, &saved).await
}

/// Stores a reviewed deletion without deleting anything; full names have their own Read boundary.
async fn node_delete_preview(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::ownership::NodeDeletePreviewResponse> {
    review::run(ctx).await
}

/// Holds a deletion that names a plan to what the plan counted: the plan is the caller's own, of this
/// node and not out of date; the node's revision, every affected node and document with its revision,
/// and the calendar day of the purge are what it showed. The set is locked first, so that nothing joins
/// it before the deletion commits. The plan is used up either way: `None` when it holds, else the plan
/// as it now is, kept for the caller to confirm, which the deletion answers with before any effect.
async fn hold_to_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    id: Id,
    preflight: Id,
    expected_revision: Option<i64>,
) -> ApiResult<Option<NodeDeletePlan>> {
    sqlx::query("SELECT id FROM preflights WHERE id=$1 FOR UPDATE")
        .bind(preflight)
        .fetch_optional(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let row = orch_db::queries::jobs::forks::preflight(conn, preflight)
        .await?
        .filter(|row| {
            row.kind == DELETE_PLAN_KIND
                && row.node_id == Some(id)
                && row.created_by == principal.user()
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::PreflightExpired,
                "there is no such plan of this node",
            )
        })?;
    if row.expires_at <= ctx.now {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "the plan is out of date: ask for it again",
        ));
    }
    let kept: KeptPlan = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    let set = hierarchy::deletion_set(conn, id, true).await?;
    review::require_origins(ctx, conn, principal, id, &kept.origins).await?;
    let revision = set
        .iter()
        .find(|(_, each, _)| *each == id)
        .map(|(_, _, revision)| *revision)
        .ok_or_else(ApiError::not_found)?;
    let purge = purge_at(ctx.now, ctx.app.config.retention.soft_delete_days);
    let same = revision == kept.revision
        && expected_revision.is_none_or(|expected| expected == revision)
        && fingerprint_of(&set) == kept.fingerprint
        && purge.date_naive() <= kept.purge_at.date_naive();
    orch_db::queries::jobs::forks::forget_preflight(conn, preflight).await?;
    if same {
        return Ok(None);
    }
    Ok(Some(
        delete_plan(ctx, conn, principal, id, &set, &kept.origins).await?,
    ))
}

fn stale_delete_plan(current: NodeDeletePlan) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "what this deletion would remove changed since the plan: confirm the plan as it now is",
    )
    .with("current", current)
}

/// Deletes a node, archiving it first when it is not archived.
async fn node_delete(ctx: Ctx, (): (), body: NodeDeleteBody) -> ApiResult<NodeDeleted> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    let (place, kind) = placed(&mut tx, id).await?;
    if kind == NodeKind::Thread {
        // The part that forks and switches: a side thread is closed here, and nothing else of a thread is.
        if let Some(parent) = orch_db::queries::threads::forking::side_parent(&mut tx, id).await? {
            if let Some(preflight) = body.preflight_id
                && let Some(current) = hold_to_plan(
                    &ctx,
                    &mut tx,
                    &principal,
                    id,
                    preflight,
                    body.expected_revision,
                )
                .await?
            {
                tx.commit().await.map_err(DbError::from)?;
                return Err(stale_delete_plan(current));
            }
            return close_side_thread(&ctx, tx, &principal, id, parent)
                .await
                .map(|()| NodeDeleted { deletion_id: None });
        }
    }
    authorization::require(
        &mut tx,
        &principal,
        id,
        MANAGE,
        Act::Delete,
        Policy::default(),
    )
    .await?;
    let rows = hierarchy::with_ancestors(&mut tx, &[id]).await?;
    let node = rows
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?
        .clone();
    if kind == NodeKind::Org {
        let org = hierarchy::org(&mut tx, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if org.org_kind == "personal" {
            return Err(invalid("id", "a personal org is never deleted"));
        }
    }
    // A deletion that names its reviewed plan removes what the plan counted, or nothing: a stale
    // plan is answered with the plan as it now is, which is kept (nothing else was written yet).
    if let Some(preflight) = body.preflight_id
        && let Some(current) = hold_to_plan(
            &ctx,
            &mut tx,
            &principal,
            id,
            preflight,
            body.expected_revision,
        )
        .await?
    {
        tx.commit().await.map_err(DbError::from)?;
        return Err(stale_delete_plan(current));
    }
    // An org that lies in another gives its members to the org above first.
    if kind == NodeKind::Thread {
        let removed = ctx
            .app
            .engine
            .delete_thread_in(&mut tx, id, Some(principal.user()), principal.now())
            .await
            .map_err(crate::routes::environments::removal_schedule_error)?;
        let settled = &removed.settled;
        let who = actor_name(&ctx)?;
        record(&ctx, &mut tx, orch_db::audit::action!(AuditAction::NodeDeleted), place,
            &format!("{who} deleted the thread {}", node.name),
            json!({"nodes":removed.gone,"archived_first":removed.archived_first,"threads":settled.threads,
                "turns":settled.turns,"inputs":settled.inputs,"approvals":settled.approvals,
                "environments":removed.environments.len(),"resources_before":[],"deleted_nodes":removed.deleted_nodes,
                "purge_at":purge_at(ctx.now,ctx.app.config.retention.soft_delete_days)})).await?;
        let deletion_id = orch_db::queries::trace::node_deletion(&mut tx, id).await?;
        tx.commit().await.map_err(DbError::from)?;
        ctx.app.engine.kick();
        return Ok(NodeDeleted { deletion_id });
    }
    if kind == NodeKind::Org
        && let Some(above) = node.parent_id
    {
        let members = hierarchy::members_inside(&mut tx, id).await?;
        if members > 0 {
            let role: String = body
                .members
                .as_ref()
                .and_then(|members| members.role)
                .map_or_else(
                    || "viewer".to_owned(),
                    |role| same(&role).unwrap_or_default(),
                );
            for org in [id, above] {
                authorization::require(
                    &mut tx,
                    &principal,
                    org,
                    exact(Permission::MembersManage),
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            hierarchy::move_members_up(&mut tx, id, above, &role, principal.user(), ctx.now)
                .await?;
        }
    }
    ctx.app
        .engine
        .lock_home_in(&mut tx, id)
        .await
        .map_err(ApiError::engine)?;
    let deleted_nodes: Vec<Id> = std::iter::once(id)
        .chain(
            hierarchy::descendants(&mut tx, id)
                .await?
                .into_iter()
                .map(|row| row.id),
        )
        .collect();
    // How each node to be deleted stands is kept before the deletion archives them: its Undo puts that back.
    hierarchy::keep_archive_states(&mut tx, id).await?;
    let archived_first = hierarchy::archive(&mut tx, id, principal.user(), ctx.now).await?;
    epoch::bump(&mut tx).await.map_err(ApiError::internal)?;
    let settled = if archived_first
        || orch_db::queries::lifecycle::archive_work_pending(&mut tx, id).await?
    {
        ctx.app
            .engine
            .archive_in(&mut tx, id, principal.user(), principal.now())
            .await
            .map_err(ApiError::engine)?
    } else {
        orch_engine::archive::Settled::default()
    };
    let keep_at = if kind == NodeKind::Org
        && body.resources == Some(orch_api_contract::ownership::DeletedResources::ToParent)
    {
        let parent = node.parent_id.ok_or_else(|| {
            invalid(
                "resources",
                "a root org has no parent to receive its resources",
            )
        })?;
        authorization::require(
            &mut tx,
            &principal,
            parent,
            MANAGE,
            Act::Use,
            Policy::default(),
        )
        .await?;
        Some(parent)
    } else {
        None
    };
    let (environments, resources_before) = if kind == NodeKind::Org {
        ctx.app
            .engine
            .end_home_resources_in(
                &mut tx,
                id,
                keep_at,
                principal.user(),
                principal.now(),
                &crate::sign_in::origin(&ctx),
            )
            .await
            .map_err(ApiError::internal)?
    } else {
        (
            ctx.app
                .engine
                .remove_home_in(&mut tx, id, principal.user(), principal.now(), keep_at)
                .await
                .map_err(crate::routes::environments::removal_schedule_error)?,
            json!([]),
        )
    };
    let gone = hierarchy::delete_subtree(&mut tx, id, principal.user(), ctx.now)
        .await?
        .saturating_add(hierarchy::include_settled_in_deletion(&mut tx, id, &deleted_nodes).await?);
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::NodeDeleted),
        place,
        &format!("{who} deleted the {} {}", kind_word(kind), node.name),
        json!({ "nodes": gone, "archived_first": archived_first, "threads": settled.threads,
            "turns": settled.turns, "inputs": settled.inputs, "approvals": settled.approvals,
            "environments": environments.len(), "resources_before":resources_before,"deleted_nodes":deleted_nodes,"purge_at":purge_at(ctx.now,ctx.app.config.retention.soft_delete_days) }),
    )
    .await?;
    hierarchy::stamp_subtree(&mut tx, id).await?;
    let deletion_id = orch_db::queries::trace::node_deletion(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(NodeDeleted { deletion_id })
}

/// Closes a side thread: its maker may, and so may whoever manages the thread it was taken from or the
/// environment it runs in. Closing one is deleting it; it can be brought back until it is purged.
async fn close_side_thread(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
    principal: &orch_authz::Principal,
    side: Id,
    parent: Id,
) -> ApiResult<()> {
    let row = orch_db::queries::threads::thread(&mut tx, side)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let first = authorization::require(
        &mut tx,
        principal,
        parent,
        MANAGE,
        Act::Use,
        Policy::default(),
    )
    .await
    .err();
    let path = orch_db::queries::threads::node_path(&mut tx, parent).await?;
    let maker = row.owner_id == principal.user()
        && principal.caller().permits(Permission::ThreadManage)
        && principal.caller().reaches(Some(&path))
        && !principal.suspension().applies(&path);
    if first.is_some() && !maker {
        let managed_environment = match row.environment_id {
            Some(environment) => authorization::require_resource(
                &mut tx,
                principal,
                orch_access_contract::permissions::ResourceKind::Environment,
                environment,
                Permission::EnvironmentManage,
                Policy::default(),
            )
            .await
            .is_ok(),
            None => false,
        };
        if !managed_environment {
            return Err(first.unwrap_or_else(ApiError::not_found));
        }
    }
    // The engine commits the same transaction that checked authority, then stops the session.
    ctx.app
        .engine
        .close_side_thread_in(
            tx,
            side,
            "closed",
            Some(principal.user()),
            None,
            principal.now(),
        )
        .await
        .map_err(|error| match error {
            // A commit refused because who may do what changed meanwhile is the caller's to try
            // again, as any write's is: 409, not a failure of the server.
            orch_engine::EngineError::Db(error) => ApiError::from(error),
            other => ApiError::internal(other),
        })?;
    Ok(())
}

fn kind_word(kind: NodeKind) -> &'static str {
    match kind {
        NodeKind::Org => "org",
        NodeKind::Project => "project",
        NodeKind::Topic => "topic",
        NodeKind::Thread => "thread",
    }
}

fn cursor_of(row: &DeletedRow) -> String {
    format!("{}|{}", row.deleted_at.timestamp_micros(), row.id)
}

/// What was deleted directly under a node and is not purged yet.
async fn deleted_list(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<Page<DeletedNode>> {
    let parent = ctx.id("id")?;
    let limit = usize::try_from(query.limit()).unwrap_or(usize::MAX);
    let after = match query.cursor.as_deref() {
        None => None,
        Some(text) => {
            let bad = || invalid("cursor", "is not a cursor of this list");
            let (micros, row) = text.split_once('|').ok_or_else(bad)?;
            Some(AuditCursor {
                at: micros
                    .parse::<i64>()
                    .ok()
                    .and_then(DateTime::<Utc>::from_timestamp_micros)
                    .ok_or_else(bad)?,
                id: row.parse().map_err(|_| bad())?,
            })
        }
    };
    let days = ctx.app.config.retention.soft_delete_days;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let batch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let mut rows = hierarchy::deleted_under(
        &mut conn,
        parent,
        purged_before(ctx.now, days),
        after,
        batch,
    )
    .await?;
    let more = rows.len() > limit;
    rows.truncate(limit);
    let items = deleted_nodes(&mut conn, &principal, &rows, days).await?;
    Ok(Page {
        next_cursor: more.then(|| rows.last().map(cursor_of)).flatten(),
        items,
    })
}

async fn deleted_nodes(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    rows: &[DeletedRow],
    days: u64,
) -> ApiResult<Vec<DeletedNode>> {
    let mut users: Vec<Id> = rows.iter().filter_map(|row| row.deleted_by).collect();
    users.sort_unstable();
    let people = refusals::user_refs(conn, &users).await?;
    let _ = principal;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let kind = refusals::node_kind(&row.kind)?;
            Some(DeletedNode {
                node: orch_api_contract::nodes::NodeRef {
                    thread_display: None,
                    id: row.id,
                    kind,
                    name: Some(row.name.clone()),
                    redacted: false,
                },
                kind,
                deleted_at: Time::new(row.deleted_at),
                deleted_by: row.deleted_by.and_then(|user| people.get(&user).cloned()),
                purge_at: Time::new(purge_at(row.deleted_at, days)),
                children: row.children,
            })
        })
        .collect())
}

/// Brings back a deleted node, archived, with everything that went with it.
async fn node_restore(
    ctx: Ctx,
    (): (),
    body: orch_api_contract::ownership::NodeRestoreBody,
) -> ApiResult<orch_api_contract::ownership::NodeRestoreResponse> {
    restoring::run(ctx, body).await
}

/// The team root orgs the caller owns that were deleted and are not purged.
async fn org_deleted_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<DeletedOrg>> {
    crate::key_operations::native::selected_read::<super::deleted_orgs::Orgs>(ctx, (), ()).await
}

/// Hands the first place among a node's owners to another person: the page
/// change that names them first, which only an owner or an admin of the place
/// makes.
async fn node_transfer(
    ctx: Ctx,
    (): (),
    body: TransferBody,
) -> ApiResult<orch_api_contract::ownership::NodeTransferResponse> {
    crate::middleware::authorization::projected_result(
        ctx.clone(),
        node_transfer_action(ctx, body),
        false,
    )
    .await
}
async fn node_transfer_action(
    ctx: Ctx,
    body: TransferBody,
) -> ApiResult<orch_api_contract::ownership::NodeTransferResponse> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, guard) = crate::routes::transfer::operation::node_action(&ctx).await?;
    let principal = guard.principal().clone();
    let (place, kind): (Placed, NodeKind) = placed(&mut tx, id).await?;
    if kind == NodeKind::Thread {
        return Err(super::threads_are_not_served_here());
    }
    let policy = super::policy_at(&mut tx, id).await?;
    let before = view::one(&mut tx, &principal, id, ctx.now, policy)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let previous = match &before {
        Node::Org(node) => node.base.owner.id,
        Node::Project(node) => node.base.owner.id,
        Node::Topic(node) => node.base.owner.id,
        Node::Thread(node) => node.base.owner.id,
    };
    let mut owners = stored::OwnersChange {
        add: Some(vec![stored::Holder::User { id: body.user_id }]),
        remove: None,
        first: Some(body.user_id),
    };
    if body.keep_previous == Some(false) && previous != body.user_id {
        // The person who was first is taken out in the same step.
        let page = pages::read(&mut tx, &principal, Target::Live(id), policy)
            .await
            .map_err(|error| ApiError::internal(error.to_string()))?;
        owners.remove = Some(
            page.owners
                .iter()
                .filter(|entry| entry.holder == stored::Holder::User { id: previous })
                .map(|entry| entry.id)
                .collect(),
        );
    }
    let outcome = pages::change_fenced(
        &mut tx,
        &principal,
        Target::Live(id),
        &stored::PageChange {
            owners: Some(owners),
            ..stored::PageChange::default()
        },
        policy,
        &origin(&ctx),
        || guard.check_deadline(&ctx),
    )
    .await;
    let changed;
    match outcome {
        Ok(outcome) => {
            changed = outcome.changed;
            if outcome.changed {
                hierarchy::stamp(&mut tx, id).await?;
            }
        }
        Err(pages::ChangeFailure::Native(error)) => {
            return Err(refusals::change_failed(&mut tx, error).await);
        }
        Err(pages::ChangeFailure::BeforeEffect(error)) => return Err(error),
    }
    let _ = place;
    let receipt = crate::routes::transfer::operation::receipt(&ctx, body.user_id, changed)?;
    tx.commit().await.map_err(DbError::from)?;
    crate::routes::transfer::operation::node_reply(&ctx, &principal, receipt).await
}
