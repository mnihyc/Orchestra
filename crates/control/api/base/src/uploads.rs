//! Browser and machine uploads in chunks sized to the link, which go on where they stopped.
//!
//! Browser sessions capture a transfer policy, accept pieces at any in-bounds offset and acknowledge
//! only synced byte intervals. Identical retries are safe; conflicting overlap cannot replace bytes.
//! Completion requires complete coverage, then hashes the whole file before registration. The
//! machine receiver uses the same file writer and range ledger under its command authority.

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
                data: serde_json::json!({"chunk":value.chunk_bytes,"policy":value.policy,"expires":value.expires_at}),
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
            policy: serde_json::from_value(get("policy")?).map_err(ApiError::internal)?,
            expires_at: serde_json::from_value(get("expires")?).map_err(ApiError::internal)?,
        };
        crate::key_operations::publication::receipt(&ctx, &reply)?;
        Ok(reply)
    })
}

/// How long an upload in pieces has to be finished in: a day, as the design says; the collection of
/// content removes a session that ran out together with the file of its pieces.
pub const SESSION_LIFE: Duration = Duration::hours(24);

/// The most characters of the name of an upload.
pub const NAME_MAX_CHARS: usize = 255;

/// The longest media type that is taken as written: what a header may hold of one.
const MEDIA_TYPE_MAX_CHARS: usize = 127;

/// Resolves sizing once; incomplete uploads retain the accepted policy across changes.
pub async fn transfer_policy(
    conn: &mut orch_db::Conn,
) -> ApiResult<orch_settings_contract::transfers::TransferPolicy> {
    use orch_settings_contract::transfers::TransferPolicy;
    let mut values = std::collections::HashMap::new();
    for key in [
        "storage.transfer_initial_piece_bytes",
        "storage.transfer_initial_requests",
        "storage.transfer_min_piece_bytes",
        "storage.transfer_max_piece_bytes",
        "storage.transfer_target_window_ms",
        "storage.transfer_window_rtts",
        "storage.transfer_max_window_bytes",
        "storage.transfer_requests",
    ] {
        let view = orch_settings::resolve(conn, Place::System, key)
            .await
            .map_err(ApiError::internal)?;
        let value = view
            .value()
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| ApiError::internal("the transfer setting is not an integer"))?;
        values.insert(key, value);
    }
    let policy = TransferPolicy::from_values(|key| values.get(key).copied().unwrap_or(0));
    if !policy.valid() {
        return Err(ApiError::invalid(
            "storage",
            "transfer sizes must fit the window and byte counters",
        ));
    }
    Ok(policy)
}

/// The immutable sizing facts of this still-owned session, before reading a request body.
pub async fn piece_limit(ctx: &Ctx, id: Id) -> ApiResult<i64> {
    let mut conn = ctx.app.db.acquire().await?;
    let row = storage::session(&mut conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    check_session(&row, ctx.user()?.id(), ctx.app.clock.now())?;
    Ok(storage::session_policy(&mut conn, id)
        .await?
        .max_piece_bytes)
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

/// The answer to a conflicting retry or an invalid offset.
fn out_of_place(received: i64) -> ApiError {
    ApiError::new(
        ErrorCode::UploadOffset,
        "the piece conflicts with acknowledged bytes or has an invalid offset",
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
        updated_at: now,
        expires_at: now + SESSION_LIFE,
        completion: None,
    };
    let policy = transfer_policy(&mut tx).await?;
    storage::begin_session(&mut tx, &session).await?;
    storage::set_session_policy(&mut tx, session.id, policy).await?;
    orch_db::idempotency::creation::observe_with(&mut tx,Some(session.user_id),"upload_session",session.id,
        serde_json::json!({"chunk":policy.max_piece_bytes,"policy":policy,"expires":orch_types::Time::new(session.expires_at)})).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(UploadSessionCreateReply {
        upload_id: session.id,
        chunk_bytes: policy.max_piece_bytes,
        policy,
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
pub(crate) fn check_session_for(
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
        let prepared = &ready.value;
        // Concurrent progress or completion can change; admitted native identity and content cannot.
        if prepared.id != locked.id
            || prepared.user_id != locked.user_id
            || prepared.created_at != locked.created_at
            || prepared.size != locked.size
            || prepared.name != locked.name
            || prepared.media_type != locked.media_type
        {
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

/// Takes a piece at its announced offset, preserving acknowledged suffixes.
pub async fn put_piece(
    ctx: &Ctx,
    id: Id,
    offset: i64,
    piece: &[u8],
) -> ApiResult<UploadSessionPutReply> {
    let len = i64::try_from(piece.len()).unwrap_or(i64::MAX);
    let app = ctx.app.clone();
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let session = own_session(&mut tx, id, initial.user().id(), initial.now().get()).await?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    check_locked_session(&current, &guard, &session)?;
    let policy = storage::session_policy(&mut tx, id).await?;
    if len > policy.max_piece_bytes {
        return Err(too_large(policy.max_piece_bytes));
    }
    if offset < 0 {
        return Err(out_of_place(session.received));
    }
    let end = offset
        .checked_add(len)
        .ok_or_else(|| ApiError::invalid("offset", "the piece offset overflows"))?;
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
    let ranges = storage::session_ranges(&mut tx, id).await?;
    let file = prepare_piece(&app.blobs.part_of_session(id), offset)
        .await
        .map_err(ApiError::internal)?;
    // Opening the file may wait, but it has not truncated the existing prefix or written a new byte.
    let (current, guard) = transaction_guard::after_wait(&current, &mut tx).await?;
    check_locked_session(&current, &guard, &session)?;
    file.write_unreceived(piece, &ranges)
        .await
        .map_err(|error| {
            if error.kind() == std::io::ErrorKind::InvalidInput {
                out_of_place(session.received)
            } else {
                ApiError::internal(error)
            }
        })?;
    let received =
        storage::cover_session(&mut tx, id, offset, end, current.app.clock.now()).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(UploadSessionPutReply { received })
}

/// Ends an upload in pieces: the whole is hashed, put in the store and recorded, and the upload is
/// over.
pub async fn complete(ctx: &Ctx, id: Id) -> ApiResult<UploadedBlob> {
    let app = ctx.app.clone();
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let session = storage::session_with_receipt(&mut tx, id, true)
        .await?
        .ok_or_else(ApiError::not_found)?;
    check_session(&session, initial.user().id(), initial.now().get())?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &session)?;
    if let Some(reply) = completion(&session)? {
        tx.commit().await.map_err(DbError::from)?;
        crate::key_operations::publication::receipt(&ctx, &reply)?;
        return Ok(reply);
    }
    let ranges = storage::session_ranges(&mut tx, id).await?;
    let covered = session.size == 0
        || ranges.len() == 1 && ranges[0].start == 0 && ranges[0].len == session.size;
    if session.received != session.size || !covered {
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
    // Hashing and the store consume the native file once. Unreferenced staging bytes
    // grant no custody: restore the same caller after those waits before publishing it.
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    check_locked_session(&ctx, &guard, &session)?;
    let now = guard.now().get();
    app.blobs
        .register(&mut tx, BlobMeta { hash, size }, now)
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
    let reply = UploadedBlob {
        blob: hash,
        size,
        name: session.name.clone(),
        media_type: session
            .media_type
            .clone()
            .unwrap_or_else(|| crate::media::media_type(&session.name, &[]).to_owned()),
    };
    // Keep the completed receipt briefly; this grants no further piece-writing authority.
    sqlx::query(
        "UPDATE upload_sessions SET completed_at=$2,expires_at=$3,completion=$4 WHERE id=$1",
    )
    .bind(id)
    .bind(now)
    .bind(now + SESSION_LIFE)
    .bind(serde_json::to_value(&reply).map_err(ApiError::internal)?)
    .execute(&mut *tx)
    .await
    .map_err(DbError::from)?;
    tx.commit().await.map_err(DbError::from)?;
    discard(&part).await;
    Ok(reply)
}

/// Retains a whole upload and its uploader's authority in one guarded action.
pub async fn register_whole(ctx: &Ctx, meta: BlobMeta) -> ApiResult<()> {
    register_named(ctx, meta, None, None).await
}

/// A complete direct upload retains only its actual supplied safe metadata.
pub async fn register_named(
    ctx: &Ctx,
    meta: BlobMeta,
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

/// A finite historical receipt, independent of whether its temporary part file still exists.
pub(crate) fn completion(session: &SessionRow) -> ApiResult<Option<UploadedBlob>> {
    session
        .completion
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)
}
