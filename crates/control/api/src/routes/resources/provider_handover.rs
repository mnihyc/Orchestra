//! The existing custody-return alternatives, with exact operation and response sources.

use super::provider_projection::NAMES;
use super::provider_runtime::not_home;
use crate::key_operations::{admission::Prepared, resolver::Disclosure, targets};
use crate::middleware::{
    authorization,
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, OperationFuture};
use crate::routes::access::operations::projection_names;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::{KeyAction, KeyFacet, KeyResolvedTarget, KeyTargetKind};
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::common::ErrorCode;
use orch_api_contract::operations::FacetUsage;
use orch_api_contract::resources::{self as api, Provider};
use orch_authz::{Caller, Policy};
use orch_db::queries::resources::provider_runtime as rows;
use orch_db::{Conn, DbError};
use orch_engine::credentials::handover::ReturnError;
use orch_engine::credentials::handover_authority::HandoverAuthority;
use orch_types::{Id, Time};

const PROVIDER: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Provider);
const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const PROVIDER_SELECTION: &str = "ProviderHandoverReturn.provider";
const HOLDER_SELECTION: &str = "ProviderHandoverReturn.holding_environment";

/// These are guarded operands, never serialized authority or caller-supplied targets.
#[derive(Clone)]
struct ReturnPrepared {
    provider: Id,
    holder: Option<Id>,
    selection: &'static str,
    interrupt: bool,
    declare_lost: bool,
}

fn scope_refusal() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "the key does not allow this operation",
    )
}

fn permission_refusal(error: &ApiError) -> bool {
    matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound)
}

fn projected_error(ctx: &Ctx, error: ApiError) -> ApiError {
    if ctx.key_receipt_required() {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

async fn provider_plan(
    ctx: &Ctx,
    conn: &mut Conn,
    provider: Id,
    selection: &str,
) -> ApiResult<Prepared> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Provider,
            id: provider,
        },
    )
    .await?;
    provider_plan_at(ctx, selection, &target)
}

fn provider_plan_at(ctx: &Ctx, selection: &str, target: &KeyResolvedTarget) -> ApiResult<Prepared> {
    let mut plan = Prepared::new(ctx.route, selection)?;
    let indices: Vec<_> = plan
        .variant()
        .facets
        .iter()
        .enumerate()
        .filter(|(_, facet)| facet.resources == [PROVIDER])
        .map(|(index, _)| index)
        .collect();
    for index in indices {
        plan.bind(index, PROVIDER, std::slice::from_ref(target))?;
    }
    Ok(plan)
}

/// Try only a complete factual candidate. A key's chosen ceiling does not grant
/// either ordinary permission, and lacking one selection does not erase the other.
async fn candidate(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    provider: &KeyResolvedTarget,
    holder: Option<Id>,
    selection: &'static str,
) -> ApiResult<Option<Prepared>> {
    if !caller.permits_operation(selection) {
        return Ok(None);
    }
    let mut plan = provider_plan_at(ctx, selection, provider)?;
    if selection == HOLDER_SELECTION {
        let holder = holder.ok_or_else(scope_refusal)?;
        let environment = targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: holder,
            },
        )
        .await?;
        plan.bind(0, ENVIRONMENT, &[environment])?;
    }
    let required = plan
        .shared()
        .required_facets()
        .map_err(crate::key_operations::admission::binding_error)?;
    if !caller.permits_facets(&required) {
        return Ok(None);
    }
    // An unselected alternative cannot permanently narrow the selected one's
    // optional response. Its private disclosure state still bounds its errors.
    let candidate_ctx = ctx.for_id(provider.object.id.ok_or_else(ApiError::not_found)?);
    let principal = plan
        .preparation_principal(&candidate_ctx, conn, caller)
        .await
        .map_err(|error| projected_error(&candidate_ctx, error))?;
    let (kind, id, permission) = if selection == PROVIDER_SELECTION {
        (
            ResourceKind::Provider,
            provider.object.id.ok_or_else(ApiError::not_found)?,
            Permission::ProviderManage,
        )
    } else {
        (
            ResourceKind::Environment,
            holder.ok_or_else(scope_refusal)?,
            Permission::EnvironmentManage,
        )
    };
    authorization::require_resource(conn, &principal, kind, id, permission, Policy::default())
        .await
        .map_err(|error| projected_error(&candidate_ctx, error))?;
    Ok(Some(plan))
}

/// The same typed producer runs before dispatch and again in the actual action guard.
pub(super) fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: api::ProviderHandoverReturnBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let provider = ctx.id("id")?;
        let state = rows::state(conn, provider)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or_else(ApiError::not_found)?;
        let target = targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Provider,
                id: provider,
            },
        )
        .await?;
        let declare_lost = body.declare_lost.unwrap_or(false);
        let selections = if !declare_lost && state.held_by_environment_id.is_some() {
            &[PROVIDER_SELECTION, HOLDER_SELECTION][..]
        } else {
            &[PROVIDER_SELECTION][..]
        };
        let mut refusal = None;
        for selection in selections {
            match candidate(
                ctx,
                conn,
                caller,
                &target,
                state.held_by_environment_id,
                selection,
            )
            .await
            {
                Ok(Some(plan)) => {
                    return Ok(plan.with_data(ReturnPrepared {
                        provider,
                        holder: state.held_by_environment_id,
                        selection,
                        interrupt: body.interrupt.unwrap_or(false),
                        declare_lost,
                    }));
                }
                Ok(None) => {}
                Err(error) if permission_refusal(&error) => {
                    refusal.get_or_insert(error);
                }
                Err(error) => return Err(projected_error(ctx, error)),
            }
        }
        Err(projected_error(ctx, refusal.unwrap_or_else(scope_refusal)))
    })
}

/// Brings custody home under its accepted variant; response Read cannot undo the act.
pub(super) async fn return_credential(
    ctx: Ctx,
    (): (),
    body: api::ProviderHandoverReturnBody,
) -> ApiResult<api::ProviderUpdateResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let prepared = guard.operation_data::<ReturnPrepared>()?;
    let principal = guard.into_principal();
    if prepared.provider != ctx.id("id")?
        || principal.caller().admitted_operation() != Some(prepared.selection)
        || prepared.interrupt != body.interrupt.unwrap_or(false)
        || prepared.declare_lost != body.declare_lost.unwrap_or(false)
    {
        return Err(ApiError::internal("the guarded return operands changed"));
    }
    let authority = HandoverAuthority::capture(
        &mut tx,
        principal.caller(),
        prepared.provider,
        prepared.interrupt,
        prepared.declare_lost,
        principal.now(),
    )
    .await
    .map_err(|error| projected_error(&ctx, not_home(error)))?;
    if prepared.selection == HOLDER_SELECTION && prepared.holder != Some(authority.holder()) {
        return Err(projected_error(
            &ctx,
            ApiError::new(
                ErrorCode::ConflictStaleRevision,
                "the credential's holding environment changed",
            ),
        ));
    }
    let returned = ctx
        .app
        .engine
        .return_credential_operation_in(
            tx,
            &principal,
            &crate::sign_in::origin(&ctx),
            prepared.provider,
            prepared.interrupt,
            prepared.declare_lost,
            authority.clone(),
        )
        .await;
    let revision = match returned {
        Ok(revision) => revision,
        Err(error) => {
            return Err(project_engine_error(&ctx, &authority, principal.caller(), error).await);
        }
    };
    let receipt = api::ProviderUpdateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        provider_id: prepared.provider,
        revision,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    let shown = crate::key_operations::completed_projection::run(&ctx, async {
        let mut reading = ctx.app.db.begin_snapshot().await?;
        let shown = read_response(&ctx, &mut reading, &authority).await?;
        reading.commit().await.map_err(DbError::from)?;
        Ok(shown)
    })
    .await?;
    Ok(match shown {
        Some(provider) => api::ProviderUpdateResponse::full(api::ProviderWriteReply {
            provider,
            warning: None,
        }),
        None => api::ProviderUpdateResponse::Receipt(receipt),
    })
}

/// A Busy count has only the Provider's primary Read source. Unresolved Group
/// or Node names for a full DTO must neither disclose nor hide this count.
async fn project_engine_error(
    ctx: &Ctx,
    authority: &HandoverAuthority,
    accepted: &Caller,
    error: ReturnError,
) -> ApiError {
    let problem = not_home(error);
    let sparse = authorization::problem_without_content(problem.clone());
    if let Err(error) = crate::key_operations::publication::refusal(ctx, &sparse) {
        return error;
    }
    if problem.code() == ErrorCode::ThreadNotIdle {
        if let Ok(Some(primary)) = busy_count_read(ctx, authority).await {
            let mut selected = accepted.admitted_facets().unwrap_or(&[]).to_vec();
            selected.extend(primary);
            crate::key_operations::activity::selected(authority.origin().operation(), &selected);
            return problem;
        }
        return authorization::problem_without_content(problem);
    }
    projected_error(ctx, problem)
}

async fn busy_count_read(
    ctx: &Ctx,
    authority: &HandoverAuthority,
) -> ApiResult<Option<Vec<KeyFacet>>> {
    let mut reading = ctx.app.db.begin_snapshot().await?;
    let Some(principal) = authority
        .authorize_response(&mut reading, Time::new(ctx.app.clock.now()))
        .await
        .map_err(ApiError::engine)?
    else {
        return Ok(None);
    };
    let primary: Vec<_> = principal
        .caller()
        .admitted_facets()
        .unwrap_or(&[])
        .iter()
        .filter(|facet| {
            facet.operation == authority.origin().operation()
                && facet.action == KeyAction::Read
                && facet.resource_kind == PROVIDER
                && facet.target.object.kind == PROVIDER
                && facet.target.object.id == Some(authority.provider())
        })
        .cloned()
        .collect();
    if primary.len() != 1 {
        return Ok(None);
    }
    reading.commit().await.map_err(DbError::from)?;
    Ok(Some(primary))
}

async fn read_response(
    ctx: &Ctx,
    conn: &mut Conn,
    authority: &HandoverAuthority,
) -> ApiResult<Option<Provider>> {
    if ctx.disclosure() == Disclosure::Denied {
        return Ok(None);
    }
    match read_response_inner(ctx, conn, authority).await {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

async fn read_response_inner(
    ctx: &Ctx,
    conn: &mut Conn,
    authority: &HandoverAuthority,
) -> ApiResult<Option<Provider>> {
    let now = Time::new(ctx.app.clock.now());
    let Some(principal) = authority
        .authorize_response(conn, now)
        .await
        .map_err(ApiError::engine)?
    else {
        ctx.complete_disclosure(Disclosure::Denied);
        return Ok(None);
    };
    let provider = authority.provider();
    let selection = authority.origin().operation();
    let mut plan = provider_plan(ctx, conn, provider, selection).await?;
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
        .find(|facet| facet.resources == [PROVIDER] && facet.usage == FacetUsage::ResponseContent)
        .ok_or_else(|| ApiError::internal("the return response Read facet is absent"))?;
    let primary = orch_engine::operation_scope::facets(selection, primary, PROVIDER, &[target])
        .map_err(crate::key_operations::admission::binding_error)?;
    let reader = match principal.caller().admit_facets(&primary) {
        Ok(admitted) => admitted.into_caller(),
        Err(_) => {
            ctx.complete_disclosure(Disclosure::Denied);
            return Ok(None);
        }
    };
    let principal = authorization::load_principal(conn, reader, now).await?;
    let principal = crate::sign_in::second_step::hold(conn, ctx.user()?, principal).await?;
    let mut prepared =
        super::providers::prepare_handover_projection(ctx, conn, &principal, provider).await?;
    prepared.projection.bounds = projection_names(
        conn,
        principal.caller(),
        &mut plan,
        prepared.projection.sources.clone(),
        &NAMES,
    )
    .await?;
    let projection = ctx.clone();
    plan.read_response(ctx, conn, principal.caller(), move |conn, reader| {
        Box::pin(async move {
            super::providers::show_prepared(&projection, conn, reader, prepared).await
        })
    })
    .await
}
