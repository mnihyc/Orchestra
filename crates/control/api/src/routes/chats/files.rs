//! The bytes of a chat's files: a file stored to be named by an `attachment` part, and an image or a file that a message carries, given back to whoever reads the chat.
//!
//! Storing a file asks what sending asks: whoever may send in the chat. The file is kept as a blob and
//! keeps its uploader's reference; a message or private draft pins it while it exists, and one
//! that is never sent goes with the grace of the store. Reading asks what reading the chat asks, and
//! finds the blob among the live messages or the caller's authorized private draft: an image or a
//! file of a message, or of a copy that a message holds. A message that was edited or deleted
//! holds its files no more. A copy made by forwarding is a part of the chat it was forwarded into, so
//! it is served from there to the people there, who need no access to where it came from.
//!
//! Pictures are shown inline; every other file is a download, under the type it was kept with.

use super::{Work, note_opened};
use crate::attachments;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::workspace::json_reply;
use axum::body::Body;
use axum::response::Response;
use http::StatusCode;
use orch_api_contract::chats::{
    ChatAttachmentBlobBody, ChatAttachmentCreate, ChatBlobGet, ChatBlobGetQuery, ChatPart,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::ImageMediaType;
use orch_db::queries::storage;
use orch_types::Hash;

mod attachment;
mod source;

/// The kinds of file a chat shows where it is read: pictures. Everything else it offers as a download.
const SHOWN_INLINE: [&str; 1] = ["image"];

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_raw_staged_operation::<_, attachment::Attachment, _, _>(ChatAttachmentCreate, attach)
        .on_raw_read_with_optional_operation(
            ChatBlobGet,
            crate::key_operations::native::selected::<source::Blobs>,
            blob_get,
        );
}

/// How a request that names a finished upload is read.
fn reference(bytes: &[u8]) -> Option<(Hash, String)> {
    serde_json::from_slice::<ChatAttachmentBlobBody>(bytes)
        .ok()
        .map(|body| (body.blob, body.name))
}

/// The refusal of a file that is larger than may be stored here.
fn too_large(limit: i64, by_request: bool) -> ApiError {
    if by_request {
        attachments::over_a_request(limit)
    } else {
        ApiError::new(
            ErrorCode::RequestTooLarge,
            format!("a file is at most {limit} bytes in this chat"),
        )
        .with("limit", limit)
    }
}

/// Stores a file for an `attachment` part, from a body that has the file and the header that names
/// it, or from the hash of an upload that was ended.
async fn attach(
    ctx: Ctx,
    (): (),
    _input: std::sync::Arc<attachment::Input>,
) -> ApiResult<Response> {
    let (tx, _guard) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let completed =
        crate::key_operations::native::effects::apply_owned::<attachment::Attachment>(&ctx, tx)
            .await?;
    crate::key_operations::publication::receipt(&completed.ctx, &completed.output)?;
    json_reply(StatusCode::OK, &completed.output)
}

/// What a message calls a blob it carries.
#[derive(PartialEq, Eq)]
struct Kept {
    name: String,
    media_type: String,
}

const fn image_type(kind: ImageMediaType) -> &'static str {
    match kind {
        ImageMediaType::Png => "image/png",
        ImageMediaType::Jpeg => "image/jpeg",
        ImageMediaType::Webp => "image/webp",
        ImageMediaType::Gif => "image/gif",
    }
}

/// What the parts of a message call the blob `hash`, if they hold it as an image or a file, or in a
/// copy of another message.
fn described_in(parts: &[ChatPart], hash: &Hash) -> Option<Kept> {
    parts.iter().find_map(|part| match part {
        ChatPart::PastedText(paste) if paste.blob == *hash => Some(Kept {
            name: paste.name.clone(),
            media_type: "text/plain".to_owned(),
        }),
        ChatPart::Image {
            blob,
            media_type,
            name,
        } if blob == hash => Some(Kept {
            name: name.clone(),
            media_type: image_type(*media_type).to_owned(),
        }),
        ChatPart::Attachment {
            blob,
            name,
            media_type,
            ..
        } if blob == hash => Some(Kept {
            name: name.clone(),
            media_type: attachments::plain_media_type(media_type, name),
        }),
        ChatPart::Forwarded(copy) => copy
            .images
            .iter()
            .find(|image| image.blob == *hash)
            .map(|image| Kept {
                name: image.name.clone(),
                media_type: attachments::plain_media_type(&image.media_type, &image.name),
            })
            .or_else(|| {
                copy.attachments
                    .iter()
                    .find(|file| file.blob == *hash)
                    .map(|file| Kept {
                        name: file.name.clone(),
                        media_type: attachments::plain_media_type(&file.media_type, &file.name),
                    })
            }),
        _ => None,
    })
}

/// Serves files from live messages to chat readers, and independently authorized private draft
/// files to their author. Other bytes in the store are not found.
async fn blob_get(ctx: Ctx, query: ChatBlobGetQuery, _: Body) -> ApiResult<Response> {
    let selected = crate::key_operations::native::selected_read::<source::Blobs>(
        ctx.clone(),
        query.clone(),
        (),
    )
    .await?;
    // The shared reader has committed its producing snapshot; subsequent checks
    // must use this association, never a new message or a different byte witness.
    ctx.retain_source(selected.witness.clone())?;
    let hash = selected.hash;
    let mut tx = ctx.app.db.begin().await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: selected.principal.clone(),
        origin: crate::sign_in::origin(&ctx),
        now: selected.principal.now().get(),
    };
    note_opened(&mut tx, &work, &selected.loaded).await?;
    let reference = if query.preview == Some(true) {
        if let Some((reference, message, carried)) = selected.reference() {
            sqlx::query("INSERT INTO chat_file_references (id,chat_id,message_id,carried) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                .bind(reference).bind(selected.loaded.row.id).bind(message).bind(carried)
                .execute(&mut *tx).await.map_err(orch_db::DbError::from)?;
            Some(reference)
        } else {
            None
        }
    } else {
        None
    };
    storage::touch_blob(&mut tx, &hash, work.now).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if query.preview == Some(true) {
        let mut response =
            crate::previews::serve(&ctx, &hash, &selected.kept.name, &selected.kept.media_type)
                .await?;
        if let Some(reference) = reference {
            response.headers_mut().insert(
                "x-orch-file-id",
                http::HeaderValue::from_str(&reference.to_string()).map_err(ApiError::internal)?,
            );
        }
        return Ok(response);
    }
    attachments::serve_kept(
        &ctx,
        &hash,
        &selected.kept.name,
        &selected.kept.media_type,
        &SHOWN_INLINE,
        query.download.unwrap_or(false),
    )
    .await
}
