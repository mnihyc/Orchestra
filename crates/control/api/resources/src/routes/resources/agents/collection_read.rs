//! Scoped Agent collections reuse ordinary selection and the actual bulk source projection.

use super::read::NAMES;

use super::*;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use crate::routes::access::operations::projection_names;

use crate::routes::resources::collection::Collection;

use crate::routes::resources::collection::Query as CollectionQuery;

use crate::routes::resources::collection::Window;

use crate::routes::resources::collection::modern as scoped;

use crate::routes::resources::projection::bind;

use crate::routes::resources::projection::bind_collection_source;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::HiddenItem;

use orch_api_contract::common::Listed;

use orch_authz::Caller;
const COLLECTION: Collection = Collection::new(ResourceKind::Agent);

const AGENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Agent);

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);

const MACHINE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Machine);

struct PreparedList {
    response: OperationPlan,
    projection: Option<PreparedListProjection>,
    private: Vec<HiddenItem>,
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::AgentListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !scoped(caller) {
            // Preserve the original ordinary reader, including physical references whose
            // permission page is unavailable. No unresolved source is falsely called empty.
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        crate::routes::resources::context::empty(&mut plan)?;
        let Some(principal) = COLLECTION.principal_for(ctx, conn, caller, &plan).await? else {
            bind(&mut plan, 0, AGENT, &[])?;
            bind_collection_source(&mut plan, "agent_providers", PROVIDER, &[])?;
            bind_collection_source(&mut plan, "agent_machines", MACHINE, &[])?;
            projection_names(conn, caller, &mut plan, NameSources::default(), &NAMES).await?;
            let response = plan.clone();
            return Ok(Some(plan.with_data(PreparedList {
                response,
                projection: None,
                private: Vec::new(),
            })));
        };
        let selected = COLLECTION
            .selected(
                conn,
                &principal,
                caller,
                &plan,
                CollectionQuery {
                    window: Window {
                        after: None,
                        limit: LIST_LIMIT,
                    },
                    value: &query,
                    filter: query_scope,
                },
            )
            .await?;
        // Keep the existing capped Vec response; it does not promise total exhaustion.
        let mut seen = selected.items;
        let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
        let mut found = COLLECTION.targets(conn, &ids).await?;
        let facts = ids
            .into_iter()
            .map(|id| found.remove(&id).ok_or_else(ApiError::not_found))
            .collect::<ApiResult<Vec<_>>>()?;
        bind(&mut plan, 0, AGENT, &facts)?;
        let private = crate::routes::resources::hidden_items(conn, &principal, &mut seen).await?;
        let mut projection = prepare_list_projection(conn, &principal, &seen).await?;
        super::read::bind_sources(conn, caller, &mut plan, &mut projection).await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(PreparedList {
            response,
            projection: Some(projection),
            private,
        })))
    })
}

pub(super) async fn get(
    ctx: Ctx,
    query: api::AgentListQuery,
    (): (),
) -> ApiResult<Vec<Listed<Agent>>> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| snapshot(ctx, query.clone())).await
}

async fn snapshot(ctx: Ctx, query: api::AgentListQuery) -> ApiResult<Vec<Listed<Agent>>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    if !scoped(principal.caller()) {
        // Dispatch after restoration, on the actual read snapshot: an opening legacy
        // token cannot miss a current revoke or newly restrictive operation policy.
        let shown = ordinary_list(&ctx, &mut tx, &principal, &query).await?;
        tx.commit().await.map_err(DbError::from)?;
        return Ok(shown);
    }
    let prepared = ctx.operation_data::<PreparedList>()?;
    let view_ctx = ctx.clone();
    let shown = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let full = match &prepared.projection {
                    Some(projection) => {
                        show_list_prepared(&view_ctx, conn, reader, projection).await?
                    }
                    None => Vec::new(),
                };
                Ok(crate::routes::resources::listed(
                    full,
                    prepared.private.clone(),
                ))
            })
        })
        .await?
        .ok_or_else(|| {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the agent list response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
