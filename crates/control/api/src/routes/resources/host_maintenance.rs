//! Host load history, safe removal of empty hosts and recycling by their managers.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use orch_api_contract::common::ErrorCode;
use orch_api_contract::{
    admin::{METRIC_MAX_POINTS, METRIC_STEP_MINUTES, MetricResolution, MetricSeries},
    environment::{RecycleBody, RecycleReply},
    resources as api,
};
use orch_db::{
    DbError,
    queries::{
        environments::changes,
        metrics::{self, Resolution},
        resources::hosting as hosts,
    },
};
use orch_types::Time;

/// Seconds per minute align metric buckets to the same widths as Management.
const SECONDS_PER_MINUTE: i64 = 60;
/// Hourly records cannot provide a finer step than one hour.
const HOUR_MINUTES: u32 = 60;

/// Binds host metrics, recycling and removal.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::HostMetricsList, series)
        .on(api::HostRecycle, recycle)
        .on(api::HostDelete, delete);
}

async fn recycle(ctx: Ctx, (): (), body: RecycleBody) -> ApiResult<RecycleReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let id = ctx.id("id")?;
    hosts::host(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let ids = changes::on_host(&mut conn, id).await?;
    drop(conn);
    crate::routes::fleet::recycle_ids(&ctx, &ids, &body, Some(id)).await
}

async fn delete(ctx: Ctx, query: api::HostDeleteQuery, (): ()) -> ApiResult<()> {
    let (ctx, mut tx, _principal) = super::begin_mutation(&ctx).await?;
    let id = ctx.id("id")?;
    orch_db::queries::resources::lock_resource(&mut tx, "host", id).await?;
    let host = hosts::host(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if query.confirm != host.name {
        return Err(invalid("confirm", "the host's name is required"));
    }
    let (kept, _, _, _) = orch_db::queries::environments::retained::on_host(&mut tx, id).await?;
    if kept > 0 {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the host still holds referenced kept files",
        )
        .with("kept_environments", kept));
    }
    let (runtimes, mounts) = changes::host_dependencies(&mut tx, id, host.machine_id).await?;
    if !runtimes.is_empty() || !mounts.is_empty() {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "the host still holds units or mount volumes",
        )
        .with("runtimes", runtimes)
        .with("mounts", mounts));
    }
    changes::forget_host(&mut tx, id, host.machine_id, ctx.now).await?;
    super::record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(orch_access_contract::audit::AuditAction::HostDeleted),
        id,
        super::Home {
            node: host.home_id,
            org: host.org_id,
        },
        "Removed an empty host",
        serde_json::json!({"host":id,"machine":host.machine_id}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

async fn series(
    ctx: Ctx,
    query: api::HostMetricsListQuery,
    (): (),
) -> ApiResult<Vec<MetricSeries>> {
    let (from, to) = (query.from.get(), query.to.get());
    if to < from {
        return Err(invalid("to", "must not be before from"));
    }
    let fine = ctx.app.config.retention.metrics_fine_days;
    let resolution = if fine == 0
        || from
            >= ctx.now - chrono::Duration::days(i64::try_from(fine).unwrap_or(i64::MAX / 86_400))
    {
        Resolution::Minutes
    } else {
        Resolution::Hours
    };
    let allowed = METRIC_STEP_MINUTES
        .into_iter()
        .filter(|step| resolution == Resolution::Minutes || *step >= HOUR_MINUTES)
        .collect::<Vec<_>>();
    let points = |step: u32| {
        let span = i64::from(step) * SECONDS_PER_MINUTE;
        to.timestamp().div_euclid(span) - from.timestamp().div_euclid(span) + 1
    };
    let step = match query.step {
        Some(step) if !allowed.contains(&step) => {
            return Err(invalid(
                "step",
                "choose a retained step: 1, 5, 60 or 1440 minutes",
            ));
        }
        Some(step) if points(step) > i64::from(METRIC_MAX_POINTS) => {
            return Err(invalid(
                "step",
                "this span exceeds 720 points; choose a larger step",
            ));
        }
        Some(step) => step,
        None => allowed
            .into_iter()
            .find(|step| points(*step) <= i64::from(METRIC_MAX_POINTS))
            .ok_or_else(|| invalid("from", "choose a shorter span"))?,
    };
    let mut conn = ctx.app.db.acquire().await?;
    let id = ctx.id("id")?;
    hosts::host(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let rows = metrics::host_steps(&mut conn, id, resolution, from, to, step as i32).await?;
    let mut series: Vec<MetricSeries> = Vec::new();
    for row in rows {
        let labels = row
            .labels
            .as_object()
            .map(|v| {
                v.iter()
                    .map(|(key, value)| {
                        (
                            key.clone(),
                            value
                                .as_str()
                                .map_or_else(|| value.to_string(), str::to_owned),
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        let mut at = series
            .iter()
            .position(|s| s.name == row.name && s.labels == labels);
        if at.is_none() {
            series.push(MetricSeries {
                name: row.name,
                labels,
                resolution: if resolution == Resolution::Minutes {
                    MetricResolution::Minute
                } else {
                    MetricResolution::Hour
                },
                points: Vec::new(),
                peaks: (resolution == Resolution::Hours).then(Vec::new),
            });
            at = Some(series.len() - 1);
        }
        if let Some(at) = at {
            series[at].points.push((Time::new(row.at), row.value));
            if let (Some(peaks), Some(peak)) = (&mut series[at].peaks, row.peak) {
                peaks.push((Time::new(row.at), peak));
            }
        }
    }
    Ok(series)
}
