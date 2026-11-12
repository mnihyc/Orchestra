//! The ordinary provider collection, with actual scoped rows selected before its page.
//!
//! Candidate identities establish no authority. A current admitted candidate permits only
//! effect-free ordinary preparation; the final read carries the returned page's full sources.

use super::collection::Collection;

use super::collection::Query as CollectionQuery;

use super::collection::Window;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationPlan;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameSources;

use crate::routes::nodes::invalid;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::HiddenItem;

use orch_api_contract::common::Listed;

use orch_api_contract::common::Page as ReplyPage;

use orch_api_contract::common::PageQuery;

use orch_api_contract::resources::Provider;

use orch_api_contract::resources::ProviderListQuery;

use orch_api_contract::resources::ProviderListScope;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::resources;

use orch_db::Conn;

use orch_types::Id;
const COLLECTION: Collection = Collection::new(ResourceKind::Provider);

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);

struct PreparedList {
    response: OperationPlan,
    projection: Option<super::providers::PreparedListProjection>,
    private: Vec<HiddenItem>,
    next_cursor: Option<String>,
}

fn after(query: &ProviderListQuery) -> ApiResult<Option<Id>> {
    query
        .cursor
        .as_deref()
        .map(|cursor| {
            cursor
                .parse::<Id>()
                .map_err(|_| invalid("cursor", "is not a cursor this list gave"))
        })
        .transpose()
}

fn limit(query: &ProviderListQuery) -> usize {
    usize::try_from(
        PageQuery {
            limit: query.limit,
            cursor: None,
        }
        .limit(),
    )
    .unwrap_or(super::LIST_LIMIT)
}

async fn in_query_scope(
    conn: &mut Conn,
    principal: &Principal,
    seen: Vec<resources::Seen>,
    query: &ProviderListQuery,
) -> ApiResult<Vec<resources::Seen>> {
    let mut seen = super::providers::in_scope(
        conn,
        principal,
        seen,
        query.scope.unwrap_or(ProviderListScope::All),
    )
    .await?;
    if let Some(org) = query.org_id {
        let homes: Vec<_> = seen.iter().filter_map(|each| each.resource.home).collect();
        let inside = orch_db::queries::resources::routes::homes_under(conn, org, &homes).await?;
        seen.retain(|each| {
            each.resource
                .home
                .is_some_and(|home| inside.contains(&home))
        });
    }
    Ok(seen)
}

fn query_scope<'a>(
    conn: &'a mut Conn,
    principal: &'a Principal,
    seen: Vec<resources::Seen>,
    query: &'a ProviderListQuery,
) -> futures::future::BoxFuture<'a, ApiResult<Vec<resources::Seen>>> {
    Box::pin(in_query_scope(conn, principal, seen, query))
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: ProviderListQuery,
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        after(&query)?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        crate::routes::resources::context::empty(&mut plan)?;
        let Some(principal) = COLLECTION.principal_for(ctx, conn, caller, &plan).await? else {
            // The factual ordinary candidate stream was exhausted without an in-scope root.
            // Only now is the returned collection genuinely empty; normal empty admission applies.
            plan.bind_empty(0)?;
            projection_names(
                conn,
                caller,
                &mut plan,
                NameSources::default(),
                &super::provider_projection::NAMES,
            )
            .await?;
            let response = plan.clone();
            return Ok(plan.with_data(PreparedList {
                response,
                projection: None,
                private: Vec::new(),
                next_cursor: None,
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
                        after: after(&query)?,
                        limit: limit(&query),
                    },
                    value: &query,
                    filter: query_scope,
                },
            )
            .await?;
        let mut seen = selected.items;
        let next_cursor = selected.next_cursor;
        let ids: Vec<_> = seen.iter().map(|row| row.resource.id).collect();
        let targets = COLLECTION.targets(conn, &ids).await?;
        if ids.is_empty() {
            plan.bind_empty(0)?;
        } else {
            let targets = ids
                .into_iter()
                .map(|id| targets.get(&id).cloned().ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?;
            plan.bind(0, PROVIDER, &targets)?;
        }
        let private = super::hidden_items(conn, &principal, &mut seen).await?;
        let mut projection =
            super::providers::prepare_list_projection(ctx, conn, &principal, &seen).await?;
        projection.projection.bounds = projection_names(
            conn,
            caller,
            &mut plan,
            projection.projection.sources.clone(),
            &super::provider_projection::NAMES,
        )
        .await?;
        let response = plan.clone();
        Ok(plan
            .with_snapshot_principal(ctx, conn, principal)
            .with_data(PreparedList {
                response,
                projection: Some(projection),
                private,
                next_cursor,
            }))
    })
}

pub(super) async fn get(
    ctx: Ctx,
    _query: ProviderListQuery,
    (): (),
) -> ApiResult<ReplyPage<Listed<Provider>>> {
    crate::key_operations::snapshot_read::run(ctx, read_snapshot).await
}

async fn read_snapshot(ctx: Ctx) -> ApiResult<ReplyPage<Listed<Provider>>> {
    let completed = crate::key_operations::native::read_once(ctx, |ctx, conn, principal| {
        Box::pin(async move {
            let prepared = ctx.operation_data::<PreparedList>()?;
            // The pending page becomes readable only under this same operation's
            // complete current response footprint and ordinary projection.
            let read_ctx = ctx.clone();
            prepared
                .response
                .clone()
                .read_snapshot_response(ctx, conn, principal, move |conn, reader| {
                    Box::pin(async move {
                        let shown = match &prepared.projection {
                            Some(projection) => {
                                super::providers::show_list_prepared(
                                    &read_ctx, conn, reader, projection,
                                )
                                .await?
                            }
                            None => Vec::new(),
                        };
                        Ok(ReplyPage {
                            items: super::listed(shown, prepared.private.clone()),
                            next_cursor: prepared.next_cursor.clone(),
                        })
                    })
                })
                .await?
                .ok_or_else(|| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::AuthzDenied,
                        "the provider list response content has not been admitted",
                    )
                })
        })
    })
    .await?;
    Ok(completed.value)
}
