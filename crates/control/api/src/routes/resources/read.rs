//! Shared native family Get/List preparation, registration and current response projection.
//!
//! A family supplies its existing ordinary gates, query filter and native factory once.
//! The adapters bind that factory's actual source facts under the consuming operation;
//! they do not walk response DTOs, infer targets from URLs or change account permissions.

use super::collection::{Collection, Query, Window, modern};
use super::projection::{self, Rendered};
use crate::key_operations::native::{ReadFrame, current_read_frame as current_frame};
use crate::key_operations::targets;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::CurrentHttpIdentity;
use crate::registry::{Ctx, OperationPlan, OptionalOperationFuture, Registry};
use crate::routes::access::operations::projection_names;
use crate::routes::access::page::NameSources;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::common::{ErrorCode, HiddenItem, Listed};
use orch_api_contract::resource_reads;
use orch_api_contract::routes::Endpoint;
use orch_authz::{Caller, Principal, resources::Seen};
use orch_db::{Conn, DbError};
use orch_types::Id;
use serde::{Serialize, de::DeserializeOwned};
use std::marker::PhantomData;

pub(super) enum Cardinality {
    #[expect(
        dead_code,
        reason = "retained cardinality for native family adapter rollout"
    )]
    Capped,
    Exhaustive,
}

/// One existing native resource family, shared by its Get/List entry points.
pub(super) trait Family: Send + Sync + 'static {
    type Item: Serialize + Send + 'static;
    type ListQuery: DeserializeOwned + Clone + Send + Sync + 'static;
    const SPEC: &'static resource_reads::Family;
    const CARDINALITY: Cardinality;

    fn require_get<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<()>>;

    /// The original native Get factory, including its shape-specific source preparation.
    /// A family whose Get differs from inventory must keep that native factory here.
    fn prepare_get<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        id: Id,
    ) -> BoxFuture<'a, ApiResult<projection::Prepared>>;

    fn filter<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        rows: Vec<Seen>,
        query: &'a Self::ListQuery,
    ) -> BoxFuture<'a, ApiResult<Vec<Seen>>>;

    fn into_items(rendered: Rendered) -> ApiResult<Vec<Self::Item>>;

    /// Existing auxiliary Get work runs once after the effect-free snapshot has committed.
    /// It cannot mutate the admitted body or be replayed with a database snapshot retry.
    fn after_get<'a>(ctx: &'a Ctx, item: &'a Self::Item) -> BoxFuture<'a, ApiResult<()>> {
        let _ = (ctx, item);
        Box::pin(async { Ok(()) })
    }
}

struct Ready<F: Family> {
    response: OperationPlan,
    projection: Option<projection::Prepared>,
    private: Vec<HiddenItem>,
    family: PhantomData<fn() -> F>,
}

struct Completed<T> {
    ctx: Ctx,
    identity: CurrentHttpIdentity,
    value: T,
}

/// The declaration supplies both typed endpoints; no endpoint-name dispatch or bespoke
/// operation resolver is added for either entry point.
pub(super) fn register<F, G, L>(registry: &mut Registry, get_endpoint: G, list_endpoint: L)
where
    F: Family,
    G: Endpoint<Query = (), Body = (), Reply = F::Item>,
    L: Endpoint<Query = F::ListQuery, Body = (), Reply = Vec<Listed<F::Item>>>,
{
    registry.on_with_optional_operation(get_endpoint, prepare_get::<F>, get::<F>);
    register_list::<F, L>(registry, list_endpoint);
}

/// Register a native list without coupling it to a Get-only auxiliary effect.
pub(super) fn register_list<F, L>(registry: &mut Registry, endpoint: L)
where
    F: Family,
    L: Endpoint<Query = F::ListQuery, Body = (), Reply = Vec<Listed<F::Item>>>,
{
    registry.on_with_optional_operation(endpoint, prepare_list::<F>, list::<F>);
}

fn prepare_get<'a, F: Family>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let id = ctx.id("id")?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: F::SPEC.kind.into(),
                id,
            },
        )
        .await?;
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        plan.bind(0, KeyTargetKind::Object(F::SPEC.kind.into()), &[target])?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        F::require_get(conn, &principal, id).await?;
        let mut projection = F::prepare_get(ctx, conn, &principal, id).await?;
        projection
            .bind_family_sources(conn, caller, &mut plan, F::SPEC)
            .await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Ready::<F> {
            response,
            projection: Some(projection),
            private: Vec::new(),
            family: PhantomData,
        })))
    })
}

fn prepare_list<'a, F: Family>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    query: F::ListQuery,
    (): (),
) -> OptionalOperationFuture<'a> {
    Box::pin(async move {
        if !modern(caller) {
            return Ok(None);
        }
        let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
        let collection = Collection::new(F::SPEC.kind);
        let Some(principal) = collection.principal_for(ctx, conn, caller, &plan).await? else {
            // This branch follows actual candidate exhaustion, never missing facts.
            plan.bind_empty(0)?;
            for role in F::SPEC.sources {
                projection::bind_collection_source(
                    &mut plan,
                    role.role,
                    KeyTargetKind::Object(role.kind.into()),
                    &[],
                )?;
            }
            projection_names(
                conn,
                caller,
                &mut plan,
                NameSources::default(),
                &projection::name_roles(F::SPEC),
            )
            .await?;
            let response = plan.clone();
            return Ok(Some(plan.with_data(Ready::<F> {
                response,
                projection: None,
                private: Vec::new(),
                family: PhantomData,
            })));
        };
        let mut seen = selected::<F>(conn, &principal, caller, &plan, &query).await?;
        let ids: Vec<_> = seen.iter().map(|row| row.resource.id).collect();
        let mut found = collection.targets(conn, &ids).await?;
        let facts = ids
            .iter()
            .map(|id| found.remove(id).ok_or_else(ApiError::not_found))
            .collect::<ApiResult<Vec<_>>>()?;
        projection::bind(
            &mut plan,
            0,
            KeyTargetKind::Object(F::SPEC.kind.into()),
            &facts,
        )?;
        let private = super::hidden_items(conn, &principal, &mut seen).await?;
        let mut projection = projection::prepare(
            ctx,
            conn,
            &principal,
            projection::Selection {
                kind: F::SPEC.kind,
                ids: seen.iter().map(|row| row.resource.id).collect(),
                seen: &seen,
            },
        )
        .await?;
        if let Some(prepared) = projection.as_mut() {
            prepared
                .bind_family_sources(conn, caller, &mut plan, F::SPEC)
                .await?;
        } else {
            for role in F::SPEC.sources {
                projection::bind_collection_source(
                    &mut plan,
                    role.role,
                    KeyTargetKind::Object(role.kind.into()),
                    &[],
                )?;
            }
            projection_names(
                conn,
                caller,
                &mut plan,
                NameSources::default(),
                &projection::name_roles(F::SPEC),
            )
            .await?;
        }
        let response = plan.clone();
        Ok(Some(plan.with_data(Ready::<F> {
            response,
            projection,
            private,
            family: PhantomData,
        })))
    })
}

async fn selected<F: Family>(
    conn: &mut Conn,
    principal: &Principal,
    caller: &Caller,
    plan: &OperationPlan,
    query: &F::ListQuery,
) -> ApiResult<Vec<Seen>> {
    let query = Query {
        window: Window {
            after: None,
            limit: super::LIST_LIMIT,
        },
        value: query,
        filter: F::filter,
    };
    let collection = Collection::new(F::SPEC.kind);
    match F::CARDINALITY {
        Cardinality::Capped => Ok(collection
            .selected(conn, principal, caller, plan, query)
            .await?
            .items),
        Cardinality::Exhaustive => collection.all(conn, principal, caller, plan, query).await,
    }
}

async fn render<F: Family>(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    prepared: &projection::Prepared,
) -> ApiResult<Vec<F::Item>> {
    F::into_items(prepared.render(ctx, conn, principal).await?)
}

async fn get<F: Family>(ctx: Ctx, (): (), (): ()) -> ApiResult<F::Item> {
    let completed = crate::key_operations::snapshot_read::run(ctx, snapshot_get::<F>).await?;
    // Auxiliary native work is outside the effect-free retry and receives the actual
    // context that produced this body. Hold either outcome through the same final cut.
    let result = F::after_get(&completed.ctx, &completed.value).await;
    completed.identity.check_deadline(&completed.ctx.app)?;
    result?;
    Ok(completed.value)
}

async fn snapshot_get<F: Family>(ctx: Ctx) -> ApiResult<Completed<F::Item>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let ReadFrame {
        ctx,
        identity,
        principal,
    } = current_frame(&ctx, &mut tx).await?;
    let result: ApiResult<F::Item> = async {
        let id = ctx.id("id")?;
        let item = if modern(principal.caller()) {
            let ready = ctx.operation_data::<Ready<F>>()?;
            let view_ctx = ctx.clone();
            ready
                .response
                .clone()
                .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
                    Box::pin(async move {
                        F::require_get(conn, reader, id).await?;
                        let prepared = ready.projection.as_ref().ok_or_else(ApiError::not_found)?;
                        render::<F>(&view_ctx, conn, reader, prepared)
                            .await?
                            .pop()
                            .ok_or_else(ApiError::not_found)
                    })
                })
                .await?
                .ok_or_else(disclosure_refused)?
        } else {
            F::require_get(&mut tx, &principal, id).await?;
            let prepared = F::prepare_get(&ctx, &mut tx, &principal, id).await?;
            render::<F>(&ctx, &mut tx, &principal, &prepared)
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?
        };
        tx.commit().await.map_err(DbError::from)?;
        Ok(item)
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    Ok(Completed {
        ctx,
        identity,
        value: result?,
    })
}

async fn list<F: Family>(ctx: Ctx, query: F::ListQuery, (): ()) -> ApiResult<Vec<Listed<F::Item>>> {
    crate::key_operations::snapshot_read::run(ctx, move |ctx| {
        snapshot_list::<F>(ctx, query.clone())
    })
    .await
}

async fn snapshot_list<F: Family>(
    ctx: Ctx,
    query: F::ListQuery,
) -> ApiResult<Vec<Listed<F::Item>>> {
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let ReadFrame {
        ctx,
        identity,
        principal,
    } = current_frame(&ctx, &mut tx).await?;
    let result: ApiResult<Vec<Listed<F::Item>>> = async {
        let shown = if modern(principal.caller()) {
            let ready = ctx.operation_data::<Ready<F>>()?;
            let view_ctx = ctx.clone();
            ready
                .response
                .clone()
                .read_response(&ctx, &mut tx, principal.caller(), move |conn, reader| {
                    Box::pin(async move {
                        let items = match &ready.projection {
                            Some(prepared) => {
                                render::<F>(&view_ctx, conn, reader, prepared).await?
                            }
                            None => Vec::new(),
                        };
                        Ok(super::listed(items, ready.private.clone()))
                    })
                })
                .await?
                .ok_or_else(disclosure_refused)?
        } else {
            let plan = OperationPlan::new(ctx.route, ctx.route.name)?;
            let mut seen =
                selected::<F>(&mut tx, &principal, principal.caller(), &plan, &query).await?;
            let private = super::hidden_items(&mut tx, &principal, &mut seen).await?;
            let prepared = projection::prepare(
                &ctx,
                &mut tx,
                &principal,
                projection::Selection {
                    kind: F::SPEC.kind,
                    ids: seen.iter().map(|row| row.resource.id).collect(),
                    seen: &seen,
                },
            )
            .await?;
            let items = match prepared {
                Some(prepared) => render::<F>(&ctx, &mut tx, &principal, &prepared).await?,
                None => Vec::new(),
            };
            super::listed(items, private)
        };
        tx.commit().await.map_err(DbError::from)?;
        Ok(shown)
    }
    .await;
    identity.check_deadline(&ctx.app)?;
    result
}

fn disclosure_refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the native resource response has not been admitted",
    )
}
