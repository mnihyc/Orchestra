//! Browser and machine uploads in chunks sized to the link, which go on where they stopped.
//!
//! A person's browser sends an image whole, or a file in pieces of any size up to four mebibytes: it
//! begins an upload (a row, with the size it is to have and a day to finish in), sends each piece at
//! the offset where the last one ended, and ends it, when the system hashes what arrived and puts it
//! in the store. A piece that begins anywhere else is refused with the number of bytes that have
//! arrived, which is how a client whose answer was lost finds out where to go on, and why it may
//! send pieces as small as its line carries well. The pieces of a machine's upload are taken by the
//! routes of the internal listener ([`crate::content`]); both leave their bytes in files of the
//! uploads directory ([`crate::blobs`]) until the whole is there.

use crate::blobs::{Blobs, BlobsError, discard, write_piece};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::Ctx;
use crate::state::AppState;
use chrono::{DateTime, Duration, Utc};
use orch_api_contract::common::{ErrorCode, UPLOAD_CHUNK_BYTES};
use orch_api_contract::threads::{
    UploadSessionCreateBody, UploadSessionCreateReply, UploadSessionPutReply, UploadedBlob,
};
use orch_blob_contract::range::BlobMeta;
use orch_db::DbError;
use orch_db::queries::storage::{self, SessionRow};
use orch_db::queries::uploads as references;
use orch_settings::Place;
use orch_types::{Id, new_id};

/// How long an upload in pieces has to be finished in: a day, as the design says; the collection of
/// content removes a session that ran out together with the file of its pieces.
pub const SESSION_LIFE: Duration = Duration::hours(24);

/// The most characters of the name of an upload.
pub const NAME_MAX_CHARS: usize = 255;

/// The longest media type that is taken as written: what a header may hold of one.
const MEDIA_TYPE_MAX_CHARS: usize = 127;

/// The size of a piece, as the contract states it.
#[must_use]
pub fn chunk_bytes() -> i64 {
    i64::try_from(UPLOAD_CHUNK_BYTES).unwrap_or(i64::MAX)
}

/// The most bytes the system stores in one file: the setting of the system, or what it ships with.
pub async fn max_blob_bytes(app: &AppState) -> ApiResult<i64> {
    let mut conn = app.db.acquire().await?;
    Ok(
        orch_settings::get::<i64>(&mut conn, Place::System, crate::content::MAX_BLOB_KEY)
            .await
            .ok()
            .flatten()
            .unwrap_or(crate::content::DEFAULT_MAX_BLOB_BYTES),
    )
}

/// A name as it is kept: without the space around it, neither empty nor longer than may be.
pub fn clean_name(path: &str, name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.chars().any(char::is_control) {
        return Err(ApiError::invalid(
            path,
            "it needs a name without control characters",
        ));
    }
    if name.chars().count() > NAME_MAX_CHARS {
        return Err(ApiError::invalid(
            path,
            &format!("a name has at most {NAME_MAX_CHARS} characters"),
        ));
    }
    Ok(name.to_owned())
}

/// The answer to a piece that does not begin where the last one ended.
fn out_of_place(received: i64) -> ApiError {
    ApiError::new(
        ErrorCode::UploadOffset,
        "the piece does not begin where the last one ended",
    )
    .with("received", received)
}

fn too_large(limit: i64) -> ApiError {
    ApiError::new(
        ErrorCode::RequestTooLarge,
        "that is larger than may be stored",
    )
    .with("limit", limit)
}

/// Why content could not be put in the store, as the problem a client acts on.
pub fn stored_badly(error: BlobsError) -> ApiError {
    match error {
        BlobsError::TooLarge { limit } => too_large(limit),
        other => ApiError::internal(other),
    }
}

/// Begins an upload in pieces for `user`.
pub async fn begin(
    ctx: &Ctx,
    body: &UploadSessionCreateBody,
) -> ApiResult<UploadSessionCreateReply> {
    let name = clean_name("name", &body.name)?;
    if body.size < 0 {
        return Err(ApiError::invalid("size", "a size is not negative"));
    }
    let max = max_blob_bytes(&ctx.app).await?;
    if body.size > max {
        return Err(too_large(max));
    }
    let media_type = match &body.media_type {
        Some(text) => {
            let text = text.trim();
            let looks_right = text.len() <= MEDIA_TYPE_MAX_CHARS
                && text.split_once('/').is_some_and(|(kind, subtype)| {
                    !kind.is_empty()
                        && !subtype.is_empty()
                        && !text.chars().any(|c| c.is_control() || c.is_whitespace())
                });
            if !looks_right {
                return Err(ApiError::invalid(
                    "media_type",
                    "this is a type and a subtype",
                ));
            }
            Some(text.to_owned())
        }
        None => None,
    };
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let now = guard.now().get();
    let session = SessionRow {
        id: new_id(),
        user_id: guard.user().id(),
        name,
        media_type,
        size: body.size,
        received: 0,
        created_at: now,
        expires_at: now + SESSION_LIFE,
    };
    storage::begin_session(&mut tx, &session).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(UploadSessionCreateReply {
        upload_id: session.id,
        chunk_bytes: chunk_bytes(),
        expires_at: orch_types::Time::new(session.expires_at),
    })
}

/// The session `id` of `user`, locked, if it is theirs and has not run out.
async fn own_session(
    conn: &mut orch_db::Conn,
    id: Id,
    user: Id,
    now: DateTime<Utc>,
) -> ApiResult<SessionRow> {
    let session = storage::lock_session(conn, id)
        .await?
        .filter(|session| session.user_id == user)
        .ok_or_else(ApiError::not_found)?;
    if session.expires_at < now {
        return Err(ApiError::new(ErrorCode::Gone, "this upload ran out"));
    }
    Ok(session)
}

/// Cancels a person's session after any in-flight piece, then discards its temporary file.
pub async fn cancel(ctx: &Ctx, id: Id) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    own_session(&mut tx, id, guard.user().id(), guard.now().get()).await?;
    sqlx::query("UPDATE upload_sessions SET cancelled_at=$2 WHERE id=$1")
        .bind(id)
        .bind(guard.now().get())
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    discard(&ctx.app.blobs.part_of_session(id)).await;
    Ok(())
}

/// Takes the next piece of an upload.
pub async fn put_piece(
    ctx: &Ctx,
    id: Id,
    offset: i64,
    piece: &[u8],
) -> ApiResult<UploadSessionPutReply> {
    let len = i64::try_from(piece.len()).unwrap_or(i64::MAX);
    if len > chunk_bytes() {
        return Err(too_large(chunk_bytes()));
    }
    let app = &ctx.app;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let session = own_session(&mut tx, id, guard.user().id(), guard.now().get()).await?;
    if offset != session.received {
        return Err(out_of_place(session.received));
    }
    let end = session.received.saturating_add(len);
    if end > session.size {
        return Err(ApiError::invalid(
            "",
            "the piece reaches past the end of the upload",
        ));
    }
    app.blobs
        .make_uploads_dir()
        .await
        .map_err(ApiError::internal)?;
    write_piece(&app.blobs.part_of_session(id), session.received, piece)
        .await
        .map_err(ApiError::internal)?;
    storage::advance_session(&mut tx, id, end).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(UploadSessionPutReply { received: end })
}

/// Ends an upload in pieces: the whole is hashed, put in the store and recorded, and the upload is
/// over.
pub async fn complete(ctx: &Ctx, id: Id) -> ApiResult<UploadedBlob> {
    let app = &ctx.app;
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    let now = guard.now().get();
    let session = own_session(&mut tx, id, guard.user().id(), now).await?;
    if session.received != session.size {
        return Err(out_of_place(session.received));
    }
    let part = app.blobs.part_of_session(id);
    app.blobs
        .make_uploads_dir()
        .await
        .map_err(ApiError::internal)?;
    if session.size == 0 {
        // An empty file sent no piece, so there is no file of pieces: there is nothing to read.
        tokio::fs::write(&part, b"")
            .await
            .map_err(ApiError::internal)?;
    }
    let (hash, size) = Blobs::hash_file(&part).await.map_err(stored_badly)?;
    if size != session.size {
        return Err(ApiError::internal(format!(
            "the file of an upload holds {size} bytes and the upload {}",
            session.size
        )));
    }
    app.blobs
        .put_file(&part, hash, size)
        .await
        .map_err(stored_badly)?;
    app.blobs
        .register(&mut tx, BlobMeta { hash, size }, now)
        .await?;
    references::remember(&mut tx, guard.user().id(), &hash, now).await?;
    // A closed session keeps only a short-lived progress receipt, never upload authority or partial bytes.
    sqlx::query("UPDATE upload_sessions SET completed_at=$2,expires_at=$3 WHERE id=$1")
        .bind(id)
        .bind(now)
        .bind(now + SESSION_LIFE)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    discard(&part).await;
    let media_type = session
        .media_type
        .clone()
        .unwrap_or_else(|| crate::media::media_type(&session.name, &[]).to_owned());
    Ok(UploadedBlob {
        blob: hash,
        size,
        media_type,
        name: session.name,
    })
}

/// Retains a whole upload and its uploader's authority in one guarded action.
pub async fn register_whole(ctx: &Ctx, meta: BlobMeta) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    ctx.app
        .blobs
        .register(&mut tx, meta, guard.now().get())
        .await?;
    references::remember(&mut tx, guard.user().id(), &meta.hash, guard.now().get()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
