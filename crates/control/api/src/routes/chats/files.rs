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

use super::{Work, load, note_opened, transaction};
use crate::attachments::{self, Refused};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::workspace::json_reply;
use axum::body::Body;
use axum::response::Response;
use http::StatusCode;
use orch_api_contract::chats::{
    ChatAttachmentBlobBody, ChatAttachmentCreate, ChatAttachmentCreateReply, ChatBlobGet,
    ChatBlobGetQuery, ChatPart,
};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::threads::ImageMediaType;
use orch_chat::attachments::largest;
use orch_chat::chats::Mode;
use orch_db::DbError;
use orch_db::queries::{chats as rows, storage};
use orch_types::{Hash, Id};

/// The kinds of file a chat shows where it is read: pictures. Everything else it offers as a download.
const SHOWN_INLINE: [&str; 1] = ["image"];

/// Binds this module's requests.
pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_raw(ChatAttachmentCreate, attach)
        .on_raw(ChatBlobGet, blob_get);
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
async fn attach(ctx: Ctx, (): (), body: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    // Who may send is settled before a byte is read, and nothing is held while the body arrives.
    let largest = {
        let mut tx = transaction(&ctx).await?;
        let work = Work::begin(&ctx, &mut tx).await?;
        let loaded = load(&mut tx, &work, id, Mode::Read).await?;
        work.refuse(&mut tx, Some(id), loaded.ensure_send()).await?;
        largest(&mut tx, &loaded.row)
            .await
            .map_err(ApiError::internal)?
    };
    let taken = match attachments::take(&ctx, body, largest, reference).await {
        Ok(taken) => taken,
        Err(Refused::Other(error)) => return Err(error),
        Err(Refused::TooLarge { limit, by_request }) => return Err(too_large(limit, by_request)),
    };
    let described = attachments::describe(&ctx, &taken.meta, &taken.name).await?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    work.refuse(&mut tx, Some(id), loaded.ensure_send()).await?;
    // Uploading retains a reference before a message exists to pin the chat by FK.
    let parent: Option<Id> = sqlx::query_scalar("SELECT id FROM chats WHERE id = $1 FOR KEY SHARE")
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    if parent.is_none() {
        return Err(ApiError::not_found());
    }
    let limit = orch_chat::attachments::largest(&mut tx, &loaded.row)
        .await
        .map_err(ApiError::internal)?;
    if taken.meta.size > limit {
        return Err(too_large(limit, false));
    }
    attachments::retain(&ctx, &mut tx, &work.principal, &taken).await?;
    tx.commit().await.map_err(DbError::from)?;
    json_reply(
        StatusCode::OK,
        &ChatAttachmentCreateReply {
            blob: taken.meta.hash,
            name: taken.name,
            size: taken.meta.size,
            media_type: described.media_type,
        },
    )
}

/// What a message calls a blob it carries.
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
    let id = ctx.id("id")?;
    let hash: Hash = ctx
        .param("hash")?
        .parse()
        .map_err(|_| ApiError::not_found())?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    note_opened(&mut tx, &work, &loaded).await?;
    let carrying = rows::message_carrying(&mut tx, id, &hash).await?;
    let mut kept = carrying
        .clone()
        .and_then(|content| serde_json::from_value::<Vec<ChatPart>>(content).ok())
        .and_then(|parts| described_in(&parts, &hash));
    if kept.is_none() && loaded.ensure_send().is_ok() && !loaded.standing.inspects() {
        kept = rows::draft(&mut tx, work.principal.user(), id)
            .await?
            .and_then(|(content, _, _)| serde_json::from_value::<Vec<ChatPart>>(content).ok())
            .and_then(|parts| described_in(&parts, &hash));
        if kept.is_some() {
            attachments::require(&mut tx, &work.principal, &hash).await?;
        }
    }
    let kept = kept.ok_or_else(ApiError::not_found)?;
    let reference = if query.preview == Some(true) {
        match carrying {
            Some(content) => {
                let message: Option<orch_types::Id> = sqlx::query_scalar("SELECT id FROM chat_messages WHERE chat_id=$1 AND content=$2 AND deleted_at IS NULL ORDER BY seq_created LIMIT 1")
                    .bind(id).bind(&content).fetch_optional(&mut *tx).await.map_err(DbError::from)?;
                let parts: Vec<ChatPart> =
                    serde_json::from_value(content).map_err(ApiError::internal)?;
                let carried = crate::routes::documents::references::chat_files(parts)
                    .iter()
                    .position(|file| file.0 == hash)
                    .map(|at| at + 1);
                if let (Some(message), Some(carried)) = (message, carried) {
                    let slot = u64::try_from(carried).unwrap_or(u64::MAX).to_be_bytes();
                    let reference = orch_engine::ids::derived(
                        "shared.chat.file",
                        &[id.as_bytes(), message.as_bytes(), &slot],
                    );
                    sqlx::query("INSERT INTO chat_file_references (id,chat_id,message_id,carried) VALUES ($1,$2,$3,$4) ON CONFLICT DO NOTHING")
                        .bind(reference).bind(id).bind(message).bind(i32::try_from(carried).unwrap_or(i32::MAX)).execute(&mut *tx).await.map_err(DbError::from)?;
                    Some(reference)
                } else {
                    None
                }
            }
            None => None,
        }
    } else {
        None
    };
    storage::touch_blob(&mut tx, &hash, ctx.now).await?;
    tx.commit().await.map_err(DbError::from)?;
    if query.preview == Some(true) {
        let mut response =
            crate::previews::serve(&ctx, &hash, &kept.name, &kept.media_type).await?;
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
        &kept.name,
        &kept.media_type,
        &SHOWN_INLINE,
        query.download.unwrap_or(false),
    )
    .await
}
