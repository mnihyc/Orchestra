//! Immutable shared suppression and the actual evaluated own-author exception.
use crate::{Conn, DbError};
use chrono::{DateTime, Utc};
use orch_types::{Id, Json};
use std::collections::BTreeMap;
/// Capture once in the ending operation's transaction.
pub async fn record(
    conn: &mut Conn,
    kind: &str,
    subject: Id,
    thread: Id,
    user: Option<Id>,
    reason: &str,
    at: DateTime<Utc>,
) -> Result<(), DbError> {
    let (turn, item) = if kind == "turn" {
        (Some(subject), None)
    } else {
        (None, Some(subject))
    };
    sqlx::query("INSERT INTO notification_decisions(turn_id,item_id,thread_id,user_id,outcome,reason,evaluated_at) VALUES($1,$2,$3,$4,'suppressed',$5,$6) ON CONFLICT(subject_kind,subject_id,user_id) DO NOTHING").bind(turn).bind(item).bind(thread).bind(user).bind(reason).bind(at).execute(conn).await?;
    Ok(())
}
/// Subjects must already be selected by current history/side/inspection authority.
pub async fn own(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    kind: &str,
    subjects: &[Id],
) -> Result<BTreeMap<Id, Json>, DbError> {
    let rows:Vec<(Id,Json)>=sqlx::query_as("SELECT DISTINCT ON(subject_id) subject_id,jsonb_build_object('outcome',outcome,'reason',reason,'evaluated_at',evaluated_at) FROM notification_decisions WHERE thread_id=$3 AND subject_kind=$4 AND subject_id=ANY($1) AND (user_id=$2 OR user_id IS NULL) ORDER BY subject_id,user_id NULLS LAST").bind(subjects).bind(user).bind(thread).bind(kind).fetch_all(&mut *conn).await?;
    let mut found: BTreeMap<Id, Json> = rows.into_iter().collect();
    let ids = subjects.iter().map(ToString::to_string).collect::<Vec<_>>();
    let query = if kind == "turn" {
        "SELECT data->>'turn_id',max(created_at) FROM notifications WHERE user_id=$1 AND thread_id=$3 AND kind IN ('turn.completed','turn.failed') AND data->>'turn_id'=ANY($2) GROUP BY data->>'turn_id'"
    } else {
        "SELECT data->>'item_id',max(created_at) FROM notifications WHERE user_id=$1 AND thread_id=$3 AND kind='subagent.ended' AND data->>'item_id'=ANY($2) GROUP BY data->>'item_id'"
    };
    let notices: Vec<(String, DateTime<Utc>)> = sqlx::query_as(sqlx::AssertSqlSafe(query))
        .bind(user)
        .bind(ids)
        .bind(thread)
        .fetch_all(conn)
        .await?;
    for (id, at) in notices {
        if let Ok(id) = id.parse::<Id>() {
            let replace = found
                .get(&id)
                .and_then(|value| value.get("evaluated_at"))
                .and_then(Json::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .is_none_or(|old| old <= at);
            if replace {
                found.insert(
                    id,
                    serde_json::json!({"outcome":"eligible","reason":null,"evaluated_at":at}),
                );
            }
        }
    }
    Ok(found)
}
