//! Deleting a node with everything in it, bringing it back until it is purged, and handing a node to another owner.
//!
//! A node that is not archived is archived first, in the same transaction;
//! then it and everything under it is marked deleted by one moment, and
//! bringing it back undoes exactly that: what was deleted before stays
//! deleted. An org that lies in another gives its members to the org above
//! first, who keep the role they hold there or are given the one the
//! request names. Nothing is removed until the retention days have passed.
//! The first owner of a node is passed on by the page change that does it.

use super::Placed;

use super::actor_name;

use super::invalid;

use super::placed;

use super::record;

use super::view;

use crate::middleware::authorization;

use crate::middleware::authorization::MANAGE;

use crate::middleware::authorization::exact;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::access::page::same;

use crate::sign_in::origin;

use chrono::DateTime;

use chrono::Duration;

use chrono::Utc;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::pages as stored;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::Page;

use orch_api_contract::common::PageQuery;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeOrSystem;

use orch_api_contract::ownership::DeletedNode;

use orch_api_contract::ownership::DeletedOrg;

use orch_api_contract::ownership::NodeDelete;

use orch_api_contract::ownership::NodeDeleteBody;

use orch_api_contract::ownership::NodeDeleteCounts;

use orch_api_contract::ownership::NodeDeletePlan;

use orch_api_contract::ownership::NodeDeletePreview;

use orch_api_contract::ownership::NodeDeleted;

use orch_api_contract::ownership::NodeDeletedList;

use orch_api_contract::ownership::NodeRestore;

use orch_api_contract::ownership::NodeTransfer;

use orch_api_contract::ownership::OrgDeletedList;

use orch_api_contract::ownership::RestoreBy;

use orch_api_contract::ownership::TransferBody;

use orch_authz::pages;

use orch_authz::pages::Target;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::epoch;

use orch_db::queries::access;

use orch_db::queries::hierarchy;

use orch_db::queries::hierarchy::AuditCursor;

use orch_db::queries::hierarchy::DeletedRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use serde_json::json;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_with_operation(
            NodeDeletePreview,
            crate::key_operations::native::effects::prepare::<review::Preview>,
            node_delete_preview,
        )
        .on_with_operation(
            NodeDelete,
            crate::key_operations::native::effects::prepare::<removing::Delete>,
            node_delete,
        )
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

/// Stores a reviewed deletion without deleting anything; full names have their own Read boundary.
async fn node_delete_preview(
    ctx: Ctx,
    (): (),
    (): (),
) -> ApiResult<orch_api_contract::ownership::NodeDeletePreviewResponse> {
    review::run(ctx).await
}

fn stale_delete_plan(current: NodeDeletePlan) -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "what this deletion would remove changed since the plan: confirm the plan as it now is",
    )
    .with("current", current)
}

/// Deletes through the same native prepared lifecycle and intrinsic id acknowledgement.
async fn node_delete(ctx: Ctx, (): (), body: NodeDeleteBody) -> ApiResult<NodeDeleted> {
    removing::run(ctx, body).await
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

#[path = "removal/removing.rs"]
pub mod removing;

#[path = "removal/restoring.rs"]
pub mod restoring;

#[path = "removal/review.rs"]
pub mod review;
