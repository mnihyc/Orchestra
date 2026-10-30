//! Append-only usage events, each with what it ran on (model, provider, agent, kind of environment, place, person) and the rate it was counted at, kept for good.
//!
//! A row says what one use was for and what it ran on, copied as it was when the use happened, so that a later
//! rename, switch or move changes no figure: the place (root org, project, topic, thread, segment and turn),
//! the person it is put down to, the provider, the agent profile with its harness kind, the environment with
//! the kind of place it was, the arrangement, the model by the name that was sent, its effort and service tier,
//! the tokens by kind, and the rate the credits were counted at with where that rate came from. A row is
//! written once and is never edited or removed; its identifier is chosen by the caller from what the use is, so
//! that a use that is reported twice makes one row ([`record`] answers whether it made one).
//!
//! What counts: exactly one source is counted for a turn, so that nothing is counted twice. The caller says
//! (`counted`) whether the row feeds the rollups or is kept as trace. Rows are read back by turn
//! ([`events_of_turn`]) and summed for the part of them that counts ([`counted_request_totals`]).
//!
//! The rollups by day and by month are derived from these rows and can be rebuilt from them at any time: a row
//! that counts is folded into them (`rollup::fold_event`) in the same transaction that writes it.

use super::rollup;
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// Who reported a use: the words of the `origin` column.
pub mod origin {
    /// The harness, in the totals it reports for a turn.
    pub const HARNESS: &str = "harness";
    /// The forwarder, for each model request it relays.
    pub const FORWARDER: &str = "forwarder";
    /// A model call Orch made itself.
    pub const ORCH: &str = "orch";
}

/// What a row measures: the words of the `scope` column.
pub mod scope {
    /// A turn's total.
    pub const TURN: &str = "turn";
    /// One model request.
    pub const REQUEST: &str = "request";
}

/// Where the rate of a row came from: the words of the `rate_source` column.
pub mod rate_source {
    /// The provider's own entry for the model.
    pub const PROVIDER: &str = "provider";
    /// The rate card at the thread's place.
    pub const RATE_CARD: &str = "rate_card";
    /// The shipped catalogue's estimate.
    pub const CATALOGUE: &str = "catalogue";
}

/// What the purpose of a use was: the words of the `purpose` column.
pub mod purpose {
    /// A turn of a thread.
    pub const TURN: &str = "turn";
    /// A summary Orch asked a model for itself.
    pub const SUMMARY: &str = "summary";
    /// A harness permission review, reported apart from its ordinary turn totals.
    pub const REVIEW: &str = "review";
    /// Vectors Orch asked a model for itself, of an org's text or of a query, for search by meaning.
    pub const EMBEDDINGS: &str = "embeddings";
}

/// Tokens by kind. Cached tokens and tokens written to a cache are parts of the input, reasoning tokens are
/// part of the output, and the total is the input plus the output.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, sqlx::FromRow)]
pub struct Tokens {
    /// Tokens of input, those read from or written to a cache among them.
    pub input: i64,
    /// Of the input, those read from a cache.
    pub cached_input: i64,
    /// Of the input, those written to a prompt cache, where the upstream reports them apart.
    pub cache_write: i64,
    /// Tokens of output, the reasoning among them.
    pub output: i64,
    /// Of the output, the reasoning.
    pub reasoning: i64,
    /// Input plus output.
    pub total: i64,
}

/// What a use of a lease is attributed to and what it ran on, as it stands when the use is written.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Attribution {
    /// The root org the work was in.
    pub org_id: Id,
    /// The project.
    pub project_id: Id,
    /// The topic.
    pub topic_id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The segment the turn was in.
    pub segment_id: Id,
    /// The turn.
    pub turn_id: Id,
    /// The provider whose key paid.
    pub provider_id: Id,
    /// The entry through which the author used the provider.
    pub entry_id: Option<Id>,
    /// The person the use is put down to: the author of the input that started the turn; nobody for an input
    /// that no person sent.
    pub user_id: Option<Id>,
    /// The agent profile the turn ran.
    pub agent_id: Option<Id>,
    /// Its harness kind.
    pub harness_kind: Option<String>,
    /// The environment the tools ran in.
    pub environment_id: Option<Id>,
    /// `managed` or `connected`: the kind of place the environment was.
    pub environment_kind: Option<String>,
    /// The arrangement type the turn ran in.
    pub arrangement: Option<String>,
    /// The model the turn was started with, by its routing name.
    pub turn_model: String,
    /// The effort the turn ran with.
    pub effort: Option<String>,
    /// The parameters the turn was started with, which may name a service tier.
    pub params: Json,
}

/// The attribution of the lease `lease`, read from the turn it is for. None where the lease has no turn or
/// the turn's thread is gone. The topic's row is held against a change while the transaction lasts: a topic
/// that is moved waits for the use being written to be committed, and gives the rows of it the project it
/// lies in then, so that no row is left naming the project it left.
pub async fn attribution_of_lease(
    conn: &mut Conn,
    lease: Id,
) -> Result<Option<Attribution>, DbError> {
    Ok(sqlx::query_as(
        "SELECT topic.org_id AS org_id, topic.parent_id AS project_id, th.topic_id AS topic_id,
                l.thread_id AS thread_id, t.segment_id AS segment_id, t.id AS turn_id,
                l.provider_id AS provider_id, l.entry_id AS entry_id, i.author_id AS user_id,
                s.agent_id AS agent_id, a.harness_kind AS harness_kind,
                s.environment_id AS environment_id, e.placement AS environment_kind,
                s.arrangement AS arrangement, t.model AS turn_model, t.effort AS effort, t.params AS params
           FROM leases l
           JOIN turns t ON t.id = l.turn_id
           JOIN segments s ON s.id = t.segment_id
           JOIN threads th ON th.node_id = l.thread_id
           JOIN nodes topic ON topic.id = th.topic_id
           LEFT JOIN thread_inputs i ON i.id = t.input_id
           LEFT JOIN agents a ON a.id = s.agent_id
           LEFT JOIN environments e ON e.id = s.environment_id
          WHERE l.id = $1
            FOR SHARE OF topic",
    )
    .bind(lease)
    .fetch_optional(conn)
    .await?)
}

/// A use to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NewEvent<'a> {
    /// Its identifier, chosen from what the use is: a use reported twice has the same one.
    pub id: Id,
    /// When it happened.
    pub at: DateTime<Utc>,
    /// Who reported it: see [`origin`].
    pub origin: &'a str,
    /// What it measures: see [`scope`].
    pub scope: &'a str,
    /// Whether it feeds the rollups.
    pub counted: bool,
    /// What it is attributed to.
    pub attribution: &'a Attribution,
    /// The model, by the name that was sent.
    pub model: &'a str,
    /// The service tier it ran with.
    pub tier: Option<&'a str>,
    /// What it used.
    pub tokens: Tokens,
    /// How large the prompt was.
    pub context_tokens: Option<i64>,
    /// The upstream's own name for the request.
    pub request_ref: Option<&'a str>,
    /// The rate it is counted at, copied.
    pub rate: Option<&'a Json>,
    /// Where the rate came from: see [`rate_source`].
    pub rate_source: Option<&'a str>,
    /// The credits it comes to, as a decimal number.
    pub credits: Option<&'a str>,
}

/// Writes a use. The answer is whether this call wrote it: a use whose identifier is in the ledger is left as
/// it is.
pub async fn record(conn: &mut Conn, event: &NewEvent<'_>) -> Result<bool, DbError> {
    record_as(conn, event, purpose::TURN, None, None).await
}

/// Records a measured request, or keeps its provisional running observation until completion.
pub async fn record_measured(
    conn: &mut Conn,
    event: &NewEvent<'_>,
    timing: &orch_api_contract::usage::RequestTiming,
) -> Result<bool, DbError> {
    record_as(conn, event, purpose::TURN, Some(timing), None).await
}

/// The engine resolves programme roles from any adapter's parameter declarations.
pub async fn record_program(
    conn: &mut Conn,
    event: &NewEvent<'_>,
    timing: Option<&orch_api_contract::usage::RequestTiming>,
    program: Option<&str>,
) -> Result<bool, DbError> {
    record_as(conn, event, purpose::TURN, timing, program).await
}

/// Records native permission-review use with its own model, without changing the turn's model total.
pub async fn record_review(conn: &mut Conn, event: &NewEvent<'_>) -> Result<bool, DbError> {
    record_as(conn, event, purpose::REVIEW, None, None).await
}

async fn record_as(
    conn: &mut Conn,
    event: &NewEvent<'_>,
    purpose: &str,
    timing: Option<&orch_api_contract::usage::RequestTiming>,
    program: Option<&str>,
) -> Result<bool, DbError> {
    let attribution = event.attribution;
    let running = timing.is_some_and(|t| {
        matches!(
            t.outcome,
            orch_api_contract::usage::UsageMeasurement::Unread
        )
    });
    // A late running notification cannot resurrect a request already in the immutable ledger.
    if running && event_exists(conn, event.id).await? {
        return Ok(false);
    }
    let table = if running {
        "usage_running"
    } else {
        "usage_events"
    };
    let conflict = if running {
        "DO UPDATE SET first_token_ms=EXCLUDED.first_token_ms, generation_ms=EXCLUDED.generation_ms, request_ms=EXCLUDED.request_ms, measurement_states=EXCLUDED.measurement_states"
    } else {
        "DO NOTHING"
    };
    let done = sqlx::query(sqlx::AssertSqlSafe(format!(
        "INSERT INTO {table} (id, at, origin, purpose, scope, counted, org_id, project_id, topic_id,
                thread_id, segment_id, turn_id, provider_id, entry_id, user_id, agent_id, harness_kind,
                environment_id, environment_kind, arrangement, model, effort, tier, input_tokens,
                cached_input_tokens, cache_write_tokens, output_tokens, reasoning_tokens, total_tokens,
                context_tokens, request_ref, rate, rate_source, credits, first_token_ms, generation_ms,
                request_ms, outcome, access_program, measurement_states, streamed_output_tokens, speed_milli)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19, $20, $21,
                 $22, $23, $24, $25, $26, $27, $28, $29, $30, $31, $32, $33, $34::text::numeric,
                 $35, $36, $37, $38, $39, $40, $41, $42)
         ON CONFLICT (id) {conflict}"
    )))
    .bind(event.id)
    .bind(event.at)
    .bind(event.origin)
    .bind(purpose)
    .bind(event.scope)
    .bind(event.counted)
    .bind(attribution.org_id)
    .bind(attribution.project_id)
    .bind(attribution.topic_id)
    .bind(attribution.thread_id)
    .bind(attribution.segment_id)
    .bind(attribution.turn_id)
    .bind(attribution.provider_id)
    .bind(attribution.entry_id)
    .bind(attribution.user_id)
    .bind(attribution.agent_id)
    .bind(attribution.harness_kind.as_deref())
    .bind(attribution.environment_id)
    .bind(attribution.environment_kind.as_deref())
    .bind(attribution.arrangement.as_deref())
    .bind(event.model)
    .bind(attribution.effort.as_deref())
    .bind(event.tier)
    .bind(event.tokens.input)
    .bind(event.tokens.cached_input)
    .bind(event.tokens.cache_write)
    .bind(event.tokens.output)
    .bind(event.tokens.reasoning)
    .bind(event.tokens.total)
    .bind(event.context_tokens)
    .bind(event.request_ref)
    .bind(event.rate)
    .bind(event.rate_source)
    .bind(event.credits)
    .bind(timing.and_then(|t| t.first_token_ms.value()).copied().filter(|n| *n >= 0))
    .bind(timing.and_then(|t| t.generation_ms.value()).copied().filter(|n| *n >= 0))
    .bind(timing.and_then(|t| t.request_ms.value()).copied().filter(|n| *n >= 0))
    .bind(timing.and_then(|t| t.outcome.value()).map(String::as_str))
    .bind(program)
    .bind(serde_json::to_value(timing.cloned().unwrap_or_default()).unwrap_or_default())
    .bind(timing.and_then(|t| t.streamed_tokens.value()).copied())
    .bind(timing.and_then(|t| t.speed_milli.value()).copied())
    .execute(&mut *conn)
    .await?;
    let wrote = done.rows_affected() == 1;
    if !running {
        sqlx::query("DELETE FROM usage_running WHERE id=$1")
            .bind(event.id)
            .execute(&mut *conn)
            .await?;
    }
    if wrote && !running && (event.counted || timing.is_some()) {
        rollup::fold_event(conn, event.id).await?;
    }
    Ok(wrote)
}

/// What a model call Orch made for itself is put down to: a place, the provider that paid, and the person
/// who set the summarizer to ask it. It belongs to a topic, or to one of its threads, and to no turn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SummaryAttribution {
    /// The root org the work was in.
    pub org_id: Id,
    /// The project.
    pub project_id: Id,
    /// The topic.
    pub topic_id: Id,
    /// The thread, where the summary is of one; none for a topic's own.
    pub thread_id: Option<Id>,
    /// The provider whose key paid.
    pub provider_id: Id,
    /// The entry through which that person uses the provider.
    pub entry_id: Option<Id>,
    /// Whoever last wrote the summarizer's setting.
    pub user_id: Option<Id>,
}

/// A model call Orch made for itself, to write.
#[derive(Debug, Clone, PartialEq)]
pub struct NewSummary<'a> {
    /// Its identifier, chosen from what the call is: asked twice it is one row.
    pub id: Id,
    /// When it happened.
    pub at: DateTime<Utc>,
    /// What it is put down to.
    pub attribution: &'a SummaryAttribution,
    /// The model, by the name that was sent.
    pub model: &'a str,
    /// The service tier it ran with.
    pub tier: Option<&'a str>,
    /// What it used.
    pub tokens: Tokens,
    /// How large the prompt was.
    pub context_tokens: Option<i64>,
    /// The upstream's own name for the request.
    pub request_ref: Option<&'a str>,
    /// The rate it is counted at, copied.
    pub rate: Option<&'a Json>,
    /// Where the rate came from: see [`rate_source`].
    pub rate_source: Option<&'a str>,
    /// The credits it comes to, as a decimal number.
    pub credits: Option<&'a str>,
}

/// Writes a model call Orch made itself: one `request` row of the origin `orch` and the purpose `summary`,
/// which always counts, against the provider and the caps that are shared. The answer is whether this call
/// wrote it.
pub async fn record_summary(conn: &mut Conn, call: &NewSummary<'_>) -> Result<bool, DbError> {
    record_summary_measured(conn, call, None).await
}

/// Writes an Orch model-client call with its observed boundaries in the same transaction.
pub async fn record_summary_measured(
    conn: &mut Conn,
    call: &NewSummary<'_>,
    timing: Option<&orch_api_contract::usage::RequestTiming>,
) -> Result<bool, DbError> {
    record_own_measured(conn, purpose::SUMMARY, call, timing).await
}

/// Writes a model call Orch made itself under `purpose` ([`purpose::SUMMARY`] or [`purpose::EMBEDDINGS`]): one
/// `request` row of the origin `orch`, which always counts, against the provider and the caps that are shared,
/// with what was measured of it. The answer is whether this call wrote it.
pub async fn record_own_measured(
    conn: &mut Conn,
    purpose: &str,
    call: &NewSummary<'_>,
    timing: Option<&orch_api_contract::usage::RequestTiming>,
) -> Result<bool, DbError> {
    let place = call.attribution;
    let done = sqlx::query(
        "INSERT INTO usage_events (id, at, origin, purpose, scope, counted, org_id, project_id, topic_id,
                thread_id, provider_id, entry_id, user_id, model, tier, input_tokens, cached_input_tokens,
                cache_write_tokens, output_tokens, reasoning_tokens, total_tokens, context_tokens, request_ref,
                rate, rate_source, credits, first_token_ms, generation_ms, request_ms, outcome, measurement_states, streamed_output_tokens, speed_milli)
         VALUES ($1, $2, $3, $4, $5, true, $6, $7, $8, $9, $10, $11, $12, $13, $14, $15, $16, $17, $18, $19,
                 $20, $21, $22, $23, $24, $25::text::numeric, $26, $27, $28, $29, $30, $31, $32)
         ON CONFLICT (id) DO NOTHING",
    )
    .bind(call.id)
    .bind(call.at)
    .bind(origin::ORCH)
    .bind(purpose)
    .bind(scope::REQUEST)
    .bind(place.org_id)
    .bind(place.project_id)
    .bind(place.topic_id)
    .bind(place.thread_id)
    .bind(place.provider_id)
    .bind(place.entry_id)
    .bind(place.user_id)
    .bind(call.model)
    .bind(call.tier)
    .bind(call.tokens.input)
    .bind(call.tokens.cached_input)
    .bind(call.tokens.cache_write)
    .bind(call.tokens.output)
    .bind(call.tokens.reasoning)
    .bind(call.tokens.total)
    .bind(call.context_tokens)
    .bind(call.request_ref)
    .bind(call.rate)
    .bind(call.rate_source)
    .bind(call.credits)
    .bind(timing.and_then(|t| t.first_token_ms.value()).copied())
    .bind(timing.and_then(|t| t.generation_ms.value()).copied())
    .bind(timing.and_then(|t| t.request_ms.value()).copied())
    .bind(timing.and_then(|t| t.outcome.value()).map(String::as_str))
    .bind(serde_json::to_value(timing.cloned().unwrap_or_default()).unwrap_or_default())
    .bind(timing.and_then(|t| t.streamed_tokens.value()).copied())
    .bind(timing.and_then(|t| t.speed_milli.value()).copied())
    .execute(&mut *conn)
    .await?;
    let wrote = done.rows_affected() == 1;
    if wrote {
        rollup::fold_event(conn, call.id).await?;
    }
    Ok(wrote)
}

/// A row of the ledger as it is read back.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct EventRow {
    /// Server-only commit-ordered cursor; it is never exposed as a global activity counter.
    pub change_seq: i64,
    /// Output whose hidden reasoning can be separated.
    pub streamed_output_tokens: Option<i64>,
    /// Eligible measured tokens per second in thousandths.
    pub speed_milli: Option<i64>,
    /// Whether this is a provisional running observation.
    pub running: bool,
    /// Request sent to first output token, when measured.
    pub first_token_ms: Option<i64>,
    /// Time from first to last output token.
    pub generation_ms: Option<i64>,
    /// Whole model request time.
    pub request_ms: Option<i64>,
    /// Done or the stable error code.
    pub outcome: Option<String>,
    /// Access programme selected for the request.
    pub access_program: Option<String>,
    /// Four-state declarations and safe error words.
    pub measurement_states: Json,
    /// Its identifier.
    pub id: Id,
    /// When it happened.
    pub at: DateTime<Utc>,
    /// What it was for: see [`purpose`].
    pub purpose: String,
    /// Who reported it.
    pub origin: String,
    /// What it measures.
    pub scope: String,
    /// Whether it feeds the rollups.
    pub counted: bool,
    /// The root org.
    pub org_id: Id,
    /// The project.
    pub project_id: Id,
    /// The topic.
    pub topic_id: Id,
    /// The thread.
    pub thread_id: Option<Id>,
    /// The segment.
    pub segment_id: Option<Id>,
    /// The turn.
    pub turn_id: Option<Id>,
    /// The provider.
    pub provider_id: Id,
    /// The entry.
    pub entry_id: Option<Id>,
    /// The person it is put down to.
    pub user_id: Option<Id>,
    /// The agent profile.
    pub agent_id: Option<Id>,
    /// Its harness kind.
    pub harness_kind: Option<String>,
    /// The environment.
    pub environment_id: Option<Id>,
    /// `managed` or `connected`.
    pub environment_kind: Option<String>,
    /// The arrangement type.
    pub arrangement: Option<String>,
    /// The model, by the name that was sent.
    pub model: String,
    /// The effort.
    pub effort: Option<String>,
    /// The service tier.
    pub tier: Option<String>,
    /// Tokens of input.
    pub input_tokens: i64,
    /// Of them, read from a cache.
    pub cached_input_tokens: i64,
    /// Of them, written to a cache.
    pub cache_write_tokens: i64,
    /// Tokens of output.
    pub output_tokens: i64,
    /// Of them, reasoning.
    pub reasoning_tokens: i64,
    /// Input plus output.
    pub total_tokens: i64,
    /// How large the prompt was.
    pub context_tokens: Option<i64>,
    /// The upstream's name for the request.
    pub request_ref: Option<String>,
    /// The rate it was counted at.
    pub rate: Option<Json>,
    /// Where the rate came from.
    pub rate_source: Option<String>,
    /// The credits, as a decimal number.
    pub credits: Option<String>,
}

const EVENT_COLUMNS: &str = "change_seq, streamed_output_tokens, speed_milli, first_token_ms, generation_ms, request_ms, outcome, access_program, measurement_states,
        id, at, purpose, origin, scope, counted, org_id, project_id, topic_id, thread_id, segment_id,
        turn_id, provider_id, entry_id, user_id, agent_id, harness_kind, environment_id, environment_kind,
        arrangement, model, effort, tier, input_tokens, cached_input_tokens, cache_write_tokens, output_tokens,
        reasoning_tokens, total_tokens, context_tokens, request_ref, rate, rate_source, credits::text AS credits";

/// The rows of a turn, oldest first.
pub async fn events_of_turn(conn: &mut Conn, turn: Id) -> Result<Vec<EventRow>, DbError> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT false AS running, {EVENT_COLUMNS} FROM usage_events WHERE turn_id = $1 ORDER BY at, id"
    )))
    .bind(turn)
    .fetch_all(conn)
    .await?)
}

/// The tokens of the model requests of a turn that count, added up: what the forwarder saw the turn use.
pub async fn counted_request_totals(conn: &mut Conn, turn: Id) -> Result<Tokens, DbError> {
    counted_request_sum(conn, turn).await
}

/// Whether the ledger has the row `id`.
pub async fn event_exists(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    Ok(
        sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM usage_events WHERE id = $1)")
            .bind(id)
            .fetch_one(conn)
            .await?,
    )
}

/// Whether request-level adapter records already replace this turn's cumulative fallback.
pub async fn has_harness_requests(conn: &mut Conn, turn: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM usage_events WHERE turn_id=$1 AND origin='harness' AND scope='request' AND purpose='turn')")
        .bind(turn).fetch_one(conn).await?)
}

/// The numbers in their threads of the turns named by ledger rows, where the turn still exists.
pub async fn turn_ordinals(
    conn: &mut Conn,
    turns: &[Id],
) -> Result<std::collections::HashMap<Id, i64>, DbError> {
    let rows: Vec<(Id, i64)> =
        sqlx::query_as("SELECT id, ordinal::bigint FROM turns WHERE id = ANY($1)")
            .bind(turns)
            .fetch_all(conn)
            .await?;
    Ok(rows.into_iter().collect())
}

/// The sum of the counted request rows of a turn.
async fn counted_request_sum(conn: &mut Conn, turn: Id) -> Result<Tokens, DbError> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(sum(input_tokens), 0)::bigint AS input,
                COALESCE(sum(cached_input_tokens), 0)::bigint AS cached_input,
                COALESCE(sum(cache_write_tokens), 0)::bigint AS cache_write,
                COALESCE(sum(output_tokens), 0)::bigint AS output,
                COALESCE(sum(reasoning_tokens), 0)::bigint AS reasoning,
                COALESCE(sum(total_tokens), 0)::bigint AS total
           FROM usage_events WHERE turn_id = $1 AND scope = 'request' AND counted AND purpose = 'turn'",
    )
    .bind(turn)
    .fetch_one(conn)
    .await?)
}

/// The root org and nearest project of a summary's actual work scope; an org without a project uses itself.
/// The scope's row is held against a change while the transaction lasts, as for [`attribution_of_lease`].
pub async fn summary_place(conn: &mut Conn, topic: Id) -> Result<Option<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT n.org_id,COALESCE(
            CASE WHEN n.kind='topic' THEN n.parent_id WHEN n.kind='project' THEN n.id END,
            (SELECT p.id FROM nodes p WHERE p.id=ANY(n.path) AND p.kind='project' ORDER BY array_position(n.path,p.id) DESC LIMIT 1),
            n.org_id)
         FROM nodes n WHERE n.id=$1 AND n.kind IN ('org','project','topic') FOR SHARE OF n",
    )
    .bind(topic)
    .fetch_optional(conn)
    .await?)
}

/// The rows of the ledger a read asks for.
#[derive(Debug, Clone)]
pub struct Listing<'a> {
    /// The first moment, inclusive; absent for a revision-only or running read.
    pub from: Option<DateTime<Utc>>,
    /// The last moment, exclusive; absent for a revision-only read.
    pub until: Option<DateTime<Utc>>,
    /// What the rows are limited to.
    pub filter: &'a rollup::Filter,
    /// Only the rows that count, or only those that do not.
    pub counted: Option<bool>,
    /// The newest first, or the oldest.
    pub newest_first: bool,
    /// Where the page after the last one begins: the moment and the identifier of its last row.
    pub after: Option<(DateTime<Utc>, Id)>,
    /// The most rows to give.
    pub limit: i64,
}

/// A page of the rows of the ledger, in the order asked for, ties broken by identifier.
pub async fn list(conn: &mut Conn, listing: &Listing<'_>) -> Result<Vec<EventRow>, DbError> {
    list_live(conn, listing, false, false).await
}

/// Reads a cursor page, with provisional observations and canonical adapter requests when requested.
pub async fn list_live(
    conn: &mut Conn,
    listing: &Listing<'_>,
    include_running: bool,
    requests_only: bool,
) -> Result<Vec<EventRow>, DbError> {
    list_live_inner(conn, listing, include_running, requests_only, None, false).await
}

/// One consistent initial feed window, without advancing past a concurrent observation it did not read.
pub async fn live_snapshot(
    conn: &mut Conn,
    listing: &Listing<'_>,
    include_running: bool,
    through: i64,
) -> Result<Vec<EventRow>, DbError> {
    list_live_inner(
        conn,
        listing,
        include_running,
        true,
        Some((0, through)),
        false,
    )
    .await
}

/// Changed rows only, in revision order; a running update replaces the same identity.
pub async fn live_changes(
    conn: &mut Conn,
    filter: &rollup::Filter,
    after: i64,
    through: i64,
    limit: i64,
) -> Result<Vec<EventRow>, DbError> {
    list_live_inner(
        conn,
        &Listing {
            from: None,
            until: None,
            filter,
            counted: None,
            newest_first: false,
            after: None,
            limit,
        },
        true,
        true,
        Some((after, through)),
        true,
    )
    .await
}

/// A cheap revision read, shared by the coordinator and initial subscriptions.
pub async fn revision(conn: &mut Conn) -> Result<i64, DbError> {
    Ok(
        sqlx::query_scalar("SELECT revision FROM usage_change_state WHERE singleton")
            .fetch_one(conn)
            .await?,
    )
}

async fn list_live_inner(
    conn: &mut Conn,
    listing: &Listing<'_>,
    include_running: bool,
    requests_only: bool,
    changes: Option<(i64, i64)>,
    order_changes: bool,
) -> Result<Vec<EventRow>, DbError> {
    let source = if include_running {
        "(SELECT false AS running,* FROM usage_events UNION ALL SELECT true AS running,* FROM usage_running r WHERE EXISTS(SELECT 1 FROM turns t WHERE t.id=r.turn_id AND t.state IN ('starting','running')))"
    } else {
        "(SELECT false AS running,* FROM usage_events)"
    };
    let mut query = sqlx::QueryBuilder::<sqlx::Postgres>::new(format!(
        "SELECT running, {EVENT_COLUMNS} FROM {source} d WHERE true"
    ));
    // Unbounded reads omit predicates. Chrono's finite extrema are outside PostgreSQL's
    // timestamp range; binding one as an infinity used to end the whole live connection.
    if let Some(from) = listing.from {
        query.push(" AND d.at >= ").push_bind(from);
    }
    if let Some(until) = listing.until {
        query.push(" AND d.at < ").push_bind(until);
    }
    if let Some((after, through)) = changes {
        query
            .push(" AND d.change_seq > ")
            .push_bind(after)
            .push(" AND d.change_seq <= ")
            .push_bind(through);
    }
    if let Some(counted) = listing.counted {
        query.push(" AND d.counted = ").push_bind(counted);
    }
    let mut filter = listing.filter.clone();
    if filter.outcome == Some(orch_api_contract::usage::UsageOutcomeFilter::Running) {
        filter.outcome = None;
        query.push(" AND d.running");
    }
    rollup::push_filter(&mut query, &filter);
    if requests_only {
        // Forwarder rows remain the token ledger, while adapter rows alone define
        // request performance for every provider kind. Legacy turns remain visible.
        query.push(" AND (d.running OR d.origin='orch' OR (d.origin='harness' AND (d.scope='request' OR NOT EXISTS (SELECT 1 FROM usage_events r WHERE r.turn_id=d.turn_id AND r.origin='harness' AND r.scope='request'))))");
    }
    if let Some((at, id)) = listing.after {
        query
            .push(if listing.newest_first {
                " AND (d.at, d.id) < ("
            } else {
                " AND (d.at, d.id) > ("
            })
            .push_bind(at)
            .push(", ")
            .push_bind(id)
            .push(")");
    }
    query.push(if order_changes {
        " ORDER BY d.change_seq, d.id LIMIT "
    } else if listing.newest_first {
        " ORDER BY d.at DESC, d.id DESC LIMIT "
    } else {
        " ORDER BY d.at, d.id LIMIT "
    });
    query.push_bind(listing.limit);
    Ok(query.build_query_as::<EventRow>().fetch_all(conn).await?)
}

/// Ends observations abandoned by a harness without inventing boundaries or counts.
/// Called inside the turn-ending transaction; the immutable id keeps late reports idempotent.
pub async fn finish_running(
    conn: &mut Conn,
    turn: Id,
    outcome: &str,
    error: Option<&Json>,
) -> Result<Vec<EventRow>, DbError> {
    use orch_api_contract::usage::UsageMeasurement;
    // The change numbers are taken at commit by orch_commit_work: no counter is locked while this runs.
    let rows=sqlx::query_as::<_,EventRow>(sqlx::AssertSqlSafe(format!("SELECT true AS running,{EVENT_COLUMNS} FROM usage_running WHERE turn_id=$1 ORDER BY at,id FOR UPDATE")))
        .bind(turn).fetch_all(&mut *conn).await?;
    let columns = EVENT_COLUMNS.replace("credits::text AS credits", "credits");
    let mut written = Vec::new();
    for mut row in rows {
        let mut timing = row.timing();
        timing.outcome = UsageMeasurement::Told {
            value: outcome.to_owned(),
        };
        timing.error_message = error
            .and_then(|error| error.get("message"))
            .and_then(Json::as_str)
            .map(str::to_owned);
        timing.vendor_words = error
            .and_then(|error| error.get("said"))
            .and_then(Json::as_str)
            .map(str::to_owned);
        for member in [
            &mut timing.first_token_ms,
            &mut timing.generation_ms,
            &mut timing.request_ms,
            &mut timing.speed_milli,
            &mut timing.streamed_tokens,
        ] {
            if matches!(member, UsageMeasurement::Unread) {
                *member = UsageMeasurement::Unsupported;
            }
        }
        let states = serde_json::to_value(&timing).unwrap_or_default();
        sqlx::query("UPDATE usage_running SET outcome=$2,measurement_states=$3 WHERE id=$1")
            .bind(row.id)
            .bind(outcome)
            .bind(&states)
            .execute(&mut *conn)
            .await?;
        let done=sqlx::query(sqlx::AssertSqlSafe(format!("INSERT INTO usage_events({columns}) SELECT {columns} FROM usage_running WHERE id=$1 ON CONFLICT(id) DO NOTHING")))
            .bind(row.id).execute(&mut *conn).await?;
        sqlx::query("DELETE FROM usage_running WHERE id=$1")
            .bind(row.id)
            .execute(&mut *conn)
            .await?;
        if done.rows_affected() == 1 {
            rollup::fold_event(conn, row.id).await?;
            row.running = false;
            row.outcome = Some(outcome.to_owned());
            row.measurement_states = states;
            written.push(row);
        }
    }
    Ok(written)
}

/// A bounded repair of observations whose turn ended before this engine process could close them.
pub async fn stale_running_turns(
    conn: &mut Conn,
    limit: i64,
) -> Result<Vec<(Id, Option<String>, Option<Json>)>, DbError> {
    Ok(sqlx::query_as("SELECT r.turn_id,t.state,t.error FROM usage_running r LEFT JOIN turns t ON t.id=r.turn_id WHERE r.turn_id IS NOT NULL AND (t.id IS NULL OR t.state NOT IN ('starting','running')) GROUP BY r.turn_id,t.state,t.error ORDER BY min(r.at) LIMIT $1")
        .bind(limit).fetch_all(conn).await?)
}

impl EventRow {
    /// Restores declarations and numeric measurements without turning missing values into zero.
    pub fn timing(&self) -> orch_api_contract::usage::RequestTiming {
        use orch_api_contract::usage::{RequestTiming, UsageMeasurement};
        let mut timing: RequestTiming =
            serde_json::from_value(self.measurement_states.clone()).unwrap_or_default();
        if let Some(value) = self.streamed_output_tokens {
            timing.streamed_tokens = UsageMeasurement::Told { value };
        }
        if let Some(value) = self.speed_milli {
            timing.speed_milli = UsageMeasurement::Told { value };
        }
        if let Some(value) = self.first_token_ms {
            timing.first_token_ms = UsageMeasurement::Told { value };
        }
        if let Some(value) = self.generation_ms {
            timing.generation_ms = UsageMeasurement::Told { value };
        }
        if let Some(value) = self.request_ms {
            timing.request_ms = UsageMeasurement::Told { value };
        }
        if let Some(value) = &self.outcome {
            timing.outcome = UsageMeasurement::Told {
                value: value.clone(),
            };
        }
        if self.running {
            timing.outcome = UsageMeasurement::Unread;
        }
        timing
    }
}

/// The rate catalogue in force, as it was loaded.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct CatalogueRow {
    /// The catalogue, as the file gave it.
    pub doc: Json,
    /// When its figures were last read from their sources.
    pub as_of: chrono::NaiveDate,
    /// `shipped`, `uploaded` or `fetched`.
    pub origin: String,
    /// Where it was fetched from.
    pub source: Option<String>,
    /// When it was loaded.
    pub loaded_at: DateTime<Utc>,
    /// Who loaded it; none when a start or a schedule did.
    pub loaded_by: Option<Id>,
}

/// The rate catalogue in force; none while none was loaded.
pub async fn catalogue_in_force(conn: &mut Conn) -> Result<Option<CatalogueRow>, DbError> {
    Ok(sqlx::query_as(
        "SELECT doc, as_of, origin, source, loaded_at, loaded_by FROM rate_catalogue WHERE id",
    )
    .fetch_optional(conn)
    .await?)
}

/// Thread and current-turn credits from the exact rows already counted by the ledger.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ThreadCredits {
    /// All counted credits, unknown when no row had a rate.
    pub credits: Option<String>,
    /// The catalogue-estimated share.
    pub estimated_credits: String,
    /// Counted tokens with no rate.
    pub unrated_tokens: i64,
    /// Current-turn counted credits.
    pub turn_credits: Option<String>,
    /// Current-turn catalogue-estimated share.
    pub turn_estimated_credits: String,
    /// Current-turn tokens with no rate.
    pub turn_unrated_tokens: i64,
}

/// One read supplies both live cost items without a query for each turn or browser tick. The thread's figures are
/// the sums of its daily rollup rows, which hold the same counted rows' sums, so a thread view costs its days and
/// not every request it ever made; the turn's are read from its own rows.
pub async fn status_credits(
    conn: &mut Conn,
    thread: Id,
    turn: Option<Id>,
) -> Result<ThreadCredits, DbError> {
    Ok(sqlx::query_as(
        "WITH days AS (
           SELECT CASE WHEN sum(rated_rows) > 0 THEN sum(credits) END AS credits,
                  COALESCE(sum(estimated_credits),0) AS estimated_credits,
                  COALESCE(sum(unrated_tokens),0)::bigint AS unrated_tokens
             FROM usage_daily WHERE thread_id=$1),
         turn AS (
           SELECT sum(credits) AS credits,
                  COALESCE(sum(credits) FILTER (WHERE rate_source='catalogue'),0) AS estimated_credits,
                  COALESCE(sum(total_tokens) FILTER (WHERE credits IS NULL),0)::bigint AS unrated_tokens
             FROM usage_events WHERE turn_id=$2 AND thread_id=$1 AND counted)
         SELECT days.credits::text AS credits, days.estimated_credits::text AS estimated_credits,
                days.unrated_tokens, turn.credits::text AS turn_credits,
                turn.estimated_credits::text AS turn_estimated_credits, turn.unrated_tokens AS turn_unrated_tokens
           FROM days, turn",
    ).bind(thread).bind(turn).fetch_one(conn).await?)
}
