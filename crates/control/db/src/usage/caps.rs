//! Usage caps, period sums, turns running at once counted under a lock, sent notices, and each time a turn was let past one.
//!
//! A cap is a row that says how much may be used, of what, by whom, where and
//! in which period, or how many turns may run at once (`usage_caps`). This
//! module keeps the rows (made once for each identity, changed against the
//! revision that was read, removed), finds the enabled ones that could cover a
//! turn, and answers the questions a cap is judged by: what the people it
//! covers used in its period (read from the daily rollups, a model at a time,
//! because which models a cap names is a rule of the engine), which covered
//! turns are running now and what they reported so far, and what waits for room
//! in it. The rest of what a cap does with these answers (who it covers, which
//! of several applies to a person, whether it has room) is the engine's. What this module does decide is what
//! the rows say: the service tiers a cap names are part of what makes two caps the same and are asked of the
//! rollups, whose rows keep the tier; the models it names, which may be groups of a provider's models, are asked
//! of the engine's rule, so the rows of a period come back by provider and model.
//!
//! Three small tables hang on a cap. Notices say that a warning or the word
//! that a cap was reached went out for a person and a period, once
//! ([`note_once`]). Bypasses say that a turn was let past a limit, by whom and
//! what the limit showed then ([`record_bypass`]). The counting of turns that
//! run at once is made exact by a lock on the caps concerned ([`lock_for_counting`]).

use crate::{Conn, DbError};
use chrono::{DateTime, NaiveDate, Utc};
use orch_types::{Id, Json};
use sqlx::{Postgres, QueryBuilder};

/// What a cap counts: the words of the `meter` column.
pub mod meter {
    /// Tokens.
    pub const TOKENS: &str = "tokens";
    /// Credits.
    pub const CREDITS: &str = "credits";
    /// Turns running at the same time.
    pub const RUNNING: &str = "running";
}

/// Whom a cap covers: the words of the `subject_kind` column.
pub mod subject {
    /// Members by domain of one org.
    pub const MEMBERS: &str = "members";
    /// Everybody.
    pub const EVERYONE: &str = "everyone";
    /// One person.
    pub const USER: &str = "user";
    /// A group.
    pub const GROUP: &str = "group";
    /// A role.
    pub const ROLE: &str = "role";
}

/// Whether an amount is shared or given to each person: the words of the `split` column.
pub mod split {
    /// One amount for all of them.
    pub const SHARED: &str = "shared";
    /// That amount for each person.
    pub const EACH: &str = "each";
}

/// The period a cap counts in: the words of the `period` column.
pub mod period {
    /// A day.
    pub const DAY: &str = "day";
    /// A week from Monday.
    pub const WEEK: &str = "week";
    /// A calendar month.
    pub const MONTH: &str = "month";
    /// Since `counted_from`.
    pub const TOTAL: &str = "total";
}

/// What happens to a turn under way when a cap is reached: the words of the `at_limit` column.
pub mod at_limit {
    /// The turn is stopped.
    pub const STOP: &str = "stop";
    /// The turn ends by itself, and no further one starts.
    pub const FINISH_TURN: &str = "finish_turn";
}

/// What a cap does with use it cannot count: the words of the `uncounted` column.
pub mod uncounted {
    /// Work goes on.
    pub const ALLOW: &str = "allow";
    /// Work is held back.
    pub const BLOCK: &str = "block";
}

/// The level of a notice: the words of the `level` column of `usage_cap_notices`.
pub mod level {
    /// The warning percent was passed.
    pub const WARNING: &str = "warning";
    /// The cap was reached.
    pub const REACHED: &str = "reached";
}

/// The columns of a cap.
const CAP_COLUMNS: &str = "id, node_id, provider_id, subject_kind, subject_id, subject_role, split, models,
        tiers, harness_kinds, meter, period, counted_from, amount::text AS amount, at_limit, warn_percent, uncounted, note,
        enabled, revision, created_by, created_at, updated_at";

/// Enabled definitions for the system's operational cap gauges; no access decision is made here.
pub async fn enabled_definitions(conn: &mut Conn) -> Result<Vec<CapRow>, DbError> {
    let sql = format!("SELECT {CAP_COLUMNS} FROM usage_caps WHERE enabled ORDER BY id");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await?)
}

/// A cap as it is kept.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct CapRow {
    /// Its identifier.
    pub id: Id,
    /// The node it covers the subtree of; none for anywhere.
    pub node_id: Option<Id>,
    /// The provider it covers; none for any.
    pub provider_id: Option<Id>,
    /// `everyone`, `user`, `group` or `role`.
    pub subject_kind: String,
    /// The person or the group.
    pub subject_id: Option<Id>,
    /// The role, for the subject `role`.
    pub subject_role: Option<String>,
    /// `shared` or `each`.
    pub split: String,
    /// The patterns that name the models it covers, among them `@<group>`; none for every model.
    pub models: Vec<String>,
    /// The service tiers it covers; none for whatever tier a turn runs with.
    pub tiers: Vec<String>,
    /// The harness kinds it covers; none for every agent.
    pub harness_kinds: Vec<String>,
    /// `tokens`, `credits` or `running`.
    pub meter: String,
    /// `day`, `week`, `month` or `total`; none for `running`.
    pub period: Option<String>,
    /// For `total`: the day counting begins; none for the beginning.
    pub counted_from: Option<NaiveDate>,
    /// The amount, as a decimal number; none for no limit.
    pub amount: Option<String>,
    /// `stop` or `finish_turn`.
    pub at_limit: String,
    /// The percent at which a warning is given.
    pub warn_percent: Option<i16>,
    /// `allow` or `block`.
    pub uncounted: String,
    /// A note of whoever set it.
    pub note: String,
    /// Whether it is in force.
    pub enabled: bool,
    /// How many times it was changed, from one.
    pub revision: i64,
    /// Who made it.
    pub created_by: Id,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was last changed.
    pub updated_at: DateTime<Utc>,
}

impl CapRow {
    /// Whether the amount is counted for each person apart: it is given to each, or the cap names one.
    #[must_use]
    pub fn per_person(&self) -> bool {
        self.split == split::EACH || self.subject_kind == subject::USER
    }
}

/// The models, tiers or harness kinds of a cap as they are kept: without space around, each once, in order, so
/// that two caps that name the same ones are the same cap.
#[must_use]
pub fn normalised(words: &[String]) -> Vec<String> {
    let mut kept: Vec<String> = words
        .iter()
        .map(|word| word.trim().to_owned())
        .filter(|word| !word.is_empty())
        .collect();
    kept.sort();
    kept.dedup();
    kept
}

/// A cap to make.
#[derive(Debug, Clone)]
pub struct NewCap<'a> {
    /// Its identifier.
    pub id: Id,
    /// The node it is defined on.
    pub node_id: Option<Id>,
    /// The provider it is defined on.
    pub provider_id: Option<Id>,
    /// Whom it covers: see [`subject`].
    pub subject_kind: &'a str,
    /// The person or the group.
    pub subject_id: Option<Id>,
    /// The role.
    pub subject_role: Option<&'a str>,
    /// See [`split`].
    pub split: &'a str,
    /// The models, as [`normalised`].
    pub models: &'a [String],
    /// The service tiers, as [`normalised`].
    pub tiers: &'a [String],
    /// The harness kinds, as [`normalised`].
    pub harness_kinds: &'a [String],
    /// See [`meter`].
    pub meter: &'a str,
    /// See [`period`].
    pub period: Option<&'a str>,
    /// For `total`: where counting begins.
    pub counted_from: Option<NaiveDate>,
    /// The amount, as a decimal number.
    pub amount: Option<&'a str>,
    /// See [`at_limit`].
    pub at_limit: &'a str,
    /// The percent at which a warning is given.
    pub warn_percent: Option<i16>,
    /// See [`uncounted`].
    pub uncounted: &'a str,
    /// A note.
    pub note: &'a str,
    /// Who makes it.
    pub created_by: Id,
    /// When.
    pub at: DateTime<Utc>,
}

/// The condition that says a cap is the same as another by what it is about.
const SAME_IDENTITY: &str = "node_id IS NOT DISTINCT FROM $1::uuid AND provider_id IS NOT DISTINCT FROM $2::uuid
        AND subject_kind = $3 AND subject_id IS NOT DISTINCT FROM $4::uuid
        AND subject_role IS NOT DISTINCT FROM $5::text AND split = $6 AND models = $7 AND harness_kinds = $8
        AND meter = $9 AND period IS NOT DISTINCT FROM $10::text AND tiers = $11";

/// Makes a cap, unless one of the same node, provider, subject, split, models, tiers, harness kinds, meter
/// and period is there: then nothing is made and the answer is none. Two requests that make the same cap at once
/// make one.
pub async fn insert(conn: &mut Conn, new: &NewCap<'_>) -> Result<Option<CapRow>, DbError> {
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!(
            "usage_cap:{:?}:{:?}:{}:{:?}:{:?}:{}:{:?}:{:?}:{:?}:{}:{:?}",
            new.node_id,
            new.provider_id,
            new.subject_kind,
            new.subject_id,
            new.subject_role,
            new.split,
            new.models,
            new.tiers,
            new.harness_kinds,
            new.meter,
            new.period
        ))
        .execute(&mut *conn)
        .await?;
    let sql = format!("SELECT id FROM usage_caps WHERE {SAME_IDENTITY}");
    let existing: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
        .bind(new.node_id)
        .bind(new.provider_id)
        .bind(new.subject_kind)
        .bind(new.subject_id)
        .bind(new.subject_role)
        .bind(new.split)
        .bind(new.models)
        .bind(new.harness_kinds)
        .bind(new.meter)
        .bind(new.period)
        .bind(new.tiers)
        .fetch_optional(&mut *conn)
        .await?;
    if existing.is_some() {
        return Ok(None);
    }
    let sql = format!(
        "INSERT INTO usage_caps (id, node_id, provider_id, subject_kind, subject_id, subject_role, split, models,
                harness_kinds, meter, period, counted_from, amount, at_limit, warn_percent, uncounted, note, created_by,
                created_at, updated_at, tiers)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13::text::numeric, $14, $15, $16, $17, $18, $19, $19,
                 $20)
         RETURNING {CAP_COLUMNS}"
    );
    Ok(Some(
        sqlx::query_as(sqlx::AssertSqlSafe(sql))
            .bind(new.id)
            .bind(new.node_id)
            .bind(new.provider_id)
            .bind(new.subject_kind)
            .bind(new.subject_id)
            .bind(new.subject_role)
            .bind(new.split)
            .bind(new.models)
            .bind(new.harness_kinds)
            .bind(new.meter)
            .bind(new.period)
            .bind(new.counted_from)
            .bind(new.amount)
            .bind(new.at_limit)
            .bind(new.warn_percent)
            .bind(new.uncounted)
            .bind(new.note)
            .bind(new.created_by)
            .bind(new.at)
            .bind(new.tiers)
            .fetch_one(conn)
            .await?,
    ))
}

/// The cap `id`.
pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<CapRow>, DbError> {
    let sql = format!("SELECT {CAP_COLUMNS} FROM usage_caps WHERE id = $1");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(conn)
        .await?)
}

/// What is changed in a cap. A member that is none is left alone; one that is `Some(None)` is cleared.
#[derive(Debug, Clone, Default)]
pub struct Change<'a> {
    /// New model patterns, as [`normalised`].
    pub models: Option<&'a [String]>,
    /// New service tiers, as [`normalised`].
    pub tiers: Option<&'a [String]>,
    /// New harness kinds, as [`normalised`].
    pub harness_kinds: Option<&'a [String]>,
    /// A new day to count from.
    pub counted_from: Option<Option<NaiveDate>>,
    /// A new amount.
    pub amount: Option<Option<&'a str>>,
    /// What happens to a turn under way.
    pub at_limit: Option<&'a str>,
    /// A new warning percent.
    pub warn_percent: Option<Option<i16>>,
    /// What it does with use it cannot count.
    pub uncounted: Option<&'a str>,
    /// A new note.
    pub note: Option<&'a str>,
    /// Whether it is in force.
    pub enabled: Option<bool>,
}

/// What came of changing a cap.
#[derive(Debug, Clone, PartialEq)]
pub enum Updated {
    /// It was changed; this is how it stands.
    Done(Box<CapRow>),
    /// It was changed by someone else since the revision that was read; this is how it stands.
    Stale(Box<CapRow>),
    /// With the models, tiers or harness kinds it would have, it would be the same as another cap.
    Exists,
    /// There is no such cap.
    Gone,
}

/// Changes a cap that is at the revision that was read.
pub async fn update(
    conn: &mut Conn,
    id: Id,
    expected_revision: i64,
    change: &Change<'_>,
    at: DateTime<Utc>,
) -> Result<Updated, DbError> {
    let sql = format!("SELECT {CAP_COLUMNS} FROM usage_caps WHERE id = $1 FOR UPDATE");
    let current: Option<CapRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(&mut *conn)
        .await?;
    let Some(current) = current else {
        return Ok(Updated::Gone);
    };
    if current.revision != expected_revision {
        return Ok(Updated::Stale(Box::new(current)));
    }
    let models = change.models.map_or(current.models.as_slice(), |each| each);
    let kinds = change
        .harness_kinds
        .map_or(current.harness_kinds.as_slice(), |each| each);
    let tiers = change.tiers.map_or(current.tiers.as_slice(), |each| each);
    if models != current.models.as_slice()
        || kinds != current.harness_kinds.as_slice()
        || tiers != current.tiers.as_slice()
    {
        let sql = format!("SELECT id FROM usage_caps WHERE id <> $12 AND {SAME_IDENTITY}");
        let same: Option<Id> = sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(current.node_id)
            .bind(current.provider_id)
            .bind(&current.subject_kind)
            .bind(current.subject_id)
            .bind(&current.subject_role)
            .bind(&current.split)
            .bind(models)
            .bind(kinds)
            .bind(&current.meter)
            .bind(&current.period)
            .bind(tiers)
            .bind(id)
            .fetch_optional(&mut *conn)
            .await?;
        if same.is_some() {
            return Ok(Updated::Exists);
        }
    }
    let mut query = QueryBuilder::<Postgres>::new(
        "UPDATE usage_caps SET revision = revision + 1, updated_at = ",
    );
    query.push_bind(at);
    if let Some(models) = change.models {
        query.push(", models = ").push_bind(models.to_vec());
    }
    if let Some(tiers) = change.tiers {
        query.push(", tiers = ").push_bind(tiers.to_vec());
    }
    if let Some(kinds) = change.harness_kinds {
        query.push(", harness_kinds = ").push_bind(kinds.to_vec());
    }
    if let Some(from) = change.counted_from {
        query.push(", counted_from = ").push_bind(from);
    }
    if let Some(amount) = change.amount {
        query
            .push(", amount = ")
            .push_bind(amount.map(str::to_owned))
            .push("::text::numeric");
    }
    if let Some(word) = change.at_limit {
        query.push(", at_limit = ").push_bind(word.to_owned());
    }
    if let Some(percent) = change.warn_percent {
        query.push(", warn_percent = ").push_bind(percent);
    }
    if let Some(word) = change.uncounted {
        query.push(", uncounted = ").push_bind(word.to_owned());
    }
    if let Some(note) = change.note {
        query.push(", note = ").push_bind(note.to_owned());
    }
    if let Some(enabled) = change.enabled {
        query.push(", enabled = ").push_bind(enabled);
    }
    query
        .push(" WHERE id = ")
        .push_bind(id)
        .push(format!(" RETURNING {CAP_COLUMNS}"));
    let changed: CapRow = query.build_query_as().fetch_one(conn).await?;
    Ok(Updated::Done(Box::new(changed)))
}

/// Removes a cap. Its notices go with it; the record of the turns let past it stays, without it. The answer
/// says whether there was one.
pub async fn delete(conn: &mut Conn, id: Id) -> Result<bool, DbError> {
    let done = sqlx::query("DELETE FROM usage_caps WHERE id = $1")
        .bind(id)
        .execute(conn)
        .await?;
    Ok(done.rows_affected() == 1)
}

/// Where a cap is defined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DefinedOn {
    /// On a node: it names that node and no provider.
    Node(Id),
    /// On a provider: it names that provider.
    Provider(Id),
    /// Nowhere: the system's.
    System,
}

/// The caps defined in a place, enabled or not, the oldest first.
pub async fn defined_on(conn: &mut Conn, place: DefinedOn) -> Result<Vec<CapRow>, DbError> {
    let condition = match place {
        DefinedOn::Node(_) => "node_id = $1 AND provider_id IS NULL",
        DefinedOn::Provider(_) => "provider_id = $1",
        DefinedOn::System => "node_id IS NULL AND provider_id IS NULL AND $1::uuid IS NULL",
    };
    let sql =
        format!("SELECT {CAP_COLUMNS} FROM usage_caps WHERE {condition} ORDER BY created_at, id");
    let key = match place {
        DefinedOn::Node(id) | DefinedOn::Provider(id) => Some(id),
        DefinedOn::System => None,
    };
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(key)
        .fetch_all(conn)
        .await?)
}

/// Enabled caps defined on a whole provider list in one statement.
pub async fn on_providers(conn: &mut Conn, providers: &[Id]) -> Result<Vec<CapRow>, DbError> {
    let sql = format!(
        "SELECT {CAP_COLUMNS} FROM usage_caps WHERE enabled AND provider_id = ANY($1) ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(providers)
        .fetch_all(conn)
        .await?)
}

/// The enabled caps that could cover work at a place: those that name no node or a node on the way down to
/// it. Which provider, model, agent or person they cover is asked of each.
pub async fn on_the_way_to(conn: &mut Conn, path: &[Id]) -> Result<Vec<CapRow>, DbError> {
    let sql = format!(
        "SELECT {CAP_COLUMNS} FROM usage_caps
          WHERE enabled AND (node_id IS NULL OR node_id = ANY($1)) ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(path)
        .fetch_all(conn)
        .await?)
}

/// The service tier a question about the caps is asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TierAsked<'a> {
    /// A turn at the ordinary tier: a cap that names tiers does not cover it.
    Ordinary,
    /// A turn at this tier.
    Is(&'a str),
    /// Whatever tier: the caps that could cover a turn at some tier or other.
    Any,
}

impl<'a> TierAsked<'a> {
    /// The tier of a turn, none being the ordinary one.
    #[must_use]
    pub fn of_turn(tier: Option<&'a str>) -> Self {
        tier.map_or(Self::Ordinary, Self::Is)
    }
}

/// The enabled caps that cover a turn of a thread whose way down from the root org is `path`, on `provider`,
/// at the service tier `tier`, by an agent of `harness_kind`, put down to `person`, as far as the rows say: the node, the provider, the tier, the harness kind and the subject,
/// where a role is left to whoever can place the person, since the role they hold is not on the cap's row.
/// The models a cap names are asked of the engine's rule for patterns. Caps with no limit are among them: for
/// a cap counted per person they decide which cap applies.
pub async fn covering(
    conn: &mut Conn,
    path: &[Id],
    provider: Id,
    tier: TierAsked<'_>,
    harness_kind: Option<&str>,
    person: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Vec<CapRow>, DbError> {
    let sql = format!(
        "SELECT {CAP_COLUMNS} FROM usage_caps c
          WHERE c.enabled
            AND (c.node_id IS NULL OR c.node_id = ANY($1))
            AND (c.provider_id IS NULL OR c.provider_id = $2)
            AND (cardinality(c.harness_kinds) = 0 OR ($3::text IS NOT NULL AND $3 = ANY(c.harness_kinds)))
            AND ($6 OR cardinality(c.tiers) = 0 OR ($5::text IS NOT NULL AND $5 = ANY(c.tiers)))
            AND (c.subject_kind = 'everyone'
                 OR (c.subject_kind = 'role' AND $4::uuid IS NOT NULL AND (c.subject_id IS NULL OR EXISTS (
                     SELECT 1 FROM access_holders($4, $7, true) h WHERE h.kind IN ('role','visible') AND h.id = c.subject_id
                       AND (c.subject_role <> 'viewer' OR h.kind = 'visible')
                       AND array_position(ARRAY['viewer','interactor','developer','admin'], h.role)
                           >= array_position(ARRAY['viewer','interactor','developer','admin'], c.subject_role))))
                 OR (c.subject_kind = 'members' AND $4::uuid IS NOT NULL AND EXISTS (
                     SELECT 1 FROM access_holders($4, $7, false) h WHERE h.kind = 'members' AND h.id = c.subject_id))
                 OR (c.subject_kind = 'user' AND c.subject_id = $4)
                 OR (c.subject_kind = 'group' AND $4::uuid IS NOT NULL AND EXISTS (
                       SELECT 1 FROM access_holders($4, $7, true) h WHERE h.kind = 'group' AND h.id = c.subject_id)))
          ORDER BY c.created_at, c.id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(path)
        .bind(provider)
        .bind(harness_kind)
        .bind(person)
        .bind(match tier {
            TierAsked::Is(name) => Some(name),
            TierAsked::Ordinary | TierAsked::Any => None,
        })
        .bind(tier == TierAsked::Any)
        .bind(at)
        .fetch_all(conn)
        .await?)
}

/// Whether a person is in a group.
pub async fn in_group(
    conn: &mut Conn,
    group: Id,
    person: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM access_holders($2, $3, true) WHERE kind = 'group' AND id = $1)",
    )
    .bind(group)
    .bind(person)
    .bind(at)
    .fetch_one(conn)
    .await?)
}

// ---------------------------------------------------------------- what was used

/// What the people a cap covers used of one model of one provider in its period.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Used {
    /// The provider it ran on: a group of models is a group of that provider's entries.
    pub provider_id: Id,
    /// The model, by its routing name.
    pub model: String,
    /// Tokens, input plus output.
    pub total_tokens: i64,
    /// The credits of the rows that had a rate.
    pub credits: String,
    /// The tokens of the rows that had none.
    pub unrated_tokens: i64,
}

/// What the rows of the daily rollup that a cap covers add up to since `since` (the beginning where none),
/// one model of one provider at a time, the rows at a tier the cap does not name left out. For a cap counted
/// per person only that person's own use of turns is added, and a summary is left out of it. The models a cap
/// names are not asked here.
pub async fn used(
    conn: &mut Conn,
    cap: &CapRow,
    person: Option<Id>,
    since: Option<NaiveDate>,
    at: DateTime<Utc>,
) -> Result<Vec<Used>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT d.provider_id, d.model, COALESCE(sum(d.total_tokens), 0)::bigint AS total_tokens,
                COALESCE(sum(d.credits), 0)::text AS credits,
                COALESCE(sum(d.unrated_tokens), 0)::bigint AS unrated_tokens
           FROM usage_daily d WHERE true",
    );
    if let Some(since) = since {
        query.push(" AND d.day >= ").push_bind(since);
    }
    if let Some(node) = cap.node_id {
        query
            .push(
                " AND EXISTS (SELECT 1 FROM nodes pn
                               WHERE pn.id = COALESCE(d.thread_id, d.topic_id) AND ",
            )
            .push_bind(node)
            .push(" = ANY(pn.path))");
    }
    if let Some(provider) = cap.provider_id {
        query.push(" AND d.provider_id = ").push_bind(provider);
    }
    if !cap.harness_kinds.is_empty() {
        query
            .push(" AND d.harness_kind = ANY(")
            .push_bind(cap.harness_kinds.clone())
            .push(")");
    }
    if !cap.tiers.is_empty() {
        query
            .push(" AND d.tier = ANY(")
            .push_bind(cap.tiers.clone())
            .push(")");
    }
    push_subject(&mut query, cap, person, "d.user_id", at);
    if cap.per_person() {
        query.push(" AND d.purpose = 'turn'");
    }
    query.push(" GROUP BY d.provider_id, d.model");
    Ok(query.build_query_as().fetch_all(conn).await?)
}

/// Adds the condition that the rows (whose person is `column`) are those a cap covers by its subject.
fn push_subject(
    query: &mut QueryBuilder<Postgres>,
    cap: &CapRow,
    person: Option<Id>,
    column: &str,
    at: DateTime<Utc>,
) {
    match (cap.subject_kind.as_str(), cap.split.as_str()) {
        (subject::USER, _) => match cap.subject_id {
            Some(user) => query.push(format!(" AND {column} = ")).push_bind(user),
            None => query.push(" AND false"),
        },
        (subject::GROUP | subject::MEMBERS, split::SHARED) => match cap.subject_id {
            Some(id) => {
                push_holder_users(query, column, cap.subject_kind.as_str(), id, at);
                query
            }
            None => query.push(" AND false"),
        },
        (subject::EVERYONE, split::SHARED) => query.push(" AND true"),
        // A cap given to each person covers one person at a time.
        _ => match person {
            Some(person) => query.push(format!(" AND {column} = ")).push_bind(person),
            None => query.push(" AND false"),
        },
    };
}

/// Uses the same current group/role/membership graph as permissions, at the caller's clock.
fn push_holder_users(
    query: &mut QueryBuilder<Postgres>,
    column: &str,
    kind: &str,
    id: Id,
    at: DateTime<Utc>,
) {
    query
        .push(format!(
            " AND {column} IN (SELECT user_id FROM access_holder_people("
        ))
        .push_bind(if kind == subject::MEMBERS {
            "members".to_owned()
        } else {
            "group".to_owned()
        })
        .push(", ")
        .push_bind(id)
        .push(", '', ")
        .push_bind(at)
        .push(", true))");
}

/// A turn that is under way and covered by a cap.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct Running {
    /// The turn.
    pub turn_id: Id,
    /// Its thread.
    pub thread_id: Id,
    /// The model it runs, by its routing name.
    pub model: String,
    /// The provider it runs on.
    pub provider_id: Id,
    /// The person it is put down to.
    pub user_id: Option<Id>,
    /// The harness kind of its agent.
    pub harness_kind: Option<String>,
    /// The parameters it was started with: among them the service tier.
    pub params: Json,
    /// The parameters its agent's harness declares, to tell which of the above is the service tier.
    pub declared: Option<Json>,
    /// What its harness reported so far.
    pub usage: Option<Json>,
    /// The limits it was let past when it started.
    pub limit_bypass: Option<Json>,
    /// The tokens the ledger counts for it already.
    pub counted_tokens: i64,
    /// The credits the ledger counts for it already.
    pub counted_credits: String,
}

/// Active turns and, for consumption caps, terminal turns awaiting accounting, covered by the
/// cap with what the ledger has counted already. Pending terminal usage is frozen, while a
/// running-turn cap counts active turns only. Model matching remains the engine's rule.
pub async fn running(
    conn: &mut Conn,
    cap: &CapRow,
    person: Option<Id>,
    at: DateTime<Utc>,
) -> Result<Vec<Running>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT t.id AS turn_id, t.thread_id, t.model, s.provider_id, i.author_id AS user_id, a.harness_kind,
                t.params, a.descriptor -> 'parameters' AS declared,
                CASE WHEN t.end_work IS NULL THEN t.usage ELSE t.end_work#>'{snapshot,usage}' END AS usage, t.limit_bypass,
                COALESCE((SELECT sum(e.total_tokens) FROM usage_events e WHERE e.turn_id = t.id AND e.counted), 0)::bigint
                    AS counted_tokens,
                COALESCE((SELECT sum(e.credits) FROM usage_events e WHERE e.turn_id = t.id AND e.counted), 0)::text
                    AS counted_credits
           FROM turns t
           JOIN segments s ON s.id = t.segment_id
           LEFT JOIN thread_inputs i ON i.id = t.input_id
           LEFT JOIN agents a ON a.id = s.agent_id
          WHERE (t.state IN ('starting', 'running')",
    );
    // Terminal consumption remains reserved until its ledger/notification transaction commits.
    // A concurrency cap still counts actual active turns only.
    if cap.meter != meter::RUNNING {
        query.push(" OR t.end_work IS NOT NULL");
    }
    query.push(")");
    push_running_scope(&mut query, cap);
    push_subject(&mut query, cap, person, "i.author_id", at);
    query.push(" ORDER BY t.created_at, t.id");
    Ok(query.build_query_as().fetch_all(conn).await?)
}

/// Adds the conditions that say a running turn (`t`, its segment `s`, its agent `a`) lies in the node, on the
/// provider and under the harness kinds a cap names; not whom it covers and which models.
fn push_running_scope(query: &mut QueryBuilder<Postgres>, cap: &CapRow) {
    if let Some(node) = cap.node_id {
        query
            .push(" AND EXISTS (SELECT 1 FROM nodes pn WHERE pn.id = t.thread_id AND ")
            .push_bind(node)
            .push(" = ANY(pn.path))");
    }
    if let Some(provider) = cap.provider_id {
        query.push(" AND s.provider_id = ").push_bind(provider);
    }
    if !cap.harness_kinds.is_empty() {
        query
            .push(" AND a.harness_kind = ANY(")
            .push_bind(cap.harness_kinds.clone())
            .push(")");
    }
}

/// The threads with a turn under way (`starting` or `running`) in the node, on the provider and under the
/// harness kinds a cap names, whomever the cap covers: where the turns a cap that was made or lowered may have to
/// end are looked for.
pub async fn running_threads(conn: &mut Conn, cap: &CapRow) -> Result<Vec<Id>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT DISTINCT t.thread_id
           FROM turns t
           JOIN segments s ON s.id = t.segment_id
           LEFT JOIN agents a ON a.id = s.agent_id
          WHERE t.state IN ('starting', 'running')",
    );
    push_running_scope(&mut query, cap);
    Ok(query.build_query_scalar().fetch_all(conn).await?)
}

/// The people, with the provider and the model of each use, who used something a cap covers since a day:
/// whose share of a cap counted per person has anything in it. The models a cap names are not asked here.
pub async fn users_of(
    conn: &mut Conn,
    cap: &CapRow,
    since: Option<NaiveDate>,
    at: DateTime<Utc>,
) -> Result<Vec<(Id, Id, String)>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT DISTINCT d.user_id, d.provider_id, d.model FROM usage_daily d
          WHERE d.user_id IS NOT NULL AND d.purpose = 'turn'",
    );
    if let Some(since) = since {
        query.push(" AND d.day >= ").push_bind(since);
    }
    if let Some(node) = cap.node_id {
        query
            .push(
                " AND EXISTS (SELECT 1 FROM nodes pn
                               WHERE pn.id = COALESCE(d.thread_id, d.topic_id) AND ",
            )
            .push_bind(node)
            .push(" = ANY(pn.path))");
    }
    if let Some(provider) = cap.provider_id {
        query.push(" AND d.provider_id = ").push_bind(provider);
    }
    if !cap.harness_kinds.is_empty() {
        query
            .push(" AND d.harness_kind = ANY(")
            .push_bind(cap.harness_kinds.clone())
            .push(")");
    }
    if !cap.tiers.is_empty() {
        query
            .push(" AND d.tier = ANY(")
            .push_bind(cap.tiers.clone())
            .push(")");
    }
    match (cap.subject_kind.as_str(), cap.subject_id) {
        (subject::USER, Some(user)) => {
            query.push(" AND d.user_id = ").push_bind(user);
        }
        (subject::GROUP | subject::MEMBERS, Some(id)) => {
            push_holder_users(&mut query, "d.user_id", cap.subject_kind.as_str(), id, at)
        }
        _ => {}
    }
    Ok(query.build_query_as().fetch_all(conn).await?)
}

/// How many threads a cap covers by its node, provider and harness kind in which nobody reports usage:
/// bound threads whose harness does not report its own totals and whose model calls do not pass the
/// forwarder. A cap in credits or tokens cannot count what they use.
pub async fn unmetered_threads(conn: &mut Conn, cap: &CapRow) -> Result<i64, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT count(*) FROM threads th
           JOIN nodes n ON n.id = th.node_id
           JOIN agents a ON a.id = th.agent_id
           JOIN segments s ON s.id = th.current_segment_id
          WHERE n.deleted_at IS NULL AND th.provider_id IS NOT NULL AND s.credential_path <> 'forwarded'
            AND COALESCE((a.descriptor #>> '{capabilities,usage_tokens}')::boolean, false) = false",
    );
    if let Some(node) = cap.node_id {
        query.push(" AND ").push_bind(node).push(" = ANY(n.path)");
    }
    if let Some(provider) = cap.provider_id {
        query.push(" AND th.provider_id = ").push_bind(provider);
    }
    if !cap.harness_kinds.is_empty() {
        query
            .push(" AND a.harness_kind = ANY(")
            .push_bind(cap.harness_kinds.clone())
            .push(")");
    }
    Ok(query.build_query_scalar().fetch_one(conn).await?)
}

/// Takes the locks that make counting turns under caps exact: one on each cap, in ascending order of their
/// identifiers, held until the transaction ends. Two pre-flights that cover the same cap count one after the
/// other, so that the second finds the first one's turn.
pub async fn lock_for_counting(conn: &mut Conn, caps: &[Id]) -> Result<(), DbError> {
    let mut ids = caps.to_vec();
    ids.sort_unstable();
    ids.dedup();
    for id in ids {
        sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
            .bind(format!("usage_cap_running:{id}"))
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}

// ---------------------------------------------------------------- what waits

/// Somebody who waits for room in a cap.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Waiter {
    /// The thread.
    pub thread_id: Id,
    /// Who answers for the input: whoever it runs on, otherwise its author.
    pub author_id: Option<Id>,
    /// Since when the oldest input that waits has waited.
    pub since: DateTime<Utc>,
    /// Whether it is an input held for its author's own cap, and not a thread blocked.
    pub held: bool,
}

/// What waits for room, in the order it will run: the threads blocked for want of room (their oldest input),
/// and the inputs held for their author's own cap, oldest first. A thread or an input is the cap's by its node,
/// provider, harness kind and subject, as the thread is bound now. These are candidates: the engine
/// resolves each thread's next model and service tier before showing or ordering them.
pub async fn waiting(
    conn: &mut Conn,
    cap: &CapRow,
    at: DateTime<Utc>,
) -> Result<Vec<Waiter>, DbError> {
    let mut query = QueryBuilder::<Postgres>::new(
        "SELECT th.node_id AS thread_id, COALESCE(i.run_as, i.author_id) AS author_id, i.created_at AS since,
                (i.state_detail IS NOT DISTINCT FROM 'cap_reached') AS held
           FROM thread_inputs i
           JOIN threads th ON th.node_id = i.thread_id
           JOIN nodes n ON n.id = th.node_id
           LEFT JOIN agents a ON a.id = th.agent_id
          WHERE i.state = 'pending' AND n.deleted_at IS NULL
            AND ((th.state = 'blocked' AND th.blocked_reason = 'quota_exceeded'
                  AND i.id = (SELECT o.id FROM thread_inputs o WHERE o.thread_id = th.node_id AND o.state = 'pending'
                               ORDER BY o.created_at, o.id LIMIT 1))
                 OR i.state_detail = 'cap_reached')",
    );
    if let Some(node) = cap.node_id {
        query.push(" AND ").push_bind(node).push(" = ANY(n.path)");
    }
    if let Some(provider) = cap.provider_id {
        query.push(" AND th.provider_id = ").push_bind(provider);
    }
    if !cap.harness_kinds.is_empty() {
        query
            .push(" AND a.harness_kind = ANY(")
            .push_bind(cap.harness_kinds.clone())
            .push(")");
    }
    match (cap.subject_kind.as_str(), cap.subject_id) {
        (subject::USER, Some(user)) => {
            query
                .push(" AND COALESCE(i.run_as, i.author_id) = ")
                .push_bind(user);
        }
        (subject::GROUP | subject::MEMBERS, Some(id)) => push_holder_users(
            &mut query,
            "COALESCE(i.run_as, i.author_id)",
            cap.subject_kind.as_str(),
            id,
            at,
        ),
        (subject::ROLE, _) => {
            query.push(" AND COALESCE(i.run_as, i.author_id) IS NOT NULL");
        }
        _ => {}
    }
    query.push(" ORDER BY i.created_at, i.id");
    Ok(query.build_query_as().fetch_all(conn).await?)
}

/// The threads blocked for want of room in a cap, the one that has waited longest first: the order in which
/// they are looked at again when room is made, so that a cap behaves as a queue.
pub async fn blocked_for_room(conn: &mut Conn, limit: i64) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id = t.node_id
          WHERE t.state = 'blocked' AND t.blocked_reason = 'quota_exceeded'
            AND n.deleted_at IS NULL AND n.archived_at IS NULL
          ORDER BY (SELECT min(i.created_at) FROM thread_inputs i WHERE i.thread_id = t.node_id AND i.state = 'pending')
                   NULLS LAST, t.node_id
          LIMIT $1",
    )
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

// ---------------------------------------------------------------- notices

/// Records that a warning or the word that a cap was reached went out, for a person (none for a shared cap)
/// and the period that began on `period_start`. The answer says whether this call recorded it: a notice is
/// sent once for each cap, person, period and level.
pub async fn note_once(
    conn: &mut Conn,
    cap: Id,
    person: Option<Id>,
    period_start: NaiveDate,
    level: &str,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO usage_cap_notices (cap_id, user_id, period_start, level, sent_at)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (cap_id, user_id, period_start, level) DO NOTHING",
    )
    .bind(cap)
    .bind(person)
    .bind(period_start)
    .bind(level)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

// ---------------------------------------------------------------- bypasses

/// A turn that was let past a limit.
#[derive(Debug, Clone)]
pub struct NewBypass<'a> {
    /// The row's identifier.
    pub id: Id,
    /// The cap; none for a provider's own limit.
    pub cap_id: Option<Id>,
    /// The provider whose own limit it was.
    pub provider_id: Option<Id>,
    /// The thread.
    pub thread_id: Id,
    /// The turn.
    pub turn_id: Id,
    /// Whose input it was.
    pub author_id: Option<Id>,
    /// Who let it past.
    pub by_user: Id,
    /// `tokens`, `credits`, `running` or `provider_limit`.
    pub meter: &'a str,
    /// What the limit showed then, in its unit.
    pub used: Option<&'a str>,
    /// What it allowed.
    pub amount: Option<&'a str>,
    /// The day the period began.
    pub period_start: Option<NaiveDate>,
    /// When.
    pub at: DateTime<Utc>,
}

/// Writes down that a turn was let past a limit.
pub async fn record_bypass(conn: &mut Conn, bypass: &NewBypass<'_>) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO usage_limit_bypasses (id, cap_id, provider_id, thread_id, turn_id, author_id, by_user, meter,
                used, amount, period_start, at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9::text::numeric, $10::text::numeric, $11, $12)",
    )
    .bind(bypass.id)
    .bind(bypass.cap_id)
    .bind(bypass.provider_id)
    .bind(bypass.thread_id)
    .bind(bypass.turn_id)
    .bind(bypass.author_id)
    .bind(bypass.by_user)
    .bind(bypass.meter)
    .bind(bypass.used)
    .bind(bypass.amount)
    .bind(bypass.period_start)
    .bind(bypass.at)
    .execute(conn)
    .await?;
    Ok(())
}

/// How often turns were let past a cap, and the last time.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BypassTally {
    /// How many since the moment asked for.
    pub count: i64,
    /// Who let the latest past.
    pub last_by: Option<Id>,
    /// When.
    pub last_at: Option<DateTime<Utc>>,
}

/// How many turns were let past a cap since `since`, and who let the latest past, and when.
pub async fn bypasses_of(
    conn: &mut Conn,
    cap: Id,
    since: DateTime<Utc>,
) -> Result<BypassTally, DbError> {
    Ok(sqlx::query_as(
        "SELECT (SELECT count(*) FROM usage_limit_bypasses WHERE cap_id = $1 AND at >= $2) AS count,
                l.by_user AS last_by, l.at AS last_at
           FROM (SELECT 1) one
           LEFT JOIN LATERAL (SELECT by_user, at FROM usage_limit_bypasses WHERE cap_id = $1
                               ORDER BY at DESC, id DESC LIMIT 1) l ON true",
    )
    .bind(cap)
    .bind(since)
    .fetch_one(conn)
    .await?)
}

/// The owner of a node, as the first person to ask about what the node's page gives.
pub async fn node_owner(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT owner_id FROM nodes WHERE id = $1")
            .bind(node)
            .fetch_optional(conn)
            .await?,
    )
}

/// The owner of a provider.
pub async fn provider_owner(conn: &mut Conn, provider: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT owner_id FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_optional(conn)
            .await?,
    )
}

/// The root org of the home of a provider.
pub async fn provider_org(conn: &mut Conn, provider: Id) -> Result<Option<Id>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT org_id FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_optional(conn)
            .await?,
    )
}

/// The root org of a node.
pub async fn node_org(conn: &mut Conn, node: Id) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT org_id FROM nodes WHERE id = $1")
        .bind(node)
        .fetch_optional(conn)
        .await?)
}

/// The name of a provider.
pub async fn provider_name(conn: &mut Conn, provider: Id) -> Result<Option<String>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT name FROM providers WHERE id = $1")
            .bind(provider)
            .fetch_optional(conn)
            .await?,
    )
}

/// The enabled caps of the same node, provider, models, tiers, harness kinds, meter and period as `cap`,
/// itself among them: those that compete to apply to a person when they are counted per person.
pub async fn siblings(conn: &mut Conn, cap: &CapRow) -> Result<Vec<CapRow>, DbError> {
    let sql = format!(
        "SELECT {CAP_COLUMNS} FROM usage_caps
          WHERE enabled AND node_id IS NOT DISTINCT FROM $1::uuid AND provider_id IS NOT DISTINCT FROM $2::uuid
            AND models = $3 AND harness_kinds = $4 AND meter = $5 AND period IS NOT DISTINCT FROM $6::text
            AND tiers = $7
          ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(cap.node_id)
        .bind(cap.provider_id)
        .bind(&cap.models)
        .bind(&cap.harness_kinds)
        .bind(&cap.meter)
        .bind(&cap.period)
        .bind(&cap.tiers)
        .fetch_all(conn)
        .await?)
}

/// All the enabled caps counted per person, wherever they are defined: the ones a person's own list of limits
/// is made of.
pub async fn per_person(conn: &mut Conn) -> Result<Vec<CapRow>, DbError> {
    let sql = format!(
        "SELECT {CAP_COLUMNS} FROM usage_caps
          WHERE enabled AND (split = 'each' OR subject_kind = 'user') ORDER BY created_at, id"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .fetch_all(conn)
        .await?)
}
