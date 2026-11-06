//! The existing Template collection and bulk projection under current result/source bounds.

use super::read::NAMES;
use super::*;
use crate::registry::{OperationFuture, OperationPlan};
use crate::routes::access::operations::projection_names;
use crate::routes::resources::collection::{Collection, Query as CollectionQuery, Window};
use orch_access_contract::key_scope::KeyTargetKind;
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::common::{HiddenItem, Listed};
use orch_authz::Caller;
use std::collections::HashSet;

const COLLECTION: Collection = Collection::new(ResourceKind::Template);
const TEMPLATE: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Template);

struct PreparedList {
    response: OperationPlan,
    projection: Option<PreparedListProjection>,
    private: Vec<HiddenItem>,
}

fn query_scope<'a>(
    conn: &'a mut Conn,
    _principal: &'a Principal,
    seen: Vec<resources::Seen>,
    query: &'a api::TemplateListQuery,
) -> futures::future::BoxFuture<'a, ApiResult<Vec<resources::Seen>>> {
    Box::pin(async move {
        let mut seen: Vec<_> = seen
            .into_iter()
            .filter(|each| {
                each.administers
                    || each.held.iter().any(|held| {
                        matches!(
                            held.permission,
                            Permission::TemplateView
                                | Permission::TemplateUse
                                | Permission::TemplateManage
                                | Permission::AccessManage
                        )
                    })
            })
            .collect();
        if let Some(org) = query.org_id {
            let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
            let inside = queries::homes_under(conn, org, &homes).await?;
            seen.retain(|each| each.resource.home.is_none_or(|home| inside.contains(&home)));
        }
        if let Some(kind) = query.kind {
            // Ordinary kind filtering follows hidden_items: management-only rows do not
            // become full template content merely to decide this filter, and stay listed.
            let visible: Vec<_> = seen
                .iter()
                .filter(|each| {
                    each.held
                        .iter()
                        .any(|held| held.permission == Permission::TemplateView)
                })
                .map(|each| each.resource.id)
                .collect();
            let matching: HashSet<_> = queries::template_kinds(conn, &visible)
                .await?
                .into_iter()
                .filter(|(_, current)| current == unit_word(kind))
                .map(|(id, _)| id)
                .collect();
            seen.retain(|each| {
                !each
                    .held
                    .iter()
                    .any(|held| held.permission == Permission::TemplateView)
                    || matching.contains(&each.resource.id)
            });
        }
        Ok(seen)
    })
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: api::TemplateListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let Some(principal) = COLLECTION.principal_for(ctx, conn, caller, &plan).await? else {
            // Factual candidate exhaustion, never a guessed root or a budget-created empty list.
            plan.bind_empty(0)?;
            projection_names(conn, caller, &mut plan, NameSources::default(), &NAMES).await?;
            let response = plan.clone();
            return Ok(plan.with_data(PreparedList {
                response,
                projection: None,
                private: Vec::new(),
            }));
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
        // This endpoint retains its existing bounded Vec reply. A full window is not
        // advertised as an exhausted collection, nor turned into a new wire pagination model.
        let mut seen = selected.items;
        let ids: Vec<_> = seen.iter().map(|each| each.resource.id).collect();
        let mut targets = COLLECTION.targets(conn, &ids).await?;
        if ids.is_empty() {
            plan.bind_empty(0)?;
        } else {
            let targets = ids
                .into_iter()
                .map(|id| targets.remove(&id).ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?;
            plan.bind(0, TEMPLATE, &targets)?;
        }
        let private = crate::routes::resources::hidden_items(conn, &principal, &mut seen).await?;
        let mut projection = prepare_list_projection(conn, &seen).await?;
        projection.bounds =
            projection_names(conn, caller, &mut plan, projection.sources.clone(), &NAMES).await?;
        let response = plan.clone();
        Ok(plan.with_data(PreparedList {
            response,
            projection: Some(projection),
            private,
        }))
    })
}

pub(super) async fn get(
    ctx: Ctx,
    _query: api::TemplateListQuery,
    (): (),
) -> ApiResult<Vec<Listed<Template>>> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}
async fn snapshot(ctx: Ctx) -> ApiResult<Vec<Listed<Template>>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut tx).await?;
    let prepared = ctx.operation_data::<PreparedList>()?;
    let shown = prepared
        .response
        .clone()
        .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
            Box::pin(async move {
                let full = match &prepared.projection {
                    Some(projection) => show_list_prepared(conn, reader, projection).await?,
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
                "the template list response content has not been admitted",
            )
        })?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(shown)
}
