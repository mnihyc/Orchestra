//! Kept environment metadata and guarded restore or permanent removal requests.

use super::{Standing, jobs};

mod changes;
mod selected;
use crate::{
    middleware::{
        authentication::Via,
        authorization,
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::{Ctx, Registry},
};
use orch_access_contract::{decisions::ObjectRef, permissions::ResourceKind};
use orch_api_contract::{
    common::ErrorCode,
    environment as api,
    nodes::{DeletedInspection, JobRef, JobState},
    workspace::Job,
};
use orch_authz::{Policy, Principal, resources};
use orch_db::{
    Conn, DbError,
    queries::{
        environments::{hosting, retained, routes as rows},
        jobs as job_rows,
    },
};
use orch_types::{Id, Json, Time};
use serde_json::json;
use std::collections::HashMap;

/// Registers the kept-data requests beside the ordinary environment reader.
pub fn mount(registry: &mut Registry) {
    registry
        .on_snapshot_read_with_optional_operation(
            api::NodeDeletedEnvironmentList,
            crate::key_operations::native::selected::<selected::RetainedRows>,
            crate::key_operations::native::selected_read::<selected::RetainedRows>,
        )
        .on_with_operation(
            api::EnvironmentRestore,
            crate::key_operations::native::effects::prepare::<changes::Restore>,
            changes::restore,
        )
        .on_with_operation(
            api::EnvironmentPurge,
            crate::key_operations::native::effects::prepare::<changes::Purge>,
            changes::purge,
        );
}

fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "kept files require their environment's manager or current home admin",
    )
    .with("required", "environment.manage")
}

pub(crate) async fn may_read(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<(bool, bool)> {
    let rights = orch_engine::jobs::retained::authority(conn, principal, id)
        .await
        .map_err(ApiError::internal)?;
    if !rights.0 && !rights.1 {
        return Err(denied());
    }
    Ok(rights)
}

pub(crate) async fn present(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<Option<api::EnvironmentRemoval>> {
    let Some(row) = retained::removal(conn, id).await? else {
        return Ok(None);
    };
    let Some(phase) = row.removal_phase.as_deref() else {
        return Ok(None);
    };
    let (manager, admin) = orch_engine::jobs::retained::authority(conn, principal, id)
        .await
        .map_err(ApiError::internal)?;
    let by = match row.deleted_by {
        Some(id) => crate::middleware::authorization::refusals::user_refs(conn, &[id])
            .await?
            .remove(&id),
        None => None,
    };
    let mut jobs = job_rows::retained::open_of_environment(conn, id).await?;
    if jobs.is_empty()
        && let Some(job) = job_rows::latest_of_environment(
            conn,
            id,
            &[
                job_rows::kind::ENVIRONMENT_REMOVE,
                job_rows::retained::RESTORE,
                job_rows::retained::PURGE,
            ],
        )
        .await?
        && job.plan.get("removal_revision").and_then(Json::as_i64) == Some(row.removal_revision)
    {
        jobs.push(job);
    }
    let job = jobs
        .iter()
        .find(|job| job_rows::retained::SERIALIZED.contains(&job.kind.as_str()))
        .map(|row| {
            let (progress, _) = orch_engine::jobs::runner::read_progress(row);
            JobRef {
                id: row.id,
                kind: row.kind.clone(),
                state: match row.state.as_str() {
                    "queued" => JobState::Queued,
                    "running" => JobState::Running,
                    "succeeded" => JobState::Succeeded,
                    "cancelled" => JobState::Cancelled,
                    "lost" => JobState::Lost,
                    _ => JobState::Failed,
                },
                phase: progress.phase,
                percent: progress.percent,
            }
        });
    let env = hosting::environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    // The recovery window starts at actual unit removal, not at the requested deadline.
    // A delayed/offline host must never produce an already-expired promise in an acceptance.
    let purge_at = row.volumes_purge_at;
    Ok(Some(api::EnvironmentRemoval {
        phase: serde_json::from_value(json!(phase)).map_err(ApiError::internal)?,
        deleted_at: row.deleted_at.map(Time::new),
        deleted_by: by,
        keep_days: i64::from(row.removed_keep_days.unwrap_or_default()),
        purge_at: purge_at.map(Time::new),
        purged_at: row.purged_at.map(Time::new),
        bytes: row.retained_bytes,
        size_at: row.retained_size_at.map(Time::new),
        reason: env.status_detail.or_else(|| {
            row.retained_bytes
                .is_none()
                .then(|| "size has not been measured".to_owned())
        }),
        revision: row.removal_revision,
        can_inspect: (manager || admin) && row.purged_at.is_none() && row.deleted_at.is_some(),
        can_restore: (manager || admin) && phase == "kept",
        can_purge: phase == "kept"
            && (admin || (manager && purge_at.is_some_and(|at| at <= ctx.now))),
        job,
    }))
}

/// The native retained view's stored operands, before a transport body is rendered.
pub(super) struct PreparedRead {
    pub(super) id: Id,
    pub(super) home: Id,
    pub(super) projection: super::PreparedInventory,
    pub(super) deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    pub(super) authority: (bool, bool),
}

impl PreparedRead {
    pub(super) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut Conn,
        principal: &Principal,
    ) -> ApiResult<api::Environment> {
        let mut shown = super::show_inventory_prepared(ctx, conn, principal, &self.projection)
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if let Some(deleted) = self.deleted_at {
            let removal = shown
                .removal
                .as_ref()
                .ok_or_else(|| ApiError::internal("the kept data has no removal metadata"))?;
            shown.inspection = Some(DeletedInspection {
                deleted_at: Time::new(deleted),
                deleted_by: removal.deleted_by.clone(),
                purge_at: removal
                    .purge_at
                    .ok_or_else(|| ApiError::internal("the kept data has no purge day"))?,
                readonly: true,
            });
        }
        Ok(shown)
    }
}

pub(super) async fn prepare_read(
    _ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<PreparedRead> {
    let authority = may_read(conn, principal, id).await?;
    let promise = retained::removal(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if promise.purged_at.is_some() {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "the owned files were removed for good",
        ));
    }
    if promise.removal_phase.is_none() {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "this older removal has no kept-file promise",
        ));
    }
    let row = rows::environments_including_deleted(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let effective = resources::effective(
        conn,
        principal,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let standing = HashMap::from([(
        id,
        Standing {
            admin_access: effective.admin_access,
            admin_place: effective.admin_place,
            participating: effective.participating,
            admin_only: effective.admin_only,
            owner: effective.owner,
            administers: effective.administers,
            held: effective.held,
        },
    )]);
    let home = row.home_node_id;
    let projection =
        super::prepare_environment_views(conn, vec![row], standing, principal.now()).await?;
    Ok(PreparedRead {
        id,
        home,
        projection,
        deleted_at: promise.deleted_at,
        authority,
    })
}

/// Once-only successful inspection observation, outside content snapshot retries.
pub(super) async fn record_inspected(ctx: &Ctx, id: Id) -> ApiResult<()> {
    let mut tx = ctx.app.db.begin().await?;
    // Serialize the native once-per-day observation before its fresh permission checkpoint.
    hosting::lock_environment(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let day:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE actor_id=$1 AND object_kind='environment' AND object_id=$2 AND action='deleted.inspected' AND (at AT TIME ZONE 'UTC')::date=($3::timestamptz AT TIME ZONE 'UTC')::date)")
        .bind(ctx.user()?.id()).bind(id).bind(ctx.app.clock.now()).fetch_one(&mut *tx).await.map_err(DbError::from)?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let source = prepare_read(&current, &mut tx, guard.principal(), id).await?;
    let org = source
        .projection
        .rows
        .first()
        .ok_or_else(ApiError::not_found)?
        .org_id;
    guard.check_deadline(&current)?;
    if !day {
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(guard.principal().user()),
            orch_db::audit::name!("deleted.inspected"),
            "environment",
            Some(id),
            "A removed environment's retained metadata was inspected",
        );
        entry.node_id = Some(source.home);
        entry.org_id = Some(org);
        orch_db::audit::append(
            &mut tx,
            &entry,
            &crate::sign_in::origin(&current),
            current.now,
        )
        .await?;
    }
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

pub(crate) fn identity(ctx: &Ctx) -> ApiResult<Json> {
    Ok(match &ctx.user()?.via {
        Via::OwnerRead { source } => json!({"owner_read":source.source()}),
        Via::Session { session_id } => json!({"session_id":session_id}),
        Via::Token { token_id, .. } => json!({"token_id":token_id}),
    })
}
