//! Ordinary system administration asks the actual bundled node to run the one release updater.
use crate::{
    middleware::requests::{ApiError, ApiResult},
    registry::{Ctx, Registry},
};
use orch_access_contract::audit::AuditAction;
use orch_api_contract::{admin::*, common::ErrorCode};
use orch_authz::CapturedOperation;
use orch_db::{Conn, DbError, queries::jobs::JobRow};
use orch_engine::jobs::server_update as updates;
use orch_types::Id;
use serde_json::json;

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on(AdminServerUpdateGet, read)
        .on(AdminServerUpdateStart, start);
}

async fn node(conn: &mut Conn) -> ApiResult<(ServerUpdateAvailability, Option<(Id, Id)>)> {
    // Two rows suffice to disprove a unique bundled source; this is not a listing limit.
    let rows: Vec<(Id, Id, bool, serde_json::Value)> = sqlx::query_as("SELECT h.id,m.id,m.online,m.facts FROM hosts h JOIN machines m ON m.id=h.machine_id WHERE h.bundled_with_server AND m.deleted_at IS NULL AND m.status='active' AND 'node'=ANY(m.roles) ORDER BY h.id LIMIT 2")
        .fetch_all(conn).await.map_err(DbError::from)?;
    let Some((host, machine, online, facts)) = rows.first() else {
        return Ok((ServerUpdateAvailability::NoBundledNode, None));
    };
    if rows.len() > 1 {
        return Ok((ServerUpdateAvailability::AmbiguousBundledNodes, None));
    }
    let selected = Some((*host, *machine));
    if !online {
        return Ok((ServerUpdateAvailability::NodeOffline, selected));
    }
    if facts
        .pointer("/server_update/supported")
        .and_then(serde_json::Value::as_bool)
        != Some(true)
    {
        return Ok((ServerUpdateAvailability::DockerUnavailable, selected));
    }
    Ok((ServerUpdateAvailability::Ready, selected))
}
async fn job_view(conn: &mut Conn, row: &JobRow) -> ApiResult<orch_api_contract::workspace::Job> {
    crate::routes::environments::jobs::view(conn, row).await
}
async fn read(ctx: Ctx, (): (), (): ()) -> ApiResult<ServerUpdateView> {
    super::admin_of(&ctx)?;
    let (mut tx, guard) = super::action(&ctx).await?;
    let release = orch_worker_core::releases::read(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    let offered_version = release.and_then(|release| release.offered_version);
    let (availability, _) = node(&mut tx).await?;
    let row = updates::latest(&mut tx).await?;
    let observation = row
        .as_ref()
        .and_then(|row| row.plan.get("observation"))
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?;
    let job = match row {
        Some(row) => Some(job_view(&mut tx, &row).await?),
        None => None,
    };
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(ServerUpdateView {
        installed_version: ctx.app.build.version.into(),
        command: format!(
            "./update.sh {}",
            offered_version
                .as_ref()
                .map_or_else(|| "latest".into(), |version| format!("v{version}"))
        ),
        update_available: offered_version
            .as_ref()
            .is_some_and(|version| crate::component_updates::newer(version, ctx.app.build.version)),
        offered_version,
        availability,
        job,
        observation,
    })
}
async fn start(
    ctx: Ctx,
    (): (),
    body: ServerUpdateStart,
) -> ApiResult<orch_api_contract::workspace::Job> {
    let by = super::admin_of(&ctx)?;
    let (mut tx, guard) = super::action(&ctx).await?;
    if let Some(row) = updates::latest(&mut tx).await?.filter(JobRow::is_open) {
        let view = job_view(&mut tx, &row).await?;
        guard.check_deadline(&ctx)?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(view);
    }
    let current = orch_worker_core::releases::read(&mut tx)
        .await
        .map_err(ApiError::internal)?;
    if !orch_build_info::release::is_release_version(&body.release)
        || current
            .as_ref()
            .and_then(|release| release.offered_version.as_deref())
            != Some(body.release.as_str())
        || !crate::component_updates::newer(&body.release, ctx.app.build.version)
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "Check the current release offer before updating the server.",
        ));
    }
    let (availability, selected) = node(&mut tx).await?;
    if availability != ServerUpdateAvailability::Ready {
        return Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "The server update needs its connected bundled node and host Docker access.",
        )
        .with("condition", availability));
    }
    let (host, machine) =
        selected.ok_or_else(|| ApiError::internal("the ready bundled node is absent"))?;
    let captured =
        CapturedOperation::capture(guard.principal().caller()).map_err(ApiError::internal)?;
    let origin =
        crate::key_operations::native::http_origin::capture_operation(&ctx, &mut tx, &captured)
            .await?;
    let seconds = orch_settings::get::<i64>(
        &mut tx,
        orch_settings::Place::System,
        orch_settings_contract::registry::keys::SERVER_UPDATE_PREPARE_TIMEOUT_SECS,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(60);
    if seconds <= 0
        || chrono::Duration::try_seconds(seconds)
            .and_then(|span| guard.now().get().checked_add_signed(span))
            .is_none()
    {
        return Err(super::invalid(
            "server.update_prepare_timeout_secs",
            "the preparation allowance must be positive and fit the native clock",
        ));
    }
    let prepare_timeout_secs = u64::try_from(seconds).map_err(ApiError::internal)?;
    let row = ctx
        .app
        .engine
        .queue_server_update(
            &mut tx,
            updates::Request {
                host,
                machine,
                release: body.release.clone(),
                prepare_timeout_secs,
                origin,
            },
            by,
        )
        .await
        .map_err(ApiError::engine)?;
    super::record(
        &ctx,
        &mut tx,
        by,
        AuditAction::ServerUpdateRequested,
        Some(row.id),
        "Server release update requested",
        json!({"job":row.id,"host":host,"machine":machine,"release":body.release}),
    )
    .await?;
    let view = job_view(&mut tx, &row).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(view)
}
