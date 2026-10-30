//! Images and files sent from a browser, whole or in pieces, and a file attached to a thread so that it reaches the agent when the input is delivered.
//!
//! An image goes up with one request and comes back as a hash to name in an `image` part; a file
//! larger than one request carries goes up in pieces ([`crate::uploads`]) and is ended by a request
//! that makes it a blob. A file is attached to a thread from either: its bytes in the body, or the
//! hash of an upload that was ended. Attaching writes nothing into the workspace: the file is kept as
//! a blob, pinned so that it stays, and the path it will have is told; the engine writes it there
//! when the input is delivered, so that attaching needs no environment to be running. What a thread
//! carries comes back by its hash to whoever may read the thread: the images and files of its
//! inputs, forwarded ones included, with the type they were stored with; nothing else the store
//! holds is found through a thread.

use crate::attachments;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::workspace::json_reply;
use crate::uploads;
use axum::body::Body;
use axum::response::{IntoResponse, Response};
use http::StatusCode;
use orch_api_contract::common::{ErrorCode, MAX_BODY_BYTES, MAX_JSON_BYTES};
use orch_api_contract::threads::{
    ContentPart, FILENAME_HEADER, IMAGE_UPLOAD_MAX_BYTES, ThreadAttachmentBlobBody,
    ThreadAttachmentCreate, ThreadAttachmentCreateReply, ThreadBlobGet, ThreadBlobGetQuery,
    UploadBlobGet, UploadBlobGetQuery, UploadCreate, UploadSessionComplete, UploadSessionCreate,
    UploadSessionCreateBody, UploadSessionCreateReply, UploadSessionPut, UploadSessionPutQuery,
    UploadedBlob,
};
use orch_api_contract::threads::{UploadBlobDelete, UploadBlobDeleteQuery, UploadSessionDelete};
use orch_blob_contract::range::BlobMeta;
use orch_db::queries::storage;
use orch_types::Hash;

/// The kinds of image the agents take, as a request states them.
const IMAGE_TYPES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// Immutable bytes still require the containing thread's current authority on every read.
const AUTHORIZED_CACHE: &str = "private, no-cache";

/// Where a file attached to a thread will be written, below the thread's directory.
const ATTACHMENTS_DIR: &str = ".orch/attachments";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on_raw(UploadCreate, create)
        .on_raw(UploadBlobGet, upload_get)
        .on(UploadSessionCreate, session_create)
        .on_raw(UploadSessionPut, session_put)
        .on(UploadSessionComplete, session_complete)
        .on(UploadSessionDelete, session_delete)
        .on(UploadBlobDelete, blob_delete)
        .on_raw(ThreadAttachmentCreate, attach)
        .on_raw(ThreadBlobGet, blob_get);
}

/// The name a request gives the file in its body: the header's text, with the escapes a browser
/// puts in a name that is not plain ASCII undone.
pub(crate) fn filename_of(ctx: &Ctx) -> Option<String> {
    let value = ctx.headers.get(FILENAME_HEADER)?;
    let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
    Some(percent_decoded(&text))
}

fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let (Some(high), Some(low)) = (
                char::from(bytes[i + 1]).to_digit(16),
                char::from(bytes[i + 2]).to_digit(16),
            )
        {
            out.push(u8::try_from(high * 16 + low).unwrap_or(b'%'));
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The type a request's `Content-Type` states, without its parameters, in lower case.
fn content_type_of(ctx: &Ctx) -> Option<String> {
    let value = ctx.headers.get(http::header::CONTENT_TYPE)?.to_str().ok()?;
    Some(
        value
            .split(';')
            .next()
            .unwrap_or(value)
            .trim()
            .to_ascii_lowercase(),
    )
}

/// Uploads an image in one request.
async fn create(ctx: Ctx, (): (), body: Body) -> ApiResult<Response> {
    let media_type = content_type_of(&ctx)
        .filter(|found| IMAGE_TYPES.contains(&found.as_str()))
        .ok_or_else(|| {
            ApiError::invalid(
                "content-type",
                "an image is sent as image/png, image/jpeg, image/webp or image/gif",
            )
        })?;
    let name = match filename_of(&ctx) {
        Some(name) => uploads::clean_name(FILENAME_HEADER, &name)?,
        None => "image".to_owned(),
    };
    let meta = ctx
        .app
        .blobs
        .store_stream(body.into_data_stream(), IMAGE_UPLOAD_MAX_BYTES)
        .await
        .map_err(uploads::stored_badly)?;
    uploads::register_whole(&ctx, meta).await?;
    json_reply(
        StatusCode::OK,
        &UploadedBlob {
            blob: meta.hash,
            size: meta.size,
            media_type,
            name,
        },
    )
}

/// Begins an upload in pieces.
async fn session_create(
    ctx: Ctx,
    (): (),
    body: UploadSessionCreateBody,
) -> ApiResult<UploadSessionCreateReply> {
    uploads::begin(&ctx, &body).await
}

/// Takes the next piece of an upload in pieces.
async fn session_put(ctx: Ctx, query: UploadSessionPutQuery, body: Body) -> ApiResult<Response> {
    let piece = axum::body::to_bytes(
        body,
        usize::try_from(uploads::chunk_bytes()).unwrap_or(usize::MAX) + 1,
    )
    .await
    .map_err(|_| {
        ApiError::new(
            ErrorCode::RequestTooLarge,
            "a piece carries at most four mebibytes",
        )
        .with("limit", uploads::chunk_bytes())
    })?;
    let reply = uploads::put_piece(&ctx, ctx.id("upload_id")?, query.offset, &piece).await?;
    json_reply(StatusCode::OK, &reply)
}

/// Ends an upload in pieces.
async fn session_complete(ctx: Ctx, (): (), (): ()) -> ApiResult<UploadedBlob> {
    uploads::complete(&ctx, ctx.id("upload_id")?).await
}

async fn session_delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    uploads::cancel(&ctx, ctx.id("upload_id")?).await
}

async fn blob_delete(ctx: Ctx, query: UploadBlobDeleteQuery, (): ()) -> ApiResult<()> {
    let hash = ctx
        .param("hash")?
        .parse::<Hash>()
        .map_err(|_| ApiError::not_found())?;
    if query.thread.is_some() != query.path.is_some() {
        return Err(ApiError::invalid(
            "path",
            "an attachment release names both its thread and path",
        ));
    }
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    // Retention takes the same lock, keeping another private copy safe while unused bytes are released.
    orch_db::queries::uploads::lock_blobs(&mut tx, &[hash]).await?;
    let content = serde_json::json!([{ "blob": hash.to_string() }]);
    let images = serde_json::json!([{ "images": [{ "blob": hash.to_string() }] }]);
    let attachments = serde_json::json!([{ "attachments": [{ "blob": hash.to_string() }] }]);
    sqlx::query("DELETE FROM thread_attachment_uploads WHERE created_by=$1 AND blob=$2 AND thread_id=$3 AND path=$4")
        .bind(guard.user().id()).bind(hash.as_bytes().as_slice()).bind(query.thread).bind(query.path)
        .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
    sqlx::query(
        "DELETE FROM completed_uploads WHERE user_id=$1 AND hash=$2
      AND NOT EXISTS(SELECT 1 FROM thread_attachment_uploads WHERE created_by=$1 AND blob=$2)
      AND NOT EXISTS(SELECT 1 FROM drafts WHERE user_id=$1 AND (content @> $3 OR content @> $4 OR content @> $5))
      AND NOT EXISTS(SELECT 1 FROM chat_states WHERE user_id=$1 AND (draft @> $3 OR draft @> $4 OR draft @> $5))",
    )
    .bind(guard.user().id())
    .bind(hash.as_bytes().as_slice())
    .bind(&content)
    .bind(&images)
    .bind(&attachments)
    .execute(&mut *tx)
    .await
    .map_err(orch_db::DbError::from)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Attaches a file to a thread, from a body that has the file and the header that names it, or from
/// the hash of an upload that was ended.
async fn attach(ctx: Ctx, (): (), body: Body) -> ApiResult<Response> {
    let supplied_bytes = filename_of(&ctx).is_some();
    let (meta, name, replace): (BlobMeta, String, Option<String>) = if let Some(name) =
        filename_of(&ctx)
    {
        let name = uploads::clean_name(FILENAME_HEADER, &name)?;
        let limit = i64::try_from(MAX_BODY_BYTES).unwrap_or(i64::MAX);
        let meta = ctx
            .app
            .blobs
            .store_stream(body.into_data_stream(), limit)
            .await
            .map_err(uploads::stored_badly)?;
        (meta, name, None)
    } else {
        let bytes = axum::body::to_bytes(body, MAX_JSON_BYTES)
            .await
            .map_err(|_| ApiError::new(ErrorCode::RequestTooLarge, "the body is too large"))?;
        let named: ThreadAttachmentBlobBody = serde_json::from_slice(&bytes).map_err(|_| {
            ApiError::invalid(
                "",
                "a file is sent as the body with its name in the header, or as the hash of an upload and a name",
            )
        })?;
        let name = uploads::clean_name("name", &named.name)?;
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        attachments::require(&mut tx, guard.principal(), &named.blob).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let size = ctx
            .app
            .blobs
            .store()
            .size(&named.blob)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
        (
            BlobMeta {
                hash: named.blob,
                size,
            },
            name,
            named.replace,
        )
    };
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    if !orch_db::queries::transaction_guard::lock_parent(&mut tx, ctx.id("id")?).await? {
        return Err(ApiError::not_found());
    }
    orch_db::queries::threads::lock(&mut tx, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    sqlx::query("DELETE FROM thread_attachment_uploads WHERE expires_at < $1")
        .bind(ctx.now)
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    attachments::retain(
        &ctx,
        &mut tx,
        guard.principal(),
        &attachments::Taken {
            meta,
            name: name.clone(),
            supplied_bytes,
        },
    )
    .await?;
    let owned: Option<String> = if let Some(path) = &replace {
        if !path.starts_with(".orch/attachments/") {
            return Err(ApiError::invalid(
                "replace",
                "an edit names its private attachment",
            ));
        }
        sqlx::query_scalar("SELECT name FROM thread_attachment_uploads WHERE thread_id=$1 AND path=$2 AND created_by=$3 AND blob IS NOT NULL FOR UPDATE")
            .bind(ctx.id("id")?).bind(path).bind(guard.user().id()).fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?
    } else {
        None
    };
    let name = if let Some(name) = owned {
        sqlx::query("UPDATE thread_attachment_uploads SET blob=$3,expires_at=$4 WHERE thread_id=$1 AND path=$2")
            .bind(ctx.id("id")?).bind(replace).bind(meta.hash.as_bytes().as_slice()).bind(ctx.now + uploads::SESSION_LIFE)
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        name
    } else {
        let used = orch_engine::paste::attachment_names(&mut tx, ctx.id("id")?)
            .await
            .map_err(ApiError::internal)?;
        let name = orch_engine::paste::available_name(&crate::media::safe_name(&name), &used);
        sqlx::query("INSERT INTO thread_attachment_uploads(thread_id,path,name,created_by,blob,expires_at) VALUES($1,$2,$3,$4,$5,$6)")
            .bind(ctx.id("id")?).bind(format!("{ATTACHMENTS_DIR}/{name}")).bind(&name).bind(guard.user().id()).bind(meta.hash.as_bytes().as_slice()).bind(ctx.now + uploads::SESSION_LIFE)
            .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
        name
    };
    // The private reservation owns a pin until it is sent, removed or expires.
    tx.commit().await.map_err(orch_db::DbError::from)?;
    json_reply(
        StatusCode::OK,
        &ThreadAttachmentCreateReply {
            path: format!("{ATTACHMENTS_DIR}/{name}"),
            name,
            size: meta.size,
            blob: meta.hash,
        },
    )
}

// ---------------------------------------------------------------- what a thread carries

/// What the parts of an input call the blob `hash` and what type they stored it with, if they hold it.
fn described_in(content: &[ContentPart], hash: &Hash) -> Option<(String, Option<String>)> {
    content.iter().find_map(|part| match part {
        ContentPart::PastedText(paste) if paste.blob == *hash => {
            Some((paste.name.clone(), Some("text/plain".to_owned())))
        }
        ContentPart::Image {
            blob,
            name,
            media_type,
            ..
        } if blob == hash => {
            let stored = serde_json::to_value(media_type)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned));
            Some((name.clone(), stored))
        }
        ContentPart::File {
            blob: Some(blob),
            name,
            ..
        } if blob == hash => Some((name.clone(), None)),
        ContentPart::Forwarded(forwarded) => forwarded
            .images
            .iter()
            .find(|image| image.blob == *hash)
            .map(|image| (image.name.clone(), Some(image.media_type.clone())))
            .or_else(|| {
                forwarded
                    .attachments
                    .iter()
                    .find(|file| file.blob == *hash)
                    .map(|file| (file.name.clone(), Some(file.media_type.clone())))
            }),
        _ => None,
    })
}

/// Serves an image or a file that an input of the thread carries, by its hash, with the type it was
/// stored with; a blob that no input of the thread carries is not found, though the store holds it.
async fn blob_get(ctx: Ctx, query: ThreadBlobGetQuery, _: Body) -> ApiResult<Response> {
    let thread = ctx.id("id")?;
    let hash: Hash = ctx
        .param("hash")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let mut conn = ctx.app.db.acquire().await?;
    let carrying = storage::input_carrying(&mut conn, thread, &hash).await?;
    let described = carrying
        .clone()
        .and_then(|content| serde_json::from_value::<Vec<ContentPart>>(content).ok())
        .and_then(|parts| described_in(&parts, &hash));
    // Tool images belong to the conversation too; arbitrary hashes in tool arguments confer no access.
    let described = match described {
        Some(described) => Some(described),
        None => {
            let item: Option<serde_json::Value> = sqlx::query_scalar(
                "SELECT payload->'tool' FROM items WHERE thread_id=$1 AND kind='tool_call' AND payload->'tool'->>'kind'='image' AND payload->'tool'->>'blob'=$2 LIMIT 1",
            ).bind(thread).bind(hash.to_string()).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
            item.map(|tool| {
                (
                    tool.get("name")
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or("image")
                        .to_owned(),
                    tool.get("media_type")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned),
                )
            })
        }
    };
    let delivered_path = if query.preview == Some(true) {
        if let Some(content) = &carrying {
            let input: Option<orch_types::Id> = sqlx::query_scalar("SELECT id FROM thread_inputs WHERE thread_id=$1 AND content=$2 ORDER BY seq_created LIMIT 1")
                .bind(thread).bind(content).fetch_optional(&mut *conn).await.map_err(orch_db::DbError::from)?;
            let parts: Vec<ContentPart> =
                serde_json::from_value(content.clone()).map_err(ApiError::internal)?;
            input.and_then(|input| {
                orch_engine::delivery::plan(input, &parts)
                    .into_iter()
                    .find(|file| file.blob == hash)
                    .map(|file| file.path)
            })
        } else {
            None
        }
    } else {
        None
    };
    let (name, stored) = if let Some(described) = described {
        storage::touch_blob(&mut conn, &hash, ctx.now).await?;
        drop(conn);
        described
    } else {
        drop(conn);
        let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
        let allowed = orch_authz::nodes::check(
            &mut tx,
            guard.principal(),
            thread,
            orch_access_contract::permissions::Permission::ThreadInteract,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?;
        if !matches!(
            allowed,
            orch_access_contract::decisions::Decision::Allowed { .. }
        ) || !orch_db::queries::uploads::owns(&mut tx, guard.user().id(), &hash).await?
        {
            return Err(ApiError::not_found());
        }
        let carried = orch_db::queries::threads::draft(&mut tx, guard.user().id(), thread)
            .await?
            .and_then(|draft| serde_json::from_value::<Vec<ContentPart>>(draft.content).ok())
            .and_then(|parts| described_in(&parts, &hash))
            .ok_or_else(ApiError::not_found)?;
        storage::touch_blob(&mut tx, &hash, guard.now().get()).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        carried
    };
    let media = match stored.as_deref() {
        Some(stored) if !stored.is_empty() => stored.to_owned(),
        _ => crate::media::media_type(&name, &[]).to_owned(),
    };
    if query.preview == Some(true) {
        let mut response = crate::previews::serve(&ctx, &hash, &name, &media).await?;
        if let Some(path) = delivered_path {
            let encoded = form_urlencoded::byte_serialize(path.as_bytes()).collect::<String>();
            response.headers_mut().insert(
                "x-orch-file-path",
                http::HeaderValue::from_str(&encoded).map_err(ApiError::internal)?,
            );
        }
        return Ok(response);
    }
    let mut response = crate::routes::workspace::files::read_for(
        &ctx,
        &hash,
        &name,
        &media,
        query.download.unwrap_or(false),
    )
    .await?;
    // What a hash names never changes; who may read it can change.
    response.headers_mut().insert(
        http::header::CACHE_CONTROL,
        http::HeaderValue::from_static(AUTHORIZED_CACHE),
    );
    Ok(response)
}

/// A private draft reads only a hash the caller independently holds, never somebody else's unfinished input.
async fn upload_get(ctx: Ctx, query: UploadBlobGetQuery, _: Body) -> ApiResult<Response> {
    let hash: Hash = ctx
        .param("hash")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let name = uploads::clean_name("name", &query.name)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = crate::middleware::authorization::principal(&ctx, &mut conn).await?;
    attachments::require(&mut conn, &principal, &hash).await?;
    drop(conn);
    let media = crate::media::media_type(&name, &[]);
    if query.archive == Some(true) {
        let size = ctx
            .app
            .blobs
            .store()
            .size(&hash)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(ApiError::not_found)?;
        let meta = BlobMeta { hash, size };
        if let Some(entry) = query.entry {
            let bytes = crate::archives::read_entry(
                &ctx.app.blobs,
                meta,
                &entry,
                ctx.app.config.limits.preview_bytes,
                crate::archives::READ_TIME,
            )
            .await
            .map_err(|error| ApiError::new(ErrorCode::CapabilityUnsupported, error.to_string()))?;
            return Ok((
                [
                    (http::header::CONTENT_TYPE, "application/octet-stream"),
                    (http::header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response());
        }
        let list = crate::archives::list(
            &ctx.app.blobs,
            meta,
            query.inside.as_deref(),
            usize::try_from(orch_api_contract::workspace::ARCHIVE_LIST_LIMIT_DEFAULT)
                .unwrap_or(usize::MAX),
            crate::archives::READ_TIME,
        )
        .await
        .map_err(|error| ApiError::new(ErrorCode::CapabilityUnsupported, error.to_string()))?;
        return json_reply(StatusCode::OK, &list);
    }
    if query.preview == Some(true) {
        return crate::previews::serve(&ctx, &hash, &name, media).await;
    }
    attachments::serve_kept(
        &ctx,
        &hash,
        &name,
        media,
        &["image", "video", "audio"],
        query.download.unwrap_or(false),
    )
    .await
}
