//! Current organization objects and optional reference sources for the existing native view.

use super::view;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::documents::build::ProjectedDocumentRef;

use crate::routes::documents::build::WrittenLabelSource;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::nodes::Node;

use orch_api_contract::nodes::Org;

use orch_authz::engine::Graph;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_db::queries::documents;

use orch_db::queries::hierarchy;

use orch_db::Conn;

use orch_db::DbError;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeSet;

use std::collections::HashMap;

use std::collections::HashSet;
const ORG: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Org);

const DOCUMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Document);

const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "org_names_orgs"),
        (ObjectKind::Project, "org_names_projects"),
        (ObjectKind::Topic, "org_names_topics"),
        (ObjectKind::Thread, "org_names_threads"),
    ],
    containers: None,
};

struct Prepared {
    ids: Vec<Id>,
    names: NameBounds,
    readmes: BTreeSet<Id>,
    response: OperationPlan,
}

fn bind(
    plan: &mut OperationPlan,
    slot: usize,
    kind: KeyTargetKind,
    rows: &[KeyResolvedTarget],
) -> ApiResult<()> {
    if rows.is_empty() {
        plan.bind_empty(slot)
    } else {
        plan.bind(slot, kind, rows)
    }
}

async fn prepare(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    by_address: bool,
) -> ApiResult<OperationPlan> {
    let raw =
        authorization::load_principal(conn, caller.clone(), Time::new(ctx.app.clock.now())).await?;
    let candidates = if by_address {
        vec![
            hierarchy::org_by_address(conn, ctx.param("slug")?)
                .await?
                .ok_or_else(ApiError::not_found)?,
        ]
    } else {
        hierarchy::visible_root_orgs(conn, raw.user(), raw.is_system_admin()).await?
    };
    let graph = Graph::load(conn, &candidates)
        .await
        .map_err(ApiError::internal)?;
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    let mut selected = Vec::new();
    for id in &candidates {
        let site = graph
            .site_of(*id)
            .filter(|site| site.kind == ObjectKind::Org)
            .ok_or_else(ApiError::not_found)?;
        let target = targets::from_site(&graph, &site)?;
        if by_address || plan.permits_binding(caller, 0, ORG, std::slice::from_ref(&target))? {
            selected.push(target);
        }
    }
    bind(&mut plan, 0, ORG, &selected)?;
    if selected.is_empty() {
        // No selected organization can supply a reference source. Complete every
        // declared collection before asking the shared empty-operation admission.
        for slot in 1..plan.variant().facets.len() {
            plan.bind_empty(slot)?;
        }
        let response = plan.clone();
        return Ok(plan.with_data(Prepared {
            ids: Vec::new(),
            names: NameBounds {
                limited: caller
                    .grant()
                    .is_some_and(|grant| grant.operation_scope().is_some()),
                ..NameBounds::default()
            },
            readmes: BTreeSet::new(),
            response,
        }));
    }
    let principal = plan.preparation_principal(ctx, conn, caller).await?;
    let ids: Vec<_> = selected
        .iter()
        .filter_map(|target| target.object.id)
        .collect();
    if by_address {
        authorization::require_projected(
            conn,
            &principal,
            ids.first().copied().ok_or_else(ApiError::not_found)?,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
            principal
                .caller()
                .grant()
                .is_some_and(|grant| grant.operation_scope().is_some()),
        )
        .await?;
    }
    let visible: HashSet<_> = orch_authz::nodes::holding(
        conn,
        &principal,
        &ids,
        authorization::VIEW,
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|row| row.id)
    .collect();
    selected.retain(|target| target.object.id.is_some_and(|id| visible.contains(&id)));
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    bind(&mut plan, 0, ORG, &selected)?;
    let ids: Vec<_> = selected
        .iter()
        .filter_map(|target| target.object.id)
        .collect();
    let mut sources = NameSources::default();
    for target in &selected {
        sources
            .nodes
            .extend(target.node_ancestors.iter().filter_map(|node| node.id));
        sources.nodes.extend(target.object.id);
    }
    let readme_rows = documents::readmes_of(conn, &ids).await?;
    let document_ids: Vec<_> = readme_rows.iter().map(|row| row.id).collect();
    let readable: HashMap<_, _> =
        orch_authz::documents::listed(conn, &principal, &document_ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|row| row.holds(Permission::DocView))
            .map(|row| (row.document.id, row))
            .collect();
    let mut document_graph = Graph::default();
    let rows = orch_authz::documents::read_into(conn, &mut document_graph, &document_ids)
        .await
        .map_err(ApiError::internal)?;
    let mut document_targets = HashMap::new();
    for row in rows {
        let Some(site) = orch_authz::documents::site_of(&document_graph, &row) else {
            continue;
        };
        document_targets.insert(row.id, targets::from_site(&document_graph, &site)?);
    }
    let mut readmes = BTreeSet::new();
    let mut accepted_documents = Vec::new();
    for row in readme_rows {
        let (Some(listed), Some(target)) = (readable.get(&row.id), document_targets.get(&row.id))
        else {
            continue;
        };
        if !plan.permits_binding(caller, 1, DOCUMENT, std::slice::from_ref(target))? {
            continue;
        }
        readmes.insert(row.id);
        accepted_documents.push(target.clone());
        for source in ProjectedDocumentRef::candidates(conn, &row, listed).await? {
            match source {
                WrittenLabelSource::Node(id) => {
                    sources.nodes.insert(id);
                }
                _ => {
                    return Err(ApiError::internal(
                        "an organization README has an undeclared projection source",
                    ));
                }
            }
        }
    }
    bind(&mut plan, 1, DOCUMENT, &accepted_documents)?;
    let names = projection_names(conn, principal.caller(), &mut plan, sources, &NAMES).await?;
    let response = plan.clone();
    Ok(plan.with_data(Prepared {
        ids,
        names,
        readmes,
        response,
    }))
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, false))
}

pub(super) fn address<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(prepare(ctx, conn, caller, true))
}

async fn read(ctx: &Ctx) -> ApiResult<Vec<Org>> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<Prepared>()?;
    let nodes = if prepared.names.limited {
        prepared
            .response
            .clone()
            .read_response(ctx, &mut conn, principal.caller(), move |conn, reader| {
                Box::pin(async move {
                    view::visible_with_read_sources(
                        conn,
                        reader,
                        &prepared.ids,
                        reader.now().get(),
                        &prepared.names.nodes,
                        &prepared.readmes,
                    )
                    .await
                })
            })
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    orch_api_contract::common::ErrorCode::AuthzDenied,
                    "the organization read is no longer allowed",
                )
            })?
    } else {
        view::visible(
            &mut conn,
            &principal,
            &prepared.ids,
            principal.now().get(),
            Policy::default(),
        )
        .await?
    };
    let result = nodes
        .into_iter()
        .filter_map(|node| match node {
            Node::Org(org) => Some(org),
            _ => None,
        })
        .collect();
    conn.commit().await.map_err(DbError::from)?;
    Ok(result)
}

pub(super) async fn get_list(ctx: Ctx, (): (), (): ()) -> ApiResult<Vec<Org>> {
    read(&ctx).await
}

pub(super) async fn get_address(ctx: Ctx, (): (), (): ()) -> ApiResult<Org> {
    read(&ctx)
        .await?
        .into_iter()
        .next()
        .ok_or_else(ApiError::not_found)
}
