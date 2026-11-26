//! Shortcuts: pointers to documents, chats, threads and places, kept in a person's own list or in a place.
//!
//! A shortcut points by kind and identity, so a move or a rename of its target changes nothing here. Its reader is
//! told the target's current name, icon and place only as far as they may see the target itself: the tree's own
//! visibility for places and threads, the names' bulk check for documents and chats. A target they may not open is
//! unavailable to them, with nothing more said of it; one in the trash, or gone for good, is deleted, and opens again
//! when it is restored. A shortcut grants nothing, and nobody is told who keeps a shortcut to what is theirs.
//!
//! Who: a shortcut is kept in a place by whoever may add documents there (`doc.create`) and may open its target;
//! one kept in a place is moved or removed by the person who put it there or by whoever manages the documents there
//! (`doc.manage`); a person's own list is theirs alone.

use super::*;

use orch_access_contract::permissions::Permission;

use orch_api_contract::common::Page;

use orch_api_contract::nodes::Node;

use orch_api_contract::personal::NodeShortcutListQuery;

use orch_api_contract::personal::SHORTCUTS_PAGE_DEFAULT;

use orch_api_contract::personal::Shortcut;

use orch_api_contract::personal::ShortcutItem;

use orch_api_contract::personal::ShortcutMoveBody;

use orch_api_contract::personal::ShortcutTargetState;

use orch_api_contract::search::NameKind;

use orch_db::Conn;

use orch_db::queries::hierarchy::ShortcutRow;

use std::collections::BTreeSet;

use std::collections::HashMap;

/// The kinds of node a shortcut can point to.
const NODE_KINDS: &[&str] = &["org", "project", "topic", "thread"];

/// The kinds a person's own list shows: places and threads, documents and chats.
pub(crate) const OWN_KINDS: &[&str] = &["org", "project", "topic", "thread", "document", "chat"];

/// What a node of the tree says of itself on a shortcut to it.
struct NodeFacts {
    archived: bool,
    color: Option<u32>,
    custom_color: Option<String>,
    org_id: Id,
}

fn node_facts(node: &Node) -> (Id, NodeFacts) {
    macro_rules! facts {
        ($base:expr) => {
            (
                $base.id,
                NodeFacts {
                    archived: $base.archived,
                    color: $base.color,
                    custom_color: $base.custom_color.clone(),
                    org_id: $base.org_id,
                },
            )
        };
    }
    match node {
        Node::Org(node) => facts!(node.base),
        Node::Project(node) => facts!(node.base),
        Node::Topic(node) => facts!(node.base),
        Node::Thread(node) => facts!(node.base),
    }
}

/// The shortcuts of `rows`, in their order, as `principal` may see their targets now. With `names` (the node names a
/// scoped reader's own operation may give), the way down to a place or a thread names only what that operation may
/// name, and a document or a chat is not named at all: it says only whether it opens.
pub(crate) async fn render(
    conn: &mut Conn,
    principal: &Principal,
    rows: &[ShortcutRow],
    names: Option<&BTreeSet<Id>>,
) -> ApiResult<Vec<Shortcut>> {
    let now = principal.now().get();
    let node_ids: Vec<Id> = rows
        .iter()
        .filter(|row| NODE_KINDS.contains(&row.kind.as_str()))
        .map(|row| row.target_id)
        .collect();
    let nodes: HashMap<Id, NodeFacts> = if node_ids.is_empty() {
        HashMap::new()
    } else {
        view::visible(conn, principal, &node_ids, now, Policy::default())
            .await?
            .iter()
            .map(node_facts)
            .collect()
    };
    let item_ids: Vec<Id> = rows
        .iter()
        .filter(|row| matches!(row.kind.as_str(), "document" | "chat"))
        .map(|row| row.target_id)
        .collect();
    let mut named = HashMap::new();
    if !item_ids.is_empty() {
        let kinds = ["chat".to_owned(), "document".to_owned()];
        let candidates = orch_db::queries::search::names(
            conn,
            &orch_db::queries::search::Names {
                identifiers: Some(&item_ids),
                starred: false,
                picker_count: None,
                at: now,
                user: principal.user(),
                trees: principal.member_of(),
                roots: &[],
                system_admin: principal.is_system_admin(),
                text: "",
                kinds: &kinds,
                org: None,
                offset: 0,
                limit: i64::try_from(item_ids.len()).map_err(ApiError::internal)?,
            },
        )
        .await?;
        let visible = orch_engine::projections::visible_names(conn, principal, &candidates)
            .await
            .map_err(ApiError::internal)?;
        for row in candidates {
            if visible.contains(&(row.kind.clone(), row.id)) {
                named.insert((row.kind.clone(), row.id), row);
            }
        }
    }
    let mut way_ids: Vec<Id> = nodes.keys().copied().collect();
    if names.is_none() {
        way_ids.extend(named.values().filter_map(|row| row.node_id));
    }
    way_ids.sort_unstable();
    way_ids.dedup();
    let ways = if way_ids.is_empty() {
        HashMap::new()
    } else {
        match names {
            Some(names) => {
                view::ways_with_name_sources(conn, principal, &way_ids, Policy::default(), names)
                    .await?
            }
            None => view::ways(conn, principal, &way_ids, Policy::default()).await?,
        }
    };
    // Whether each target opens for the reader, and what they are told of it.
    let mut seen = Vec::with_capacity(rows.len());
    for row in rows {
        let entry = if let Some(facts) = nodes.get(&row.target_id) {
            match ways.get(&row.target_id).and_then(|way| way.split_last()) {
                Some((own, above)) => (
                    true,
                    Some(ShortcutItem {
                        name: own.name.clone(),
                        path: above.to_vec(),
                        org_id: Some(facts.org_id),
                        archived: facts.archived,
                        color: facts.color,
                        custom_color: facts.custom_color.clone(),
                        thread_display: own.thread_display.as_deref().cloned(),
                    }),
                ),
                None => (false, None),
            }
        } else if let Some(found) = named.get(&(row.kind.clone(), row.target_id)) {
            (
                true,
                names.is_none().then(|| ShortcutItem {
                    name: Some(found.name.clone()),
                    path: found
                        .node_id
                        .and_then(|node| ways.get(&node).cloned())
                        .unwrap_or_default(),
                    org_id: found.org_id,
                    archived: found.archived,
                    color: None,
                    custom_color: None,
                    thread_display: None,
                }),
            )
        } else {
            (false, None)
        };
        seen.push(entry);
    }
    let closed: Vec<(String, Id)> = rows
        .iter()
        .zip(&seen)
        .filter(|(_, (open, _))| !open)
        .map(|(row, _)| (row.kind.clone(), row.target_id))
        .collect();
    let gone = hierarchy::gone_targets(conn, &closed).await?;
    let mut shortcuts = Vec::with_capacity(rows.len());
    for (row, (open, item)) in rows.iter().zip(seen) {
        let kind: NameKind =
            serde_json::from_value(Json::String(row.kind.clone())).map_err(ApiError::internal)?;
        let state = if open {
            ShortcutTargetState::Open
        } else if gone.contains(&(row.kind.clone(), row.target_id)) {
            ShortcutTargetState::Deleted
        } else {
            ShortcutTargetState::Unavailable
        };
        shortcuts.push(Shortcut {
            id: row.id,
            node_id: row.node_id,
            target: PersonalTarget {
                kind,
                id: row.target_id,
            },
            position: row.position.clone(),
            state,
            item,
            view: row
                .view
                .clone()
                .filter(|_| row.node_id.is_none())
                .and_then(|view| serde_json::from_value(view).ok()),
        });
    }
    Ok(shortcuts)
}

/// What a reader is told of the targets of the shortcuts their request reads: a scoped key's operation names no target
/// on its own (an empty set of names), an ordinary reader everything they may see.
fn reading(principal: &Principal) -> Option<BTreeSet<Id>> {
    crate::key_operations::native::modern(principal.caller()).then(BTreeSet::new)
}

/// The shortcuts kept in a place, a page at a time, each as the caller may see its target now.
pub(super) async fn list(
    ctx: Ctx,
    query: NodeShortcutListQuery,
    (): (),
) -> ApiResult<Page<Shortcut>> {
    let place = ctx.id("id")?;
    let limit = query.limit.unwrap_or(SHORTCUTS_PAGE_DEFAULT);
    if limit == 0 {
        return Err(invalid("limit", "must be greater than zero"));
    }
    let after = match query.cursor.as_deref() {
        None => None,
        Some(cursor) => Some(
            cursor
                .parse::<Id>()
                .map_err(|_| invalid("cursor", "is no cursor this list gave"))?,
        ),
    };
    let page = usize::try_from(limit).map_err(ApiError::internal)?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let mut rows =
        hierarchy::place_shortcuts(&mut conn, place, after, i64::from(limit) + 1).await?;
    let more = rows.len() > page;
    rows.truncate(page);
    let next_cursor = if more {
        rows.last().map(|row| row.id.to_string())
    } else {
        None
    };
    let names = reading(&principal);
    let items = render(&mut conn, &principal, &rows, names.as_ref()).await?;
    Ok(Page { items, next_cursor })
}

/// Refuses a place that keeps no shortcuts, and a kind of thing a place keeps none to.
async fn place_for(conn: &mut Conn, place: Id, kind: &str) -> ApiResult<()> {
    if !hierarchy::PLACE_SHORTCUT_KINDS.contains(&kind) {
        return Err(invalid(
            "kind",
            "is none of document, chat, thread, project and topic",
        ));
    }
    let (_, place_kind) = placed(conn, place).await?;
    if place_kind == NodeKind::Thread {
        return Err(invalid(
            "node_id",
            "is a thread; a shortcut is kept in an org, a project or a topic",
        ));
    }
    Ok(())
}

/// Keeps a shortcut in a place to a thing the caller may open. The route has checked that they may add documents
/// there; the one kept there already answers.
pub(super) async fn create(ctx: Ctx, (): (), target: PersonalTarget) -> ApiResult<Shortcut> {
    let place = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    place_for(&mut tx, place, target.kind.as_str()).await?;
    require_target(&mut tx, principal, &target, guard.now()).await?;
    let row = hierarchy::put_place_shortcut(
        &mut tx,
        place,
        principal.user(),
        target.kind.as_str(),
        target.id,
        guard.now().get(),
    )
    .await?;
    hierarchy::stamp(&mut tx, place).await?;
    let names = reading(principal);
    let shortcut = render(
        &mut tx,
        principal,
        std::slice::from_ref(&row),
        names.as_ref(),
    )
    .await?
    .pop()
    .ok_or_else(|| ApiError::internal("the kept shortcut was not rendered"))?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shortcut)
}

/// Whether the caller may move or remove the shortcut where it is: their own list is theirs; in a place, the person
/// who put it there, or whoever manages the documents there.
async fn may_change(conn: &mut Conn, principal: &Principal, row: &ShortcutRow) -> ApiResult<()> {
    match row.node_id {
        None if row.user_id == principal.user() => Ok(()),
        None => Err(ApiError::not_found()),
        Some(_) if row.user_id == principal.user() => Ok(()),
        Some(place) => {
            authorization::require(
                conn,
                principal,
                place,
                authorization::exact(Permission::DocManage),
                Act::Use,
                Policy::default(),
            )
            .await
        }
    }
}

/// Puts a shortcut in another place, or in the caller's own list. Its target is not touched; the one already there
/// to the same thing answers.
pub(super) async fn move_to(ctx: Ctx, (): (), body: ShortcutMoveBody) -> ApiResult<Shortcut> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    let row = hierarchy::shortcut_for_change(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    may_change(&mut tx, principal, &row).await?;
    if let Some(place) = body.node_id {
        place_for(&mut tx, place, &row.kind).await?;
        authorization::require(
            &mut tx,
            principal,
            place,
            authorization::exact(Permission::DocCreate),
            Act::Use,
            Policy::default(),
        )
        .await?;
    }
    let moved = hierarchy::move_shortcut(
        &mut tx,
        id,
        body.node_id,
        principal.user(),
        &row.kind,
        row.target_id,
    )
    .await?;
    for place in [row.node_id, body.node_id].into_iter().flatten() {
        hierarchy::stamp(&mut tx, place).await?;
    }
    let names = reading(principal);
    let shortcut = render(
        &mut tx,
        principal,
        std::slice::from_ref(&moved),
        names.as_ref(),
    )
    .await?
    .pop()
    .ok_or_else(|| ApiError::internal("the moved shortcut was not rendered"))?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shortcut)
}

/// Removes a shortcut; its target is not touched. One that is gone already is not there to remove.
pub(super) async fn delete(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let id = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let principal = guard.principal();
    let Some(row) = hierarchy::shortcut_for_change(&mut tx, id).await? else {
        return Err(ApiError::not_found());
    };
    may_change(&mut tx, principal, &row).await?;
    hierarchy::delete_shortcut(&mut tx, id).await?;
    if let Some(place) = row.node_id {
        hierarchy::stamp(&mut tx, place).await?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
