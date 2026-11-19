//! Scope the existing Shared union; native candidates, ACL, merge and DTOs stay native.

use super::projection::{self, Projection};
use super::read_sources::Sources;
use super::tree::{self, SharedCandidate, SharedKind};
use crate::key_operations::{admission, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture};
use crate::routes::access::page::NameBounds;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::PageQuery;
use orch_api_contract::nodes::{SharedListItem, SharedListReply};
use orch_authz::engine::Graph;
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_types::{Id, Time};
use std::collections::HashMap;

type Key = (SharedKind, Id);

struct Prepared {
    response: OperationPlan,
    reply: SharedListReply,
}

fn modern(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
fn key(item: &SharedListItem) -> Key {
    match item {
        SharedListItem::Node(node) => (SharedKind::Node, super::node_id(node)),
        SharedListItem::Document { document, .. } => (SharedKind::Document, document.id),
        SharedListItem::DocumentContainer { folder, .. } => (SharedKind::Folder, folder.id),
    }
}

async fn primary_targets(
    conn: &mut Conn,
    candidates: &[SharedCandidate],
) -> ApiResult<HashMap<Key, KeyResolvedTarget>> {
    let node_ids: Vec<_> = candidates
        .iter()
        .filter(|row| row.kind == SharedKind::Node)
        .map(|row| row.id)
        .collect();
    let graph = Graph::load(conn, &node_ids)
        .await
        .map_err(ApiError::internal)?;
    let mut found = HashMap::new();
    for id in node_ids {
        let site = graph.site_of(id).ok_or_else(ApiError::not_found)?;
        found.insert((SharedKind::Node, id), targets::from_site(&graph, &site)?);
    }
    let document_objects: Vec<_> = candidates
        .iter()
        .filter_map(|row| match row.kind {
            SharedKind::Node => None,
            SharedKind::Document => Some(ObjectRef {
                kind: ObjectKind::Document,
                id: row.id,
            }),
            SharedKind::Folder => Some(ObjectRef {
                kind: ObjectKind::DocumentContainer,
                id: row.id,
            }),
        })
        .collect();
    for (object, target) in Sources::from_objects(document_objects)
        .targets(conn)
        .await?
    {
        let kind = match object.kind {
            ObjectKind::Document => SharedKind::Document,
            ObjectKind::DocumentContainer => SharedKind::Folder,
            _ => return Err(ApiError::internal("a shared document target changed kind")),
        };
        found.insert((kind, object.id), target);
    }
    Ok(found)
}

fn bind_primary(
    plan: &mut OperationPlan,
    candidates: &[SharedCandidate],
    targets: &HashMap<Key, KeyResolvedTarget>,
) -> ApiResult<()> {
    projection::bind(
        plan,
        0,
        projection::groups(
            candidates
                .iter()
                .map(|row| {
                    let target = targets
                        .get(&(row.kind, row.id))
                        .ok_or_else(ApiError::not_found)?;
                    Ok((target.object.kind, target.clone()))
                })
                .collect::<ApiResult<Vec<_>>>()?,
        ),
    )
}

fn source_objects(
    items: &[SharedListItem],
    targets: &HashMap<Key, KeyResolvedTarget>,
) -> ApiResult<Sources> {
    let mut objects = Vec::new();
    for item in items {
        let primary = &targets
            .get(&key(item))
            .ok_or_else(ApiError::not_found)?
            .object;
        objects.extend(Sources::of_shared(item)?.objects().filter(|object| {
            !(KeyTargetKind::Object(object.kind) == primary.kind && Some(object.id) == primary.id)
        }));
    }
    Ok(Sources::from_objects(objects))
}

/// The same ordinary render first decides entrances. Scope then bounds optional
/// references using native builders and excludes an unrepresentable full payload.
#[allow(
    clippy::too_many_arguments,
    reason = "one native read checkpoint keeps its request, exact plan, principal and typed primary facts together"
)]
async fn bounded_items(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &OperationPlan,
    principal: &Principal,
    candidates: &[SharedCandidate],
    targets: &HashMap<Key, KeyResolvedTarget>,
) -> ApiResult<(Vec<SharedListItem>, Vec<Key>)> {
    let ordinary = tree::shared_items(conn, principal, ctx.now, candidates, None).await?;
    let ordinary_keys: Vec<_> = ordinary.iter().map(key).collect();
    let resolved = source_objects(&ordinary, targets)?.targets(conn).await?;
    let mut names = NameBounds {
        limited: true,
        ..NameBounds::default()
    };
    for (object, target) in &resolved {
        if object.kind.node().is_some()
            && plan.permits_binding(
                caller,
                1,
                KeyTargetKind::Object(object.kind),
                std::slice::from_ref(target),
            )?
        {
            names.nodes.insert(object.id);
        }
    }
    let nodes = ordinary
        .into_iter()
        .filter_map(|item| match item {
            SharedListItem::Node(node) => Some(*node),
            _ => None,
        })
        .collect();
    let node_projection = Projection::new(ctx, caller, "SharedList")?;
    let mut projected: Vec<_> = node_projection
        .project(conn, principal, nodes)
        .await?
        .into_iter()
        .map(|node| SharedListItem::Node(Box::new(node)))
        .collect();
    // Run the same complete document/folder producer with trusted optional-name
    // bounds. This does not replace its DocView or ordinary containing-place check.
    let documents: Vec<_> = candidates
        .iter()
        .filter(|row| row.kind != SharedKind::Node)
        .map(|row| row.id)
        .collect();
    projected.extend(
        tree::shared_documents(conn, principal, &documents, ctx.now, Some(&names))
            .await?
            .into_values(),
    );
    let mut complete = Vec::new();
    for item in projected {
        let sources = source_objects(std::slice::from_ref(&item), targets)?;
        let mut admitted = true;
        for object in sources.objects() {
            let target = resolved.get(&object).ok_or_else(|| {
                ApiError::internal("a shared projection introduced an uncaptured source")
            })?;
            if !plan.permits_binding(
                caller,
                1,
                KeyTargetKind::Object(object.kind),
                std::slice::from_ref(target),
            )? {
                admitted = false;
                break;
            }
        }
        if admitted {
            complete.push(item);
        }
    }
    let present: Vec<_> = complete.iter().map(key).collect();
    let excluded = ordinary_keys
        .into_iter()
        .filter(|key| !present.contains(key))
        .collect();
    Ok((complete, excluded))
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: PageQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, "SharedList")?;
        plan.shared()
            .compatible(caller)
            .map_err(admission::binding_error)?;
        let raw =
            authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now()))
                .await?;
        let mut before = query
            .cursor
            .as_deref()
            .map(|cursor| tree::shared_cursor(cursor, raw.user()))
            .transpose()?;
        let limit = usize::try_from(query.limit()).map_err(ApiError::internal)?;
        let mut selected = Vec::new();
        let mut selected_targets = HashMap::new();
        let mut selected_items = HashMap::new();
        loop {
            let mut scanned =
                tree::shared_candidates(conn, &raw, ctx.now, before, query.limit()).await?;
            // Each domain supplies its native limit+lookahead. Never advance past
            // an older item from the other domain before consuming the globally
            // preceding native batch: doing so would skip unqueried candidates.
            scanned.truncate(limit + 1);
            let Some(last) = scanned.last().copied() else {
                break;
            };
            before = Some(last);
            let targets = primary_targets(conn, &scanned).await?;
            let mut candidates = Vec::new();
            for row in scanned {
                let target = targets
                    .get(&(row.kind, row.id))
                    .ok_or_else(ApiError::not_found)?;
                if plan.permits_binding(
                    caller,
                    0,
                    target.object.kind,
                    std::slice::from_ref(target),
                )? {
                    candidates.push(row);
                }
            }
            if candidates.is_empty() {
                continue;
            }
            let mut partial = plan.clone();
            bind_primary(&mut partial, &candidates, &targets)?;
            let principal = partial.preparation_principal(ctx, conn, caller).await?;
            let (items, excluded) =
                bounded_items(ctx, conn, caller, &plan, &principal, &candidates, &targets).await?;
            selected_items.extend(items.into_iter().map(|item| (key(&item), item)));
            selected.extend(
                candidates
                    .into_iter()
                    .filter(|row| !excluded.contains(&(row.kind, row.id))),
            );
            selected_targets.extend(targets);
            if selected.len() > limit {
                break;
            }
        }
        let partial = selected.len() > limit;
        let witness = selected.get(limit).copied();
        selected.truncate(limit);
        let next_cursor = partial
            .then(|| {
                selected
                    .last()
                    .map(|last| tree::shared_cursor_text(raw.user(), *last))
            })
            .flatten();
        let items: Vec<_> = selected
            .iter()
            .filter_map(|row| selected_items.remove(&(row.kind, row.id)))
            .collect();
        let mut footprint = selected;
        footprint.extend(witness);
        bind_primary(&mut plan, &footprint, &selected_targets)?;
        let sources = source_objects(&items, &selected_targets)?
            .targets(conn)
            .await?;
        projection::bind(
            &mut plan,
            1,
            projection::groups(
                sources
                    .into_iter()
                    .map(|(object, target)| (KeyTargetKind::Object(object.kind), target)),
            ),
        )?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Prepared {
            response,
            reply: SharedListReply {
                items,
                partial,
                next_cursor,
            },
        })))
    })
}

pub(super) async fn read(ctx: Ctx, query: PageQuery) -> ApiResult<SharedListReply> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let reply = if modern(principal.caller()) {
        let prepared = ctx.operation_data::<Prepared>()?;
        projection::disclose_read(
            &ctx,
            &mut conn,
            principal.caller(),
            prepared.response.clone(),
            prepared.reply.clone(),
            "the shared list is no longer allowed",
        )
        .await?
    } else {
        tree::shared_in(&mut conn, &principal, ctx.now, &query).await?
    };
    conn.commit().await.map_err(DbError::from)?;
    Ok(reply)
}
