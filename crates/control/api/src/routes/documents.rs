//! Documents: listing beside a node, blocks and their operations, comments, versions and what changed between two of them, files, export as Markdown, HTML and Typst with what it is asked to hold, import, moving and deleting.
//!
//! A document is a page of blocks written beside a node. This module makes
//! one (from blocks, or from Markdown, which is the way in for a script and
//! for an import), lists those beside a node that the caller may read, reads
//! one whole or what changed in it since a sequence, renames it and changes
//! its description, says how far the caller has had it on screen, moves it to
//! another node of its tree, and
//! deletes it and brings it back. The submodules hold the rest: `ops`
//! applies batches of changes, `history` lists versions, says what changed
//! between two and puts one back, `comments` holds the discussions beside
//! the text, `export` writes a document out, `files` puts files into it and
//! gives them back, and [`page`] is its permission page. Who may do what is
//! the access engine's, asked through [`crate::middleware::authorization`];
//! what a document may hold where it stands is `limits`; the names a reader
//! is shown are put in by `build`.
//!
//! Every change is one transaction that stamps the document from its own
//! sequence under the lock on its row and announces it when it commits, which
//! is how every open copy of a document stays the same document.

pub(crate) mod build;
mod comments;
mod convert;
mod creation;
pub(crate) mod export;
mod files;
mod history;
mod imports;
mod limits;
mod listing;
mod mentions;
mod notices;
mod ops;
pub mod page;
pub(crate) mod references;
mod subscription;

use std::collections::{BTreeMap, HashMap};

use crate::middleware::authorization::{self, exact, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::{actor_name, invalid, placed};
use crate::sign_in::origin;
use chrono::{DateTime, Utc};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::decisions::Decision;
use orch_access_contract::permissions::Permission;
use orch_api_contract::access::{PREVIEW_MAX_PEOPLE, PagePreview, PreviewPerson};
use orch_api_contract::common::{ErrorCode, Listed as ListedItem, Page, PageQuery};
use orch_api_contract::documents::{
    DOCUMENT_SYNC_MAX_CHANGES, DOCUMENT_TITLE_MAX_CHARS, Document, DocumentDelete,
    DocumentFileChange, DocumentFileChangeKind, DocumentFileDelete, DocumentGet, DocumentMove,
    DocumentMoveBody, DocumentMovePreview, DocumentOp, DocumentRef, DocumentRestore, DocumentSeen,
    DocumentSeenBody, DocumentSync, DocumentSyncGet, DocumentSyncGetQuery, DocumentUpdate,
    DocumentUpdateBody, NewBlock, NewDocumentAccess, NodeDocumentCreate, NodeDocumentCreateBody,
    NodeDocumentList, NodeDocumentListQuery, NodeDocumentPage, NodeDocumentPagesGet,
    NodeDocumentPagesGetQuery,
};
use orch_api_contract::routes::Endpoint;
use orch_authz::documents::{self, Listed, State};
use orch_authz::pages;
use orch_authz::usable::ResourcePolicy;
use orch_authz::{Act, Policy, Principal};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::documents::batch::{self, Outcome};
use orch_db::queries::documents::discussions;
use orch_db::queries::documents::{self as rows, DocumentRow, ListCursor, ListWhere};
use orch_db::{Conn, DbError};
use orch_documents::markdown::{self, Directory};
use orch_documents::placement::{self, Destination, MoveRefusal};
use orch_documents::versions::Writer;
use orch_engine::document_tree as document_tree_domain;
use orch_types::{Id, Json, Time};
use serde_json::json;

use limits::DocSettings;

/// How many rounds of reading a list take at most to find a page of documents
/// the caller may read: a person who may read few of many is shown the page
/// they have so far, with the cursor to go on from, instead of waiting for the
/// whole list to be read.
const LIST_ROUNDS_MAX: usize = 20;

/// Carries only the fact that an authorized private row was encountered on an earlier readable page.
const WITH_PRIVATE_CURSOR: &str = "with-private:";
/// A sparse list advances by a scan count relative to its last readable cursor, not a hidden timestamp.
const SCAN_CURSOR: &str = "scan:";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    mentions::mount(registry);
    creation::mount(registry);
    convert::mount(registry);
    imports::mount(registry);
    registry
        .on(NodeDocumentList, list)
        .on(NodeDocumentPagesGet, first_pages)
        .on(NodeDocumentCreate, create)
        .on(
            orch_api_contract::document_tree::DocumentContainerDocumentCreate,
            create,
        )
        .on(DocumentGet, get)
        .on(DocumentSyncGet, sync)
        .on(DocumentUpdate, update)
        .on(DocumentSeen, seen)
        .on(DocumentDelete, delete)
        .on(DocumentRestore, restore)
        .on(DocumentMovePreview, move_preview)
        .on(DocumentMove, move_to)
        .on(DocumentFileDelete, file_delete);
    ops::mount(registry);
    history::mount(registry);
    comments::mount(registry);
    export::mount(registry);
    files::mount(registry);
    subscription::mount(registry);
    references::mount(registry);
}

// ---------------------------------------------------------------- shared by the submodules

/// Writes the audit row of an act of the caller on a document, in the caller's
/// transaction. A document's rows stand beside no node: they are lines of its
/// own history, and of the audit trail of the root org it lies in.
async fn record(
    ctx: &Ctx,
    conn: &mut Conn,
    action: AuditAction,
    document: &DocumentRow,
    summary: &str,
    details: Json,
) -> ApiResult<()> {
    let mut entry = Entry::new(
        Actor::User(ctx.user()?.id()),
        action.as_str(),
        action.object_kind().unwrap_or("document"),
        Some(document.id),
        summary,
    );
    entry.history = action.is_history_line();
    entry.node_id = Some(document.node_id);
    entry.org_id = Some(document.org_id);
    entry.details = details;
    if action == AuditAction::DocumentDeleted {
        let days =
            i64::try_from(ctx.app.config.retention.soft_delete_days).map_err(ApiError::internal)?;
        entry.details["purge_at"] = json!(ctx.now + chrono::Duration::days(days));
    }
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(())
}

/// The document `id` that is there: not deleted.
async fn live(conn: &mut Conn, id: Id) -> ApiResult<DocumentRow> {
    rows::get(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none() || orch_db::queries::trace::retained_read(row.id))
        .ok_or_else(ApiError::not_found)
}

/// The document `id` that is there, with its row locked until the transaction
/// ends.
async fn live_locked(conn: &mut Conn, id: Id) -> ApiResult<DocumentRow> {
    rows::lock(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)
}

/// A title as it is kept: without the space around it, one to the most characters.
fn clean_title(path: &str, title: &str) -> ApiResult<String> {
    let title = title.trim();
    if title.is_empty() {
        return Err(invalid(path, "must not be empty"));
    }
    let most = usize::try_from(DOCUMENT_TITLE_MAX_CHARS).unwrap_or(usize::MAX);
    if title.chars().count() > most {
        return Err(invalid(
            path,
            &format!("must be at most {DOCUMENT_TITLE_MAX_CHARS} characters"),
        ));
    }
    Ok(title.to_owned())
}

/// The moment and the identifier a list resumes after: the moment is written in
/// microseconds, which is what the database keeps.
fn read_cursor(path: &str, cursor: Option<&str>) -> ApiResult<Option<(DateTime<Utc>, Id)>> {
    let Some(text) = cursor else {
        return Ok(None);
    };
    let bad = || invalid(path, "is not a cursor of this list");
    let (micros, id) = text.split_once('|').ok_or_else(bad)?;
    let at = micros
        .parse::<i64>()
        .ok()
        .and_then(DateTime::<Utc>::from_timestamp_micros)
        .ok_or_else(bad)?;
    Ok(Some((at, id.parse().map_err(|_| bad())?)))
}

/// A cursor, as [`read_cursor`] reads it.
fn write_cursor(at: DateTime<Utc>, id: Id) -> String {
    format!("{}|{id}", at.timestamp_micros())
}

// ---------------------------------------------------------------- listing and reading

/// Batch only the transport: every place retains its native permission and document-page supplier.
async fn first_pages(
    ctx: Ctx,
    query: NodeDocumentPagesGetQuery,
    (): (),
) -> ApiResult<Vec<NodeDocumentPage>> {
    let ids: Vec<Id> = query
        .node_ids
        .split(',')
        .map(|id| {
            id.parse()
                .map_err(|_| invalid("node_ids", "must contain node identifiers"))
        })
        .collect::<ApiResult<_>>()?;
    // A batch has the ordinary page-size bound, keeping a transport aggregation bounded too.
    if ids.is_empty()
        || ids.len() > orch_api_contract::common::MAX_LIMIT as usize
        || ids.first() != Some(&ctx.id("id")?)
    {
        return Err(invalid(
            "node_ids",
            "must begin with the path node and fit one page",
        ));
    }
    let mut pages = Vec::with_capacity(ids.len());
    for id in ids {
        let mut child = ctx.for_id(id);
        child.route = NodeDocumentList::ROUTE;
        let read = async {
            authorization::require_on_path(&child, "node.view").await?;
            list(
                child,
                NodeDocumentListQuery {
                    limit: query.limit,
                    ..Default::default()
                },
                (),
            )
            .await
        }
        .await;
        pages.push(match read {
            Ok(page) => NodeDocumentPage {
                node_id: id,
                page: Some(page),
                error: None,
            },
            Err(error) => NodeDocumentPage {
                node_id: id,
                page: None,
                error: Some(*error.problem),
            },
        });
    }
    Ok(pages)
}

/// The documents beside a node that the caller may read, the most recently
/// changed first.
async fn list(
    ctx: Ctx,
    query: NodeDocumentListQuery,
    (): (),
) -> ApiResult<Page<ListedItem<DocumentRef>>> {
    let node = ctx.id("id")?;
    let paging = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = i64::from(paging.limit());
    let private_offset = listing::offset(query.cursor.as_deref())?;
    if query.participating.unwrap_or(false) && private_offset.is_some() {
        return Err(invalid(
            "cursor",
            "is not a cursor of the ordinary participation list",
        ));
    }
    let mut have_private = query
        .cursor
        .as_deref()
        .is_some_and(|cursor| cursor.starts_with(WITH_PRIVATE_CURSOR));
    let raw_readable_cursor = query
        .cursor
        .as_deref()
        .map(|cursor| cursor.strip_prefix(WITH_PRIVATE_CURSOR).unwrap_or(cursor));
    let (readable_cursor, mut first_offset) = if let Some(scan) =
        raw_readable_cursor.and_then(|cursor| cursor.strip_prefix(SCAN_CURSOR))
    {
        let (count, base) = scan
            .split_once('|')
            .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
        let offset = count
            .parse::<i64>()
            .ok()
            .filter(|value| *value >= 0)
            .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
        (if base.is_empty() { None } else { Some(base) }, offset)
    } else {
        (raw_readable_cursor, 0)
    };
    let mut scanned = first_offset;
    let mut after = if private_offset.is_none() {
        read_cursor("cursor", readable_cursor)?.map(|(at, id)| ListCursor { at, id })
    } else {
        None
    };
    let want_deleted = query.deleted.is_some();
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    placed(&mut conn, node).await?;
    if let Some(offset) = private_offset {
        if want_deleted {
            return Err(invalid("cursor", "is not a cursor of this list"));
        }
        return listing::append(
            &mut conn,
            &principal,
            listing::PrivateList {
                node,
                under: query.under.unwrap_or(false),
                limit,
                offset,
            },
            Vec::new(),
        )
        .await;
    }
    let policy = Policy::default();
    let mut chosen: Vec<(DocumentRow, Listed)> = Vec::new();
    let mut more = false;
    let mut exhausted = false;
    for _ in 0..LIST_ROUNDS_MAX {
        let found = rows::list_beside(
            &mut conn,
            &ListWhere {
                node,
                under: query.under.unwrap_or(false),
                deleted: want_deleted,
                after,
                offset: std::mem::take(&mut first_offset),
                limit: limit + 1,
            },
        )
        .await?;
        exhausted = i64::try_from(found.len()).unwrap_or(i64::MAX) <= limit;
        let ids: Vec<Id> = found.iter().map(|row| row.id).collect();
        let standing: HashMap<Id, Listed> = documents::listed(&mut conn, &principal, &ids, policy)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|each| (each.document.id, each))
            .collect();
        have_private |= !want_deleted
            && standing.values().any(|held| {
                !held.holds(Permission::DocView) && held.holds(Permission::AccessManage)
            });
        for row in found {
            scanned = scanned
                .checked_add(1)
                .ok_or_else(|| invalid("cursor", "is not a cursor of this list"))?;
            after = Some(ListCursor {
                at: row.updated_at,
                id: row.id,
            });
            // Whoever could have deleted a document sees it among the deleted.
            let needs = if want_deleted {
                Permission::DocManage
            } else {
                Permission::DocView
            };
            let Some(held) = standing.get(&row.id).filter(|each| {
                each.holds(needs) && (!query.participating.unwrap_or(false) || each.participating)
            }) else {
                continue;
            };
            if i64::try_from(chosen.len()).unwrap_or(i64::MAX) == limit {
                more = true;
                break;
            }
            chosen.push((row, held.clone()));
        }
        if more || exhausted {
            break;
        }
    }
    let next_cursor = if more {
        chosen
            .last()
            .map(|(row, _)| write_cursor(row.updated_at, row.id))
    } else if exhausted {
        None
    } else if let Some((row, _)) = chosen.last() {
        Some(write_cursor(row.updated_at, row.id))
    } else {
        Some(format!(
            "{SCAN_CURSOR}{scanned}|{}",
            readable_cursor.unwrap_or_default()
        ))
    };
    let next_cursor = next_cursor.map(|cursor| {
        if have_private {
            format!("{WITH_PRIVATE_CURSOR}{cursor}")
        } else {
            cursor
        }
    });
    let readable_end = chosen.last().map(|(row, _)| {
        format!(
            "{WITH_PRIVATE_CURSOR}{}",
            write_cursor(row.updated_at, row.id)
        )
    });
    let items = build::refs(&mut conn, &principal, &chosen, ctx.now, policy)
        .await?
        .into_iter()
        .map(ListedItem::visible)
        .collect();
    if want_deleted
        || !have_private
        || next_cursor.is_some()
        || query.participating.unwrap_or(false)
    {
        return Ok(Page { items, next_cursor });
    }
    let mut page = listing::append(
        &mut conn,
        &principal,
        listing::PrivateList {
            node,
            under: query.under.unwrap_or(false),
            limit,
            offset: 0,
        },
        items,
    )
    .await?;
    // A full readable page has no room to show its private group yet. Resume from the last
    // readable row so the first private page has room, also for content-only list consumers.
    if i64::try_from(page.items.len()).unwrap_or(i64::MAX) == limit
        && page.items.iter().all(|row| row.as_visible().is_some())
        && page.next_cursor.is_some()
    {
        page.next_cursor = readable_end;
    }
    Ok(page)
}

/// The people named in Markdown, looked up by the names the text uses.
struct Names {
    users: HashMap<String, Id>,
}

impl Directory for Names {
    fn user_id(&self, username: &str) -> Option<Id> {
        self.users.get(&username.to_ascii_lowercase()).copied()
    }
}

async fn names_in(conn: &mut Conn, text: &str) -> ApiResult<Names> {
    let candidates = markdown::mentioned_names(text);
    let found = rows::users_by_username(conn, &candidates).await?;
    Ok(Names {
        users: found.into_iter().collect(),
    })
}

/// Operations that put blocks at the top of an empty document, one after the other.
fn creation_ops(blocks: Vec<NewBlock>) -> Vec<DocumentOp> {
    let mut previous: Option<Id> = None;
    blocks
        .into_iter()
        .map(|block| {
            let id = block.id;
            let op = DocumentOp::Insert {
                block,
                parent_id: None,
                after_id: previous,
            };
            previous = Some(id);
            op
        })
        .collect()
}

/// Makes a document beside a node, from blocks or from Markdown. The caller
/// owns it, whatever they hold on the documents of others there.
async fn create(ctx: Ctx, (): (), body: NodeDocumentCreateBody) -> ApiResult<Document> {
    orch_db::queries::transaction_guard::request_scope(
        orch_db::queries::transaction_guard::Scope::Creation,
        create_prepared(ctx, body, None, Vec::new(), None, Vec::new()),
    )
    .await
}

/// The ordinary guarded creation producer, also used for imported and copied current material.
async fn create_prepared(
    mut ctx: Ctx,
    body: NodeDocumentCreateBody,
    identity: Option<Id>,
    copied_files: Vec<rows::HeldFile>,
    checked: Option<(orch_db::Tx, Principal)>,
    receipt_warnings: Vec<String>,
) -> ApiResult<Document> {
    let title = clean_title("title", &body.title)?;
    let (mut tx, principal) = match checked {
        Some((tx, principal)) => {
            ctx.now = principal.now().get();
            (tx, principal)
        }
        None => {
            let (tx, guard) = transaction_guard::begin(&ctx).await?;
            ctx.now = guard.now().get();
            (tx, guard.principal().clone())
        }
    };
    let parent = creation::parent(&ctx, &mut tx, &principal).await?;
    let node = match parent {
        document_tree_domain::Directory::NodeDocs(node) => node,
        document_tree_domain::Directory::Container(id) => {
            rows::containers::get(&mut tx, id)
                .await?
                .ok_or_else(ApiError::not_found)?
                .node_id
        }
    };
    placed(&mut tx, node).await?;
    let settings = DocSettings::at(&mut tx, node).await?;
    if !settings.enabled {
        return Err(limits::disabled(&mut tx, &principal, &settings, Policy::default()).await);
    }
    let blocks = match (body.blocks, body.markdown) {
        (Some(_), Some(_)) => {
            return Err(invalid("markdown", "give blocks or markdown, not both"));
        }
        (Some(blocks), None) => blocks,
        (None, Some(text)) => {
            let names = names_in(&mut tx, &text).await?;
            markdown::read(&text, &names)
        }
        (None, None) => Vec::new(),
    };
    let blocks = build::label_written(&mut tx, &principal, blocks).await?;
    let id = identity.unwrap_or_else(orch_types::new_id);
    let maker = principal.user();
    let created = document_tree_domain::create_text_in(
        &mut tx,
        document_tree_domain::Actor::Person(&principal),
        parent,
        id,
        &title,
        None,
        Time::new(ctx.now),
    )
    .await;
    let created = crate::routes::document_tree::result(&mut tx, created).await?;
    if !created.created {
        let listed = build::standing(&mut tx, &principal, id, Policy::default()).await?;
        let made = build::document(
            &mut tx,
            &principal,
            &created.document,
            &listed,
            ctx.now,
            Policy::default(),
        )
        .await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(made);
    }
    let mut total = 0_i64;
    for file in &copied_files {
        if file.file.size > settings.file_max_bytes.value {
            return Err(limits::too_large(
                &mut tx,
                &principal,
                &settings,
                limits::Limit::FileBytes,
                Policy::default(),
            )
            .await);
        }
        total = total.saturating_add(file.file.size);
    }
    if total > settings.files_max_bytes.value {
        return Err(limits::too_large(
            &mut tx,
            &principal,
            &settings,
            limits::Limit::FilesBytes,
            Policy::default(),
        )
        .await);
    }
    convert::register(&ctx, &mut tx, &copied_files, ctx.now).await?;
    rows::copy::install_files(&mut tx, id, &copied_files, Some(maker), None, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    let policy = Policy::default();
    if body.access == Some(NewDocumentAccess::Private) {
        let change = orch_access_contract::pages::PageChange {
            whole: Some(orch_access_contract::pages::Whole::Private),
            ..Default::default()
        };
        let home = ResourcePolicy {
            owners_name_outsiders: crate::routes::nodes::policy_at(&mut tx, node)
                .await?
                .owners_name_outsiders,
            shared_subscriptions: None,
        };
        let object = orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Document,
            id,
        };
        if let Err(error) =
            pages::change_resource(&mut tx, &principal, object, &change, &home, &origin(&ctx)).await
        {
            return Err(refusals::change_failed(&mut tx, error).await);
        }
    }
    let ops = creation_ops(blocks);
    let rules = settings.version_rules(ctx.app.config.documents.version_window_secs);
    // Creation may contain the place's full block allowance. Each ordinary operation batch keeps
    // its own request bound; all chunks remain in this one transaction, so no partial page is visible.
    let chunk_max = usize::try_from(orch_api_contract::documents::DOCUMENT_OPS_MAX)
        .map_err(ApiError::internal)?;
    for chunk in ops.chunks(chunk_max) {
        let outcome = batch::apply(
            &mut tx,
            id,
            &Writer::User(maker),
            None,
            chunk,
            settings.batch_limits(),
            rules,
            ctx.now,
        )
        .await?;
        match outcome {
            Outcome::Applied(_) => {}
            other => return Err(ops::refuse(&mut tx, &principal, &settings, other, policy).await),
        }
    }
    let row = live(&mut tx, id).await?;
    let who = actor_name(&ctx)?;
    let mut details = json!({ "node": node, "title": title, "access": body.access });
    if !receipt_warnings.is_empty() {
        details["import_warnings"] = json!(receipt_warnings);
    }
    record(
        &ctx,
        &mut tx,
        orch_db::audit::action!(AuditAction::DocumentCreated),
        &row,
        &format!("{who} made the document \"{title}\""),
        details,
    )
    .await?;
    let listed = build::standing(&mut tx, &principal, id, policy).await?;
    let made = build::document(&mut tx, &principal, &row, &listed, ctx.now, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(made)
}

/// Reads a document without its blocks.
pub(crate) async fn get(
    ctx: Ctx,
    _query: orch_api_contract::nodes::DeletedInspectionQuery,
    (): (),
) -> ApiResult<Document> {
    let id = ctx.id("id")?;
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let row = live(&mut conn, id).await?;
    let listed = build::standing(&mut conn, &principal, id, Policy::default()).await?;
    let shown = build::document(
        &mut conn,
        &principal,
        &row,
        &listed,
        ctx.now,
        Policy::default(),
    )
    .await?;
    let cut = crate::routes::document_tree::metadata::cut(
        &mut conn,
        &principal,
        row.node_id,
        ctx.app.clock.time(),
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    finish_document_read(&ctx, &cut).await?;
    Ok(shown)
}

/// A document and every live block, or what changed since a sequence, as a
/// reader holding the document open is brought up to date.
async fn read_sync(
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    since: Option<i64>,
    now: DateTime<Utc>,
) -> ApiResult<DocumentSync> {
    let policy = Policy::default();
    let row = live(conn, id).await?;
    let listed = build::standing(conn, principal, id, policy).await?;
    let document = build::document(conn, principal, &row, &listed, now, policy).await?;
    let seq = row.doc_seq;
    let (blocks, found, complete) = match since {
        None => (
            rows::live_blocks(conn, id).await?,
            discussions::list(conn, id, discussions::Which::All, None, 50)
                .await?
                .into_iter()
                .map(|(full, _)| full)
                .collect(),
            true,
        ),
        Some(since) => {
            let changed_discussions =
                discussions::changed_previews(conn, id, since, seq, 51).await?;
            let too_far = since < row.purged_seq
                || changed_discussions.len() > 50
                || rows::count_blocks_changed(conn, id, since).await? > DOCUMENT_SYNC_MAX_CHANGES;
            if too_far {
                (Vec::new(), Vec::new(), false)
            } else {
                (
                    rows::blocks_changed(conn, id, since, seq, DOCUMENT_SYNC_MAX_CHANGES).await?,
                    changed_discussions,
                    true,
                )
            }
        }
    };
    Ok(DocumentSync {
        document,
        blocks: build::blocks_for(conn, principal, &blocks, policy).await?,
        discussions: build::discussions_for(conn, principal, id, &found, policy).await?,
        seq,
        since,
        complete,
    })
}

/// Reads a document and every live block, or what changed since a sequence.
async fn sync(ctx: Ctx, query: DocumentSyncGetQuery, (): ()) -> ApiResult<DocumentSync> {
    let id = ctx.id("id")?;
    if query.since.is_some_and(|since| since < 0) {
        return Err(invalid("since", "must not be negative"));
    }
    let mut conn = orch_db::queries::feeds::begin_reading(&ctx.app.db).await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let shown = read_sync(&mut conn, &principal, id, query.since, ctx.now).await?;
    let cut = crate::routes::document_tree::metadata::cut(
        &mut conn,
        &principal,
        shown.document.base.node.id,
        ctx.app.clock.time(),
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    finish_document_read(&ctx, &cut).await?;
    Ok(shown)
}

async fn finish_document_read(
    ctx: &Ctx,
    cut: &crate::routes::document_tree::metadata::Cut,
) -> ApiResult<()> {
    let (mut tx, guard) = transaction_guard::begin(ctx).await?;
    crate::routes::document_tree::metadata::unchanged(&mut tx, guard.principal(), cut, guard.now())
        .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

// ---------------------------------------------------------------- the document as a thing

/// Renames a document, changes its description, or both. A description that a person writes is theirs from then
/// on: automation leaves it alone.
async fn update(mut ctx: Ctx, (): (), body: DocumentUpdateBody) -> ApiResult<Document> {
    let id = ctx.id("id")?;
    if body.title.is_none() && body.description.is_none() {
        return Err(invalid("title", "give a title, a description, or both"));
    }
    let title = body
        .title
        .as_deref()
        .map(|title| clean_title("title", title))
        .transpose()?;
    let description = body
        .description
        .as_deref()
        .map(|text| crate::routes::nodes::clean_description("description", text))
        .transpose()?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let before = live(&mut tx, id).await?;
    if let Some(description) = description
        && (description != before.description || before.description_source != "user")
    {
        Box::pin(rows::describe(
            &mut tx,
            id,
            &description,
            &Writer::User(principal.user()),
            ctx.now,
        ))
        .await?
        .ok_or_else(ApiError::not_found)?;
    }
    if let Some(title) = title
        && before.title != title
    {
        let request = document_tree_domain::Request {
            command: document_tree_domain::Command::Rename,
            items: vec![document_tree_domain::Item::Document(id)],
            to: None,
            name: Some(title),
            request_ref: orch_types::new_id().to_string(),
            version: None,
        };
        let renamed = document_tree_domain::execute_in(
            &mut tx,
            document_tree_domain::Actor::Person(&principal),
            &request,
            None,
            Time::new(ctx.now),
            crate::routes::document_tree::options(&ctx),
        )
        .await;
        crate::routes::document_tree::result(&mut tx, renamed).await?;
    }
    let row = live(&mut tx, id).await?;
    let listed = build::standing(&mut tx, &principal, id, policy).await?;
    let changed = build::document(&mut tx, &principal, &row, &listed, ctx.now, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(changed)
}

/// Says how far the caller has had a document on screen.
async fn seen(mut ctx: Ctx, (): (), body: DocumentSeenBody) -> ApiResult<()> {
    let id = ctx.id("id")?;
    if body.seq < 0 {
        return Err(invalid("seq", "must not be negative"));
    }
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let row = live(&mut conn, id).await?;
    // Nobody has had on screen what does not exist yet.
    let seq = body.seq.min(row.doc_seq);
    rows::mark_seen(&mut conn, principal.user(), id, seq, ctx.now).await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Deletes a document, which can be restored until it is purged.
async fn delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    rows::containers::lock_document_scope(&mut tx, id).await?;
    live(&mut tx, id).await?;
    // The existing single-document DELETE keeps its request shape; its confirmation is
    // the ordinary document action. It uses the same complete producer, never a folder target.
    let request = document_tree_domain::Request {
        command: document_tree_domain::Command::Delete,
        items: vec![document_tree_domain::Item::Document(id)],
        to: None,
        name: None,
        request_ref: orch_types::new_id().to_string(),
        version: None,
    };
    let options = crate::routes::document_tree::options(&ctx);
    let plan = document_tree_domain::plan_delete_in(
        &mut tx,
        document_tree_domain::Actor::Person(&principal),
        &request,
        Time::new(ctx.now),
        options,
    )
    .await;
    let plan = crate::routes::document_tree::result(&mut tx, plan).await?;
    let deleted = document_tree_domain::execute_in(
        &mut tx,
        document_tree_domain::Actor::Person(&principal),
        &request,
        Some(&plan.stamp),
        Time::new(ctx.now),
        options,
    )
    .await;
    crate::routes::document_tree::result(&mut tx, deleted).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Restores a deleted document: whoever could have deleted it, as it was.
async fn restore(
    mut ctx: Ctx,
    (): (),
    body: orch_api_contract::documents::DocumentRestoreBody,
) -> ApiResult<Document> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let stored = rows::get(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    match orch_db::queries::trace::read_scope(
        &[id, stored.node_id],
        documents::check_all(
            &mut tx,
            &principal,
            id,
            &[Permission::DocManage],
            Act::Delete,
            State::AsItWas,
            policy,
        ),
    )
    .await
    {
        Ok(Decision::Allowed { .. }) => {}
        Ok(Decision::Refused(refusal)) => {
            return Err(refusals::denied(&mut tx, refusal).await?);
        }
        Err(error) => return Err(refusals::problem(&mut tx, error).await),
    }
    rows::containers::lock_document_scope(&mut tx, id).await?;
    let row = rows::lock(&mut tx, id)
        .await?
        .filter(|row| row.deleted_at.is_some())
        .ok_or_else(ApiError::not_found)?;
    let life = orch_db::queries::trace::life(&mut tx, "document", id).await?;
    let current = life.as_ref().and_then(|l| l.deletion_id);
    if body.expected_deletion_id.is_some() && body.expected_deletion_id != current {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the selected deletion is no longer current",
        )
        .with("current_deletion_id", current));
    }
    if orch_db::queries::hierarchy::deleted_node(&mut tx, row.node_id)
        .await?
        .is_some()
    {
        let refs =
            crate::routes::nodes::view::refs(&mut tx, &principal, &[row.node_id], policy).await?;
        return Err(ApiError::new(
            ErrorCode::NodeRestoreParentFirst,
            "bring back the document's place under its own restoration authority first",
        )
        .with("parent", refs.get(&row.node_id)));
    }
    if let Some(parent) = rows::containers::ancestors_many(&mut tx, &[id])
        .await?
        .into_iter()
        .find(|parent| parent.deleted_at.is_some())
    {
        let refs =
            crate::routes::document_tree::container_refs(&mut tx, &principal, &[parent.id]).await?;
        return Err(ApiError::new(
            ErrorCode::NodeRestoreParentFirst,
            "bring back the document's containing folder first",
        )
        .with("parent", refs.get(&parent.id)));
    }
    let request = document_tree_domain::Request {
        command: document_tree_domain::Command::Restore,
        items: vec![document_tree_domain::Item::Document(id)],
        to: None,
        name: None,
        request_ref: orch_types::new_id().to_string(),
        version: None,
    };
    let restored = document_tree_domain::execute_in(
        &mut tx,
        document_tree_domain::Actor::Person(&principal),
        &request,
        None,
        Time::new(ctx.now),
        crate::routes::document_tree::options(&ctx),
    )
    .await;
    crate::routes::document_tree::result(&mut tx, restored).await?;
    let restored = live(&mut tx, id).await?;
    let listed = build::standing(&mut tx, &principal, id, policy).await?;
    let shown = build::document(&mut tx, &principal, &restored, &listed, ctx.now, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

// ---------------------------------------------------------------- moving

/// What a move needs held, and what it needs of the place: Manage of the
/// document, what makes a document where it goes, documents being on there, and
/// the node being within the tree the document was made in.
async fn may_move(
    conn: &mut Conn,
    principal: &Principal,
    ctx_policy: Policy,
    document: &DocumentRow,
    to: Id,
) -> ApiResult<Option<Id>> {
    authorization::require_document(
        conn,
        principal,
        document.id,
        Permission::DocManage,
        ctx_policy,
    )
    .await?;
    let (target, _) = placed(conn, to).await?;
    authorization::require(
        conn,
        principal,
        to,
        exact(Permission::DocCreate),
        Act::Use,
        ctx_policy,
    )
    .await?;
    let settings = DocSettings::at(conn, to).await?;
    let destination = Destination {
        root: target.org,
        deleted: false,
        docs_enabled: settings.enabled,
    };
    match placement::check_move(document.node_id, document.org_id, to, &destination) {
        Ok(()) => Ok(Some(target.org)),
        Err(MoveRefusal::Already) => {
            let entry = rows::containers::entry(conn, "document", document.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            Ok(entry.parent_container_id.map(|_| target.org))
        }
        Err(MoveRefusal::Gone) => Err(ApiError::not_found()),
        Err(MoveRefusal::OtherTree) => Err(ApiError::new(
            ErrorCode::NodeWrongParent,
            "a document stays within the tree of orgs it was made in",
        )),
        Err(MoveRefusal::Disabled) => {
            Err(limits::disabled(conn, principal, &settings, ctx_policy).await)
        }
    }
}

/// Says who gains and who loses what when a document's rows follow those of
/// another node.
async fn move_preview(mut ctx: Ctx, (): (), body: DocumentMoveBody) -> ApiResult<PagePreview> {
    let id = ctx.id("id")?;
    let (mut conn, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let row = live(&mut conn, id).await?;
    let target = may_move(&mut conn, &principal, policy, &row, body.node_id).await?;
    if target.is_none() {
        return Ok(PagePreview {
            pages: 1,
            orgs: None,
            gaining: Vec::new(),
            losing: Vec::new(),
            more: 0,
        });
    }
    let plan = documents::plan_move(&mut conn, &principal, id, body.node_id, policy)
        .await
        .map_err(ApiError::internal)?;
    let people: Vec<Id> = plan
        .gaining
        .keys()
        .chain(plan.losing.keys())
        .copied()
        .collect();
    let known = refusals::user_refs(&mut conn, &people).await?;
    let shown = |side: &BTreeMap<Id, Vec<Permission>>| -> (Vec<PreviewPerson>, i64) {
        let most = usize::try_from(PREVIEW_MAX_PEOPLE).unwrap_or(usize::MAX);
        let mut listed = Vec::new();
        let mut more = 0_i64;
        for (person, permissions) in side {
            match known.get(person) {
                Some(user) if listed.len() < most => listed.push(PreviewPerson {
                    user: user.clone(),
                    permissions: permissions
                        .iter()
                        .map(|permission| permission.as_str().to_owned())
                        .collect(),
                    pages: 1,
                }),
                _ => more += 1,
            }
        }
        (listed, more)
    };
    let (gaining, more_gaining) = shown(&plan.gaining);
    let (losing, more_losing) = shown(&plan.losing);
    Ok(PagePreview {
        pages: 1,
        orgs: None,
        gaining,
        losing,
        more: more_gaining + more_losing,
    })
}

/// Moves a document beside another node of its tree.
async fn move_to(mut ctx: Ctx, (): (), body: DocumentMoveBody) -> ApiResult<Document> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.principal().clone();
    let policy = Policy::default();
    let row = live(&mut tx, id).await?;
    if may_move(&mut tx, &principal, policy, &row, body.node_id)
        .await?
        .is_some()
    {
        let request = document_tree_domain::Request {
            command: document_tree_domain::Command::Move,
            items: vec![document_tree_domain::Item::Document(id)],
            to: Some(document_tree_domain::Directory::NodeDocs(body.node_id)),
            name: None,
            request_ref: orch_types::new_id().to_string(),
            version: None,
        };
        let moved = document_tree_domain::execute_in(
            &mut tx,
            document_tree_domain::Actor::Person(&principal),
            &request,
            None,
            Time::new(ctx.now),
            crate::routes::document_tree::options(&ctx),
        )
        .await;
        crate::routes::document_tree::result(&mut tx, moved).await?;
    }
    let moved = live(&mut tx, id).await?;
    let listed = build::standing(&mut tx, &principal, id, policy).await?;
    let shown = build::document(&mut tx, &principal, &moved, &listed, ctx.now, policy).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

// ---------------------------------------------------------------- files

/// Takes a file out of a document, unless a live block names it.
async fn file_delete(mut ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let file = ctx.id("file_id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let row = live_locked(&mut tx, id).await?;
    let held = rows::file(&mut tx, id, file)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if !rows::files_in_use(&mut tx, id, &[file]).await?.is_empty() {
        return Err(ApiError::new(
            ErrorCode::ResourceInUse,
            "a block of the document still names this file",
        ));
    }
    rows::remove_file(&mut tx, id, file).await?;
    // The document no longer holds the bytes: the pin it took when the file was put in is let go.
    ctx.app.blobs.unpin(&mut tx, &[held.blob], ctx.now).await?;
    let settings = DocSettings::at(&mut tx, row.node_id).await?;
    rows::history::record_file(
        &mut tx,
        id,
        &Writer::User(guard.principal().user()),
        DocumentFileChange {
            name: held.file.name.clone(),
            kind: DocumentFileChangeKind::Removed,
        },
        settings.version_rules(ctx.app.config.documents.version_window_secs),
        ctx.now,
    )
    .await?;
    rows::touch(&mut tx, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
