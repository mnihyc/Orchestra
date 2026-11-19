//! One renewable Inbox item per person for a reviewed operation on the same resource set.

use super::people::{Announced, NewNotification, notification_data};
use crate::{Conn, DbError};
use orch_types::{Hash, Id};

/// Stores an immutable accepted choice once, shared by its independent children.
pub async fn save(
    conn: &mut Conn,
    id: Id,
    data: &orch_types::Json,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    sqlx::query("INSERT INTO notification_operations(id,data,created_at,updated_at) VALUES($1,$2,$3,$3) ON CONFLICT(id) DO NOTHING")
        .bind(id).bind(data).bind(at).execute(&mut *conn).await?;
    let current: orch_types::Json =
        sqlx::query_scalar("SELECT data FROM notification_operations WHERE id=$1")
            .bind(id)
            .fetch_one(conn)
            .await?;
    if current != *data {
        return Err(DbError::Rule {
            constraint: "operation_identity_changed".into(),
        });
    }
    Ok(())
}

/// Serializes completion writers of the same accepted operation.
pub async fn lock(conn: &mut Conn, id: Id) -> Result<Option<orch_types::Json>, DbError> {
    Ok(
        sqlx::query_scalar("SELECT data FROM notification_operations WHERE id=$1 FOR UPDATE")
            .bind(id)
            .fetch_optional(conn)
            .await?,
    )
}

/// A stable resource-set key, independent of the next operation's identifier or outcome.
pub fn group_key(family: &str, resources: &[(String, Id)]) -> String {
    let mut resources = resources.to_vec();
    resources.sort();
    resources.dedup();
    let text = resources
        .iter()
        .map(|(kind, id)| format!("{kind}:{id}"))
        .collect::<Vec<_>>()
        .join("\n");
    format!("operation:{family}:{}", Hash::of(text.as_bytes()))
}

/// Creates or renews one item, preserving its identity across later operations on the same things.
pub async fn publish(
    conn: &mut Conn,
    operation: Id,
    new: &NewNotification<'_>,
) -> Result<Option<Announced>, DbError> {
    let receiver:Option<i64>=sqlx::query_scalar("SELECT u.inbox_seq FROM users u LEFT JOIN notification_prefs p ON p.user_id=u.id WHERE u.id=$1 AND u.kind='human' AND u.status='active' AND NOT COALESCE(p.disabled_kinds ? $2,false) FOR UPDATE OF u").bind(new.user_id).bind(new.kind).fetch_optional(&mut *conn).await?;
    if receiver.is_none() {
        return Ok(None);
    }
    let incoming: i64 =
        sqlx::query_scalar("SELECT ordinal FROM notification_operations WHERE id=$1")
            .bind(operation)
            .fetch_one(&mut *conn)
            .await?;
    let existing:Option<(Id,i64,Option<i64>)>=sqlx::query_as("SELECT n.id,n.inbox_seq,o.ordinal FROM notifications n LEFT JOIN notification_operations o ON o.id=(n.data->>'operation_id')::uuid WHERE n.user_id=$1 AND n.dedupe_key=$2 FOR UPDATE OF n")
        .bind(new.user_id).bind(new.dedupe_key).fetch_optional(&mut *conn).await?;
    if let Some((_, _, Some(ordinal))) = existing
        && ordinal > incoming
    {
        return Ok(None);
    }
    let seq: i64 = sqlx::query_scalar(
        "UPDATE users SET inbox_seq=inbox_seq+1 WHERE id=$1 RETURNING inbox_seq",
    )
    .bind(new.user_id)
    .fetch_one(&mut *conn)
    .await?;
    let existing = existing.map(|row| row.0);
    let mut data = new.data.clone();
    data["operation_id"] = serde_json::json!(operation);
    let data = notification_data(new.kind, &data);
    let id = existing.unwrap_or_else(orch_types::new_id);
    sqlx::query("INSERT INTO notifications(id,user_id,kind,node_id,thread_id,approval_id,chat_id,title,body,data,dedupe_key,inbox_seq,created_at)
        VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13)
        ON CONFLICT(id) DO UPDATE SET kind=EXCLUDED.kind,node_id=EXCLUDED.node_id,thread_id=EXCLUDED.thread_id,
          approval_id=EXCLUDED.approval_id,chat_id=EXCLUDED.chat_id,title=EXCLUDED.title,body=EXCLUDED.body,data=EXCLUDED.data,
          inbox_seq=EXCLUDED.inbox_seq,created_at=EXCLUDED.created_at,read_at=NULL,read_batch_id=NULL")
        .bind(id).bind(new.user_id).bind(new.kind).bind(new.node_id).bind(new.thread_id).bind(new.approval_id).bind(new.chat_id)
        .bind(new.title).bind(new.body).bind(data).bind(new.dedupe_key).bind(seq).bind(new.at).execute(&mut *conn).await?;
    crate::sequences::notify_user(conn, new.user_id, seq).await?;
    Ok(Some(Announced { id, inbox_seq: seq }))
}

/// Active human receivers who have not switched this kind off in their own choices.
pub async fn receivers(conn: &mut Conn, users: &[Id], kind: &str) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar(
        "SELECT u.id FROM users u LEFT JOIN notification_prefs p ON p.user_id=u.id
       WHERE u.id=ANY($1) AND u.kind='human' AND u.status='active'
         AND NOT COALESCE(p.disabled_kinds ? $2,false) ORDER BY u.id",
    )
    .bind(users)
    .bind(kind)
    .fetch_all(conn)
    .await?)
}

/// Saves only identifiers privately; names are decided afresh under the reader's current access.
pub async fn preview(conn: &mut Conn, id: Id, users: &[Id]) -> Result<(), DbError> {
    preview_for(conn, id, users, "environment.attention").await
}

/// Another kind reuses the same count and people-list contract under its recipients' own choices.
pub async fn preview_for(conn: &mut Conn, id: Id, users: &[Id], kind: &str) -> Result<(), DbError> {
    sqlx::query(
        "INSERT INTO operation_recipient_previews(id,users,notification_kind) VALUES($1,$2,$3)",
    )
    .bind(id)
    .bind(users)
    .bind(kind)
    .execute(conn)
    .await?;
    Ok(())
}

/// People from a reviewed audience whose names their profile audience allows this reader to see.
pub async fn visible_names(conn: &mut Conn, reader: Id, users: &[Id]) -> Result<Vec<Id>, DbError> {
    Ok(sqlx::query_scalar("SELECT u.id FROM users u LEFT JOIN user_profiles p ON p.user_id=u.id
      WHERE u.id=ANY($2) AND (u.id=$1 OR COALESCE(p.audience,'system')='system'
        OR (p.audience='orgs' AND EXISTS(SELECT 1 FROM org_members a JOIN org_members b ON a.org_id=b.org_id
          WHERE a.user_id=$1 AND b.user_id=u.id)))
      ORDER BY u.id")
      .bind(reader).bind(users).fetch_all(conn).await?)
}

/// Confirmation cannot notify a different set of people than the one the actor reviewed.
pub async fn matches_preview(conn: &mut Conn, id: Id, users: &[Id]) -> Result<bool, DbError> {
    let reviewed: Option<Vec<Id>> =
        sqlx::query_scalar("SELECT users FROM operation_recipient_previews WHERE id=$1")
            .bind(id)
            .fetch_optional(conn)
            .await?;
    let mut users = users.to_vec();
    users.sort_unstable();
    users.dedup();
    Ok(reviewed.is_some_and(|mut reviewed| {
        reviewed.sort_unstable();
        reviewed.dedup();
        reviewed == users
    }))
}

/// The rolling explicit-notice total is one SQL aggregate, never an in-memory audit history.
pub async fn person_rate_used(
    conn: &mut Conn,
    sender: Id,
    root: Option<Id>,
    since: chrono::DateTime<chrono::Utc>,
) -> Result<i64, DbError> {
    let query = if root.is_some() {
        "SELECT LEAST(COALESCE(sum(person_notify_count),0),9223372036854775807)::bigint
         FROM audit_log WHERE actor_id=$1 AND org_id=$3 AND action='person.notified' AND at>$2"
    } else {
        "SELECT LEAST(COALESCE(sum(person_notify_count),0),9223372036854775807)::bigint
         FROM audit_log WHERE actor_id=$1 AND org_id IS NULL AND action='person.notified' AND at>$2 AND $3::uuid IS NULL"
    };
    Ok(sqlx::query_scalar(query)
        .bind(sender)
        .bind(since)
        .bind(root)
        .fetch_one(conn)
        .await?)
}

/// The earliest expiring prefix that admits the whole next act; only its timestamp leaves SQL.
pub async fn person_rate_release(
    conn: &mut Conn,
    sender: Id,
    root: Option<Id>,
    since: chrono::DateTime<chrono::Utc>,
    needed: i64,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, DbError> {
    let query = if root.is_some() {
        "SELECT at FROM (SELECT at,sum(person_notify_count) OVER (ORDER BY at,id) AS expired
         FROM audit_log WHERE actor_id=$1 AND org_id=$3 AND action='person.notified' AND at>$2) pending
         WHERE expired >= $4 ORDER BY at LIMIT 1"
    } else {
        "SELECT at FROM (SELECT at,sum(person_notify_count) OVER (ORDER BY at,id) AS expired
         FROM audit_log WHERE actor_id=$1 AND org_id IS NULL AND action='person.notified' AND at>$2 AND $3::uuid IS NULL) pending
         WHERE expired >= $4 ORDER BY at LIMIT 1"
    };
    Ok(sqlx::query_scalar(query)
        .bind(sender)
        .bind(since)
        .bind(root)
        .bind(needed)
        .fetch_optional(conn)
        .await?)
}
