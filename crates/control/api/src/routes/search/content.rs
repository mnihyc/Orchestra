//! Content search over readable conversations, chats and current document blocks, with stable pages and Unicode excerpts.
//!
//! Everything's exact access projection is applied inside the indexed content statement.
//! Ordinary threads inherit their topic's decisions; only returned rows are hydrated.
//! The same snapshot supplies access and content, and hidden results never fill a page.
//!
//! The streamed search runs this lexical retrieval beside its independent meaning retrieval.

use super::{kinds, word};
use crate::middleware::authorization::refusals::user_refs;
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use crate::routes::{
    chats as chat_routes,
    nodes::{invalid, view},
};
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_access_contract::permissions::Permission;
use orch_api_contract::{
    chats::{ChatMessage, ChatRef},
    common::{MAX_LIMIT, Page, PageQuery},
    documents::{Author, AuthorAgent, DocumentRef},
    nodes::{NodeKind, PageState},
    search::{
        SEARCH_INDEXED_MIN_CHARS, SEARCH_QUERY_MIN_CHARS, SEARCH_SNIPPET_MAX_CHARS, SearchFound,
        SearchHit, SearchListQuery, SearchSort,
    },
};
use orch_authz::{Policy, Principal};
use orch_db::{
    Conn,
    queries::{chats, documents, search as rows},
};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};
use std::collections::{HashMap, HashSet};

pub(super) const CONTENT_KINDS: &[&str] = &[
    "user_message",
    "agent_message",
    "reasoning",
    "plan",
    "tool_call",
    "announcement",
    "approval",
    "error",
    "marker",
    "chat_message",
    "document",
];
/// One third before the match leaves twice as much context after it.
const SNIPPET_LEADING_DIVISOR: usize = 3;

#[derive(serde::Serialize, serde::Deserialize)]
struct Continuation {
    selection: SearchListQuery,
    position: rows::Cursor,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    projection: Option<orch_api_contract::threads::ThreadContinuationProjection>,
}

fn selection(query: &SearchListQuery) -> SearchListQuery {
    SearchListQuery {
        cursor: None,
        limit: None,
        sort: Some(query.sort.unwrap_or_default()),
        ..query.clone()
    }
}

fn cursor(query: &SearchListQuery) -> ApiResult<Option<Continuation>> {
    query
        .cursor
        .as_deref()
        .map(|text| {
            let bad = || invalid("cursor", "is not a cursor of this search and order");
            let decoded = URL_SAFE_NO_PAD.decode(text).map_err(|_| bad())?;
            let continued: Continuation = serde_json::from_slice(&decoded).map_err(|_| bad())?;
            if continued.selection != selection(query) {
                return Err(bad());
            }
            Ok(continued)
        })
        .transpose()
}

fn cursor_text(
    query: &SearchListQuery,
    position: rows::Cursor,
    projection: Option<&orch_api_contract::threads::ThreadContinuationProjection>,
) -> ApiResult<String> {
    Ok(URL_SAFE_NO_PAD.encode(
        serde_json::to_vec(&Continuation {
            selection: selection(query),
            position,
            projection: projection.cloned(),
        })
        .map_err(ApiError::internal)?,
    ))
}

fn changed_projection(
    projection: &orch_api_contract::threads::ThreadContinuationProjection,
) -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "The searched history changed. Start a new search in its current view.",
    )
    .with("projection", projection)
}

/// Serves a page of current content the caller can read.
pub(super) async fn search(ctx: Ctx, query: SearchListQuery, (): ()) -> ApiResult<Page<SearchHit>> {
    if query.regex == Some(true) {
        return Err(invalid("regex", "use the streamed search for expressions"));
    }
    let text = query.q.trim();
    let length = u32::try_from(text.chars().count()).unwrap_or(u32::MAX);
    if length < SEARCH_QUERY_MIN_CHARS {
        return Err(invalid("q", "needs at least two characters"));
    }
    if rows::Query::parse(text).terms().is_empty() {
        return Err(invalid("q", "give at least one word or quoted phrase"));
    }
    if query
        .limit
        .is_some_and(|limit| limit == 0 || limit > MAX_LIMIT)
    {
        return Err(invalid("limit", "is outside the search page limits"));
    }
    if query.from.zip(query.to).is_some_and(|(from, to)| from > to) {
        return Err(invalid("to", "precedes from"));
    }
    if query.chat_id.is_some() && query.document_id.is_some() {
        return Err(invalid("document_id", "cannot be combined with a chat"));
    }
    let requested = kinds(query.kinds.as_deref(), CONTENT_KINDS)?;
    let continued = cursor(&query)?;
    let after = continued.as_ref().map(|continued| &continued.position);
    let limit = PageQuery {
        limit: query.limit,
        cursor: None,
    }
    .limit();
    let mut tx = ctx.app.db.begin().await?;
    rows::snapshot(&mut tx).await?;
    let mut cancellation = rows::cancel_on_drop(&ctx.app.db, &mut tx).await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let scoped = match query.scope {
        Some(id) => rows::place(&mut tx, id).await?,
        None => None,
    };
    let readable_scope = if length < SEARCH_INDEXED_MIN_CHARS {
        let eligible = scoped
            .as_ref()
            .is_some_and(|node| matches!(node.kind.as_str(), "topic" | "thread"));
        if let Some(node) = scoped.as_ref().filter(|_| eligible) {
            view::refs(&mut tx, &principal, &[node.id], Policy::default())
                .await?
                .get(&node.id)
                .is_some_and(|node| !node.redacted)
        } else {
            false
        }
    } else {
        false
    };
    let exact_thread = scoped
        .as_ref()
        .filter(|scope| scope.kind == "thread")
        .map(|scope| scope.id);
    let history_selected = query.continuation_id.is_some() || query.projection_revision.is_some();
    if history_selected
        && query.chat_id.is_none()
        && query.document_id.is_none()
        && let Some(scope) = query.scope
    {
        let visible = scoped.is_some()
            && view::refs(&mut tx, &principal, &[scope], Policy::default())
                .await?
                .get(&scope)
                .is_some_and(|node| !node.redacted);
        if !visible {
            return Err(ApiError::not_found());
        }
    }
    if history_selected
        && (exact_thread.is_none() || query.chat_id.is_some() || query.document_id.is_some())
    {
        return Err(invalid(
            "continuation_id",
            "choose a thread scope to search its retained history",
        ));
    }
    let history = if let Some(thread) = exact_thread {
        let readable = if length < SEARCH_INDEXED_MIN_CHARS {
            readable_scope
        } else {
            view::refs(&mut tx, &principal, &[thread], Policy::default())
                .await?
                .get(&thread)
                .is_some_and(|node| !node.redacted)
        };
        if !readable {
            if history_selected {
                return Err(ApiError::not_found());
            }
            if length < SEARCH_INDEXED_MIN_CHARS {
                return Err(invalid(
                    "q",
                    "needs three characters unless the scope is a readable topic or thread",
                ));
            }
            cancellation.disarm();
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(Page {
                items: Vec::new(),
                next_cursor: None,
            });
        }
        let scope = orch_db::queries::thread_continuations::projection(
            &mut tx,
            thread,
            query.continuation_id,
        )
        .await?
        .ok_or_else(ApiError::not_found)?;
        let dto = orch_engine::continuations::projection::scope(&scope);
        if !scope.accepts_cursor(false, query.projection_revision) {
            return Err(changed_projection(&dto));
        }
        if let Some(cursor) = &continued {
            match &cursor.projection {
                Some(old)
                    if old.viewed_continuation_id == dto.viewed_continuation_id
                        && old.projection_revision == dto.projection_revision
                        && old.follows_active == dto.follows_active => {}
                None if scope.accepts_cursor(true, None) => {}
                _ => return Err(changed_projection(&dto)),
            }
        }
        Some(dto)
    } else {
        None
    };
    if length < SEARCH_INDEXED_MIN_CHARS
        && query.chat_id.is_none()
        && query.document_id.is_none()
        && !readable_scope
    {
        return Err(invalid(
            "q",
            "needs three characters unless the scope is a topic or thread",
        ));
    }
    // Exact selections need only their own metadata; they never enumerate an installation to choose a projection root.
    let selected_doc = match query.document_id {
        Some(id) => documents::get(&mut tx, id).await?,
        None => None,
    };
    let selected_chat = match query.chat_id {
        Some(id) => chats::chat(&mut tx, id).await?,
        None => None,
    };
    if query.document_id.is_some() && selected_doc.is_none()
        || query.chat_id.is_some() && selected_chat.is_none()
    {
        if length < SEARCH_INDEXED_MIN_CHARS && !readable_scope {
            return Err(invalid(
                "q",
                "needs three characters unless a readable conversation or document is selected",
            ));
        }
        cancellation.disarm();
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(Page {
            items: Vec::new(),
            next_cursor: None,
        });
    }
    // The two-character exception checks at most the exact chosen objects; all result access is decided by SQL below.
    if length < SEARCH_INDEXED_MIN_CHARS && !readable_scope {
        let mut readable = false;
        if let Some(doc) = &selected_doc {
            let live_home =
                query.scope.is_none() || rows::place(&mut tx, doc.node_id).await?.is_some();
            if live_home && rows::within(&mut tx, doc.node_id, query.scope).await? {
                readable = orch_authz::documents::listed(
                    &mut tx,
                    &principal,
                    &[doc.id],
                    Policy::default(),
                )
                .await
                .map_err(ApiError::internal)?
                .iter()
                .any(|held| {
                    !held.document.deleted
                        && held.holds(Permission::DocView)
                        && (!query.participating.unwrap_or(false) || held.participating)
                });
            }
        }
        if let Some(chat) = &selected_chat {
            let within = match chat.node_id {
                Some(node) => {
                    rows::place(&mut tx, node).await?.is_some()
                        && rows::within(&mut tx, node, query.scope).await?
                }
                None => {
                    (query.scope.is_none() || query.scope == Some(chat.org_id))
                        && rows::within(&mut tx, chat.org_id, None).await?
                }
            };
            if within {
                readable |= orch_authz::chats::listed_access(
                    &mut tx,
                    &principal,
                    std::slice::from_ref(chat),
                )
                .await
                .map_err(ApiError::internal)?
                .get(&chat.id)
                .is_some_and(|held| !query.participating.unwrap_or(false) || held.participating);
            }
        }
        if !readable {
            return Err(invalid(
                "q",
                "needs three characters unless a readable conversation or document is selected",
            ));
        }
    }
    let parent = query
        .scope
        .or_else(|| selected_doc.as_ref().map(|doc| doc.node_id))
        .or_else(|| {
            selected_chat
                .as_ref()
                .map(|chat| chat.node_id.unwrap_or(chat.org_id))
        });
    let advanced = super::advanced_selection(&mut tx, &principal, &query, ctx.now).await?;
    let candidate_grants = serde_json::json!({"search_advanced": advanced});
    let item_kinds: Vec<String> = requested
        .iter()
        .filter(|kind| !matches!(kind.as_str(), "chat_message" | "document"))
        .cloned()
        .collect();
    let base = rows::Content {
        continuation: query.continuation_id,
        selected: None,
        user: principal.user(),
        groups: principal.groups(),
        member_roots: &[],
        grants: &candidate_grants,
        at: ctx.now,
        scope: query.scope,
        chat: query.chat_id,
        document: query.document_id,
        participating: false,
        relevance: query.sort.unwrap_or_default() == SearchSort::Relevance,
        names: (query.sort == Some(SearchSort::Relevance) || query.title_only == Some(true))
            && (query.kinds.is_none()
                || (query.title_only == Some(true) && !item_kinds.is_empty())),
        with_file: query.with_file.unwrap_or(false),
        mentions_me: query
            .mentions_me
            .unwrap_or(false)
            .then_some(principal.user()),
        text,
        item_kinds: &item_kinds,
        chat_messages: requested.iter().any(|kind| kind == "chat_message"),
        document_content: requested.iter().any(|kind| kind == "document"),
        author: query.author_id,
        from: query.from.map(Time::get),
        to: query.to.map(Time::get),
        after: None,
        limit: i64::from(limit) + 1,
        items: None,
    };
    // The conversation items of each candidate page are chosen first, by reads that stop at what the page
    // needs (`search::items`): a long history is never matched whole. Best matches rank the newest matches
    // up to the word search's recent sample, the same pool on every page of one search.
    let place = query.scope.map_or(Place::System, Place::Node);
    let recent = get::<i64>(&mut tx, place, keys::SEARCH_WORD_RECENT_CANDIDATES)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("recent word setting missing"))?;
    let probe = get::<i64>(&mut tx, place, keys::SEARCH_WORD_PROBE_CANDIDATES)
        .await
        .map_err(ApiError::internal)?
        .ok_or_else(|| ApiError::internal("word probe setting missing"))?;
    let bounds = rows::items::Bounds {
        wanted: if base.relevance {
            base.limit.max(recent)
        } else {
            base.limit
        },
        recent,
        probe,
    };

    // Page the ordered matches, not the installation's document inventory. Each exact frame
    // holds at most one result page. Hidden matches never create a public cursor.
    let mut scan = after.cloned();
    let mut hits = Vec::new();
    loop {
        let page_after = scan.clone();
        let position = page_after
            .as_ref()
            .filter(|_| !base.relevance)
            .map(|cursor| (cursor.at, cursor.id));
        let items = rows::items::newest(
            &mut tx,
            &rows::Content {
                after: page_after.as_ref(),
                ..base
            },
            position,
            bounds,
        )
        .await?;
        let raw = rows::content_candidates(
            &mut tx,
            &rows::Content {
                after: page_after.as_ref(),
                items: Some(&items),
                ..base
            },
        )
        .await?;
        if raw.is_empty() {
            break;
        }
        let exhausted = raw.len() < usize::try_from(base.limit).map_err(ApiError::internal)?;
        scan = raw.last().map(rows::HitRow::cursor);
        let selected = rows::CandidateSet::from_content(&raw);
        let projection = orch_authz::everything::project_search(
            &mut tx,
            &principal,
            parent,
            &selected.documents,
            &selected.chats,
            &selected.nodes,
        )
        .await
        .map_err(ApiError::internal)?;
        let mut grants = projection.grants;
        grants["search_advanced"] = candidate_grants["search_advanced"].clone();
        let remaining =
            i64::from(limit) + 1 - i64::try_from(hits.len()).map_err(ApiError::internal)?;
        let found = rows::content(
            &mut tx,
            &rows::Content {
                selected: Some(&selected),
                member_roots: &projection.member_roots,
                grants: &grants,
                participating: query.participating.unwrap_or(false),
                after: page_after.as_ref(),
                limit: remaining,
                ..base
            },
        )
        .await?;
        hits.extend(found);
        if hits.len() > usize::try_from(limit).map_err(ApiError::internal)? || exhausted {
            break;
        }
    }
    let limit = usize::try_from(limit).map_err(ApiError::internal)?;
    let more = hits.len() > limit;
    hits.truncate(limit);
    let next_cursor = if more {
        hits.last()
            .map(|hit| cursor_text(&query, hit.cursor(), history.as_ref()))
            .transpose()?
    } else {
        None
    };
    let items = hydrate(&mut tx, &principal, &hits, &query, ctx.now).await?;
    cancellation.disarm();
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Page { items, next_cursor })
}

/// The hits of a page as they are answered, with what each names read in the same snapshot.
pub(super) async fn hydrate(
    conn: &mut Conn,
    principal: &Principal,
    hits: &[rows::HitRow],
    query: &SearchListQuery,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<SearchHit>> {
    if hits.is_empty() {
        return Ok(Vec::new());
    }
    let chat_ids: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == "chat_message")
        .map(|hit| hit.object_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let chat_rows = chats::chats(conn, &chat_ids)
        .await?
        .into_iter()
        .map(|chat| (chat.id, chat))
        .collect();
    let doc_ids: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == "document")
        .map(|hit| hit.object_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let phase = std::time::Instant::now();
    let listed = orch_authz::documents::listed(conn, principal, &doc_ids, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .map(|held| (held.document.id, held))
        .collect();
    tracing::debug!(target: "orch::search::timing", phase="document_access", elapsed_ms=phase.elapsed().as_secs_f64()*1000., documents=doc_ids.len());
    assemble(conn, principal, hits, &chat_rows, &listed, query, now).await
}

async fn assemble(
    conn: &mut Conn,
    principal: &Principal,
    hits: &[rows::HitRow],
    chats: &HashMap<Id, chats::ChatRow>,
    listed: &HashMap<Id, orch_authz::documents::Listed>,
    query: &SearchListQuery,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<Vec<SearchHit>> {
    let blocks: Vec<_> = hits.iter().filter_map(|hit| hit.block_id).collect();
    let headings: HashSet<Id> = if blocks.is_empty() {
        HashSet::new()
    } else {
        sqlx::query_scalar("SELECT id FROM document_blocks WHERE id=ANY($1) AND type='heading'")
            .bind(&blocks)
            .fetch_all(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
            .into_iter()
            .collect()
    };
    let text = query.q.trim();
    let expression = (query.regex == Some(true))
        .then(|| {
            orch_engine::tools::search::search_expression(text)
                .map_err(|reason| invalid("q", &reason))
        })
        .transpose()?;
    let file_focused = query.with_file == Some(true) || query.file_type.is_some();
    let file_hits: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == "document" && (file_focused || hit.block_id.is_none()))
        .collect();
    let matched = if !file_hits.is_empty() {
        // Only already authorized documents are read. Prefer a matched filename, then a current file of the selected type.
        let document_ids: Vec<_> = file_hits.iter().map(|hit| hit.object_id).collect();
        let names: Vec<_> = file_hits.iter().map(|hit| hit.text.as_str()).collect();
        rows::files_for_hits(
            conn,
            &document_ids,
            &names,
            file_focused,
            query.file_type.as_deref(),
        )
        .await?
    } else {
        Vec::new()
    };
    let mut wanted = crate::routes::documents::build::Wanted::default();
    for matched in &matched {
        if let Some(person) = matched.file.added_by {
            wanted.user(person);
        }
        if let Some(thread) = matched.file.added_by_thread {
            wanted.node(thread);
        }
    }
    for hit in hits {
        if let Some(user) = hit.author_id {
            wanted.user(user);
        }
        if let Some(thread) = hit.author_thread_id {
            wanted.node(thread);
        }
    }
    let authors =
        crate::routes::documents::build::Cast::load(conn, principal, wanted, Policy::default())
            .await?;
    let matching_files: HashMap<_, _> = matched
        .into_iter()
        .map(|matched| {
            let file = matched.file;
            (
                matched.document_id,
                orch_api_contract::documents::DocumentFile {
                    id: file.id,
                    name: file.name,
                    media_type: file.media_type,
                    size: file.size,
                    is_text: file.is_text,
                    added_by: authors.author(file.added_by, file.added_by_thread, None),
                    added_at: Time::new(file.added_at),
                },
            )
        })
        .collect();
    let item_ids: Vec<Id> = hits
        .iter()
        .filter(|hit| hit.kind == "item")
        .map(|hit| hit.id)
        .collect();
    let item_rows =
        orch_db::queries::thread_history::search_items(conn, &item_ids, query.continuation_id)
            .await?;
    let item_threads: Vec<_> = item_rows
        .iter()
        .map(|item| item.thread_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let item_projections: HashMap<_, _> = orch_db::queries::thread_history::search_projections(
        conn,
        &item_threads,
        query.continuation_id,
    )
    .await?
    .into_iter()
    .map(|projection| {
        (
            projection.thread_id,
            orch_engine::continuations::projection::scope(&projection),
        )
    })
    .collect();
    let people = orch_engine::views::People::load(
        conn,
        item_rows
            .iter()
            .flat_map(|item| item.author_id.into_iter().chain(item.pinned_by)),
    )
    .await
    .map_err(ApiError::internal)?
    .with_approvals(
        conn,
        item_rows
            .iter()
            .filter_map(orch_engine::views::source_approval),
    )
    .await
    .map_err(ApiError::internal)?;
    let items: HashMap<Id, _> = item_rows
        .iter()
        .map(|item| (item.id, orch_engine::views::item_view(item, &people)))
        .collect();
    let doc_ids: Vec<Id> = hits
        .iter()
        .filter(|hit| hit.kind == "document")
        .map(|hit| hit.object_id)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let docs = documents::get_many(conn, &doc_ids).await?;
    let phase = std::time::Instant::now();
    let docs = document_refs(conn, principal, &docs, listed, now).await?;
    tracing::debug!(target: "orch::search::timing", phase="document_metadata", elapsed_ms=phase.elapsed().as_secs_f64()*1000., documents=docs.len());
    let mut paths: Vec<Id> = item_rows.iter().map(|item| item.thread_id).collect();
    paths.extend(
        hits.iter()
            .filter(|hit| hit.kind == "name")
            .map(|hit| hit.object_id),
    );
    paths.extend(
        hits.iter()
            .filter_map(|hit| chats.get(&hit.object_id).and_then(|chat| chat.node_id)),
    );
    paths.extend(docs.values().map(|doc| doc.node.id));
    let phase = std::time::Instant::now();
    let (ways, thread_markers) =
        view::ways_with_admin(conn, principal, &paths, Policy::default()).await?;
    tracing::debug!(target: "orch::search::timing", phase="search_paths", elapsed_ms=phase.elapsed().as_secs_f64()*1000., paths=paths.len());
    let chat_rows: Vec<_> = hits
        .iter()
        .filter(|hit| hit.kind == "chat_message")
        .filter_map(|hit| chats.get(&hit.object_id).cloned())
        .collect();
    let chat_access = orch_authz::chats::listed_access(conn, principal, &chat_rows)
        .await
        .map_err(ApiError::internal)?;
    let chat_places: HashMap<_, _> = chat_access
        .iter()
        .filter_map(|(id, access)| access.admin_access.map(|place| (*id, place)))
        .collect();
    let chat_markers = view::named_admin_access(conn, principal, &chat_places).await?;
    let mut messages: HashMap<Id, ChatMessage> = HashMap::new();
    for chat in hits
        .iter()
        .filter(|hit| hit.kind == "chat_message")
        .map(|hit| hit.object_id)
        .collect::<HashSet<_>>()
    {
        let ids: Vec<Id> = hits
            .iter()
            .filter(|hit| hit.object_id == chat && hit.kind == "chat_message")
            .map(|hit| hit.id)
            .collect();
        let rows = orch_db::queries::chats::messages_by_ids(conn, chat, &ids).await?;
        messages.extend(
            chat_routes::messages_view(conn, principal, chat, &rows)
                .await?
                .into_iter()
                .map(|message| (message.id, message)),
        );
    }
    hits.iter()
        .map(|hit| {
            let absent =
                || ApiError::internal("a matched search object vanished from its snapshot");
            let found = match hit.kind.as_str() {
                "name" => {
                    let path = ways.get(&hit.object_id).cloned().ok_or_else(absent)?;
                    let node = path.last().cloned().ok_or_else(absent)?;
                    let kind = match node.kind {
                        NodeKind::Org => "org",
                        NodeKind::Project => "project",
                        NodeKind::Topic => "topic",
                        NodeKind::Thread => "thread",
                    };
                    let href = crate::routes::destinations::href(
                        kind,
                        &node.id.to_string(),
                        None,
                        None,
                        &path,
                        None,
                    )
                    .ok_or_else(absent)?;
                    SearchFound::Name {
                        node,
                        path,
                        href,
                        at: Time::new(hit.at),
                    }
                }
                "item" => {
                    let item = items.get(&hit.id).cloned().ok_or_else(absent)?;
                    let way = ways.get(&item.thread_id).ok_or_else(absent)?;
                    let thread = way
                        .iter()
                        .rev()
                        .find(|node| node.kind == NodeKind::Thread)
                        .cloned()
                        .ok_or_else(absent)?;
                    let topic = way
                        .iter()
                        .rev()
                        .find(|node| node.kind == NodeKind::Topic)
                        .cloned()
                        .ok_or_else(absent)?;
                    let project = way
                        .iter()
                        .rev()
                        .find(|node| node.kind == NodeKind::Project)
                        .cloned()
                        .ok_or_else(absent)?;
                    SearchFound::Item {
                        projection: Some(
                            item_projections
                                .get(&item.thread_id)
                                .cloned()
                                .ok_or_else(absent)?,
                        ),
                        item,
                        thread,
                        topic,
                        project,
                    }
                }
                "chat_message" => {
                    let row = chats.get(&hit.object_id).ok_or_else(absent)?;
                    let path = row
                        .node_id
                        .and_then(|node| ways.get(&node).cloned())
                        .unwrap_or_default();
                    SearchFound::ChatMessage {
                        message: messages.get(&hit.id).cloned().ok_or_else(absent)?,
                        chat: ChatRef {
                            id: row.id,
                            kind: word(&row.kind)?,
                            org_id: row.org_id,
                            node: path.last().cloned(),
                            name: row.name.clone(),
                        },
                        path,
                    }
                }
                "document" => {
                    let document = docs.get(&hit.object_id).cloned().ok_or_else(absent)?;
                    let path = ways.get(&document.node.id).cloned().unwrap_or_default();
                    SearchFound::Document {
                        file: if file_focused || hit.block_id.is_none() {
                            matching_files.get(&hit.object_id).cloned()
                        } else {
                            None
                        },
                        by: authors
                            .author(
                                hit.author_id,
                                hit.author_thread_id,
                                hit.author_agent.as_deref(),
                            )
                            .or_else(|| {
                                hit.block_id
                                    .is_none()
                                    .then(|| document.updated_by.clone())
                                    .flatten()
                            }),
                        document,
                        block_id: hit.block_id,
                        path,
                    }
                }
                _ => return Err(absent()),
            };
            let (snippet, matches) = if let Some(expression) = &expression {
                clipped_excerpt(
                    &hit.text,
                    expression
                        .find_iter(&hit.text)
                        .map(|found| {
                            (
                                hit.text[..found.start()].chars().count(),
                                hit.text[..found.end()].chars().count(),
                            )
                        })
                        .collect(),
                )
            } else {
                excerpt(&hit.text, text, Some((&hit.folded_text, &hit.folded_query)))
            };
            let admin_access = match &found {
                SearchFound::Name { node, .. } => thread_markers.get(&node.id).cloned(),
                SearchFound::Item { thread, .. } => thread_markers.get(&thread.id).cloned(),
                SearchFound::ChatMessage { chat, .. } => chat_markers.get(&chat.id).cloned(),
                SearchFound::Document { document, .. } => document.admin_access.clone(),
            };
            Ok(SearchHit {
                admin_access,
                match_field: Some(match_location(hit, &found, &headings)),
                found,
                snippet,
                matches,
            })
        })
        .collect()
}

fn match_location(
    hit: &rows::HitRow,
    found: &SearchFound,
    headings: &HashSet<Id>,
) -> orch_api_contract::search::SearchMatchField {
    use orch_api_contract::search::SearchMatchField as Field;
    match found {
        SearchFound::Name { node, .. } => {
            if node.name.as_deref() == Some(hit.text.as_str()) {
                Field::Name
            } else {
                Field::Description
            }
        }
        SearchFound::Document {
            document,
            block_id: None,
            ..
        } => {
            if hit.text == document.title {
                Field::DocumentTitle
            } else if hit.text == document.description {
                Field::Description
            } else {
                Field::FileName
            }
        }
        SearchFound::Document {
            block_id: Some(block),
            ..
        } if headings.contains(block) => Field::DocumentHeading,
        _ => Field::Body,
    }
}

async fn document_refs(
    conn: &mut Conn,
    principal: &Principal,
    docs: &[documents::DocumentRow],
    listed: &HashMap<Id, orch_authz::documents::Listed>,
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<Id, DocumentRef>> {
    if docs.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<Id> = docs.iter().map(|doc| doc.id).collect();
    let owners = documents::owner_counts(conn, &ids, now).await?;
    let discussions = documents::open_discussion_counts(conn, &ids).await?;
    let seen = documents::seen_seqs(conn, principal.user(), &ids).await?;
    let levels = documents::people::levels(conn, principal.user(), &ids).await?;
    let agents = documents::last_agents(conn, &ids).await?;
    let lines = Box::pin(crate::routes::documents::build::lines_for(conn, docs)).await?;
    let user_ids: Vec<Id> = docs
        .iter()
        .flat_map(|doc| std::iter::once(doc.owner_id).chain(doc.updated_by))
        .collect();
    let users = user_refs(conn, &user_ids).await?;
    let node_ids: Vec<Id> = docs
        .iter()
        .flat_map(|doc| std::iter::once(doc.node_id).chain(doc.created_by_thread))
        .chain(agents.values().map(|(thread, _)| *thread))
        .collect();
    let nodes = view::refs(conn, principal, &node_ids, Policy::default()).await?;
    let marker_places: HashMap<_, _> = listed
        .iter()
        .filter_map(|(id, held)| held.admin_access.map(|place| (*id, place)))
        .collect();
    let admin_markers = view::named_admin_access(conn, principal, &marker_places).await?;
    let agent = |thread: Id, name: &str| {
        nodes.get(&thread).cloned().map(|thread| {
            Author::Agent(AuthorAgent {
                agent: name.to_owned(),
                thread,
            })
        })
    };
    docs.iter()
        .map(|doc| {
            let owner = users
                .get(&doc.owner_id)
                .cloned()
                .ok_or_else(|| ApiError::internal("a document's owner is missing"))?;
            let held = listed
                .get(&doc.id)
                .ok_or_else(|| ApiError::internal("a document's access was not resolved"))?;
            let created_by = doc
                .created_by_thread
                .zip(doc.created_by_agent.as_deref())
                .and_then(|(thread, name)| agent(thread, name))
                .unwrap_or_else(|| Author::User(owner.clone()));
            let updated_by = doc
                .updated_by
                .and_then(|id| users.get(&id).cloned().map(Author::User))
                .or_else(|| {
                    agents
                        .get(&doc.id)
                        .and_then(|(thread, name)| agent(*thread, name))
                });
            Ok((
                doc.id,
                DocumentRef {
                    admin_access: admin_markers.get(&doc.id).cloned(),
                    id: doc.id,
                    title: doc.title.clone(),
                    description: doc.description.clone(),
                    description_source: crate::routes::documents::build::description_source(
                        &doc.description_source,
                    ),
                    description_line: lines.get(&doc.id).cloned().unwrap_or_default(),
                    node: nodes
                        .get(&doc.node_id)
                        .cloned()
                        .ok_or_else(|| ApiError::internal("a document's node is missing"))?,
                    owner,
                    owners: owners.get(&doc.id).copied().unwrap_or_default(),
                    created_by,
                    page: match held.state {
                        orch_access_contract::pages::PageState::Follows => PageState::Follows,
                        orch_access_contract::pages::PageState::Custom => PageState::Custom,
                        orch_access_contract::pages::PageState::Restricted => PageState::Restricted,
                    },
                    seq: doc.doc_seq,
                    block_count: i64::from(doc.block_count),
                    bytes: doc.bytes,
                    files_bytes: doc.files_bytes,
                    version: i64::from(doc.version),
                    open_discussions: discussions.get(&doc.id).copied().unwrap_or_default(),
                    updated_at: Time::new(doc.updated_at),
                    updated_by,
                    discussion_level: if levels.get(&doc.id).is_some_and(|level| level == "all") {
                        orch_api_contract::documents::DocumentDiscussionLevel::All
                    } else {
                        orch_api_contract::documents::DocumentDiscussionLevel::Involved
                    },
                    seen_seq: seen.get(&doc.id).map(|(seq, _)| *seq),
                    archived: held.archived_node.is_some(),
                    deleted: false,
                },
            ))
        })
        .collect()
}

/// The excerpt keeps one third of its budget before the first match and the
/// rest after it. Lowercase expansions retain their original character index.
fn excerpt(
    text: &str,
    query: &str,
    database_fold: Option<(&str, &str)>,
) -> (String, Vec<(u32, u32)>) {
    let chars: Vec<char> = text.chars().collect();
    let mut folded = String::new();
    let mut offsets = Vec::new();
    for (index, character) in chars.iter().enumerate() {
        for lower in character.to_lowercase() {
            offsets.extend(std::iter::repeat_n(index, lower.len_utf8()));
            folded.push(lower);
        }
    }
    if let Some((database_text, _)) = database_fold {
        // A collation that preserves character counts can use the original
        // positions directly, including mappings such as dotted I to i.
        if database_text.chars().count() == chars.len() {
            offsets.clear();
            for (index, character) in database_text.chars().enumerate() {
                offsets.extend(std::iter::repeat_n(index, character.len_utf8()));
            }
        }
        folded = database_text.to_owned();
    }
    let needles = rows::Query::parse(
        &database_fold.map_or_else(|| query.to_lowercase(), |(_, query)| query.to_owned()),
    );
    let mut ranges: Vec<(usize, usize)> = needles
        .terms()
        .iter()
        .flat_map(|needle| {
            folded
                .match_indices(needle.as_str())
                .filter_map(|(byte, matched)| {
                    Some((
                        *offsets.get(byte)?,
                        offsets
                            .get(byte + matched.len().checked_sub(1)?)?
                            .checked_add(1)?,
                    ))
                })
                .collect::<Vec<_>>()
        })
        .collect();
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (from, to) in ranges {
        if let Some(last) = merged.last_mut().filter(|last| from <= last.1) {
            last.1 = last.1.max(to);
        } else {
            merged.push((from, to));
        }
    }
    clipped_excerpt(text, merged)
}

fn clipped_excerpt(text: &str, ranges: Vec<(usize, usize)>) -> (String, Vec<(u32, u32)>) {
    let chars: Vec<_> = text.chars().collect();
    let budget = SEARCH_SNIPPET_MAX_CHARS as usize;
    let before = budget / SNIPPET_LEADING_DIVISOR;
    let first = ranges.first().map_or(0, |(start, _)| *start);
    let start = first
        .saturating_sub(before)
        .min(chars.len().saturating_sub(budget));
    let end = (start + budget).min(chars.len());
    let snippet = chars[start..end].iter().collect();
    let matches = ranges
        .into_iter()
        .filter_map(|(from, to)| {
            if from >= end || to <= start {
                return None;
            }
            Some((
                u32::try_from(from.max(start) - start).ok()?,
                u32::try_from(to.min(end) - start).ok()?,
            ))
        })
        .collect();
    (snippet, matches)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn excerpts_and_highlights_use_characters_for_any_script_and_lowercase_expansions() {
        let (snippet, matches) = excerpt("你好 HELLO 世界 HELLO", "hello", None);
        assert_eq!(snippet, "你好 HELLO 世界 HELLO");
        assert_eq!(matches, [(3, 8), (12, 17)]);
        let (_, matches) = excerpt("İstanbul", "i\u{307}s", None);
        assert_eq!(matches, [(0, 2)]);
        let (snippet, matches) = excerpt(
            &format!("{}needle{}", "界".repeat(300), "好".repeat(300)),
            "needle",
            None,
        );
        assert_eq!(snippet.chars().count(), SEARCH_SNIPPET_MAX_CHARS as usize);
        assert_eq!(matches, [(80, 86)]);
    }
}
