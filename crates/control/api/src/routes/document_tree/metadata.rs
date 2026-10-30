//! One person-projected metadata cut for HTTP, operation answers and ephemeral views.

use super::{convert, options, result};
use crate::middleware::{
    authentication::Via,
    authorization, inspection,
    requests::{ApiError, ApiResult},
};
use crate::registry::Ctx;
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
        Via::Session { session_id } => serde_json::json!({"session":session_id}),
        Via::Token {
            token_id,
            scopes,
            node_id,
        } => serde_json::json!({"token":token_id,"scopes":scopes,"node":node_id}),
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
async fn place(
    conn: &mut Conn,
    principal: &Principal,
    node: Id,
    retained: bool,
) -> ApiResult<DocumentTreePlace> {
    let retained_anchor: Vec<_> = retained.then_some(node).into_iter().collect();
    orch_db::queries::trace::read_scope(&retained_anchor, async {
        let path = crate::routes::nodes::view::ways(conn, principal, &[node], Policy::default())
            .await?
            .remove(&node)
            .ok_or_else(ApiError::not_found)?;
        let node = path
            .iter()
            .find(|part| part.id == node)
            .cloned()
            .ok_or_else(ApiError::not_found)?;
        Ok(DocumentTreePlace { node, path })
    })
    .await
}

/// Add only retained facts for entries already admitted by the native person projection.
async fn projected(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    read: domain::TreeRead,
) -> ApiResult<(DocumentTreeRead, Vec<inspection::Info>)> {
    let native: HashMap<_, _> = read
        .entries
        .iter()
        .map(|entry| (entry.entry.item, entry.entry.clone()))
        .collect();
    let ids: Vec<_> = read
        .entries
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
    let people: Vec<_> = retained.values().filter_map(|row| row.deleted_by).collect();
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
            let facts = retained.get(&item);
            if facts.is_some_and(|facts| facts.purged_at.is_some()) {
                continue;
            }
            let until = domain::restore_deadline_from(
                stored,
                facts.and_then(|facts| facts.purge_at),
                facts.and_then(|facts| facts.cohort_until),
                options(ctx),
            )
            .map_err(ApiError::internal)?;
            let Some(until) = until else { continue };
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
    document_refs(ctx, conn, principal, &mut view.entries).await?;
    Ok((view, inspections))
}

/// Reuse the existing flat list's batched facts and redaction on this same connection/read cut.
/// Retained metadata deliberately never enters this ordinary live-text projection.
async fn document_refs(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    listed: &mut [DocumentTreeListed],
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
    let mut refs: HashMap<_, _> = crate::routes::documents::build::refs(
        conn,
        principal,
        &entries,
        ctx.now,
        Policy::default(),
    )
    .await?
    .into_iter()
    .map(|row| (row.id, row))
    .collect();
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

async fn read_item(
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

async fn context(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    at: DocumentDirectory,
    retained: bool,
) -> ApiResult<(DocumentDirectoryContext, Cut, Vec<inspection::Info>)> {
    let now = ctx.app.clock.time();
    let (node, folder, ancestors, inspections) = match at {
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
            (node, None, Vec::new(), Vec::new())
        }
        DocumentDirectory::Container(id) => {
            let row = containers::get(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let read =
                read_item(conn, principal, domain::Item::Container(id), now, retained).await?;
            let (mut view, inspections) = projected(ctx, conn, principal, read).await?;
            let folder = view.entries.pop().ok_or_else(ApiError::not_found)?;
            (row.node_id, Some(folder), view.ancestors, inspections)
        }
    };
    let cut = cut(conn, principal, node, now).await?;
    let inspection = folder.as_ref().and_then(|folder| folder.inspection.clone());
    let DocumentTreePlace {
        node: reference,
        path,
    } = place(conn, principal, node, inspection.is_some()).await?;
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

pub(crate) async fn page(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    node: Option<Id>,
    at: DocumentDirectory,
    query: &DocumentTreeListQuery,
) -> ApiResult<Projection<DocumentTreePage>> {
    let deleted = query.deleted.is_some();
    let (directory, cut, mut inspections) = context(ctx, conn, principal, at, deleted).await?;
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
    let start = offset(ctx, principal, query.cursor.as_deref(), at, deleted, &cut)?;
    let parent = match at {
        DocumentDirectory::NodeDocs(_) => None,
        DocumentDirectory::Container(id) => Some(id),
    };
    let mut raw =
        containers::page_candidates(conn, cut.node, parent, deleted, start, (SCAN + 1) as i64)
            .await?;
    let tail = raw.len() > SCAN;
    raw.truncate(SCAN);
    let read = domain::project_entries_in(
        conn,
        domain::Actor::Person(principal),
        raw.clone(),
        ctx.app.clock.time(),
        deleted,
    )
    .await;
    let read = result(conn, read).await?;
    let (mut view, infos) = projected(ctx, conn, principal, read).await?;
    let mut visible: HashMap<_, _> = view
        .entries
        .drain(..)
        .map(|entry| (entry.entry.item, entry))
        .collect();
    let mut items = Vec::new();
    let mut consumed = 0_usize;
    for candidate in &raw {
        consumed += 1;
        let item = if candidate.kind == "container" {
            DocumentTreeItem::Container(candidate.id)
        } else {
            DocumentTreeItem::Document(candidate.id)
        };
        if let Some(row) = visible.remove(&item) {
            items.push(row);
        }
        if items.len() >= usize::from(limit) {
            break;
        }
    }
    let selected: HashSet<_> = items
        .iter()
        .map(|entry| match entry.entry.item {
            DocumentTreeItem::Document(id) | DocumentTreeItem::Container(id) => id,
        })
        .collect();
    inspections.extend(infos.into_iter().filter(|info| selected.contains(&info.id)));
    let ancestor_ids: HashSet<_> =
        containers::ancestors_many(conn, &selected.iter().copied().collect::<Vec<_>>())
            .await?
            .into_iter()
            .map(|row| row.id)
            .collect();
    view.ancestors
        .retain(|ancestor| ancestor_ids.contains(&ancestor.id));
    let more = tail || consumed < raw.len();
    let next_cursor = if more {
        let advanced = start
            .checked_add(i64::try_from(consumed).map_err(ApiError::internal)?)
            .ok_or_else(stale)?;
        if advanced <= start {
            return Err(stale());
        }
        Some(next(ctx, principal, at, deleted, &cut, advanced)?)
    } else {
        None
    };
    let feed = Feed::DocumentTree(DocumentTreeView::Directory {
        at,
        deleted: query.deleted,
        cursor: query.cursor.clone(),
        limit: Some(limit),
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

pub(crate) async fn entry(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    item: DocumentTreeItem,
    retained: bool,
) -> ApiResult<Projection<DocumentTreeRead>> {
    let native: domain::Item = convert(item)?;
    let read = read_item(conn, principal, native, ctx.app.clock.time(), retained).await?;
    let (mut view, mut inspections) = projected(ctx, conn, principal, read).await?;
    let node = view
        .entries
        .first()
        .ok_or_else(ApiError::not_found)?
        .entry
        .node;
    let cut = cut(conn, principal, node, ctx.app.clock.time()).await?;
    if let DocumentTreeItem::Container(id) = item {
        let (directory, _, extra) = context(
            ctx,
            conn,
            principal,
            DocumentDirectory::Container(id),
            retained,
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
        view.place = Some(place(conn, principal, node, inspected).await?);
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

/// API-only current view; the durable domain receipt remains caller-independent.
pub(crate) async fn receipt(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    receipt: domain::Receipt,
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
    let (mut view, _) = projected(ctx, conn, principal, read).await?;
    if single && let [entry] = view.entries.as_slice() {
        view.place = Some(
            place(
                conn,
                principal,
                entry.entry.node,
                entry.inspection.is_some(),
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
