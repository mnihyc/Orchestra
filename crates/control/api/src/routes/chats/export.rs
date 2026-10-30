//! A chat taken away as a file: its written form or its messages as records, and with its files an archive of them.
//!
//! The text is the one renderer's (`orch_engine::written`), the same that an agent reads, a copy holds and an
//! archive of a place has, so that a chat exported by itself is the very bytes of its file in an archive. Taking a
//! node's chat away asks for the Export row of its node (`chat.export`) as well as for reading it; being in a direct
//! or a group chat is what lets one take that away, and somebody who reads it as an admin, without being in it,
//! does not.

use super::{Work, denied_for, load, transaction};
use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use axum::body::Body;
use axum::response::Response;
use bytes::Bytes;
use futures::Stream;
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::exports::{ChatExportFiles, ChatExportGet, ChatExportGetQuery};
use orch_api_contract::threads::ExportFormat;
use orch_authz::{Act, Policy};
use orch_chat::chats::Mode;
use orch_db::DbError;
use orch_db::audit::{Actor, Entry as AuditEntry, append};
use orch_engine::written::{self, Detail};
use serde_json::json;
use std::path::PathBuf;
use std::pin::Pin;
use std::task::{Context, Poll};
use tokio_util::io::ReaderStream;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_raw(ChatExportGet, export);
}

/// A file that is streamed to the caller and removed when the answer is over, however it ends.
struct Streamed {
    inner: ReaderStream<tokio::fs::File>,
    path: PathBuf,
}

impl Stream for Streamed {
    type Item = std::io::Result<Bytes>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        Pin::new(&mut self.inner).poll_next(cx)
    }
}

impl Drop for Streamed {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// A name made safe for a file: letters, digits and hyphens as they are, anything else an underscore.
fn file_name(name: &str) -> String {
    let safe: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if safe.trim_matches('_').is_empty() {
        "chat".to_owned()
    } else {
        safe
    }
}

/// Exports a chat.
async fn export(ctx: Ctx, query: ChatExportGetQuery, _body: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let mut tx = transaction(&ctx).await?;
    let work = Work::begin(&ctx, &mut tx).await?;
    let loaded = load(&mut tx, &work, id, Mode::Read).await?;
    // Taking a chat away: a node's takes the Export row of its node; a direct or a group chat, being in it.
    if loaded.row.is_people_chat() {
        if !loaded.standing.export {
            return Err(denied_for(
                Some(id),
                "chat.export",
                "a direct or a group chat is taken away as a file by those who are in it",
            ));
        }
    } else if let Some(node) = loaded.row.node_id {
        authorization::require(
            &mut tx,
            &work.principal,
            node,
            exact(Permission::ChatExport),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let format = query.format.unwrap_or(ExportFormat::Markdown);
    let range = (query.from, query.to);
    let with_files = query.files == Some(ChatExportFiles::Include);
    let title = written::chat_title(&mut tx, id)
        .await
        .map_err(ApiError::internal)?;
    let stem = file_name(title.strip_prefix("Chat of ").unwrap_or(&title));
    let mut response = if with_files {
        let (path, size) = ctx
            .app
            .engine
            .chat_with_files(&mut tx, id, format, range)
            .await
            .map_err(ApiError::internal)?;
        let file = match tokio::fs::File::open(&path).await {
            Ok(file) => file,
            Err(error) => {
                let _ = std::fs::remove_file(&path);
                return Err(ApiError::internal(error));
            }
        };
        let streamed = Streamed {
            inner: ReaderStream::new(file),
            path,
        };
        let mut response = Response::new(Body::from_stream(streamed));
        response.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("application/zip"),
        );
        response
            .headers_mut()
            .insert(header::CONTENT_LENGTH, HeaderValue::from(size));
        attach(&mut response, &format!("{stem}-chat.zip"));
        response
    } else {
        let (body, media_type, extension) = match format {
            ExportFormat::Markdown => {
                let read = written::read_chat(&mut tx, id, &title, range, Detail::Normal, None)
                    .await
                    .map_err(ApiError::internal)?;
                (read.text.into_bytes(), "text/markdown; charset=utf-8", "md")
            }
            ExportFormat::Json => {
                let messages = written::chat_records(&mut tx, id, range)
                    .await
                    .map_err(ApiError::internal)?;
                (
                    serde_json::to_vec(&messages).map_err(ApiError::internal)?,
                    "application/json",
                    "json",
                )
            }
        };
        let mut response = Response::new(Body::from(body));
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(media_type));
        attach(&mut response, &format!("{stem}-chat.{extension}"));
        response
    };
    *response.status_mut() = StatusCode::OK;
    let who = crate::routes::nodes::actor_name(&ctx)?;
    let summary = format!("{who} exported a chat.");
    let mut audit = AuditEntry::new(
        Actor::User(work.principal.user()),
        orch_db::audit::action!(AuditAction::ChatExported).as_str(),
        "chat",
        Some(id),
        &summary,
    );
    audit.node_id = loaded.row.node_id;
    audit.org_id = Some(loaded.row.org_id);
    audit.details = json!({
        "format": format,
        "files": with_files,
        "from": query.from,
        "to": query.to,
    });
    append(&mut tx, &audit, &work.origin, ctx.now).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(response)
}

/// Says the answer is a file to be saved under `name`.
fn attach(response: &mut Response, name: &str) {
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, disposition);
    }
}
