//! Counted consumption in tokens and credits, tokens without a rate, the rollups by day and by month for every dimension, and reproducible rebuilds.
//!
//! **One rule, written once.** The measures of a daily row are one `SELECT` over the ledger rows it stands for
//! ([`daily_select`]). [`rebuild`] runs it over all of them; [`fold_event`] runs it over the one row that arrived, in
//! the transaction that wrote it, and adds what it gives to the row's day and month. A ledger row is written once
//! and never changed, and only a row that was just written is folded, so each row is added exactly once and the
//! live tables are what a rebuild makes of the same ledger (`tests/usage_rollup.rs` compares them). Making the
//! row again from the ledger at each arrival would read every earlier row of its day with each one: the square of
//! a busy thread's requests in a day. The monthly rows are the sums of the daily rows of their month, which makes
//! a figure read from the months equal the one read from the days.
//!
//! The service tier a row ran with is part of the key of both rollups, so that a cap can cover one tier
//! alone and the figures by tier are read from the days and the months, and a rebuild writes it as the
//! live fold does.
//!
//! What a row of the ledger adds: its tokens by kind; its credits where it
//! had a rate, and the part of them the shipped catalogue estimated; its
//! tokens in `unrated_tokens` where it had none, which is not the same as a
//! row that cost nothing. A turn counts once for each daily row it has rows
//! under, however many requests it made. Only rows that count are folded;
//! the others are kept in the ledger as trace.
//! Public turn figures separately count distinct historical turn ids within
//! the same span/filter/group, including unmetered turns. They cannot be found
//! by summing the stored meter-key turn counts across days or model choices.
//!
//! **Reading.** [`read`] adds the rollups (or, for a span read by the hour
//! and for the dimensions the rollups do not keep, the ledger itself) over a
//! span, optionally cut into buckets and split by a dimension, for the place,
//! the provider or the person a [`Filter`] names.

use crate::{Conn, DbError};

/// A person's daily work in one thread and topic, kept separate until the reader's access is known.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProfileDay {
    /// The UTC day of the rollup.
    pub day: chrono::NaiveDate,
    /// The thread whose View permission gates this work.
    pub thread_id: orch_types::Id,
    /// The topic recorded with this work.
    pub topic_id: orch_types::Id,
    /// Turns counted by the daily rollup.
    pub turns: i64,
    /// The latest counted ledger activity in this thread on this day.
    pub last_at: chrono::DateTime<chrono::Utc>,
}

/// Reads daily work without summing across threads, so permission cuts precede all public totals.
pub async fn profile_days(
    conn: &mut Conn,
    user: orch_types::Id,
    from: chrono::NaiveDate,
    to: chrono::NaiveDate,
) -> Result<Vec<ProfileDay>, DbError> {
    Ok(sqlx::query_as(
        "WITH recent AS (
             SELECT (at AT TIME ZONE 'UTC')::date AS day, thread_id, max(at) AS last_at
             FROM usage_events WHERE user_id=$1 AND counted AND purpose='turn'
               AND at >= $2::date::timestamp AT TIME ZONE 'UTC'
               AND at < ($3::date + 1)::timestamp AT TIME ZONE 'UTC'
             GROUP BY (at AT TIME ZONE 'UTC')::date, thread_id
         )
         SELECT d.day,d.thread_id,d.topic_id,sum(d.turns)::bigint AS turns,
                COALESCE(max(r.last_at),d.day::timestamp AT TIME ZONE 'UTC') AS last_at
         FROM usage_daily d LEFT JOIN recent r ON r.day=d.day AND r.thread_id=d.thread_id
         WHERE d.user_id=$1 AND d.day >= $2 AND d.day <= $3 AND d.purpose='turn'
           AND d.thread_id IS NOT NULL
         GROUP BY d.day,d.thread_id,d.topic_id HAVING sum(d.turns)>0
         ORDER BY d.day,d.thread_id,d.topic_id",
    )
    .bind(user)
    .bind(from)
    .bind(to)
    .fetch_all(conn)
    .await?)
}
use chrono::{DateTime, Datelike, Duration, NaiveDate, Utc};
use orch_api_contract::usage::{UsageBucket, UsageDimension};
use orch_types::Id;
use sqlx::{FromRow, Postgres, QueryBuilder};

/// A historical attribution dimension supported by the shared resource-facts reader.
#[derive(Debug, Clone, Copy)]
pub enum ResourceTurnDimension {
    /// Consumption attributed to a provider, including unmetered turns.
    Provider,
    /// Actual turns on the profile fixed in their segment.
    Agent,
    /// Actual turns in the environment fixed in their segment.
    Environment,
}

/// Distinct turns and people for one resource, from the same immutable source as Usage.
#[derive(Debug, Clone, FromRow)]
pub struct ResourceTurnCounts {
    /// Its stable identity.
    pub id: Id,
    /// Distinct recorded turns over its lifetime.
    pub all_time: i64,
    /// Distinct threads represented by those same real turns.
    pub threads: Option<i64>,
    /// Distinct turns in the thirty UTC daily buckets ending today.
    pub last_30_days: i64,
    /// Distinct captured user identities, without substituting today's owner.
    pub people: Option<i64>,
    /// Distinct captured user identities in those thirty daily buckets.
    pub people_30_days: Option<i64>,
    /// The exact latest source fact.
    pub last_at: Option<DateTime<Utc>>,
}

/// Reads a whole page in one indexed grouped query. Meter rows, retries, copied history and
/// per-model/day rollup memberships never become additional turns. The ledger survives deletion;
/// current started turns supplement profile/environment facts before their usage arrives.
pub async fn resource_turn_counts(
    conn: &mut Conn,
    ids: &[Id],
    dimension: ResourceTurnDimension,
    at: DateTime<Utc>,
) -> Result<Vec<ResourceTurnCounts>, DbError> {
    let column = match dimension {
        ResourceTurnDimension::Provider => "provider_id",
        ResourceTurnDimension::Agent => "agent_id",
        ResourceTurnDimension::Environment => "environment_id",
    };
    let live = if matches!(dimension, ResourceTurnDimension::Provider) {
        String::new()
    } else {
        format!(
            " UNION ALL SELECT s.{column},t.id,t.audit_for_user_id,t.started_at,t.thread_id
              FROM segments s JOIN turns t ON t.segment_id=s.id
              WHERE s.{column}=ANY($1) AND t.started_at IS NOT NULL AND t.started_at<=$2
                AND (t.native_turn_ref IS NULL OR t.native_turn_ref NOT LIKE 'fork-copy:%')"
        )
    };
    // Like turn_counts below, every historical turn id counts, regardless of the meter's counted
    // flag. Standalone model work has no turn id and is not a turn of a resource.
    let sql = format!(
        "WITH facts AS (
          SELECT {column} AS id,turn_id,user_id,at,thread_id FROM usage_events
          WHERE {column}=ANY($1) AND turn_id IS NOT NULL AND at<=$2 {live}
        ) SELECT id,count(DISTINCT turn_id)::bigint AS all_time,
          CASE WHEN bool_or(thread_id IS NULL) THEN NULL ELSE count(DISTINCT thread_id)::bigint END AS threads,
          count(DISTINCT turn_id) FILTER(WHERE at>=$3)::bigint AS last_30_days,
          CASE WHEN bool_or(user_id IS NULL) THEN NULL ELSE count(DISTINCT user_id)::bigint END AS people,
          CASE WHEN bool_or(user_id IS NULL AND at>=$3) THEN NULL ELSE count(DISTINCT user_id) FILTER(WHERE at>=$3)::bigint END AS people_30_days,
          max(at) AS last_at FROM facts GROUP BY id"
    );
    let from = day_start(at.date_naive() - Duration::days(29))?;
    // Every SQL identifier above comes from the closed dimension enum; values remain bound.
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(ids)
        .bind(at)
        .bind(from)
        .fetch_all(conn)
        .await?)
}

// ---------------------------------------------------------------- the fold

/// The day of a ledger row, in UTC.
const DAY: &str = "(e.at AT TIME ZONE 'UTC')::date";

/// This is a fixed contract vocabulary, never input from a query or adapter.
fn provider_error_sql(column: &str) -> String {
    let codes = orch_api_contract::usage::PROVIDER_ERROR_CODES
        .iter()
        .map(|code| format!("'{code}'"))
        .collect::<Vec<_>>()
        .join(",");
    format!("{column} IN ({codes})")
}

fn ledger_measures() -> String {
    LEDGER_MEASURES.replace("PROVIDER_ERROR", &provider_error_sql("d.outcome"))
}

/// The columns of a daily row, in the order [`daily_select`] gives them.
const DAILY_COLUMNS: &str = "day, org_id, project_id, topic_id, thread_id, purpose, provider_id, entry_id,
                user_id, model, tier, harness_kind, environment_kind, turns, input_tokens, cached_input_tokens, cache_write_tokens,
                output_tokens, reasoning_tokens, total_tokens, credits, estimated_credits, unrated_tokens,
                effort, access_program, requests, errors, speed_tokens, generation_ms_sum, speed_buckets, first_token_buckets, hidden_requests, short_requests,
                rated_rows";

/// The key of a daily row.
const DAILY_KEY: &str =
    "day, topic_id, thread_id, purpose, provider_id, entry_id, model, tier, user_id,
                      harness_kind, environment_kind, effort, access_program";

/// The measures of the daily rows of the ledger rows (`e`) that `restriction` leaves of the counted ones, grouped
/// by the key of a daily row: the one rule both a rebuild and a fold apply.
fn daily_select(restriction: &str) -> String {
    let provider_error = provider_error_sql("e.outcome");
    format!(
        "SELECT {DAY} AS day, (array_agg(e.org_id ORDER BY e.at, e.id))[1] AS org_id,
                (array_agg(e.project_id ORDER BY e.at, e.id))[1] AS project_id,
                e.topic_id, e.thread_id, e.purpose, e.provider_id, e.entry_id, e.user_id, e.model, e.tier,
                e.harness_kind, e.environment_kind, count(DISTINCT e.turn_id) FILTER (WHERE e.counted)::int AS turns,
                COALESCE(sum(e.input_tokens) FILTER (WHERE e.counted), 0)::bigint AS input_tokens,
                COALESCE(sum(e.cached_input_tokens) FILTER (WHERE e.counted), 0)::bigint AS cached_input_tokens,
                COALESCE(sum(e.cache_write_tokens) FILTER (WHERE e.counted), 0)::bigint AS cache_write_tokens,
                COALESCE(sum(e.output_tokens) FILTER (WHERE e.counted), 0)::bigint AS output_tokens,
                COALESCE(sum(e.reasoning_tokens) FILTER (WHERE e.counted), 0)::bigint AS reasoning_tokens,
                COALESCE(sum(e.total_tokens) FILTER (WHERE e.counted), 0)::bigint AS total_tokens,
                COALESCE(sum(e.credits) FILTER (WHERE e.counted), 0) AS credits,
                COALESCE(sum(e.credits) FILTER (WHERE e.counted AND e.rate_source = 'catalogue'), 0) AS estimated_credits,
                COALESCE(sum(e.total_tokens) FILTER (WHERE e.counted AND e.credits IS NULL), 0)::bigint AS unrated_tokens,
                e.effort, e.access_program, count(e.outcome)::bigint AS requests,
                usage_counts(CASE WHEN {provider_error} THEN e.outcome END) AS errors,
                COALESCE(sum(e.streamed_output_tokens) FILTER (WHERE e.speed_milli IS NOT NULL),0)::bigint AS speed_tokens,
                COALESCE(sum(e.generation_ms) FILTER (WHERE e.speed_milli IS NOT NULL),0)::bigint AS generation_ms_sum,
                usage_histogram(e.speed_milli::double precision/1000) AS speed_buckets,
                usage_histogram(e.first_token_ms::double precision) AS first_token_buckets,
                count(*) FILTER (WHERE e.outcome IS NOT NULL AND e.measurement_states->>'reasoning_hidden'='true') AS hidden_requests,
                count(*) FILTER (WHERE e.outcome IS NOT NULL AND e.measurement_states->'speed_milli'->>'state'='empty') AS short_requests,
                count(*) FILTER (WHERE e.counted AND e.credits IS NOT NULL) AS rated_rows
           FROM usage_events e
          WHERE (e.counted OR e.outcome IS NOT NULL) AND {restriction}
          GROUP BY {DAY}, e.topic_id, e.thread_id, e.purpose, e.provider_id, e.entry_id, e.user_id, e.model,
                   e.tier, e.harness_kind, e.environment_kind, e.effort, e.access_program"
    )
}

/// The statement that makes daily rows from the ledger rows that `restriction`
/// leaves of the counted ones, grouped by the key of a daily row.
fn daily_statement(restriction: &str) -> String {
    format!(
        "INSERT INTO usage_daily ({DAILY_COLUMNS})
         {select}
         ON CONFLICT ({DAILY_KEY})
         DO UPDATE SET org_id = EXCLUDED.org_id, project_id = EXCLUDED.project_id, turns = EXCLUDED.turns,
                input_tokens = EXCLUDED.input_tokens, cached_input_tokens = EXCLUDED.cached_input_tokens, cache_write_tokens = EXCLUDED.cache_write_tokens,
                output_tokens = EXCLUDED.output_tokens, reasoning_tokens = EXCLUDED.reasoning_tokens,
                total_tokens = EXCLUDED.total_tokens, credits = EXCLUDED.credits,
                estimated_credits = EXCLUDED.estimated_credits, unrated_tokens = EXCLUDED.unrated_tokens,
                requests=EXCLUDED.requests, errors=EXCLUDED.errors, speed_tokens=EXCLUDED.speed_tokens,
                generation_ms_sum=EXCLUDED.generation_ms_sum, speed_buckets=EXCLUDED.speed_buckets,
                first_token_buckets=EXCLUDED.first_token_buckets,hidden_requests=EXCLUDED.hidden_requests,short_requests=EXCLUDED.short_requests,
                rated_rows=EXCLUDED.rated_rows",
        select = daily_select(restriction)
    )
}

/// The ledger rows under the same daily row as `EXCLUDED` (a row being added), for the rare repair of its place.
const SAME_DAY_ROW: &str = "x.at >= EXCLUDED.day::timestamp AT TIME ZONE 'UTC' AND x.at < (EXCLUDED.day + 1)::timestamp AT TIME ZONE 'UTC'
         AND x.topic_id = EXCLUDED.topic_id AND x.thread_id IS NOT DISTINCT FROM EXCLUDED.thread_id
         AND x.purpose = EXCLUDED.purpose AND x.provider_id = EXCLUDED.provider_id
         AND x.entry_id IS NOT DISTINCT FROM EXCLUDED.entry_id AND x.user_id IS NOT DISTINCT FROM EXCLUDED.user_id
         AND x.model = EXCLUDED.model AND x.tier IS NOT DISTINCT FROM EXCLUDED.tier
         AND x.harness_kind IS NOT DISTINCT FROM EXCLUDED.harness_kind
         AND x.environment_kind IS NOT DISTINCT FROM EXCLUDED.environment_kind
         AND x.effort IS NOT DISTINCT FROM EXCLUDED.effort AND x.access_program IS NOT DISTINCT FROM EXCLUDED.access_program
         AND (x.counted OR x.outcome IS NOT NULL)";

/// The statement that adds the ledger row `$1` to its daily row; `$2` says whether it is the first counted row of
/// its turn under that row. Sums and counts add; error and speed counts merge; the place is the earliest row's,
/// which only a row that names another place can change, and only then is it read again from the ledger.
fn daily_addition() -> String {
    format!(
        "INSERT INTO usage_daily ({DAILY_COLUMNS})
         {select}
         ON CONFLICT ({DAILY_KEY})
         DO UPDATE SET
                org_id = CASE WHEN EXCLUDED.org_id = usage_daily.org_id
                                   AND EXCLUDED.project_id IS NOT DISTINCT FROM usage_daily.project_id
                              THEN usage_daily.org_id
                              ELSE (SELECT (array_agg(x.org_id ORDER BY x.at, x.id))[1] FROM usage_events x WHERE {SAME_DAY_ROW}) END,
                project_id = CASE WHEN EXCLUDED.org_id = usage_daily.org_id
                                       AND EXCLUDED.project_id IS NOT DISTINCT FROM usage_daily.project_id
                                  THEN usage_daily.project_id
                                  ELSE (SELECT (array_agg(x.project_id ORDER BY x.at, x.id))[1] FROM usage_events x WHERE {SAME_DAY_ROW}) END,
                turns = usage_daily.turns + CASE WHEN $2 THEN EXCLUDED.turns ELSE 0 END,
                input_tokens = usage_daily.input_tokens + EXCLUDED.input_tokens,
                cached_input_tokens = usage_daily.cached_input_tokens + EXCLUDED.cached_input_tokens,
                cache_write_tokens = usage_daily.cache_write_tokens + EXCLUDED.cache_write_tokens,
                output_tokens = usage_daily.output_tokens + EXCLUDED.output_tokens,
                reasoning_tokens = usage_daily.reasoning_tokens + EXCLUDED.reasoning_tokens,
                total_tokens = usage_daily.total_tokens + EXCLUDED.total_tokens,
                credits = usage_daily.credits + EXCLUDED.credits,
                estimated_credits = usage_daily.estimated_credits + EXCLUDED.estimated_credits,
                unrated_tokens = usage_daily.unrated_tokens + EXCLUDED.unrated_tokens,
                requests = usage_daily.requests + EXCLUDED.requests,
                errors = usage_counts_merge(usage_daily.errors, EXCLUDED.errors),
                speed_tokens = usage_daily.speed_tokens + EXCLUDED.speed_tokens,
                generation_ms_sum = usage_daily.generation_ms_sum + EXCLUDED.generation_ms_sum,
                speed_buckets = usage_counts_merge(usage_daily.speed_buckets, EXCLUDED.speed_buckets),
                first_token_buckets = usage_counts_merge(usage_daily.first_token_buckets, EXCLUDED.first_token_buckets),
                hidden_requests = usage_daily.hidden_requests + EXCLUDED.hidden_requests,
                short_requests = usage_daily.short_requests + EXCLUDED.short_requests,
                rated_rows = usage_daily.rated_rows + EXCLUDED.rated_rows",
        select = daily_select("e.id = $1")
    )
}

/// The statement that makes monthly rows from the daily rows that
/// `restriction` leaves, grouped by the key of a monthly row.
fn monthly_statement(restriction: &str) -> String {
    format!(
        "INSERT INTO usage_monthly (month, org_id, project_id, topic_id, user_id, purpose, provider_id, model, tier,
                harness_kind, environment_kind, turns, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
                reasoning_tokens, total_tokens, credits, estimated_credits, unrated_tokens)
         SELECT date_trunc('month', d.day)::date, (array_agg(d.org_id ORDER BY d.day, d.org_id, d.project_id))[1],
                (array_agg(d.project_id ORDER BY d.day, d.org_id, d.project_id))[1], d.topic_id, d.user_id, d.purpose, d.provider_id, d.model,
                d.tier, d.harness_kind, d.environment_kind, sum(d.turns)::int, sum(d.input_tokens)::bigint,
                sum(d.cached_input_tokens)::bigint, sum(d.cache_write_tokens)::bigint, sum(d.output_tokens)::bigint, sum(d.reasoning_tokens)::bigint,
                sum(d.total_tokens)::bigint, sum(d.credits), sum(d.estimated_credits), sum(d.unrated_tokens)::bigint
           FROM usage_daily d
          WHERE {restriction}
          GROUP BY date_trunc('month', d.day)::date, d.topic_id, d.user_id, d.purpose, d.provider_id, d.model,
                   d.tier, d.harness_kind, d.environment_kind
         ON CONFLICT (month, topic_id, user_id, purpose, provider_id, model, tier, harness_kind, environment_kind)
         DO UPDATE SET org_id = EXCLUDED.org_id, project_id = EXCLUDED.project_id, turns = EXCLUDED.turns,
                input_tokens = EXCLUDED.input_tokens, cached_input_tokens = EXCLUDED.cached_input_tokens, cache_write_tokens = EXCLUDED.cache_write_tokens,
                output_tokens = EXCLUDED.output_tokens, reasoning_tokens = EXCLUDED.reasoning_tokens,
                total_tokens = EXCLUDED.total_tokens, credits = EXCLUDED.credits,
                estimated_credits = EXCLUDED.estimated_credits, unrated_tokens = EXCLUDED.unrated_tokens"
    )
}

/// The statement that adds the ledger row `$1` to its monthly row, as [`daily_addition`] adds it to its daily
/// row (`$2`: the first counted row of its turn under that daily row, which then adds a turn to the month too).
/// The place is the earliest day's, read again from the days only when the row names another.
fn monthly_addition() -> String {
    let same_month = "d.day >= EXCLUDED.month AND d.day < (EXCLUDED.month + interval '1 month')::date
         AND d.topic_id = EXCLUDED.topic_id AND d.user_id IS NOT DISTINCT FROM EXCLUDED.user_id
         AND d.purpose = EXCLUDED.purpose AND d.provider_id = EXCLUDED.provider_id AND d.model = EXCLUDED.model
         AND d.tier IS NOT DISTINCT FROM EXCLUDED.tier AND d.harness_kind IS NOT DISTINCT FROM EXCLUDED.harness_kind
         AND d.environment_kind IS NOT DISTINCT FROM EXCLUDED.environment_kind";
    format!(
        "INSERT INTO usage_monthly (month, org_id, project_id, topic_id, user_id, purpose, provider_id, model, tier,
                harness_kind, environment_kind, turns, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
                reasoning_tokens, total_tokens, credits, estimated_credits, unrated_tokens)
         SELECT date_trunc('month', c.day)::date, c.org_id, c.project_id, c.topic_id, c.user_id, c.purpose, c.provider_id,
                c.model, c.tier, c.harness_kind, c.environment_kind, CASE WHEN $2 THEN c.turns ELSE 0 END, c.input_tokens,
                c.cached_input_tokens, c.cache_write_tokens, c.output_tokens, c.reasoning_tokens, c.total_tokens, c.credits,
                c.estimated_credits, c.unrated_tokens
           FROM ({select}) c
         ON CONFLICT (month, topic_id, user_id, purpose, provider_id, model, tier, harness_kind, environment_kind)
         DO UPDATE SET
                org_id = CASE WHEN EXCLUDED.org_id = usage_monthly.org_id
                                   AND EXCLUDED.project_id IS NOT DISTINCT FROM usage_monthly.project_id
                              THEN usage_monthly.org_id
                              ELSE (SELECT (array_agg(d.org_id ORDER BY d.day, d.org_id, d.project_id))[1] FROM usage_daily d WHERE {same_month}) END,
                project_id = CASE WHEN EXCLUDED.org_id = usage_monthly.org_id
                                       AND EXCLUDED.project_id IS NOT DISTINCT FROM usage_monthly.project_id
                                  THEN usage_monthly.project_id
                                  ELSE (SELECT (array_agg(d.project_id ORDER BY d.day, d.org_id, d.project_id))[1] FROM usage_daily d WHERE {same_month}) END,
                turns = usage_monthly.turns + EXCLUDED.turns,
                input_tokens = usage_monthly.input_tokens + EXCLUDED.input_tokens,
                cached_input_tokens = usage_monthly.cached_input_tokens + EXCLUDED.cached_input_tokens,
                cache_write_tokens = usage_monthly.cache_write_tokens + EXCLUDED.cache_write_tokens,
                output_tokens = usage_monthly.output_tokens + EXCLUDED.output_tokens,
                reasoning_tokens = usage_monthly.reasoning_tokens + EXCLUDED.reasoning_tokens,
                total_tokens = usage_monthly.total_tokens + EXCLUDED.total_tokens,
                credits = usage_monthly.credits + EXCLUDED.credits,
                estimated_credits = usage_monthly.estimated_credits + EXCLUDED.estimated_credits,
                unrated_tokens = usage_monthly.unrated_tokens + EXCLUDED.unrated_tokens",
        select = daily_select("e.id = $1")
    )
}

/// A date that the calendar has no name for: only a corrupt value gets here.
fn calendar() -> DbError {
    DbError::Other(sqlx::Error::Protocol(
        "a day outside the calendar".to_owned(),
    ))
}

/// The first day of the month `day` is in.
fn month_of(day: NaiveDate) -> Result<NaiveDate, DbError> {
    day.with_day(1).ok_or_else(calendar)
}

/// The moment a day begins, in UTC.
fn day_start(day: NaiveDate) -> Result<DateTime<Utc>, DbError> {
    Ok(day.and_hms_opt(0, 0, 0).ok_or_else(calendar)?.and_utc())
}

/// Brings the rollups up to date with a ledger row that was just written: what the row adds is added to its daily
/// row and to the monthly row that one belongs to. A row that does not count changes nothing. Call it once, in the
/// transaction that wrote the row; two writers of one topic and month wait for each other, so that a turn's first
/// row under a daily row is told by a read that sees every committed row before it.
pub async fn fold_event(conn: &mut Conn, event: Id) -> Result<(), DbError> {
    let key: Option<(DateTime<Utc>, Id)> = sqlx::query_as(
        "SELECT at, topic_id FROM usage_events WHERE id = $1 AND (counted OR outcome IS NOT NULL)",
    )
    .bind(event)
    .fetch_optional(&mut *conn)
    .await?;
    let Some((at, topic)) = key else {
        return Ok(());
    };
    let month = month_of(at.date_naive())?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("usage_rollup:{topic}:{month}"))
        .execute(&mut *conn)
        .await?;
    // A turn counts once for each daily row it has counted rows under.
    let first_of_turn: bool = sqlx::query_scalar(
        "SELECT e.counted AND e.turn_id IS NOT NULL AND NOT EXISTS (
             SELECT 1 FROM usage_events x
              WHERE x.turn_id = e.turn_id AND x.counted AND x.id <> e.id
                AND (x.at AT TIME ZONE 'UTC')::date = (e.at AT TIME ZONE 'UTC')::date
                AND x.topic_id = e.topic_id AND x.thread_id IS NOT DISTINCT FROM e.thread_id
                AND x.purpose = e.purpose AND x.provider_id = e.provider_id
                AND x.entry_id IS NOT DISTINCT FROM e.entry_id AND x.user_id IS NOT DISTINCT FROM e.user_id
                AND x.model = e.model AND x.tier IS NOT DISTINCT FROM e.tier
                AND x.harness_kind IS NOT DISTINCT FROM e.harness_kind
                AND x.environment_kind IS NOT DISTINCT FROM e.environment_kind
                AND x.effort IS NOT DISTINCT FROM e.effort AND x.access_program IS NOT DISTINCT FROM e.access_program)
           FROM usage_events e WHERE e.id = $1",
    )
    .bind(event)
    .fetch_one(&mut *conn)
    .await?;
    sqlx::query(sqlx::AssertSqlSafe(daily_addition()))
        .bind(event)
        .bind(first_of_turn)
        .execute(&mut *conn)
        .await?;
    sqlx::query(sqlx::AssertSqlSafe(monthly_addition()))
        .bind(event)
        .bind(first_of_turn)
        .execute(&mut *conn)
        .await?;
    Ok(())
}

/// What a rebuild wrote.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Rebuilt {
    /// How many daily rows.
    pub daily: u64,
    /// How many monthly rows.
    pub monthly: u64,
}

/// Makes the rollups again from the ledger alone: every daily row from the day
/// `from` on (all of them where none is given), and every monthly row from the
/// month that day is in. It is the statement [`fold_event`] runs for one row,
/// run for all of them, so the result is what the live tables hold. Live
/// writers wait for it, and it waits for those that are under way. Run it in a
/// transaction.
pub async fn rebuild(conn: &mut Conn, from: Option<NaiveDate>) -> Result<Rebuilt, DbError> {
    sqlx::query("LOCK TABLE usage_daily, usage_monthly IN SHARE ROW EXCLUSIVE MODE")
        .execute(&mut *conn)
        .await?;
    let (first_day, first_month) = match from {
        Some(day) => (Some(day), Some(month_of(day)?)),
        None => (None, None),
    };
    sqlx::query("DELETE FROM usage_daily WHERE $1::date IS NULL OR day >= $1")
        .bind(first_day)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM usage_monthly WHERE $1::date IS NULL OR month >= $1")
        .bind(first_month)
        .execute(&mut *conn)
        .await?;
    let begins = first_day.map(day_start).transpose()?;
    let daily = sqlx::query(sqlx::AssertSqlSafe(daily_statement(
        "($1::timestamptz IS NULL OR e.at >= $1)",
    )))
    .bind(begins)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let monthly = sqlx::query(sqlx::AssertSqlSafe(monthly_statement(
        "($1::date IS NULL OR d.day >= $1)",
    )))
    .bind(first_month)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    Ok(Rebuilt { daily, monthly })
}

// ---------------------------------------------------------------- what a read is limited to

/// How the rows of a place are found: by a column of their own where the
/// place has one, else by the way down to them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reach {
    /// A root org: rows name it.
    Root,
    /// A project: rows name it, or a project inside it. A row names the project its topic lies in
    /// directly, so the figures of a project are those of its own topics and of the topics of the
    /// folders in it, summed when they are asked for as a department's are.
    Project,
    /// A topic: rows name it.
    Topic,
    /// An org inside another, or a thread: found on the way down from each
    /// row's thread, or topic where it has none.
    Inside,
}

/// A place whose subtree a read covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Place {
    /// The node.
    pub id: Id,
    /// How rows are found inside it.
    pub reach: Reach,
}

/// The place a node is, for a read: found by its kind and by whether it lies in
/// another. None for a node that is not there.
pub async fn place_of(conn: &mut Conn, node: Id) -> Result<Option<Place>, DbError> {
    let found: Option<(String, Option<Id>)> =
        sqlx::query_as("SELECT kind, parent_id FROM nodes WHERE id = $1")
            .bind(node)
            .fetch_optional(conn)
            .await?;
    Ok(found.map(|(kind, parent)| Place {
        id: node,
        reach: match (kind.as_str(), parent) {
            ("org", None) => Reach::Root,
            ("project", _) => Reach::Project,
            ("topic", _) => Reach::Topic,
            _ => Reach::Inside,
        },
    }))
}

/// What a read is limited to. Every member that is set must hold.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only this turn; reads use the ledger when it is selected.
    pub turn: Option<Id>,
    /// Only work in this org or department.
    pub org: Option<Id>,
    /// Only this reasoning effort; `none` selects a null value.
    pub effort: Option<String>,
    /// Only this service tier; `none` selects a null value.
    pub tier: Option<String>,
    /// Only this access programme; `none` selects a null value.
    pub program: Option<String>,
    /// Only this outcome code; `running` selects provisional rows.
    pub outcome: Option<orch_api_contract::usage::UsageOutcomeFilter>,
    /// Only what was done in this place or below it.
    pub node: Option<Place>,
    /// Only what ran on this provider.
    pub provider: Option<Id>,
    /// Only what is put down to this person.
    pub person: Option<Id>,
    /// Only this model, by its routing name.
    pub model: Option<String>,
    /// Only this harness kind.
    pub harness_kind: Option<String>,
    /// Only this kind of environment: `managed` or `connected`.
    pub environment_kind: Option<String>,
    /// Only this project.
    pub project: Option<Id>,
    /// Only this topic.
    pub topic: Option<Id>,
    /// Only this thread.
    pub thread: Option<Id>,
}

/// Adds ` AND …` for each member of the filter, for rows aliased `d`.
pub(crate) fn push_filter(query: &mut QueryBuilder<Postgres>, filter: &Filter) {
    if let Some(turn) = filter.turn {
        query.push(" AND d.turn_id = ").push_bind(turn);
    }
    if let Some(org) = filter.org {
        query.push(" AND EXISTS(SELECT 1 FROM nodes usage_place WHERE usage_place.id=d.project_id AND ").push_bind(org).push("=ANY(usage_place.path))");
    }
    for (column, value) in [
        ("effort", &filter.effort),
        ("tier", &filter.tier),
        ("access_program", &filter.program),
    ] {
        if let Some(value) = value {
            query
                .push(format!(" AND d.{column} IS NOT DISTINCT FROM "))
                .push_bind((value != "none").then(|| value.clone()));
        }
    }
    if let Some(outcome) = filter.outcome {
        use orch_api_contract::usage::UsageOutcomeFilter;
        match outcome {
            UsageOutcomeFilter::Running => {
                query.push(" AND false");
            }
            UsageOutcomeFilter::Errors => {
                query.push(" AND d.outcome IS NOT NULL AND d.outcome NOT IN ('done','forced')");
            }
            UsageOutcomeFilter::NotMeasured => {
                query.push(" AND d.outcome IS NULL");
            }
            code => {
                query.push(" AND d.outcome = ").push_bind(code.code());
            }
        }
    }
    if let Some(place) = filter.node {
        match place.reach {
            Reach::Root => query.push(" AND d.org_id = ").push_bind(place.id),
            Reach::Project => query
                .push(" AND d.project_id IN (SELECT pn.id FROM nodes pn WHERE pn.kind = 'project' AND ")
                .push_bind(place.id)
                .push(" = ANY(pn.path))"),
            Reach::Topic => query.push(" AND d.topic_id = ").push_bind(place.id),
            Reach::Inside => query
                .push(
                    " AND EXISTS (SELECT 1 FROM nodes pn
                                   WHERE pn.id = COALESCE(d.thread_id, d.topic_id) AND ",
                )
                .push_bind(place.id)
                .push(" = ANY(pn.path))"),
        };
    }
    if let Some(provider) = filter.provider {
        query.push(" AND d.provider_id = ").push_bind(provider);
    }
    if let Some(person) = filter.person {
        query.push(" AND d.user_id = ").push_bind(person);
    }
    if let Some(model) = &filter.model {
        query.push(" AND d.model = ").push_bind(model.clone());
    }
    if let Some(kind) = &filter.harness_kind {
        query.push(" AND d.harness_kind = ").push_bind(kind.clone());
    }
    if let Some(kind) = &filter.environment_kind {
        query
            .push(" AND d.environment_kind = ")
            .push_bind(kind.clone());
    }
    if let Some(project) = filter.project {
        query.push(" AND d.project_id = ").push_bind(project);
    }
    if let Some(topic) = filter.topic {
        query.push(" AND d.topic_id = ").push_bind(topic);
    }
    if let Some(thread) = filter.thread {
        query.push(" AND d.thread_id = ").push_bind(thread);
    }
}

// ---------------------------------------------------------------- reading

/// The sums of the counted rows of a span.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Figures {
    /// Raw performance sums and mergeable histograms.
    #[sqlx(flatten)]
    pub performance: super::performance::Sums,
    /// How many turns.
    pub turns: i64,
    /// Tokens of input.
    pub input_tokens: i64,
    /// Of them, read from a cache.
    pub cached_input_tokens: i64,
    /// Of the input, written to the cache.
    pub cache_write_tokens: i64,
    /// Tokens of output.
    pub output_tokens: i64,
    /// Of them, reasoning.
    pub reasoning_tokens: i64,
    /// Input plus output.
    pub total_tokens: i64,
    /// The credits of the rows that had a rate, as a decimal number.
    pub credits: String,
    /// The part of them that the shipped catalogue estimated.
    pub estimated_credits: String,
    /// The tokens of the rows that had none.
    pub unrated_tokens: i64,
}

impl Default for Figures {
    fn default() -> Self {
        Self {
            performance: super::performance::Sums::default(),
            turns: 0,
            input_tokens: 0,
            cached_input_tokens: 0,
            cache_write_tokens: 0,
            output_tokens: 0,
            reasoning_tokens: 0,
            total_tokens: 0,
            credits: "0".to_owned(),
            estimated_credits: "0".to_owned(),
            unrated_tokens: 0,
        }
    }
}

/// One group of a read: the sums of the rows of a bucket, a member of a
/// dimension, or both.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct Cell {
    /// The start of the bucket, where the read was cut into buckets.
    pub bucket: Option<DateTime<Utc>>,
    /// The member of the dimension, as text (an identifier, a name), where the
    /// read was split by one; none for rows that have no such member.
    pub key: Option<String>,
    /// The sums.
    #[sqlx(flatten)]
    pub figures: Figures,
}

/// What to add up.
#[derive(Debug, Clone)]
pub struct Read<'a> {
    /// The first day, UTC.
    pub from: NaiveDate,
    /// The last day, UTC.
    pub to: NaiveDate,
    /// What the rows are limited to.
    pub filter: &'a Filter,
    /// How to cut the span.
    pub bucket: Option<UsageBucket>,
    /// What to split by.
    pub by: Option<UsageDimension>,
    /// For the dimension `org`: the org asked about, whose child orgs are the
    /// members. Without it the members are the root orgs.
    pub under: Option<Id>,
}

/// Whether a read has to come from the ledger: a bucket by the hour, and the
/// dimension the rollups do not keep (the effort; the tier they do).
fn needs_ledger(read: &Read<'_>) -> bool {
    read.bucket == Some(UsageBucket::Hour)
        || read.filter.outcome.is_some()
        || read.filter.turn.is_some()
}

/// The measures of a read, from the daily rows (`d`).
const DAILY_MEASURES: &str = "COALESCE(sum(d.requests),0)::bigint AS requests,
        COALESCE(sum(d.hidden_requests),0)::bigint AS hidden_requests,COALESCE(sum(d.short_requests),0)::bigint AS short_requests,
        usage_merge_counts(d.errors) AS errors, COALESCE(sum(d.speed_tokens),0)::bigint AS speed_tokens,
        COALESCE(sum(d.generation_ms_sum),0)::bigint AS generation_ms_sum,
        usage_merge_counts(d.speed_buckets) AS speed_buckets, usage_merge_counts(d.first_token_buckets) AS first_token_buckets,
        COALESCE(sum(d.turns), 0)::bigint AS turns,
        COALESCE(sum(d.input_tokens), 0)::bigint AS input_tokens,
        COALESCE(sum(d.cached_input_tokens), 0)::bigint AS cached_input_tokens,
        COALESCE(sum(d.cache_write_tokens), 0)::bigint AS cache_write_tokens,
        COALESCE(sum(d.output_tokens), 0)::bigint AS output_tokens,
        COALESCE(sum(d.reasoning_tokens), 0)::bigint AS reasoning_tokens,
        COALESCE(sum(d.total_tokens), 0)::bigint AS total_tokens,
        COALESCE(sum(d.credits), 0)::text AS credits,
        COALESCE(sum(d.estimated_credits), 0)::text AS estimated_credits,
        COALESCE(sum(d.unrated_tokens), 0)::bigint AS unrated_tokens";

/// The same measures, from the counted rows of the ledger (`d`).
const LEDGER_MEASURES: &str = "count(d.outcome)::bigint AS requests, usage_counts(CASE WHEN PROVIDER_ERROR THEN d.outcome END) AS errors,
        count(*) FILTER (WHERE d.outcome IS NOT NULL AND d.measurement_states->>'reasoning_hidden'='true')::bigint AS hidden_requests,
        count(*) FILTER (WHERE d.outcome IS NOT NULL AND d.measurement_states->'speed_milli'->>'state'='empty')::bigint AS short_requests,
        COALESCE(sum(d.streamed_output_tokens) FILTER (WHERE d.speed_milli IS NOT NULL),0)::bigint AS speed_tokens,
        COALESCE(sum(d.generation_ms) FILTER (WHERE d.speed_milli IS NOT NULL),0)::bigint AS generation_ms_sum,
        usage_histogram(d.speed_milli::double precision/1000) AS speed_buckets,
        usage_histogram(d.first_token_ms::double precision) AS first_token_buckets,
        count(DISTINCT d.turn_id) FILTER (WHERE d.counted)::bigint AS turns,
        COALESCE(sum(d.input_tokens) FILTER (WHERE d.counted), 0)::bigint AS input_tokens,
        COALESCE(sum(d.cached_input_tokens) FILTER (WHERE d.counted), 0)::bigint AS cached_input_tokens,
        COALESCE(sum(d.cache_write_tokens) FILTER (WHERE d.counted), 0)::bigint AS cache_write_tokens,
        COALESCE(sum(d.output_tokens) FILTER (WHERE d.counted), 0)::bigint AS output_tokens,
        COALESCE(sum(d.reasoning_tokens) FILTER (WHERE d.counted), 0)::bigint AS reasoning_tokens,
        COALESCE(sum(d.total_tokens) FILTER (WHERE d.counted), 0)::bigint AS total_tokens,
        COALESCE(sum(d.credits) FILTER (WHERE d.counted), 0)::text AS credits,
        COALESCE(sum(d.credits) FILTER (WHERE d.counted AND d.rate_source = 'catalogue'), 0)::text AS estimated_credits,
        COALESCE(sum(d.total_tokens) FILTER (WHERE d.counted AND d.credits IS NULL), 0)::bigint AS unrated_tokens";

/// Adds up the counted use of a span, whole, in buckets, by a dimension, or
/// both. The rollups are read where they hold what is asked; the ledger where
/// they do not. Either way the figures are the same: the rollups are the
/// ledger's sums.
pub async fn read(conn: &mut Conn, read: &Read<'_>) -> Result<Vec<Cell>, DbError> {
    read_cells(conn, read, true).await
}

/// As [`read`], without the count of turns, which is `0` in every cell: turns are counted from the ledger, every row
/// of the span, while the rest comes from the rollups' rows of its days. What compares a provider's speed and errors
/// with its last week reads no turn.
pub async fn performance_of(conn: &mut Conn, read: &Read<'_>) -> Result<Vec<Cell>, DbError> {
    read_cells(conn, read, false).await
}

async fn read_cells(
    conn: &mut Conn,
    read: &Read<'_>,
    count_turns: bool,
) -> Result<Vec<Cell>, DbError> {
    let ledger_source = needs_ledger(read);
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    let mut groups = 0;
    // The bucket.
    match read.bucket {
        Some(bucket) => {
            groups += 1;
            let day = if ledger_source {
                "(d.at AT TIME ZONE 'UTC')"
            } else {
                "d.day::timestamp"
            };
            let unit = match bucket {
                UsageBucket::Hour => "hour",
                UsageBucket::Day => "day",
                UsageBucket::Week => "week",
                UsageBucket::Month => "month",
            };
            query.push(format!(
                "date_trunc('{unit}', {day}) AT TIME ZONE 'UTC' AS bucket, "
            ));
        }
        None => {
            query.push("NULL::timestamptz AS bucket, ");
        }
    }
    // The dimension.
    match read.by {
        Some(dimension) => {
            groups += 1;
            push_dimension(&mut query, dimension, read.under);
            query.push(" AS key, ");
        }
        None => {
            query.push("NULL::text AS key, ");
        }
    }
    if ledger_source {
        query.push(ledger_measures());
    } else {
        query.push(DAILY_MEASURES);
    }
    let from = day_start(read.from)?;
    let after = day_start(read.to.succ_opt().ok_or_else(calendar)?)?;
    if ledger_source {
        query
            .push(" FROM usage_events d WHERE (d.counted OR d.outcome IS NOT NULL) AND d.at >= ")
            .push_bind(from)
            .push(" AND d.at < ")
            .push_bind(after);
    } else {
        query
            .push(" FROM usage_daily d WHERE d.day >= ")
            .push_bind(read.from)
            .push(" AND d.day <= ")
            .push_bind(read.to);
    }
    push_filter(&mut query, read.filter);
    // The bucket is the first column of the answer and the member the second, whether or not they are used.
    let mut positions: Vec<&str> = Vec::new();
    if read.bucket.is_some() {
        positions.push("1");
    }
    if read.by.is_some() {
        positions.push("2");
    }
    if groups > 0 {
        query.push(format!(" GROUP BY {}", positions.join(", ")));
    }
    let mut cells = query.build_query_as::<Cell>().fetch_all(&mut *conn).await?;
    if !count_turns {
        for cell in &mut cells {
            cell.figures.turns = 0;
        }
        return Ok(cells);
    }
    // Counting tokens chooses one meter; counting turns must not depend on that meter having
    // reported tokens. Nor may a turn be counted twice because it crossed a day, model or tier.
    // Use precisely the historical records' attribution/span/filter, distinct by turn within
    // each requested group. This also supplies groups containing only unmetered turns.
    let turns = turn_counts(conn, read).await?;
    let mut positions: std::collections::HashMap<_, _> = cells
        .iter()
        .enumerate()
        .map(|(at, cell)| ((cell.bucket, cell.key.clone()), at))
        .collect();
    for cell in &mut cells {
        cell.figures.turns = 0;
    }
    for count in turns {
        if let Some(at) = positions.get(&(count.bucket, count.key.clone())) {
            cells[*at].figures.turns = count.turns;
        } else {
            positions.insert((count.bucket, count.key.clone()), cells.len());
            cells.push(Cell {
                bucket: count.bucket,
                key: count.key,
                figures: Figures {
                    turns: count.turns,
                    ..Figures::default()
                },
            });
        }
    }
    Ok(cells)
}

#[derive(FromRow)]
struct TurnCount {
    bucket: Option<DateTime<Utc>>,
    key: Option<String>,
    turns: i64,
}

/// A turn is work even when the harness reports no tokens; standalone summaries have no turn id.
async fn turn_counts(conn: &mut Conn, read: &Read<'_>) -> Result<Vec<TurnCount>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    if let Some(bucket) = read.bucket {
        let unit = match bucket {
            UsageBucket::Hour => "hour",
            UsageBucket::Day => "day",
            UsageBucket::Week => "week",
            UsageBucket::Month => "month",
        };
        query.push(format!(
            "date_trunc('{unit}',d.at AT TIME ZONE 'UTC') AT TIME ZONE 'UTC' AS bucket, "
        ));
    } else {
        query.push("NULL::timestamptz AS bucket, ");
    }
    if let Some(by) = read.by {
        push_dimension(&mut query, by, read.under);
        query.push(" AS key, ");
    } else {
        query.push("NULL::text AS key, ");
    }
    query
        .push("count(DISTINCT d.turn_id)::bigint AS turns FROM usage_events d WHERE d.at >= ")
        .push_bind(day_start(read.from)?)
        .push(" AND d.at < ")
        .push_bind(day_start(read.to.succ_opt().ok_or_else(calendar)?)?);
    push_filter(&mut query, read.filter);
    if read.bucket.is_some() || read.by.is_some() {
        query.push(if read.bucket.is_some() && read.by.is_some() {
            " GROUP BY 1,2"
        } else if read.bucket.is_some() {
            " GROUP BY 1"
        } else {
            " GROUP BY 2"
        });
    }
    Ok(query.build_query_as::<TurnCount>().fetch_all(conn).await?)
}

/// Exact-time request figures for short health windows, using the provider/time ledger index.
pub async fn window(
    conn: &mut Conn,
    from: DateTime<Utc>,
    until: DateTime<Utc>,
    filter: &Filter,
) -> Result<Figures, DbError> {
    let mut query = QueryBuilder::<Postgres>::new("SELECT ");
    query
        .push(ledger_measures())
        .push(" FROM usage_events d WHERE d.outcome IS NOT NULL AND d.at >= ")
        .push_bind(from)
        .push(" AND d.at < ")
        .push_bind(until);
    push_filter(&mut query, filter);
    Ok(query.build_query_as::<Figures>().fetch_one(conn).await?)
}

/// The SQL of a dimension's member, for rows aliased `d`, as text.
pub(crate) fn push_dimension(
    query: &mut QueryBuilder<Postgres>,
    dimension: UsageDimension,
    under: Option<Id>,
) {
    match dimension {
        UsageDimension::Person => query.push("d.user_id::text"),
        UsageDimension::Model => query.push("d.model"),
        UsageDimension::Provider => query.push("d.provider_id::text"),
        UsageDimension::Agent => query.push("d.harness_kind"),
        UsageDimension::Environment => query.push("d.environment_kind"),
        UsageDimension::Effort => query.push("d.effort"),
        UsageDimension::Tier => query.push("d.tier"),
        UsageDimension::Program => query.push("d.access_program"),
        UsageDimension::Org => match under {
            Some(org) => query
                .push(
                    "(SELECT c.id::text FROM nodes pn JOIN nodes c ON c.id = pn.path[array_position(pn.path, ",
                )
                .push_bind(org)
                .push(") + 1] WHERE pn.id = d.project_id AND c.kind = 'org')"),
            None => query.push("d.org_id::text"),
        },
        UsageDimension::Project => query.push("d.project_id::text"),
        UsageDimension::Topic => query.push("d.topic_id::text"),
        UsageDimension::Thread => query.push("d.thread_id::text"),
    };
}

/// What was used without a rate since a day, for each model of each provider:
/// the tokens a figure in credits leaves out, so that whoever sets rates is led
/// to the models that have none.
pub async fn unrated_by_model(
    conn: &mut Conn,
    filter: &Filter,
    since: NaiveDate,
) -> Result<Vec<(Id, String, i64)>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT d.provider_id, d.model, COALESCE(sum(d.unrated_tokens), 0)::bigint
           FROM usage_daily d WHERE d.unrated_tokens > 0 AND d.day >= ",
    );
    query.push_bind(since);
    push_filter(&mut query, filter);
    query.push(" GROUP BY d.provider_id, d.model");
    Ok(query
        .build_query_as::<(Id, String, i64)>()
        .fetch_all(conn)
        .await?)
}

/// How many days a span covers, both ends included.
#[must_use]
pub fn days_in(from: NaiveDate, to: NaiveDate) -> i64 {
    (to - from).num_days() + 1
}

/// The start of the bucket a day falls in.
#[must_use]
pub fn bucket_start(day: NaiveDate, bucket: UsageBucket) -> NaiveDate {
    match bucket {
        UsageBucket::Hour | UsageBucket::Day => day,
        UsageBucket::Week => day - Duration::days(i64::from(day.weekday().num_days_from_monday())),
        UsageBucket::Month => day.with_day(1).unwrap_or(day),
    }
}

// ---------------------------------------------------------------- what the figures are named by

/// Performance for each model, tier, effort and programme, read from daily histograms.
pub async fn comparison(
    conn: &mut Conn,
    from: NaiveDate,
    to: NaiveDate,
    filter: &Filter,
) -> Result<Vec<orch_api_contract::usage::UsageComparison>, DbError> {
    #[derive(sqlx::FromRow)]
    struct Combination {
        model: String,
        tier: Option<String>,
        effort: Option<String>,
        program: Option<String>,
        #[sqlx(flatten)]
        sums: super::performance::Sums,
    }
    let ledger = filter.outcome.is_some() || filter.turn.is_some();
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT d.model,d.tier,d.effort,d.access_program AS program,",
    );
    if ledger {
        query.push(ledger_measures());
    } else {
        query.push(DAILY_MEASURES);
    }
    if ledger {
        query
            .push(" FROM usage_events d WHERE (d.counted OR d.outcome IS NOT NULL) AND d.at >= ")
            .push_bind(day_start(from)?)
            .push(" AND d.at < ")
            .push_bind(day_start(to.succ_opt().ok_or_else(calendar)?)?);
    } else {
        query
            .push(" FROM usage_daily d WHERE d.day >= ")
            .push_bind(from)
            .push(" AND d.day <= ")
            .push_bind(to);
    }
    push_filter(&mut query, filter);
    query.push(" GROUP BY d.model,d.tier,d.effort,d.access_program ORDER BY d.model,d.tier,d.effort,d.access_program");
    // Legacy usage without an observed request outcome is not a zero-request comparison.
    Ok(query
        .build_query_as::<Combination>()
        .fetch_all(conn)
        .await?
        .into_iter()
        .filter(|row| row.sums.requests > 0)
        .map(|r| orch_api_contract::usage::UsageComparison {
            model: r.model,
            tier: r.tier,
            effort: r.effort,
            program: r.program,
            performance: r.sums.figures(),
        })
        .collect())
}

/// Seven-day model comparisons for a permitted provider list, grouped in one statement.
pub async fn provider_comparisons(
    conn: &mut Conn,
    from: NaiveDate,
    to: NaiveDate,
    providers: &[Id],
) -> Result<Vec<(Id, orch_api_contract::usage::UsageComparison)>, DbError> {
    #[derive(sqlx::FromRow)]
    struct Combination {
        provider_id: Id,
        model: String,
        tier: Option<String>,
        effort: Option<String>,
        program: Option<String>,
        #[sqlx(flatten)]
        sums: super::performance::Sums,
    }
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT d.provider_id,d.model,d.tier,d.effort,d.access_program AS program,",
    );
    query.push(DAILY_MEASURES)
        .push(" FROM usage_daily d WHERE d.provider_id = ANY(").push_bind(providers)
        .push(") AND d.day >= ").push_bind(from).push(" AND d.day <= ").push_bind(to)
        .push(" GROUP BY d.provider_id,d.model,d.tier,d.effort,d.access_program ORDER BY d.provider_id,d.model,d.tier,d.effort,d.access_program");
    Ok(query
        .build_query_as::<Combination>()
        .fetch_all(conn)
        .await?
        .into_iter()
        .filter(|row| row.sums.requests > 0)
        .map(|row| {
            (
                row.provider_id,
                orch_api_contract::usage::UsageComparison {
                    model: row.model,
                    tier: row.tier,
                    effort: row.effort,
                    program: row.program,
                    performance: row.sums.figures(),
                },
            )
        })
        .collect())
}

/// A model that a place could run, with what a rate for it is looked up by.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct UsableModel {
    /// The provider.
    pub provider_id: Id,
    /// The routing name.
    pub model_name: String,
    /// The name to show; empty when the routing name is.
    pub display_name: String,
    /// What the entry holds, among it the provider's own rate.
    pub parameters: orch_types::Json,
}

/// The enabled models of the providers a place could run: those of the root org it lies in, and those
/// that were used inside it (a provider of another org that was shared with it, or the system's).
pub async fn models_usable_at(
    conn: &mut Conn,
    root_org: Id,
    place: Option<Place>,
) -> Result<Vec<UsableModel>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT pm.provider_id, pm.model_name, pm.display_name, pm.parameters
           FROM provider_models pm JOIN providers p ON p.id = pm.provider_id
          WHERE pm.enabled AND p.deleted_at IS NULL
            AND (p.org_id = ",
    );
    query.push_bind(root_org);
    if place.is_some() {
        query.push(" OR p.id IN (SELECT DISTINCT d.provider_id FROM usage_daily d WHERE true");
        push_filter(
            &mut query,
            &Filter {
                node: place,
                ..Filter::default()
            },
        );
        query.push(")");
    }
    query.push(") ORDER BY p.created_at, p.id, pm.model_name");
    Ok(query.build_query_as().fetch_all(conn).await?)
}

/// The providers a place could run, whether or not they have a model on offer yet: those of the root org it
/// lies in, and those that were used inside it. The caps defined on them bound work there.
pub async fn providers_usable_at(
    conn: &mut Conn,
    root_org: Id,
    place: Option<Place>,
) -> Result<Vec<Id>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT p.id FROM providers p WHERE p.deleted_at IS NULL AND (p.org_id = ",
    );
    query.push_bind(root_org);
    if place.is_some() {
        query.push(" OR p.id IN (SELECT DISTINCT d.provider_id FROM usage_daily d WHERE true");
        push_filter(
            &mut query,
            &Filter {
                node: place,
                ..Filter::default()
            },
        );
        query.push(")");
    }
    query.push(") ORDER BY p.created_at, p.id");
    Ok(query.build_query_scalar().fetch_all(conn).await?)
}

/// What each model of the providers asked about is shown as there: its display name, else its routing
/// name. A pair that has no entry any more is not among them.
pub async fn displays_of(
    conn: &mut Conn,
    pairs: &[(Id, String)],
) -> Result<std::collections::HashMap<(Id, String), String>, DbError> {
    if pairs.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let providers: Vec<Id> = pairs.iter().map(|pair| pair.0).collect();
    let names: Vec<String> = pairs.iter().map(|pair| pair.1.clone()).collect();
    let found: Vec<(Id, String, String)> = sqlx::query_as(
        "SELECT provider_id, model_name, display_name FROM provider_models
          WHERE provider_id = ANY($1) AND model_name = ANY($2)",
    )
    .bind(providers)
    .bind(names)
    .fetch_all(conn)
    .await?;
    Ok(found
        .into_iter()
        .map(|(provider, model, display)| ((provider, model), display))
        .collect())
}

/// What a model is shown as where no provider is named: the display name of the entry that was made first.
pub async fn display_names(
    conn: &mut Conn,
    models: &[String],
) -> Result<std::collections::HashMap<String, String>, DbError> {
    if models.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let found: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (model_name) model_name, display_name FROM provider_models
          WHERE model_name = ANY($1) AND display_name <> '' ORDER BY model_name, created_at, id",
    )
    .bind(models)
    .fetch_all(conn)
    .await?;
    Ok(found.into_iter().collect())
}

/// The name of an agent profile of each harness kind, for the screens that name an agent by its kind: the
/// system's own profile where there is one, else the oldest.
pub async fn agent_names(
    conn: &mut Conn,
    kinds: &[String],
) -> Result<std::collections::HashMap<String, String>, DbError> {
    if kinds.is_empty() {
        return Ok(std::collections::HashMap::new());
    }
    let found: Vec<(String, String)> = sqlx::query_as(
        "SELECT DISTINCT ON (harness_kind) harness_kind, name FROM agents
          WHERE harness_kind = ANY($1) AND deleted_at IS NULL
          ORDER BY harness_kind, (owner_id IS NULL) DESC, created_at, id",
    )
    .bind(kinds)
    .fetch_all(conn)
    .await?;
    Ok(found.into_iter().collect())
}
