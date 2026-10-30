//! Kept environment metadata and guarded restore or permanent removal requests.

use super::{Standing, environment_views, jobs};
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
    common::{ErrorCode, Page},
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
        .on(api::NodeDeletedEnvironmentList, list)
        .on(api::EnvironmentRestore, restore)
        .on(api::EnvironmentPurge, purge);
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

pub(crate) async fn read(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
) -> ApiResult<api::Environment> {
    may_read(conn, principal, id).await?;
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
    let mut shown = environment_views(ctx, conn, principal, vec![row], &standing)
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    if let Some(deleted) = promise.deleted_at {
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

pub(crate) async fn inspect(mut ctx: Ctx, id: Id) -> ApiResult<api::Environment> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let shown = read(&ctx, &mut tx, guard.principal(), id).await?;
    let day:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM audit_log WHERE actor_id=$1 AND object_kind='environment' AND object_id=$2 AND action='deleted.inspected' AND (at AT TIME ZONE 'UTC')::date=($3::timestamptz AT TIME ZONE 'UTC')::date)")
        .bind(guard.principal().user()).bind(id).bind(ctx.now).fetch_one(&mut *tx).await.map_err(DbError::from)?;
    if !day {
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(guard.principal().user()),
            orch_db::audit::name!("deleted.inspected"),
            "environment",
            Some(id),
            "A removed environment's retained metadata was inspected",
        );
        entry.node_id = Some(shown.home.id);
        entry.org_id = Some(shown.org_id);
        orch_db::audit::append(&mut tx, &entry, &crate::sign_in::origin(&ctx), ctx.now).await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

async fn list(
    ctx: Ctx,
    query: api::NodeDeletedEnvironmentListQuery,
    (): (),
) -> ApiResult<Page<api::DeletedEnvironment>> {
    let node = ctx.id("id")?;
    let limit = usize::try_from(
        query
            .limit
            .unwrap_or(orch_api_contract::common::DEFAULT_LIMIT)
            .clamp(1, orch_api_contract::common::MAX_LIMIT),
    )
    .map_err(ApiError::internal)?;
    let mut after = query
        .cursor
        .as_deref()
        .map(|v| {
            v.parse::<Id>()
                .map_err(|_| super::invalid("cursor", "is not a cursor of this list"))
        })
        .transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut entries = Vec::new();
    loop {
        let candidates:Vec<Id>=sqlx::query_scalar("SELECT e.id FROM environments e JOIN nodes n ON n.id=e.home_node_id WHERE e.deleted_at IS NOT NULL AND e.purged_at IS NULL AND e.removal_phase IN ('kept','restoring','purging') AND (e.home_node_id=$1 OR ($2 AND $1=ANY(n.path))) AND ($3::uuid IS NULL OR e.id>$3) ORDER BY e.id LIMIT $4")
            .bind(node).bind(query.under.unwrap_or(false)).bind(after).bind(i64::from(orch_api_contract::common::MAX_LIMIT)).fetch_all(&mut *conn).await.map_err(DbError::from)?;
        if candidates.is_empty() {
            break;
        }
        for id in &candidates {
            after = Some(*id);
            match may_read(&mut conn, &principal, *id).await {
                Ok(_) => {}
                Err(error)
                    if matches!(
                        error.code(),
                        ErrorCode::AuthzDenied | ErrorCode::NotFound | ErrorCode::Gone
                    ) =>
                {
                    continue;
                }
                Err(error) => return Err(error),
            }
            let shown = read(&ctx, &mut conn, &principal, *id).await?;
            if let Some(removal) = shown.removal {
                entries.push(api::DeletedEnvironment {
                    environment: shown.base,
                    home: shown.home,
                    host: shown.host,
                    storage: shown.workspace_storage,
                    removal,
                });
            }
            if entries.len() > limit {
                break;
            }
        }
        if entries.len() > limit
            || candidates.len()
                < usize::try_from(orch_api_contract::common::MAX_LIMIT)
                    .map_err(ApiError::internal)?
        {
            break;
        }
    }
    let more = entries.len() > limit;
    entries.truncate(limit);
    let next_cursor = more
        .then(|| entries.last().map(|e| e.environment.id.to_string()))
        .flatten();
    Ok(Page {
        items: entries,
        next_cursor,
    })
}

pub(crate) fn identity(ctx: &Ctx) -> ApiResult<Json> {
    Ok(match &ctx.user()?.via {
        Via::Session { session_id } => json!({"session_id":session_id}),
        Via::Token { token_id, .. } => json!({"token_id":token_id}),
    })
}

async fn change(mut ctx: Ctx, expected: i64, purge: bool, confirm: Option<&str>) -> ApiResult<Job> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let (manager, admin) = may_read(&mut tx, guard.principal(), id).await?;
    let env = hosting::lock_environment(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let removal = retained::removal(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if removal.purged_at.is_some() {
        return Err(ApiError::new(
            ErrorCode::Gone,
            "the owned files were removed for good",
        ));
    }
    if removal.removal_revision != expected {
        return Err(
            ApiError::new(ErrorCode::ConflictStaleRevision, "the removal changed")
                .with("current", removal.removal_revision),
        );
    }
    if purge && (!admin && !(manager && removal.volumes_purge_at.is_some_and(|at| at <= ctx.now))) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "only a current home admin may shorten the kept-file window",
        ));
    }
    if confirm.is_some_and(|v| v != env.name) {
        return Err(super::invalid("confirm", "must be the environment's name"));
    }
    if let Some(job) = job_rows::retained::open_of_environment(&mut tx, id)
        .await?
        .into_iter()
        .next()
    {
        if job.kind
            == if purge {
                job_rows::retained::PURGE
            } else {
                job_rows::retained::RESTORE
            }
        {
            return jobs::view(&mut tx, &job).await;
        }
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "another lifecycle job owns the kept files",
        )
        .with("job", job.id));
    }
    if removal.removal_phase.as_deref() != Some("kept") {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the kept files are unavailable during their current phase",
        ));
    }
    let job = orch_engine::jobs::retained::queue(
        &mut tx,
        id,
        expected,
        purge,
        Some(guard.principal().user()),
        identity(&ctx)?,
        ctx.now,
    )
    .await
    .map_err(|e| match e {
        orch_engine::jobs::runner::MakeError::Running(job) => ApiError::new(
            ErrorCode::ResourceInUse,
            "another job owns this environment",
        )
        .with("job", job.id),
        _ => ApiError::new(ErrorCode::ConflictStaleRevision, "the removal changed"),
    })?;
    let shown = jobs::view(&mut tx, &job).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(shown)
}
async fn restore(ctx: Ctx, (): (), body: api::EnvironmentRestoreBody) -> ApiResult<Job> {
    change(ctx, body.expected_revision, false, None).await
}
async fn purge(ctx: Ctx, query: api::EnvironmentPurgeQuery, (): ()) -> ApiResult<Job> {
    change(ctx, query.expected_revision, true, Some(&query.confirm)).await
}
