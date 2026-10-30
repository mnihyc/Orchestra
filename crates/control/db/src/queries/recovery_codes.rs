//! Hashed one-use authenticator recovery codes, replaced and consumed in the caller's transaction.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::Id;

/// Only metadata can be read after the plaintext batch was issued.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct Status {
    /// Unused codes of the enabled authenticator.
    pub remaining: i64,
    /// When its current batch was issued; none before a batch exists.
    pub created_at: Option<DateTime<Utc>>,
}

/// The current batch's metadata, never its codes or hashes. Disabled authenticators have no usable batch.
pub async fn status(conn: &mut Conn, user: Id) -> Result<Status, DbError> {
    Ok(sqlx::query_as(
        "SELECT count(*) FILTER (WHERE c.used_at IS NULL) AS remaining, max(c.created_at) AS created_at
           FROM second_step_recovery_codes c JOIN users u ON u.id = c.user_id
          WHERE c.user_id = $1 AND u.kind = 'human' AND u.totp_enabled_at IS NOT NULL",
    ).bind(user).fetch_one(conn).await?)
}

/// Whether this exact hash is unspent, for this person with their authenticator still enabled.
/// This is only a preview of a proof: [`spend`] must succeed in the act's transaction too.
pub async fn available(conn: &mut Conn, user: Id, hash: &[u8]) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM second_step_recovery_codes c JOIN users u ON u.id = c.user_id
                         WHERE c.user_id = $1 AND c.code_hash = $2 AND c.used_at IS NULL
                           AND u.kind = 'human' AND u.totp_enabled_at IS NOT NULL)",
    )
    .bind(user)
    .bind(hash)
    .fetch_one(conn)
    .await?)
}

/// Replaces every older code with a newly issued batch. The caller holds the person's row lock
/// and commits this with activation or fresh-proof verification; plaintext is never stored here.
pub async fn replace(
    conn: &mut Conn,
    user: Id,
    hashes: &[Vec<u8>],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    clear(conn, user).await?;
    sqlx::query(
        "INSERT INTO second_step_recovery_codes (user_id, code_hash, created_at)
         SELECT $1, code_hash, $3 FROM unnest($2::bytea[]) AS codes(code_hash)",
    )
    .bind(user)
    .bind(hashes)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    crate::sequences::notify_identity(conn, user).await?;
    Ok(())
}

/// Accepts a code exactly once, with the completed sign-in/act. An older generation, another
/// person's code, a replay or a disabled authenticator cannot spend it. Rollback leaves it unused.
pub async fn spend(
    conn: &mut Conn,
    user: Id,
    hash: &[u8],
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE second_step_recovery_codes c SET used_at = $3 FROM users u
          WHERE c.user_id = $1 AND c.code_hash = $2 AND c.used_at IS NULL
            AND u.id = c.user_id AND u.kind = 'human' AND u.totp_enabled_at IS NOT NULL",
    )
    .bind(user)
    .bind(hash)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    if result.rows_affected() == 1 {
        crate::sequences::notify_identity(conn, user).await?;
    }
    Ok(result.rows_affected() == 1)
}

/// Invalidates the complete batch when an authenticator is disabled, reset or replaced.
pub async fn clear(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("DELETE FROM second_step_recovery_codes WHERE user_id = $1")
        .bind(user)
        .execute(&mut *conn)
        .await?;
    sqlx::query("DELETE FROM sensitive_proofs p USING sessions s WHERE p.session_id = s.id AND s.user_id = $1 AND p.purpose = 'recovery_codes_after_verification'")
        .bind(user).execute(conn).await?;
    Ok(())
}

/// A completed recovery proof grants this exact session one short-lived recovery-management act.
/// It cannot be spent for tokens or another session/authenticator, and still needs a fresh first proof.
pub async fn grant_management(
    conn: &mut Conn,
    user: Id,
    session: Id,
    factor: &[u8],
    reference: &[u8],
    now: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("DELETE FROM sensitive_proofs WHERE session_id = $1 AND purpose = 'recovery_codes_after_verification'")
        .bind(session).execute(&mut *conn).await?;
    sqlx::query(
        "INSERT INTO sensitive_proofs (proof_hash, session_id, purpose, factor_hash, verified_at, expires_at)
         SELECT $4, s.id, 'recovery_codes_after_verification', $3, $5, $5 + interval '5 minutes'
           FROM sessions s WHERE s.id = $2 AND s.user_id = $1 AND s.revoked_at IS NULL AND s.expires_at > $5",
    ).bind(user).bind(session).bind(factor).bind(reference).bind(now).execute(conn).await?;
    Ok(())
}

/// Looks only for the current session's unused management allowance; the opaque reference never leaves the server.
pub async fn management(
    conn: &mut Conn,
    user: Id,
    session: Id,
    factor: &[u8],
    now: DateTime<Utc>,
) -> Result<Option<Vec<u8>>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT p.proof_hash FROM sensitive_proofs p JOIN sessions s ON s.id = p.session_id
          WHERE s.id = $2 AND s.user_id = $1 AND s.revoked_at IS NULL AND s.expires_at > $4
            AND p.purpose = 'recovery_codes_after_verification' AND p.factor_hash = $3
            AND p.used_at IS NULL AND p.expires_at > $4
          ORDER BY p.verified_at DESC LIMIT 1",
    )
    .bind(user)
    .bind(session)
    .bind(factor)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Spends only this session/authenticator's allowance, with generation replacement in the same transaction.
pub async fn spend_management(
    conn: &mut Conn,
    user: Id,
    session: Id,
    factor: &[u8],
    reference: &[u8],
    now: DateTime<Utc>,
) -> Result<bool, DbError> {
    let result = sqlx::query(
        "UPDATE sensitive_proofs p SET used_at = $5 FROM sessions s
          WHERE p.proof_hash = $4 AND p.session_id = $2 AND p.factor_hash = $3
            AND p.purpose = 'recovery_codes_after_verification' AND p.used_at IS NULL AND p.expires_at > $5
            AND s.id = p.session_id AND s.user_id = $1 AND s.revoked_at IS NULL AND s.expires_at > $5",
    ).bind(user).bind(session).bind(factor).bind(reference).bind(now).execute(conn).await?;
    Ok(result.rows_affected() == 1)
}
