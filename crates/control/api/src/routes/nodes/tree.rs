//! The nodes of a whole tree for a view of it, what somebody let the caller into by name, and the topics of a project with the filters of a list.
//!
//! A tree view is every node of a root org that the caller may view, or the
//! ones that changed since a sequence they hold. The engine says which they
//! may view for all of them in the statements it takes for one. A node
//! somebody named the caller on, whose parent they cannot view, is listed
//! apart, so that it can be found without the tree above it. Document-only
//! folders use their own current Read and never become hierarchy nodes. The topics of
//! a project are filtered, ordered and paged in the database, and the ones
//! the caller may not view are dropped afterwards, a batch at a time, until
//! the page is full.

use super::{
    NodeKind, Placed, actor_name, invalid, placed, record, shown, threads_are_not_served_here, view,
};
use crate::middleware::authorization::{self, refusals, refusals::node_kind};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_access_contract::audit::AuditAction;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::common::{ErrorCode, HiddenItem, Listed, Page, PageQuery};
use orch_api_contract::nodes::{
    ArchivedFilter, AttentionFilter, Node, NodeTagSet, NodeTagSetBody, OrgTreeGet, OrgTreeGetQuery,
    OrgTreeGetReply, SharedList, SharedListItem, SharedListReply, TREE_SYNC_MAX_CHANGES, Topic,
    TopicList, TopicListQuery, TopicSort, TopicStatus, TopicStatusFilter, TopicUpdate,
    TopicUpdateBody, TreeChat, TreeNode, TreeThread, TreeTopic, TreeWaiting,
};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::users::UserRef;
use orch_authz::nodes as access;
use orch_authz::{Act, Policy};
use orch_db::queries::hierarchy::{self, ArchivedMode, TopicCursor, TopicOrder, TopicQuery};
use orch_types::Id;
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(OrgTreeGet, org_tree)
        .on(SharedList, shared)
        .on(TopicList, topic_list)
        .on(TopicUpdate, topic_update)
        .on(NodeTagSet, tag_set);
}

fn word<T: DeserializeOwned>(text: &str) -> Option<T> {
    serde_json::from_value(serde_json::Value::String(text.to_owned())).ok()
}

/// The nodes of an org's tree that the caller may view, whole or since a
/// sequence.
async fn org_tree(ctx: Ctx, query: OrgTreeGetQuery, (): ()) -> ApiResult<OrgTreeGetReply> {
    let root = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let (place, kind) = placed(&mut conn, root).await?;
    let is_root = hierarchy::org(&mut conn, root)
        .await?
        .is_some_and(|org| org.parent_id.is_none());
    if kind != NodeKind::Org || !is_root || place.org != root {
        return Err(ApiError::not_found());
    }
    if query.since.is_some() && (query.cursor.is_some() || query.limit.is_some()) {
        return Err(invalid(
            "since",
            "cannot be combined with a full-load cursor or limit",
        ));
    }
    let limit = query.limit.map_or(TREE_SYNC_MAX_CHANGES, i64::from);
    if !(1..=TREE_SYNC_MAX_CHANGES).contains(&limit) {
        return Err(invalid("limit", "must be between 1 and 5000"));
    }
    let (seq, after) = if let Some(cursor) = query.cursor.as_deref() {
        let bad = || invalid("cursor", "is not a cursor of this tree");
        let parts: Vec<&str> = cursor.split('|').collect();
        if parts.len() != 4
            || parts[0] != root.to_string()
            || parts[1] != principal.user().to_string()
        {
            return Err(bad());
        }
        let seq: i64 = parts[2].parse().map_err(|_| bad())?;
        if seq < 0 {
            return Err(bad());
        }
        (seq, Some(parts[3].parse::<Id>().map_err(|_| bad())?))
    } else {
        (hierarchy::tree_seq(&mut conn, root).await?, None)
    };
    let mut rows = if query.since.is_some() {
        hierarchy::tree(&mut conn, root, query.since, TREE_SYNC_MAX_CHANGES + 1).await?
    } else {
        hierarchy::tree_page(&mut conn, root, after, limit + 1).await?
    };
    if query.since.is_some()
        && i64::try_from(rows.len()).unwrap_or(i64::MAX) > TREE_SYNC_MAX_CHANGES
    {
        return Ok(OrgTreeGetReply {
            settings: None,
            seq,
            since: query.since,
            complete: false,
            next_cursor: None,
            nodes: Vec::new(),
        });
    }
    // Advance by scanned ids, including inaccessible rows. An empty visible
    // page can therefore never strand the caller before later readable rows.
    let next_cursor =
        if query.since.is_none() && i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit {
            rows.truncate(usize::try_from(limit).unwrap_or(0));
            rows.last()
                .map(|row| format!("{root}|{}|{seq}|{}", principal.user(), row.id))
        } else {
            None
        };
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let readable_docs = super::tree_docs::readable(&mut conn, &principal, &ids)
        .await
        .map_err(ApiError::internal)?;
    let listed: HashMap<Id, access::Listed> = access::holding(
        &mut conn,
        &principal,
        &ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|each| (each.id, each))
    .collect();
    // A private node the caller administers and may not view is its minimal row.
    let unseen = unviewed(&rows, &listed);
    let managed = if unseen.is_empty() {
        Vec::new()
    } else {
        access::holding(
            &mut conn,
            &principal,
            &unseen,
            PermissionName::Exact(Permission::AccessManage),
            Act::Use,
            Policy::default(),
        )
        .await
        .map_err(ApiError::internal)?
    };
    let people = refusals::user_refs(&mut conn, &owners_of(&rows, &managed)).await?;
    let mut hidden = hidden_rows(&rows, &managed, &people);
    let admin_ids: Vec<Id> = listed
        .values()
        .filter_map(|each| each.admin_access.and_then(|reach| reach.place.node()))
        .collect();
    let admin_refs =
        view::qualified_refs(&mut conn, &principal, &admin_ids, Policy::default()).await?;
    let waiting_threads: Vec<Id> = rows
        .iter()
        .filter(|row| {
            row.kind == "thread"
                && !row.deleted
                && !row.archived
                && row.pending_approvals > 0
                && listed.contains_key(&row.id)
        })
        .map(|row| row.id)
        .collect();
    let waiting = super::tree_work::waiting(
        &mut conn,
        &principal,
        &waiting_threads,
        ctx.now,
        &ctx.app.engine,
    )
    .await?;
    let display_ids: Vec<Id> = rows
        .iter()
        .filter(|row| row.kind == "thread" && !row.deleted && listed.contains_key(&row.id))
        .map(|row| row.id)
        .collect();
    let displays = orch_engine::views::thread_displays(
        &mut conn,
        &display_ids,
        Some(principal.user()),
        principal.now(),
    )
    .await
    .map_err(ApiError::internal)?;
    let nodes = rows
        .into_iter()
        .filter_map(|row| {
            let kind = node_kind(&row.kind)?;
            if let Some(minimal) = hidden.remove(&row.id) {
                return Some(Listed::hidden(minimal));
            }
            let Some(each) = listed.get(&row.id).filter(|_| !row.deleted) else {
                // Changed since the caller last looked, and not theirs to see
                // now: the view drops it, and learns no more than that.
                return query.since.map(|_| Listed::visible(gone(&row, kind)));
            };
            Some(Listed::visible(TreeNode {
                activity_at: Some(orch_types::Time::new(row.activity_at)),
                admin_access: each
                    .admin_access
                    .and_then(|place| view::admin_access(place, &admin_refs)),
                participating: each.participating,
                address: row.address.clone(),
                id: row.id,
                kind,
                parent_id: row.parent_id,
                name: each.name.clone(),
                color: row.color.and_then(|color| u32::try_from(color).ok()),
                custom_color: row.custom_color.clone(),
                docs: if readable_docs.contains(&row.id) {
                    row.docs
                } else {
                    0
                },
                archived: each.archived,
                restricted: each.restricted,
                deleted: false,
                tree_seq: row.tree_seq,
                topic: (kind == NodeKind::Topic).then(|| TreeTopic {
                    status: if row.topic_status.as_deref() == Some("done") {
                        TopicStatus::Done
                    } else {
                        TopicStatus::Open
                    },
                    tag_ids: row.tag_ids.clone(),
                }),
                thread: (kind == NodeKind::Thread)
                    .then(|| thread_of(&row, waiting.get(&row.id), displays.get(&row.id)))
                    .flatten(),
                chat: TreeChat {
                    id: row.chat_id.unwrap_or_else(Id::nil),
                    seq: row.chat_seq,
                },
            }))
        })
        .collect();
    // Only a full opening needs the order/defaults; incremental tree repairs keep their native cut.
    drop(conn);
    let settings = if query.since.is_none() && query.cursor.is_none() {
        let mut child = ctx;
        child.route = orch_api_contract::settings::NodeSettingList::ROUTE;
        let read = async {
            authorization::require_on_path(&child, "node.view").await?;
            crate::routes::settings::setting_list(child, (), ()).await
        }
        .await;
        match read {
            Ok(settings) => Some(settings),
            Err(error) if error.problem.status == 403 => None,
            Err(error) => return Err(error),
        }
    } else {
        None
    };
    Ok(OrgTreeGetReply {
        settings,
        seq,
        since: query.since,
        complete: next_cursor.is_none(),
        next_cursor,
        nodes,
    })
}

/// The live nodes of `rows` that the caller may not view: those an admin of a place above may still
/// be shown as minimal rows.
pub(crate) fn unviewed(
    rows: &[hierarchy::TreeRow],
    viewable: &HashMap<Id, access::Listed>,
) -> Vec<Id> {
    rows.iter()
        .filter(|row| !row.deleted && !viewable.contains_key(&row.id))
        .map(|row| row.id)
        .collect()
}

/// The first owners of the nodes the caller administers among `rows`, to be named on their rows.
pub(crate) fn owners_of(rows: &[hierarchy::TreeRow], managed: &[access::Listed]) -> Vec<Id> {
    let mut owners: Vec<Id> = rows
        .iter()
        .filter(|row| managed.iter().any(|each| each.id == row.id))
        .map(|row| row.owner_id)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    owners
}

/// The minimal rows of the nodes the caller administers and may not view, by id: what places each in
/// the tree (its parent, kind and name), its first owner, and that it is private; nothing of its
/// content, work, counts, tags or activity.
pub(crate) fn hidden_rows(
    rows: &[hierarchy::TreeRow],
    managed: &[access::Listed],
    people: &HashMap<Id, UserRef>,
) -> HashMap<Id, HiddenItem> {
    managed
        .iter()
        .filter_map(|each| {
            let row = rows.iter().find(|row| row.id == each.id)?;
            Some((
                row.id,
                HiddenItem {
                    id: row.id,
                    parent_id: row.parent_id,
                    kind: row.kind.clone(),
                    name: each.name.clone(),
                    owner: people.get(&row.owner_id).cloned(),
                    content_hidden: true,
                    private: true,
                },
            ))
        })
        .collect()
}

/// What a tree view says of a thread, from the columns that hold it.
pub(crate) fn thread_of(
    row: &hierarchy::TreeRow,
    waiting: Option<&TreeWaiting>,
    display: Option<&orch_api_contract::threads::ThreadDisplay>,
) -> Option<TreeThread> {
    Some(TreeThread {
        display: Box::new(display.cloned().unwrap_or_default()),
        attention: row
            .attention
            .clone()
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        state: word(row.state.as_deref()?)?,
        blocked_reason: row.blocked_reason.as_deref().and_then(word),
        blocked_cause: super::tree_work::blocked_cause(
            row.state.as_deref(),
            row.blocked_reason.as_deref(),
            row.blocked_detail.as_deref(),
        ),
        waiting: waiting.copied(),
        pending_approvals: row.pending_approvals,
        activity_seq: row.activity_seq.unwrap_or_default(),
        last_activity_at: row.last_activity_at.map(orch_types::Time::new),
    })
}

/// A node of a tree that changed and that the caller is not shown now: it
/// is marked deleted, with nothing else of it.
fn gone(row: &hierarchy::TreeRow, kind: NodeKind) -> TreeNode {
    TreeNode {
        activity_at: None,
        admin_access: None,
        participating: false,
        address: None,
        id: row.id,
        kind,
        parent_id: row.parent_id,
        name: String::new(),
        color: None,
        custom_color: None,
        docs: 0,
        archived: false,
        restricted: false,
        deleted: true,
        tree_seq: row.tree_seq,
        topic: None,
        thread: None,
        chat: TreeChat {
            id: row.chat_id.unwrap_or_else(Id::nil),
            seq: 0,
        },
    }
}

/// Stable descending tie order: hierarchy node, document folder, text document.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
enum SharedKind {
    Document,
    Folder,
    Node,
}

impl SharedKind {
    fn name(self) -> &'static str {
        match self {
            Self::Node => "node",
            Self::Folder => "document_container",
            Self::Document => "document",
        }
    }
}

/// One scanned identity in the merged creation-time order; no hidden name is retained here.
#[derive(Clone, Copy)]
struct SharedCandidate {
    id: Id,
    at: chrono::DateTime<chrono::Utc>,
    kind: SharedKind,
}

fn shared_cursor(text: &str, user: Id) -> ApiResult<SharedCandidate> {
    let bad = || invalid("cursor", "is not a cursor of your shared places");
    let parts: Vec<&str> = text.split('|').collect();
    if !matches!(parts.len(), 3 | 4) || parts[0] != user.to_string() {
        return Err(bad());
    }
    let at = parts[1]
        .parse::<i64>()
        .ok()
        .and_then(chrono::DateTime::from_timestamp_micros)
        .ok_or_else(bad)?;
    // Existing node-only cursors remain valid. New cursors distinguish exact cross-kind ties.
    let (kind, id) = if parts.len() == 3 {
        (SharedKind::Node, parts[2])
    } else {
        (
            match parts[2] {
                "node" => SharedKind::Node,
                "document_container" => SharedKind::Folder,
                "document" => SharedKind::Document,
                _ => return Err(bad()),
            },
            parts[3],
        )
    };
    Ok(SharedCandidate {
        id: id.parse().map_err(|_| bad())?,
        at,
        kind,
    })
}

async fn shared(ctx: Ctx, query: PageQuery, (): ()) -> ApiResult<SharedListReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let before = query
        .cursor
        .as_deref()
        .map(|cursor| shared_cursor(cursor, principal.user()))
        .transpose()?;
    let limit = query.limit();
    let boundary = before.map(|last| (last.at, last.id));
    let mut candidates: Vec<_> = hierarchy::named_for(
        &mut conn,
        principal.user(),
        principal.groups(),
        principal.member_of(),
        ctx.now,
        boundary,
        i64::from(limit) + 1,
    )
    .await?
    .into_iter()
    .map(|(id, at)| SharedCandidate {
        id,
        at,
        kind: SharedKind::Node,
    })
    .collect();
    let holders =
        orch_authz::holders::held(&mut conn, principal.user(), orch_types::Time::new(ctx.now))
            .await
            .map_err(ApiError::internal)?;
    candidates.extend(
        orch_db::queries::shared::documents(
            &mut conn,
            &orch_db::queries::shared::DocumentQuery {
                user: principal.user(),
                held: &holders.held,
                at: ctx.now,
                before: boundary,
                after_kind: before.map_or("node", |last| last.kind.name()),
                limit: i64::from(limit) + 1,
            },
        )
        .await?
        .into_iter()
        .map(|entry| SharedCandidate {
            id: entry.id,
            at: entry.created_at,
            kind: if entry.kind == "document" {
                SharedKind::Document
            } else {
                SharedKind::Folder
            },
        }),
    );
    candidates.sort_by(|one, other| {
        other
            .at
            .cmp(&one.at)
            .then(other.id.cmp(&one.id))
            .then(other.kind.cmp(&one.kind))
    });
    let partial = candidates.len() > usize::try_from(limit).unwrap_or(0);
    candidates.truncate(usize::try_from(limit).unwrap_or(0));
    let next_cursor = if partial {
        candidates.last().map(|last| {
            format!(
                "{}|{}|{}|{}",
                principal.user(),
                last.at.timestamp_micros(),
                last.kind.name(),
                last.id
            )
        })
    } else {
        None
    };
    let ids: Vec<Id> = candidates
        .iter()
        .filter(|candidate| candidate.kind == SharedKind::Node)
        .map(|candidate| candidate.id)
        .collect();
    let nodes = view::visible(&mut conn, &principal, &ids, ctx.now, Policy::default()).await?;
    let parents: Vec<Id> = nodes
        .iter()
        .filter_map(view::parent_of)
        .collect::<HashSet<_>>()
        .into_iter()
        .collect();
    let seen_parents: HashSet<Id> = access::holding(
        &mut conn,
        &principal,
        &parents,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|each| each.id)
    .collect();
    let mut nodes: HashMap<_, _> = nodes
        .into_iter()
        .filter(|node| view::parent_of(node).is_none_or(|parent| !seen_parents.contains(&parent)))
        .map(|node| (super::node_id(&node), SharedListItem::Node(Box::new(node))))
        .collect();
    let ids: Vec<_> = candidates
        .iter()
        .filter(|candidate| candidate.kind != SharedKind::Node)
        .map(|candidate| candidate.id)
        .collect();
    let mut documents = shared_documents(&mut conn, &principal, &ids, ctx.now).await?;
    let items = candidates
        .into_iter()
        .filter_map(|candidate| {
            if candidate.kind == SharedKind::Node {
                nodes.remove(&candidate.id)
            } else {
                documents.remove(&(candidate.kind, candidate.id))
            }
        })
        .collect();
    Ok(SharedListReply {
        partial,
        next_cursor,
        items,
    })
}

/// Project both document-domain kinds through their one native permission frame.
async fn shared_documents(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    ids: &[Id],
    now: chrono::DateTime<chrono::Utc>,
) -> ApiResult<HashMap<(SharedKind, Id), SharedListItem>> {
    use orch_access_contract::permissions::ObjectKind;
    use orch_api_contract::document_tree::{DocumentContainerMark, DocumentContainerRef};
    use orch_api_contract::documents::DocumentMark;
    if ids.is_empty() {
        return Ok(HashMap::new());
    }
    let readable: Vec<_> = orch_authz::documents::listed(conn, principal, ids, Policy::default())
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|listed| !listed.document.deleted && listed.holds(Permission::DocView))
        .collect();
    if readable.is_empty() {
        return Ok(HashMap::new());
    }
    let nodes: Vec<_> = readable.iter().map(|listed| listed.document.node).collect();
    let paths = view::ways(conn, principal, &nodes, Policy::default()).await?;
    let admin_ids: Vec<_> = readable
        .iter()
        .filter_map(|listed| listed.admin_access.and_then(|reach| reach.place.node()))
        .collect();
    let admin_refs = view::qualified_refs(conn, principal, &admin_ids, Policy::default()).await?;
    let mut result = HashMap::new();
    let mut text = HashMap::new();
    for listed in readable {
        if listed.document.kind == ObjectKind::Document {
            text.insert(listed.document.id, listed);
            continue;
        }
        if listed.document.kind != ObjectKind::DocumentContainer {
            continue;
        }
        let admin_access = listed
            .admin_access
            .and_then(|reach| view::admin_access(reach, &admin_refs));
        let folder = listed.document;
        result.insert(
            (SharedKind::Folder, folder.id),
            SharedListItem::DocumentContainer {
                kind: DocumentContainerMark::DocumentContainer,
                folder: DocumentContainerRef {
                    id: folder.id,
                    kind: DocumentContainerMark::DocumentContainer,
                    name: Some(folder.title),
                    redacted: false,
                },
                path: paths.get(&folder.node).cloned().unwrap_or_default(),
                participating: listed.participating,
                admin_access,
            },
        );
    }
    if text.is_empty() {
        return Ok(result);
    }
    // Ordinary work remains in its browsable place. Shared supplies the entrance
    // for a directly readable document whose containing place is unavailable.
    let places: Vec<_> = text.values().map(|listed| listed.document.node).collect();
    let visible: HashSet<_> = access::holding(
        conn,
        principal,
        &places,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|node| node.id)
    .collect();
    text.retain(|_, listed| !visible.contains(&listed.document.node));
    if text.is_empty() {
        return Ok(result);
    }
    let wanted: Vec<_> = text.keys().copied().collect();
    let entries: Vec<_> = orch_db::queries::documents::get_many(conn, &wanted)
        .await?
        .into_iter()
        .filter_map(|row| text.remove(&row.id).map(|listed| (row, listed)))
        .collect();
    let participation: HashMap<_, _> = entries
        .iter()
        .map(|(row, listed)| (row.id, listed.participating))
        .collect();
    let references =
        crate::routes::documents::build::refs(conn, principal, &entries, now, Policy::default())
            .await?;
    for document in references {
        let path = paths.get(&document.node.id).cloned().unwrap_or_default();
        result.insert(
            (SharedKind::Document, document.id),
            SharedListItem::Document {
                kind: DocumentMark::Document,
                participating: participation.get(&document.id).copied().unwrap_or(false),
                document: Box::new(document),
                path,
            },
        );
    }
    Ok(result)
}

/// A cursor of a list of topics: the key of the order, then the identifier.
fn topic_cursor(order: TopicOrder, text: &str) -> ApiResult<TopicCursor> {
    let bad = || invalid("cursor", "is not a cursor of this list");
    let (key, id) = text.rsplit_once('|').ok_or_else(bad)?;
    let id: Id = id.parse().map_err(|_| bad())?;
    Ok(match order {
        TopicOrder::Activity => TopicCursor::Activity(
            key.parse::<i64>()
                .ok()
                .and_then(chrono::DateTime::from_timestamp_micros)
                .ok_or_else(bad)?,
            id,
        ),
        TopicOrder::Created => TopicCursor::Created(id),
        TopicOrder::Name => TopicCursor::Name(key.to_owned(), id),
    })
}

fn cursor_text(cursor: &TopicCursor) -> String {
    match cursor {
        TopicCursor::Activity(at, id) => format!("{}|{id}", at.timestamp_micros()),
        TopicCursor::Created(id) => format!("|{id}"),
        TopicCursor::Name(name, id) => format!("{name}|{id}"),
    }
}

/// The topics of a project that the caller may view.
/// The most private topics a first page names for an admin: an existence list, sorted by name, and
/// bounded so that the first page costs what the others do.
const PRIVATE_TOPICS_MOST: i64 = 200;

/// The minimal rows of the topics of a project that the caller administers and may not view, sorted by
/// name and then id: only the name filter applies to them, since a status, tag or attention filter
/// would tell their work. The first page of the list carries them, after the readable rows.
async fn private_topics(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    project: Id,
    query: &TopicListQuery,
    archived: ArchivedMode,
) -> ApiResult<Vec<HiddenItem>> {
    let candidates = hierarchy::topics_of(
        conn,
        &TopicQuery {
            project,
            status: None,
            tags: &[],
            attention: None,
            with_folders: query.with_folders.unwrap_or(false),
            text: query.q.as_deref().filter(|text| !text.is_empty()),
            archived,
            order: TopicOrder::Name,
            after: None,
            limit: PRIVATE_TOPICS_MOST,
        },
    )
    .await?;
    let ids: Vec<Id> = candidates.iter().map(|(id, _)| *id).collect();
    private_rows(conn, principal, &ids).await
}

/// The minimal rows of the nodes `ids` that the caller administers and may not view, sorted by name
/// and then id: their identity, the place each lies in, kind, name, first owner and Private, and
/// nothing else. The statements are the same for any number of nodes; none beyond the first is taken
/// when the caller may view them all.
pub(crate) async fn private_rows(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    ids: &[Id],
) -> ApiResult<Vec<HiddenItem>> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let viewable: HashSet<Id> = access::holding(
        conn,
        principal,
        ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|each| each.id)
    .collect();
    let unseen: Vec<Id> = ids
        .iter()
        .copied()
        .filter(|id| !viewable.contains(id))
        .collect();
    if unseen.is_empty() {
        return Ok(Vec::new());
    }
    let managed = access::holding(
        conn,
        principal,
        &unseen,
        PermissionName::Exact(Permission::AccessManage),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    if managed.is_empty() {
        return Ok(Vec::new());
    }
    let managed_ids: Vec<Id> = managed.iter().map(|each| each.id).collect();
    let rows = hierarchy::with_ancestors(conn, &managed_ids).await?;
    let mut owners: Vec<Id> = rows
        .iter()
        .filter(|row| managed_ids.contains(&row.id))
        .map(|row| row.owner_id)
        .collect();
    owners.sort_unstable();
    owners.dedup();
    let people = refusals::user_refs(conn, &owners).await?;
    let mut hidden: Vec<HiddenItem> = managed
        .iter()
        .filter_map(|each| {
            let row = rows.iter().find(|row| row.id == each.id)?;
            Some(HiddenItem {
                id: row.id,
                parent_id: row.parent_id,
                kind: row.kind.clone(),
                name: each.name.clone(),
                owner: people.get(&row.owner_id).cloned(),
                content_hidden: true,
                private: true,
            })
        })
        .collect();
    hidden.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.id.cmp(&b.id))
    });
    Ok(hidden)
}

async fn topic_list(ctx: Ctx, query: TopicListQuery, (): ()) -> ApiResult<Page<Listed<Topic>>> {
    let project = ctx.id("id")?;
    let page = PageQuery {
        limit: query.limit,
        cursor: query.cursor.clone(),
    };
    let limit = usize::try_from(page.limit()).unwrap_or(usize::MAX);
    let order = match query.sort.unwrap_or_default() {
        TopicSort::Activity => TopicOrder::Activity,
        TopicSort::Created => TopicOrder::Created,
        TopicSort::Name => TopicOrder::Name,
    };
    let status = match query.status.unwrap_or_default() {
        TopicStatusFilter::Open => Some("open"),
        TopicStatusFilter::Done => Some("done"),
        TopicStatusFilter::All => None,
    };
    let attention = query.attention.map(|each| match each {
        AttentionFilter::NeedsAttention => "needs_attention",
        AttentionFilter::Running => "running",
        AttentionFilter::WaitingApproval => "waiting_approval",
        AttentionFilter::Blocked => "blocked",
    });
    let archived = match query.archived.unwrap_or_default() {
        ArchivedFilter::Exclude => ArchivedMode::Exclude,
        ArchivedFilter::Include => ArchivedMode::Include,
        ArchivedFilter::Only => ArchivedMode::Only,
    };
    let mut after = query
        .cursor
        .as_deref()
        .map(|text| topic_cursor(order, text))
        .transpose()?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let batch = i64::try_from(limit + 1).unwrap_or(i64::MAX);
    let mut items: Vec<(Topic, TopicCursor)> = Vec::new();
    loop {
        let found = hierarchy::topics_of(
            &mut conn,
            &TopicQuery {
                project,
                status,
                tags: &query.tag_id,
                attention,
                with_folders: query.with_folders.unwrap_or(false),
                text: query.q.as_deref().filter(|text| !text.is_empty()),
                archived,
                order,
                after: after.clone(),
                limit: batch,
            },
        )
        .await?;
        let exhausted = i64::try_from(found.len()).is_ok_and(|count| count < batch);
        let Some((_, last)) = found.last() else {
            break;
        };
        after = Some(last.clone());
        let ids: Vec<Id> = found.iter().map(|(id, _)| *id).collect();
        let cursors: HashMap<Id, TopicCursor> = found.into_iter().collect();
        for node in view::visible(&mut conn, &principal, &ids, ctx.now, Policy::default()).await? {
            if let Node::Topic(topic) = node
                && (!query.participating.unwrap_or(false) || topic.base.access.participating)
                && let Some(cursor) = cursors.get(&topic.base.id)
            {
                items.push((topic, cursor.clone()));
            }
        }
        if items.len() > limit || exhausted {
            break;
        }
    }
    let more = items.len() > limit;
    items.truncate(limit);
    // The private topics the caller administers stand once, on the first page, after what they may read.
    let hidden = if query.cursor.is_none() && !query.participating.unwrap_or(false) {
        private_topics(&mut conn, &principal, project, &query, archived).await?
    } else {
        Vec::new()
    };
    Ok(Page {
        next_cursor: more
            .then(|| items.last().map(|(_, cursor)| cursor_text(cursor)))
            .flatten(),
        items: items
            .into_iter()
            .map(|(topic, _)| Listed::visible(topic))
            .chain(hidden.into_iter().map(Listed::hidden))
            .collect(),
    })
}

/// Changes a topic's status or tags.
async fn topic_update(ctx: Ctx, (): (), body: TopicUpdateBody) -> ApiResult<Topic> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    if !hierarchy::lock(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    let (place, kind) = placed(&mut tx, id).await?;
    if kind != NodeKind::Topic {
        return Err(if kind == NodeKind::Thread {
            threads_are_not_served_here()
        } else {
            ApiError::not_found()
        });
    }
    let before = match shown(&ctx, &mut tx, &principal, id).await? {
        Node::Topic(topic) => topic,
        _ => return Err(ApiError::not_found()),
    };
    if let Some(expected) = body.expected_revision
        && expected != before.base.revision
    {
        return Err(ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "the topic was changed since it was read",
        )
        .with("current", &before));
    }
    let who = actor_name(&ctx)?;
    if let Some(tags) = &body.tag_ids {
        check_tags(&mut tx, id, tags).await?;
        hierarchy::set_node_tags(&mut tx, id, tags, ctx.now).await?;
        record_topic(
            &ctx,
            &mut tx,
            AuditAction::NodeUpdated,
            place,
            &format!("{who} set the tags of {}", before.base.name),
            json!({ "tags": tags }),
        )
        .await?;
    }
    if let Some(status) = body.status {
        let word = match status {
            TopicStatus::Open => "open",
            TopicStatus::Done => "done",
        };
        if hierarchy::set_topic_status(&mut tx, id, word, principal.user(), ctx.now).await? {
            let said = match status {
                TopicStatus::Open => "reopened",
                TopicStatus::Done => "marked done",
            };
            record_topic(
                &ctx,
                &mut tx,
                orch_db::audit::action!(AuditAction::TopicStatusChanged),
                place,
                &format!("{who} {said} {}", before.base.name),
                json!({ "status": word }),
            )
            .await?;
        }
    }
    hierarchy::stamp(&mut tx, id).await?;
    let node = shown(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    match node {
        Node::Topic(topic) => Ok(topic),
        _ => Err(ApiError::not_found()),
    }
}

async fn tag_set(ctx: Ctx, (): (), body: NodeTagSetBody) -> ApiResult<Node> {
    let id = ctx.id("id")?;
    let (ctx, mut tx, principal) = super::action(&ctx).await?;
    if !hierarchy::lock(&mut tx, id).await? {
        return Err(ApiError::not_found());
    }
    let (place, _) = placed(&mut tx, id).await?;
    let before = shown(&ctx, &mut tx, &principal, id).await?;
    let revision = match &before {
        Node::Org(row) => row.base.revision,
        Node::Project(row) => row.base.revision,
        Node::Topic(row) => row.base.revision,
        Node::Thread(row) => row.base.revision,
    };
    if body
        .expected_revision
        .is_some_and(|expected| expected != revision)
    {
        return Err(super::stale(&before));
    }
    check_tags(&mut tx, id, &body.tag_ids).await?;
    hierarchy::set_node_tags(&mut tx, id, &body.tag_ids, ctx.now).await?;
    let who = actor_name(&ctx)?;
    record(
        &ctx,
        &mut tx,
        AuditAction::NodeUpdated,
        place,
        &format!("{who} set the tags of {}", super::before_name(&before)),
        json!({ "tags": body.tag_ids }),
    )
    .await?;
    hierarchy::stamp(&mut tx, id).await?;
    let node = shown(&ctx, &mut tx, &principal, id).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(node)
}

async fn record_topic(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    action: AuditAction,
    place: Placed,
    summary: &str,
    details: serde_json::Value,
) -> ApiResult<()> {
    record(ctx, conn, action, place, summary, details).await
}

/// The tags a topic is to carry must be defined by its project, by a folder or project above it or
/// by an org above it, and be no more than the setting allows. A tag it carries already may stay
/// when the place that defined it is no longer above it, as it is when the topic has been moved:
/// only a tag that is added is held to the rule.
async fn check_tags(conn: &mut orch_db::Conn, topic: Id, tags: &[Id]) -> ApiResult<()> {
    let mut distinct = tags.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let most = orch_settings::get::<i64>(
        conn,
        orch_settings::Place::Node(topic),
        orch_settings_contract::registry::keys::TAGS_MAX_PER_PLACE,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or_default();
    if i64::try_from(distinct.len()).unwrap_or(i64::MAX) > most {
        return Err(invalid(
            "tag_ids",
            &format!("a place carries at most {most} tags here"),
        ));
    }
    let found = hierarchy::tags_by_id(conn, &distinct).await?;
    let carried: Vec<Id> = hierarchy::node_tags(conn, &[topic])
        .await?
        .into_iter()
        .map(|each| each.id)
        .collect();
    let rows = hierarchy::with_ancestors(conn, &[topic]).await?;
    let above: Vec<Id> = rows
        .iter()
        .find(|row| row.id == topic)
        .map(|row| row.path.clone())
        .unwrap_or_default();
    for (index, tag) in distinct.iter().enumerate() {
        let defined = found.iter().find(|each| each.id == *tag);
        if !defined.is_some_and(|each| above.contains(&each.node_id) || carried.contains(tag)) {
            return Err(invalid(
                &format!("tag_ids[{index}]"),
                "is no tag defined here or above this place",
            ));
        }
    }
    Ok(())
}
