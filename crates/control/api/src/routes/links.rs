//! A document's Links section: which tabs it shows, what refers to it, what it refers to, and the graph around it.
//!
//! Each answer is read in one snapshot of the database and checked for the reader inside it: a source, a target or a
//! node the reader may not read is never named, counted, drawn or left in a cursor. Cursors are sealed under the
//! deployment key and bound to the reader and the list, so a position that fell on a hidden source tells its holder
//! nothing. The tabs a document shows are display only; nothing here reads them to decide what to answer.

mod graph;
mod placed;
mod references;

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard;
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::Permission;
use orch_api_contract::common::{ErrorCode, MAX_LIMIT};
use orch_api_contract::links::{
    DocumentGraphGet, DocumentLinks, DocumentLinksGet, DocumentLinksSet, DocumentLinksSetBody,
    DocumentReferencesList, LinkDirection, LinksTab,
};
use orch_authz::{Policy, Principal};
use orch_db::audit::{self, Actor, Entry};
use orch_db::queries::documents::{self as document_rows, DocumentRow};
use orch_db::queries::links as rows;
use orch_db::{Conn, DbError};
use orch_settings::Place;
use orch_settings_contract::registry::keys;
use orch_types::{Hash, Id};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// What a sealed cursor of a Links list is for, so that no other sealed value opens as one.
const CURSOR_PURPOSE: &[u8] = b"links/list/v1";

/// The tabs in the order the section shows them.
const TAB_ORDER: [LinksTab; 4] = [
    LinksTab::ReferencedBy,
    LinksTab::References,
    LinksTab::Related,
    LinksTab::Graph,
];

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(DocumentLinksGet, tabs_get)
        .on(DocumentLinksSet, tabs_set)
        .on(DocumentReferencesList, references::list)
        .on(DocumentGraphGet, graph::get);
}

/// The word a tab is stored and written with.
fn word(tab: LinksTab) -> &'static str {
    match tab {
        LinksTab::ReferencedBy => "referenced_by",
        LinksTab::References => "references",
        LinksTab::Related => "related",
        LinksTab::Graph => "graph",
    }
}

/// The tabs a list of stored words names, once each, in the section's order; a word that names none is left out.
fn tabs_of(words: &[String]) -> Vec<LinksTab> {
    TAB_ORDER
        .into_iter()
        .filter(|tab| words.iter().any(|stored| stored == word(*tab)))
        .collect()
}

/// The document `id` that is there: not deleted.
async fn live_document(conn: &mut Conn, id: Id) -> ApiResult<DocumentRow> {
    document_rows::get(conn, id)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)
}

/// The numbers of the Links settings at a document's place.
#[derive(Debug, Clone, Copy)]
struct Limits {
    page_size: usize,
    occurrences: usize,
    mentions_counted: usize,
    line_chars: usize,
    access_checks: usize,
    graph_nodes: usize,
}

impl Limits {
    async fn read(conn: &mut Conn, node: Id) -> ApiResult<Self> {
        let number = |key: &'static str, value: Option<i64>| {
            value
                .and_then(|value| usize::try_from(value).ok())
                .filter(|value| *value > 0)
                .ok_or_else(|| ApiError::internal(format!("the setting {key} is not a count")))
        };
        let place = Place::Node(node);
        Ok(Self {
            page_size: number(
                keys::DOCUMENTS_LINKS_PAGE_SIZE,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_PAGE_SIZE)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
            occurrences: number(
                keys::DOCUMENTS_LINKS_OCCURRENCES,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_OCCURRENCES)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
            mentions_counted: number(
                keys::DOCUMENTS_LINKS_MENTIONS_COUNTED,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_MENTIONS_COUNTED)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
            line_chars: number(
                keys::DOCUMENTS_LINKS_LINE_CHARS,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_LINE_CHARS)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
            access_checks: number(
                keys::DOCUMENTS_LINKS_ACCESS_CHECKS,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_ACCESS_CHECKS)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
            graph_nodes: number(
                keys::DOCUMENTS_LINKS_GRAPH_NODES,
                orch_settings::get(conn, place, keys::DOCUMENTS_LINKS_GRAPH_NODES)
                    .await
                    .map_err(ApiError::internal)?,
            )?,
        })
    }

    /// The size of a page: what was asked for, within the API's limits, or the place's page size.
    fn page(&self, asked: Option<u32>) -> usize {
        asked.map_or(self.page_size, |asked| {
            usize::try_from(asked.clamp(1, MAX_LIMIT)).unwrap_or(self.page_size)
        })
    }
}

/// Which tabs a document shows, from its place's setting and its own choice.
async fn tabs(
    conn: &mut Conn,
    principal: &Principal,
    document: &DocumentRow,
) -> ApiResult<DocumentLinks> {
    let view = orch_settings::resolve(
        conn,
        Place::Node(document.node_id),
        keys::DOCUMENTS_LINKS_SHOWN,
    )
    .await
    .map_err(ApiError::internal)?;
    let stored: Vec<String> = view
        .resolved
        .value
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)?
        .unwrap_or_default();
    let inherited = tabs_of(&stored);
    // A lock anywhere on the way down decides the setting for the whole place, and a document lies in it.
    let locked = view.resolved.locked_by.is_some();
    let own = rows::own_tabs(conn, document.id)
        .await?
        .map(|words| tabs_of(&words));
    let shown = if locked {
        inherited.clone()
    } else {
        own.clone().unwrap_or_else(|| inherited.clone())
    };
    let manages = orch_authz::documents::listed(conn, principal, &[document.id], Policy::default())
        .await
        .map_err(ApiError::internal)?
        .iter()
        .any(|held| held.holds(Permission::DocManage));
    Ok(DocumentLinks {
        shown,
        own,
        inherited,
        locked,
        writable: manages && !locked,
    })
}

async fn tabs_get(ctx: Ctx, (): (), (): ()) -> ApiResult<DocumentLinks> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let document = live_document(&mut conn, ctx.id("id")?).await?;
    tabs(&mut conn, &principal, &document).await
}

async fn tabs_set(ctx: Ctx, (): (), body: DocumentLinksSetBody) -> ApiResult<DocumentLinks> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let document = document_rows::lock(&mut tx, ctx.id("id")?)
        .await?
        .filter(|row| row.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let current = tabs(&mut tx, guard.principal(), &document).await?;
    if current.locked {
        return Err(ApiError::new(
            ErrorCode::AccessLocked,
            "a lock above decides which tabs documents here show",
        ));
    }
    let words: Option<Vec<String>> = body
        .shown
        .as_ref()
        .map(|shown| {
            tabs_of(
                &shown
                    .iter()
                    .map(|tab| word(*tab).to_owned())
                    .collect::<Vec<_>>(),
            )
        })
        .map(|shown| shown.into_iter().map(|tab| word(tab).to_owned()).collect());
    rows::set_own_tabs(
        &mut tx,
        document.id,
        words.as_deref(),
        guard.principal().user(),
        guard.now().get(),
    )
    .await?;
    let answer = tabs(&mut tx, guard.principal(), &document).await?;
    if current.own != answer.own {
        let mut entry = Entry::new(
            Actor::User(guard.principal().user()),
            orch_db::audit::action!(AuditAction::DocumentLinksTabsChanged).as_str(),
            "document",
            Some(document.id),
            "Changed the tabs of the document's Links section.",
        );
        entry.history = true;
        entry.node_id = Some(document.node_id);
        entry.org_id = Some(document.org_id);
        entry.details = json!({"before":current.own,"after":answer.own,"shown":answer.shown});
        audit::append(
            &mut tx,
            &entry,
            &crate::sign_in::origin(&ctx),
            guard.now().get(),
        )
        .await?;
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(answer)
}

/// Where a sealed cursor of a list goes on.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "list", rename_all = "snake_case")]
enum Position {
    /// Sources: everything with a smaller identity.
    Sources { below: Id },
    /// The mentions of one source: those after this part.
    Mentions { part: Id },
    /// Targets: those after this kind, identity and path digest in stable index order.
    TargetKeys {
        id: Id,
        kind: String,
        /// The digest of the path of a workspace file, as text; empty for the rest.
        key: String,
    },
}

/// What a cursor is bound to: its reader, its document, its direction and the source it is about.
fn audience(
    principal: &Principal,
    document: Id,
    direction: LinkDirection,
    of: Option<&str>,
) -> Vec<u8> {
    let mut bound = CURSOR_PURPOSE.to_vec();
    bound.extend_from_slice(principal.user().as_bytes());
    bound.extend_from_slice(document.as_bytes());
    bound.push(match direction {
        LinkDirection::In => 1,
        LinkDirection::Out => 2,
    });
    if let Some(of) = of {
        bound.extend_from_slice(of.as_bytes());
    }
    bound
}

fn seal(
    principal: &Principal,
    document: Id,
    direction: LinkDirection,
    of: Option<&str>,
    position: &Position,
) -> ApiResult<String> {
    let plain = serde_json::to_vec(position).map_err(ApiError::internal)?;
    let sealed = crate::routes::resources::sealing::master_key()?
        .seal(&plain, &audience(principal, document, direction, of))
        .map_err(ApiError::internal)?;
    Ok(URL_SAFE_NO_PAD.encode(sealed))
}

fn open(
    principal: &Principal,
    document: Id,
    direction: LinkDirection,
    of: Option<&str>,
    text: &str,
) -> ApiResult<Position> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
        return Err(bad());
    }
    let sealed = URL_SAFE_NO_PAD.decode(text).map_err(|_| bad())?;
    let plain = crate::routes::resources::sealing::master_key()?
        .open(&sealed, &audience(principal, document, direction, of))
        .map_err(|_| bad())?;
    serde_json::from_slice(&plain).map_err(|_| bad())
}

/// Where a Related page goes on, as the next request sends it back: the last item delivered, which the reader holds.
#[derive(Debug, Serialize, Deserialize)]
struct RelatedCursor {
    score_bits: u64,
    changed_micros: i64,
    id: Id,
    fingerprint: Hash,
}

/// The cursor that continues a Related list after `position`.
pub(crate) fn related_cursor(
    position: orch_engine::links::RelatedPosition,
    fingerprint: Hash,
) -> ApiResult<String> {
    let plain = serde_json::to_vec(&RelatedCursor {
        score_bits: position.score.to_bits(),
        changed_micros: position.changed_at.timestamp_micros(),
        id: position.id,
        fingerprint,
    })
    .map_err(ApiError::internal)?;
    Ok(URL_SAFE_NO_PAD.encode(plain))
}

/// The position a Related cursor continues after.
pub(crate) fn related_after(text: &str) -> ApiResult<(orch_engine::links::RelatedPosition, Hash)> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    if text.len() > orch_realtime_contract::version::MAX_CLIENT_FRAME_BYTES {
        return Err(bad());
    }
    let plain = URL_SAFE_NO_PAD.decode(text).map_err(|_| bad())?;
    let cursor: RelatedCursor = serde_json::from_slice(&plain).map_err(|_| bad())?;
    Ok((
        orch_engine::links::RelatedPosition {
            score: f64::from_bits(cursor.score_bits),
            changed_at: chrono::DateTime::from_timestamp_micros(cursor.changed_micros)
                .ok_or_else(bad)?,
            id: cursor.id,
        },
        cursor.fingerprint,
    ))
}
