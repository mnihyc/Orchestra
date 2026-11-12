//! Shared native family Get/List preparation, registration and current response projection.
//!
//! A family supplies its existing ordinary gates, query filter and native factory once.
//! The adapters bind that factory's actual source facts under the consuming operation;
//! they do not walk response DTOs, infer targets from URLs or change account permissions.

use super::collection::Collection;

use super::collection::Query;

use super::collection::Window;

use super::collection::modern;

use super::projection;

use super::projection::Rendered;

use super::view::Authority;

use crate::key_operations::native;

use crate::key_operations::native::Completed;

use crate::key_operations::targets;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationPlan;

use crate::registry::OptionalOperationFuture;

use crate::registry::Registry;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameSources;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ErrorCode;

use orch_api_contract::common::HiddenItem;

use orch_api_contract::common::Listed;

use orch_api_contract::resource_reads;

use orch_api_contract::routes::Endpoint;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_authz::resources::Seen;

use orch_db::Conn;

use orch_types::Id;

use serde::Serialize;

use serde::de::DeserializeOwned;

use std::collections::BTreeMap;

use std::marker::PhantomData;

use std::sync::Arc;
pub(super) enum Cardinality {
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

    /// Existing native list auxiliary work runs once on the successful producing context
    /// and actual selected items, after snapshot commit/connection release. Any first effect
    /// keeps its own current native gate; this hook is never a stored authority lease.
    fn after_list<'a>(
        _ctx: &'a Ctx,
        _items: &'a [Listed<Self::Item>],
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async { Ok(()) })
    }
}

struct Ready<F: Family> {
    response: OperationPlan,
    projection: Option<projection::Prepared>,
    private: Vec<HiddenItem>,
    authority: BTreeMap<(ObjectKind, Id), Authority>,
    family: PhantomData<fn() -> F>,
}

struct Produced<T, F: Family> {
    value: T,
    opened: Option<(Arc<Ready<F>>, Caller)>,
}

enum ReadKind<Q> {
    Get,
    List(Q),
}

/// The declaration supplies both typed endpoints; no endpoint-name dispatch or bespoke
/// operation resolver is added for either entry point.
pub(super) fn register<F, G, L>(registry: &mut Registry, get_endpoint: G, list_endpoint: L)
where
    F: Family,
    G: Endpoint<Query = (), Body = (), Reply = F::Item>,
    L: Endpoint<Query = F::ListQuery, Body = (), Reply = Vec<Listed<F::Item>>>,
{
    registry.on_snapshot_read_with_optional_operation(get_endpoint, prepare_get::<F>, get::<F>);
    register_list::<F, L>(registry, list_endpoint);
}

/// Register a native list without coupling it to a Get-only auxiliary effect.
pub(super) fn register_list<F, L>(registry: &mut Registry, endpoint: L)
where
    F: Family,
    L: Endpoint<Query = F::ListQuery, Body = (), Reply = Vec<Listed<F::Item>>>,
{
    registry.on_snapshot_read_with_optional_operation(endpoint, prepare_list::<F>, list::<F>);
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
        let object = ObjectRef {
            kind: F::SPEC.kind.into(),
            id,
        };
        let held = orch_authz::resources::effective(
            conn,
            &principal,
            object,
            orch_authz::Policy::default(),
        )
        .await
        .map_err(crate::middleware::authorization::restoration_error)?;
        let authority = BTreeMap::from([((object.kind, id), Authority::effective(&held))]);
        let mut projection = F::prepare_get(ctx, conn, &principal, id).await?;
        projection
            .bind_family_sources(conn, caller, &mut plan, F::SPEC)
            .await?;
        let response = plan.clone();
        Ok(Some(plan.with_data(Ready::<F> {
            response,
            projection: Some(projection),
            private: Vec::new(),
            authority,
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
                authority: BTreeMap::new(),
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
        let authority = seen
            .iter()
            .map(|row| {
                (
                    (row.resource.kind.into(), row.resource.id),
                    Authority::seen(row),
                )
            })
            .collect();
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
            authority,
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
    let produced = completed.value;
    if let Some((opened, accepted)) = produced.opened {
        retain::<F>(&completed.ctx, &accepted, opened, ReadKind::Get)?;
    }
    // Auxiliary native work is outside the effect-free retry and receives the actual
    // context that produced this body. Hold either outcome through the same final cut.
    let result = F::after_get(&completed.ctx, &produced.value).await;
    completed.identity.check_deadline(&completed.ctx.app)?;
    result?;
    Ok(produced.value)
}

async fn snapshot_get<F: Family>(ctx: Ctx) -> ApiResult<Completed<Produced<F::Item, F>>> {
    native::read_once(ctx, |ctx, conn, principal| {
        Box::pin(async move {
            let id = ctx.id("id")?;
            if modern(principal.caller()) {
                let ready = ctx.operation_data::<Ready<F>>()?;
                let opened = Arc::clone(&ready);
                let accepted = principal.caller().clone();
                let view_ctx = ctx.clone();
                let value = ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
                        Box::pin(async move {
                            F::require_get(conn, reader, id).await?;
                            let prepared =
                                ready.projection.as_ref().ok_or_else(ApiError::not_found)?;
                            render::<F>(&view_ctx, conn, reader, prepared)
                                .await?
                                .pop()
                                .ok_or_else(ApiError::not_found)
                        })
                    })
                    .await?
                    .ok_or_else(disclosure_refused)?;
                Ok(Produced {
                    value,
                    opened: Some((opened, accepted)),
                })
            } else {
                F::require_get(conn, principal, id).await?;
                let prepared = F::prepare_get(ctx, conn, principal, id).await?;
                let value = render::<F>(ctx, conn, principal, &prepared)
                    .await?
                    .pop()
                    .ok_or_else(ApiError::not_found)?;
                Ok(Produced {
                    value,
                    opened: None,
                })
            }
        })
    })
    .await
}

async fn list<F: Family>(ctx: Ctx, query: F::ListQuery, (): ()) -> ApiResult<Vec<Listed<F::Item>>> {
    let retained_query = query.clone();
    let completed = crate::key_operations::snapshot_read::run(ctx, move |ctx| {
        snapshot_list::<F>(ctx, query.clone())
    })
    .await?;
    let produced = completed.value;
    if let Some((opened, accepted)) = produced.opened {
        retain::<F>(
            &completed.ctx,
            &accepted,
            opened,
            ReadKind::List(retained_query),
        )?;
    }
    // Hold either auxiliary outcome through the same original HTTP/all-key deadline cut.
    let result = F::after_list(&completed.ctx, &produced.value).await;
    completed.identity.check_deadline(&completed.ctx.app)?;
    result?;
    Ok(produced.value)
}

async fn snapshot_list<F: Family>(
    ctx: Ctx,
    query: F::ListQuery,
) -> ApiResult<Completed<Produced<Vec<Listed<F::Item>>, F>>> {
    native::read_once(ctx, move |ctx, conn, principal| {
        Box::pin(async move {
            if modern(principal.caller()) {
                let ready = ctx.operation_data::<Ready<F>>()?;
                let opened = Arc::clone(&ready);
                let accepted = principal.caller().clone();
                let view_ctx = ctx.clone();
                let value = ready
                    .response
                    .clone()
                    .read_operation(ctx, conn, principal.caller(), move |conn, reader| {
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
                    .ok_or_else(disclosure_refused)?;
                Ok(Produced {
                    value,
                    opened: Some((opened, accepted)),
                })
            } else {
                let plan = OperationPlan::new(ctx.route, ctx.route.name)?;
                let mut seen =
                    selected::<F>(conn, principal, principal.caller(), &plan, &query).await?;
                let private = super::hidden_items(conn, principal, &mut seen).await?;
                let prepared = projection::prepare(
                    ctx,
                    conn,
                    principal,
                    projection::Selection {
                        kind: F::SPEC.kind,
                        ids: seen.iter().map(|row| row.resource.id).collect(),
                        seen: &seen,
                    },
                )
                .await?;
                let items = match prepared {
                    Some(prepared) => render::<F>(ctx, conn, principal, &prepared).await?,
                    None => Vec::new(),
                };
                Ok(Produced {
                    value: super::listed(items, private),
                    opened: None,
                })
            }
        })
    })
    .await
}

/// Retain the producing native factory and its actual authority shape after snapshot success.
/// Refresh only effect-free preparation; no renderer, Get auxiliary supplier or writer repeats.
fn retain<F: Family>(
    ctx: &Ctx,
    accepted: &Caller,
    ready: Arc<Ready<F>>,
    kind: ReadKind<F::ListQuery>,
) -> ApiResult<()> {
    crate::key_operations::publication::retain(
        ctx,
        accepted,
        (ready, kind),
        |ctx, conn, reader, (opened, kind)| {
            Box::pin(async move {
                let plan = match kind {
                    ReadKind::Get => prepare_get::<F>(ctx, conn, reader.caller(), (), ()).await?,
                    ReadKind::List(query) => {
                        prepare_list::<F>(ctx, conn, reader.caller(), query.clone(), ()).await?
                    }
                }
                .ok_or_else(disclosure_refused)?;
                if !plan.shared().covers_sources(opened.response.shared()) {
                    return Err(disclosure_refused());
                }
                let admitted = plan.admit(reader.caller())?;
                if !admitted.content {
                    return Err(disclosure_refused());
                }
                let fresh = admitted.data.get::<Ready<F>>()?;
                if !opened.authority.iter().all(|(object, authority)| {
                    fresh
                        .authority
                        .get(object)
                        .is_some_and(|current| authority.preserved_by(current))
                }) {
                    return Err(disclosure_refused());
                }
                match (&opened.projection, &fresh.projection) {
                    (Some(old), Some(current)) if !old.preserves_sources(current) => {
                        return Err(disclosure_refused());
                    }
                    (Some(_), None) => return Err(disclosure_refused()),
                    _ => {}
                }
                let principal = crate::middleware::authorization::load_principal(
                    conn,
                    admitted.caller,
                    ctx.app.clock.time(),
                )
                .await?;
                let principal =
                    crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
                if let ReadKind::Get = kind {
                    F::require_get(conn, &principal, ctx.id("id")?).await?;
                }
                ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Full);
                if ctx.key_receipt_required() {
                    return Err(disclosure_refused());
                }
                Ok(())
            })
        },
    )
}

fn disclosure_refused() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the native resource response has not been admitted",
    )
}
