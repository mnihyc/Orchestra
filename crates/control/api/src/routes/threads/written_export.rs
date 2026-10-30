//! A thread exported in its written form: the conversation as Markdown at one of three depths, the same text that an agent which may read the thread is given, and the thread's own chat after it when it is asked for.
//!
//! The text is the engine's (`orch_engine::written`): one renderer for what an agent reads, a download, a copy, an
//! export and a forward, so a person who exports a conversation holds the words that its readers were given. The
//! agent's reasoning is not part of it at any depth, whatever the query includes. What the person must hold to take
//! it away (reading the thread, the Export row, and for the chat its two rows) is asked before this is called.

use super::export::file_name;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_api_contract::threads::{ExportInclude, ThreadExportQuery};
use orch_db::queries::chats as chat_rows;
use orch_engine::written::{self, Detail};
use orch_types::Id;

/// The media type of what is served: Markdown, in UTF-8.
const MARKDOWN: &str = "text/markdown; charset=utf-8";

/// The conversation of `thread` in its written form, as a file to keep.
pub async fn markdown(
    ctx: &Ctx,
    thread: Id,
    query: &ThreadExportQuery,
    include: &[ExportInclude],
) -> ApiResult<Response> {
    let detail = Detail::from(query.detail.unwrap_or_default());
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let projection = orch_db::queries::thread_continuations::projection(
        &mut conn,
        thread,
        query.continuation_id,
    )
    .await?
    .ok_or_else(ApiError::not_found)?;
    if !projection.accepts_cursor(false, query.projection_revision) {
        return Err(super::read_failed(
            orch_engine::views::ReadError::ProjectionChanged(
                orch_engine::continuations::projection::scope(&projection),
            ),
        ));
    }
    let read = written::read_conversation_view(
        &mut conn,
        thread,
        projection.viewed_continuation_id,
        (query.from, query.to),
        detail,
        None,
        None,
    )
    .await
    .map_err(ApiError::internal)?
    .ok_or_else(ApiError::not_found)?;
    let mut text = read.text;
    if include.contains(&ExportInclude::Chat)
        && let Some(chat) = chat_rows::chat_of_node(&mut conn, thread).await?
    {
        let title = format!("Chat of {}", file_name_of(&mut conn, thread).await?);
        let chat_text = written::read_chat(&mut conn, chat.id, &title, (None, None), detail, None)
            .await
            .map_err(ApiError::internal)?
            .text;
        text.push('\n');
        text.push_str(&chat_text);
    }
    let name = file_name_of(&mut conn, thread).await?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    let mut response = Response::new(Body::from(text));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(MARKDOWN));
    if let Ok(disposition) =
        HeaderValue::from_str(&format!("attachment; filename=\"{}.md\"", file_name(&name)))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    Ok(response)
}

/// The name of the thread, which names the file.
async fn file_name_of(conn: &mut orch_db::Conn, thread: Id) -> ApiResult<String> {
    orch_db::queries::threads::node_name(conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)
}
