//! Mixed native tree windows retain the real directory, selected rows and actual named origins.
use super::metadata;
use crate::key_operations::native;
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture};
use crate::routes::access::page::NameBounds;
use crate::routes::documents::{build, read_operation};
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::document_sources::{self as sources, TreeSource as Source};
use orch_api_contract::document_tree::{
    DocumentDirectory, DocumentTreeListQuery, DocumentTreePage,
};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::{Caller, Policy, Principal, engine::Graph};
use orch_db::{
    Conn,
    queries::documents::{self as documents, containers},
};
use orch_engine::document_tree as domain;
use orch_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::sync::Arc;
const DIRECTORY: KeyTargetKind = KeyTargetKind::Object(ObjectKind::DocumentContainer);
pub(super) struct Ready {
    response: OperationPlan,
    page: metadata::PageReady,
    names: NameBounds,
    at: DocumentDirectory,
    query: DocumentTreeListQuery,
    selected: Vec<containers::EntryRow>,
    nodes: BTreeSet<Id>,
    containers: BTreeSet<Id>,
}
fn declaration(query: &DocumentTreeListQuery) -> &'static Declaration<Source> {
    if query.deleted.is_some() {
        &sources::TREE_LIST.inspect
    } else {
        &sources::TREE_LIST.read
    }
}
fn refused() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::AuthzDenied,
        "the original native document tree page is no longer readable",
    )
}

fn protected_refusal(error: ApiError) -> ApiError {
    if matches!(error.problem.status, 401 | 403 | 404 | 410) {
        error.without_content()
    } else {
        error
    }
}
async fn directory_target(conn: &mut Conn, at: DocumentDirectory) -> ApiResult<KeyResolvedTarget> {
    match at {
        DocumentDirectory::NodeDocs(id) => {
            crate::key_operations::targets::node_identity(conn, id).await
        }
        DocumentDirectory::Container(id) => read_operation::identities(conn, &[id])
            .await?
            .remove(&id)
            .filter(|target| target.object.kind == DIRECTORY)
            .ok_or_else(ApiError::not_found),
    }
}
fn item(row: &containers::EntryRow) -> domain::Item {
    if row.kind == "container" {
        domain::Item::Container(row.id)
    } else {
        domain::Item::Document(row.id)
    }
}
async fn row_facts(
    conn: &mut Conn,
    rows: &[containers::EntryRow],
) -> ApiResult<native::Facts<Source>> {
    let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
    let mut targets = read_operation::identities(conn, &ids).await?;
    let mut groups = BTreeMap::<KeyTargetKind, Vec<KeyResolvedTarget>>::new();
    for row in rows {
        let kind = if row.kind == "container" {
            DIRECTORY
        } else {
            KeyTargetKind::Object(ObjectKind::Document)
        };
        let target = targets
            .remove(&row.id)
            .filter(|target| target.object.kind == kind)
            .ok_or_else(ApiError::not_found)?;
        groups.entry(kind).or_default().push(target);
    }
    Ok(native::Facts::collection(
        Source::Entries,
        groups.into_iter().collect(),
    ))
}
async fn name_facts(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: &DocumentTreeListQuery,
    at: DocumentDirectory,
    rows: &[containers::EntryRow],
    nodes: &BTreeSet<Id>,
    folders: &BTreeSet<Id>,
) -> ApiResult<(OperationPlan, NameBounds)> {
    let declaration = declaration(query);
    let mut facts = vec![
        native::Facts::one(
            Source::Directory,
            DIRECTORY,
            directory_target(conn, at).await?,
        ),
        row_facts(conn, rows).await?,
    ];
    let selection = native::bind_known(ctx, declaration, &facts)?;
    let graph = Graph::load(conn, &nodes.iter().copied().collect::<Vec<_>>())
        .await
        .map_err(ApiError::internal)?;
    let (node_facts, allowed) = native::node_name_facts(
        &selection,
        declaration,
        caller,
        &graph,
        nodes,
        &[
            (ObjectKind::Org, Source::Orgs),
            (ObjectKind::Project, Source::Projects),
            (ObjectKind::Topic, Source::Topics),
            (ObjectKind::Thread, Source::Threads),
        ],
    )?;
    facts.extend(node_facts);
    let ids: Vec<_> = folders.iter().copied().collect();
    let mut found = read_operation::identities(conn, &ids).await?;
    let mut selected = Vec::new();
    let mut names = NameBounds {
        limited: native::modern(caller),
        nodes: allowed,
        ..Default::default()
    };
    for id in ids {
        let target = found
            .remove(&id)
            .filter(|target| target.object.kind == DIRECTORY)
            .ok_or_else(ApiError::not_found)?;
        if native::modern(caller)
            && !native::permits_usage(
                &selection,
                declaration,
                caller,
                &Source::Containers,
                orch_api_contract::operations::FacetUsage::ResponseContent,
                DIRECTORY,
                &target,
            )?
        {
            continue;
        }
        names.containers.insert(id);
        selected.push(target);
    }
    facts.push(native::Facts::collection(
        Source::Containers,
        if selected.is_empty() {
            Vec::new()
        } else {
            vec![(DIRECTORY, selected)]
        },
    ));
    Ok((native::bind(ctx, declaration, &facts)?, names))
}
async fn native_nodes(
    conn: &mut Conn,
    principal: &Principal,
    ready: &metadata::PageReady,
) -> ApiResult<(
    BTreeSet<Id>,
    Vec<(documents::DocumentRow, orch_authz::documents::Listed)>,
)> {
    let node = ready.start.directory.node;
    let graph = Graph::load(conn, &[node])
        .await
        .map_err(ApiError::internal)?;
    let mut wanted: BTreeSet<Id> = graph
        .get(node)
        .ok_or_else(ApiError::not_found)?
        .path
        .iter()
        .copied()
        .collect();
    let ids: Vec<_> = ready
        .selected
        .iter()
        .filter(|row| row.kind == "document" && row.deleted_at.is_none())
        .map(|row| row.id)
        .collect();
    let mut listed: std::collections::HashMap<_, _> =
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
        .filter_map(|row| listed.remove(&row.id).map(|listed| (row, listed)))
        .collect();
    for source in build::ProjectedDocumentRef::candidates_many(conn, &entries)
        .await?
        .into_values()
        .flatten()
    {
        if let build::WrittenLabelSource::Node(id) = source {
            wanted.insert(id);
        } else {
            return Err(ApiError::internal(
                "the native tree metadata has an undeclared content source",
            ));
        }
    }
    let retained: Vec<_> = ready
        .start
        .directory
        .folder
        .as_ref()
        .is_some_and(|folder| {
            folder
                .entries
                .iter()
                .any(|entry| entry.entry.deleted_at.is_some())
        })
        .then_some(node)
        .into_iter()
        .collect();
    let refs = orch_db::queries::trace::read_scope(
        &retained,
        crate::routes::nodes::view::refs(
            conn,
            principal,
            &wanted.iter().copied().collect::<Vec<_>>(),
            Policy::default(),
        ),
    )
    .await?;
    wanted.retain(|id| {
        refs.get(id)
            .is_some_and(|value| value.name.is_some() || value.thread_display.is_some())
    });
    Ok((wanted, entries))
}
async fn select(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    query: DocumentTreeListQuery,
) -> ApiResult<Ready> {
    let node = ctx.id("id")?;
    let at = query.parent_container_id.map_or(
        DocumentDirectory::NodeDocs(node),
        DocumentDirectory::Container,
    );
    let declaration = declaration(&query);
    let initial = native::bind_known(
        ctx,
        declaration,
        &[native::Facts::one(
            Source::Directory,
            DIRECTORY,
            directory_target(conn, at).await?,
        )],
    )?;
    let principal = native::preparation_principal(ctx, conn, caller, &initial).await?;
    let start = metadata::page_start(ctx, conn, &principal, Some(node), at, &query).await?;
    let mut allowed = HashSet::new();
    let ids: Vec<_> = start.raw.iter().map(|row| row.id).collect();
    let targets = read_operation::identities(conn, &ids).await?;
    for row in &start.raw {
        let target = targets.get(&row.id).ok_or_else(ApiError::not_found)?;
        let kind = if row.kind == "container" {
            DIRECTORY
        } else {
            KeyTargetKind::Object(ObjectKind::Document)
        };
        if target.object.kind != kind {
            return Err(ApiError::not_found());
        }
        if !native::modern(caller)
            || native::permits(
                &initial,
                declaration,
                caller,
                &Source::Entries,
                kind,
                target,
            )?
        {
            allowed.insert(item(row));
        }
    }
    let page = metadata::page_selected(ctx, conn, &principal, start, Some(&allowed)).await?;
    let (wanted, references) = native_nodes(conn, &principal, &page).await?;
    let mut folders: BTreeSet<_> = page
        .read
        .ancestors
        .iter()
        .filter(|row| row.title.is_some() || row.basename.is_some())
        .map(|row| row.id)
        .collect();
    if let Some(read) = &page.start.directory.folder {
        folders.extend(
            read.ancestors
                .iter()
                .filter(|row| row.title.is_some() || row.basename.is_some())
                .map(|row| row.id),
        );
    }
    let mut intrinsic: BTreeSet<_> = page
        .selected
        .iter()
        .filter(|row| row.kind == "container")
        .map(|row| row.id)
        .collect();
    if let DocumentDirectory::Container(id) = at {
        intrinsic.insert(id);
    }
    folders.retain(|id| !intrinsic.contains(id));
    let (_, mut bounds) = name_facts(
        ctx,
        conn,
        caller,
        &query,
        at,
        &page.selected,
        &wanted,
        &folders,
    )
    .await?;
    bounds.containers.extend(intrinsic);
    let allowed: Vec<_> = bounds
        .nodes
        .iter()
        .copied()
        .map(build::WrittenLabelSource::Node)
        .collect();
    let (_, actual) = build::ProjectedDocumentRef::capture_many_allowed(
        conn,
        &principal,
        &references,
        principal.now().get(),
        Policy::default(),
        &allowed,
    )
    .await?;
    // Record only path names exposed by the existing native directory supplier.
    let graph = Graph::load(conn, &[page.start.directory.node])
        .await
        .map_err(ApiError::internal)?;
    let path: BTreeSet<_> = graph
        .get(page.start.directory.node)
        .ok_or_else(ApiError::not_found)?
        .path
        .iter()
        .copied()
        .collect();
    let mut nodes: BTreeSet<_> = bounds.nodes.intersection(&path).copied().collect();
    for source in actual {
        if let build::WrittenLabelSource::Node(id) = source {
            nodes.insert(id);
        } else {
            return Err(ApiError::internal(
                "the native tree returned an undeclared reference source",
            ));
        }
    }
    let containers: BTreeSet<_> = folders.intersection(&bounds.containers).copied().collect();
    bounds.nodes = nodes.clone();
    let (response, _) = name_facts(
        ctx,
        conn,
        caller,
        &query,
        at,
        &page.selected,
        &nodes,
        &containers,
    )
    .await?;
    Ok(Ready {
        response,
        selected: page.selected.clone(),
        page,
        names: bounds,
        at,
        query,
        nodes,
        containers,
    })
}
pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentTreeListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !native::modern(caller) {
            return Ok(None);
        }
        let ready = select(ctx, conn, caller, query)
            .await
            .map_err(protected_refusal)?;
        Ok(Some(ready.response.clone().with_data(ready)))
    })
}
pub(super) async fn check_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ready: &Ready,
) -> ApiResult<()> {
    let checked = check_current_sources(ctx, conn, principal, ready).await;
    if native::modern(principal.caller()) {
        checked.map_err(protected_refusal)
    } else {
        checked
    }
}

async fn check_current_sources(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    ready: &Ready,
) -> ApiResult<()> {
    let (plan, bounds) = name_facts(
        ctx,
        conn,
        principal.caller(),
        &ready.query,
        ready.at,
        &ready.selected,
        &ready.nodes,
        &ready.containers,
    )
    .await?;
    if !plan.shared().covers_sources(ready.response.shared())
        || !ready.nodes.is_subset(&bounds.nodes)
        || !ready.containers.is_subset(&bounds.containers)
    {
        return Err(refused());
    }
    if native::modern(principal.caller()) {
        plan.clone().admit(principal.caller())?;
    }
    let reader = native::preparation_principal(ctx, conn, principal.caller(), &plan).await?;
    let directory =
        metadata::prepare_directory(conn, &reader, ready.at, ready.query.deleted.is_some()).await?;
    match metadata::unchanged(conn, &reader, &ready.page.start.directory.cut, reader.now()).await {
        Ok(()) => {}
        Err(error)
            if error.code() == orch_api_contract::common::ErrorCode::ConflictStaleRevision =>
        {
            return Err(refused());
        }
        Err(error) => return Err(error),
    }
    let mut rows = Vec::new();
    for old in &ready.selected {
        let row = containers::entry(conn, &old.kind, old.id)
            .await?
            .ok_or_else(refused)?;
        if row.node_id != ready.page.start.directory.cut.node
            || row.deleted_at.is_some() != ready.query.deleted.is_some()
            || row.parent_container_id != old.parent_container_id
            || row.deleted_at != old.deleted_at
            || row.deleted_in != old.deleted_in
        {
            return Err(refused());
        }
        if ready.query.deleted.is_none() {
            let parent = match ready.at {
                DocumentDirectory::NodeDocs(_) => None,
                DocumentDirectory::Container(id) => Some(id),
            };
            if row.parent_container_id != parent {
                return Err(refused());
            }
        }
        rows.push(row);
    }
    if let DocumentDirectory::Container(parent) = ready.at
        && ready.query.deleted.is_some()
    {
        let ids: Vec<_> = rows.iter().map(|row| row.id).collect();
        let ancestors: BTreeMap<_, _> = containers::ancestors_many(conn, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row.parent_container_id))
            .collect();
        for row in &rows {
            let mut seen = HashSet::new();
            let mut next = row.parent_container_id;
            while let Some(id) = next {
                if id == parent {
                    break;
                }
                if !seen.insert(id) {
                    return Err(refused());
                }
                next = ancestors.get(&id).copied().flatten();
            }
            if next != Some(parent) {
                return Err(refused());
            }
        }
    }
    let read = domain::project_entries_in(
        conn,
        domain::Actor::Person(&reader),
        rows,
        reader.now(),
        ready.query.deleted.is_some(),
    )
    .await;
    let read = super::result(conn, read).await?;
    let available = metadata::available_entries(conn, &read.entries, super::options(ctx)).await?;
    if ready
        .selected
        .iter()
        .any(|row| !available.contains_key(&item(row)))
    {
        return Err(refused());
    }
    let mut named: BTreeSet<_> = read
        .ancestors
        .iter()
        .filter(|row| row.title.is_some() || row.basename.is_some())
        .map(|row| row.id)
        .collect();
    if let Some(folder) = &directory.folder {
        named.extend(
            folder
                .ancestors
                .iter()
                .filter(|row| row.title.is_some() || row.basename.is_some())
                .map(|row| row.id),
        );
    }
    if !ready.containers.is_subset(&named) {
        return Err(refused());
    }
    let retained: Vec<_> = directory
        .folder
        .as_ref()
        .is_some_and(|folder| {
            folder
                .entries
                .iter()
                .any(|entry| entry.entry.deleted_at.is_some())
        })
        .then_some(directory.node)
        .into_iter()
        .collect();
    let refs = orch_db::queries::trace::read_scope(
        &retained,
        crate::routes::nodes::view::refs(
            conn,
            &reader,
            &ready.nodes.iter().copied().collect::<Vec<_>>(),
            Policy::default(),
        ),
    )
    .await?;
    if ready.nodes.iter().any(|id| {
        !refs
            .get(id)
            .is_some_and(|value| value.name.is_some() || value.thread_display.is_some())
    }) {
        return Err(refused());
    }
    Ok(())
}
pub(super) async fn get(
    ctx: Ctx,
    query: DocumentTreeListQuery,
    (): (),
) -> ApiResult<DocumentTreePage> {
    // The existing signed cursor key is installed before any actual read snapshot.
    crate::routes::resources::sealing::master_key()?;
    let (projection, actual, ready) = native::read(ctx, move |ctx, conn, principal| {
        let query = query.clone();
        Box::pin(async move {
            let ready = if native::modern(principal.caller()) {
                ctx.operation_data::<Ready>()?
            } else {
                Arc::new(select(ctx, conn, principal.caller(), query).await?)
            };
            let projection = if native::modern(principal.caller()) {
                let source = Arc::clone(&ready);
                let projection = ctx.clone();
                ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            check_sources(&projection, conn, reader, &source).await?;
                            metadata::page_ready(
                                &projection,
                                conn,
                                reader,
                                &source.page,
                                Some(&source.names),
                            )
                            .await
                        })
                    })
                    .await?
                    .ok_or_else(refused)?
            } else {
                check_sources(ctx, conn, principal, &ready).await?;
                metadata::page_ready(ctx, conn, principal, &ready.page, None).await?
            };
            Ok((projection, ctx.clone(), ready))
        })
    })
    .await?;
    super::finish_read(
        &actual,
        &projection,
        Some(super::ReadSource::TreePage(&ready)),
    )
    .await?;
    let accepted = match actual.admitted_caller() {
        Some(caller) => caller.clone(),
        None => authorization::caller_of(actual.user()?)?,
    };
    crate::key_operations::publication::retain(
        &actual,
        &accepted,
        ready,
        |ctx, conn, reader, ready| Box::pin(check_sources(ctx, conn, reader, ready)),
    )?;
    Ok(projection.value)
}
