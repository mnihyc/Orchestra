//! Native typed entry sources shared by document and folder metadata reads.

use super::convert;

use super::metadata;

use crate::key_operations::native;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use crate::routes::access::page::NameBounds;

use crate::routes::documents::build;

use crate::routes::documents::read_operation;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::document_sources as sources;

use orch_api_contract::document_sources::EntryRead;

use orch_api_contract::document_sources::EntrySource;

use orch_api_contract::document_tree::DocumentTreeEntryGetQuery;

use orch_api_contract::document_tree::DocumentTreeItem;

use orch_api_contract::document_tree::DocumentTreeKind;

use orch_api_contract::document_tree::DocumentTreeRead;

use orch_api_contract::nodes::DeletedInspectionQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::engine::Graph;

use orch_db::Conn;

use orch_db::queries::documents::containers;

use orch_engine::document_tree as domain;

use orch_engine::operation_scope::targets as shared;

use orch_types::Id;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::sync::Arc;
pub(super) struct Ready {
    response: OperationPlan,
    item: DocumentTreeItem,
    names: NameBounds,
    declaration: &'static Declaration<EntrySource>,
}

fn intent(family: &'static EntryRead, inspect: bool) -> &'static Declaration<EntrySource> {
    if inspect {
        &family.inspect
    } else {
        &family.read
    }
}

fn node_source(kind: KeyTargetKind) -> ApiResult<EntrySource> {
    match kind {
        KeyTargetKind::Object(ObjectKind::Org) => Ok(EntrySource::Orgs),
        KeyTargetKind::Object(ObjectKind::Project) => Ok(EntrySource::Projects),
        KeyTargetKind::Object(ObjectKind::Topic) => Ok(EntrySource::Topics),
        KeyTargetKind::Object(ObjectKind::Thread) => Ok(EntrySource::Threads),
        _ => Err(ApiError::internal(
            "the native entry has an undeclared node source",
        )),
    }
}

async fn bound_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    declaration: &'static Declaration<EntrySource>,
    item: DocumentTreeItem,
    node_ids: &BTreeSet<Id>,
    container_ids: &BTreeSet<Id>,
    select: Option<&Caller>,
) -> ApiResult<(OperationPlan, NameBounds)> {
    let id = match item {
        DocumentTreeItem::Document(id) | DocumentTreeItem::Container(id) => id,
    };
    let mut ids: Vec<_> = container_ids.iter().copied().collect();
    ids.push(id);
    let mut documents = read_operation::identities(conn, &ids).await?;
    let primary = documents.remove(&id).ok_or_else(ApiError::not_found)?;
    let mut facts = vec![native::Facts::one(
        EntrySource::Entry,
        primary.object.kind,
        primary,
    )];
    let selection = native::bind_known(ctx, declaration, &facts)?;
    let mut nodes: HashMap<_, Vec<KeyResolvedTarget>> = HashMap::new();
    let mut names = NameBounds {
        limited: true,
        ..Default::default()
    };
    let graph = Graph::load(conn, &node_ids.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    for id in node_ids {
        if graph.get(*id).is_none() && select.is_some() {
            continue;
        }
        let target = shared::from_node_identity(&graph, *id).ok_or_else(ApiError::not_found)?;
        let source = node_source(target.object.kind)?;
        if let Some(caller) = select.filter(|caller| native::modern(caller))
            && !native::permits(
                &selection,
                declaration,
                caller,
                &source,
                target.object.kind,
                &target,
            )?
        {
            continue;
        }
        names.nodes.insert(*id);
        nodes.entry(target.object.kind).or_default().push(target);
    }
    for kind in [
        ObjectKind::Org,
        ObjectKind::Project,
        ObjectKind::Topic,
        ObjectKind::Thread,
    ] {
        let family = KeyTargetKind::Object(kind);
        let groups = nodes
            .remove(&family)
            .map(|targets| vec![(family, targets)])
            .unwrap_or_default();
        facts.push(native::Facts::collection(node_source(family)?, groups));
    }
    let family = KeyTargetKind::Object(ObjectKind::DocumentContainer);
    let mut selected = Vec::new();
    for id in container_ids {
        let target = documents.remove(id).ok_or_else(ApiError::not_found)?;
        if target.object.kind != family {
            return Err(ApiError::not_found());
        }
        if let Some(caller) = select.filter(|caller| native::modern(caller))
            && !native::permits(
                &selection,
                declaration,
                caller,
                &EntrySource::Containers,
                family,
                &target,
            )?
        {
            continue;
        }
        names.containers.insert(*id);
        selected.push(target);
    }
    facts.push(native::Facts::collection(
        EntrySource::Containers,
        if selected.is_empty() {
            Vec::new()
        } else {
            vec![(family, selected)]
        },
    ));
    Ok((native::bind(ctx, declaration, &facts)?, names))
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    item: DocumentTreeItem,
    declaration: &'static Declaration<EntrySource>,
) -> ApiResult<Option<OperationPlan>> {
    let native_item: domain::Item = convert(item)?;
    let kind = match native_item {
        domain::Item::Document(_) => "document",
        domain::Item::Container(_) => "container",
    };
    let row = containers::entry(conn, kind, native_item.id())
        .await?
        .ok_or_else(ApiError::not_found)?;
    let primary = read_operation::identities(conn, &[row.id])
        .await?
        .remove(&row.id)
        .ok_or_else(ApiError::not_found)?;
    let known = native::bind_known(
        ctx,
        declaration,
        &[native::Facts::one(
            EntrySource::Entry,
            primary.object.kind,
            primary,
        )],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
    let read = metadata::read_item(
        conn,
        &principal,
        native_item,
        principal.now(),
        ctx.inspect_deleted,
    )
    .await?;
    let mut node_ids = BTreeSet::new();
    let graph = Graph::load(conn, &[row.node_id])
        .await
        .map_err(ApiError::internal)?;
    node_ids.extend(
        graph
            .get(row.node_id)
            .ok_or_else(ApiError::not_found)?
            .path
            .iter()
            .copied(),
    );
    let path_ids = node_ids.clone();
    let container_ids = read
        .ancestors
        .iter()
        .filter(|ancestor| ancestor.title.is_some() || ancestor.basename.is_some())
        .map(|ancestor| ancestor.id)
        .filter(|id| *id != row.id)
        .collect();
    let mut reference = None;
    if matches!(native_item, domain::Item::Document(_)) && row.deleted_at.is_none() {
        let document = orch_db::queries::documents::get(conn, row.id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let listed = build::standing(conn, &principal, row.id, Policy::default()).await?;
        node_ids.extend(
            build::ProjectedDocumentRef::candidates(conn, &document, &listed)
                .await?
                .into_iter()
                .filter_map(|source| match source {
                    build::WrittenLabelSource::Node(id) => Some(id),
                    _ => None,
                }),
        );
        reference = Some((document, listed));
    }
    let retained: Vec<_> = ctx
        .inspect_deleted
        .then_some(row.node_id)
        .into_iter()
        .collect();
    let readable = orch_db::queries::trace::read_scope(
        &retained,
        crate::routes::nodes::view::refs(
            conn,
            &principal,
            &node_ids.iter().copied().collect::<Vec<_>>(),
            Policy::default(),
        ),
    )
    .await?;
    node_ids.retain(|id| {
        readable
            .get(id)
            .is_some_and(|node| node.name.is_some() || node.thread_display.is_some())
    });
    let (_, names) = bound_sources(
        ctx,
        conn,
        declaration,
        item,
        &node_ids,
        &container_ids,
        Some(caller),
    )
    .await?;
    let mut actual_nodes: BTreeSet<_> = path_ids.intersection(&names.nodes).copied().collect();
    if let Some((document, listed)) = reference {
        let allowed: Vec<_> = names
            .nodes
            .iter()
            .copied()
            .map(build::WrittenLabelSource::Node)
            .collect();
        let projected = build::ProjectedDocumentRef::capture_allowed(
            conn,
            &principal,
            &document,
            &listed,
            principal.now().get(),
            Policy::default(),
            &allowed,
        )
        .await?;
        actual_nodes.extend(
            projected
                .sources()
                .iter()
                .filter_map(|source| match source {
                    build::WrittenLabelSource::Node(id) => Some(*id),
                    _ => None,
                }),
        );
    }
    let (response, names) = bound_sources(
        ctx,
        conn,
        declaration,
        item,
        &actual_nodes,
        &names.containers,
        None,
    )
    .await?;
    let plan = response.clone();
    Ok(Some(plan.with_data(Ready {
        response,
        item,
        names,
        declaration,
    })))
}

pub(super) fn container<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: DeletedInspectionQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        prepare(
            ctx,
            conn,
            caller,
            DocumentTreeItem::Container(ctx.id("id")?),
            intent(&sources::CONTAINER_GET, ctx.inspect_deleted),
        )
        .await
    })
}

pub(super) fn entry<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentTreeEntryGetQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        let item = match query.kind {
            DocumentTreeKind::Document => DocumentTreeItem::Document(ctx.id("id")?),
            DocumentTreeKind::Container => DocumentTreeItem::Container(ctx.id("id")?),
        };
        prepare(
            ctx,
            conn,
            caller,
            item,
            intent(&sources::ENTRY_GET, ctx.inspect_deleted),
        )
        .await
    })
}

async fn read(ctx: Ctx, item: DocumentTreeItem) -> ApiResult<DocumentTreeRead> {
    let (ctx, projection, ready) = native::read(ctx, move |ctx, conn, principal| {
        Box::pin(async move {
            if native::modern(principal.caller()) {
                let ready = ctx.operation_data::<Ready>()?;
                let render = Arc::clone(&ready);
                let mut producing = ctx.clone();
                let projection = ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        producing.now = reader.now().get();
                        Box::pin(async move {
                            metadata::entry_bounded(
                                &producing,
                                conn,
                                reader,
                                render.item,
                                producing.inspect_deleted,
                                Some(&render.names),
                            )
                            .await
                        })
                    })
                    .await?
                    .ok_or_else(|| {
                        ApiError::new(
                            orch_api_contract::common::ErrorCode::AuthzDenied,
                            "the native entry read was not admitted",
                        )
                    })?;
                Ok((ctx.clone(), projection, Some(ready)))
            } else {
                let projection =
                    metadata::entry(ctx, conn, principal, item, ctx.inspect_deleted).await?;
                Ok((
                    ctx.clone(),
                    projection,
                    Some(ctx.operation_data::<Ready>()?),
                ))
            }
        })
    })
    .await?;
    super::finish_read_scoped(&ctx, &projection, ready.as_deref()).await?;
    if let Some(ready) = ready {
        let accepted = ctx.admitted_caller().cloned().ok_or_else(|| {
            ApiError::internal("the native entry response lost its original caller")
        })?;
        crate::key_operations::publication::retain(
            &ctx,
            &accepted,
            (ready, projection.cut.clone()),
            |ctx, conn, reader, (ready, cut)| {
                Box::pin(async move {
                    let plan = source_plan(ctx, conn, ready).await?;
                    if native::modern(reader.caller()) {
                        plan.clone().admit(reader.caller())?;
                    }
                    let current =
                        native::preparation_principal(ctx, conn, reader.caller(), &plan).await?;
                    metadata::read_item(
                        conn,
                        &current,
                        convert(ready.item)?,
                        current.now(),
                        ctx.inspect_deleted,
                    )
                    .await?;
                    metadata::unchanged(conn, &current, cut, current.now()).await
                })
            },
        )?;
    }
    Ok(projection.value)
}

pub(super) async fn container_get(
    ctx: Ctx,
    _query: DeletedInspectionQuery,
    (): (),
) -> ApiResult<DocumentTreeRead> {
    let item = DocumentTreeItem::Container(ctx.id("id")?);
    read(ctx, item).await
}

pub(super) async fn entry_get(
    ctx: Ctx,
    query: DocumentTreeEntryGetQuery,
    (): (),
) -> ApiResult<DocumentTreeRead> {
    let item = match query.kind {
        DocumentTreeKind::Document => DocumentTreeItem::Document(ctx.id("id")?),
        DocumentTreeKind::Container => DocumentTreeItem::Container(ctx.id("id")?),
    };
    read(ctx, item).await
}

pub(super) async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    ready: &Ready,
) -> ApiResult<()> {
    let plan = source_plan(ctx, conn, ready).await?;
    plan.admit(caller)?;
    Ok(())
}

async fn source_plan(ctx: &Ctx, conn: &mut Conn, ready: &Ready) -> ApiResult<OperationPlan> {
    let (plan, _) = bound_sources(
        ctx,
        conn,
        ready.declaration,
        ready.item,
        &ready.names.nodes,
        &ready.names.containers,
        None,
    )
    .await?;
    Ok(plan)
}
