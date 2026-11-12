//! A chat taken away as a file: its written form or its messages as records, and with its files an archive of them.
//!
//! The text is the one renderer's (`orch_engine::written`), the same that an agent reads, a copy holds and an
//! archive of a place has, so that a chat exported by itself is the very bytes of its file in an archive. Taking a
//! node's chat away asks for the Export row of its node (`chat.export`) as well as for reading it; being in a direct
//! or a group chat is what lets one take that away, and somebody who reads it as an admin, without being in it,
//! does not.

use super::denied_for;

use super::failed_projected;

use crate::middleware::authorization;

use crate::middleware::authorization::exact;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use axum::body::Body;

use axum::response::Response;

use bytes::Bytes;

use futures::Stream;

use http::HeaderValue;

use http::StatusCode;

use http::header;

use orch_access_contract::audit::AuditAction;

use orch_access_contract::permissions::Permission;

use orch_api_contract::exports::ChatExportFiles;

use orch_api_contract::exports::ChatExportGet;

use orch_api_contract::exports::ChatExportGetQuery;

use orch_api_contract::threads::ExportFormat;

use orch_authz::Act;

use orch_authz::Policy;

use orch_chat::chats::Mode;

use orch_db::DbError;

use orch_db::audit::Actor;

use orch_db::audit::Entry as AuditEntry;

use orch_db::audit::append;

use orch_engine::written;

use serde_json::json;

use std::path::PathBuf;

use std::pin::Pin;

use std::task::Context;

use std::task::Poll;

use tokio_util::io::ReaderStream;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_raw_read_with_operation(
        ChatExportGet,
        crate::key_operations::native::effects::prepare::<opening::Export>,
        export,
    );
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

/// Exports through the shared native preparation and its once-only supply/audit boundary.
async fn export(ctx: Ctx, query: ChatExportGetQuery, _body: Body) -> ApiResult<Response> {
    opening::read(ctx, query).await
}

async fn supply(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    prepared: &opening::Prepared,
) -> ApiResult<Response> {
    let id = prepared.row.id;
    let query = &prepared.query;
    let format = query.format.unwrap_or(ExportFormat::Markdown);
    let range = prepared.sources.range();
    let with_files = query.files == Some(ChatExportFiles::Include);
    let title = written::chat_title(conn, id)
        .await
        .map_err(ApiError::internal)?;
    let stem = file_name(title.strip_prefix("Chat of ").unwrap_or(&title));
    let mut response = if with_files {
        let (path, size) = ctx
            .app
            .engine
            .chat_with_files_with_names(conn, id, format, range, prepared.names.as_ref())
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
            inner: ReaderStream::with_capacity(file, opening::FRAME_BYTES),
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
                let read = written::read_chat_export(
                    conn,
                    &prepared.sources,
                    &title,
                    prepared.names.as_ref(),
                )
                .await
                .map_err(ApiError::internal)?;
                (read.text.into_bytes(), "text/markdown; charset=utf-8", "md")
            }
            ExportFormat::Json => {
                let messages =
                    written::chat_records_with_names(conn, id, range, prepared.names.as_ref())
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
    Ok(response)
}

async fn record(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    prepared: &opening::Prepared,
    before: impl FnOnce() -> ApiResult<()>,
) -> ApiResult<()> {
    let query = &prepared.query;
    let format = query.format.unwrap_or(ExportFormat::Markdown);
    let who = crate::routes::nodes::actor_name(ctx)?;
    let summary = format!("{who} exported a chat.");
    let mut audit = AuditEntry::new(
        Actor::User(principal.user()),
        orch_db::audit::action!(AuditAction::ChatExported).as_str(),
        "chat",
        Some(prepared.row.id),
        &summary,
    );
    audit.node_id = prepared.row.node_id;
    audit.org_id = Some(prepared.row.org_id);
    audit.details = json!({"format":format,"files":query.files==Some(ChatExportFiles::Include),"from":query.from,"to":query.to});
    before()?;
    append(conn, &audit, &crate::sign_in::origin(ctx), ctx.now).await?;
    Ok(())
}

/// Says the answer is a file to be saved under `name`.
fn attach(response: &mut Response, name: &str) {
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{name}\"")) {
        response
            .headers_mut()
            .insert(header::CONTENT_DISPOSITION, disposition);
    }
}

#[path = "export/opening.rs"]
pub mod opening;
