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

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::middleware::transaction_guard;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::workspace::json_reply;

use crate::uploads;

use axum::body::Body;

use axum::response::IntoResponse;

use axum::response::Response;

use http::StatusCode;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::threads::ContentPart;

use orch_api_contract::threads::FILENAME_HEADER;

use orch_api_contract::threads::IMAGE_UPLOAD_MAX_BYTES;

use orch_api_contract::threads::ThreadAttachmentCreate;

use orch_api_contract::threads::ThreadBlobGet;

use orch_api_contract::threads::ThreadBlobGetQuery;

use orch_api_contract::threads::UploadBlobGet;

use orch_api_contract::threads::UploadBlobGetQuery;

use orch_api_contract::threads::UploadCreate;

use orch_api_contract::threads::UploadSessionComplete;

use orch_api_contract::threads::UploadSessionCreate;

use orch_api_contract::threads::UploadSessionCreateBody;

use orch_api_contract::threads::UploadSessionCreateReply;

use orch_api_contract::threads::UploadSessionPut;

use orch_api_contract::threads::UploadSessionPutQuery;

use orch_api_contract::threads::UploadedBlob;

use orch_api_contract::threads::UploadBlobDelete;

use orch_api_contract::threads::UploadBlobDeleteQuery;

use orch_api_contract::threads::UploadSessionDelete;

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
    personal_inventory::mount(registry);
    registry
        .creation(UploadCreate, raw_creation())
        .creation(ThreadAttachmentCreate, attachment::raw_creation())
        .creation(UploadSessionCreate, uploads::creation_binding())
        .on_raw(UploadCreate, create)
        .on_raw_read_with_optional_operation(
            UploadBlobGet,
            crate::key_operations::native::selected::<blob_source::Blobs>,
            upload_get,
        )
        .on(UploadSessionCreate, session_create)
        .on_raw_query_with_optional_operation(
            UploadSessionPut,
            crate::key_operations::native::stored::<uploads::session_operation::Sessions, _, _>,
            session_put,
        )
        .on_with_optional_operation(
            UploadSessionComplete,
            crate::key_operations::native::stored::<uploads::session_operation::Sessions, _, _>,
            session_complete,
        )
        .on_with_optional_operation(
            UploadSessionDelete,
            crate::key_operations::native::stored::<uploads::session_operation::Sessions, _, _>,
            session_delete,
        )
        .on_with_optional_operation(
            UploadBlobDelete,
            crate::key_operations::native::stored_query::<custody::Custody, _>,
            blob_delete,
        )
        .on_raw_staged_operation::<_, attachment::Attachment, _, _>(ThreadAttachmentCreate, attach)
        .on_raw_read_with_operation(ThreadBlobGet, super::read_operation::prepare, blob_get);
}

fn raw_creation() -> crate::middleware::idempotency::RawCreation<(), UploadedBlob> {
    crate::middleware::idempotency::RawCreation {
        kind: "uploaded_blob",
        input: |ctx| {
            Box::pin(async move {
                Ok(crate::middleware::idempotency::RawInputSpec {
                    limit: IMAGE_UPLOAD_MAX_BYTES,
                    format: crate::middleware::idempotency::RawFormat::Bytes,
                    fields: serde_json::json!({"name":filename_of(ctx),"media_type":content_type_of(ctx)}),
                })
            })
        },
        replay: |ctx, (), incoming, created| {
            Box::pin(async move {
                if created.id != ctx.user()?.id() {
                    return Err(ApiError::not_found());
                }
                let meta = incoming.meta();
                let blob =
                    serde_json::from_value(created.data.get("blob").cloned().ok_or_else(|| {
                        ApiError::internal("the accepted upload has no native hash")
                    })?)
                    .map_err(ApiError::internal)?;
                let size = created
                    .data
                    .get("size")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| {
                        ApiError::internal("the accepted upload has no native byte count")
                    })?;
                if blob != meta.hash || size != meta.size {
                    return Err(ApiError::internal(
                        "the supplied body does not match the native uploaded identity",
                    ));
                }
                let name = match incoming
                    .fields()
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                {
                    Some(name) => uploads::clean_name(FILENAME_HEADER, name)?,
                    None => "image".into(),
                };
                let media_type = incoming
                    .fields()
                    .get("media_type")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| ApiError::internal("the supplied upload has no media type"))?
                    .to_owned();
                let reply = UploadedBlob {
                    blob,
                    size,
                    name,
                    media_type,
                };
                crate::key_operations::publication::receipt(&ctx, &reply)?;
                Ok(reply)
            })
        },
    }
}

/// The name a request gives the file in its body: the header's text, with the escapes a browser
/// puts in a name that is not plain ASCII undone.
pub(crate) fn filename_of(ctx: &Ctx) -> Option<String> {
    let value = ctx.headers.get(FILENAME_HEADER)?;
    let text = String::from_utf8_lossy(value.as_bytes()).into_owned();
    Some(percent_decoded(&text))
}
orch_api_base::__api_bind_0658d2fd50!();

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
    uploads::register_named(&ctx, meta, Some(&name), Some(&media_type)).await?;
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
    crate::key_operations::publication::receipt(&ctx, &reply)?;
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
    let asked = custody::input(&ctx, query)?;
    let (mut tx, _initial) = transaction_guard::begin(&ctx).await?;
    // Retention takes the same lock, keeping another private copy safe while unused bytes are released.
    orch_db::queries::uploads::lock_blobs(&mut tx, &[asked.hash]).await?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let prepared = if crate::key_operations::native::modern(guard.principal().caller()) {
        Some(guard.operation_data::<crate::key_operations::native::Prepared<custody::Release>>()?)
    } else {
        None
    };
    let value = prepared.as_ref().map_or(&asked, |ready| &ready.value);
    if value.hash != asked.hash {
        return Err(ApiError::internal("the locked upload hash changed"));
    }
    custody::require(&ctx, guard.principal(), value)?;
    guard.check_deadline(&ctx)?;
    custody::release(&mut tx, value).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Attaches a file to a thread, from a body that has the file and the header that names it, or from
/// the hash of an upload that was ended.
async fn attach(
    ctx: Ctx,
    (): (),
    _input: std::sync::Arc<attachment::Input>,
) -> ApiResult<Response> {
    let (tx, _guard) = transaction_guard::begin(&ctx).await?;
    let completed =
        crate::key_operations::native::effects::apply_owned::<attachment::Attachment>(&ctx, tx)
            .await?;
    crate::key_operations::publication::receipt(&completed.ctx, &completed.output)?;
    json_reply(StatusCode::OK, &completed.output)
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

/// Serves an input or a declared tool output of this thread; unrelated stored hashes are not found.
async fn blob_get(ctx: Ctx, query: ThreadBlobGetQuery, _: Body) -> ApiResult<Response> {
    let thread = ctx.id("id")?;
    let hash: Hash = ctx
        .param("hash")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let mut conn = ctx.app.db.acquire().await?;
    super::read_operation::require_current(&ctx, &mut conn).await?;
    let carrying = storage::input_carrying(&mut conn, thread, &hash).await?;
    let described = carrying
        .clone()
        .and_then(|content| serde_json::from_value::<Vec<ContentPart>>(content).ok())
        .and_then(|parts| described_in(&parts, &hash));
    // Declared tool outputs belong to the conversation too; opaque arguments confer no access.
    let described = match described {
        Some(described) => Some(described),
        None => tool_outputs::in_thread(&mut conn, thread, hash).await?,
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
    let selected = crate::key_operations::native::selected_value::<blob_source::Blobs>(
        ctx.clone(),
        query.clone(),
        (),
    )
    .await?;
    blob_source::retain(&ctx, &selected)?;
    serve_completed(&ctx, selected.hash, selected.name.clone(), query).await
}

/// Both custody readers use the same preview, archive, range and raw-byte implementation.
/// Their caller has already retained its own native selected source in this context.
pub(crate) async fn serve_completed(
    ctx: &Ctx,
    hash: Hash,
    name: String,
    query: UploadBlobGetQuery,
) -> ApiResult<Response> {
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
        return crate::previews::serve(ctx, &hash, &name, media).await;
    }
    attachments::serve_kept(
        ctx,
        &hash,
        &name,
        media,
        &["image", "video", "audio"],
        query.download.unwrap_or(false),
    )
    .await
}
orch_api_base::__api_bind_02b2e50501!();

#[path = "uploads/attachment.rs"]
pub mod attachment;

#[path = "uploads/blob_source.rs"]
pub mod blob_source;

#[path = "uploads/custody.rs"]
pub mod custody;

#[path = "uploads/personal_inventory.rs"]
pub mod personal_inventory;

#[path = "uploads/tool_outputs.rs"]
pub mod tool_outputs;
