//! What keeps an API token out beyond its scopes: the orgs that refused it, the longest life a place admits, and the one-use proofs a sensitive act spends.
//!
//! A token acts for its person inside the scopes and the subtree it was made
//! with. An org may refuse it there (`api_token_refusals`), which keeps it out
//! of that org and everything inside while it works elsewhere; and a place may
//! admit tokens only up to a total life (`tokens.max_days`, the smallest value
//! of the system and the orgs on the way), which a token made elsewhere or
//! before the bound may exceed. Both are read when the token is used, so a
//! refusal or a new bound holds from the next request on.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// One place an API token is kept out of: an org with everything inside, or the whole system.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct BlockedAt {
    /// The org; `None` for the system's own bound, which keeps the token out everywhere.
    pub org: Option<Id>,
    /// The org refused the token.
    pub refused: bool,
    /// The longest life in days the place admits, which the token's whole life exceeds; `None` for a
    /// refusal.
    pub max_days: Option<i64>,
}

/// Where the token `token` is kept out of now: the orgs that refused it, and every scope whose
/// `tokens.max_days` its whole life exceeds (from when it was made to when it ends; a token that never
/// ends exceeds every bound).
pub async fn blocked(conn: &mut Conn, token: Id) -> Result<Vec<BlockedAt>, DbError> {
    Ok(sqlx::query_as(
        "SELECT r.org_id AS org, true AS refused, NULL::bigint AS max_days
           FROM api_token_refusals r
          WHERE r.token_id = $1
         UNION ALL
         SELECT s.node_id AS org, false AS refused, (s.value #>> '{}')::bigint AS max_days
           FROM settings s
           JOIN api_tokens t ON t.id = $1
          WHERE s.key = 'tokens.max_days' AND jsonb_typeof(s.value) = 'number'
            AND (t.expires_at IS NULL
                 OR t.expires_at - t.created_at > make_interval(days => (s.value #>> '{}')::int))",
    )
    .bind(token)
    .fetch_all(conn)
    .await?)
}

/// Refuses the token `token` in the org `org` and everything inside it; nothing when it is refused
/// there already. Answers whether this call refused it.
pub async fn refuse(
    conn: &mut Conn,
    token: Id,
    org: Id,
    by: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "INSERT INTO api_token_refusals (token_id, org_id, by_user_id, at)
         VALUES ($1, $2, $3, $4)
         ON CONFLICT (token_id, org_id) DO NOTHING",
    )
    .bind(token)
    .bind(org)
    .bind(by)
    .bind(at)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A token that can reach an org, as its managers list it: never its secret.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct ReachingToken {
    /// The token.
    pub id: Id,
    /// The name its maker gave it.
    pub name: String,
    /// Its scope words.
    pub scopes: Vec<String>,
    /// The node it is kept to.
    pub node_id: Option<Id>,
    /// That node's name, where it is kept to one.
    pub node_name: Option<String>,
    /// That node's kind.
    pub node_kind: Option<String>,
    /// When it was made.
    pub created_at: DateTime<Utc>,
    /// When it was last used.
    pub last_used_at: Option<DateTime<Utc>>,
    /// When it ends.
    pub expires_at: Option<DateTime<Utc>>,
    /// Whose it is.
    pub user_id: Id,
    /// It belongs to a service identity.
    pub service: bool,
    /// The org refused it already.
    pub refused_here: bool,
}

/// The active tokens that can reach the org `org`, oldest first, after `after` (the id of the last row
/// of the page before): those whose person belongs to its tree at or above it, holds a role of their own
/// there or above, or is a service identity kept there or above, and whose node, where it is kept to
/// one, lies inside the org or on its way down.
pub async fn reaching(
    conn: &mut Conn,
    org: Id,
    now: DateTime<Utc>,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<ReachingToken>, DbError> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE up AS (
             SELECT id, parent_id FROM nodes WHERE id = $1
             UNION ALL
             SELECT n.id, n.parent_id FROM nodes n JOIN up ON n.id = up.parent_id
         ), down AS (
             SELECT id FROM nodes WHERE id = $1
             UNION ALL
             SELECT n.id FROM nodes n JOIN down ON n.parent_id = down.id
         ), people AS (
             SELECT m.user_id FROM org_members m WHERE m.domain_id IN (SELECT id FROM up)
                OR m.domain_id IN (SELECT id FROM down)
             UNION
             SELECT r.subject_id FROM node_roles r
              WHERE r.subject_kind = 'user' AND r.removed_at IS NULL
                AND (r.expires_at IS NULL OR r.expires_at > $2)
                AND (r.node_id IN (SELECT id FROM up) OR r.node_id IN (SELECT id FROM down))
             UNION
             SELECT u.id FROM users u WHERE u.kind = 'service' AND u.service_org_id IN (SELECT id FROM up)
         )
         SELECT t.id, t.name, t.scopes, t.node_id, n.name AS node_name, n.kind AS node_kind,
                t.created_at, t.last_used_at, t.expires_at, t.user_id, u.kind = 'service' AS service,
                EXISTS (SELECT 1 FROM api_token_refusals r WHERE r.token_id = t.id AND r.org_id = $1)
                    AS refused_here
           FROM api_tokens t
           JOIN users u ON u.id = t.user_id
           LEFT JOIN nodes n ON n.id = t.node_id
          WHERE t.user_id IN (SELECT user_id FROM people)
            AND t.revoked_at IS NULL AND (t.expires_at IS NULL OR t.expires_at > $2)
            AND (t.node_id IS NULL OR t.node_id IN (SELECT id FROM up) OR t.node_id IN (SELECT id FROM down))
            AND ($3::uuid IS NULL OR t.id > $3)
          ORDER BY t.id
          LIMIT $4",
    )
    .bind(org)
    .bind(now)
    .bind(after)
    .bind(limit)
    .fetch_all(conn)
    .await?)
}

/// Spends the one-use proof whose hash is `proof_hash`, given to the session `session` for `purpose`
/// and, for a service identity's token, for that identity (`target`): once, before it ends. Answers
/// whether it was there to spend.
pub async fn spend_sensitive_proof(
    conn: &mut Conn,
    proof_hash: &[u8],
    session: Id,
    purpose: &str,
    target: Option<Id>,
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let done = sqlx::query(
        "UPDATE sensitive_proofs SET used_at = $4
          WHERE proof_hash = $1 AND session_id = $2 AND purpose = $3
            AND service_account_id IS NOT DISTINCT FROM $5
            AND used_at IS NULL AND expires_at > $4",
    )
    .bind(proof_hash)
    .bind(session)
    .bind(purpose)
    .bind(now)
    .bind(target)
    .execute(conn)
    .await?;
    Ok(done.rows_affected() == 1)
}

/// A one-use proof an identity provider gave, as it is kept: only its hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSensitiveProof<'a> {
    /// The SHA-256 of the proof.
    pub proof_hash: &'a [u8],
    /// The browser session it was made for.
    pub session: Id,
    /// The act it may be spent on.
    pub purpose: &'a str,
    /// The service identity a token proof is for.
    pub target: Option<Id>,
    /// When the provider proved the person.
    pub verified_at: DateTime<Utc>,
    /// When it may no longer be spent.
    pub expires_at: DateTime<Utc>,
}

/// Keeps a one-use proof that an identity provider gave.
pub async fn keep_sensitive_proof(
    conn: &mut Conn,
    proof: &NewSensitiveProof<'_>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO sensitive_proofs (proof_hash, session_id, purpose, service_account_id, verified_at, expires_at)
         VALUES ($1, $2, $3, $4, $5, $6)",
    )
    .bind(proof.proof_hash)
    .bind(proof.session)
    .bind(proof.purpose)
    .bind(proof.target)
    .bind(proof.verified_at)
    .bind(proof.expires_at)
    .execute(conn)
    .await?;
    Ok(())
}

// A fresh proof's sign-in is kept and taken as every sign-in is: `identity::begin_sign_in` and
// `identity::take_sign_in`.

/// The user the identity `subject` of the provider `provider` is linked to, if it is linked.
pub async fn identity_owner(
    conn: &mut Conn,
    provider: Id,
    subject: &str,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM user_identities WHERE identity_provider_id = $1 AND subject = $2",
    )
    .bind(provider)
    .bind(subject)
    .fetch_optional(conn)
    .await?)
}

/// The person of the session `session`, while it is neither ended nor past its end at `now`.
pub async fn live_session_user(
    conn: &mut Conn,
    session: Id,
    now: DateTime<Utc>,
) -> Result<Option<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT user_id FROM sessions WHERE id = $1 AND revoked_at IS NULL AND expires_at > $2",
    )
    .bind(session)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Whether the user `user` has an identity linked at the provider `provider`.
pub async fn has_identity_at(conn: &mut Conn, provider: Id, user: Id) -> Result<bool, DbError> {
    let (linked,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM user_identities WHERE identity_provider_id = $1 AND user_id = $2)",
    )
    .bind(provider)
    .bind(user)
    .fetch_one(conn)
    .await?;
    Ok(linked)
}

/// Records that the session `session` gave its second proof at `at`.
pub async fn stamp_second_step(
    conn: &mut Conn,
    session: Id,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let user: Option<Id> = sqlx::query_scalar(
        "UPDATE sessions SET second_step_at = $2 WHERE id = $1 RETURNING user_id",
    )
    .bind(session)
    .bind(at)
    .fetch_optional(&mut *conn)
    .await?;
    if let Some(user) = user {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(())
}
