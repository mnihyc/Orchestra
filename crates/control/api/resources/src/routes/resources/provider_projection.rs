//! Current Provider projections behind the same operation's declared content facets.
//! S02 owns the shared source walkers/binder and ordinary bounded name projections.

use crate::key_operations::admission::Prepared;

use crate::key_operations::resolver::Disclosure;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::routes::access::operations::projection_names;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::operations::TargetResolver;

use orch_authz::Caller;

use orch_authz::CapturedCaller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_types::Id;

use orch_types::Time;

pub use orch_api_base::routes::resources::provider_projection::NAMES;
const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);

#[derive(Clone, Copy)]
pub(super) enum Purpose {
    Mutation,
    CompletedContent,
    Replay,
}

fn unavailable(error: &ApiError) -> bool {
    matches!(error.problem.status, 401 | 403 | 404 | 410)
}

fn named(plan: &Prepared) -> bool {
    plan.variant().facets.iter().any(|facet| {
        NAMES
            .groups
            .is_some_and(|group| facet.target == TargetResolver::ResolvedCollection(group))
    })
}

async fn base(ctx: &Ctx, conn: &mut Conn, id: Id) -> ApiResult<Prepared> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id,
        },
    )
    .await?;
    let mut plan = Prepared::new(ctx.route, ctx.route.name)?;
    crate::routes::resources::context::empty(&mut plan)?;
    let indices: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.resources == [PROVIDER])
        .map(|(index, _)| index)
        .collect();
    for index in indices {
        plan.bind(index, PROVIDER, std::slice::from_ref(&target))?;
    }
    Ok(plan)
}

/// ProviderGet prepares the actual read snapshot. Mutations admit only their
/// existing effects now; response source sets are resolved after the accepted write.
pub(super) fn prepare<'a, Q, B>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    _query: Q,
    _body: B,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let mut plan = base(ctx, conn, id).await?;
        if ctx.route.name == "ProviderGet" {
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            let mut prepared =
                super::providers::prepare_projection(ctx, conn, &principal, id).await?;
            prepared.projection.bounds = projection_names(
                conn,
                caller,
                &mut plan,
                prepared.projection.sources.clone(),
                &NAMES,
            )
            .await?;
            Ok(plan.with_data(prepared))
        } else {
            Ok(plan)
        }
    })
}

/// Complete the existing mutation's optional content without rerunning its effects.
/// Actual ProviderManage remains the ordinary response authority, preserving exact
/// legacy provider.manage behavior; explicit operation Read is an additional bound.
pub(super) async fn read<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    read_current(ctx, conn, accepted, provider, Purpose::Mutation, project).await
}

/// The same current Provider projection for a recorded creation and a new credential.
/// Only this operation's actual response Read is admitted, never its spent source roles.
pub(super) async fn read_completed_replay<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    read_current(ctx, conn, accepted, provider, Purpose::Replay, project).await
}

async fn read_current<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    purpose: Purpose,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    let result = read_inner(ctx, conn, accepted, provider, purpose, project).await;
    match result {
        Err(error) if unavailable(&error) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

async fn read_inner<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    purpose: Purpose,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    let plan = base(ctx, conn, provider).await?;
    read_plan_inner(ctx, conn, accepted, provider, plan, purpose, project).await
}

/// A creator supplies its original home/path footprint and actual new-provider Read target;
/// the same bounded provider source/name producer completes its optional content.
pub(super) async fn read_plan<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    plan: Prepared,
    purpose: Purpose,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    match read_plan_inner(ctx, conn, accepted, provider, plan, purpose, project).await {
        Err(error) if unavailable(&error) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

async fn read_plan_inner<T, F>(
    ctx: &Ctx,
    conn: &mut Conn,
    accepted: &Caller,
    provider: Id,
    mut plan: Prepared,
    purpose: Purpose,
    project: F,
) -> ApiResult<Option<T>>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    #[cfg(any(test, feature = "test-support"))]
    crate::key_operations::completed_projection_tests::before_read(ctx, accepted, provider).await;
    let mut prepared = None;
    if named(&plan) {
        let now = Time::new(ctx.app.clock.now());
        let Some(current) = CapturedCaller::capture(accepted)
            .restore(conn, now)
            .await
            .map_err(authorization::restoration_error)?
        else {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        };
        // Only the known primary Read is admitted for effect-free source preparation.
        // Missing Group payloads cannot be projected until the complete Read below.
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
        )
        .await?;
        let primary = plan
            .variant()
            .facets
            .iter()
            .find(|facet| {
                facet.resources == [PROVIDER] && facet.usage == FacetUsage::ResponseContent
            })
            .ok_or_else(|| ApiError::internal("the provider response Read facet is absent"))?;
        let facts =
            orch_engine::operation_scope::facets(ctx.route.name, primary, PROVIDER, &[target])
                .map_err(crate::key_operations::admission::binding_error)?;
        let reader = match current.admit_facets(&facts) {
            Ok(admitted) => admitted.into_caller(),
            Err(_) => {
                ctx.complete_disclosure(Disclosure::Denied);
                return Ok(None);
            }
        };
        let principal = authorization::load_principal(conn, reader, now).await?;
        let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
        let mut view =
            super::providers::prepare_projection(ctx, conn, &principal, provider).await?;
        view.projection.bounds = projection_names(
            conn,
            principal.caller(),
            &mut plan,
            view.projection.sources.clone(),
            &NAMES,
        )
        .await?;
        prepared = Some(view);
    }
    let response = match purpose {
        Purpose::Mutation => None,
        Purpose::CompletedContent => {
            Some(crate::key_operations::admission::ResponseProjection::CompletedContent)
        }
        Purpose::Replay => {
            let indices = plan
                .variant()
                .facets
                .iter()
                .enumerate()
                .filter_map(|(index, facet)| {
                    (facet.usage == FacetUsage::ResponseContent).then_some(index)
                })
                .collect();
            Some(crate::key_operations::admission::ResponseProjection::Replay(indices))
        }
    };
    if let Some(response) = response {
        plan.read_response_projection(ctx, conn, accepted, response, move |conn, reader| {
            Box::pin(render_provider(conn, reader, provider, prepared, project))
        })
        .await
    } else {
        plan.read_response(ctx, conn, accepted, move |conn, reader| {
            Box::pin(render_provider(conn, reader, provider, prepared, project))
        })
        .await
    }
}

async fn render_provider<T, F>(
    conn: &mut Conn,
    reader: &Principal,
    provider: Id,
    prepared: Option<super::providers::PreparedProjection>,
    project: F,
) -> ApiResult<T>
where
    T: Send + 'static,
    F: for<'a> FnOnce(
            &'a mut Conn,
            &'a Principal,
            Option<super::providers::PreparedProjection>,
        ) -> futures::future::BoxFuture<'a, ApiResult<T>>
        + Send
        + 'static,
{
    super::require(
        conn,
        reader,
        ResourceKind::Provider,
        provider,
        Permission::ProviderManage,
    )
    .await?;
    project(conn, reader, prepared).await
}
