//! A thread exported whole as JSON: the thread, its segments, its turns and its items, which is what a person keeps when they want the conversation outside Orch.
//!
//! Taking a conversation away asks for the thread's Export row (`thread.export`) and for reading it, which the row
//! does not give by itself; the thread's own chat, asked for with `include=chat`, asks for the chat's two rows
//! (`chat.view` and `chat.export`) as well, and a person who lacks one is refused instead of being given a file that
//! quietly lacks the chat. The written form of a conversation (Markdown, at three depths, with the thread's chat
//! after it when asked for) is served by [`super::written_export`]. Both are the very bytes that the archive of a
//! place holds for the same thread.

use super::{one, read_failed};
use crate::middleware::authorization::{self, exact};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{placed, record};
use axum::body::Body;
use axum::response::Response;
use http::{HeaderValue, StatusCode, header};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::threads::{
    ExportFormat, ExportInclude, ThreadExport, ThreadExportJson, ThreadExportQuery,
};
use orch_authz::{Act, Policy};
use orch_db::queries::chats as chat_rows;
use orch_engine::written;
use orch_types::Id;
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_raw(ThreadExport, export);
}

/// The kinds of line an export holds besides the conversation, as asked.
fn included(asked: Option<&str>) -> ApiResult<Vec<ExportInclude>> {
    let Some(asked) = asked else {
        return Ok(vec![ExportInclude::SubStreams]);
    };
    asked
        .split(',')
        .filter(|word| !word.is_empty())
        .map(|word| {
            serde_json::from_value(serde_json::Value::String(word.trim().to_owned())).map_err(
                |_| {
                    ApiError::invalid(
                        "include",
                        "is a list of `reasoning`, `sub_streams` and `chat`",
                    )
                },
            )
        })
        .collect()
}

/// What is asked before any file is made, besides the Export row the request declares: reading the thread, and for
/// its chat the two rows of the chat.
async fn require_to_take(ctx: &Ctx, thread: Id, with_chat: bool) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let mut needed = vec![Permission::ThreadView];
    if with_chat {
        needed.extend([Permission::ChatView, Permission::ChatExport]);
    }
    for permission in needed {
        authorization::require(
            &mut conn,
            &principal,
            thread,
            exact(permission),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    Ok(())
}

/// Writes the audit row of a thread that was taken away as a file.
async fn recorded(
    ctx: &Ctx,
    thread: Id,
    format: ExportFormat,
    query: &ThreadExportQuery,
    include: &[ExportInclude],
) -> ApiResult<()> {
    let mut conn = ctx.app.db.acquire().await?;
    let (node, _) = placed(&mut conn, thread).await?;
    let who = crate::routes::nodes::actor_name(ctx)?;
    record(
        ctx,
        &mut conn,
        orch_db::audit::action!(AuditAction::ThreadExported),
        node,
        &format!("{who} exported the conversation of a thread."),
        json!({
            "format": format,
            "detail": query.detail,
            "from": query.from,
            "to": query.to,
            "include": include,
        }),
    )
    .await
}

/// The thread, whole, as a file to keep.
async fn export(ctx: Ctx, query: ThreadExportQuery, _: Body) -> ApiResult<Response> {
    let id = ctx.id("id")?;
    let include = included(query.include.as_deref())?;
    let with_chat = include.contains(&ExportInclude::Chat);
    require_to_take(&ctx, id, with_chat).await?;
    let response = if query.format == ExportFormat::Markdown {
        super::written_export::markdown(&ctx, id, &query, &include).await?
    } else {
        records(&ctx, id, &query, &include).await?
    };
    recorded(&ctx, id, query.format, &query, &include).await?;
    Ok(response)
}

/// The thread as records: the file of the JSON format.
async fn records(
    ctx: &Ctx,
    id: Id,
    query: &ThreadExportQuery,
    include: &[ExportInclude],
) -> ApiResult<Response> {
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let data = ctx
        .app
        .engine
        .export_view_in(
            &mut conn,
            id,
            orch_engine::views::ExportSelection {
                range: (query.from, query.to),
                reasoning: include.contains(&ExportInclude::Reasoning),
                sub_streams: include.contains(&ExportInclude::SubStreams),
                continuation: query.continuation_id,
                revision: query.projection_revision,
            },
        )
        .await
        .map_err(read_failed)?;
    let thread = one(ctx, &mut conn, id).await?;
    let name = thread.base.name.clone();
    let chat_messages = if include.contains(&ExportInclude::Chat) {
        Some(match chat_rows::chat_of_node(&mut conn, id).await? {
            Some(chat) => written::chat_records(&mut conn, chat.id, (None, None))
                .await
                .map_err(ApiError::internal)?,
            None => Vec::new(),
        })
    } else {
        None
    };
    let body = serde_json::to_vec(&ThreadExportJson {
        projection: Some(data.projection),
        thread,
        segments: data.segments,
        turns: data.turns,
        items: data.items,
        chat_messages,
    })
    .map_err(ApiError::internal)?;
    conn.commit().await.map_err(orch_db::DbError::from)?;
    let mut response = Response::new(Body::from(body));
    *response.status_mut() = StatusCode::OK;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let safe = file_name(&name);
    if let Ok(disposition) = HeaderValue::from_str(&format!("attachment; filename=\"{safe}.json\""))
    {
        headers.insert(header::CONTENT_DISPOSITION, disposition);
    }
    Ok(response)
}

/// A name made safe for the name of a file: letters, digits and hyphens as they are, anything else an underscore.
pub(super) fn file_name(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}
