//! A completed group move is an accepted receipt for its actual original Inbox recipients.
use super::*;
use orch_types::Json;
use serde::{Deserialize, Serialize};

/// Only factual identities; no current group membership or content is carried by this receipt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChatMove {
    /// The group ended by the same accepted act.
    pub source: Id,
    /// The independent group created by that act.
    pub created: Id,
}

/// The accepted new thread and actual native job, with no copied conversation content.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadMove {
    /// Original thread ended by the same transaction.
    pub source: Id,
    /// Actual destination created by the native fork job.
    pub created: Id,
    /// The actual native job and stable receipt identity.
    pub job: Id,
}

/// Compatibility entry for the original completed group move source.
pub async fn chat_move_readable(
    conn: &mut Conn,
    receiver: Id,
    kind: &str,
    data: &Json,
) -> Result<bool, DbError> {
    if data.get("completed_chat_move").is_none() {
        return Ok(false);
    }
    move_readable(conn, receiver, kind, data).await
}

/// Current personal receipt readability shared by native move capture, Inbox and delivery.
/// The public payload alone proves nothing: exact private capture, original recipient,
/// actual immutable Inbox row and completed original ending all remain required.
pub async fn move_readable(
    conn: &mut Conn,
    receiver: Id,
    kind: &str,
    data: &Json,
) -> Result<bool, DbError> {
    if kind != "person.notify"
        || data.get("completed_chat_move").is_some() && data.get("completed_thread_move").is_some()
    {
        return Ok(false);
    }
    let Some(operation) = data
        .get("operation_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
    else {
        return Ok(false);
    };
    let Some(saved) = super::get(conn, operation).await? else {
        return Ok(false);
    };
    let Ok(expected) =
        serde_json::from_value::<super::super::outbox::Snapshot>(saved["expected"].clone())
    else {
        return Ok(false);
    };
    let (source, created, selection, field, path, subject, thread) = if data
        .get("completed_chat_move")
        .is_some()
    {
        let Ok(receipt) = serde_json::from_value::<ChatMove>(saved["completed_chat_move"].clone())
        else {
            return Ok(false);
        };
        if data.get("completed_chat_move") != Some(&serde_json::json!(receipt)) {
            return Ok(false);
        }
        (
            receipt.source,
            receipt.created,
            "ChatMoveOut",
            "completed_chat_move",
            format!("/chat/{}", receipt.created),
            serde_json::json!({"kind":"Chat","subject":receipt.source}),
            false,
        )
    } else {
        let Ok(receipt) =
            serde_json::from_value::<ThreadMove>(saved["completed_thread_move"].clone())
        else {
            return Ok(false);
        };
        if receipt.job != operation
            || receipt.job.is_nil()
            || data.get("completed_thread_move") != Some(&serde_json::json!(receipt))
        {
            return Ok(false);
        }
        (
            receipt.source,
            receipt.created,
            "ThreadMoveOut",
            "completed_thread_move",
            format!("/thread/{}", receipt.created),
            serde_json::json!({"kind":"Object","subject":{"kind":"thread","id":receipt.source}}),
            true,
        )
    };
    if saved["producer"] != "explicit_person_notice"
        || saved["version"] != 1
        || saved["origin"]["operation"] != selection
        || expected.kind != kind
        || expected.data != notification_data(kind, data)
        || expected.id.is_some()
        || !expected.body.is_empty()
        || expected.node_id.is_some()
        || expected.thread_id.is_some()
        || expected.chat_id.is_some()
        || expected.approval_id.is_some()
        || source == created
        || source.is_nil()
        || created.is_nil()
        || saved["subject"] != subject
        || data.get("path").and_then(Json::as_str) != Some(path.as_str())
        || data.get(field) != saved.get(field)
    {
        return Ok(false);
    }
    let Ok(receivers) = serde_json::from_value::<Vec<(Id, Id)>>(saved["receivers"].clone()) else {
        return Ok(false);
    };
    let ids: Vec<_> = receivers
        .iter()
        .filter_map(|(user, inbox)| (*user == receiver).then_some(*inbox))
        .collect();
    if ids.len() != 1 {
        return Ok(false);
    }
    Ok(sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM notifications n WHERE n.id=$1 AND n.user_id=$2 AND n.kind=$3 AND n.data=$4 AND n.node_id IS NULL AND n.thread_id IS NULL AND n.chat_id IS NULL AND n.approval_id IS NULL AND n.title=$6 AND n.body=$7 AND n.created_at=$8) AND CASE WHEN $9::bool THEN NOT EXISTS(SELECT 1 FROM nodes WHERE id=$5 AND deleted_at IS NULL) ELSE NOT EXISTS(SELECT 1 FROM chats WHERE id=$5) END")
        .bind(ids[0]).bind(receiver).bind(kind).bind(notification_data(kind,data)).bind(source)
        .bind(&expected.title).bind(&expected.body).bind(expected.at.get()).bind(thread).fetch_one(conn).await?)
}
