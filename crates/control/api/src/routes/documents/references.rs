//! The card a reference's chip opens: what the reference names, and the part it names, as the caller may read them.
//!
//! A chip in a document, in a chat or in what an agent wrote names a thing (a place, a thread, a document, a chat)
//! and may name one of its parts (a block, a turn, a message). Its card asks here once, when it opens: the thing's
//! name, where it lies, the first line of what it is about, when it last changed and who changed it, and the part
//! itself. Each is read with the checks of its own reading: `doc.view` on a document, `node.view` on a place, also
//! `thread.view` for a turn, and the chat's own reading for a chat. A caller who may not read the thing is answered
//! as if nothing were there, with no word of what it is: their chip is plain text already.

use std::collections::HashMap;

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::documents::build::{self, Cast, Wanted};
use crate::routes::nodes::{invalid, view};
use chrono::{DateTime, Utc};
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::documents::{
    DOCUMENT_LINE_MAX_CHARS, InlineRefKind, REFERENCE_PREVIEW_TEXT_MAX_CHARS, RefPart,
    ReferencePreview, ReferencePreviewFile, ReferencePreviewGet, ReferencePreviewPart,
    ReferencePreviewQuery,
};
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::workspace::{DirEntryKind, FileStat};
use orch_authz::{Act, Policy, Principal};
use orch_blob_contract::range::ByteRange;
use orch_blob_contract::store::read_all;
use orch_db::queries::chats as chat_rows;
use orch_db::queries::documents::{self as rows, BlockRow};
use orch_db::queries::{hierarchy, shares as share_rows};
use orch_db::{Conn, DbError};
use orch_documents::describe;
use orch_documents::position::Position;
use orch_link_contract::commands::{FsFetch, FsStat, SerialKey};
use orch_types::Hash;
use orch_types::{Id, Time};

/// Binds this module's request.
pub fn mount(registry: &mut Registry) {
    registry.on(ReferencePreviewGet, preview);
}

/// What a card shows of a text: its first characters, with an ellipsis where there were more.
fn shown_text(text: &str) -> String {
    let most = usize::try_from(REFERENCE_PREVIEW_TEXT_MAX_CHARS).unwrap_or(usize::MAX);
    let flat = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if flat.chars().count() <= most {
        return flat;
    }
    let mut cut: String = flat.chars().take(most).collect();
    cut.push('…');
    cut
}

/// The card of a known reference, including its safe unavailable state.
async fn preview(ctx: Ctx, query: ReferencePreviewQuery, (): ()) -> ApiResult<ReferencePreview> {
    let mut shown = match readable_preview(&ctx, &query).await {
        Ok(shown) => shown,
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::NotFound => {
            unavailable_preview(&ctx, &query).await?
        }
        Err(error) => return Err(error),
    };
    let (kind, id) = if let Some(unavailable) = &shown.unavailable {
        let Some(object) = &unavailable.object else {
            return Ok(shown);
        };
        (object.kind.clone(), object.id)
    } else {
        match shown.kind {
            InlineRefKind::Node => ("node".to_owned(), shown.id),
            InlineRefKind::Document => ("document".to_owned(), shown.id),
            InlineRefKind::Chat => ("chat".to_owned(), shown.id),
            InlineRefKind::EnvironmentFile => ("environment".to_owned(), shown.id),
            InlineRefKind::File => match shown
                .file
                .as_ref()
                .and_then(|file| file.owner_id.zip(file.owner_kind))
            {
                Some((id, InlineRefKind::Document)) => ("document".to_owned(), id),
                Some((id, InlineRefKind::Chat)) => ("chat".to_owned(), id),
                _ => return Ok(shown),
            },
        }
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    if shown.unavailable.is_none() {
        let next: Option<DateTime<Utc>> = sqlx::query_scalar("SELECT min(expires_at) FROM permission_entries WHERE object_kind=$1 AND object_id=$2 AND holder_kind='user' AND holder_id=$3 AND removed_at IS NULL AND expires_at>$4")
            .bind(&kind).bind(id).bind(principal.user()).bind(ctx.now).fetch_one(&mut *conn).await.map_err(DbError::from)?;
        let membership = if kind == "chat" {
            sqlx::query_scalar::<_,Option<DateTime<Utc>>>("SELECT expires_at FROM chat_members WHERE chat_id=$1 AND user_id=$2 AND left_at IS NULL AND expires_at>$3").bind(id).bind(principal.user()).bind(ctx.now).fetch_optional(&mut *conn).await.map_err(DbError::from)?.flatten()
        } else {
            None
        };
        shown.refresh_at = next.into_iter().chain(membership).min().map(Time::new);
    }
    let pending = orch_db::queries::approvals::access::pending_on(&mut conn, id, ctx.now).await?;
    if !pending.is_empty() {
        let requests = crate::routes::approvals::views(
            &mut conn,
            &principal,
            &pending,
            ctx.now,
            &ctx.app.engine,
        )
        .await?
        .into_iter()
        .filter(|approval| approval.can_decide)
        .map(|mut approval| {
            if shown.unavailable.is_some() {
                approval.node = None;
                approval.title = "Access request".to_owned();
                if let Some(object) = &mut approval.object {
                    object.name.clear();
                }
            }
            approval
        })
        .collect::<Vec<_>>();
        if !requests.is_empty() {
            shown.access_requests = Some(requests);
        }
    }
    Ok(shown)
}

async fn readable_preview(ctx: &Ctx, query: &ReferencePreviewQuery) -> ApiResult<ReferencePreview> {
    // A part is the kind's own: a block of a document, a turn of a thread, a message of a chat.
    let wrong = match query.kind {
        InlineRefKind::Document => query
            .turn
            .map(|_| "turn")
            .or(query.message.map(|_| "message")),
        InlineRefKind::Node => query
            .block
            .map(|_| "block")
            .or(query.message.map(|_| "message")),
        InlineRefKind::Chat => query.block.map(|_| "block").or(query.turn.map(|_| "turn")),
        InlineRefKind::File | InlineRefKind::EnvironmentFile => query
            .block
            .map(|_| "block")
            .or(query.turn.map(|_| "turn"))
            .or(query.message.map(|_| "message")),
    };
    if let Some(member) = wrong {
        return Err(invalid(member, "is a part of another kind of thing"));
    }
    if query.line.is_some_and(|line| line < 1) {
        return Err(invalid("line", "is numbered from one"));
    }
    if matches!(
        query.kind,
        InlineRefKind::File | InlineRefKind::EnvironmentFile
    ) {
        return file_preview(ctx, query).await;
    }
    if query.path.is_some() || query.line.is_some() {
        return Err(invalid("path", "belongs to a file reference"));
    }
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let shown = match query.kind {
        InlineRefKind::Document => document(&mut conn, &principal, query, ctx.now).await?,
        InlineRefKind::Node => node(&mut conn, &principal, query, ctx.now).await?,
        InlineRefKind::Chat => chat(&mut conn, &principal, query).await?,
        InlineRefKind::File | InlineRefKind::EnvironmentFile => return Err(ApiError::not_found()),
    };
    conn.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

/// Reveals only existence/deletion and an unnamed request target. The authored label stays with the text.
async fn unavailable_preview(
    ctx: &Ctx,
    query: &ReferencePreviewQuery,
) -> ApiResult<ReferencePreview> {
    use orch_access_contract::decisions::ObjectRef;
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::access::AccessRequestObject;
    use orch_api_contract::documents::ReferenceUnavailable;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let target: Option<(String, Id, bool)> = match query.kind {
        InlineRefKind::Document => sqlx::query_as("SELECT 'document'::text,id,deleted_at IS NOT NULL FROM documents WHERE id=$1").bind(query.id).fetch_optional(&mut *conn).await,
        InlineRefKind::Node => sqlx::query_as("SELECT kind,id,deleted_at IS NOT NULL FROM nodes WHERE id=$1").bind(query.id).fetch_optional(&mut *conn).await,
        InlineRefKind::Chat => sqlx::query_as("SELECT 'chat'::text,c.id,COALESCE(n.deleted_at IS NOT NULL,false) FROM chats c LEFT JOIN nodes n ON n.id=c.node_id WHERE c.id=$1").bind(query.id).fetch_optional(&mut *conn).await,
        InlineRefKind::EnvironmentFile => sqlx::query_as("SELECT 'environment'::text,id,deleted_at IS NOT NULL FROM environments WHERE id=$1").bind(query.id).fetch_optional(&mut *conn).await,
        InlineRefKind::File => sqlx::query_as("SELECT 'document'::text,d.id,d.deleted_at IS NOT NULL FROM document_files f JOIN documents d ON d.id=f.document_id WHERE f.id=$1 UNION ALL SELECT 'chat'::text,c.id,m.deleted_at IS NOT NULL FROM chat_file_references f JOIN chats c ON c.id=f.chat_id JOIN chat_messages m ON m.id=f.message_id WHERE f.id=$1 LIMIT 1").bind(query.id).fetch_optional(&mut *conn).await,
    }.map_err(DbError::from)?;
    let (mut kind, mut id, deleted) = target.ok_or_else(ApiError::not_found)?;
    if kind == "chat" {
        let chat = chat_rows::chat(&mut conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if let Some(node) = chat.node_id {
            let node = hierarchy::with_ancestors(&mut conn, &[node])
                .await?
                .into_iter()
                .find(|row| row.id == node)
                .ok_or_else(ApiError::not_found)?;
            kind = node.kind;
            id = node.id;
        } else {
            let group = chat.kind == chat_rows::KIND_GROUP;
            let requestable = group
                && !deleted
                && !orch_engine::membership_requests::deciders(
                    &mut conn,
                    id,
                    principal.user(),
                    principal.now(),
                )
                .await
                .map_err(ApiError::internal)?
                .is_empty();
            let pending = if group {
                orch_db::queries::approvals::access::pending_for_person(
                    &mut conn,
                    principal.user(),
                    id,
                    ctx.now,
                )
                .await?
                .map(|row| row.id)
            } else {
                None
            };
            return Ok(ReferencePreview {
                thread_display: None,
                kind: query.kind,
                id: query.id,
                node_kind: None,
                title: String::new(),
                place: None,
                description: String::new(),
                updated_at: None,
                updated_by: None,
                part: None,
                file: None,
                unavailable: Some(ReferenceUnavailable {
                    deleted,
                    object: (group && !deleted).then_some(AccessRequestObject {
                        kind: "chat".to_owned(),
                        id,
                    }),
                    permission: None,
                    requestable,
                    pending,
                }),
                refresh_at: None,
                access_requests: None,
            });
        }
    }
    let object_kind: ObjectKind = kind.parse().map_err(ApiError::internal)?;
    let permission = match query.kind {
        InlineRefKind::Document => Permission::DocView,
        InlineRefKind::EnvironmentFile => Permission::FilesView,
        InlineRefKind::File if kind == "document" => Permission::DocView,
        InlineRefKind::Chat | InlineRefKind::File => Permission::ChatView,
        InlineRefKind::Node => match kind.as_str() {
            "thread" => Permission::ThreadView,
            "topic" => Permission::TopicView,
            "project" => Permission::ProjectView,
            _ => Permission::OrgView,
        },
    };
    let object = ObjectRef {
        kind: object_kind,
        id,
    };
    let mut requestable = false;
    if !deleted {
        let thing = orch_authz::things::thing_of(object);
        let (_, site) = orch_authz::things::site(&mut conn, thing)
            .await
            .map_err(ApiError::internal)?;
        let policy =
            crate::routes::access::subject::resource_policy(&mut conn, thing, &site).await?;
        let ask = orch_authz::requests::Ask {
            label: None,
            object,
            permissions: vec![permission],
            options: None,
            whom: orch_authz::requests::Whom::Requester,
        };
        requestable = orch_authz::requests::plan(&mut conn, &principal, &ask, &policy)
            .await
            .is_ok();
    }
    let pending: Option<Id> = sqlx::query_scalar("SELECT id FROM approvals WHERE kind='access' AND state='pending' AND requester_user_id=$1 AND COALESCE(object_id,node_id)=$2 AND (expires_at IS NULL OR expires_at>$3) ORDER BY created_at LIMIT 1").bind(principal.user()).bind(id).bind(ctx.now).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
    Ok(ReferencePreview {
        thread_display: None,
        kind: query.kind,
        id: query.id,
        node_kind: None,
        title: String::new(),
        place: None,
        description: String::new(),
        updated_at: None,
        updated_by: None,
        part: None,
        file: None,
        refresh_at: None,
        access_requests: None,
        unavailable: Some(ReferenceUnavailable {
            deleted,
            object: (!deleted).then_some(AccessRequestObject { kind, id }),
            permission: (!deleted).then(|| permission.as_str().to_owned()),
            requestable,
            pending,
        }),
    })
}

/// Whether a decision let the caller through; any refusal is the same not-found for a card.
fn allowed(decision: Result<Decision, orch_authz::Error>) -> ApiResult<()> {
    match decision {
        Ok(Decision::Allowed { .. }) => Ok(()),
        _ => Err(ApiError::not_found()),
    }
}

/// A document's card, with the block it names.
async fn document(
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
    now: DateTime<Utc>,
) -> ApiResult<ReferencePreview> {
    let policy = Policy::default();
    allowed(
        orch_authz::documents::check(conn, principal, query.id, Permission::DocView, policy).await,
    )?;
    let row = rows::get(conn, query.id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let listed = build::standing(conn, principal, row.id, policy).await?;
    let shown = build::reference(conn, principal, &row, &listed, now, policy).await?;
    let part = match query.block {
        Some(block) => block_part(conn, principal, row.id, block, policy).await?,
        None => None,
    };
    Ok(ReferencePreview {
        thread_display: None,
        kind: InlineRefKind::Document,
        id: row.id,
        node_kind: None,
        title: row.title.clone(),
        place: Some(shown.node),
        description: shown.description_line,
        updated_at: Some(shown.updated_at),
        updated_by: shown.updated_by,
        part,
        file: None,
        unavailable: None,
        refresh_at: None,
        access_requests: None,
    })
}

/// The block of a document a reference names, numbered as the top-level block it is or lies under: none where it
/// is gone.
async fn block_part(
    conn: &mut Conn,
    principal: &Principal,
    document: Id,
    block: Id,
    policy: Policy,
) -> ApiResult<Option<ReferencePreviewPart>> {
    let live = rows::live_blocks(conn, document).await?;
    let parents: HashMap<Id, Option<Id>> = live.iter().map(|row| (row.id, row.parent_id)).collect();
    let Some(row) = live.iter().find(|row| row.id == block) else {
        return Ok(None);
    };
    let mut top = block;
    for _ in 0..=orch_documents::blocks::nesting_limit() {
        match parents.get(&top) {
            Some(Some(parent)) => top = *parent,
            _ => break,
        }
    }
    let mut roots: Vec<&BlockRow> = live.iter().filter(|row| row.parent_id.is_none()).collect();
    roots.sort_by(|a, b| {
        Position::parse(&a.position)
            .cmp(&Position::parse(&b.position))
            .then(a.id.cmp(&b.id))
    });
    let Some(number) = roots
        .iter()
        .position(|root| root.id == top)
        .and_then(|at| i64::try_from(at + 1).ok())
    else {
        return Ok(None);
    };
    let by = build::blocks_for(conn, principal, std::slice::from_ref(row), policy)
        .await?
        .into_iter()
        .next()
        .and_then(|shown| shown.updated_by);
    Ok(Some(ReferencePreviewPart {
        part: RefPart::Block(block),
        number,
        anchor: Some(block),
        text: shown_text(&row.text),
        by,
    }))
}

fn node_kind(word: &str) -> NodeKind {
    match word {
        "org" => NodeKind::Org,
        "project" => NodeKind::Project,
        "topic" => NodeKind::Topic,
        _ => NodeKind::Thread,
    }
}

/// A place's or a thread's card, with the turn it names.
async fn node(
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
    now: DateTime<Utc>,
) -> ApiResult<ReferencePreview> {
    let policy = Policy::default();
    allowed(
        orch_authz::nodes::check_named(
            conn,
            principal,
            query.id,
            PermissionName::OfNode(NodePermission::View),
            Act::Use,
            policy,
        )
        .await,
    )?;
    let detail = hierarchy::details(conn, &[query.id], now)
        .await?
        .into_iter()
        .next()
        .filter(|detail| detail.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let place = match detail.parent_id {
        Some(parent) => view::refs(conn, principal, &[parent], policy)
            .await?
            .remove(&parent),
        None => None,
    };
    let part = match query.turn {
        Some(turn) if detail.kind == "thread" => {
            allowed(
                orch_authz::nodes::check(
                    conn,
                    principal,
                    detail.id,
                    Permission::ThreadView,
                    Act::Use,
                    policy,
                )
                .await,
            )?;
            turn_part(conn, detail.id, turn).await?
        }
        Some(_) => return Err(invalid("turn", "is a part of a thread")),
        None => None,
    };
    let thread_display = if detail.kind == "thread" {
        view::refs(conn, principal, &[detail.id], policy)
            .await?
            .remove(&detail.id)
            .and_then(|node| node.thread_display.map(|display| *display))
    } else {
        None
    };
    let chars = usize::try_from(DOCUMENT_LINE_MAX_CHARS).unwrap_or(usize::MAX);
    Ok(ReferencePreview {
        thread_display,
        kind: InlineRefKind::Node,
        id: detail.id,
        node_kind: Some(node_kind(&detail.kind)),
        title: detail.name.clone(),
        place,
        description: describe::line(&detail.description, None, chars),
        updated_at: Some(Time::new(detail.updated_at)),
        updated_by: None,
        part,
        file: None,
        unavailable: None,
        refresh_at: None,
        access_requests: None,
    })
}

/// A turn of a thread's conversation, by its number: the opening of what its agent last said, or of what it was
/// asked where it said nothing yet. None where the thread has no turn of that number.
async fn turn_part(
    conn: &mut Conn,
    thread: Id,
    number: i64,
) -> ApiResult<Option<ReferencePreviewPart>> {
    let said: Option<(Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT
           (SELECT i.search_text FROM items i WHERE i.turn_id = t.id AND i.stream = 'main'
              AND i.kind = 'agent_message' AND i.search_text IS NOT NULL ORDER BY i.seq_created DESC LIMIT 1),
           (SELECT i.search_text FROM items i WHERE i.turn_id = t.id AND i.stream = 'main'
              AND i.kind = 'user_message' AND i.search_text IS NOT NULL ORDER BY i.seq_created LIMIT 1)
         FROM turns t WHERE t.thread_id = $1 AND t.ordinal = $2
         ORDER BY t.created_at DESC LIMIT 1",
    )
    .bind(thread)
    .bind(i32::try_from(number).unwrap_or(i32::MAX))
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(said.map(|(answered, asked)| ReferencePreviewPart {
        part: RefPart::Turn(number),
        number,
        anchor: None,
        text: shown_text(answered.or(asked).as_deref().unwrap_or_default()),
        by: None,
    }))
}

/// A chat's card, with the message it names.
async fn chat(
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
) -> ApiResult<ReferencePreview> {
    let row = chat_rows::chat(conn, query.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let access = orch_authz::chats::listed_access(conn, principal, std::slice::from_ref(&row))
        .await
        .map_err(|_| ApiError::not_found())?;
    if !access.contains_key(&row.id) {
        return Err(ApiError::not_found());
    }
    let policy = Policy::default();
    let place = match row.node_id {
        Some(node) => view::refs(conn, principal, &[node], policy)
            .await?
            .remove(&node),
        None => None,
    };
    let title = match (&row.name, &place) {
        (Some(name), _) => name.clone(),
        (None, Some(node)) => node.name.clone().unwrap_or_default(),
        (None, None) => {
            let members = chat_rows::members(conn, row.id).await?;
            crate::routes::chats::people_label(conn, principal, &row, &members).await?
        }
    };
    let part = match query.message {
        Some(number) => message_part(conn, principal, row.id, number, policy).await?,
        None => None,
    };
    Ok(ReferencePreview {
        thread_display: None,
        kind: InlineRefKind::Chat,
        id: row.id,
        node_kind: None,
        title,
        place,
        description: String::new(),
        updated_at: row.last_message_at.map(Time::new),
        updated_by: None,
        part,
        file: None,
        unavailable: None,
        refresh_at: None,
        access_requests: None,
    })
}

/// A message of a chat by its number: its text and who wrote it; none where there is no such message or it was
/// deleted.
async fn message_part(
    conn: &mut Conn,
    principal: &Principal,
    chat: Id,
    number: i64,
    policy: Policy,
) -> ApiResult<Option<ReferencePreviewPart>> {
    let Some(found) = share_rows::chat_messages_numbered(conn, chat, number, number)
        .await?
        .into_iter()
        .next()
        .filter(|found| !found.row.is_deleted())
    else {
        return Ok(None);
    };
    let parts: Vec<orch_api_contract::chats::ChatPart> =
        serde_json::from_value(found.row.content.clone()).unwrap_or_default();
    let text: Vec<String> = parts
        .into_iter()
        .filter_map(|part| match part {
            orch_api_contract::chats::ChatPart::Text { text } => Some(text),
            _ => None,
        })
        .collect();
    let mut wanted = Wanted::default();
    if let Some(user) = found.row.author_id {
        wanted.user(user);
    }
    if let Some(thread) = found.row.author_thread_id {
        wanted.node(thread);
    }
    let cast = Cast::load(conn, principal, wanted, policy).await?;
    Ok(Some(ReferencePreviewPart {
        part: RefPart::Message(number),
        number,
        anchor: Some(found.row.id),
        text: shown_text(&text.join(" ")),
        by: cast.author(
            found.row.author_id,
            found.row.author_thread_id,
            found.row.author_agent.as_deref(),
        ),
    }))
}

fn whole(kind: InlineRefKind, id: Id) -> ReferencePreviewQuery {
    ReferencePreviewQuery {
        kind,
        id,
        block: None,
        turn: None,
        message: None,
        path: None,
        line: None,
    }
}

struct FileTarget {
    preview: ReferencePreview,
    blob: Option<Hash>,
    text: bool,
    place: Id,
}

/// Reads attachment ownership before asking for any bytes. An unknown identifier and an unreadable owner agree.
async fn attachment_target(ctx: &Ctx, query: &ReferencePreviewQuery) -> ApiResult<FileTarget> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    attachment_target_on(&mut conn, &principal, query, ctx.now).await
}

async fn attachment_target_on(
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
    now: DateTime<Utc>,
) -> ApiResult<FileTarget> {
    let owner: Option<Id> =
        sqlx::query_scalar("SELECT document_id FROM document_files WHERE id=$1")
            .bind(query.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    if let Some(owner) = owner {
        let card = document(conn, principal, &whole(InlineRefKind::Document, owner), now).await?;
        let file = rows::file(conn, owner, query.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let place = card
            .place
            .as_ref()
            .map(|node| node.id)
            .ok_or_else(ApiError::not_found)?;
        return Ok(FileTarget {
            preview: ReferencePreview {
                thread_display: None,
                kind: InlineRefKind::File,
                id: query.id,
                node_kind: None,
                title: file.file.name,
                place: card.place,
                description: String::new(),
                updated_at: Some(Time::new(file.file.added_at)),
                updated_by: None,
                part: None,
                file: Some(ReferencePreviewFile {
                    disabled: false,
                    enable_path: None,
                    blob: Some(file.blob),
                    media_type: Some(file.file.media_type),
                    path: None,
                    size: Some(file.file.size),
                    owner_kind: Some(InlineRefKind::Document),
                    owner_id: Some(owner),
                    message: None,
                    owner_name: card.title,
                    missing: false,
                }),
                unavailable: None,
                refresh_at: None,
                access_requests: None,
            },
            blob: Some(file.blob),
            text: file.file.is_text,
            place,
        });
    }
    let saved: Option<(Id, Id, i32)> =
        sqlx::query_as("SELECT chat_id,message_id,carried FROM chat_file_references WHERE id=$1")
            .bind(query.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let (chat_id, message_id, carried) = saved.ok_or_else(ApiError::not_found)?;
    let card = chat(conn, principal, &whole(InlineRefKind::Chat, chat_id)).await?;
    let message = chat_rows::message(conn, chat_id, message_id)
        .await?
        .filter(|row| !row.is_deleted())
        .ok_or_else(ApiError::not_found)?;
    let parts: Vec<orch_api_contract::chats::ChatPart> =
        serde_json::from_value(message.content).map_err(ApiError::internal)?;
    let files = chat_files(parts);
    let (blob, name, size, media) = usize::try_from(carried)
        .ok()
        .and_then(|n| n.checked_sub(1))
        .and_then(|at| files.into_iter().nth(at))
        .ok_or_else(ApiError::not_found)?;
    let size = match size {
        Some(size) => Some(size),
        None => orch_db::queries::storage::blob(conn, &blob)
            .await?
            .map(|blob| blob.size),
    };
    let row = chat_rows::chat(conn, chat_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let place = row.node_id.unwrap_or(row.org_id);
    Ok(FileTarget {
        preview: ReferencePreview {
            thread_display: None,
            kind: InlineRefKind::File,
            id: query.id,
            node_kind: None,
            title: name,
            place: card.place,
            description: String::new(),
            updated_at: Some(Time::new(message.created_at)),
            updated_by: None,
            part: None,
            file: Some(ReferencePreviewFile {
                disabled: false,
                enable_path: None,
                blob: Some(blob),
                media_type: Some(media.clone()),
                path: None,
                size,
                owner_kind: Some(InlineRefKind::Chat),
                owner_id: Some(chat_id),
                message: Some(message_id),
                owner_name: card.title,
                missing: false,
            }),
            unavailable: None,
            refresh_at: None,
            access_requests: None,
        },
        blob: Some(blob),
        text: media.starts_with("text/") || media == "application/json",
        place,
    })
}

pub(crate) fn chat_files(
    parts: Vec<orch_api_contract::chats::ChatPart>,
) -> Vec<(Hash, String, Option<i64>, String)> {
    use orch_api_contract::chats::ChatPart;
    let mut files = Vec::new();
    for part in parts {
        match part {
            ChatPart::Attachment {
                blob,
                name,
                size,
                media_type,
            } => files.push((blob, name, Some(size), media_type)),
            ChatPart::Image {
                blob,
                name,
                media_type,
            } => files.push((
                blob,
                name,
                None,
                match media_type {
                    orch_api_contract::threads::ImageMediaType::Png => "image/png",
                    orch_api_contract::threads::ImageMediaType::Jpeg => "image/jpeg",
                    orch_api_contract::threads::ImageMediaType::Gif => "image/gif",
                    orch_api_contract::threads::ImageMediaType::Webp => "image/webp",
                }
                .to_owned(),
            )),
            ChatPart::Forwarded(copy) => {
                files.extend(
                    copy.images
                        .into_iter()
                        .map(|file| (file.blob, file.name, None, file.media_type)),
                );
                files.extend(
                    copy.attachments
                        .into_iter()
                        .map(|file| (file.blob, file.name, Some(file.size), file.media_type)),
                );
            }
            _ => {}
        }
    }
    files
}

async fn environment_access(
    ctx: &Ctx,
    query: &ReferencePreviewQuery,
) -> ApiResult<orch_db::queries::environments::workspace::EnvironmentRow> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    environment_access_on(&mut conn, &principal, query).await
}

async fn environment_access_on(
    conn: &mut Conn,
    principal: &Principal,
    query: &ReferencePreviewQuery,
) -> ApiResult<orch_db::queries::environments::workspace::EnvironmentRow> {
    allowed(
        orch_authz::resources::check(
            conn,
            principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: query.id,
            },
            Permission::FilesView,
            Policy::default(),
        )
        .await,
    )?;
    orch_db::queries::environments::workspace::environment(conn, query.id)
        .await?
        .ok_or_else(ApiError::not_found)
}

async fn environment_target(ctx: &Ctx, query: &ReferencePreviewQuery) -> ApiResult<FileTarget> {
    let path = query
        .path
        .as_deref()
        .filter(|path| orch_documents::references::relative_path(path))
        .ok_or_else(|| invalid("path", "is a file path from the workspace root"))?;
    let env = environment_access(ctx, query).await?;
    let mut conn = ctx.app.db.acquire().await?;
    let disabled = env.status == "disabled";
    let principal = authorization::principal(ctx, &mut conn).await?;
    let enable_path = if disabled
        && matches!(
            orch_authz::resources::check(
                &mut conn,
                &principal,
                orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id: env.id
                },
                Permission::EnvironmentManage,
                Policy::default()
            )
            .await,
            Ok(Decision::Allowed { .. })
        ) {
        Some(format!("/resources/environments/{}?tab=danger", env.id))
    } else {
        None
    };
    let machine = crate::routes::workspace::live::machine_of(ctx, &mut conn, &env).await?;
    let cached = orch_db::queries::storage::cached_latest(&mut conn, env.id, path).await?;
    drop(conn);
    let (stat, missing): (Option<FileStat>, bool) = match machine {
        Some(machine) => match crate::routes::workspace::live::run(
            ctx,
            machine,
            SerialKey::None,
            &FsStat {
                environment_id: env.id,
                mount: None,
                path: path.to_owned(),
            },
        )
        .await
        {
            Ok(stat) => (Some(stat), false),
            Err(failure) => (None, failure.code() == Some("not_found")),
        },
        None => (None, false),
    };
    let is_file = stat
        .as_ref()
        .is_some_and(|stat| matches!(stat.entry.kind, DirEntryKind::File | DirEntryKind::Symlink));
    let mut blob = cached
        .as_ref()
        .filter(|cached| {
            stat.as_ref().is_none_or(|stat| {
                stat.entry.size == cached.size
                    && stat.entry.mtime.get().timestamp_millis()
                        == cached
                            .mtime_ns
                            .div_euclid(orch_db::queries::storage::NANOS_PER_MILLI)
                    && stat.hash.is_none_or(|hash| hash == cached.hash)
            })
        })
        .map(|cached| cached.hash);
    let size = stat
        .as_ref()
        .map(|stat| stat.entry.size)
        .or_else(|| cached.as_ref().map(|file| file.size));
    let text = stat
        .as_ref()
        .map_or_else(|| crate::media::is_text_by_name(path), |stat| stat.is_text);
    if blob.is_none()
        && is_file
        && text
        && query.line.is_some()
        && let Some(machine) = machine
    {
        let mut conn = ctx.app.db.acquire().await?;
        let limit = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::Node(env.home_node_id),
            orch_settings_contract::registry::keys::REFERENCES_PREVIEW_BYTES,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        drop(conn);
        if size.is_some_and(|size| size <= limit) {
            let fetched = crate::routes::workspace::live::run(
                ctx,
                machine,
                SerialKey::None,
                &FsFetch {
                    environment_id: env.id,
                    mount: None,
                    path: path.to_owned(),
                    expect: None,
                    max_bytes: Some(limit),
                },
            )
            .await
            .ok();
            if let Some(file) = fetched.filter(|file| {
                stat.as_ref().is_some_and(|stat| {
                    stat.entry.size == file.size
                        && stat.entry.mtime.get().timestamp_millis()
                            == file
                                .mtime_ns
                                .div_euclid(orch_db::queries::storage::NANOS_PER_MILLI)
                        && stat.hash.is_none_or(|hash| hash == file.hash)
                })
            }) {
                let mut conn = ctx.app.db.acquire().await?;
                orch_db::queries::storage::register_blob(&mut conn, &file.hash, file.size, ctx.now)
                    .await?;
                orch_db::queries::storage::record_cached(
                    &mut conn,
                    env.id,
                    &orch_db::queries::storage::CachedFile {
                        path: path.to_owned(),
                        size: file.size,
                        mtime_ns: file.mtime_ns,
                        hash: file.hash,
                        cached_at: ctx.now,
                    },
                )
                .await?;
                blob = Some(file.hash);
            }
        }
    }
    Ok(FileTarget {
        preview: ReferencePreview {
            thread_display: None,
            kind: InlineRefKind::EnvironmentFile,
            id: env.id,
            node_kind: None,
            title: path.rsplit('/').next().unwrap_or(path).to_owned(),
            place: None,
            description: String::new(),
            updated_at: stat.as_ref().map(|stat| stat.entry.mtime).or_else(|| {
                cached
                    .as_ref()
                    .map(|file| Time::new(chrono::DateTime::from_timestamp_nanos(file.mtime_ns)))
            }),
            updated_by: None,
            part: None,
            file: Some(ReferencePreviewFile {
                disabled,
                enable_path,
                blob,
                media_type: stat.as_ref().map(|stat| stat.media_type.clone()),
                path: Some(path.to_owned()),
                size,
                owner_kind: None,
                owner_id: None,
                message: None,
                owner_name: env.name,
                missing,
            }),
            unavailable: None,
            refresh_at: None,
            access_requests: None,
        },
        blob,
        text,
        place: env.home_node_id,
    })
}

async fn file_preview(ctx: &Ctx, query: &ReferencePreviewQuery) -> ApiResult<ReferencePreview> {
    let mut target = match query.kind {
        InlineRefKind::EnvironmentFile => environment_target(ctx, query).await?,
        _ => attachment_target(ctx, query).await?,
    };
    target.preview.part = query.line.map(|line| ReferencePreviewPart {
        part: RefPart::Line(line),
        number: line,
        anchor: None,
        text: String::new(),
        by: None,
    });
    if target.text
        && let (Some(line), Some(blob)) = (query.line, target.blob)
    {
        let mut conn = ctx.app.db.acquire().await?;
        let bytes = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::Node(target.place),
            orch_settings_contract::registry::keys::REFERENCES_PREVIEW_BYTES,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        let context = orch_settings::get::<i64>(
            &mut conn,
            orch_settings::Place::Node(target.place),
            orch_settings_contract::registry::keys::REFERENCES_PREVIEW_LINES,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        drop(conn);
        let bytes = target
            .preview
            .file
            .as_ref()
            .and_then(|file| file.size)
            .map_or(bytes, |size| bytes.min(size));
        let text = if bytes > 0 {
            match ctx
                .app
                .blobs
                .read(
                    &blob,
                    Some(ByteRange::Between {
                        start: 0,
                        end: bytes - 1,
                    }),
                )
                .await
            {
                Ok(read) => read_all(read.body)
                    .await
                    .ok()
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned()),
                Err(_) => None,
            }
        } else {
            None
        };
        if let Some(text) = text
            && let Some(text) = excerpt(&text, line, context)
        {
            target.preview.part = Some(ReferencePreviewPart {
                part: RefPart::Line(line),
                number: line,
                anchor: None,
                text,
                by: None,
            });
        }
    }
    // Native and blob reads hold no authority lock. Recheck the exact credential and current access at commit.
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    if query.kind == InlineRefKind::EnvironmentFile {
        let env = environment_access_on(&mut tx, guard.principal(), query).await?;
        if let Some(file) = &mut target.preview.file {
            file.owner_name = env.name;
        }
    } else {
        let mut fresh =
            attachment_target_on(&mut tx, guard.principal(), query, guard.now().get()).await?;
        if fresh.text {
            fresh.preview.part = target.preview.part.take();
            if fresh.blob != target.blob
                && let Some(part) = &mut fresh.preview.part
            {
                part.text.clear();
            }
        }
        target = fresh;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(target.preview)
}

fn excerpt(text: &str, line: i64, context: i64) -> Option<String> {
    let at = usize::try_from(line.checked_sub(1)?).ok()?;
    let context = usize::try_from(context).ok()?;
    let lines: Vec<&str> = text.lines().collect();
    lines.get(at)?;
    let start = at.saturating_sub(context);
    let end = at
        .saturating_add(context)
        .saturating_add(1)
        .min(lines.len());
    let most = usize::try_from(REFERENCE_PREVIEW_TEXT_MAX_CHARS).unwrap_or(usize::MAX);
    Some(lines[start..end].join("\n").chars().take(most).collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::Scene;
    use orch_api_contract::documents::{
        DocumentFileCreate, DocumentFileCreateQuery, NodeDocumentCreate, NodeDocumentCreateBody,
    };

    #[tokio::test]
    async fn file_cards_show_their_owner_size_and_lines_only_to_current_readers() {
        let scene = Scene::new().await;
        let app = &scene.rig.world.app;
        let doc = app
            .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
            .token(&scene.ada.token)
            .body(&NodeDocumentCreateBody {
                title: "Results".into(),
                blocks: None,
                markdown: Some("Results.".into()),
                access: None,
            })
            .send()
            .await
            .ok();
        let file = app
            .call(DocumentFileCreate, &[&doc.base.id])
            .token(&scene.ada.token)
            .query(&DocumentFileCreateQuery { overwrite: None })
            .header(orch_api_contract::threads::FILENAME_HEADER, "result.txt")
            .bytes(b"first\nsecond\nthird\n")
            .send()
            .await
            .ok();
        let query = ReferencePreviewQuery {
            line: Some(2),
            ..whole(InlineRefKind::File, file.id)
        };
        let card = app
            .call(ReferencePreviewGet, &[])
            .token(&scene.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert_eq!(card.title, "result.txt");
        assert_eq!(card.file.as_ref().and_then(|file| file.size), Some(19));
        assert_eq!(
            card.file.as_ref().and_then(|file| file.owner_id),
            Some(doc.base.id)
        );
        assert_eq!(
            card.part.as_ref().map(|part| part.text.as_str()),
            Some("first\nsecond\nthird")
        );
        let refused = app
            .call(ReferencePreviewGet, &[])
            .token(&scene.bea.token)
            .query(&query)
            .send()
            .await;
        assert_eq!(refused.status, 200);
        assert!(!String::from_utf8_lossy(&refused.raw).contains("result.txt"));
        assert!(refused.ok().unavailable.is_some());
    }

    #[tokio::test]
    async fn an_environment_file_card_uses_its_file_permission_and_can_show_a_kept_text_excerpt() {
        let scene = Scene::new().await;
        let app = &scene.rig.world.app;
        let doc = app
            .call(NodeDocumentCreate, &[&scene.tree.topic.base.id])
            .token(&scene.ada.token)
            .body(&NodeDocumentCreateBody {
                title: "Source".into(),
                blocks: None,
                markdown: None,
                access: None,
            })
            .send()
            .await
            .ok();
        let file = app
            .call(DocumentFileCreate, &[&doc.base.id])
            .token(&scene.ada.token)
            .query(&DocumentFileCreateQuery { overwrite: None })
            .header(orch_api_contract::threads::FILENAME_HEADER, "auth.rs")
            .bytes(b"fn first() {}\nfn second() {}\n")
            .send()
            .await
            .ok();
        let mut conn = scene.rig.db.acquire().await.unwrap();
        let held = rows::file(&mut conn, doc.base.id, file.id)
            .await
            .unwrap()
            .unwrap();
        orch_db::queries::storage::record_cached(
            &mut conn,
            scene.on.environment,
            &orch_db::queries::storage::CachedFile {
                path: "src/auth.rs".into(),
                size: file.size,
                mtime_ns: scene.rig.world.now().timestamp_nanos_opt().unwrap(),
                hash: held.blob,
                cached_at: scene.rig.world.now(),
            },
        )
        .await
        .unwrap();
        sqlx::query("UPDATE environments SET status='offline' WHERE id=$1")
            .bind(scene.on.environment)
            .execute(&mut *conn)
            .await
            .unwrap();
        // The environment's shipped defaults name org viewers too. Make this door private explicitly.
        orch_db::queries::access::testing::set_row(
            &mut conn,
            &orch_db::queries::access::PageKey::new("environment", scene.on.environment, "live"),
            "any",
            "files.view",
            false,
            false,
        )
        .await
        .unwrap();
        drop(conn);
        let query = ReferencePreviewQuery {
            path: Some("src/auth.rs".into()),
            line: Some(2),
            ..whole(InlineRefKind::EnvironmentFile, scene.on.environment)
        };
        let card = app
            .call(ReferencePreviewGet, &[])
            .token(&scene.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        sqlx::query("UPDATE environments SET status='disabled' WHERE id=$1")
            .bind(scene.on.environment)
            .execute(scene.rig.db.pool())
            .await
            .unwrap();
        let disabled = app
            .call(ReferencePreviewGet, &[])
            .token(&scene.ada.token)
            .query(&query)
            .send()
            .await
            .ok();
        assert!(disabled.file.as_ref().is_some_and(|file| file.disabled));
        assert_eq!(
            disabled
                .file
                .as_ref()
                .and_then(|file| file.enable_path.as_deref()),
            Some(
                format!(
                    "/resources/environments/{}?tab=danger",
                    scene.on.environment
                )
                .as_str()
            )
        );
        assert_eq!(card.title, "auth.rs");
        assert_eq!(
            card.file.as_ref().and_then(|file| file.path.as_deref()),
            Some("src/auth.rs")
        );
        assert!(
            card.part
                .as_ref()
                .is_some_and(|part| part.text.contains("fn second() {}"))
        );
        assert_eq!(
            app.call(
                orch_api_contract::nodes::NodeGet,
                &[&scene.tree.topic.base.id]
            )
            .token(&scene.cai.token)
            .send()
            .await
            .status,
            200,
            "the person still reads the topic"
        );
        assert_eq!(
            app.call(
                orch_api_contract::workspace::EnvironmentFileStat,
                &[&scene.on.environment]
            )
            .token(&scene.cai.token)
            .query(&orch_api_contract::workspace::EnvironmentFileStatQuery {
                path: "src/auth.rs".into(),
                mount: None,
                thread: None
            })
            .send()
            .await
            .status,
            403,
            "the canonical file door refuses this reader"
        );
        let refused = app
            .call(ReferencePreviewGet, &[])
            .token(&scene.cai.token)
            .query(&query)
            .send()
            .await;
        assert_eq!(
            refused.status, 200,
            "the known target has a safe card without granting environment files"
        );
        assert!(!String::from_utf8_lossy(&refused.raw).contains("auth.rs"));
        assert!(refused.ok().unavailable.is_some());
    }
}
