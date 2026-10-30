//! Plan-window measurements and early start-overs, kept with provider observations and read with their audit markers.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Timelike, Utc};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::BTreeMap;

/// The stored gauge for the percentage shown by each plan window.
pub const PLAN_USED_PERCENT: &str = "provider_plan_used_percent";
/// A five-point fall distinguishes a start-over from small vendor corrections.
const EARLY_RESET_DROP_PERCENT: f64 = 5.0;
/// A reading around the scheduled reset is an ordinary new window.
const RESET_GRACE: Duration = Duration::minutes(1);

/// A window whose latest reading demonstrates an early start-over.
#[derive(Debug, Clone, PartialEq)]
pub struct EarlyReset {
    /// The window's stable id.
    pub window: String,
    /// Its display name.
    pub label: String,
    /// The previous percentage.
    pub before: f64,
    /// The new percentage.
    pub after: f64,
    /// The reset previously scheduled by the vendor.
    pub previous_reset: DateTime<Utc>,
}

fn indexed_windows(facts: &Json) -> BTreeMap<&str, &Json> {
    facts
        .get("windows")
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(|window| Some((window.get("id")?.as_str()?, window)))
        .collect()
}

/// Detects only material falls before a known reset, matching windows by id.
#[must_use]
pub fn early_resets(before: &Json, after: &Json, at: DateTime<Utc>) -> Vec<EarlyReset> {
    let old = indexed_windows(before);
    indexed_windows(after)
        .into_iter()
        .filter_map(|(id, window)| {
            let previous = old.get(id)?;
            let before = previous.get("used_percent")?.as_f64()?;
            let after = window.get("used_percent")?.as_f64()?;
            let reset = DateTime::parse_from_rfc3339(previous.get("resets_at")?.as_str()?)
                .ok()?
                .with_timezone(&Utc);
            (before.is_finite()
                && after.is_finite()
                && before - after >= EARLY_RESET_DROP_PERCENT
                && reset - at > RESET_GRACE)
                .then(|| EarlyReset {
                    window: id.to_owned(),
                    label: window
                        .get("label")
                        .and_then(Json::as_str)
                        .unwrap_or(id)
                        .to_owned(),
                    before,
                    after,
                    previous_reset: reset,
                })
        })
        .collect()
}

/// Stores the displayed readings and any external early reset in the caller's transaction.
#[expect(
    clippy::too_many_arguments,
    reason = "the locked provider row supplies observation and audit context together"
)]
pub async fn record(
    conn: &mut Conn,
    provider: Id,
    home: Id,
    org: Id,
    before: &Json,
    after: &Json,
    overrides: &Json,
    previous_at: Option<DateTime<Utc>>,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let minute = at
        .with_second(0)
        .and_then(|at| at.with_nanosecond(0))
        .unwrap_or(at);
    let displayed = overrides.get("windows").or_else(|| after.get("windows"));
    let mut windows = BTreeMap::new();
    for window in displayed.and_then(Json::as_array).into_iter().flatten() {
        if let (Some(id), Some(percent)) = (
            window.get("id").and_then(Json::as_str),
            window.get("used_percent").and_then(Json::as_f64),
        ) && percent.is_finite()
        {
            windows.insert(id, percent);
        }
    }
    if !windows.is_empty() {
        crate::migrations::ensure_metric_partition(conn, minute.date_naive()).await?;
    }
    if !windows.is_empty() {
        let values = Json::Array(
            windows
                .into_iter()
                .map(|(window, value)| json!({"window_id":window,"value":value}))
                .collect(),
        );
        sqlx::query(
            "INSERT INTO metric_samples(at,name,labels,value)
            SELECT $1,$2,jsonb_build_object('provider',$3::uuid,'window',s.window_id),s.value
              FROM jsonb_to_recordset($4::jsonb) AS s(window_id text,value float8)
            ON CONFLICT(name,labels,at) DO UPDATE SET value=EXCLUDED.value",
        )
        .bind(minute)
        .bind(PLAN_USED_PERCENT)
        .bind(provider)
        .bind(values)
        .execute(&mut *conn)
        .await?;
    }
    let detected = early_resets(before, after, at);
    if detected.is_empty() {
        return Ok(());
    }
    // A successful explicit reset writes its receipt before applying its returned observation.
    let own: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM audit_log WHERE object_kind='provider' AND object_id=$1 AND action='provider.reset_used' AND at >= $2 AND at <= $3 AND (at > $2 OR xmin=pg_current_xact_id()::xid))")
        .bind(provider).bind(previous_at.unwrap_or(DateTime::UNIX_EPOCH)).bind(at).fetch_one(&mut *conn).await?;
    if own {
        return Ok(());
    }
    let count = |facts: &Json| {
        facts
            .pointer("/available_resets/available_count")
            .and_then(Json::as_i64)
    };
    let cause = if count(before)
        .zip(count(after))
        .is_some_and(|(before, after)| after < before)
    {
        "outside"
    } else {
        "vendor"
    };
    let mut details=detected.into_iter().map(|reset|json!({"window_id":reset.window,"window_label":reset.label,
        "before_percent":reset.before,"after_percent":reset.after,"previous_resets_at":reset.previous_reset,"cause":cause}));
    if let Some(first) = details.next() {
        let action = crate::audit::action!(
            orch_access_contract::audit::AuditAction::ProviderWindowStartedOver
        );
        let mut entry = crate::audit::Entry::new(
            crate::audit::Actor::System,
            action.as_str(),
            "provider",
            Some(provider),
            if cause == "outside" {
                "A reset was spent outside Orch"
            } else {
                "The provider started a usage window over early"
            },
        );
        entry.node_id = Some(home);
        entry.org_id = Some(org);
        entry.history = action.is_history_line();
        entry.details = first;
        let first =
            crate::audit::append(conn, &entry, &crate::audit::Origin::default(), at).await?;
        let rest: Vec<_> = details
            .map(|details| json!({"id":orch_types::new_id(),"details":details}))
            .collect();
        if !rest.is_empty() {
            // Every row describes this provider at this instant; its retained names, origin and place are shared.
            sqlx::query("INSERT INTO audit_log(id,at,actor_kind,actor_id,for_user_id,action,as_admin,history,override,
                object_kind,object_id,object_name,scope_node_ids,node_id,org_id,environment_id,summary,details,ip,user_agent,request_id)
                SELECT r.id,a.at,a.actor_kind,a.actor_id,a.for_user_id,a.action,a.as_admin,a.history,a.override,
                a.object_kind,a.object_id,a.object_name,a.scope_node_ids,a.node_id,a.org_id,a.environment_id,a.summary,r.details,a.ip,a.user_agent,a.request_id
                  FROM audit_log a CROSS JOIN jsonb_to_recordset($2::jsonb) AS r(id uuid,details jsonb) WHERE a.id=$1")
                .bind(first).bind(Json::Array(rest)).execute(&mut *conn).await?;
        }
    }
    Ok(())
}

/// A window from the bounded provider-facts snapshot, with its original look-back.
#[derive(Debug, Clone, serde::Serialize)]
pub struct WindowDefinition {
    /// Its stable source id.
    pub window_id: String,
    /// The scheduled reset minus its original duration, when both are known.
    pub natural_start: Option<orch_types::Time>,
    /// The shared forecast function's hour or day look-back, in seconds.
    pub lookback_seconds: i64,
}

/// Six hundred current observations preserve peaks without growing with stored history.
pub const MAX_PLAN_POINTS: i64 = 600;

/// One chart read covers one page of source windows and one frozen report span.
pub struct PlanRead<'a> {
    /// The authorized provider.
    pub provider: Id,
    /// Only the source windows needed by this page.
    pub windows: &'a [WindowDefinition],
    /// The report's first UTC instant.
    pub from: DateTime<Utc>,
    /// The report's exclusive last UTC instant.
    pub to: DateTime<Utc>,
    /// The observation cut shared by every page.
    pub now: DateTime<Utc>,
    /// One validated calendar interval: hour, day, week or month.
    pub bucket: &'a str,
}

/// A bounded chart payload already aggregated in SQL.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ChartRow {
    /// Its source window.
    pub window_id: String,
    /// The later of the scheduled start and the last early reset.
    pub starts_at: Option<DateTime<Utc>>,
    /// At most six hundred peak-preserving measured points.
    pub points: Json,
    /// One usage aggregate per offered report interval.
    pub history: Json,
    /// At most one representative marker per interval, carrying its exact count.
    pub resets: Json,
    /// The current cycle's latest early-reset receipt, when there is one.
    pub current_reset: Option<Json>,
    /// SQL means and counts for the one-line history summary.
    pub summary: Json,
    /// The requested interval exceeds the chart's bounded aggregate count.
    pub overflow: bool,
}

/// One window's pace evidence; observations never leave the store.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PaceRow {
    /// Its source window.
    pub window_id: String,
    /// Its reset-aware current start.
    pub starts_at: Option<DateTime<Utc>>,
    /// The earliest observation inside the clipped look-back.
    pub first_at: Option<DateTime<Utc>>,
    /// That observation's percentage.
    pub first_percent: Option<f64>,
    /// The latest actual observation.
    pub last_at: Option<DateTime<Utc>>,
    /// That observation's percentage.
    pub last_percent: Option<f64>,
    /// The sum of nonnegative rises between readings.
    pub rise: f64,
}

const READ_BASE: &str = include_str!("provider_plan_base.sql");
const CHART_QUERY: &str = include_str!("provider_plan_chart.sql");
const PACE_QUERY: &str = include_str!("provider_plan_pace.sql");

fn definitions(read: &PlanRead<'_>) -> Result<Json, DbError> {
    serde_json::to_value(read.windows)
        .map_err(|error| DbError::from(sqlx::Error::Encode(Box::new(error))))
}

/// Reads a page of chart vectors, aggregated and peak-capped before the application receives them.
pub async fn chart(conn: &mut Conn, read: &PlanRead<'_>) -> Result<Vec<ChartRow>, DbError> {
    let statement = format!("{READ_BASE}{CHART_QUERY}");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement))
        .bind(read.provider)
        .bind(definitions(read)?)
        .bind(read.from)
        .bind(read.to)
        .bind(read.now)
        .bind(read.bucket)
        .bind(MAX_PLAN_POINTS)
        .bind(orch_provider_contract::forecast::FORECAST_MAX_AGE.num_seconds())
        .bind(EARLY_RESET_DROP_PERCENT)
        .bind(RESET_GRACE.num_seconds())
        .bind(i64::from(
            orch_api_contract::usage::USAGE_DAY_BUCKET_MAX_DAYS,
        ))
        .fetch_all(conn)
        .await?)
}

/// Reads one SQL pace summary per source window, using the indexed short look-back only.
pub async fn paces(conn: &mut Conn, read: &PlanRead<'_>) -> Result<Vec<PaceRow>, DbError> {
    let statement = format!("{READ_BASE}{PACE_QUERY}");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(statement))
        .bind(read.provider)
        .bind(definitions(read)?)
        .bind(read.from)
        .bind(read.to)
        .bind(read.now)
        .bind(read.bucket)
        .bind(MAX_PLAN_POINTS)
        .bind(orch_provider_contract::forecast::FORECAST_MAX_AGE.num_seconds())
        .bind(EARLY_RESET_DROP_PERCENT)
        .bind(RESET_GRACE.num_seconds())
        .bind(i64::from(
            orch_api_contract::usage::USAGE_DAY_BUCKET_MAX_DAYS,
        ))
        .fetch_all(conn)
        .await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_five_point_fall_before_a_known_reset_is_an_early_start_over() {
        let now = DateTime::UNIX_EPOCH;
        let before = json!({"windows":[{"id":"weekly","used_percent":60,"resets_at":now + Duration::hours(1)}]});
        for (after, expected) in [(55.1, 0), (55.0, 1), (0.0, 1)] {
            let after = json!({"windows":[{"id":"weekly","label":"Weekly","used_percent":after}]});
            assert_eq!(early_resets(&before, &after, now).len(), expected);
        }
    }

    #[test]
    fn an_ordinary_scheduled_reset_or_an_unknown_reset_time_is_not_called_early() {
        let now = DateTime::UNIX_EPOCH;
        let after = json!({"windows":[{"id":"weekly","used_percent":0}]});
        for reset in [Json::Null, json!(now), json!(now + RESET_GRACE)] {
            let before = json!({"windows":[{"id":"weekly","used_percent":100,"resets_at":reset}]});
            assert!(early_resets(&before, &after, now).is_empty());
        }
    }
}
