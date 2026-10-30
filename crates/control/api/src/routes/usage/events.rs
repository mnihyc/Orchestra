//! The ledger rows behind a figure, with the same scope and filters, each with its thread and its turn, what it ran on, the rate it was counted at and its credits; a page of them, or all of a page as CSV.
//!
//! A row is shown whole to a reader who may see everybody's records in the
//! scope, and only their own to one who sees a place's figures, so that a figure
//! can be followed row by row as far as the reader's access goes. A thread the
//! reader cannot see is a reference without a name, and nothing of its
//! conversation is shown.

use super::csv::{self, json_response};
use super::names::{Names, rate_of};
use super::scope::{self, Wanted};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::resources::view as resource_view;
use axum::body::Body;
use axum::response::Response;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use http::{HeaderValue, StatusCode};
use orch_api_contract::common::{DEFAULT_LIMIT, MAX_LIMIT, Page};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::threads::TokenUsage;
use orch_api_contract::usage::{
    RateSource, UsageEnvironmentKind, UsageEvent, UsageEventListQuery, UsageEventOrder,
    UsageEventScope, UsageOrigin, UsagePurpose,
};
use orch_db::usage::ledger::{self, EventRow, Listing};
use orch_types::{Id, Time};

/// The name of the header that carries the cursor of the next page of a CSV answer.
pub const NEXT_CURSOR_HEADER: &str = "x-next-cursor";

fn midnight(day: NaiveDate) -> ApiResult<DateTime<Utc>> {
    day.and_hms_opt(0, 0, 0)
        .map(|moment| moment.and_utc())
        .ok_or_else(|| ApiError::invalid("from", "is not a day"))
}

/// What a cursor says: the moment and the identifier of the last row of the page before.
fn parse_cursor(text: &str) -> ApiResult<(DateTime<Utc>, Id)> {
    let bad = || ApiError::invalid("cursor", "is not a cursor this list gave");
    let (micros, id) = text.split_once('.').ok_or_else(bad)?;
    let micros: i64 = micros.parse().map_err(|_| bad())?;
    let at = DateTime::from_timestamp_micros(micros)
        .filter(|at| scope::date_in_range(at.date_naive()))
        .ok_or_else(bad)?;
    Ok((at, id.parse().map_err(|_| bad())?))
}

fn cursor_of(row: &EventRow) -> String {
    format!("{}.{}", row.at.timestamp_micros(), row.id)
}

fn word<T: serde::de::DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// Lists the rows of the ledger behind a figure.
pub async fn list(ctx: Ctx, query: UsageEventListQuery, _body: Body) -> ApiResult<Response> {
    let mut conn = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let asked = scope::parse(&query.scope)?;
    let opened = scope::open(&mut conn, &principal, asked).await?;
    let from = scope::parse_day("from", &query.from)?;
    let to = scope::parse_day("to", &query.to)?;
    if to < from {
        return Err(ApiError::invalid("to", "is not before from"));
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
    let mut filter = scope::filter(&mut conn, &principal, &opened, &wanted, true).await?;
    filter.effort = query.effort.clone();
    filter.tier = query.tier.clone();
    filter.program = query.program.clone();
    filter.outcome = query.outcome;
    filter.org = query.org_id;
    filter.turn = query.turn_id;
    let limit = i64::from(query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT));
    let newest_first = query.order.unwrap_or_default() == UsageEventOrder::Desc;
    let after = query.cursor.as_deref().map(parse_cursor).transpose()?;
    let all_places = scope::all_record_places(&mut conn, &principal, &opened).await?;
    let mut rows = record_page(
        &mut conn,
        &principal,
        all_places,
        &Listing {
            from: Some(midnight(from)?),
            until: Some(midnight(to)? + Duration::days(1)),
            filter: &filter,
            counted: query.counted,
            newest_first,
            after,
            limit: limit + 1,
        },
        query.include_running.unwrap_or(false),
        query.requests_only.unwrap_or(false),
        None,
    )
    .await?;
    let more = rows.len() > usize::try_from(limit).unwrap_or(usize::MAX);
    rows.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
    let next_cursor = more.then(|| rows.last().map(cursor_of)).flatten();

    let items = events(&ctx.app, &mut conn, &principal, &rows).await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    if scope::wants_csv(&ctx.headers) {
        let mut answer = csv::response(events_csv(&items));
        if let Some(next) = &next_cursor
            && let Ok(value) = HeaderValue::from_str(next)
        {
            answer.headers_mut().insert(NEXT_CURSOR_HEADER, value);
        }
        return Ok(answer);
    }
    json_response(&Page { items, next_cursor }, StatusCode::OK)
}

/// Page in bounded candidate batches until the visible page is full. A hidden row never becomes
/// a public cursor; neither an id nor its time is exposed to a provider's ordinary users.
pub(crate) async fn record_page(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    all_places: bool,
    listing: &Listing<'_>,
    include_running: bool,
    requests_only: bool,
    through: Option<i64>,
) -> ApiResult<Vec<EventRow>> {
    let mut candidate = listing.clone();
    let mut visible = Vec::new();
    loop {
        let rows = if let Some(through) = through {
            ledger::live_snapshot(conn, &candidate, include_running, through).await?
        } else {
            ledger::list_live(conn, &candidate, include_running, requests_only).await?
        };
        let full = rows.len() == usize::try_from(candidate.limit).unwrap_or(usize::MAX);
        candidate.after = rows.last().map(|row| (row.at, row.id));
        visible.extend(scope::visible_records(conn, principal, all_places, rows).await?);
        if visible.len() >= usize::try_from(listing.limit).unwrap_or(usize::MAX) || !full {
            break;
        }
    }
    visible.truncate(usize::try_from(listing.limit).unwrap_or(usize::MAX));
    Ok(visible)
}

/// The rows as the contract has them, named for the reader.
pub(crate) async fn events(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    rows: &[EventRow],
) -> ApiResult<Vec<UsageEvent>> {
    let mut users: Vec<Id> = rows.iter().filter_map(|row| row.user_id).collect();
    users.sort_unstable();
    users.dedup();
    let mut providers: Vec<Id> = rows.iter().map(|row| row.provider_id).collect();
    providers.sort_unstable();
    providers.dedup();
    let mut nodes: Vec<Id> = rows
        .iter()
        .flat_map(|row| {
            [row.project_id, row.topic_id]
                .into_iter()
                .chain(row.thread_id)
        })
        .collect();
    nodes.sort_unstable();
    nodes.dedup();
    let names = Names::load(conn, principal, &users, &providers, &nodes).await?;
    let agent_ids: Vec<Id> = rows.iter().filter_map(|row| row.agent_id).collect();
    let agents: std::collections::HashMap<Id, orch_api_contract::resources::AgentRef> =
        resource_view::agent_refs(app, conn, &agent_ids, &[])
            .await?
            .into_iter()
            .map(|(_, each)| (each.id, each))
            .collect();
    let environment_ids: Vec<Id> = rows.iter().filter_map(|row| row.environment_id).collect();
    let environments: std::collections::HashMap<
        Id,
        orch_api_contract::environment::EnvironmentRef,
    > = resource_view::environment_refs(conn, &environment_ids, &[], principal.now())
        .await?
        .into_iter()
        .map(|(_, each)| (each.id, each))
        .collect();
    let pairs: Vec<(Id, String)> = rows
        .iter()
        .map(|row| (row.provider_id, row.model.clone()))
        .collect();
    let displays = orch_db::usage::rollup::displays_of(conn, &pairs).await?;
    let turns: Vec<Id> = rows
        .iter()
        .filter(|row| {
            row.thread_id
                .is_some_and(|id| !names.node(id, NodeKind::Thread).redacted)
        })
        .filter_map(|row| row.turn_id)
        .collect();
    let ordinals = ledger::turn_ordinals(conn, &turns).await?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            Some(UsageEvent {
                timing: row.timing(),
                access_program: row.access_program.clone(),
                running: row.running,
                id: row.id,
                at: Time::new(row.at),
                origin: word::<UsageOrigin>(&row.origin)?,
                purpose: word::<UsagePurpose>(&row.purpose)?,
                scope: word::<UsageEventScope>(&row.scope)?,
                counted: row.counted,
                project: names.node(row.project_id, NodeKind::Project),
                topic: names.node(row.topic_id, NodeKind::Topic),
                thread: row.thread_id.map(|id| names.node(id, NodeKind::Thread)),
                turn_id: row.turn_id.filter(|_| {
                    row.thread_id
                        .is_some_and(|id| !names.node(id, NodeKind::Thread).redacted)
                }),
                turn_ordinal: row
                    .turn_id
                    .filter(|_| {
                        row.thread_id
                            .is_some_and(|id| !names.node(id, NodeKind::Thread).redacted)
                    })
                    .and_then(|id| ordinals.get(&id).copied()),
                user: row.user_id.map(|id| names.user(id)),
                provider: names.provider(row.provider_id)?,
                agent: row.agent_id.and_then(|id| agents.get(&id).cloned()),
                harness: row
                    .harness_kind
                    .as_deref()
                    .map(|kind| crate::programs::shipped_name(kind, None)),
                harness_kind: row.harness_kind.clone(),
                environment: row
                    .environment_id
                    .and_then(|id| environments.get(&id).cloned()),
                environment_kind: row
                    .environment_kind
                    .as_deref()
                    .and_then(word::<UsageEnvironmentKind>),
                arrangement: row.arrangement.as_deref().and_then(word),
                model: row.model.clone(),
                model_display: displays
                    .get(&(row.provider_id, row.model.clone()))
                    .cloned()
                    .unwrap_or_default(),
                effort: row.effort.clone(),
                tier: row.tier.clone(),
                tokens: TokenUsage {
                    input_tokens: row.input_tokens,
                    cached_input_tokens: row.cached_input_tokens,
                    cache_write_tokens: row.cache_write_tokens,
                    output_tokens: row.output_tokens,
                    reasoning_tokens: row.reasoning_tokens,
                    total_tokens: row.total_tokens,
                },
                request_ref: row.request_ref.clone(),
                context_tokens: row.context_tokens,
                rate: row.rate.as_ref().and_then(rate_of),
                rate_source: row.rate_source.as_deref().and_then(word::<RateSource>),
                credits: row.credits.clone(),
            })
        })
        .collect())
}

/// The rows as CSV: a line for each, with what a spreadsheet wants to filter by.
fn events_csv(events: &[UsageEvent]) -> String {
    let mut text = csv::line(&[
        "id",
        "at",
        "origin",
        "purpose",
        "scope",
        "counted",
        "project",
        "topic",
        "thread",
        "turn_id",
        "turn_ordinal",
        "user",
        "provider",
        "agent",
        "harness_kind",
        "environment_kind",
        "arrangement",
        "model",
        "model_display",
        "effort",
        "tier",
        "input_tokens",
        "cached_input_tokens",
        "output_tokens",
        "reasoning_tokens",
        "total_tokens",
        "context_tokens",
        "request_ref",
        "rate_source",
        "credits",
        "access_program",
        "first_output_ms",
        "generation_ms",
        "request_ms",
        "speed_tokens_per_second",
        "outcome",
        "outcome_message",
        "vendor_words",
        "reasoning_hidden",
        "running",
    ]);
    for each in events {
        let name = |node: &orch_api_contract::nodes::NodeRef| node.name.clone().unwrap_or_default();
        let reported = each.timing.reported_tokens.as_ref();
        let count = |value: Option<i64>, legacy: i64| {
            if each.scope == orch_api_contract::usage::UsageEventScope::Request
                && !each.timing.is_unsupported()
            {
                value.map(|n| n.to_string()).unwrap_or_default()
            } else {
                legacy.to_string()
            }
        };
        text.push_str(&csv::line(&[
            each.id.to_string(),
            each.at.get().format("%Y-%m-%dT%H:%M:%S%.3fZ").to_string(),
            serde_word(&each.origin),
            serde_word(&each.purpose),
            serde_word(&each.scope),
            each.counted.to_string(),
            name(&each.project),
            name(&each.topic),
            each.thread.as_ref().map(name).unwrap_or_default(),
            each.turn_id.map(|id| id.to_string()).unwrap_or_default(),
            each.turn_ordinal
                .map(|ordinal| ordinal.to_string())
                .unwrap_or_default(),
            each.user
                .as_ref()
                .map(|user| user.username.clone())
                .unwrap_or_default(),
            each.provider.name.clone(),
            each.agent
                .as_ref()
                .map(|agent| agent.name.clone())
                .unwrap_or_default(),
            each.harness_kind.clone().unwrap_or_default(),
            each.environment_kind
                .as_ref()
                .map(serde_word)
                .unwrap_or_default(),
            each.arrangement
                .as_ref()
                .map(serde_word)
                .unwrap_or_default(),
            each.model.clone(),
            each.model_display.clone(),
            each.effort.clone().unwrap_or_default(),
            each.tier.clone().unwrap_or_default(),
            count(
                reported.and_then(|tokens| tokens.input_tokens),
                each.tokens.input_tokens,
            ),
            count(
                reported.and_then(|tokens| tokens.cached_input_tokens),
                each.tokens.cached_input_tokens,
            ),
            count(
                reported.and_then(|tokens| tokens.output_tokens),
                each.tokens.output_tokens,
            ),
            count(
                reported.and_then(|tokens| tokens.reasoning_tokens),
                each.tokens.reasoning_tokens,
            ),
            count(
                reported.and_then(|tokens| tokens.total_tokens),
                each.tokens.total_tokens,
            ),
            each.context_tokens
                .map(|n| n.to_string())
                .unwrap_or_default(),
            each.request_ref.clone().unwrap_or_default(),
            each.rate_source
                .as_ref()
                .map(serde_word)
                .unwrap_or_default(),
            // A row without a rate has no credits, which a spreadsheet reads as an empty cell, never 0.
            each.credits.clone().unwrap_or_default(),
            each.access_program.clone().unwrap_or_default(),
            each.timing
                .first_token_ms
                .value()
                .map(ToString::to_string)
                .unwrap_or_default(),
            each.timing
                .generation_ms
                .value()
                .map(ToString::to_string)
                .unwrap_or_default(),
            each.timing
                .request_ms
                .value()
                .map(ToString::to_string)
                .unwrap_or_default(),
            each.timing
                .speed_milli
                .value()
                .map(|n| (*n as f64 / 1000.0).to_string())
                .unwrap_or_default(),
            each.timing.outcome.value().cloned().unwrap_or_default(),
            each.timing.error_message.clone().unwrap_or_default(),
            each.timing.vendor_words.clone().unwrap_or_default(),
            each.timing.reasoning_hidden.to_string(),
            each.running.to_string(),
        ]));
    }
    text
}

fn serde_word<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_owned))
        .unwrap_or_default()
}
