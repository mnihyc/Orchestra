//! The second step an org can require, as sessions give it, and where a person was last seen inside an org: when a session last gave its second proof, whether any place requires one, and the one coalesced time of use per org and person.
//!
//! A session stamps `second_step_at` when it gives a second proof: at signing
//! in with two proofs, when the person sets their second step up, or when they
//! give it again for a session that had not. Whether a place requires a second
//! step at all is asked first and cheaply, so that an installation where none
//! does pays one indexed read a request and nothing more.
//!
//! `org_user_activity` holds one time per org and person: the last accepted
//! use of something in that org. It is written for every org on the path of
//! what was used, at most once in a while for each (the caller says how often),
//! and is read only by whoever manages the org's members. It is no list of
//! reads and no sign-in record.

use crate::{Conn, DbError};
use chrono::{DateTime, Duration, Utc};
use orch_types::Id;

/// When the session `session` last gave a second proof, if it ever did.
pub async fn session_given_at(
    conn: &mut Conn,
    session: Id,
) -> Result<Option<DateTime<Utc>>, DbError> {
    let row: Option<(Option<DateTime<Utc>>,)> =
        sqlx::query_as("SELECT second_step_at FROM sessions WHERE id = $1")
            .bind(session)
            .fetch_optional(conn)
            .await?;
    Ok(row.and_then(|(at,)| at))
}

/// Whether any place, the system or an org, sets `auth.second_step` to `required`. Where none
/// does, nobody's sign-in needs a second step and nothing more is read.
pub async fn required_anywhere(conn: &mut Conn) -> Result<bool, DbError> {
    let (any,): (bool,) = sqlx::query_as(
        "SELECT EXISTS (SELECT 1 FROM settings
                         WHERE key = 'auth.second_step' AND value = '\"required\"'::jsonb)",
    )
    .fetch_one(conn)
    .await?;
    Ok(any)
}

/// The root orgs of the people `users`, each person with the root orgs of the trees they are a
/// member of: their own org left out, and trees whose domain or an org above it is deleted too.
pub async fn root_orgs_of(conn: &mut Conn, users: &[Id]) -> Result<Vec<(Id, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT m.user_id, m.org_id
           FROM org_members m
           JOIN users u ON u.id = m.user_id
           JOIN nodes d ON d.id = m.domain_id
          WHERE m.user_id = ANY($1) AND m.org_id IS DISTINCT FROM u.personal_org_id
            AND d.deleted_at IS NULL
            AND NOT EXISTS (SELECT 1 FROM nodes gone
                             WHERE gone.id = ANY(d.path) AND gone.deleted_at IS NOT NULL)
          ORDER BY m.user_id, m.org_id",
    )
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// Notes that `user` used something that lies at `node`: every org on its path, from the root org
/// down, takes `at` as the time they were last seen there, unless it took one less than `coalesce`
/// before. A node that is not there notes nothing. Nothing is written, and no row locked, where every
/// org of the path took a time lately: a person who works on is one read a request.
pub async fn note_use(
    conn: &mut Conn,
    user: Id,
    node: Id,
    at: DateTime<Utc>,
    coalesce: Duration,
) -> Result<(), DbError> {
    let since = at - coalesce;
    let (lately,): (bool,) = sqlx::query_as(
        "SELECT COALESCE(bool_and(EXISTS (
                    SELECT 1 FROM org_user_activity s
                     WHERE s.org_id = o.id AND s.user_id = $2 AND s.last_seen_at >= $3)),
                 true)
           FROM nodes n
           CROSS JOIN LATERAL unnest(n.path) AS a(id)
           JOIN nodes o ON o.id = a.id AND o.kind = 'org' AND o.deleted_at IS NULL
          WHERE n.id = $1",
    )
    .bind(node)
    .bind(user)
    .bind(since)
    .fetch_one(&mut *conn)
    .await?;
    if lately {
        return Ok(());
    }
    sqlx::query(
        "INSERT INTO org_user_activity (org_id, user_id, last_seen_at)
         SELECT o.id, $2, $3
           FROM nodes n
           CROSS JOIN LATERAL unnest(n.path) AS a(id)
           JOIN nodes o ON o.id = a.id AND o.kind = 'org' AND o.deleted_at IS NULL
          WHERE n.id = $1
         ON CONFLICT (org_id, user_id) DO UPDATE SET last_seen_at = EXCLUDED.last_seen_at
          WHERE org_user_activity.last_seen_at < $4",
    )
    .bind(node)
    .bind(user)
    .bind(at)
    .bind(since)
    .execute(conn)
    .await?;
    Ok(())
}

/// How a person signs in, as far as a second step is concerned: what an org's managers are told.
#[derive(Debug, Clone, PartialEq, Eq, sqlx::FromRow)]
pub struct SignInFacts {
    /// The person.
    pub id: Id,
    /// A service identity, which has no human second step.
    pub service: bool,
    /// They have a password to sign in with.
    pub has_password: bool,
    /// An authenticator is their second step.
    pub totp_on: bool,
    /// A mailed code is their second step.
    pub email_code_on: bool,
    /// They are an admin of the system, who may sign in by a password where nobody else may.
    pub is_system_admin: bool,
}

/// How each of `users` signs in, as far as a second step is concerned.
pub async fn sign_in_facts(conn: &mut Conn, users: &[Id]) -> Result<Vec<SignInFacts>, DbError> {
    Ok(sqlx::query_as(
        "SELECT id, kind = 'service' AS service, password_hash IS NOT NULL AS has_password,
                totp_enabled_at IS NOT NULL AS totp_on, email_code_at IS NOT NULL AS email_code_on,
                is_system_admin
           FROM users WHERE id = ANY($1)",
    )
    .bind(users)
    .fetch_all(conn)
    .await?)
}

/// When each of `users` was last seen inside the org `org` (its own time, which every use of
/// something inside it set), for those who were.
pub async fn last_seen_in(
    conn: &mut Conn,
    org: Id,
    users: &[Id],
) -> Result<Vec<(Id, DateTime<Utc>)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT user_id, last_seen_at FROM org_user_activity WHERE org_id = $1 AND user_id = ANY($2)",
    )
    .bind(org)
    .bind(users)
    .fetch_all(conn)
    .await?)
}
