//! Narrow answers allowed for one thread (`thread_approval_rules`): kept by Orch, bound to what the thread runs on
//! (harness, provider, profile, environment and directory), answering a later request that offers the same matcher
//! once each, taken back by a person or ended by a change of that binding. Never a harness's own settings.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json, Seq};

/// What a rule is bound to: the thread's binding when it was allowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    /// The harness.
    pub harness_kind: String,
    /// The provider.
    pub provider_id: Id,
    /// The agent profile.
    pub agent_id: Id,
    /// The environment.
    pub environment_id: Id,
    /// The thread's directory there.
    pub directory: String,
}

/// One rule.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct RuleRow {
    /// Its identifier.
    pub id: Id,
    /// The thread.
    pub thread_id: Id,
    /// The request whose option allowed it.
    pub approval_id: Id,
    /// The matcher, as the option showed it.
    pub matcher: Json,
    /// Who allowed it.
    pub created_by: Id,
    /// When.
    pub created_at: DateTime<Utc>,
    /// When it ended, if it did.
    pub revoked_at: Option<DateTime<Utc>>,
    /// Who took it back; none when the binding changed.
    pub revoked_by: Option<Id>,
    /// Why it no longer answers.
    pub inactive_reason: Option<String>,
    /// How many times it changed.
    pub revision: i64,
}

const RULE_SELECT: &str =
    "SELECT id, thread_id, approval_id, matcher, created_by, created_at, revoked_at, revoked_by,
        inactive_reason, revision FROM thread_approval_rules";

/// Keeps a rule a person allowed.
#[expect(
    clippy::too_many_arguments,
    reason = "each value is one column of the rule"
)]
pub async fn insert(
    conn: &mut Conn,
    id: Id,
    thread: Id,
    approval: Id,
    binding: &Binding,
    matcher: &Json,
    by: Id,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO thread_approval_rules (id, thread_id, approval_id, harness_kind, provider_id, agent_id,
                                           environment_id, directory, matcher, created_by, created_at, seq_updated)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)",
    )
    .bind(id)
    .bind(thread)
    .bind(approval)
    .bind(&binding.harness_kind)
    .bind(binding.provider_id)
    .bind(binding.agent_id)
    .bind(binding.environment_id)
    .bind(&binding.directory)
    .bind(matcher)
    .bind(by)
    .bind(at)
    .bind(seq)
    .execute(conn)
    .await?;
    Ok(())
}

/// The active rule of a thread, bound to this very binding, whose matcher is this one, if there is one.
pub async fn active_matching(
    conn: &mut Conn,
    thread: Id,
    binding: &Binding,
    matcher: &Json,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM thread_approval_rules
          WHERE thread_id = $1 AND revoked_at IS NULL AND inactive_reason IS NULL
            AND harness_kind = $2 AND provider_id = $3 AND agent_id = $4 AND environment_id = $5 AND directory = $6
            AND matcher = $7
          ORDER BY created_at LIMIT 1",
    )
    .bind(thread)
    .bind(&binding.harness_kind)
    .bind(binding.provider_id)
    .bind(binding.agent_id)
    .bind(binding.environment_id)
    .bind(&binding.directory)
    .bind(matcher)
    .fetch_optional(conn)
    .await?)
}

/// A thread's rules, newest first, before the rule `before` where one is named.
pub async fn list(
    conn: &mut Conn,
    thread: Id,
    before: Option<Id>,
    limit: i64,
) -> Result<Vec<RuleRow>, DbError> {
    let sql = format!(
        "{RULE_SELECT} WHERE thread_id = $1
            AND ($2::uuid IS NULL OR (created_at, id) < (SELECT created_at, id FROM thread_approval_rules WHERE id = $2))
          ORDER BY created_at DESC, id DESC LIMIT $3"
    );
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(before)
        .bind(limit)
        .fetch_all(conn)
        .await?)
}

/// One rule of a thread.
pub async fn rule(conn: &mut Conn, thread: Id, rule: Id) -> Result<Option<RuleRow>, DbError> {
    let sql = format!("{RULE_SELECT} WHERE thread_id = $1 AND id = $2");
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(thread)
        .bind(rule)
        .fetch_optional(conn)
        .await?)
}

/// Ends an active rule: taken back by `by`, or ended for `reason` by nobody. False when it was not active.
pub async fn revoke(
    conn: &mut Conn,
    rule: Id,
    by: Option<Id>,
    reason: &str,
    at: DateTime<Utc>,
    seq: Seq,
) -> Result<bool, DbError> {
    Ok(sqlx::query(
        "UPDATE thread_approval_rules
            SET revoked_at = $3, revoked_by = $2, inactive_reason = $4, revision = revision + 1, seq_updated = $5
          WHERE id = $1 AND revoked_at IS NULL AND inactive_reason IS NULL",
    )
    .bind(rule)
    .bind(by)
    .bind(at)
    .bind(reason)
    .bind(seq)
    .execute(conn)
    .await?
    .rows_affected()
        == 1)
}

/// The active rules of a thread, oldest first.
pub async fn active_of(conn: &mut Conn, thread: Id) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT id FROM thread_approval_rules
          WHERE thread_id = $1 AND revoked_at IS NULL AND inactive_reason IS NULL ORDER BY created_at, id",
    )
    .bind(thread)
    .fetch_all(conn)
    .await?)
}
