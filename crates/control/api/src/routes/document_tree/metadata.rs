//! One person-projected metadata cut for HTTP, operation answers and ephemeral views.

use super::{convert, options, result};
use crate::middleware::{
    authentication::Via,
    authorization, inspection,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
use crate::routes::access::page::NameBounds;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::{common::ErrorCode, document_tree::*};
use orch_authz::{Act, Policy, Principal, engine::Graph};
use orch_db::{
    Conn,
    queries::documents::{self as documents, containers},
};
use orch_engine::document_tree as domain;
use orch_realtime_contract::{document_tree::DocumentTreeView, feeds::Feed};
use orch_types::{Hash, Id, Seq, Time};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

pub(crate) const DEFAULT_LIMIT: u16 = 50;
pub(crate) const MAX_LIMIT: u16 = 500;
// One existing maximum page of candidates bounds work even when every candidate is unreadable.
const SCAN: usize = MAX_LIMIT as usize;
const CURSOR_PURPOSE: &[u8] = b"document-tree/page/v1";

#[derive(Debug, Clone)]
pub(crate) struct Cut {
    pub(crate) node: Id,
    pub(crate) root: Id,
    hierarchy: Hash,
    pub(crate) membership: Seq,
    pub(crate) epoch: i64,
    pub(crate) until: Option<Time>,
    pub(crate) as_admin: bool,
}

pub(crate) struct Projection<T> {
    pub(crate) value: T,
    pub(crate) cut: Cut,
    pub(crate) inspections: Vec<inspection::Info>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Cursor {
    at: DocumentDirectory,
    deleted: bool,
    node: Id,
    membership: Seq,
    epoch: i64,
    until: Option<Time>,
    offset: i64,
}

pub(crate) fn stale() -> ApiError {
    ApiError::new(
        ErrorCode::ConflictStaleRevision,
        "The document directory changed. Start this directory again.",
    )
    .with("restart", true)
    .with("reason", "document_directory")
}

fn bounded<T: Serialize>(value: &T) -> ApiResult<()> {
    // Reuse half the existing live frame budget, leaving its other half for framing and queueing.
    if serde_json::to_vec(value).map_err(ApiError::internal)?.len()
        > crate::backplane::MAX_BACKPLANE_FRAME_BYTES / 2
    {
        return Err(ApiError::new(
            ErrorCode::RequestTooLarge,
            "This metadata page is too large. Request fewer entries.",
        ));
    }
    Ok(())
}

fn audience(ctx: &Ctx, principal: &Principal) -> ApiResult<Vec<u8>> {
    let actor = ctx.user()?;
    let credential = match &actor.via {
        Via::OwnerRead { source } => {
            serde_json::json!({"owner_read":{"owner":source.owner(),"thread":source.thread(),"root":source.root()}})
        }
        Via::Session { session_id } => serde_json::json!({"session":session_id}),
        Via::Token {
            token_id,
            scopes,
            node_id,
            operation_scope,
            expires_at,
        } => {
            serde_json::json!({"token":token_id,"scopes":scopes,"node":node_id,"operation_scope":operation_scope,"expires_at":expires_at})
        }
    };
    let bytes = serde_json::to_vec(&(actor.id(), principal.caller().as_admin(), credential))
        .map_err(ApiError::internal)?;
    let mut binding = CURSOR_PURPOSE.to_vec();
    binding.extend_from_slice(Hash::of(&bytes).as_bytes());
    Ok(binding)
}

fn offset(
    ctx: &Ctx,
    principal: &Principal,
    text: Option<&str>,
    at: DocumentDirectory,
    deleted: bool,
    cut: &Cut,
) -> ApiResult<i64> {
    let Some(text) = text else { return Ok(0) };
    if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
        return Err(stale());
    }
    let key = crate::routes::resources::sealing::master_key()?;
    let sealed = URL_SAFE_NO_PAD.decode(text).map_err(|_| stale())?;
    let plain = key
        .open(&sealed, &audience(ctx, principal)?)
        .map_err(|_| stale())?;
    let cursor: Cursor = serde_json::from_slice(&plain).map_err(|_| stale())?;
    if cursor.at != at
        || cursor.deleted != deleted
        || cursor.node != cut.node
        || cursor.membership != cut.membership
        || cursor.epoch != cut.epoch
        || cursor.offset < 0
        || cursor
            .until
            .is_some_and(|until| until <= ctx.app.clock.time())
    {
        return Err(stale());
    }
    Ok(cursor.offset)
}

fn next(
    ctx: &Ctx,
    principal: &Principal,
    at: DocumentDirectory,
    deleted: bool,
    cut: &Cut,
    offset: i64,
) -> ApiResult<String> {
    let plain = serde_json::to_vec(&Cursor {
        at,
        deleted,
        node: cut.node,
        membership: cut.membership,
        epoch: cut.epoch,
        until: cut.until,
        offset,
    })
    .map_err(ApiError::internal)?;
    let sealed = crate::routes::resources::sealing::master_key()?
        .seal(&plain, &audience(ctx, principal)?)
        .map_err(ApiError::internal)?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

pub(crate) async fn cut(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    now: Time,
) -> ApiResult<Cut> {
    let graph = Graph::load(conn, &[node])
        .await
        .map_err(ApiError::internal)?;
    // Entry authority was checked separately; retained metadata can have a deleted hierarchy anchor.
    let root = graph
        .get(node)
        .map(|node| node.root)
        .ok_or_else(ApiError::not_found)?;
    Ok(Cut {
        node,
        root,
        hierarchy: hierarchy_stamp(&graph, node)?,
        membership: containers::scope_seq(conn, node).await?,
        epoch: orch_db::queries::access::epoch(conn).await?,
        until: containers::authority_until(conn, now.get())
            .await?
            .map(Time::new),
        as_admin: principal.caller().as_admin(),
    })
}

/// Only the actual ancestor chain matters; unrelated org activity cannot make this cut stale.
/// Raw names remain internal to the hash. The payload uses the native person-redacted supplier.
fn hierarchy_stamp(graph: &Graph, id: Id) -> ApiResult<Hash> {
    let node = graph.get(id).ok_or_else(stale)?;
    let path: Vec<_> = node
        .path
        .iter()
        .map(|id| {
            graph.get(*id).map(|part| {
                (
                    part.id,
                    part.parent,
                    part.root,
                    part.name.as_str(),
                    part.archived,
                    part.deleted,
                )
            })
        })
        .collect();
    Ok(Hash::of(
        &serde_json::to_vec(&(node.root, &node.path, path)).map_err(ApiError::internal)?,
    ))
}

/// The existing guard has retaken credentials. Its mode and this exact metadata generation must still agree.
pub(crate) async fn unchanged(
    conn: &mut Conn,
    principal: &Principal,
    cut: &Cut,
    now: Time,
) -> ApiResult<()> {
    if cut.as_admin != principal.caller().as_admin()
        || cut.until.is_some_and(|until| until <= now)
        || orch_db::queries::access::epoch(conn).await? != cut.epoch
        || containers::scope_seq(conn, cut.node).await? != cut.membership
    {
        return Err(stale());
    }
    let graph = Graph::load(conn, &[cut.node])
        .await
        .map_err(ApiError::internal)?;
    if hierarchy_stamp(&graph, cut.node)? != cut.hierarchy {
        return Err(stale());
    }
    Ok(())
}

/// The same batched, name-redacted path supplier used by full document reads, on this connection.
/// A document grant is enough to reach this projection; it grants no View on the containing node.
async fn place_bounded(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    retained: bool,
    names: Option<&NameBounds>,
) -> ApiResult<DocumentTreePlace> {
    let retained_anchor: Vec<_> = retained.then_some(node).into_iter().collect();
    orch_db::queries::trace::read_scope(&retained_anchor, async {
        let mut paths = match names {
            Some(names) => {
                crate::routes::nodes::view::ways_with_name_sources(
                    conn,
                    principal,
                    &[node],
                    Policy::default(),
                    &names.nodes,
                )
                .await?
            }
            None => {
                crate::routes::nodes::view::ways(conn, principal, &[node], Policy::default())
                    .await?
            }
        };
        let path = paths.remove(&node).ok_or_else(ApiError::not_found)?;
        let node = path
            .iter()
            .find(|part| part.id == node)
            .cloned()
            .ok_or_else(ApiError::not_found)?;
        Ok(DocumentTreePlace { node, path })
    })
    .await
}

pub(super) struct AvailableEntry {
    until: Option<chrono::DateTime<chrono::Utc>>,
    deleted_by: Option<Id>,
}

/// The native retained eligibility check shared by projection and pre-window selection.
pub(super) async fn available_entries(
    conn: &mut Conn,
    entries: &[domain::Listed],
    options: domain::Options,
) -> ApiResult<HashMap<domain::Item, AvailableEntry>> {
    let ids: Vec<_> = entries
        .iter()
        .filter(|entry| entry.entry.deleted_at.is_some())
        .map(|entry| entry.entry.item.id())
        .collect();
    let retained: HashMap<_, _> = containers::retention_rows(conn, &ids)
        .await?
        .into_iter()
        .map(|row| {
            let item = if row.kind == "container" {
                domain::Item::Container(row.id)
            } else {
                domain::Item::Document(row.id)
            };
            (item, row)
        })
        .collect();
    let mut available = HashMap::new();
    for listed in entries {
        let stored = &listed.entry;
        if stored.deleted_at.is_none() {
            available.insert(
                stored.item,
                AvailableEntry {
                    until: None,
                    deleted_by: None,
                },
            );
            continue;
        }
        let facts = retained.get(&stored.item);
        if facts.is_some_and(|facts| facts.purged_at.is_some()) {
            continue;
        }
        let until = domain::restore_deadline_from(
            stored,
            facts.and_then(|facts| facts.purge_at),
            facts.and_then(|facts| facts.cohort_until),
            options,
        )
        .map_err(ApiError::internal)?;
        if let Some(until) = until {
            available.insert(
                stored.item,
                AvailableEntry {
                    until: Some(until),
                    deleted_by: facts.and_then(|facts| facts.deleted_by),
                },
            );
        }
    }
    Ok(available)
}

async fn projected_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    mut read: domain::TreeRead,
    names: Option<&NameBounds>,
) -> ApiResult<(DocumentTreeRead, Vec<inspection::Info>)> {
    if let Some(names) = names {
        for ancestor in &mut read.ancestors {
            if !names.containers.contains(&ancestor.id) {
                ancestor.basename = None;
                ancestor.title = None;
            }
        }
    }
    let available = available_entries(conn, &read.entries, options(ctx)).await?;
    read.entries
        .retain(|entry| available.contains_key(&entry.entry.item));
    let native: HashMap<_, _> = read
        .entries
        .iter()
        .map(|entry| (entry.entry.item, entry.entry.clone()))
        .collect();
    let people: Vec<_> = available
        .values()
        .filter_map(|entry| entry.deleted_by)
        .collect();
    let people = authorization::refusals::user_refs(conn, &people).await?;
    let mut view: DocumentTreeRead = convert(read)?;
    let mut inspections = Vec::new();
    let mut keep = Vec::new();
    for mut listed in view.entries {
        if let Some(deleted) = listed.entry.deleted_at {
            let item: domain::Item = convert(listed.entry.item)?;
            let Some(stored) = native.get(&item) else {
                continue;
            };
            let facts = available.get(&item);
            let Some(until) = facts.and_then(|facts| facts.until) else {
                continue;
            };
            let banner = orch_api_contract::nodes::DeletedInspection {
                deleted_at: Time::new(deleted),
                deleted_by: facts
                    .and_then(|facts| facts.deleted_by)
                    .and_then(|id| people.get(&id))
                    .cloned(),
                purge_at: Time::new(until),
                readonly: true,
            };
            listed.inspection = Some(banner.clone());
            inspections.push(inspection::Info {
                kind: if matches!(item, domain::Item::Container(_)) {
                    "document_container"
                } else {
                    "document"
                }
                .to_owned(),
                id: item.id(),
                deletion: stored.deletion,
                banner,
                reads: vec![item.id(), stored.node],
            });
        }
        keep.push(listed);
    }
    view.entries = keep;
    document_refs(ctx, conn, principal, &mut view.entries, names).await?;
    Ok((view, inspections))
}

/// Reuse the existing flat list's batched facts and redaction on this same connection/read cut.
/// Retained metadata deliberately never enters this ordinary live-text projection.
async fn document_refs(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    listed: &mut [DocumentTreeListed],
    names: Option<&NameBounds>,
) -> ApiResult<()> {
    let ids: Vec<_> = listed
        .iter()
        .filter(|row| row.entry.deleted_at.is_none() && row.standing.read)
        .filter_map(|row| match row.entry.item {
            DocumentTreeItem::Document(id) => Some(id),
            DocumentTreeItem::Container(_) => None,
        })
        .collect();
    if ids.is_empty() {
        return Ok(());
    }
    let mut held: HashMap<_, _> =
        orch_authz::documents::listed(conn, principal, &ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|row| {
                !row.document.deleted
                    && row.holds(orch_access_contract::permissions::Permission::DocView)
            })
            .map(|row| (row.document.id, row))
            .collect();
    let entries: Vec<_> = documents::get_many(conn, &ids)
        .await?
        .into_iter()
        .filter(|row| row.deleted_at.is_none())
        .filter_map(|row| held.remove(&row.id).map(|standing| (row, standing)))
        .collect();
    let participation: HashMap<_, _> = entries
        .iter()
        .map(|(row, standing)| (row.id, standing.participating))
        .collect();
    let references = match names {
        Some(names) => {
            let allowed: Vec<_> = names
                .nodes
                .iter()
                .copied()
                .map(crate::routes::documents::build::WrittenLabelSource::Node)
                .collect();
            crate::routes::documents::build::ProjectedDocumentRef::capture_many_allowed(
                conn,
                principal,
                &entries,
                ctx.now,
                Policy::default(),
                &allowed,
            )
            .await?
            .0
        }
        None => {
            crate::routes::documents::build::refs(
                conn,
                principal,
                &entries,
                ctx.now,
                Policy::default(),
            )
            .await?
        }
    };
    let mut refs: HashMap<_, _> = references.into_iter().map(|row| (row.id, row)).collect();
    for row in listed {
        if let DocumentTreeItem::Document(id) = row.entry.item
            && let Some(document) = refs.remove(&id)
        {
            row.document = Some(document);
            row.participating = participation.get(&id).copied();
        }
    }
    Ok(())
}

pub(super) async fn read_item(
    conn: &mut Conn,
    principal: &Principal,
    item: domain::Item,
    now: Time,
    retained: bool,
) -> ApiResult<domain::TreeRead> {
    let kind = match item {
        domain::Item::Document(_) => "document",
        domain::Item::Container(_) => "container",
    };
    let row = containers::entry(conn, kind, item.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let mut reads = Vec::new();
    if retained && row.deleted_at.is_some() {
        reads.extend([row.id, row.node_id]);
        reads.extend(
            containers::ancestors_many(conn, &[row.id])
                .await?
                .into_iter()
                .filter(|ancestor| ancestor.deleted_at.is_some())
                .map(|ancestor| ancestor.id),
        );
    }
    let loaded = orch_db::queries::trace::read_scope(
        &reads,
        domain::metadata_in(conn, domain::Actor::Person(principal), item, now, retained),
    )
    .await;
    result(conn, loaded).await
}

pub(super) struct DirectoryReady {
    pub(super) at: DocumentDirectory,
    pub(super) node: Id,
    pub(super) folder: Option<domain::TreeRead>,
    pub(super) cut: Cut,
}

/// Native directory/ordinary authority only; the response renderer remains below.
pub(super) async fn prepare_directory(
    conn: &mut Conn,
    principal: &Principal,
    at: DocumentDirectory,
    retained: bool,
) -> ApiResult<DirectoryReady> {
    let now = principal.now();
    let (node, folder) = match at {
        DocumentDirectory::NodeDocs(node) => {
            authorization::require(
                conn,
                principal,
                node,
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await?;
            (node, None)
        }
        DocumentDirectory::Container(id) => {
            let row = containers::get(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let read =
                read_item(conn, principal, domain::Item::Container(id), now, retained).await?;
            (row.node_id, Some(read))
        }
    };
    let cut = cut(conn, principal, node, now).await?;
    Ok(DirectoryReady {
        at,
        node,
        folder,
        cut,
    })
}

async fn context_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    at: DocumentDirectory,
    retained: bool,
    names: Option<&NameBounds>,
) -> ApiResult<(DocumentDirectoryContext, Cut, Vec<inspection::Info>)> {
    let ready = prepare_directory(conn, principal, at, retained).await?;
    context_ready(ctx, conn, principal, &ready, names).await
}

pub(super) async fn context_ready(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ready: &DirectoryReady,
    names: Option<&NameBounds>,
) -> ApiResult<(DocumentDirectoryContext, Cut, Vec<inspection::Info>)> {
    let now = principal.now();
    let DirectoryReady { at, node, cut, .. } = ready;
    let at = *at;
    let node = *node;
    let (folder, ancestors, inspections) = match &ready.folder {
        Some(read) => {
            let (mut view, inspections) =
                projected_bounded(ctx, conn, principal, read.clone(), names).await?;
            let folder = view.entries.pop().ok_or_else(ApiError::not_found)?;
            (Some(folder), view.ancestors, inspections)
        }
        None => (None, Vec::new(), Vec::new()),
    };
    let cut = cut.clone();
    let inspection = folder.as_ref().and_then(|folder| folder.inspection.clone());
    let DocumentTreePlace {
        node: reference,
        path,
    } = place_bounded(conn, principal, node, inspection.is_some(), names).await?;
    let can_create = if inspection.is_some() {
        false
    } else {
        let decision =
            domain::can_create_in(conn, domain::Actor::Person(principal), convert(at)?, now).await;
        result(conn, decision).await?
    };
    Ok((
        DocumentDirectoryContext {
            at,
            node: reference,
            path,
            folder,
            ancestors,
            can_create,
            inspection,
            membership_seq: cut.membership,
        },
        cut,
        inspections,
    ))
}

pub(super) struct PageStart {
    pub(super) directory: DirectoryReady,
    pub(super) raw: Vec<containers::EntryRow>,
    at: DocumentDirectory,
    query: DocumentTreeListQuery,
    limit: u16,
    start: i64,
    tail: bool,
}

pub(super) struct PageReady {
    pub(super) start: PageStart,
    pub(super) read: domain::TreeRead,
    pub(super) selected: Vec<containers::EntryRow>,
    consumed: usize,
}

/// Native directory/cursor/candidate discovery, before the consuming scope selects rows.
pub(super) async fn page_start(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    at: DocumentDirectory,
    query: &DocumentTreeListQuery,
) -> ApiResult<PageStart> {
    let directory = prepare_directory(conn, principal, at, query.deleted.is_some()).await?;
    let cut = &directory.cut;
    if node.is_some_and(|node| node != cut.node) {
        return Err(if query.cursor.is_some() {
            stale()
        } else {
            ApiError::new(
                ErrorCode::NodeWrongParent,
                "That folder belongs to another document area.",
            )
        });
    }
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let start = offset(
        ctx,
        principal,
        query.cursor.as_deref(),
        at,
        query.deleted.is_some(),
        cut,
    )?;
    let parent = match at {
        DocumentDirectory::NodeDocs(_) => None,
        DocumentDirectory::Container(id) => Some(id),
    };
    let mut raw = containers::page_candidates(
        conn,
        cut.node,
        parent,
        query.deleted.is_some(),
        start,
        (SCAN + 1) as i64,
    )
    .await?;
    let tail = raw.len() > SCAN;
    raw.truncate(SCAN);
    Ok(PageStart {
        directory,
        raw,
        at,
        query: query.clone(),
        limit,
        start,
        tail,
    })
}

/// Scoping and native retained/ordinary eligibility precede the existing output window.
pub(super) async fn page_selected(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    start: PageStart,
    allowed: Option<&HashSet<domain::Item>>,
) -> ApiResult<PageReady> {
    let raw = start
        .raw
        .iter()
        .filter(|row| allowed.is_none_or(|allowed| allowed.contains(&native_item(row))))
        .cloned()
        .collect();
    let now = ctx.app.clock.time();
    let selection = domain::select_entries_in(
        conn,
        domain::Actor::Person(principal),
        raw,
        now,
        start.query.deleted.is_some(),
    )
    .await;
    let mut selection = result(conn, selection).await?;
    let available = available_entries(conn, selection.entries(), options(ctx)).await?;
    selection.retain(|entry| available.contains_key(&entry.entry.item));
    let visible: HashSet<_> = selection
        .entries()
        .iter()
        .map(|entry| entry.entry.item)
        .collect();
    let mut selected = HashSet::new();
    let mut consumed = 0;
    for candidate in &start.raw {
        consumed += 1;
        let item = native_item(candidate);
        if visible.contains(&item) {
            selected.insert(item);
        }
        if selected.len() >= usize::from(start.limit) {
            break;
        }
    }
    selection.retain(|entry| selected.contains(&entry.entry.item));
    let read = domain::project_selected_in(
        conn,
        domain::Actor::Person(principal),
        selection,
        now,
        start.query.deleted.is_some(),
    )
    .await;
    let read = result(conn, read).await?;
    let selected = start
        .raw
        .iter()
        .filter(|row| selected.contains(&native_item(row)))
        .cloned()
        .collect();
    Ok(PageReady {
        start,
        read,
        selected,
        consumed,
    })
}
fn native_item(row: &containers::EntryRow) -> domain::Item {
    if row.kind == "container" {
        domain::Item::Container(row.id)
    } else {
        domain::Item::Document(row.id)
    }
}

/// The original personalized renderer, once from the final native selected window.
pub(super) async fn page_ready(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ready: &PageReady,
    names: Option<&NameBounds>,
) -> ApiResult<Projection<DocumentTreePage>> {
    let (directory, cut, mut inspections) =
        context_ready(ctx, conn, principal, &ready.start.directory, names).await?;
    let (view, infos) = projected_bounded(ctx, conn, principal, ready.read.clone(), names).await?;
    inspections.extend(infos);
    let mut visible: HashMap<_, _> = view
        .entries
        .into_iter()
        .map(|entry| (entry.entry.item, entry))
        .collect();
    let mut items = Vec::new();
    for candidate in &ready.start.raw[..ready.consumed] {
        let item: DocumentTreeItem = convert(native_item(candidate))?;
        if let Some(entry) = visible.remove(&item) {
            items.push(entry);
        }
    }
    let more = ready.start.tail || ready.consumed < ready.start.raw.len();
    let next_cursor = if more {
        let advanced = ready
            .start
            .start
            .checked_add(i64::try_from(ready.consumed).map_err(ApiError::internal)?)
            .ok_or_else(stale)?;
        if advanced <= ready.start.start {
            return Err(stale());
        }
        Some(next(
            ctx,
            principal,
            ready.start.at,
            ready.start.query.deleted.is_some(),
            &cut,
            advanced,
        )?)
    } else {
        None
    };
    let feed = Feed::DocumentTree(DocumentTreeView::Directory {
        at: ready.start.at,
        deleted: ready.start.query.deleted,
        cursor: ready.start.query.cursor.clone(),
        limit: Some(ready.start.limit),
    })
    .to_string();
    let value = DocumentTreePage {
        items,
        next_cursor,
        feed,
        directory,
        ancestors: view.ancestors,
    };
    bounded(&value)?;
    Ok(Projection {
        value,
        cut,
        inspections,
    })
}

pub(crate) async fn page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    at: DocumentDirectory,
    query: &DocumentTreeListQuery,
) -> ApiResult<Projection<DocumentTreePage>> {
    let start = page_start(ctx, conn, principal, node, at, query).await?;
    let ready = page_selected(ctx, conn, principal, start, None).await?;
    page_ready(ctx, conn, principal, &ready, None).await
}

pub(crate) async fn entry(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    item: DocumentTreeItem,
    retained: bool,
) -> ApiResult<Projection<DocumentTreeRead>> {
    entry_bounded(ctx, conn, principal, item, retained, None).await
}

/// The same native entry/inspection/context projection with explicit protected-name bounds.
pub(crate) async fn entry_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    item: DocumentTreeItem,
    retained: bool,
    names: Option<&NameBounds>,
) -> ApiResult<Projection<DocumentTreeRead>> {
    let native: domain::Item = convert(item)?;
    let read = read_item(conn, principal, native, ctx.app.clock.time(), retained).await?;
    let (mut view, mut inspections) = projected_bounded(ctx, conn, principal, read, names).await?;
    let node = view
        .entries
        .first()
        .ok_or_else(ApiError::not_found)?
        .entry
        .node;
    let cut = cut(conn, principal, node, principal.now()).await?;
    if let DocumentTreeItem::Container(id) = item {
        let (directory, _, extra) = context_bounded(
            ctx,
            conn,
            principal,
            DocumentDirectory::Container(id),
            retained,
            names,
        )
        .await?;
        view.place = Some(DocumentTreePlace {
            node: directory.node.clone(),
            path: directory.path.clone(),
        });
        view.directory = Some(directory);
        for info in extra {
            if !inspections.iter().any(|prior| prior.id == info.id) {
                inspections.push(info);
            }
        }
    } else {
        let inspected = view
            .entries
            .first()
            .is_some_and(|row| row.inspection.is_some());
        view.place = Some(place_bounded(conn, principal, node, inspected, names).await?);
    }
    view.feed = Some(
        Feed::DocumentTree(DocumentTreeView::Entry {
            item,
            inspect_deleted: retained,
        })
        .to_string(),
    );
    bounded(&view)?;
    Ok(Projection {
        value: view,
        cut,
        inspections,
    })
}

/// Existing full document answers gain placement without another connection or authority source.
pub(crate) async fn location(
    conn: &mut Conn,
    principal: &Principal,
    row: &documents::DocumentRow,
    now: Time,
) -> ApiResult<DocumentLocation> {
    let retained = row.deleted_at.is_some() && orch_db::queries::trace::retained_read(row.id);
    let read = read_item(
        conn,
        principal,
        domain::Item::Document(row.id),
        now,
        retained,
    )
    .await?;
    let entry = read.entries.first().ok_or_else(ApiError::not_found)?;
    if entry.entry.node != row.node_id
        || entry.entry.title != row.title
        || entry.entry.revision != row.doc_seq
    {
        return Err(stale());
    }
    Ok(DocumentLocation {
        parent: convert(entry.entry.parent)?,
        basename: entry.entry.basename.clone(),
        ancestors: convert(read.ancestors)?,
    })
}

/// Keep the ordinary placement/standing cut while supplying only admitted live folder names.
/// Basename of the document itself is its own stored content; ancestor names belong to folders.
pub(crate) async fn location_with_name_sources(
    conn: &mut Conn,
    principal: &Principal,
    row: &documents::DocumentRow,
    now: Time,
    allowed: &std::collections::BTreeSet<Id>,
) -> ApiResult<DocumentLocation> {
    let mut shown = location(conn, principal, row, now).await?;
    for ancestor in &mut shown.ancestors {
        if !allowed.contains(&ancestor.id) {
            ancestor.basename = None;
            ancestor.title = None;
        }
    }
    Ok(shown)
}

/// API-only current view; the durable domain receipt remains caller-independent.
/// The same native receipt with factual protected-label bounds from its operation.
pub(crate) async fn receipt_bounded(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    receipt: domain::Receipt,
    names: Option<&NameBounds>,
) -> ApiResult<DocumentTreeReceipt> {
    let single = receipt.entries.len() == 1;
    let mut rows = Vec::new();
    for entry in &receipt.entries {
        let kind = match entry.item {
            domain::Item::Document(_) => "document",
            domain::Item::Container(_) => "container",
        };
        if let Some(row) = containers::entry(conn, kind, entry.item.id()).await? {
            rows.push(row);
        }
    }
    let read = domain::project_entries_in(
        conn,
        domain::Actor::Person(principal),
        rows,
        ctx.app.clock.time(),
        true,
    )
    .await;
    let read = result(conn, read).await?;
    let (mut view, _) = projected_bounded(ctx, conn, principal, read, names).await?;
    if single && let [entry] = view.entries.as_slice() {
        view.place = Some(
            place_bounded(
                conn,
                principal,
                entry.entry.node,
                entry.inspection.is_some(),
                names,
            )
            .await?,
        );
    }
    let mut answer: DocumentTreeReceipt = convert(receipt)?;
    answer.view = view;
    Ok(answer)
}

/// A raw shared path may be named only when this person reads every actual folder component.
pub(crate) async fn named_document_paths(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashSet<Id>> {
    let parents = containers::document_parents(conn, ids).await?;
    let folders: HashMap<_, _> = containers::ancestors_many(conn, ids)
        .await?
        .into_iter()
        .map(|row| (row.id, row))
        .collect();
    let folder_ids: Vec<_> = folders.keys().copied().collect();
    let readable: HashSet<_> =
        orch_authz::documents::listed(conn, principal, &folder_ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|row| {
                !row.document.deleted
                    && row.holds(orch_access_contract::permissions::Permission::DocView)
            })
            .map(|row| row.document.id)
            .collect();
    Ok(parents
        .into_iter()
        .filter_map(|(id, mut parent)| {
            let mut seen = HashSet::new();
            while let Some(folder) = parent {
                if !seen.insert(folder) || !readable.contains(&folder) {
                    return None;
                }
                parent = folders.get(&folder)?.parent_container_id;
            }
            Some(id)
        })
        .collect())
}
