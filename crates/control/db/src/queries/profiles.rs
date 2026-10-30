//! Personal pages and ordered pins, with serialized changes and daily inspection deduplication.

use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};

/// The person's stored page, or the shipped defaults when they have never edited it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ProfileRow {
    /// Their own words.
    pub bio: String,
    /// Their pronouns.
    pub pronouns: String,
    /// Their chosen links.
    pub links: Json,
    /// Whether others see their local time.
    pub show_time: bool,
    /// Whether others see their visible work.
    pub show_activity: bool,
    /// `system`, `orgs` or `private`.
    pub audience: String,
}

/// Reads a person's page with the defaults for an unwritten page.
pub async fn get(conn: &mut Conn, user: Id) -> Result<ProfileRow, DbError> {
    Ok(sqlx::query_as(
        "SELECT COALESCE(p.bio, '') AS bio, COALESCE(p.pronouns, '') AS pronouns,
                COALESCE(p.links, '[]'::jsonb) AS links, COALESCE(p.show_time, true) AS show_time,
                COALESCE(p.show_activity, true) AS show_activity,
                COALESCE(p.audience, 'system') AS audience
           FROM users u LEFT JOIN user_profiles p ON p.user_id = u.id WHERE u.id = $1",
    )
    .bind(user)
    .fetch_one(conn)
    .await?)
}

/// Serializes page and pin replacements even before a profile row exists.
pub async fn lock(conn: &mut Conn, user: Id) -> Result<(), DbError> {
    sqlx::query("SELECT id FROM users WHERE id = $1 FOR UPDATE")
        .bind(user)
        .execute(conn)
        .await?;
    Ok(())
}

/// Stores a whole page after its patch was merged under the person's lock.
pub async fn put(
    conn: &mut Conn,
    user: Id,
    row: &ProfileRow,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO user_profiles (user_id,bio,pronouns,links,show_time,show_activity,audience,updated_at)
         VALUES ($1,$2,$3,$4,$5,$6,$7,$8)
         ON CONFLICT (user_id) DO UPDATE SET bio=$2,pronouns=$3,links=$4,show_time=$5,
             show_activity=$6,audience=$7,updated_at=$8",
    ).bind(user).bind(&row.bio).bind(&row.pronouns).bind(&row.links)
        .bind(row.show_time).bind(row.show_activity).bind(&row.audience).bind(at)
        .execute(conn).await?;
    Ok(())
}

/// The kind and identifier of a stored pin, in the person's order.
pub async fn pins(conn: &mut Conn, user: Id) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "SELECT object_kind, object_id FROM profile_pins WHERE user_id=$1 ORDER BY position",
    )
    .bind(user)
    .fetch_all(conn)
    .await?)
}

/// Replaces every pin atomically; the caller has validated distinctness, access and the six-pin bound.
pub async fn put_pins(conn: &mut Conn, user: Id, pins: &[(String, Id)]) -> Result<(), DbError> {
    sqlx::query("DELETE FROM profile_pins WHERE user_id=$1")
        .bind(user)
        .execute(&mut *conn)
        .await?;
    for (position, (kind, id)) in pins.iter().enumerate() {
        sqlx::query("INSERT INTO profile_pins (user_id,position,object_kind,object_id) VALUES ($1,$2,$3,$4)")
            .bind(user).bind(i16::try_from(position).unwrap_or(i16::MAX)).bind(kind).bind(id)
            .execute(&mut *conn).await?;
    }
    Ok(())
}

/// Whether this reader has already inspected this person's page in the supplied UTC day.
pub async fn inspected_today(
    conn: &mut Conn,
    reader: Id,
    person: Id,
    at: DateTime<Utc>,
) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM audit_log WHERE action='profile.inspected'
         AND actor_id=$1 AND object_kind='user' AND object_id=$2
         AND at >= date_trunc('day',$3::timestamptz AT TIME ZONE 'UTC') AT TIME ZONE 'UTC'
         AND at < (date_trunc('day',$3::timestamptz AT TIME ZONE 'UTC') + interval '1 day') AT TIME ZONE 'UTC')",
    ).bind(reader).bind(person).bind(at).fetch_one(conn).await?)
}

/// Distinct active direct shares between two people, in stable kind/id order. Visibility is decided by authz.
pub async fn direct_shares(
    conn: &mut Conn,
    reader: Id,
    person: Id,
    at: DateTime<Utc>,
    after: Option<(&str, Id)>,
    limit: i64,
) -> Result<Vec<(String, Id)>, DbError> {
    Ok(sqlx::query_as(
        "WITH shared AS (
           SELECT e.object_kind AS kind, e.object_id AS id FROM permission_entries e
            WHERE e.object_kind IN ('node','document','document_container') AND e.page='live'
              AND e.holder_kind='user' AND e.removed_at IS NULL
              AND (e.expires_at IS NULL OR e.expires_at > $3)
              AND ((e.created_by=$1 AND e.holder_id=$2) OR (e.created_by=$2 AND e.holder_id=$1))
           UNION
           SELECT 'node', r.node_id FROM node_roles r
            WHERE r.subject_kind='user' AND r.removed_at IS NULL
              AND (r.expires_at IS NULL OR r.expires_at > $3)
              AND ((r.created_by=$1 AND r.subject_id=$2) OR (r.created_by=$2 AND r.subject_id=$1))
         ) SELECT kind, id FROM shared
            WHERE ($4::text IS NULL OR (kind,id) > ($4,$5::uuid))
            ORDER BY kind,id LIMIT $6",
    )
    .bind(reader)
    .bind(person)
    .bind(at)
    .bind(after.map(|(kind, _)| kind))
    .bind(after.map(|(_, id)| id))
    .bind(limit)
    .fetch_all(conn)
    .await?)
}
