//! Exact reported amounts, sampled at their source time without ledger arithmetic or float rollups.

use crate::{Conn, DbError};
use chrono::{DateTime, Timelike, Utc};
use orch_types::{Id, Json};
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

/// The existing provider-history point limit also bounds exact amount series.
pub const MAX_POINTS: i64 = super::provider_plan::MAX_PLAN_POINTS;

#[derive(sqlx::FromRow)]
struct Known {
    member: String,
    item_id: String,
    quantity: String,
    label: Option<String>,
    unit: Option<String>,
    decimal_places: Option<i64>,
}

fn items<'a>(facts: &'a Json, member: &str) -> BTreeMap<String, &'a Json> {
    if member == "extra_usage" {
        return facts
            .get(member)
            .filter(|value| value.is_object())
            .map(|value| BTreeMap::from([(String::new(), value)]))
            .unwrap_or_default();
    }
    facts
        .get(member)
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .take(if member == "balances" {
            orch_provider_contract::limits::BALANCES_MAX
        } else {
            orch_provider_contract::limits::SPEND_MAX
        })
        .filter_map(|value| {
            let id = value.get("id")?.as_str()?;
            (id.chars().count() <= orch_provider_contract::limits::TEXT_MAX_CHARS)
                .then(|| (id.to_owned(), value))
        })
        .collect()
}

fn at(facts: &Json, key: &str) -> Option<DateTime<Utc>> {
    facts.get("field_times")?.get(key)?.as_str()?.parse().ok()
}

fn prefix(member: &str, item: &str) -> String {
    if item.is_empty() {
        member.to_owned()
    } else {
        format!("{member}.{item}")
    }
}

/// An explicit finer observation wins over its item or member. The ordinary merge advances
/// previously known omitted finer clocks for accepted whole replacements, including nulls.
/// An item's own clock still wins over an unrelated collection refresh.
fn field_at(facts: &Json, member: &str, item: &str, field: &str) -> Option<DateTime<Utc>> {
    let item = prefix(member, item);
    at(
        facts,
        &orch_provider_contract::credit_fields::field_time_key(&item, field),
    )
    .or_else(|| at(facts, &item))
    .or_else(|| at(facts, member))
}

fn amount(value: Option<&Json>) -> Option<String> {
    value
        .and_then(Json::as_str)
        .filter(|text| orch_provider_contract::limits::is_decimal(text))
        .map(str::to_owned)
}

fn flag(value: Option<&Json>, field: &str) -> Option<bool> {
    value
        .and_then(|value| value.get(field))
        .and_then(Json::as_bool)
}

fn text(value: Option<&Json>, field: &str) -> Option<String> {
    value
        .and_then(|value| value.get(field))
        .and_then(Json::as_str)
        .filter(|text| text.chars().count() <= orch_provider_contract::limits::TEXT_MAX_CHARS)
        .map(str::to_owned)
}

fn scale(value: Option<&Json>) -> Option<i64> {
    value
        .and_then(|value| value.get("decimal_places"))
        .and_then(Json::as_u64)
        .filter(|value| *value <= u64::from(u32::MAX))
        .and_then(|value| i64::try_from(value).ok())
}

/// Records only newly accepted quantity times in the caller's locked facts transaction.
/// Raw facts are used: manual overrides and a refreshed read clock never create vendor samples.
pub async fn record(
    conn: &mut Conn,
    provider: Id,
    source_revision: i64,
    before: &Json,
    after: &Json,
) -> Result<u64, DbError> {
    let has_credit_time = after
        .get("field_times")
        .and_then(Json::as_object)
        .is_some_and(|times| {
            times.keys().any(|key| {
                orch_provider_contract::credit_fields::is_credit_time_key(key)
                    && at(after, key).is_some_and(|new| at(before, key).is_none_or(|old| new > old))
            })
        });
    if !has_credit_time {
        return Ok(0);
    }
    // One metadata read supplies existing series even after a previous observation cleared them.
    // Their amount is deliberately not selected, so no cached value can become a new sample.
    let candidates: Vec<_> = ["balances", "spend", "extra_usage"]
        .into_iter()
        .flat_map(|member| {
            let ids: BTreeSet<_> = items(before, member)
                .into_keys()
                .chain(items(after, member).into_keys())
                .collect();
            ids.into_iter()
                .map(move |item_id| json!({"member":member,"item_id":item_id}))
        })
        .collect();
    if candidates.is_empty() {
        return Ok(0);
    }
    let known: Vec<Known> = sqlx::query_as(
        "SELECT DISTINCT ON (member,item_id,quantity) member,item_id,quantity,label,unit,decimal_places
           FROM provider_credit_observations WHERE provider_id=$1 AND source_revision=$2
            AND (member,item_id) IN (SELECT r.member,r.item_id FROM jsonb_to_recordset($3::jsonb) AS r(member text,item_id text))
          ORDER BY member,item_id,quantity,observed_at DESC",
    )
    .bind(provider)
    .bind(source_revision)
    .bind(Json::Array(candidates))
    .fetch_all(&mut *conn)
    .await?;
    let known: BTreeMap<_, _> = known
        .into_iter()
        .map(|row| {
            (
                (
                    row.member.clone(),
                    row.item_id.clone(),
                    row.quantity.clone(),
                ),
                row,
            )
        })
        .collect();
    let mut observations = Vec::new();
    for (member, quantities) in [
        ("balances", &["remaining", "total", "used"][..]),
        ("spend", &["used", "limit", "remaining"][..]),
        ("extra_usage", &["used", "limit"][..]),
    ] {
        let previous = items(before, member);
        let current = items(after, member);
        let ids: BTreeSet<_> = previous
            .keys()
            .chain(current.keys())
            .chain(
                known
                    .keys()
                    .filter(|(found, _, _)| found == member)
                    .map(|(_, item, _)| item),
            )
            .cloned()
            .collect();
        for item_id in ids {
            let old = previous.get(&item_id).copied();
            let value = current.get(&item_id).copied();
            let cleared = value.is_none();
            for quantity in quantities {
                let retained =
                    known.get(&(member.to_owned(), item_id.clone(), (*quantity).to_owned()));
                let observed = if cleared {
                    at(after, member)
                } else {
                    field_at(after, member, &item_id, quantity)
                };
                let previous_at = field_at(before, member, &item_id, quantity);
                let fresh_amount =
                    observed.filter(|time| previous_at.is_none_or(|old| *time > old));
                let mut observation_at = fresh_amount;
                let mut reported = if fresh_amount.is_some() {
                    amount(value.and_then(|value| value.get(*quantity)))
                } else {
                    None
                };
                let unlimited = flag(value, "unlimited");
                let enabled = flag(value, "enabled");
                // A sparse source event can update enabled alone. Retain the actual transition
                // as a gap on known quantities, not an observation of cached paid amounts.
                for field in ["enabled", "unlimited"] {
                    if flag(old, field) == flag(value, field) || flag(value, field).is_none() {
                        continue;
                    }
                    let changed = field_at(after, member, &item_id, field).filter(|time| {
                        field_at(before, member, &item_id, field).is_none_or(|old| *time > old)
                    });
                    if let Some(changed) = changed
                        && observation_at.is_none_or(|time| changed > time)
                    {
                        observation_at = Some(changed);
                        reported = None;
                    }
                }
                let Some(observed_at) = observation_at else {
                    continue;
                };
                if reported.is_none()
                    && retained.is_none()
                    && !(member == "balances"
                        && *quantity == "remaining"
                        && unlimited == Some(true))
                {
                    // A never-reported null is current unknown, not invented history. This also
                    // omits Codex's deliberately unsupported total/used normalized null fields.
                    continue;
                }
                let sample_at = observed_at
                    .with_second(0)
                    .and_then(|time| time.with_nanosecond(0))
                    .unwrap_or(observed_at);
                let (label, unit, decimal_places) = if cleared {
                    (
                        text(old, "label").or_else(|| retained.and_then(|row| row.label.clone())),
                        text(old, "unit").or_else(|| retained.and_then(|row| row.unit.clone())),
                        scale(old).or_else(|| retained.and_then(|row| row.decimal_places)),
                    )
                } else {
                    (text(value, "label"), text(value, "unit"), scale(value))
                };
                let had_gap = cleared
                    || reported.is_none()
                    || unlimited == Some(true)
                    || enabled == Some(false);
                observations.push(json!({
                    "member":member,"item_id":item_id,"quantity":quantity,
                    "sample_at":sample_at,"observed_at":observed_at,"label":label,
                    "unit":unit,"decimal_places":decimal_places,"amount":reported,
                    "unlimited":unlimited,"enabled":enabled,"cleared":cleared,
                    "had_gap":had_gap,
                }));
            }
        }
    }
    if observations.is_empty() {
        return Ok(0);
    }
    Ok(sqlx::query(
        "INSERT INTO provider_credit_observations
         (provider_id,source_revision,member,item_id,quantity,sample_at,observed_at,label,unit,decimal_places,amount,unlimited,enabled,cleared,had_gap)
         SELECT $1,$2,r.member,r.item_id,r.quantity,r.sample_at,r.observed_at,r.label,r.unit,r.decimal_places,r.amount,r.unlimited,r.enabled,r.cleared,r.had_gap
         FROM jsonb_to_recordset($3::jsonb) AS r(member text,item_id text,quantity text,sample_at timestamptz,
              observed_at timestamptz,label text,unit text,decimal_places bigint,amount text,unlimited boolean,enabled boolean,cleared boolean,had_gap boolean)
         ON CONFLICT(provider_id,source_revision,member,item_id,quantity,sample_at) DO UPDATE SET
            observed_at=EXCLUDED.observed_at,label=EXCLUDED.label,unit=EXCLUDED.unit,decimal_places=EXCLUDED.decimal_places,
            amount=EXCLUDED.amount,unlimited=EXCLUDED.unlimited,enabled=EXCLUDED.enabled,cleared=EXCLUDED.cleared,
            had_gap=provider_credit_observations.had_gap OR EXCLUDED.had_gap
                OR provider_credit_observations.unit IS DISTINCT FROM EXCLUDED.unit
                OR provider_credit_observations.decimal_places IS DISTINCT FROM EXCLUDED.decimal_places
         WHERE EXCLUDED.observed_at > provider_credit_observations.observed_at",
    ).bind(provider).bind(source_revision).bind(Json::Array(observations)).execute(conn).await?.rows_affected())
}

/// One current source-revision cut for history cursors. No configuration or credential identity leaves storage.
pub async fn source_revision(conn: &mut Conn, provider: Id) -> Result<Option<i64>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT credit_source_revision FROM providers WHERE id=$1 AND deleted_at IS NULL",
    )
    .bind(provider)
    .fetch_optional(conn)
    .await?)
}

/// The same span and group-page bound as the provider history request.
pub struct Read<'a> {
    /// Provider already admitted by the ordinary provider reader.
    pub provider: Id,
    /// Inclusive source-time start.
    pub from: DateTime<Utc>,
    /// Exclusive source-time end.
    pub to: DateTime<Utc>,
    /// Frozen observation cut for the page.
    pub as_of: DateTime<Utc>,
    /// Existing hour/day/week/month chart step.
    pub bucket: &'a str,
    /// Previous complete group key, never a quantity from inside it.
    pub after: Option<&'a str>,
    /// Groups requested; one additional group is returned to establish continuation.
    pub limit: i64,
}

/// Exact points already bounded in SQL. Amount strings have never passed through a float.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SeriesRow {
    /// Internal ordered identity; HTTP emits an opaque hash of this safe typed identity.
    pub key: String,
    /// Internal grouping identity without quantity, so used and limit stay on one page.
    pub group_key: String,
    /// Server-local source generation.
    pub source_revision: i64,
    /// Existing fact member.
    pub member: String,
    /// Existing fact item identifier.
    pub item_id: String,
    /// One explicitly reported quantity.
    pub quantity: String,
    /// Last observed caption.
    pub label: Option<String>,
    /// Exact vendor unit, where told.
    pub unit: Option<String>,
    /// Exact declared decimal scale.
    pub decimal_places: Option<i64>,
    /// At most MAX_POINTS actual timestamps, decimal strings and gap flags.
    pub points: Json,
    /// The requested bucket has more points than this existing provider-history limit.
    pub overflow: bool,
}

/// Reads all quantities of a bounded group page, with last actual observations in each chart step.
pub async fn read(conn: &mut Conn, read: &Read<'_>) -> Result<Vec<SeriesRow>, DbError> {
    Ok(sqlx::query_as(include_str!("provider_credits.sql"))
        .bind(read.provider)
        .bind(read.from)
        .bind(read.to)
        .bind(read.as_of)
        .bind(read.bucket)
        .bind(read.after)
        .bind(read.limit.saturating_add(1))
        .bind(MAX_POINTS)
        .fetch_all(conn)
        .await?)
}

/// Uses the existing retained-metrics duration. Its zero setting does not call this trim.
pub async fn trim(conn: &mut Conn, before: DateTime<Utc>) -> Result<u64, DbError> {
    Ok(
        sqlx::query("DELETE FROM provider_credit_observations WHERE sample_at < $1")
            .bind(before)
            .execute(conn)
            .await?
            .rows_affected(),
    )
}
