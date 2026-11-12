//! Accepted administrator messages and their frozen membership audience.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use sqlx::FromRow;

// Both the preview count and the accepted snapshot use this exact membership-domain selector.
// The organization branch starts from membership rows, not every account in the installation.
const AUDIENCE: &str = "WITH audience AS (
    SELECT m.user_id AS id FROM org_members m JOIN nodes n ON n.id=m.domain_id
      WHERE $1::uuid IS NOT NULL AND n.deleted_at IS NULL
        AND (n.id=$1 OR n.path @> ARRAY[$1::uuid])
    UNION SELECT $2::uuid WHERE $1::uuid IS NOT NULL
    UNION SELECT u.id FROM users u WHERE $1::uuid IS NULL AND u.kind='human' AND u.status='active'
)";

#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct MessageRow {
    pub id: Id,
    pub org_id: Option<Id>,
    pub org_name: Option<String>,
    pub title: String,
    pub body: String,
    pub sender_id: Id,
    pub sender: Json,
    pub created_at: DateTime<Utc>,
    pub recipient_count: i64,
    pub delivery_job_id: Option<Id>,
}

/// Sent-list metadata never loads the Markdown bodies into a page.
#[derive(Debug, Clone, PartialEq, Eq, FromRow)]
pub struct SummaryRow {
    pub id: Id,
    pub org_id: Option<Id>,
    pub org_name: Option<String>,
    pub title: String,
    pub sender_id: Id,
    pub sender: Json,
    pub created_at: DateTime<Utc>,
    pub recipient_count: i64,
    pub delivery_job_id: Option<Id>,
}

/// Newest first within the exact issuing scope, with a stable identifier tiebreaker.
pub async fn sent_page(
    conn: &mut Conn,
    org: Option<Id>,
    before: Option<(DateTime<Utc>, Id)>,
    limit: i64,
) -> Result<Vec<SummaryRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,org_id,org_name,title,sender_id,sender,created_at,recipient_count,delivery_job_id
        FROM admin_messages WHERE org_id IS NOT DISTINCT FROM $1
          AND ($2::timestamptz IS NULL OR (created_at,id)<($2,$3::uuid))
        ORDER BY created_at DESC,id DESC LIMIT $4")
        .bind(org).bind(before.map(|(at,_)|at)).bind(before.map(|(_,id)|id)).bind(limit)
        .fetch_all(conn).await?)
}

/// Publication reloads just the originally emitted identities, never another cursor window.
pub async fn sent_by_ids(
    conn: &mut Conn,
    org: Option<Id>,
    ids: &[Id],
) -> Result<Vec<SummaryRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,org_id,org_name,title,sender_id,sender,created_at,recipient_count,delivery_job_id
        FROM admin_messages WHERE org_id IS NOT DISTINCT FROM $1 AND id=ANY($2)
        ORDER BY created_at DESC,id DESC")
        .bind(org).bind(ids).fetch_all(conn).await?)
}

pub async fn get(conn: &mut Conn, id: Id) -> Result<Option<MessageRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,org_id,org_name,title,body,sender_id,sender,created_at,recipient_count,delivery_job_id FROM admin_messages WHERE id=$1")
        .bind(id).fetch_optional(conn).await?)
}

/// The accepted message owns one durable notice job even if enqueue is retried internally.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<MessageRow>, DbError> {
    Ok(sqlx::query_as("SELECT id,org_id,org_name,title,body,sender_id,sender,created_at,recipient_count,delivery_job_id FROM admin_messages WHERE id=$1 FOR UPDATE")
        .bind(id).fetch_optional(conn).await?)
}

pub async fn attach_job(conn: &mut Conn, message: Id, job: Id) -> Result<(), DbError> {
    let changed = sqlx::query(
        "UPDATE admin_messages SET delivery_job_id=$2 WHERE id=$1 AND delivery_job_id IS NULL",
    )
    .bind(message)
    .bind(job)
    .execute(conn)
    .await?
    .rows_affected();
    if changed != 1 {
        return Err(DbError::Other(sqlx::Error::Protocol(
            "the accepted message lost its unique delivery job".into(),
        )));
    }
    Ok(())
}

/// Materialization and cursor progress are committed with these exact accepted people.
pub async fn delivered(
    conn: &mut Conn,
    message: Id,
    people: &[Id],
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    sqlx::query("UPDATE admin_message_recipients SET delivered_at=COALESCE(delivered_at,$3) WHERE message_id=$1 AND user_id=ANY($2)")
        .bind(message).bind(people).bind(at).execute(conn).await?;
    Ok(())
}

pub async fn insert(conn: &mut Conn, row: &MessageRow) -> Result<(), DbError> {
    sqlx::query("INSERT INTO admin_messages(id,org_id,org_name,title,body,sender_id,sender,created_at,recipient_count,delivery_job_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10)")
        .bind(row.id).bind(row.org_id).bind(&row.org_name).bind(&row.title).bind(&row.body)
        .bind(row.sender_id).bind(&row.sender).bind(row.created_at).bind(row.recipient_count).bind(row.delivery_job_id)
        .execute(conn).await?;
    Ok(())
}

/// Snapshot actual membership identities in SQL, including the sender, without an in-memory roster.
pub async fn capture_audience(
    conn: &mut Conn,
    message: Id,
    org: Option<Id>,
    sender: Id,
) -> Result<i64, DbError> {
    let count = sqlx::query(sqlx::AssertSqlSafe(format!(
        "{AUDIENCE} INSERT INTO admin_message_recipients(message_id,user_id) SELECT $3,id FROM audience ON CONFLICT DO NOTHING"
    )))
    .bind(org)
    .bind(sender)
    .bind(message)
    .execute(&mut *conn)
    .await?
    .rows_affected();
    let count = i64::try_from(count).map_err(|_| {
        DbError::Other(sqlx::Error::Protocol(
            "the message audience count is not representable".into(),
        ))
    })?;
    sqlx::query("UPDATE admin_messages SET recipient_count=$2 WHERE id=$1")
        .bind(message)
        .bind(count)
        .execute(conn)
        .await?;
    Ok(count)
}

/// A count for the send dialog, from the same actual audience as acceptance; not a reservation.
pub async fn audience_count(conn: &mut Conn, org: Option<Id>, sender: Id) -> Result<i64, DbError> {
    Ok(sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "{AUDIENCE} SELECT count(*) FROM audience"
    )))
    .bind(org)
    .bind(sender)
    .fetch_one(conn)
    .await?)
}

/// Durable recipients in immutable identifier order; no live membership expansion at delivery.
pub async fn recipient_page(
    conn: &mut Conn,
    message: Id,
    after: Option<Id>,
    limit: i64,
) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT user_id FROM admin_message_recipients WHERE message_id=$1 AND ($2::uuid IS NULL OR user_id>$2) ORDER BY user_id LIMIT $3")
        .bind(message).bind(after).bind(limit).fetch_all(conn).await?)
}

pub async fn is_recipient(conn: &mut Conn, message: Id, user: Id) -> Result<bool, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM admin_message_recipients WHERE message_id=$1 AND user_id=$2)",
    )
    .bind(message)
    .bind(user)
    .fetch_one(conn)
    .await?)
}
