//! Usage over a span: totals, a series cut into buckets, and breakdowns by dimension, each as deep as the reader's access to the scope goes; and the series as CSV.
//!
//! Every figure is a sum of counted ledger rows, read from the daily rollups
//! (the ledger itself for a bucket by the hour and for the dimensions the
//! rollups do not keep), so that it can be followed row by row with the same
//! scope and filters in the list of records. Credits are exact decimals and say,
//! beside them, how much of them the shipped catalogue estimated and how many
//! tokens had no rate and so are in none.

use super::names::Names;

pub use super::scope;

pub use super::scope::Scope;

use super::scope::Wanted;

pub use crate::middleware::requests::ApiError;

pub use crate::middleware::requests::ApiResult;

pub use crate::registry::Ctx;

pub use chrono::Duration;

use orch_api_contract::usage::USAGE_BREAKDOWN_LIMIT_DEFAULT;

use orch_api_contract::usage::USAGE_BREAKDOWN_LIMIT_MAX;

use orch_api_contract::usage::USAGE_DAY_BUCKET_MAX_DAYS;

use orch_api_contract::usage::USAGE_HOUR_BUCKET_MAX_DAYS;

use orch_api_contract::usage::USAGE_SERIES_KEYS_MAX;

pub use orch_api_contract::usage::UsageAnalytics;

use orch_api_contract::usage::UsageBreakdown;

pub use orch_api_contract::usage::UsageBucket;

pub use orch_api_contract::usage::UsageDimension;

pub use orch_api_contract::usage::UsageGetQuery;

use orch_api_contract::usage::UsageRow;

use orch_api_contract::usage::UsageSeriesPoint;

use orch_api_contract::usage::UsageSplit;

pub use orch_db::usage::micros::Micros;

pub use orch_db::usage::rollup;

pub use orch_db::usage::rollup::Cell;

pub use orch_db::usage::rollup::Figures;

pub use orch_db::usage::rollup::Read;

use orch_types::Time;
orch_api_base::api_family_callback! {
    bind __api_bind_28d6ff760e, __api_install_28d6ff760e, __api_callback_28d6ff760e;
    slot __ApiCallback28d6ff760e, __API_CALLBACK_28D6FF760E, __api_set_28d6ff760e;
    base orch_api_base::routes::usage::figures;

    fn figures<'__api>(arg0: &'__api orch_db::usage::rollup::Figures) -> orch_api_contract::usage::UsageFigures;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a3e19cca5d, __api_install_a3e19cca5d, __api_callback_a3e19cca5d;
    slot __ApiCallbacka3e19cca5d, __API_CALLBACK_A3E19CCA5D, __api_set_a3e19cca5d;
    base orch_api_base::routes::usage::figures;

    fn credits_of<'__api>(arg0: &'__api orch_db::usage::rollup::Figures) -> orch_db::usage::micros::Micros;
}

orch_api_base::api_family_callback! {
    bind __api_bind_2f2f8642a0, __api_install_2f2f8642a0, __api_callback_2f2f8642a0;
    slot __ApiCallback2f2f8642a0, __API_CALLBACK_2F2F8642A0, __api_set_2f2f8642a0;
    base orch_api_base::routes::usage::figures;

    fn default_bucket(arg0: i64) -> orch_api_contract::usage::UsageBucket;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e9ec524ffe, __api_install_e9ec524ffe, __api_callback_e9ec524ffe;
    slot __ApiCallbacke9ec524ffe, __API_CALLBACK_E9EC524FFE, __api_set_e9ec524ffe;
    base orch_api_base::routes::usage::figures;

    fn default_breakdowns(arg0: orch_api_base::routes::usage::scope::Scope) -> Vec<orch_api_contract::usage::UsageDimension>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_e5b5427b0c, __api_install_e5b5427b0c, __api_callback_e5b5427b0c;
    slot __ApiCallbacke5b5427b0c, __API_CALLBACK_E5B5427B0C, __api_set_e5b5427b0c;
    base orch_api_base::routes::usage::figures;

    fn dimensions<'__api>(arg0: &'__api str) -> orch_api_base::middleware::requests::ApiResult<Vec<orch_api_contract::usage::UsageDimension>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_a33c56a037, __api_install_a33c56a037, __api_callback_a33c56a037;
    slot __ApiCallbacka33c56a037, __API_CALLBACK_A33C56A037, __api_set_a33c56a037;
    base orch_api_base::routes::usage::figures;

    fn bucket_starts(arg0: chrono::NaiveDate, arg1: chrono::NaiveDate, arg2: orch_api_contract::usage::UsageBucket) -> Vec<chrono::DateTime<chrono::Utc>>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_c62d9b5510, __api_install_c62d9b5510, __api_callback_c62d9b5510;
    slot __ApiCallbackc62d9b5510, __API_CALLBACK_C62D9B5510, __api_set_c62d9b5510;
    base orch_api_base::routes::usage::figures;

    fn ranked(arg0: Vec<orch_db::usage::rollup::Cell>, arg1: Option<orch_api_contract::usage::UsageMeasure>) -> Vec<orch_db::usage::rollup::Cell>;
}

/// Adds up figures.
pub fn added(all: &[&Figures]) -> Figures {
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
        unrated_services: sum(|one| one.unrated_services),
        service_units: {
            let mut units = std::collections::BTreeMap::<
                String,
                orch_api_contract::usage::ServiceUnitTotals,
            >::new();
            for row in all {
                let part: std::collections::BTreeMap<
                    String,
                    orch_api_contract::usage::ServiceUnitTotals,
                > = serde_json::from_value(row.service_units.clone()).unwrap_or_default();
                for (unit, value) in part {
                    let held = units.entry(unit).or_default();
                    held.quantity = held.quantity.saturating_add(value.quantity);
                    held.known_requests = held.known_requests.saturating_add(value.known_requests);
                    held.unknown_requests =
                        held.unknown_requests.saturating_add(value.unknown_requests);
                }
            }
            serde_json::to_value(units).unwrap_or_default()
        },
    }
}

/// The composite opening shares the same arithmetic and retains its native disclosure cut.
pub async fn read_in(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: UsageGetQuery,
    reading: Option<&crate::routes::nodes::overview::Reading<'_>>,
) -> ApiResult<UsageAnalytics> {
    let (answer, names, opened) = materialize(ctx, conn, principal, &query, reading).await?;
    super::native::Disclosure {
        scope: scope::parse(&query.scope)?,
        depth: opened.depth,
        names,
        records: Vec::new(),
    }
    .require(conn, principal)
    .await?;
    Ok(answer)
}

/// The same native arithmetic and reader-specific name projection, on the adapter's snapshot.
pub async fn materialize(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    query: &UsageGetQuery,
    reading: Option<&crate::routes::nodes::overview::Reading<'_>>,
) -> ApiResult<(UsageAnalytics, super::names::Cut, scope::Opened)> {
    let mut cut = super::names::Cut::default();
    let asked = scope::parse(&query.scope)?;
    let opened = scope::open(conn, principal, asked).await?;
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
        scope::require_dimension(conn, principal, &opened, *dimension, "this dimension").await?;
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
    let mut filter = scope::filter(conn, principal, &opened, &wanted, false).await?;
    filter.service = query.service.clone();
    filter.purpose = query.purpose.clone();
    filter.effort = query.effort.clone();
    filter.tier = query.tier.clone();
    filter.tier_unknown = query.tier_unknown;
    filter.program = query.program.clone();
    filter.outcome = query.outcome;
    filter.org = query.org_id;
    if query.turn_id.is_some() {
        scope::require_dimension(
            conn,
            principal,
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

    let totals = rollup::read(conn, &read(None, None))
        .await?
        .into_iter()
        .next()
        .map(|cell| cell.figures)
        .unwrap_or_default();
    let points = rollup::read(conn, &read(Some(bucket), None)).await?;
    let own_part = if opened.has_own_part {
        let own = orch_db::usage::rollup::Filter {
            person: Some(opened.caller),
            ..filter.clone()
        };
        let cells = rollup::read(
            conn,
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
                rollup::read(conn, &read(None, Some(dimension))).await?,
                query.measure,
            );
            let per_bucket = rollup::read(conn, &read(Some(bucket), Some(dimension))).await?;
            let keep = usize::try_from(USAGE_SERIES_KEYS_MAX).unwrap_or(usize::MAX);
            let kept: Vec<(Option<String>, bool)> = over_span
                .iter()
                .take(keep)
                .map(|cell| (cell.key.clone(), cell.tier_unknown))
                .collect();
            let values: Vec<Option<String>> =
                over_span.iter().map(|cell| cell.key.clone()).collect();
            let names =
                Names::for_dimension_in(conn, principal, dimension, &values, reading).await?;
            cut.extend(
                names.cut_for_dimension(
                    dimension,
                    &kept
                        .iter()
                        .map(|(value, _)| value.clone())
                        .collect::<Vec<_>>(),
                ),
            );
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
                    .map(|(key, tier_unknown)| UsageSplit {
                        key: names.key(*dimension, key.as_deref(), *tier_unknown),
                        figures: figures(
                            &in_bucket
                                .iter()
                                .find(|cell| &cell.key == key && cell.tier_unknown == *tier_unknown)
                                .map(|cell| cell.figures.clone())
                                .unwrap_or_default(),
                        ),
                    })
                    .collect();
                if *has_rest {
                    let rest: Vec<&Figures> = in_bucket
                        .iter()
                        .filter(|cell| !kept.contains(&(cell.key.clone(), cell.tier_unknown)))
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
            rollup::read(conn, &read(None, Some(dimension))).await?,
            query.measure,
        );
        let values: Vec<Option<String>> = cells.iter().map(|cell| cell.key.clone()).collect();
        let names = Names::for_dimension_in(conn, principal, dimension, &values, reading).await?;
        cut.extend(
            names.cut_for_dimension(
                dimension,
                &cells
                    .iter()
                    .take(limit)
                    .map(|cell| cell.key.clone())
                    .collect::<Vec<_>>(),
            ),
        );
        let more = i64::try_from(cells.len().saturating_sub(limit)).unwrap_or(i64::MAX);
        let rows = cells
            .iter()
            .take(limit)
            .map(|cell| UsageRow {
                key: names.key(dimension, cell.key.as_deref(), cell.tier_unknown),
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
        conn,
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
            Some(provider_health(conn, &opened.base, ctx.app.clock.now()).await?)
        } else {
            None
        },
        performance_before: previous.performance,
        comparison: rollup::comparison(conn, from, to, &filter).await?,
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
    Ok((answer, cut, opened))
}

orch_api_base::api_family_callback! {
    bind __api_bind_fc2b3630df, __api_install_fc2b3630df, __api_callback_fc2b3630df;
    slot __ApiCallbackfc2b3630df, __API_CALLBACK_FC2B3630DF, __api_set_fc2b3630df;
    base orch_api_base::routes::usage::figures;

    async fn health_for<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::usage::UsageDegradation>;
}

orch_api_base::api_family_callback! {
    bind __api_bind_cb3cbe2f30, __api_install_cb3cbe2f30, __api_callback_cb3cbe2f30;
    slot __ApiCallbackcb3cbe2f30, __API_CALLBACK_CB3CBE2F30, __api_set_cb3cbe2f30;
    base orch_api_base::routes::usage::figures;

    async fn provider_health<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_db::usage::rollup::Filter, arg2: chrono::DateTime<chrono::Utc>) -> orch_api_base::middleware::requests::ApiResult<orch_api_contract::usage::UsageDegradation>;
}
