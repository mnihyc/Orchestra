//! Read-only archive consequences and caller-bound subtree fences, including exact inverse reviews.

use super::{may_archive, placed, view};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use chrono::Duration;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::forks::PlanBlocker;
use orch_api_contract::nodes::{
    NodeArchiveBody, NodeArchiveEnvironment, NodeArchiveItem, NodeArchivePlan, NodeArchivePreview,
    NodeArchiveWork, NodeKind, NodeUnarchivePreview,
};
use orch_authz::{Act, Policy, Principal};
use orch_db::queries::{archive_plans, hierarchy};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Archive reviews share the fifteen-minute confirmation window of ordinary node deletion reviews.
const PLAN_MINUTES: i64 = 15;
const KIND: &str = "node_archive";

#[derive(Serialize, Deserialize)]
struct Kept {
    archiving: bool,
    stamp: String,
    revision: i64,
}

/// Binds both directions to the ordinary hierarchy action guard.
pub fn mount(registry: &mut Registry) {
    registry
        .on(NodeArchivePreview, archive)
        .on(NodeUnarchivePreview, unarchive);
}

async fn stamp(conn: &mut Conn, id: Id) -> ApiResult<String> {
    let snapshot = archive_plans::snapshot(conn, id).await?;
    let bytes = serde_json::to_vec(&snapshot).map_err(ApiError::internal)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

async fn preview(ctx: Ctx, archiving: bool) -> ApiResult<NodeArchivePlan> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    let root = hierarchy::with_ancestors(&mut tx, &[id]).await?;
    let own = root
        .iter()
        .any(|row| row.id == id && row.archived_at.is_some());
    may_archive(
        &mut tx,
        &principal,
        id,
        if archiving && !own {
            Act::Use
        } else {
            Act::Unarchive
        },
    )
    .await?;
    ctx.app
        .engine
        .lock_archive_in(&mut tx, id)
        .await
        .map_err(ApiError::internal)?;
    hierarchy::deletion_set(&mut tx, id, true).await?;
    let plan = make(&ctx, &mut tx, &principal, id, archiving).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

async fn archive(ctx: Ctx, (): (), (): ()) -> ApiResult<NodeArchivePlan> {
    preview(ctx, true).await
}
async fn unarchive(ctx: Ctx, (): (), (): ()) -> ApiResult<NodeArchivePlan> {
    preview(ctx, false).await
}

/// Builds a review without changing any node, work, share or link; only its expiring confirmation is kept.
pub(super) async fn make(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    archiving: bool,
) -> ApiResult<NodeArchivePlan> {
    let set = hierarchy::deletion_set(conn, id, false).await?;
    let ids: Vec<Id> = set
        .iter()
        .filter(|(kind, _, _)| kind != "document")
        .map(|(_, id, _)| *id)
        .collect();
    let nodes = hierarchy::with_ancestors(conn, &ids).await?;
    let root = nodes
        .iter()
        .find(|node| node.id == id)
        .ok_or_else(ApiError::not_found)?;
    let revision = root.revision;
    let refs = view::refs(conn, principal, &ids, Policy::default()).await?;
    let node = refs.get(&id).cloned().ok_or_else(ApiError::not_found)?;
    let mut subtree = Vec::new();
    for each in &ids {
        let row = nodes
            .iter()
            .find(|node| node.id == *each)
            .ok_or_else(ApiError::not_found)?;
        let reference = refs.get(each).cloned().ok_or_else(ApiError::not_found)?;
        let read_only_after = row.path.iter().any(|parent| {
            if *parent == id {
                archiving
            } else {
                nodes
                    .iter()
                    .any(|ancestor| ancestor.id == *parent && ancestor.archived_at.is_some())
            }
        });
        subtree.push(NodeArchiveItem {
            node: reference,
            revision: row.revision,
            own_archived: row.archived_at.is_some(),
            read_only_after,
        });
    }
    let mut work = Vec::new();
    for row in archive_plans::work(conn, id).await? {
        let thread = refs.get(&row.id).cloned().ok_or_else(ApiError::not_found)?;
        let visible = !thread.redacted;
        work.push(NodeArchiveWork {
            thread,
            turns: visible.then_some(row.turns),
            inputs: visible.then_some(row.inputs),
            approvals: visible.then_some(row.approvals),
            leases: visible.then_some(row.leases),
            session: visible.then_some(row.session),
            links: visible.then_some(row.links),
        });
    }
    let mut environments = Vec::new();
    for (environment, name) in archive_plans::environments(conn, id).await? {
        let allowed = orch_authz::resources::check(
            conn,
            principal,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
            Permission::EnvironmentView,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
        .is_allowed();
        environments.push(NodeArchiveEnvironment {
            id: environment,
            name: allowed.then_some(name),
            redacted: !allowed,
        });
    }
    let mut blockers = Vec::new();
    let (_, kind) = placed(conn, id).await?;
    if kind == NodeKind::Thread
        && orch_db::queries::threads::forking::side_parent(conn, id)
            .await?
            .is_some()
    {
        blockers.push(PlanBlocker {
            code: "thread.side".to_owned(),
            message: "A side thread is closed, not archived.".to_owned(),
            missing: None,
        });
    }
    let (copies_kept, live_shares_kept) = archive_plans::shares(conn, id).await?;
    let stamp = stamp(conn, id).await?;
    let preflight_id = orch_types::new_id();
    let expires_at = ctx.now + Duration::minutes(PLAN_MINUTES);
    let kept = Kept {
        archiving,
        stamp: stamp.clone(),
        revision,
    };
    orch_db::queries::jobs::forks::save_preflight(
        conn,
        &orch_db::queries::jobs::forks::PreflightRow {
            id: preflight_id,
            kind: KIND.to_owned(),
            node_id: Some(id),
            environment_id: None,
            created_by: principal.user(),
            plan: serde_json::to_value(kept).map_err(ApiError::internal)?,
            expires_at,
            created_at: ctx.now,
        },
    )
    .await?;
    Ok(NodeArchivePlan {
        preflight_id,
        expires_at: Time::new(expires_at),
        stamp,
        archiving,
        node,
        revision,
        subtree,
        counts: super::removal::counts_of(&set),
        work,
        environments,
        copies_kept,
        live_shares_kept,
        blockers,
    })
}

/// Consumes the caller's review under the same locks the archive writer holds; stale facts return a fresh review.
pub(super) async fn verify(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    archiving: bool,
    body: &NodeArchiveBody,
) -> ApiResult<Option<NodeArchivePlan>> {
    let row = orch_db::queries::jobs::forks::preflight(conn, body.preflight_id)
        .await?
        .filter(|row| {
            row.kind == KIND && row.node_id == Some(id) && row.created_by == principal.user()
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::PreflightExpired,
                "This archive review is not yours or is for another place. Review it again.",
            )
        })?;
    if row.expires_at <= ctx.now {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "This archive review expired. Review it again.",
        ));
    }
    let kept: Kept = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    if kept.archiving != archiving {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "This review is for the opposite archive action.",
        ));
    }
    hierarchy::deletion_set(conn, id, true).await?;
    let current = stamp(conn, id).await?;
    orch_db::queries::jobs::forks::forget_preflight(conn, body.preflight_id).await?;
    if current == kept.stamp && body.stamp == kept.stamp && body.expected_revision == kept.revision
    {
        return Ok(None);
    }
    Ok(Some(make(ctx, conn, principal, id, archiving).await?))
}
