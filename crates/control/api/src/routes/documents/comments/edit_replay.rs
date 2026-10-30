//! Exact modern comment edits reuse ordinary per-person request receipts.
//! The caller holds the current document scope and document row before these
//! helpers run, and has checked current View, authorship and archive state.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::nodes::invalid;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::documents::DocumentCommentUpdateBody;
use orch_db::Conn;
use orch_db::idempotency::{self, Replay};
use orch_db::queries::documents::discussions::CommentRow;
use orch_types::{Id, Seq};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub(super) struct Attempt {
    client: Id,
    pub expected_seq: Seq,
    digest: String,
}

#[derive(Serialize, Deserialize)]
struct Recorded {
    digest: String,
    result: CommentRow,
}

pub(super) fn attempt(body: &DocumentCommentUpdateBody) -> ApiResult<Option<Attempt>> {
    let (client, expected_seq) = match (&body.client_id, body.expected_seq) {
        (None, None) => return Ok(None),
        (Some(client), Some(seq)) if seq >= 0 => (super::client_id(client)?, seq),
        (Some(_), Some(_)) => {
            return Err(invalid(
                "expected_seq",
                "must be a nonnegative comment revision",
            ));
        }
        _ => {
            return Err(invalid(
                "client_id",
                "client_id and expected_seq must be supplied together",
            ));
        }
    };
    // Hash the complete typed request, including fields normalization would
    // discard. A changed payload under a retained identity is never a retry.
    let bytes = serde_json::to_vec(&(&body.content, expected_seq)).map_err(ApiError::internal)?;
    Ok(Some(Attempt {
        client,
        expected_seq,
        digest: hex::encode(Sha256::digest(bytes)),
    }))
}

fn path(row: &CommentRow) -> String {
    format!("/documents/{}/comments/{}", row.document_id, row.id)
}

fn key(row: &CommentRow, attempt: &Attempt) -> String {
    format!(
        "document-comment-edit:{}:{}:{}",
        row.document_id, row.id, attempt.client
    )
}

fn since(ctx: &Ctx) -> DateTime<Utc> {
    i64::try_from(ctx.app.config.api.idempotency_ttl_hours)
        .ok()
        .and_then(Duration::try_hours)
        .and_then(|ttl| ctx.now.checked_sub_signed(ttl))
        .unwrap_or(DateTime::<Utc>::MIN_UTC)
}

pub(super) async fn find(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &CommentRow,
    attempt: &Attempt,
) -> ApiResult<Option<CommentRow>> {
    let Some(replay) =
        idempotency::find(conn, current.author_id, &key(current, attempt), since(ctx)).await?
    else {
        return Ok(None);
    };
    if replay.method != "PATCH" || replay.path != path(current) || replay.status != 200 {
        return Err(ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "the edit identity was used for another request",
        ));
    }
    let recorded: Recorded = serde_json::from_value(replay.response)
        .map_err(|_| ApiError::internal("a retained comment edit is not a canonical result"))?;
    if recorded.digest != attempt.digest {
        return Err(ApiError::new(
            ErrorCode::ConflictIdempotencyMismatch,
            "the edit identity was used with different content or base revision",
        ));
    }
    let row = recorded.result;
    if row.id != current.id
        || row.document_id != current.document_id
        || row.discussion_id != current.discussion_id
        || row.author_id != current.author_id
        || row.deleted_at.is_some()
    {
        return Err(ApiError::internal(
            "a retained comment edit belongs to a different source",
        ));
    }
    Ok(Some(row))
}

pub(super) async fn keep(
    ctx: &Ctx,
    conn: &mut Conn,
    row: &CommentRow,
    attempt: &Attempt,
) -> ApiResult<()> {
    let response = serde_json::to_value(Recorded {
        digest: attempt.digest.clone(),
        result: row.clone(),
    })
    .map_err(ApiError::internal)?;
    idempotency::keep(
        conn,
        row.author_id,
        &key(row, attempt),
        &Replay {
            method: "PATCH".to_owned(),
            path: path(row),
            status: 200,
            response,
        },
        ctx.now,
    )
    .await?;
    Ok(())
}
