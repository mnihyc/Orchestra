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

use crate::blobs::Blobs;

use crate::blobs::BlobsError;

use crate::blobs::discard;

use crate::blobs::prepare_piece;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::state::AppState;

use chrono::DateTime;

use chrono::Duration;

use chrono::Utc;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::UPLOAD_CHUNK_BYTES;

use orch_api_contract::threads::UploadSessionCreateBody;

use orch_api_contract::threads::UploadSessionCreateReply;

use orch_api_contract::threads::UploadSessionPutReply;

use orch_api_contract::threads::UploadedBlob;

use orch_blob_contract::range::BlobMeta;

use orch_db::DbError;

use orch_db::queries::storage;

use orch_db::queries::storage::SessionRow;

use orch_db::queries::uploads as references;

use orch_settings::Place;

use orch_types::Id;

use orch_types::new_id;
pub fn creation_binding()
-> crate::middleware::idempotency::Creation<(), UploadSessionCreateBody, UploadSessionCreateReply> {
    crate::middleware::idempotency::Creation {
        kind: "upload_session",
        replay: creation_replay,
        identity: |value| {
            Ok(orch_db::idempotency::creation::Created {
                kind: "upload_session".into(),
                id: value.upload_id,
                data: serde_json::json!({"chunk":value.chunk_bytes,"expires":value.expires_at}),
            })
        },
    }
}

fn creation_replay(
    ctx: Ctx,
    (): (),
    _body: UploadSessionCreateBody,
    created: orch_db::idempotency::creation::Created,
) -> futures::future::BoxFuture<'static, ApiResult<UploadSessionCreateReply>> {
    Box::pin(async move {
        let get = |name: &str| {
            created.data.get(name).cloned().ok_or_else(|| {
                ApiError::internal("the accepted upload has incomplete acknowledgement facts")
            })
        };
        let reply = UploadSessionCreateReply {
            upload_id: created.id,
            chunk_bytes: serde_json::from_value(get("chunk")?).map_err(ApiError::internal)?,
            expires_at: serde_json::from_value(get("expires")?).map_err(ApiError::internal)?,
        };
        crate::key_operations::publication::receipt(&ctx, &reply)?;
        Ok(reply)
    })
}

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
    let _ = body;
    let tx = ctx.app.db.begin().await?;
    Ok(
        crate::key_operations::native::effects::apply_owned::<creation::Begin>(ctx, tx)
            .await?
            .output,
    )
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
        .ok_or_else(ApiError::not_found)?;
    check_session(&session, user, now)?;
    Ok(session)
}

/// The native owner/lifetime rule shared by preparation and the serialized writer.
pub(crate) fn check_session(session: &SessionRow, user: Id, now: DateTime<Utc>) -> ApiResult<()> {
    if session.user_id != user {
        return Err(ApiError::not_found());
    }
    if session.expires_at < now {
        return Err(ApiError::new(ErrorCode::Gone, "this upload ran out"));
    }
    Ok(())
}

/// Expiry ends completion and append authority; the same owner may still cancel the partial file.
pub fn check_session_for(
    ctx: &Ctx,
    session: &SessionRow,
    user: Id,
    now: DateTime<Utc>,
) -> ApiResult<()> {
    use orch_api_contract::{routes::Endpoint, threads::UploadSessionDelete};
    if ctx.route.name == UploadSessionDelete::ROUTE.name {
        return if session.user_id == user {
            Ok(())
        } else {
            Err(ApiError::not_found())
        };
    }
    check_session(session, user, now)
}

/// The guarded source must be the very row whose native writer lock is held.
fn check_locked_session(
    ctx: &Ctx,
    guard: &transaction_guard::Guard,
    locked: &SessionRow,
) -> ApiResult<()> {
    if crate::key_operations::native::modern(guard.principal().caller()) {
        let ready =
            guard.operation_data::<crate::key_operations::native::Prepared<SessionRow>>()?;
        if ready.value != *locked {
            return Err(ApiError::internal(
                "the locked upload session changed its prepared source",
            ));
        }
    }
    check_session_for(ctx, locked, guard.user().id(), guard.now().get())?;
    check_session_for(ctx, locked, guard.user().id(), ctx.app.clock.now())?;
    guard.check_deadline(ctx)
}

/// Cancels a person's session after any in-flight piece, then discards its temporary file.
pub async fn cancel(ctx: &Ctx, id: Id) -> ApiResult<()> {
    let _session = ctx.app.blobs.relays().session(id).await;
    let native = {
        let mut conn = ctx.app.db.acquire().await?;
        references::targets::of(&mut conn, id)
            .await?
            .is_some_and(|target| target.destination.relayed)
    };
    if native {
        return self::native::cancel(ctx, id).await;
    }
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let locked = storage::lock_session(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    check_session_for(ctx, &locked, initial.user().id(), initial.now().get())?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &locked)?;
    sqlx::query("UPDATE upload_sessions SET cancelled_at=$2 WHERE id=$1")
        .bind(id)
        .bind(guard.now().get())
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    discard(&ctx.app.blobs.part_of_session(locked.id)).await;
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
    let app = ctx.app.clone();
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let session = own_session(&mut tx, id, initial.user().id(), initial.now().get()).await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    check_locked_session(&current, &guard, &session)?;
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
    let file = prepare_piece(&app.blobs.part_of_session(id), session.received)
        .await
        .map_err(ApiError::internal)?;
    // Opening the file may wait, but it has not truncated the existing prefix or written a new byte.
    let (current, guard) = transaction_guard::after_wait(&current, &mut tx).await?;
    check_locked_session(&current, &guard, &session)?;
    file.write(piece).await.map_err(ApiError::internal)?;
    let at = current.app.clock.now();
    let expires = at
        .checked_add_signed(Duration::seconds(session.idle_secs))
        .unwrap_or(DateTime::<Utc>::MAX_UTC);
    references::targets::progress(&mut tx, id, end, at, expires).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(UploadSessionPutReply {
        received: end,
        expires_at: orch_types::Time::new(expires),
    })
}

/// Ends an upload in pieces: the whole is hashed, put in the store and recorded, and the upload is
/// over.
pub async fn complete(ctx: &Ctx, id: Id) -> ApiResult<UploadedBlob> {
    let app = ctx.app.clone();
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let session = own_session(&mut tx, id, initial.user().id(), initial.now().get()).await?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &session)?;
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
    let _publication = app
        .blobs
        .put_file(&part, hash, size)
        .await
        .map_err(stored_badly)?;
    // Hashing and the store consume the native file once. Unreferenced staging bytes
    // grant no custody: restore the same caller after those waits before publishing it.
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &session)?;
    let now = guard.now().get();
    app.blobs
        .register(&mut tx, &BlobMeta { hash, size }, now)
        .await?;
    // The rollbackable blob catalogue stage also takes the hash's native writer
    // lock (including a concurrent first insertion). Every custody writer takes
    // that lock before its reference; refresh after it before retaining any bytes.
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &session)?;
    let now = guard.now().get();
    references::remember_file(
        &mut tx,
        guard.user().id(),
        &hash,
        Some(&session.name),
        session.media_type.as_deref(),
        now,
    )
    .await?;
    // A closed session keeps only a short-lived progress receipt, never upload authority or partial bytes.
    sqlx::query("UPDATE upload_sessions SET completed_at=$2,expires_at=$3 WHERE id=$1")
        .bind(id)
        .bind(now)
        .bind(
            now.checked_add_signed(Duration::seconds(session.idle_secs))
                .unwrap_or(DateTime::<Utc>::MAX_UTC),
        )
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
        relay: None,
        blob: hash,
        size,
        media_type,
        name: session.name,
    })
}

/// Retains a whole upload and its uploader's authority in one guarded action.
pub async fn register_whole(ctx: &Ctx, meta: &BlobMeta) -> ApiResult<()> {
    register_named(ctx, meta, None, None).await
}

/// A complete direct upload retains only its actual supplied safe metadata.
pub async fn register_named(
    ctx: &Ctx,
    meta: &BlobMeta,
    name: Option<&str>,
    media_type: Option<&str>,
) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    ctx.app
        .blobs
        .register(&mut tx, meta, guard.now().get())
        .await?;
    let (_current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    if name.is_some() || media_type.is_some() {
        references::remember_file(
            &mut tx,
            guard.user().id(),
            &meta.hash,
            name,
            media_type,
            guard.now().get(),
        )
        .await?;
    } else {
        references::remember(&mut tx, guard.user().id(), &meta.hash, guard.now().get()).await?;
    }
    // The private upload's native key is (uploader, blob hash).
    orch_db::idempotency::creation::observe_with(
        &mut tx,
        Some(guard.user().id()),
        "uploaded_blob",
        guard.user().id(),
        serde_json::json!({"blob":meta.hash,"size":meta.size}),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

#[path = "uploads/session_operation.rs"]
pub mod session_operation;

mod creation;
pub use creation::Begin;

mod native;
mod native_command;

/// Takes a browser body after the session's current scope was prepared, with one reader at a time.
pub async fn put_body(
    ctx: &Ctx,
    id: Id,
    offset: i64,
    body: axum::body::Body,
) -> ApiResult<UploadSessionPutReply> {
    let _session = ctx.app.blobs.relays().session(id).await;
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let state = references::targets::state(&mut tx, id, false)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let target = references::targets::of(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    if target.is_some_and(|target| target.destination.relayed) {
        return native::put(ctx, id, offset, body, state.session.size).await;
    }
    let bytes = axum::body::to_bytes(
        body,
        usize::try_from(chunk_bytes()).unwrap_or(usize::MAX) + 1,
    )
    .await
    .map_err(|_| too_large(chunk_bytes()))?;
    put_piece(ctx, id, offset, &bytes).await
}

/// Reads the actual carrier once; pending native verification does not hold a browser request open.
pub async fn complete_session(
    ctx: &Ctx,
    id: Id,
) -> ApiResult<orch_api_contract::threads::UploadSessionCompletion> {
    let _session = ctx.app.blobs.relays().session(id).await;
    let native = {
        let mut conn = ctx.app.db.acquire().await?;
        references::targets::of(&mut conn, id)
            .await?
            .is_some_and(|target| target.destination.relayed)
    };
    if native {
        self::native::complete(ctx, id).await
    } else {
        complete(ctx, id)
            .await
            .map(|file| orch_api_contract::threads::UploadSessionCompletion::Ready { file })
    }
}
