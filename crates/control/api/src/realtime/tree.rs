//! The organization feed: the nodes of a root organization's tree that changed, cut down to the ones the subscriber may view.
//!
//! The feed is a root organization's and carries its whole tree, the
//! organizations inside it among it. Its sequence is the tree's counter, which
//! a change to any node of the tree moves, and a subscriber is sent the nodes
//! stamped after the number they hold, as `GET /orgs/{id}/tree` returns them
//! from that number: the ones they may view as they see them, and for one that
//! changed and is not theirs to see any longer a row that says it was deleted
//! and nothing else.
//!
//! Anybody who is signed in may follow the feed of an organization that is
//! there; what they are sent is decided for each node.

use crate::middleware::authorization::refusals::{self, node_kind};
use crate::realtime::subscriptions::{Access, principal_of};
use crate::realtime::sync::{Frame, Head, Rows};
use crate::realtime::{Fail, Inner};
use crate::routes::nodes::tree as tree_rows;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName};
use orch_api_contract::common::Listed;
use orch_api_contract::nodes::{
    NodeKind, TREE_SYNC_MAX_CHANGES, TopicStatus, TreeChat, TreeNode, TreeTopic,
};
use orch_api_contract::users::UserRef;
use orch_authz::nodes as access;
use orch_authz::{Act, Policy};
use orch_db::queries::{feeds, hierarchy, identity};
use orch_realtime_contract::feeds::{OrgData, SyncData};
use orch_realtime_contract::sync::FeedHead;
use orch_types::{Id, Seq};
use std::collections::HashMap;

/// Whether `org` is a root organization that is there.
pub(crate) async fn decide(inner: &Inner, _user: Id, org: Id) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(row) = hierarchy::org(&mut conn, org).await? else {
        return Ok(Access::NotFound);
    };
    if row.parent_id.is_some() {
        // An organization inside another has no feed of its own.
        return Ok(Access::NotFound);
    }
    if !feeds::gone_nodes(&mut conn, &[org]).await?.is_empty() {
        return Ok(Access::Gone);
    }
    Ok(Access::Allowed)
}

/// Where the tree's feed stands, and how many nodes lie above `since`.
pub(crate) async fn head(inner: &Inner, org: Id, since: Option<Seq>) -> Result<Option<Head>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some((_, seq)) = feeds::tree_seqs(&mut conn, &[org])
        .await?
        .into_iter()
        .next()
    else {
        return Ok(None);
    };
    let rows_after = match since {
        Some(since) if since <= seq => feeds::tree_changed_after(&mut conn, org, since).await?,
        _ => 0,
    };
    Ok(Some(Head {
        feed: FeedHead {
            seq,
            purged_seq: 0,
            incremental_limit: TREE_SYNC_MAX_CHANGES,
        },
        rows_after,
    }))
}

/// A node that changed and that the subscriber is not shown now: it is marked
/// deleted, with nothing else of it.
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

/// The nodes of the tree stamped in `(after, upto]`, for one subscriber.
pub(crate) async fn read(
    inner: &Inner,
    person: Id,
    org: Id,
    after: Seq,
    upto: Seq,
) -> Result<Rows, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, person).await? else {
        return Ok(Rows::Nothing);
    };
    let mut rows = hierarchy::tree(&mut conn, org, Some(after), TREE_SYNC_MAX_CHANGES + 1).await?;
    if i64::try_from(rows.len()).unwrap_or(i64::MAX) > TREE_SYNC_MAX_CHANGES {
        return Ok(Rows::TooMany);
    }
    // A node stamped after the number named waits for the frame that follows.
    rows.retain(|row| row.tree_seq <= upto);
    let Some(highest) = rows.iter().map(|row| row.tree_seq).max() else {
        return Ok(Rows::Nothing);
    };
    let ids: Vec<Id> = rows.iter().map(|row| row.id).collect();
    let readable_docs =
        crate::routes::nodes::tree_docs::readable(&mut conn, &principal, &ids).await?;
    let listed: HashMap<Id, access::Listed> = access::holding(
        &mut conn,
        &principal,
        &ids,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await?
    .into_iter()
    .map(|each| (each.id, each))
    .collect();
    // A private node the subscriber administers and may not view is its minimal row, as over HTTP.
    let unseen = tree_rows::unviewed(&rows, &listed);
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
        .await?
    };
    let owners = tree_rows::owners_of(&rows, &managed);
    let people: HashMap<Id, UserRef> = if owners.is_empty() {
        HashMap::new()
    } else {
        identity::users_brief(&mut conn, &owners)
            .await?
            .iter()
            .map(|brief| (brief.id, refusals::user_ref(brief)))
            .collect()
    };
    let mut hidden = tree_rows::hidden_rows(&rows, &managed, &people);
    let admin_ids: Vec<Id> = listed
        .values()
        .filter_map(|each| each.admin_access.and_then(|reach| reach.place.node()))
        .collect();
    let admin_refs = crate::routes::nodes::view::qualified_refs(
        &mut conn,
        &principal,
        &admin_ids,
        Policy::default(),
    )
    .await
    .map_err(Fail::other)?;
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
    let waiting = if let Some(engine) = inner.engine.get() {
        crate::routes::nodes::tree_work::waiting(
            &mut conn,
            &principal,
            &waiting_threads,
            inner.clock.now(),
            engine,
        )
        .await
        .map_err(Fail::other)?
    } else {
        // Before the engine starts there is no authoritative personal claim to publish.
        HashMap::new()
    };
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
    .map_err(Fail::other)?;
    let nodes: Vec<Listed<TreeNode>> = rows
        .into_iter()
        .filter_map(|row| {
            let kind = node_kind(&row.kind)?;
            if let Some(minimal) = hidden.remove(&row.id) {
                return Some(Listed::hidden(minimal));
            }
            let Some(each) = listed.get(&row.id).filter(|_| !row.deleted) else {
                return Some(Listed::visible(gone(&row, kind)));
            };
            Some(Listed::visible(TreeNode {
                activity_at: Some(orch_types::Time::new(row.activity_at)),
                admin_access: each
                    .admin_access
                    .and_then(|place| crate::routes::nodes::view::admin_access(place, &admin_refs)),
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
                    .then(|| {
                        tree_rows::thread_of(&row, waiting.get(&row.id), displays.get(&row.id))
                    })
                    .flatten(),
                chat: TreeChat {
                    id: row.chat_id.unwrap_or_else(Id::nil),
                    seq: row.chat_seq,
                },
            }))
        })
        .collect();
    Ok(Rows::Frame(Box::new(Frame {
        data: SyncData::Org(OrgData { nodes: Some(nodes) }),
        highest,
        held: Vec::new(),
    })))
}
