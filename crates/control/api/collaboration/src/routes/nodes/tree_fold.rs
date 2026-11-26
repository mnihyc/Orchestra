//! Permission-resolved fold probes and sparse attention windows, without a quiet subtree inventory.

use super::hierarchy_operation as scope;
use crate::key_operations::{native, targets};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_access_contract::permissions::{NodePermission, PermissionName};
use orch_api_contract::hierarchy_sources::{self as sources, Source};
use orch_api_contract::nodes::{TreeFold, TreeFoldCounts};
use orch_api_contract::threads::{THREAD_DISPLAY_URGENCY, ThreadDisplayState};
use orch_authz::{Act, Caller, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::{chats, hierarchy};
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashMap};

#[derive(Default)]
pub(super) struct Folded {
    pub folds: HashMap<Id, TreeFold>,
    pub nodes: BTreeMap<Id, KeyResolvedTarget>,
    pub visible: BTreeSet<Id>,
    pub group_targets: BTreeMap<Id, KeyResolvedTarget>,
    pub groups: BTreeSet<Id>,
    keep_groups: BTreeSet<Id>,
    raw_containers: BTreeSet<Id>,
}

pub(super) struct Read<'a> {
    pub root: Id,
    pub parents: &'a [Id],
    pub limit: i64,
    pub opening: bool,
}

fn class(fold: &mut TreeFold, archived: bool) -> &mut TreeFoldCounts {
    if archived {
        fold.archived.get_or_insert_with(Default::default)
    } else {
        &mut fold.active
    }
}

fn urgency(fold: &mut TreeFoldCounts, state: ThreadDisplayState) -> bool {
    let rank = |state| THREAD_DISPLAY_URGENCY.iter().position(|at| *at == state);
    if let Some(next) = rank(state)
        && fold.urgency.and_then(rank).is_none_or(|held| next < held)
    {
        fold.urgency = Some(state);
        true
    } else {
        false
    }
}

fn retain_groups(output: &mut Folded) {
    output.groups.retain(|id| output.keep_groups.contains(id));
    output
        .group_targets
        .retain(|id, _| output.keep_groups.contains(id));
}

async fn scoped_nodes(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    ids: &[Id],
) -> ApiResult<BTreeMap<Id, KeyResolvedTarget>> {
    let graph = orch_authz::engine::Graph::load(conn, ids)
        .await
        .map_err(ApiError::internal)?;
    let mut targets = BTreeMap::new();
    for id in ids {
        let target = orch_engine::operation_scope::targets::from_node_identity(&graph, *id)
            .ok_or_else(ApiError::not_found)?;
        if scope::permits(
            ctx,
            caller,
            &sources::TREE,
            Source::Results,
            target.object.kind,
            &target,
        )? {
            targets.insert(*id, target);
        }
    }
    Ok(targets)
}

async fn visible_at_least(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    mut query: hierarchy::TreeFoldStructure,
    wanted: i64,
    scan: i64,
) -> ApiResult<bool> {
    let mut count = 0_i64;
    loop {
        let batch = hierarchy::tree_fold_structure_window(conn, &query).await?;
        let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < query.limit;
        query.after = batch.last().copied();
        if query.direct && !batch.is_empty() {
            output.raw_containers.insert(query.parent);
        }
        if !batch.is_empty() {
            let mut targets = scoped_nodes(ctx, conn, caller, &batch).await?;
            let plan = native::bind_known(
                ctx,
                &sources::TREE,
                &[
                    scope::collection(Source::Results, targets.values().cloned().collect()),
                    scope::collection(Source::Groups, Vec::new()),
                ],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            let visible = orch_authz::nodes::holding(
                conn,
                &principal,
                &targets.keys().copied().collect::<Vec<_>>(),
                PermissionName::OfNode(NodePermission::View),
                Act::Use,
                Policy::default(),
            )
            .await
            .map_err(ApiError::internal)?;
            for row in visible {
                if let Some(target) = targets.remove(&row.id) {
                    output.visible.insert(row.id);
                    output.nodes.insert(row.id, target);
                    count += 1;
                }
                if count >= wanted {
                    return Ok(true);
                }
            }
        }
        if exhausted {
            return Ok(false);
        }
        query.limit = query.limit.max(scan);
    }
}

/// Which of `children` the reader may view, decided at once as [`visible_at_least`] decides each window; each visible
/// child is kept with its fold's results.
async fn visible_of(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    children: &[Id],
) -> ApiResult<BTreeSet<Id>> {
    if children.is_empty() {
        return Ok(BTreeSet::new());
    }
    let mut targets = scoped_nodes(ctx, conn, caller, children).await?;
    let plan = native::bind_known(
        ctx,
        &sources::TREE,
        &[
            scope::collection(Source::Results, targets.values().cloned().collect()),
            scope::collection(Source::Groups, Vec::new()),
        ],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let visible = orch_authz::nodes::holding(
        conn,
        &principal,
        &targets.keys().copied().collect::<Vec<_>>(),
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let mut shown = BTreeSet::new();
    for row in visible {
        if let Some(target) = targets.remove(&row.id) {
            output.visible.insert(row.id);
            output.nodes.insert(row.id, target);
            shown.insert(row.id);
        }
    }
    Ok(shown)
}

/// Every place's work, finished, unread and group signals, each read once for all the places that still look for it, in
/// the order and with the stop rules the one-place windows below keep: work before finished history, a place already
/// waiting for an approval reads no more work, a place with a state reads no finished history, an unread place reads no
/// more unread. A window that comes back full (more candidates than one page) leaves the reading to those one-place
/// windows (false); re-reading a candidate there changes nothing, since a signal raises a place's fold once.
async fn signals_together(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    parents: &[Id],
    limit: i64,
    possible_groups: bool,
) -> ApiResult<bool> {
    let full = |count: usize| i64::try_from(count).unwrap_or(i64::MAX) >= limit;
    fn read<'a>(
        caller: &Caller,
        ctx: &Ctx,
        places: &'a [Id],
        limit: i64,
        archived: bool,
    ) -> hierarchy::TreeFoldRead<'a> {
        hierarchy::TreeFoldRead {
            user: caller.user(),
            parents: places,
            now: ctx.now,
            after: None,
            limit,
            archived: Some(archived),
        }
    }
    for signal in [
        hierarchy::TreeFoldSignal::Work,
        hierarchy::TreeFoldSignal::Finished,
    ] {
        let places = parents
            .iter()
            .copied()
            .filter(|parent| output.raw_containers.contains(parent))
            .filter(|parent| {
                let held = state(output, *parent);
                held != Some(ThreadDisplayState::WaitingApproval)
                    && !(signal == hierarchy::TreeFoldSignal::Finished && held.is_some())
            })
            .collect::<Vec<_>>();
        if places.is_empty() {
            continue;
        }
        let batch =
            hierarchy::tree_fold_window(conn, &read(caller, ctx, &places, limit, false), signal)
                .await?;
        if full(batch.len()) {
            return Ok(false);
        }
        nodes(output, ctx, conn, caller, batch, false).await?;
    }
    for archived in [false, true] {
        let places = parents
            .iter()
            .copied()
            .filter(|parent| {
                output.raw_containers.contains(parent) && !unread(output, *parent, archived)
            })
            .collect::<Vec<_>>();
        if !places.is_empty() {
            let batch = hierarchy::tree_fold_window(
                conn,
                &read(caller, ctx, &places, limit, archived),
                hierarchy::TreeFoldSignal::Unread,
            )
            .await?;
            if full(batch.len()) {
                return Ok(false);
            }
            nodes(output, ctx, conn, caller, batch, false).await?;
        }
        if possible_groups {
            let places = parents
                .iter()
                .copied()
                .filter(|parent| !unread(output, *parent, archived))
                .collect::<Vec<_>>();
            if !places.is_empty() {
                let batch = hierarchy::tree_group_signal_window(
                    conn,
                    &read(caller, ctx, &places, limit, archived),
                    false,
                )
                .await?;
                if full(batch.len()) {
                    return Ok(false);
                }
                groups(output, ctx, conn, caller, batch, false).await?;
            }
        }
    }
    Ok(true)
}

/// The optional chat source is admitted separately from each counted node's View.
async fn reader(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    nodes: &[KeyResolvedTarget],
    ids: &[Id],
) -> ApiResult<(Principal, BTreeSet<Id>)> {
    let rows = if ids.is_empty() {
        Vec::new()
    } else {
        chats::chats(conn, ids).await?
    };
    let targets = if rows.is_empty() {
        HashMap::new()
    } else {
        targets::from_chat_rows(conn, &rows).await?
    };
    let mut admitted = Vec::new();
    let mut facts = Vec::new();
    for row in rows {
        if let Some(target) = targets.get(&row.id)
            && scope::permits(
                ctx,
                caller,
                &sources::TREE,
                Source::Groups,
                target.object.kind,
                target,
            )?
        {
            facts.push(target.clone());
            admitted.push(row);
        }
    }
    let plan = native::bind_known(
        ctx,
        &sources::TREE,
        &[
            scope::collection(Source::Results, nodes.to_vec()),
            scope::collection(Source::Groups, facts.clone()),
        ],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
    let allowed = orch_authz::chats::listed_access(conn, &principal, &admitted)
        .await
        .map_err(ApiError::internal)?;
    let allowed = allowed.into_keys().collect::<BTreeSet<_>>();
    output.groups.extend(allowed.iter().copied());
    for target in facts {
        if let Some(id) = target.object.id {
            output.group_targets.insert(id, target);
        }
    }
    Ok((principal, allowed))
}

async fn nodes(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: Vec<hierarchy::TreeFoldCandidate>,
    totals: bool,
) -> ApiResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut targets = scoped_nodes(
        ctx,
        conn,
        caller,
        &rows.iter().map(|row| row.id).collect::<Vec<_>>(),
    )
    .await?;
    let chats = rows
        .iter()
        .filter(|row| {
            totals && row.mentions > 0
                || row.chat_unread
                    && row
                        .path
                        .iter()
                        .filter(|id| **id != row.id && output.folds.contains_key(id))
                        .any(|id| !unread(output, *id, row.archived))
        })
        .filter_map(|row| row.chat_id)
        .collect::<Vec<_>>();
    let (principal, chats) = reader(
        output,
        ctx,
        conn,
        caller,
        &targets.values().cloned().collect::<Vec<_>>(),
        &chats,
    )
    .await?;
    let visible = orch_authz::nodes::holding(
        conn,
        &principal,
        &targets.keys().copied().collect::<Vec<_>>(),
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let visible = visible
        .into_iter()
        .map(|row| row.id)
        .collect::<BTreeSet<_>>();
    let threads = rows
        .iter()
        .filter(|row| row.kind == "thread" && visible.contains(&row.id))
        .map(|row| row.id)
        .collect::<Vec<_>>();
    let displays =
        orch_engine::views::thread_displays(conn, &threads, Some(caller.user()), principal.now())
            .await
            .map_err(ApiError::internal)?;
    for row in rows.into_iter().filter(|row| visible.contains(&row.id)) {
        let chat_visible = row.chat_id.is_some_and(|id| chats.contains(&id));
        let mut contributes = false;
        let mut chat_contributes = false;
        for id in row.path.iter().filter(|id| **id != row.id) {
            if let Some(fold) = output.folds.get_mut(id) {
                let fold = class(fold, row.archived);
                if totals {
                    fold.waiting += row.waiting;
                    contributes |= row.waiting > 0;
                    if chat_visible {
                        fold.mentions += row.mentions;
                        chat_contributes |= row.mentions > 0;
                    }
                }
                if !fold.unread && (row.unread || chat_visible && row.chat_unread) {
                    fold.unread = true;
                    contributes = true;
                    chat_contributes |= !row.unread && chat_visible && row.chat_unread;
                }
                if let Some(display) = displays.get(&row.id) {
                    contributes |= urgency(fold, display.state);
                }
            }
        }
        if chat_contributes && let Some(id) = row.chat_id {
            output.keep_groups.insert(id);
            contributes = true;
        }
        if contributes && let Some(target) = targets.remove(&row.id) {
            output.nodes.insert(row.id, target);
            output.visible.insert(row.id);
        }
    }
    retain_groups(output);
    Ok(())
}

async fn groups(
    output: &mut Folded,
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    rows: Vec<hierarchy::TreeGroupSignal>,
    totals: bool,
) -> ApiResult<()> {
    if rows.is_empty() {
        return Ok(());
    }
    let (_, allowed) = reader(
        output,
        ctx,
        conn,
        caller,
        &[],
        &rows.iter().map(|row| row.chat_id).collect::<Vec<_>>(),
    )
    .await?;
    for row in rows
        .into_iter()
        .filter(|row| allowed.contains(&row.chat_id))
    {
        let mut contributes = false;
        for id in &row.path {
            if let Some(fold) = output.folds.get_mut(id) {
                let fold = class(fold, row.archived);
                contributes |= !fold.unread && row.unread || totals && row.mentions > 0;
                fold.unread |= row.unread;
                if totals {
                    fold.mentions += row.mentions;
                }
            }
        }
        if contributes {
            output.keep_groups.insert(row.chat_id);
        }
    }
    retain_groups(output);
    Ok(())
}

fn unread(output: &Folded, parent: Id, archived: bool) -> bool {
    output
        .folds
        .get(&parent)
        .and_then(|fold| {
            if archived {
                fold.archived.as_ref()
            } else {
                Some(&fold.active)
            }
        })
        .is_some_and(|fold| fold.unread)
}

fn state(output: &Folded, parent: Id) -> Option<ThreadDisplayState> {
    output
        .folds
        .get(&parent)
        .and_then(|fold| fold.active.urgency)
}

pub(super) async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    read: Read<'_>,
    rows: &[hierarchy::TreeRow],
) -> ApiResult<Folded> {
    let Read {
        root,
        parents,
        limit,
        opening,
    } = read;
    let mut output = Folded {
        folds: parents
            .iter()
            .map(|id| (*id, TreeFold::default()))
            .collect(),
        ..Default::default()
    };
    let containers = hierarchy::tree_fold_containers(conn, root, parents)
        .await?
        .into_iter()
        .collect::<BTreeSet<_>>();
    // Every place's first live and first archived child, in one statement and one decision of the reader's View: only a
    // place whose first child of a class is hidden from the reader looks further, a window at a time.
    let firsts: HashMap<(Id, bool), Id> = hierarchy::tree_fold_first_children(
        conn,
        root,
        &containers.iter().copied().collect::<Vec<_>>(),
    )
    .await?
    .into_iter()
    .map(|(parent, archived, child)| ((parent, archived), child))
    .collect();
    let shown = visible_of(
        &mut output,
        ctx,
        conn,
        caller,
        &firsts.values().copied().collect::<Vec<_>>(),
    )
    .await?;
    for parent in parents {
        for archived in [false, true]
            .into_iter()
            .filter(|_| containers.contains(parent))
        {
            let first = firsts.get(&(*parent, archived)).copied();
            if first.is_some() {
                output.raw_containers.insert(*parent);
            }
            let has_children = match first {
                None => false,
                Some(child) if shown.contains(&child) => true,
                Some(child) => {
                    visible_at_least(
                        &mut output,
                        ctx,
                        conn,
                        caller,
                        hierarchy::TreeFoldStructure {
                            root,
                            parent: *parent,
                            direct: true,
                            archived: Some(archived),
                            after: Some(child),
                            limit,
                        },
                        1,
                        limit,
                    )
                    .await?
                }
            };
            if (!archived || has_children)
                && let Some(fold) = output.folds.get_mut(parent)
            {
                class(fold, archived).has_children = has_children;
            }
        }
        if opening && *parent == root {
            let small = !visible_at_least(
                &mut output,
                ctx,
                conn,
                caller,
                hierarchy::TreeFoldStructure {
                    root,
                    parent: *parent,
                    direct: false,
                    archived: None,
                    after: None,
                    limit: limit.saturating_add(1),
                },
                limit.saturating_add(1),
                limit,
            )
            .await?;
            if let Some(fold) = output.folds.get_mut(parent) {
                fold.small_tree = Some(small);
            }
        }
    }
    let own_chats = rows
        .iter()
        .filter_map(|row| row.chat_id)
        .collect::<Vec<_>>();
    if !own_chats.is_empty() {
        output.keep_groups.extend(own_chats.iter().copied());
        reader(&mut output, ctx, conn, caller, &[], &own_chats).await?;
    }
    if parents.is_empty() {
        return Ok(output);
    }
    // A valid hierarchy has no descendant without a direct child. This local raw hint is never published.
    let node_parents = parents
        .iter()
        .filter(|id| output.raw_containers.contains(id))
        .copied()
        .collect::<Vec<_>>();
    // Exact totals visit only actual positive records, a native page at a time.
    let mut after = None;
    loop {
        let batch = hierarchy::tree_fold_window(
            conn,
            &hierarchy::TreeFoldRead {
                user: caller.user(),
                parents: &node_parents,
                now: ctx.now,
                after,
                limit,
                archived: None,
            },
            hierarchy::TreeFoldSignal::Counts,
        )
        .await?;
        let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < limit;
        after = batch.last().map(|row| row.id);
        nodes(&mut output, ctx, conn, caller, batch, true).await?;
        if exhausted {
            break;
        }
    }
    let mut after = None;
    loop {
        let batch = hierarchy::tree_group_signal_window(
            conn,
            &hierarchy::TreeFoldRead {
                user: caller.user(),
                parents,
                now: ctx.now,
                after,
                limit,
                archived: None,
            },
            true,
        )
        .await?;
        let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < limit;
        after = batch.last().map(|row| row.chat_id);
        groups(&mut output, ctx, conn, caller, batch, true).await?;
        if exhausted {
            break;
        }
    }
    let possible_groups = !hierarchy::tree_group_signal_window(
        conn,
        &hierarchy::TreeFoldRead {
            user: caller.user(),
            parents,
            now: ctx.now,
            after: None,
            limit: 1,
            archived: None,
        },
        false,
    )
    .await?
    .is_empty();
    if signals_together(
        &mut output,
        ctx,
        conn,
        caller,
        parents,
        limit,
        possible_groups,
    )
    .await?
    {
        return Ok(output);
    }
    for parent in parents {
        // All current-work candidates precede finished-only history. The native display still decides the icon.
        for signal in [
            hierarchy::TreeFoldSignal::Work,
            hierarchy::TreeFoldSignal::Finished,
        ] {
            let mut after = None;
            loop {
                if !output.raw_containers.contains(parent) {
                    break;
                }
                if state(&output, *parent) == Some(ThreadDisplayState::WaitingApproval)
                    || signal == hierarchy::TreeFoldSignal::Finished
                        && state(&output, *parent).is_some()
                {
                    break;
                }
                let batch = hierarchy::tree_fold_window(
                    conn,
                    &hierarchy::TreeFoldRead {
                        user: caller.user(),
                        parents: &[*parent],
                        now: ctx.now,
                        after,
                        limit,
                        archived: Some(false),
                    },
                    signal,
                )
                .await?;
                let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < limit;
                after = batch.last().map(|row| row.id);
                nodes(&mut output, ctx, conn, caller, batch, false).await?;
                if exhausted {
                    break;
                }
            }
        }
        for archived in [false, true] {
            let mut after = None;
            while output.raw_containers.contains(parent) && !unread(&output, *parent, archived) {
                let batch = hierarchy::tree_fold_window(
                    conn,
                    &hierarchy::TreeFoldRead {
                        user: caller.user(),
                        parents: &[*parent],
                        now: ctx.now,
                        after,
                        limit,
                        archived: Some(archived),
                    },
                    hierarchy::TreeFoldSignal::Unread,
                )
                .await?;
                let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < limit;
                after = batch.last().map(|row| row.id);
                nodes(&mut output, ctx, conn, caller, batch, false).await?;
                if exhausted {
                    break;
                }
            }
            let mut after = None;
            while possible_groups && !unread(&output, *parent, archived) {
                let batch = hierarchy::tree_group_signal_window(
                    conn,
                    &hierarchy::TreeFoldRead {
                        user: caller.user(),
                        parents: &[*parent],
                        now: ctx.now,
                        after,
                        limit,
                        archived: Some(archived),
                    },
                    false,
                )
                .await?;
                let exhausted = i64::try_from(batch.len()).unwrap_or(i64::MAX) < limit;
                after = batch.last().map(|row| row.chat_id);
                groups(&mut output, ctx, conn, caller, batch, false).await?;
                if exhausted {
                    break;
                }
            }
        }
    }
    Ok(output)
}
