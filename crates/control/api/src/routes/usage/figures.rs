//! Usage over a span: totals, a series cut into buckets, and breakdowns by dimension, each as deep as the reader's access to the scope goes; and the series as CSV.
//!
//! Every figure is a sum of counted ledger rows, read from the daily rollups
//! (the ledger itself for a bucket by the hour and for the dimensions the
//! rollups do not keep), so that it can be followed row by row with the same
//! scope and filters in the list of records. Credits are exact decimals and say,
//! beside them, how much of them the shipped catalogue estimated and how many
//! tokens had no rate and so are in none.

use super::csv::{self, json_response};
use super::names::Names;
use super::scope::{self, Scope, Wanted};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use axum::body::Body;
use axum::response::Response;
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use http::StatusCode;
use orch_api_contract::threads::TokenUsage;
use orch_api_contract::usage::{
    USAGE_BREAKDOWN_LIMIT_DEFAULT, USAGE_BREAKDOWN_LIMIT_MAX, USAGE_BUCKET_DAY_UP_TO_DAYS,
    USAGE_BUCKET_HOUR_UP_TO_DAYS, USAGE_BUCKET_WEEK_UP_TO_YEARS, USAGE_DAY_BUCKET_MAX_DAYS,
    USAGE_HOUR_BUCKET_MAX_DAYS, USAGE_SERIES_KEYS_MAX, UsageAnalytics, UsageBreakdown, UsageBucket,
    UsageDimension, UsageFigures, UsageGetQuery, UsageRow, UsageSeriesPoint, UsageSplit,
};
use orch_db::usage::micros::Micros;
use orch_db::usage::rollup::{self, Cell, Figures, Read};
use orch_types::{Id, Time};

/// How many days make a year, for the span a bucket by the week is chosen up to.
const DAYS_IN_A_YEAR: i64 = 365;
/// Health uses fifteen minutes, ten requests and twenty percent errors, as shown in the status part.
const HEALTH_MINUTES: i64 = 15;
const HEALTH_MIN_REQUESTS: i64 = 10;
const HEALTH_ERROR_PERCENT: i64 = 20;
/// The provider is compared with its own seven UTC calendar days, including today.
const HEALTH_BASELINE_DAYS: i64 = 7;
/// A first-output median above three times that baseline is degraded.
const HEALTH_LATENCY_MULTIPLE: i64 = 3;

/// A decimal as the answers write it: the shortest form.
fn canonical(text: &str) -> String {
    Micros::parse(text).map_or_else(|| text.to_owned(), Micros::text)
}

/// The figures of the database as the contract has them.
#[must_use]
pub fn figures(found: &Figures) -> UsageFigures {
    UsageFigures {
        performance: found.performance.figures(),
        turns: found.turns,
        tokens: TokenUsage {
            input_tokens: found.input_tokens,
            cached_input_tokens: found.cached_input_tokens,
            cache_write_tokens: found.cache_write_tokens,
            output_tokens: found.output_tokens,
            reasoning_tokens: found.reasoning_tokens,
            total_tokens: found.total_tokens,
        },
        credits: canonical(&found.credits),
        estimated_credits: canonical(&found.estimated_credits),
        unrated_tokens: found.unrated_tokens,
    }
}

fn credits_of(found: &Figures) -> Micros {
    Micros::parse(&found.credits).unwrap_or(Micros::ZERO)
}

/// The bucket a span is cut into when none is asked for.
#[must_use]
pub fn default_bucket(days: i64) -> UsageBucket {
    if days <= i64::from(USAGE_BUCKET_HOUR_UP_TO_DAYS) {
        UsageBucket::Hour
    } else if days <= i64::from(USAGE_BUCKET_DAY_UP_TO_DAYS) {
        UsageBucket::Day
    } else if days <= i64::from(USAGE_BUCKET_WEEK_UP_TO_YEARS) * DAYS_IN_A_YEAR {
        UsageBucket::Week
    } else {
        UsageBucket::Month
    }
}

/// The breakdowns a scope has unless it is asked for others.
fn default_breakdowns(scope: Scope) -> Vec<UsageDimension> {
    match scope {
        Scope::Me => vec![
            UsageDimension::Project,
            UsageDimension::Model,
            UsageDimension::Provider,
            UsageDimension::Agent,
            UsageDimension::Environment,
        ],
        Scope::Node(_) => vec![
            UsageDimension::Person,
            UsageDimension::Model,
            UsageDimension::Provider,
            UsageDimension::Agent,
            UsageDimension::Environment,
        ],
        Scope::Provider(_) => vec![
            UsageDimension::Person,
            UsageDimension::Model,
            UsageDimension::Org,
            UsageDimension::Environment,
        ],
    }
}

fn dimensions(text: &str) -> ApiResult<Vec<UsageDimension>> {
    let mut found: Vec<UsageDimension> = Vec::new();
    for word in text
        .split(',')
        .map(str::trim)
        .filter(|word| !word.is_empty())
    {
        let dimension: UsageDimension =
            serde_json::from_value(serde_json::Value::String(word.to_owned())).map_err(|_| {
                ApiError::invalid("breakdowns", &format!("{word} is not a dimension"))
            })?;
        if !found.contains(&dimension) {
            found.push(dimension);
        }
    }
    Ok(found)
}

/// The start of every bucket of a span, in order.
fn bucket_starts(from: NaiveDate, to: NaiveDate, bucket: UsageBucket) -> Vec<DateTime<Utc>> {
    let midnight = |day: NaiveDate| day.and_hms_opt(0, 0, 0).map(|moment| moment.and_utc());
    let mut starts = Vec::new();
    match bucket {
        UsageBucket::Hour => {
            let (Some(mut at), Some(end)) = (midnight(from), to.succ_opt().and_then(midnight))
            else {
                return starts;
            };
            while at < end {
                starts.push(at);
                at += Duration::hours(1);
            }
        }
        UsageBucket::Day | UsageBucket::Week => {
            let step = if bucket == UsageBucket::Day { 1 } else { 7 };
            let mut day = rollup::bucket_start(from, bucket);
            while day <= to {
                starts.extend(midnight(day));
                day += Duration::days(step);
            }
        }
        UsageBucket::Month => {
            let mut day = rollup::bucket_start(from, bucket);
            while day <= to {
                starts.extend(midnight(day));
                let (year, month) = if day.month() == 12 {
                    (day.year() + 1, 1)
                } else {
                    (day.year(), day.month() + 1)
                };
                let Some(next) = NaiveDate::from_ymd_opt(year, month, 1) else {
                    break;
                };
                day = next;
            }
        }
    }
    starts
}

/// The cells of a read ordered the largest first: by credits, then by tokens, then by key.
fn ranked(
    mut cells: Vec<Cell>,
    measure: Option<orch_api_contract::usage::UsageMeasure>,
) -> Vec<Cell> {
    cells.sort_by(|a, b| {
        use orch_api_contract::usage::UsageMeasure;
        if matches!(
            measure,
            Some(UsageMeasure::Speed | UsageMeasure::FirstToken | UsageMeasure::Errors)
        ) {
            let value = |cell: &Cell| {
                let p = cell.figures.performance.figures();
                match measure {
                    Some(UsageMeasure::Speed) => p.speed_milli.map(|n| n as f64),
                    Some(UsageMeasure::FirstToken) => p.first_token_ms.map(|n| n as f64),
                    Some(UsageMeasure::Errors) if p.requests > 0 => {
                        Some(p.errors.values().sum::<i64>() as f64 / p.requests as f64)
                    }
                    _ => None,
                }
                .unwrap_or(f64::NEG_INFINITY)
            };
            return value(b)
                .total_cmp(&value(a))
                .then_with(|| a.key.cmp(&b.key));
        }
        credits_of(&b.figures)
            .cmp(&credits_of(&a.figures))
            .then_with(|| b.figures.total_tokens.cmp(&a.figures.total_tokens))
            .then_with(|| a.key.cmp(&b.key))
    });
    cells
}

/// Adds up figures.
fn added(all: &[&Figures]) -> Figures {
    let sum = |each: fn(&Figures) -> i64| all.iter().map(|one| each(one)).sum::<i64>();
    let credits: Micros = all.iter().map(|one| credits_of(one)).sum();
    let estimated: Micros = all
        .iter()
        .map(|one| Micros::parse(&one.estimated_credits).unwrap_or(Micros::ZERO))
        .sum();
    Figures {
        performance: all.iter().fold(
            orch_db::usage::performance::Sums::default(),
            |mut sum, one| {
                sum.plus(&one.performance);
                sum
            },
        ),
        turns: sum(|one| one.turns),
        input_tokens: sum(|one| one.input_tokens),
        cached_input_tokens: sum(|one| one.cached_input_tokens),
        cache_write_tokens: sum(|one| one.cache_write_tokens),
        output_tokens: sum(|one| one.output_tokens),
        reasoning_tokens: sum(|one| one.reasoning_tokens),
        total_tokens: sum(|one| one.total_tokens),
        credits: credits.text(),
        estimated_credits: estimated.text(),
        unrated_tokens: sum(|one| one.unrated_tokens),
    }
}

/// Reads usage over a span.
pub async fn get(ctx: Ctx, query: UsageGetQuery, _body: Body) -> ApiResult<Response> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let asked = scope::parse(&query.scope)?;
    let opened = scope::open(&mut conn, &principal, asked).await?;
    let from = scope::parse_day("from", &query.from)?;
    let to = scope::parse_day("to", &query.to)?;
    if to < from {
        return Err(ApiError::invalid("to", "is not before from"));
    }
    let days = rollup::days_in(from, to);
    let bucket = query.bucket.unwrap_or_else(|| default_bucket(days));
    if bucket == UsageBucket::Hour && days > i64::from(USAGE_HOUR_BUCKET_MAX_DAYS) {
        return Err(ApiError::invalid(
            "bucket",
            &format!("by the hour is for a span of up to {USAGE_HOUR_BUCKET_MAX_DAYS} days"),
        ));
    }
    if bucket == UsageBucket::Day && days > i64::from(USAGE_DAY_BUCKET_MAX_DAYS) {
        return Err(ApiError::invalid(
            "bucket",
            &format!("by the day is for a span of up to {USAGE_DAY_BUCKET_MAX_DAYS} days"),
        ));
    }
    let breakdowns = match query.breakdowns.as_deref() {
        Some(text) => dimensions(text)?,
        None => default_breakdowns(asked),
    };
    for dimension in query.series_by.iter().chain(breakdowns.iter()) {
        scope::require_dimension(&mut conn, &principal, &opened, *dimension, "this dimension")
            .await?;
    }
    let wanted = Wanted {
        user_id: query.user_id,
        model: query.model.clone(),
        provider_id: query.provider_id,
        harness_kind: query.harness_kind.clone(),
        environment_kind: query.environment_kind,
        project_id: query.project_id,
        topic_id: query.topic_id,
        thread_id: query.thread_id,
    };
    let mut filter = scope::filter(&mut conn, &principal, &opened, &wanted, false).await?;
    filter.effort = query.effort.clone();
    filter.tier = query.tier.clone();
    filter.program = query.program.clone();
    filter.outcome = query.outcome;
    filter.org = query.org_id;
    if query.turn_id.is_some() {
        scope::require_dimension(
            &mut conn,
            &principal,
            &opened,
            UsageDimension::Thread,
            "the use of one turn",
        )
        .await?;
    }
    filter.turn = query.turn_id;
    let limit = usize::try_from(
        query
            .limit
            .unwrap_or(USAGE_BREAKDOWN_LIMIT_DEFAULT)
            .clamp(1, USAGE_BREAKDOWN_LIMIT_MAX),
    )
    .unwrap_or(usize::MAX);
    let under = opened.node;
    let read = |bucket: Option<UsageBucket>, by: Option<UsageDimension>| Read {
        from,
        to,
        filter: &filter,
        bucket,
        by,
        under,
    };

    let totals = rollup::read(&mut conn, &read(None, None))
        .await?
        .into_iter()
        .next()
        .map(|cell| cell.figures)
        .unwrap_or_default();
    let points = rollup::read(&mut conn, &read(Some(bucket), None)).await?;
    let own_part = if opened.has_own_part {
        let own = orch_db::usage::rollup::Filter {
            person: Some(opened.caller),
            ..filter.clone()
        };
        let cells = rollup::read(
            &mut conn,
            &Read {
                from,
                to,
                filter: &own,
                bucket: None,
                by: None,
                under,
            },
        )
        .await?;
        Some(
            cells
                .into_iter()
                .next()
                .map(|cell| cell.figures)
                .unwrap_or_default(),
        )
    } else {
        None
    };

    // The series, each point split by a dimension into its largest keys and the rest.
    let split = match query.series_by {
        Some(dimension) => {
            let over_span = ranked(
                rollup::read(&mut conn, &read(None, Some(dimension))).await?,
                query.measure,
            );
            let per_bucket = rollup::read(&mut conn, &read(Some(bucket), Some(dimension))).await?;
            let keep = usize::try_from(USAGE_SERIES_KEYS_MAX).unwrap_or(usize::MAX);
            let kept: Vec<Option<String>> = over_span
                .iter()
                .take(keep)
                .map(|cell| cell.key.clone())
                .collect();
            let values: Vec<Option<String>> =
                over_span.iter().map(|cell| cell.key.clone()).collect();
            let names = Names::for_dimension(&mut conn, &principal, dimension, &values).await?;
            Some((dimension, kept, per_bucket, names, over_span.len() > keep))
        }
        None => None,
    };

    let mut series: Vec<UsageSeriesPoint> = Vec::new();
    for start in bucket_starts(from, to, bucket) {
        let here = points.iter().find(|cell| cell.bucket == Some(start));
        let by = match &split {
            None => Vec::new(),
            Some((dimension, kept, per_bucket, names, has_rest)) => {
                let in_bucket: Vec<&Cell> = per_bucket
                    .iter()
                    .filter(|cell| cell.bucket == Some(start))
                    .collect();
                let mut by: Vec<UsageSplit> = kept
                    .iter()
                    .map(|key| UsageSplit {
                        key: names.key(*dimension, key.as_deref()),
                        figures: figures(
                            &in_bucket
                                .iter()
                                .find(|cell| &cell.key == key)
                                .map(|cell| cell.figures.clone())
                                .unwrap_or_default(),
                        ),
                    })
                    .collect();
                if *has_rest {
                    let rest: Vec<&Figures> = in_bucket
                        .iter()
                        .filter(|cell| !kept.contains(&cell.key))
                        .map(|cell| &cell.figures)
                        .collect();
                    by.push(UsageSplit {
                        key: orch_api_contract::usage::UsageKey::default(),
                        figures: figures(&added(&rest)),
                    });
                }
                by
            }
        };
        series.push(UsageSeriesPoint {
            at: Time::new(start),
            figures: figures(&here.map(|cell| cell.figures.clone()).unwrap_or_default()),
            by,
        });
    }

    // The totals by dimension, the largest first.
    let total_credits = credits_of(&totals);
    let mut tables: Vec<UsageBreakdown> = Vec::new();
    for dimension in breakdowns {
        let cells = ranked(
            rollup::read(&mut conn, &read(None, Some(dimension))).await?,
            query.measure,
        );
        let values: Vec<Option<String>> = cells.iter().map(|cell| cell.key.clone()).collect();
        let names = Names::for_dimension(&mut conn, &principal, dimension, &values).await?;
        let more = i64::try_from(cells.len().saturating_sub(limit)).unwrap_or(i64::MAX);
        let rows = cells
            .iter()
            .take(limit)
            .map(|cell| UsageRow {
                key: names.key(dimension, cell.key.as_deref()),
                share: if total_credits > Micros::ZERO {
                    credits_of(&cell.figures).share_of(total_credits)
                } else {
                    Micros::whole(cell.figures.total_tokens)
                        .share_of(Micros::whole(totals.total_tokens))
                },
                figures: figures(&cell.figures),
            })
            .collect();
        tables.push(UsageBreakdown {
            by: dimension,
            rows,
            more,
        });
    }

    let previous = rollup::read(
        &mut conn,
        &Read {
            from: from - Duration::days(days),
            to: from - Duration::days(1),
            filter: &filter,
            bucket: None,
            by: None,
            under,
        },
    )
    .await?
    .first()
    .map(|cell| figures(&cell.figures))
    .unwrap_or_else(|| figures(&Figures::default()));
    let answer = UsageAnalytics {
        figures_before: Some(previous.clone()),
        degradation: if query.health == Some(true) && matches!(asked, Scope::Provider(_)) {
            Some(provider_health(&mut conn, &opened.base, ctx.app.clock.now()).await?)
        } else {
            None
        },
        performance_before: previous.performance,
        comparison: rollup::comparison(&mut conn, from, to, &filter).await?,
        scope: opened.scope.clone(),
        from: query.from.clone(),
        to: query.to.clone(),
        bucket,
        depth: opened.depth,
        totals: figures(&totals),
        series,
        breakdowns: tables,
        me: own_part.as_ref().map(figures),
    };
    if scope::wants_csv(&ctx.headers) {
        return Ok(csv::response(series_csv(&answer)));
    }
    json_response(&answer, StatusCode::OK)
}

/// The status bar reads only a fifteen-minute indexed window and one seven-day aggregate.
pub async fn health(
    ctx: Ctx,
    query: orch_api_contract::usage::UsageHealthGetQuery,
    (): (),
) -> ApiResult<orch_api_contract::usage::UsageDegradation> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    health_for(
        &mut conn,
        &principal,
        query.provider_id,
        ctx.app.clock.now(),
    )
    .await
}

/// The HTTP reader and live provider status share the same authority and bounded aggregates.
pub(crate) async fn health_for(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    provider: Id,
    now: DateTime<Utc>,
) -> ApiResult<orch_api_contract::usage::UsageDegradation> {
    let opened = scope::open(conn, principal, Scope::Provider(provider)).await?;
    provider_health(conn, &opened.base, now).await
}

async fn provider_health(
    conn: &mut orch_db::Conn,
    filter: &rollup::Filter,
    now: DateTime<Utc>,
) -> ApiResult<orch_api_contract::usage::UsageDegradation> {
    let since = now - Duration::minutes(HEALTH_MINUTES);
    let recent = rollup::window(conn, since, now + Duration::milliseconds(1), filter)
        .await?
        .performance
        .figures();
    let baseline = rollup::performance_of(
        conn,
        &Read {
            from: now.date_naive() - Duration::days(HEALTH_BASELINE_DAYS - 1),
            to: now.date_naive(),
            filter,
            bucket: None,
            by: None,
            under: None,
        },
    )
    .await?
    .first()
    .map(|cell| cell.figures.performance.figures())
    .unwrap_or_default();
    let (errors, first_output) = degraded(&recent, &baseline);
    Ok(orch_api_contract::usage::UsageDegradation {
        since: since.into(),
        recent,
        baseline,
        errors,
        first_output,
    })
}

/// Whether a provider meets the error or latency degradation thresholds.
fn degraded(
    recent: &orch_api_contract::usage::UsagePerformance,
    baseline: &orch_api_contract::usage::UsagePerformance,
) -> (bool, bool) {
    let failed = recent.error_count();
    let errors = recent.requests >= HEALTH_MIN_REQUESTS
        && failed.saturating_mul(100) >= recent.requests.saturating_mul(HEALTH_ERROR_PERCENT);
    let latency = recent.first_token_samples >= HEALTH_MIN_REQUESTS
        && baseline.first_token_samples >= HEALTH_MIN_REQUESTS
        && recent
            .first_token_ms
            .zip(baseline.first_token_ms)
            .is_some_and(|(recent, baseline)| {
                recent > baseline.saturating_mul(HEALTH_LATENCY_MULTIPLE)
            });
    (errors, latency)
}

/// The series as CSV: a line for each point.
fn series_csv(answer: &UsageAnalytics) -> String {
    let mut text = csv::line(&[
        "at",
        "turns",
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "total_tokens",
        "credits",
        "estimated_credits",
        "unrated_tokens",
        "requests",
        "speed_tokens_per_second",
        "speed_p90_tokens_per_second",
        "first_output_ms",
        "first_output_p90_ms",
        "errors_by_code",
    ]);
    for point in &answer.series {
        let each = &point.figures;
        text.push_str(&csv::line(&[
            point.at.get().format("%Y-%m-%dT%H:%M:%SZ").to_string(),
            each.turns.to_string(),
            each.tokens.input_tokens.to_string(),
            each.tokens.cached_input_tokens.to_string(),
            each.tokens.output_tokens.to_string(),
            each.tokens.reasoning_tokens.to_string(),
            each.tokens.total_tokens.to_string(),
            each.credits.clone(),
            each.estimated_credits.clone(),
            each.unrated_tokens.to_string(),
            each.performance.requests.to_string(),
            each.performance
                .speed_milli
                .map(|n| (n as f64 / 1000.0).to_string())
                .unwrap_or_default(),
            each.performance
                .speed_p90_milli
                .map(|n| (n as f64 / 1000.0).to_string())
                .unwrap_or_default(),
            each.performance
                .first_token_ms
                .map(|n| n.to_string())
                .unwrap_or_default(),
            each.performance
                .first_token_p90_ms
                .map(|n| n.to_string())
                .unwrap_or_default(),
            serde_json::to_string(&each.performance.errors).unwrap_or_default(),
        ]));
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn degradation_requires_ten_requests_and_twenty_percent_errors_or_more_than_triple_latency() {
        use orch_api_contract::usage::UsagePerformance;
        let baseline = UsagePerformance {
            first_token_ms: Some(1000),
            first_token_samples: 10,
            ..Default::default()
        };
        let mut recent = UsagePerformance {
            requests: 9,
            errors: std::collections::BTreeMap::from([("provider_busy".into(), 2)]),
            first_token_ms: Some(3000),
            ..Default::default()
        };
        assert_eq!(degraded(&recent, &baseline), (false, false));
        recent.requests = 10;
        assert_eq!(degraded(&recent, &baseline), (true, false));
        recent.errors.clear();
        recent.first_token_ms = Some(3001);
        recent.first_token_samples = 9;
        assert_eq!(degraded(&recent, &baseline), (false, false));
        recent.first_token_samples = 10;
        assert_eq!(degraded(&recent, &baseline), (false, true));
        recent.first_token_ms = None;
        recent.errors = std::collections::BTreeMap::from([
            ("harness_error".into(), 5),
            ("session_lost".into(), 2),
            ("environment_unreachable".into(), 3),
        ]);
        assert_eq!(degraded(&recent, &baseline), (false, false));
    }

    fn day(y: i32, m: u32, d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(y, m, d).unwrap()
    }

    #[test]
    fn a_span_is_cut_by_the_hour_up_to_two_days_by_the_day_to_92_by_the_week_to_two_years_and_then_by_the_month()
     {
        assert_eq!(default_bucket(1), UsageBucket::Hour);
        assert_eq!(default_bucket(2), UsageBucket::Hour);
        assert_eq!(default_bucket(3), UsageBucket::Day);
        assert_eq!(default_bucket(92), UsageBucket::Day);
        assert_eq!(default_bucket(93), UsageBucket::Week);
        assert_eq!(default_bucket(730), UsageBucket::Week);
        assert_eq!(default_bucket(731), UsageBucket::Month);
    }

    #[test]
    fn every_bucket_of_a_span_has_a_point_and_a_week_begins_on_the_monday_before_the_span() {
        // Wednesday the 14th to Tuesday the 20th of October 2026.
        let weeks = bucket_starts(day(2026, 10, 14), day(2026, 10, 20), UsageBucket::Week);
        assert_eq!(weeks.len(), 2);
        assert_eq!(weeks[0].date_naive(), day(2026, 10, 12));
        assert_eq!(weeks[1].date_naive(), day(2026, 10, 19));
        assert_eq!(
            bucket_starts(day(2026, 10, 14), day(2026, 10, 15), UsageBucket::Hour).len(),
            48
        );
        let months = bucket_starts(day(2026, 11, 20), day(2027, 2, 3), UsageBucket::Month);
        assert_eq!(
            months.iter().map(DateTime::date_naive).collect::<Vec<_>>(),
            vec![
                day(2026, 11, 1),
                day(2026, 12, 1),
                day(2027, 1, 1),
                day(2027, 2, 1)
            ]
        );
    }

    #[test]
    fn credits_are_written_in_their_shortest_exact_form() {
        assert_eq!(canonical("2.500000"), "2.5");
        assert_eq!(canonical("0"), "0");
        assert_eq!(canonical("0.003550"), "0.00355");
    }
}
