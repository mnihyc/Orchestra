//! Visible provider plan measurements, peak-preserving charts and reset-aware forecasts.

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use base64::Engine as _;

use chrono::DateTime;

use chrono::Duration;

use chrono::NaiveDate;

use chrono::Utc;

use orch_api_contract::common::DEFAULT_LIMIT;

use orch_api_contract::common::MAX_LIMIT;

use orch_api_contract::common::Page;

use orch_api_contract::resources::Provider;

use orch_api_contract::resources::ProviderCreditMember;

use orch_api_contract::resources::ProviderCreditQuantity;

use orch_api_contract::resources::ProviderCreditSeries;

use orch_api_contract::resources::ProviderCreditSeriesKind;

use orch_api_contract::resources::ProviderHistoryMetric;

use orch_api_contract::resources::ProviderHistorySeries;

use orch_api_contract::resources::ProviderPlanGetQuery;

use orch_api_contract::resources::ProviderPlanSeries;

use orch_api_contract::resources::UsageWindow;

use orch_api_contract::usage::USAGE_DAY_BUCKET_MAX_DAYS;

use orch_api_contract::usage::USAGE_HOUR_BUCKET_MAX_DAYS;

use orch_api_contract::usage::UsageBucket;

use orch_db::queries::provider_credits;

use orch_db::queries::provider_plan as rows;

use orch_db::queries::provider_plan::PlanRead;

use orch_db::queries::provider_plan::WindowDefinition;

use orch_provider_contract::forecast::FORECAST_LONG_LOOKBACK;

use orch_provider_contract::forecast::FORECAST_SHORT_LOOKBACK;

use orch_provider_contract::forecast::ForecastSource;

use orch_provider_contract::forecast::ReadingPace;

use orch_provider_contract::forecast::forecast;

use orch_types::Hash;

use orch_types::Time;

use std::collections::BTreeMap;
fn readable(provider: &Provider) -> bool {
    provider
        .access
        .permissions
        .iter()
        .any(|permission| permission == "provider.view")
}

fn scheduled_start(window: &UsageWindow) -> Option<DateTime<Utc>> {
    let length = Duration::try_minutes(window.duration_minutes.filter(|length| *length > 0)?)?;
    window.resets_at?.get().checked_sub_signed(length)
}

fn definition(window: &UsageWindow) -> WindowDefinition {
    let long = window
        .duration_minutes
        .and_then(Duration::try_minutes)
        .is_some_and(|length| length >= FORECAST_LONG_LOOKBACK);
    WindowDefinition {
        window_id: window.id.clone(),
        natural_start: scheduled_start(window).map(Time::new),
        lookback_seconds: if long {
            FORECAST_LONG_LOOKBACK
        } else {
            FORECAST_SHORT_LOOKBACK
        }
        .num_seconds(),
    }
}

fn bucket_name(bucket: UsageBucket) -> &'static str {
    match bucket {
        UsageBucket::Hour => "hour",
        UsageBucket::Day => "day",
        UsageBucket::Week => "week",
        UsageBucket::Month => "month",
    }
}

/// Adds one SQL pace summary per window, without loading that window's stored history.
pub async fn add_forecasts(
    conn: &mut orch_db::Conn,
    provider: &mut Provider,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    if !readable(provider) || provider.facts.windows.is_empty() {
        return Ok(());
    }
    provider.plan_forecasts = Some(BTreeMap::new());
    if provider.facts_at.is_none() {
        return Ok(());
    }
    let windows: Vec<_> = provider
        .facts
        .windows
        .iter()
        .filter(|window| scheduled_start(window).is_some())
        .map(definition)
        .collect();
    let Some(from) = windows
        .iter()
        .filter_map(|window| window.natural_start.map(|at| at.get()))
        .min()
    else {
        return Ok(());
    };
    let paces = rows::paces(
        conn,
        &PlanRead {
            provider: provider.base.id,
            windows: &windows,
            from,
            to: now,
            now,
            bucket: "day",
        },
    )
    .await?;
    let by_id: BTreeMap<_, _> = provider
        .facts
        .windows
        .iter()
        .map(|window| (window.id.as_str(), window))
        .collect();
    let forecasts = paces
        .into_iter()
        .filter_map(|pace| {
            let window = by_id.get(pace.window_id.as_str())?;
            let evidence = ReadingPace {
                first_at: Time::new(pace.first_at?),
                first_percent: pace.first_percent?,
                last_at: Time::new(pace.last_at?),
                last_percent: pace.last_percent?,
                rise: pace.rise,
            };
            forecast(
                pace.starts_at?,
                window.resets_at?.get(),
                Duration::try_minutes(window.duration_minutes?)?,
                now,
                ForecastSource::Aggregate(evidence),
            )
            .map(|projection| (pace.window_id, projection))
        })
        .collect();
    provider.plan_forecasts = Some(forecasts);
    Ok(())
}

fn date(name: &str, value: &str) -> ApiResult<DateTime<Utc>> {
    if value.len() != 10 || !value.as_bytes()[..4].iter().all(u8::is_ascii_digit) {
        return Err(ApiError::invalid(name, "must be a UTC date"));
    }
    NaiveDate::parse_from_str(value, "%Y-%m-%d")
        .ok()
        .and_then(|day| day.and_hms_opt(0, 0, 0))
        .map(|day| day.and_utc())
        .ok_or_else(|| ApiError::invalid(name, "must be a UTC date"))
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    epoch: Hash,
    after: String,
    as_of: Time,
}

fn encode(cursor: &Cursor) -> ApiResult<String> {
    let bytes = serde_json::to_vec(cursor).map_err(ApiError::internal)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn decode(value: &str) -> ApiResult<Cursor> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(value)
        .map_err(|_| ApiError::invalid("cursor", "is not a cursor this plan gave"))?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::invalid("cursor", "is not a cursor this plan gave"))
}

/// A single bounded page supplies both chart modes, with exact SQL history counts.
pub async fn get(
    ctx: Ctx,
    query: ProviderPlanGetQuery,
    (): (),
) -> ApiResult<Page<ProviderHistorySeries>> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| {
        let query = query.clone();
        async move {
            let completed =
                crate::key_operations::native::read_once(ctx, move |ctx, conn, principal| {
                    Box::pin(async move {
                        super::providers::provider_for(
                            conn,
                            principal,
                            ctx.id("id")?,
                            orch_access_contract::permissions::Permission::ProviderView,
                        )
                        .await?;
                        window_page(ctx, conn, query).await
                    })
                })
                .await?;
            Ok(completed.value)
        }
    })
    .await
}

async fn window_page(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    query: ProviderPlanGetQuery,
) -> ApiResult<Page<ProviderHistorySeries>> {
    if query.metric == Some(ProviderHistoryMetric::Credits) {
        return credits(ctx, conn, query).await;
    }
    let id = ctx.id("id")?;
    let row = orch_db::queries::resources::routes::providers(&mut *conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let (facts, _) = super::providers::facts_shown(&row.facts, &row.facts_overrides)?;
    let from = date("from", &query.from)?;
    let to = date("to", &query.to)?
        .checked_add_signed(Duration::days(1))
        .ok_or_else(|| ApiError::invalid("to", "is outside the supported dates"))?;
    if from >= to {
        return Err(ApiError::invalid("to", "must not be before from"));
    }
    let bucket = query.bucket.unwrap_or(UsageBucket::Day);
    let days = (to - from).num_days();
    if bucket == UsageBucket::Hour && days > i64::from(USAGE_HOUR_BUCKET_MAX_DAYS)
        || bucket == UsageBucket::Day && days > i64::from(USAGE_DAY_BUCKET_MAX_DAYS)
    {
        return Err(ApiError::invalid("bucket", "is too fine for this span"));
    }
    // This is one negotiated source snapshot, never a history/table read; the large readings stay in SQL.
    let epoch = Hash::of(
        &serde_json::to_vec(&(
            id,
            &facts.windows,
            row.facts_at,
            &query.from,
            &query.to,
            bucket,
        ))
        .map_err(ApiError::internal)?,
    );
    let cursor = query.cursor.as_deref().map(decode).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.epoch != epoch || cursor.as_of.get() > ctx.now)
    {
        return Err(ApiError::invalid(
            "cursor",
            "the plan changed; reopen its first page",
        ));
    }
    let as_of = cursor.as_ref().map_or(ctx.now, |cursor| cursor.as_of.get());
    let mut windows: Vec<_> = facts
        .windows
        .into_iter()
        .map(|window| (window.id.clone(), window))
        .collect::<BTreeMap<_, _>>()
        .into_values()
        .collect();
    windows.sort_by(|a, b| {
        (b.is_active == Some(true))
            .cmp(&(a.is_active == Some(true)))
            .then_with(|| {
                a.duration_minutes
                    .filter(|n| *n > 0)
                    .unwrap_or(i64::MAX)
                    .cmp(&b.duration_minutes.filter(|n| *n > 0).unwrap_or(i64::MAX))
            })
            .then_with(|| a.id.cmp(&b.id))
    });
    let after = match cursor.as_ref() {
        None => 0,
        Some(cursor) => windows
            .iter()
            .position(|window| window.id == cursor.after)
            .map(|index| index + 1)
            .ok_or_else(|| ApiError::invalid("cursor", "is not a cursor this plan gave"))?,
    };
    let limit = usize::try_from(query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT))
        .map_err(ApiError::internal)?;
    let selected: Vec<_> = windows.iter().skip(after).take(limit).collect();
    let more = windows.len().saturating_sub(after) > selected.len();
    let definitions: Vec<_> = selected.iter().map(|window| definition(window)).collect();
    let data = if definitions.is_empty() {
        Vec::new()
    } else {
        rows::chart(
            &mut *conn,
            &PlanRead {
                provider: id,
                windows: &definitions,
                from,
                to,
                now: as_of,
                bucket: bucket_name(bucket),
            },
        )
        .await?
    };
    if data.iter().any(|row| row.overflow) {
        return Err(ApiError::invalid(
            "bucket",
            "is too fine for the retained history; choose a larger interval",
        ));
    }
    let mut data: BTreeMap<_, _> = data
        .into_iter()
        .map(|row| (row.window_id.clone(), row))
        .collect();
    let items = selected
        .iter()
        .map(|window| {
            let row = data
                .remove(&window.id)
                .ok_or_else(|| ApiError::internal("a requested plan window was not aggregated"))?;
            Ok(ProviderPlanSeries {
                window_id: row.window_id,
                starts_at: row.starts_at.map(Time::new),
                points: serde_json::from_value(row.points).map_err(ApiError::internal)?,
                history: serde_json::from_value(row.history).map_err(ApiError::internal)?,
                resets: serde_json::from_value(row.resets).map_err(ApiError::internal)?,
                current_reset: row
                    .current_reset
                    .map(serde_json::from_value)
                    .transpose()
                    .map_err(ApiError::internal)?,
                summary: serde_json::from_value(row.summary).map_err(ApiError::internal)?,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let next_cursor = if more {
        selected
            .last()
            .map(|window| {
                encode(&Cursor {
                    epoch,
                    after: window.id.clone(),
                    as_of: Time::new(as_of),
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(Page {
        items: items
            .into_iter()
            .map(ProviderHistorySeries::Subscription)
            .collect(),
        next_cursor,
    })
}

/// Exact credit observations use this same ordinary ProviderPlanGet gate and history span.
/// Groups page together, so the reported used amount and limit cannot be split arbitrarily.
async fn credits(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    query: ProviderPlanGetQuery,
) -> ApiResult<Page<ProviderHistorySeries>> {
    let id = ctx.id("id")?;
    let row = orch_db::queries::resources::routes::providers(&mut *conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let source_revision = provider_credits::source_revision(&mut *conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let from = date("from", &query.from)?;
    let to = date("to", &query.to)?
        .checked_add_signed(Duration::days(1))
        .ok_or_else(|| ApiError::invalid("to", "is outside the supported dates"))?;
    if from >= to {
        return Err(ApiError::invalid("to", "must not be before from"));
    }
    let bucket = query.bucket.unwrap_or(UsageBucket::Day);
    let days = (to - from).num_days();
    if bucket == UsageBucket::Hour && days > i64::from(USAGE_HOUR_BUCKET_MAX_DAYS)
        || bucket == UsageBucket::Day && days > i64::from(USAGE_DAY_BUCKET_MAX_DAYS)
    {
        return Err(ApiError::invalid("bucket", "is too fine for this span"));
    }
    // The safe observation/source cut distinguishes this metric from subscription cursors.
    // No configuration, source address, credential identity or amount becomes a series label.
    let epoch = Hash::of(
        &serde_json::to_vec(&(
            id,
            "credits",
            source_revision,
            row.facts_at,
            row.revision,
            &query.from,
            &query.to,
            bucket,
        ))
        .map_err(ApiError::internal)?,
    );
    let cursor = query.cursor.as_deref().map(decode).transpose()?;
    if cursor
        .as_ref()
        .is_some_and(|cursor| cursor.epoch != epoch || cursor.as_of.get() > ctx.now)
    {
        return Err(ApiError::invalid(
            "cursor",
            "the plan changed; reopen its first page",
        ));
    }
    let as_of = cursor.as_ref().map_or(ctx.now, |cursor| cursor.as_of.get());
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let data = provider_credits::read(
        &mut *conn,
        &provider_credits::Read {
            provider: id,
            from,
            to,
            as_of,
            bucket: bucket_name(bucket),
            after: cursor.as_ref().map(|cursor| cursor.after.as_str()),
            limit: i64::from(limit),
        },
    )
    .await?;
    let groups: Vec<_> = data
        .iter()
        .map(|row| row.group_key.clone())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let limit = usize::try_from(limit).map_err(ApiError::internal)?;
    let selected: std::collections::BTreeSet<_> = groups.iter().take(limit).cloned().collect();
    let more = groups.len() > limit;
    let mut items = Vec::new();
    for row in data
        .into_iter()
        .filter(|row| selected.contains(&row.group_key))
    {
        if row.overflow {
            return Err(ApiError::invalid(
                "bucket",
                "is too fine for the retained history; choose a larger interval",
            ));
        }
        let member = match row.member.as_str() {
            "balances" => ProviderCreditMember::Balances,
            "spend" => ProviderCreditMember::Spend,
            "extra_usage" => ProviderCreditMember::ExtraUsage,
            _ => return Err(ApiError::internal("a credit observation member is unknown")),
        };
        let quantity = match row.quantity.as_str() {
            "remaining" => ProviderCreditQuantity::Remaining,
            "total" => ProviderCreditQuantity::Total,
            "used" => ProviderCreditQuantity::Used,
            "limit" => ProviderCreditQuantity::Limit,
            _ => {
                return Err(ApiError::internal(
                    "a credit observation quantity is unknown",
                ));
            }
        };
        items.push(ProviderHistorySeries::Credits(ProviderCreditSeries {
            kind: ProviderCreditSeriesKind::Credits,
            series_id: Hash::of(row.key.as_bytes()).to_string(),
            group_id: Hash::of(row.group_key.as_bytes()).to_string(),
            member,
            item_id: row.item_id,
            quantity,
            label: row.label,
            unit: row.unit,
            decimal_places: row
                .decimal_places
                .map(u32::try_from)
                .transpose()
                .map_err(ApiError::internal)?,
            points: serde_json::from_value(row.points).map_err(ApiError::internal)?,
        }));
    }
    let next_cursor = if more {
        groups
            .get(limit.saturating_sub(1))
            .map(|after| {
                encode(&Cursor {
                    epoch,
                    after: after.clone(),
                    as_of: Time::new(as_of),
                })
            })
            .transpose()?
    } else {
        None
    };
    Ok(Page { items, next_cursor })
}
