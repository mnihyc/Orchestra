//! Exact MCP reader subjects and actual full-response sources on their guarded snapshot.

use super::*;
use crate::key_operations::targets;
use crate::registry::{OperationPlan, OptionalOperationFuture};
use crate::routes::access::operations::{NameRoles, projection_names};
use crate::routes::access::page::{NameBounds, NameSources};
use crate::routes::resources::collection::{Collection, Query as CollectionQuery, Window};
use crate::routes::resources::mcp::projection::{self, Prepared, Style};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::{ErrorCode, HiddenItem};
use orch_api_contract::operations::TargetResolver;
use orch_authz::Caller;
use orch_db::Conn;

const COLLECTION: Collection = Collection::new(ResourceKind::McpServer);
const ENVIRONMENTS: Collection = Collection::new(ResourceKind::Environment);
const MCP: KeyTargetKind = KeyTargetKind::Object(ObjectKind::McpServer);
const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const NAMES: NameRoles = NameRoles {
    groups: Some("mcp_groups"),
    nodes: [
        (ObjectKind::Org, "mcp_names_orgs"),
        (ObjectKind::Project, "mcp_names_projects"),
        (ObjectKind::Topic, "mcp_names_topics"),
        (ObjectKind::Thread, "mcp_names_threads"),
    ],
    containers: None,
};
struct Read {
    response: OperationPlan,
    projection: Option<Prepared>,
    names: NameBounds,
    private: Vec<HiddenItem>,
}
fn scoped(caller: &Caller) -> bool {
    caller
        .grant()
        .is_some_and(|grant| grant.operation_scope().is_some())
}
fn bind(
    plan: &mut OperationPlan,
    role: &str,
    kind: KeyTargetKind,
    targets: &[KeyResolvedTarget],
) -> ApiResult<()> {
    let index = plan
        .variant()
        .facets
        .iter()
        .position(|facet| {
            matches!(facet.target,
        TargetResolver::Resolved(name) | TargetResolver::ResolvedCollection(name) if name == role)
        })
        .ok_or_else(|| ApiError::internal("the MCP reader source role is not declared"))?;
    if targets.is_empty() {
        plan.bind_empty(index)
    } else {
        plan.bind(index, kind, targets)
    }
}
async fn resource_facts(
    collection: Collection,
    conn: &mut Conn,
    ids: &[Id],
) -> ApiResult<Vec<KeyResolvedTarget>> {
    let mut targets = collection.targets(conn, ids).await?;
    ids.iter()
        .map(|id| targets.remove(id).ok_or_else(ApiError::not_found))
        .collect()
}
async fn sources(
    conn: &mut Conn,
    caller: &Caller,
    plan: &mut OperationPlan,
    prepared: Option<&Prepared>,
    environment: bool,
) -> ApiResult<NameBounds> {
    let machines = match prepared {
        Some(prepared) => {
            let mut subjects = targets::machine_subjects(conn, &prepared.machines).await?;
            prepared
                .machines
                .iter()
                .map(|id| subjects.remove(id).ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?
        }
        None => Vec::new(),
    };
    bind(plan, "mcp_machines", MACHINE, &machines)?;
    if environment {
        let ids = prepared.map_or(&[][..], |prepared| prepared.environments.as_slice());
        let facts = resource_facts(ENVIRONMENTS, conn, ids).await?;
        bind(plan, "mcp_environment", ENVIRONMENT, &facts)?;
    }
    let names = prepared.map_or_else(NameSources::default, |prepared| prepared.names.clone());
    projection_names(conn, caller, plan, names, &NAMES).await
}

pub(super) fn list<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::McpServerListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let Some(principal) = COLLECTION.principal_for(ctx, conn, caller, &plan).await? else {
            bind(&mut plan, "results", MCP, &[])?;
            let names = sources(conn, caller, &mut plan, None, false).await?;
            let response = plan.clone();
            return Ok(Some(plan.with_data(Read {
                response,
                projection: None,
                names,
                private: Vec::new(),
            })));
        };
        // This endpoint already promises the complete ordinary collection. Drain the shared
        // scoped windows to factual exhaustion instead of imposing the other lists' 500 cap.
        let mut seen = COLLECTION
            .all(
                conn,
                &principal,
                caller,
                &plan,
                CollectionQuery {
                    window: Window {
                        after: None,
                        limit: super::super::LIST_LIMIT,
                    },
                    value: &query,
                    filter: query_scope,
                },
            )
            .await?;
        let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
        let facts = resource_facts(COLLECTION, conn, &ids).await?;
        bind(&mut plan, "results", MCP, &facts)?;
        let private = super::super::hidden_items(conn, &principal, &mut seen).await?;
        let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
        let projection = projection::prepare_for_snapshot(conn, &principal, &ids, &seen).await?;
        let names = sources(conn, caller, &mut plan, Some(&projection), false).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Read {
            response,
            projection: Some(projection),
            names,
            private,
        })))
    })
}

pub(super) fn get<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.shared()
            .compatible(caller)
            .map_err(crate::key_operations::admission::binding_error)?;
        let subject = orch_db::queries::resources::mcp::subject(conn, ctx.id("id")?)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let object = match subject.environment_id {
            Some(id) => ObjectRef {
                kind: ObjectKind::Environment,
                id,
            },
            None => ObjectRef {
                kind: ObjectKind::McpServer,
                id: subject.id,
            },
        };
        let target = targets::page_object(conn, object).await?;
        // Actual declared kind: an environment-owned server has no fabricated MCP page.
        bind(&mut plan, "server", target.object.kind, &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        super::super::mcp::require_subject(
            conn,
            &principal,
            subject.id,
            subject.environment_id,
            false,
        )
        .await?;
        let style = if subject.environment_id.is_some() {
            Style::Individual
        } else {
            Style::Presets
        };
        let projection = projection::prepare(conn, &principal, &[subject.id], None, style).await?;
        let names = sources(conn, caller, &mut plan, Some(&projection), true).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Read {
            response,
            projection: Some(projection),
            names,
            private: Vec::new(),
        })))
    })
}

pub(super) async fn read_list(
    ctx: Ctx,
    query: api::McpServerListQuery,
    (): (),
) -> ApiResult<Vec<Listed<McpServer>>> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| list_snapshot(ctx, query.clone()))
        .await
}
async fn list_snapshot(
    ctx: Ctx,
    query: api::McpServerListQuery,
) -> ApiResult<Vec<Listed<McpServer>>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    if !scoped(principal.caller()) {
        let shown = ordinary_list(&ctx, &mut tx, &principal, &query).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(shown);
    }
    let read = ctx.operation_data::<Read>()?;
    let view_ctx = ctx.clone();
    let shown = read
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let full = match &read.projection {
                    Some(prepared) => {
                        projection::render(&view_ctx, conn, reader, prepared, Some(&read.names))
                            .await?
                    }
                    None => Vec::new(),
                };
                Ok(super::super::listed(full, read.private.clone()))
            })
        })
        .await?
        .ok_or_else(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}
pub(super) async fn read_get(ctx: Ctx, (): (), (): ()) -> ApiResult<McpServer> {
    crate::key_operations::snapshot_read::run(ctx, get_snapshot).await
}
async fn get_snapshot(ctx: Ctx) -> ApiResult<McpServer> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    if !scoped(principal.caller()) {
        let shown = read_for(&ctx, &mut tx, &principal, ctx.id("id")?).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        return Ok(shown);
    }
    let read = ctx.operation_data::<Read>()?;
    let view_ctx = ctx.clone();
    let shown = read
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let prepared = read.projection.as_ref().ok_or_else(ApiError::not_found)?;
                projection::render(&view_ctx, conn, reader, prepared, Some(&read.names))
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)
            })
        })
        .await?
        .ok_or_else(refused)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(shown)
}
fn refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the MCP response content has not been admitted",
    )
}
