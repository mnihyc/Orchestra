//! Read-only archive consequences and caller-bound subtree fences, including exact inverse reviews.

use super::archive_operation;

use super::placed;

use super::view;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use chrono::Duration;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::forks::PlanBlocker;

use orch_api_contract::nodes::NodeArchiveBody;

use orch_api_contract::nodes::NodeArchiveEnvironment;

use orch_api_contract::nodes::NodeArchiveItem;

use orch_api_contract::nodes::NodeArchivePlan;

use orch_api_contract::nodes::NodeArchivePreview;

use orch_api_contract::nodes::NodeArchiveWork;

use orch_api_contract::nodes::NodeKind;

use orch_api_contract::nodes::NodeUnarchivePreview;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::archive_plans;

use orch_db::queries::hierarchy;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use serde::Deserialize;

use serde::Serialize;

use sha2::Digest;

use sha2::Sha256;
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
        .on_with_operation(NodeArchivePreview, archive_operation::review, archive)
        .on_with_operation(NodeUnarchivePreview, archive_operation::review, unarchive);
}

async fn stamp(conn: &mut Conn, id: Id) -> ApiResult<String> {
    let snapshot = archive_plans::snapshot(conn, id).await?;
    let bytes = serde_json::to_vec(&snapshot).map_err(ApiError::internal)?;
    Ok(hex::encode(Sha256::digest(bytes)))
}

async fn preview(ctx: Ctx) -> ApiResult<NodeArchivePlan> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, _) = archive_operation::action(&ctx).await?;
    let (ctx, guard, bounds) = archive_operation::effect(&ctx, &mut tx, id).await?;
    let Some(archive_operation::NativePrefix::Preview(plan)) = bounds.native else {
        return Err(ApiError::internal(
            "an archive preview lost its native operands",
        ));
    };
    guard.check_deadline(&ctx)?;
    let plan = plan.keep(&mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

async fn archive(ctx: Ctx, (): (), (): ()) -> ApiResult<NodeArchivePlan> {
    let projection = ctx.clone();
    preview(ctx)
        .await
        .map_err(|error| archive_operation::problem(&projection, error))
}

async fn unarchive(ctx: Ctx, (): (), (): ()) -> ApiResult<NodeArchivePlan> {
    let projection = ctx.clone();
    preview(ctx)
        .await
        .map_err(|error| archive_operation::problem(&projection, error))
}

/// A complete native review value plus its existing private expiring confirmation.
#[derive(Clone)]
pub(super) struct PreparedPlan {
    answer: NodeArchivePlan,
    kept: orch_db::queries::jobs::forks::PreflightRow,
}

impl PreparedPlan {
    /// Its first SQL is the existing private confirmation INSERT; no validation/read awaits.
    pub(super) async fn keep(self, conn: &mut Conn) -> ApiResult<NodeArchivePlan> {
        orch_db::queries::jobs::forks::save_preflight(conn, &self.kept).await?;
        Ok(self.answer)
    }
}

/// Builds the same review and stores it for ordinary accepted inverse/read callers.
pub(super) async fn make(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    archiving: bool,
    bounds: Option<&archive_operation::Bounds>,
) -> ApiResult<NodeArchivePlan> {
    prepare_make(ctx, conn, principal, id, archiving, bounds)
        .await?
        .keep(conn)
        .await
}

/// The complete original read-only review producer, without storing its confirmation yet.
pub(super) async fn prepare_make(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    archiving: bool,
    bounds: Option<&archive_operation::Bounds>,
) -> ApiResult<PreparedPlan> {
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
    let refs = match bounds.filter(|bounds| bounds.names.limited) {
        Some(bounds) => bounds.names.refs(conn, principal, &ids).await?,
        None => view::refs(conn, principal, &ids, Policy::default()).await?,
    };
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
        let allowed = match bounds {
            Some(bounds) if bounds.environment_candidates.contains(&environment) => {
                bounds.environments.contains(&environment)
            }
            Some(bounds) if bounds.names.limited => false,
            _ => environment_view(conn, principal, environment).await?,
        };
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
    let kept = orch_db::queries::jobs::forks::PreflightRow {
        id: preflight_id,
        kind: KIND.to_owned(),
        node_id: Some(id),
        environment_id: None,
        created_by: principal.user(),
        plan: serde_json::to_value(kept).map_err(ApiError::internal)?,
        expires_at,
        created_at: ctx.now,
    };
    Ok(PreparedPlan {
        kept,
        answer: NodeArchivePlan {
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
        },
    })
}

/// The existing environment-name predicate, also prepared once for the same guarded projection.
pub(super) async fn environment_view(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
) -> ApiResult<bool> {
    orch_authz::resources::check(
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
    .map_err(ApiError::internal)
    .map(|decision| decision.is_allowed())
}

/// The same caller-bound confirmation decision, prepared before the first private write.
#[derive(Clone)]
pub(super) struct PreparedReview {
    id: Id,
    replacement: Option<PreparedPlan>,
}

impl PreparedReview {
    /// The existing confirmation DELETE is first; a stale replacement uses the same INSERT.
    pub(super) async fn consume(self, conn: &mut Conn) -> ApiResult<Option<NodeArchivePlan>> {
        orch_db::queries::jobs::forks::forget_preflight(conn, self.id).await?;
        match self.replacement {
            Some(plan) => Ok(Some(plan.keep(conn).await?)),
            None => Ok(None),
        }
    }
}

/// Reads the exact original review and canonical locked subtree facts, without consuming it.
pub(super) async fn prepare_verify(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    archiving: bool,
    body: &NodeArchiveBody,
    bounds: Option<&archive_operation::Bounds>,
) -> ApiResult<PreparedReview> {
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
    // The action's native Thread→Node/Document serialization is owned by effect().
    // Discovery is factual only; the final guarded producer repeats this under those locks.
    hierarchy::deletion_set(conn, id, false).await?;
    let current = stamp(conn, id).await?;
    if current == kept.stamp && body.stamp == kept.stamp && body.expected_revision == kept.revision
    {
        return Ok(PreparedReview {
            id: body.preflight_id,
            replacement: None,
        });
    }
    Ok(PreparedReview {
        id: body.preflight_id,
        replacement: Some(prepare_make(ctx, conn, principal, id, archiving, bounds).await?),
    })
}
