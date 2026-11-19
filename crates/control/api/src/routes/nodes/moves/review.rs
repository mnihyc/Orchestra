//! Stored move-consequence stamps and the ordinary access engine's rolled-back move comparison.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::roles::PlaceRef;
use orch_api_contract::access::PagePreview;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::nodes::NodeMovePlan;
use orch_authz::previews::{Before, Scope};
use orch_authz::things::Thing;
use orch_authz::{Policy, Principal, epoch};
use orch_db::queries::{archive_plans, hierarchy};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Moves use the same fifteen-minute confirmation window as other stored place reviews.
const PLAN_MINUTES: i64 = 15;
const KIND: &str = "node_move";

#[derive(Serialize, Deserialize)]
struct Kept {
    parent: Id,
    stamp: String,
    revision: i64,
}

/// Compares the real move's inheritance without keeping a path, epoch, usage or audit change.
pub(super) async fn effects(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    parent: Id,
) -> ApiResult<PagePreview> {
    let before = [
        Before::take(
            conn,
            principal,
            Scope::Page(Thing::Node(node)),
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?,
        Before::take(
            conn,
            principal,
            Scope::Inside(PlaceRef::Node(node)),
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?,
    ];
    sqlx::query("SAVEPOINT move_access_preview")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    hierarchy::move_under(conn, node, parent, ctx.now).await?;
    epoch::bump(conn).await.map_err(ApiError::internal)?;
    let result = crate::routes::access::share_preview::effects(conn, principal, &before).await;
    sqlx::query("ROLLBACK TO SAVEPOINT move_access_preview")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("RELEASE SAVEPOINT move_access_preview")
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    result
}

async fn fingerprint(ctx: &Ctx, conn: &mut Conn, plan: &NodeMovePlan) -> ApiResult<String> {
    let mut consequences = serde_json::to_value(plan).map_err(ApiError::internal)?;
    if let Some(object) = consequences.as_object_mut() {
        for field in ["preflight_id", "stamp", "expires_at"] {
            object.remove(field);
        }
    }
    let subtree = archive_plans::snapshot(conn, plan.node.id).await?;
    let epoch = epoch::current(conn).await.map_err(ApiError::internal)?;
    let ancestors: Vec<_> = hierarchy::with_ancestors(conn, &[plan.to.id])
        .await?
        .into_iter()
        .map(|node| node.id)
        .collect();
    // Lightweight access rows omit tree sequences; the detailed rows retain each ancestor's exact stamp.
    let mut destination: Vec<_> = hierarchy::details(conn, &ancestors, ctx.now)
        .await?
        .into_iter()
        .map(|node| (node.id, node.revision, node.tree_seq))
        .collect();
    destination.sort_unstable();
    let bytes = serde_json::to_vec(&(epoch, subtree, destination, consequences))
        .map_err(ApiError::internal)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

/// Keeps a read-only review for this caller, node, destination and exact current consequences.
pub(super) async fn keep(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    mut plan: NodeMovePlan,
) -> ApiResult<NodeMovePlan> {
    plan.preflight_id = orch_types::new_id();
    plan.expires_at = Time::new(ctx.now + chrono::Duration::minutes(PLAN_MINUTES));
    plan.stamp = fingerprint(ctx, conn, &plan).await?;
    let kept = Kept {
        parent: plan.to.id,
        stamp: plan.stamp.clone(),
        revision: plan.revision,
    };
    orch_db::queries::jobs::forks::save_preflight(
        conn,
        &orch_db::queries::jobs::forks::PreflightRow {
            id: plan.preflight_id,
            kind: KIND.to_owned(),
            node_id: Some(plan.node.id),
            environment_id: None,
            created_by: principal.user(),
            plan: serde_json::to_value(kept).map_err(ApiError::internal)?,
            expires_at: plan.expires_at.get(),
            created_at: ctx.now,
        },
    )
    .await?;
    Ok(plan)
}

/// A displayed review is never silently replaced before its ordinary revision-checked move.
pub(super) async fn changed(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    current: NodeMovePlan,
) -> ApiResult<Option<NodeMovePlan>> {
    let row = orch_db::queries::jobs::forks::preflight(conn, id)
        .await?
        .filter(|row| {
            row.kind == KIND
                && row.created_by == principal.user()
                && row.node_id == Some(current.node.id)
        })
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::PreflightExpired,
                "This move review is not yours or is for another place. Review it again.",
            )
        })?;
    if row.expires_at <= ctx.now {
        return Err(ApiError::new(
            ErrorCode::PreflightExpired,
            "This move review expired. Review it again.",
        ));
    }
    let kept: Kept = serde_json::from_value(row.plan).map_err(ApiError::internal)?;
    let same = kept.parent == current.to.id
        && kept.revision == current.revision
        && kept.stamp == fingerprint(ctx, conn, &current).await?;
    orch_db::queries::jobs::forks::forget_preflight(conn, id).await?;
    if same {
        Ok(None)
    } else {
        Ok(Some(keep(ctx, conn, principal, current).await?))
    }
}
