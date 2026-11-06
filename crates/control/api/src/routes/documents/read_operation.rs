//! Current document reads and existing feed projections share one scope/source preparation.

use super::{DocumentRow, SyncRows, build, discussions, rows};
use crate::key_operations::{admission::binding_error, targets};
use crate::middleware::{
    authorization, inspection,
    requests::{ApiError, ApiResult},
};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use crate::routes::access::{
    operations::{NameRoles, projection_names},
    page::{NameBounds, NameSources},
};
use build::WrittenLabelSource;
use orch_access_contract::key_scope::{
    KeyIdentityKind, KeyObjectRef, KeyResolvedTarget, KeyTargetKind,
};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::documents::{
    Document, DocumentRef, DocumentSync, DocumentSyncGet, DocumentSyncGetQuery,
};
use orch_api_contract::nodes::DeletedInspectionQuery;
use orch_api_contract::operations::TargetResolver;
use orch_api_contract::routes::{Endpoint, Route};
use orch_authz::{Caller, Policy, Principal, engine::Graph};
use orch_db::Conn;
use orch_types::Id;
use std::collections::BTreeSet;

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "document_names_orgs"),
        (ObjectKind::Project, "document_names_projects"),
        (ObjectKind::Topic, "document_names_topics"),
        (ObjectKind::Thread, "document_names_threads"),
    ],
    containers: Some("document_names_containers"),
};
const DOCUMENTS: &str = "document_reference_documents";
const CHATS: &str = "document_reference_chats";

pub(super) struct PreparedRead {
    row: DocumentRow,
    listed: orch_authz::documents::Listed,
    blocks: Vec<rows::BlockRow>,
    discussions: Vec<discussions::Full>,
    since: Option<i64>,
    complete: bool,
    names: NameBounds,
    sources: Vec<WrittenLabelSource>,
    inspection: Option<inspection::Info>,
    cut: crate::routes::document_tree::metadata::Cut,
}

/// Factual retained identity only; the ordinary View/restoration producer still decides admission.
pub(super) async fn identity(conn: &mut Conn, id: Id) -> ApiResult<KeyResolvedTarget> {
    let mut graph = Graph::default();
    let row = orch_authz::documents::read_into(conn, &mut graph, &[id])
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .find(|row| row.kind == ObjectKind::Document)
        .ok_or_else(ApiError::not_found)?;
    // Placement is factual even when a parent was deleted. The existing restoration
    // producer below must still authorize retained-content inspection separately.
    let mut target = orch_engine::operation_scope::targets::from_node_identity(&graph, row.node)
        .ok_or_else(ApiError::not_found)?;
    target.object = KeyObjectRef {
        kind: KeyTargetKind::Object(ObjectKind::Document),
        id: Some(id),
    };
    target.document_ancestors = graph
        .document_ancestors(id)
        .into_iter()
        .map(|ancestor| {
            graph
                .is_document_container(ancestor)
                .then_some(KeyObjectRef {
                    kind: KeyTargetKind::Object(ObjectKind::DocumentContainer),
                    id: Some(ancestor),
                })
        })
        .collect::<Option<Vec<_>>>()
        .ok_or_else(ApiError::not_found)?;
    Ok(target)
}

async fn base(conn: &mut Conn, route: Route, id: Id, inspect: bool) -> ApiResult<OperationPlan> {
    let selected = format!(
        "{}.{}",
        route.name,
        if inspect { "inspect_deleted" } else { "read" }
    );
    let mut plan = OperationPlan::new(route, &selected)?;
    let target = identity(conn, id).await?;
    plan.bind(0, KeyTargetKind::Object(ObjectKind::Document), &[target])?;
    Ok(plan)
}

/// Subscription publishes no rows and never selects retained-content inspection.
pub(crate) async fn root_plan(conn: &mut Conn, id: Id) -> ApiResult<OperationPlan> {
    let mut plan = base(conn, DocumentSyncGet::ROUTE, id, false).await?;
    for index in 1..plan.variant().facets.len() {
        plan.bind_empty(index)?;
    }
    Ok(plan)
}

/// Current ordinary authority and the actual primary document operation, before rows project.
pub(super) struct ReadRoot {
    pub(super) plan: OperationPlan,
    pub(super) principal: Principal,
    pub(super) inspection: Option<inspection::Info>,
}

pub(super) async fn prepare_root(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<ReadRoot> {
    let plan = base(conn, ctx.route, ctx.id("id")?, ctx.inspect_deleted).await?;
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let inspection = inspection::prepare(ctx, conn, &principal).await?;
    #[cfg(test)]
    crate::realtime::history_read_tests::document_authority_cut(ctx.id("id")?, principal.caller())
        .await;
    if inspection.is_none() {
        authorization::require_document(
            conn,
            &principal,
            ctx.id("id")?,
            Permission::DocView,
            Policy::default(),
        )
        .await?;
    }
    Ok(ReadRoot {
        plan,
        principal,
        inspection,
    })
}

pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: DeletedInspectionQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_http(ctx, conn, caller, false, None))
}
pub(super) fn sync<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: DocumentSyncGetQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare_http(ctx, conn, caller, true, query.since))
}

async fn prepare_http(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    sync: bool,
    since: Option<i64>,
) -> ApiResult<OperationPlan> {
    if since.is_some_and(|since| since < 0) {
        return Err(super::invalid("since", "must not be negative"));
    }
    let id = ctx.id("id")?;
    let ReadRoot {
        mut plan,
        principal,
        inspection,
    } = prepare_root(ctx, conn, caller).await?;
    let reads = inspection
        .as_ref()
        .map(|info| info.reads.clone())
        .unwrap_or_default();
    orch_db::queries::trace::read_scope(&reads, async {
        let row = super::live(conn, id).await?;
        let listed = build::standing(conn, &principal, id, Policy::default()).await?;
        let selected = if sync {
            super::sync_rows(conn, &row, since).await?
        } else {
            SyncRows {
                blocks: Vec::new(),
                discussions: Vec::new(),
                complete: true,
            }
        };
        let (names, sources) = prepare_sources(
            conn,
            &principal,
            &mut plan,
            &row,
            &listed,
            &selected.blocks,
            &selected.discussions,
            true,
        )
        .await?;
        let cut = crate::routes::document_tree::metadata::cut(
            conn,
            &principal,
            row.node_id,
            principal.now(),
        )
        .await?;
        Ok(plan.with_data(PreparedRead {
            row,
            listed,
            blocks: selected.blocks,
            discussions: selected.discussions,
            since,
            complete: selected.complete,
            names,
            sources,
            inspection,
            cut,
        }))
    })
    .await
}

fn slot(plan: &OperationPlan, role: &'static str) -> Option<usize> {
    plan.variant()
        .facets
        .iter()
        .position(|facet| facet.target == TargetResolver::ResolvedCollection(role))
}
fn bind(
    plan: &mut OperationPlan,
    role: &'static str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let Some(index) = slot(plan, role) else {
        return if targets.is_empty() {
            Ok(())
        } else {
            Err(ApiError::internal(
                "a document read has an undeclared reference source",
            ))
        };
    };
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}

/// Batched factual reference identities; ordinary Cast visibility decides whether any name is shown.
async fn reference_bounds(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    document: Id,
    candidates: &[WrittenLabelSource],
) -> ApiResult<Vec<WrittenLabelSource>> {
    let mut allowed = vec![WrittenLabelSource::Document(document)]; // the document's own title is already primary content
    let ids: Vec<_> = candidates
        .iter()
        .filter_map(|source| match source {
            WrittenLabelSource::Document(id) if *id != document => Some(*id),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let mut graph = Graph::default();
    let rows = orch_authz::documents::read_into(conn, &mut graph, &ids)
        .await
        .map_err(ApiError::internal)?;
    let mut docs = Vec::new();
    if let Some(index) = slot(plan, DOCUMENTS) {
        for row in rows
            .into_iter()
            .filter(|row| row.kind == ObjectKind::Document)
        {
            let Some(site) = orch_authz::documents::site_of(&graph, &row) else {
                continue;
            };
            let target = targets::from_site(&graph, &site)?;
            if plan.permits_binding(
                caller,
                index,
                KeyTargetKind::Object(ObjectKind::Document),
                std::slice::from_ref(&target),
            )? {
                allowed.push(WrittenLabelSource::Document(row.id));
                docs.push(target);
            }
        }
    }
    bind(
        plan,
        DOCUMENTS,
        KeyTargetKind::Object(ObjectKind::Document),
        &docs,
    )?;
    let ids: Vec<_> = candidates
        .iter()
        .filter_map(|source| match source {
            WrittenLabelSource::Chat(id) => Some(*id),
            _ => None,
        })
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let chats = orch_db::queries::chats::chats(conn, &ids).await?;
    let places: Vec<_> = chats
        .iter()
        .map(|chat| chat.node_id.unwrap_or(chat.org_id))
        .collect();
    let graph = Graph::load(conn, &places)
        .await
        .map_err(ApiError::internal)?;
    let mut found = Vec::new();
    if let Some(index) = slot(plan, CHATS) {
        for chat in chats {
            let Some(site) = graph.site_of(chat.node_id.unwrap_or(chat.org_id)) else {
                continue;
            };
            let mut target = targets::from_site(&graph, &site)?;
            target.object = KeyObjectRef {
                kind: KeyTargetKind::Identity(KeyIdentityKind::Chat),
                id: Some(chat.id),
            };
            if plan.permits_binding(
                caller,
                index,
                KeyTargetKind::Identity(KeyIdentityKind::Chat),
                std::slice::from_ref(&target),
            )? {
                allowed.push(WrittenLabelSource::Chat(chat.id));
                found.push(target);
            }
        }
    }
    bind(
        plan,
        CHATS,
        KeyTargetKind::Identity(KeyIdentityKind::Chat),
        &found,
    )?;
    Ok(allowed)
}

async fn prepare_sources(
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    row: &DocumentRow,
    listed: &orch_authz::documents::Listed,
    blocks: &[rows::BlockRow],
    discussions: &[discussions::Full],
    full: bool,
) -> ApiResult<(NameBounds, Vec<WrittenLabelSource>)> {
    plan.shared()
        .preparation_caller(principal.caller())
        .map_err(binding_error)?;
    let names = if full {
        build::document_name_sources(conn, row, listed).await?
    } else {
        NameSources::default()
    };
    let mut candidates = build::ProjectedBlocks::candidates(conn, blocks).await?;
    candidates.extend(build::discussion_sources(conn, discussions).await?);
    candidates.extend(build::ProjectedDocumentRef::candidates(conn, row, listed).await?);
    prepare_names(conn, principal, plan, row.id, names, candidates).await
}

fn name_roles(plan: &OperationPlan) -> NameRoles {
    NameRoles {
        // A reference-only projection never declares or reads a container label. An
        // undeclared nonempty source remains an error in the shared name binder.
        containers: slot(plan, "document_names_containers").map(|_| "document_names_containers"),
        ..NAMES
    }
}

/// Binds only actual returned references; no full-document metadata is loaded here.
pub(super) async fn prepare_references(
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    document: Id,
    candidates: Vec<WrittenLabelSource>,
) -> ApiResult<(NameBounds, Vec<WrittenLabelSource>)> {
    plan.shared()
        .preparation_caller(principal.caller())
        .map_err(binding_error)?;
    prepare_names(
        conn,
        principal,
        plan,
        document,
        NameSources::default(),
        candidates,
    )
    .await
}

async fn prepare_names(
    conn: &mut Conn,
    principal: &Principal,
    plan: &mut OperationPlan,
    document: Id,
    mut names: NameSources,
    mut candidates: Vec<WrittenLabelSource>,
) -> ApiResult<(NameBounds, Vec<WrittenLabelSource>)> {
    candidates.sort_unstable();
    candidates.dedup();
    names
        .nodes
        .extend(candidates.iter().filter_map(|source| match source {
            WrittenLabelSource::Node(id) => Some(*id),
            _ => None,
        }));
    let mut sources =
        reference_bounds(conn, principal.caller(), plan, document, &candidates).await?;
    let roles = name_roles(plan);
    let names = projection_names(conn, principal.caller(), plan, names, &roles).await?;
    sources.extend(names.nodes.iter().copied().map(WrittenLabelSource::Node));
    Ok((names, sources))
}

pub(super) async fn project_document(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedRead,
) -> ApiResult<Document> {
    build::document_bounded(
        conn,
        principal,
        &prepared.row,
        &prepared.listed,
        principal.now().get(),
        Policy::default(),
        Some(&prepared.names),
        Some(&prepared.sources),
    )
    .await
}
pub(super) async fn project_sync(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedRead,
) -> ApiResult<DocumentSync> {
    let document = project_document(conn, principal, prepared).await?;
    let (blocks, discussions) = project_parts(
        conn,
        principal,
        &prepared.row,
        &prepared.blocks,
        &prepared.discussions,
        &prepared.names,
        &prepared.sources,
    )
    .await?;
    Ok(DocumentSync {
        document,
        blocks,
        discussions,
        seq: prepared.row.doc_seq,
        since: prepared.since,
        complete: prepared.complete,
    })
}
async fn project_parts(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    blocks: &[rows::BlockRow],
    discussions: &[discussions::Full],
    names: &NameBounds,
    sources: &[WrittenLabelSource],
) -> ApiResult<(
    Vec<orch_api_contract::documents::DocumentBlock>,
    Vec<orch_api_contract::documents::DocumentDiscussion>,
)> {
    let blocks = if names.limited {
        build::ProjectedBlocks::capture_allowed(conn, principal, blocks, Policy::default(), sources)
            .await?
            .blocks
    } else {
        build::blocks_for(conn, principal, blocks, Policy::default()).await?
    };
    let discussions = build::discussions_bounded(
        conn,
        principal,
        row.id,
        discussions,
        Policy::default(),
        names.limited.then_some(sources),
    )
    .await?;
    Ok((blocks, discussions))
}

pub(super) fn same_inspection(ctx: &Ctx, prepared: &PreparedRead) -> ApiResult<()> {
    same_inspection_info(ctx, prepared.inspection.as_ref())
}

pub(super) fn same_inspection_info(
    ctx: &Ctx,
    prepared: Option<&inspection::Info>,
) -> ApiResult<()> {
    if ctx.inspection.as_ref().map(|info| (info.id, info.deletion))
        != prepared.map(|info| (info.id, info.deletion))
    {
        return Err(crate::routes::document_tree::metadata::stale());
    }
    Ok(())
}

/// Retain the existing post-snapshot metadata cut and recheck the held response's exact source bounds.
pub(super) async fn finish(ctx: &Ctx, prepared: &PreparedRead) -> ApiResult<()> {
    #[cfg(test)]
    crate::realtime::history_read_tests::document_response_cut(
        prepared.row.id,
        prepared.names.limited,
        prepared.names.nodes.iter().copied().collect(),
        prepared.sources.clone(),
    )
    .await;
    finish_sources(
        ctx,
        &prepared.cut,
        prepared.row.id,
        &prepared.names,
        &prepared.sources,
    )
    .await
}

/// An ordinary neutral projection has a metadata cut, without claiming typed source proof.
/// The same finisher owns its guard, freshness check and commit.
pub(super) async fn finish_cut(
    ctx: &Ctx,
    cut: &crate::routes::document_tree::metadata::Cut,
) -> ApiResult<()> {
    finish_read(ctx, cut, None).await
}

/// Actual bounds of one document projection, independent of its response DTO.
struct SourceBounds<'a> {
    document: Id,
    names: &'a NameBounds,
    references: &'a [WrittenLabelSource],
}

pub(super) async fn finish_sources(
    ctx: &Ctx,
    cut: &crate::routes::document_tree::metadata::Cut,
    document: Id,
    names: &NameBounds,
    references: &[WrittenLabelSource],
) -> ApiResult<()> {
    finish_read(
        ctx,
        cut,
        Some(SourceBounds {
            document,
            names,
            references,
        }),
    )
    .await
}

async fn finish_read(
    ctx: &Ctx,
    cut: &crate::routes::document_tree::metadata::Cut,
    prepared: Option<SourceBounds<'_>>,
) -> ApiResult<()> {
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    crate::routes::document_tree::metadata::unchanged(&mut tx, guard.principal(), cut, guard.now())
        .await?;
    if let Some(prepared) = prepared.filter(|read| {
        read.names.limited
            || guard
                .principal()
                .caller()
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some())
    }) {
        let mut plan = base(&mut tx, ctx.route, prepared.document, ctx.inspect_deleted).await?;
        let names = NameSources {
            nodes: prepared.names.nodes.clone(),
            containers: prepared.names.containers.clone(),
            ..Default::default()
        };
        let roles = name_roles(&plan);
        let fresh = projection_names(
            &mut tx,
            guard.principal().caller(),
            &mut plan,
            names,
            &roles,
        )
        .await?;
        let sources = reference_bounds(
            &mut tx,
            guard.principal().caller(),
            &mut plan,
            prepared.document,
            prepared.references,
        )
        .await?;
        if !prepared.names.nodes.is_subset(&fresh.nodes)
            || !prepared.names.containers.is_subset(&fresh.containers)
            || prepared
                .references
                .iter()
                .filter(|source| !matches!(source, WrittenLabelSource::Node(_)))
                .any(|source| !sources.contains(source))
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "the document read source is no longer admitted",
            ));
        }
        plan.admit(guard.principal().caller())?;
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

pub(crate) struct FeedProjection {
    pub document: Option<DocumentRef>,
    pub blocks: Vec<orch_api_contract::documents::DocumentBlock>,
    pub discussions: Vec<orch_api_contract::documents::DocumentDiscussion>,
}

/// Feed rows stay at their existing sequence cut; only their current per-caller projection is rebuilt.
pub(crate) async fn read_for(
    conn: &mut Conn,
    principal: &Principal,
    row: &DocumentRow,
    blocks: &[rows::BlockRow],
    discussions: &[discussions::Full],
    include_document: bool,
) -> ApiResult<FeedProjection> {
    authorization::require_document(
        conn,
        principal,
        row.id,
        Permission::DocView,
        Policy::default(),
    )
    .await?;
    let listed = build::standing(conn, principal, row.id, Policy::default()).await?;
    let mut plan = base(conn, DocumentSyncGet::ROUTE, row.id, false).await?;
    let (names, sources) = prepare_sources(
        conn,
        principal,
        &mut plan,
        row,
        &listed,
        blocks,
        discussions,
        false,
    )
    .await?;
    plan.admit(principal.caller())?;
    let (blocks, discussions) =
        project_parts(conn, principal, row, blocks, discussions, &names, &sources).await?;
    let document = if include_document {
        Some(if names.limited {
            build::ProjectedDocumentRef::capture_allowed(
                conn,
                principal,
                row,
                &listed,
                principal.now().get(),
                Policy::default(),
                &sources,
            )
            .await?
            .document
        } else {
            build::reference(
                conn,
                principal,
                row,
                &listed,
                principal.now().get(),
                Policy::default(),
            )
            .await?
        })
    } else {
        None
    };
    Ok(FeedProjection {
        document,
        blocks,
        discussions,
    })
}
