//! Kept environment metadata and guarded restore or permanent removal requests.

use super::Standing;

use super::jobs;

use crate::middleware::authentication::Via;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::environment as api;

use orch_api_contract::nodes::JobRef;

use orch_api_contract::nodes::JobState;

use orch_api_contract::workspace::Job;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::queries::environments::hosting;

use orch_db::queries::environments::retained;

use orch_db::queries::environments::routes as rows;

use orch_db::queries::jobs as job_rows;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

use std::collections::HashMap;

pub use orch_api_base::routes::environments::retained::PreparedRead;
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
orch_api_base::__api_bind_069be0bc4b!();

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
orch_api_base::__api_bind_c4a5a90a7f!();

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
orch_api_base::__api_bind_e0b0aea787!();

#[path = "retained/changes.rs"]
pub mod changes;

#[path = "retained/selected.rs"]
pub mod selected;
