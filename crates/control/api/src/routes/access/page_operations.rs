//! Credential bounds around the existing permission-page and ownership writers.
//! The native page engine remains the only grant, ownership and sharing evaluator.

use super::{Which, operations, subject};
use crate::key_operations::targets;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::transaction_guard::{self, Guard};
use crate::registry::{Ctx, OperationFuture, OperationPlan};
use orch_access_contract::permissions::ObjectKind;
use orch_api_contract::access as api;
use orch_api_contract::common::ReceiptProjection;
use orch_authz::things::{self, Thing};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, Tx};
use std::future::Future;

/// Only the factual native target, never permission or a copy of the ACL decision.
struct PreparedTarget {
    thing: Thing,
    kind: ObjectKind,
    selection: String,
}

fn defaults(ctx: &Ctx) -> bool {
    ctx.route.name == "NodeDefaultsUpdate"
}
fn which(ctx: &Ctx) -> Which {
    if defaults(ctx) {
        Which::Defaults
    } else {
        Which::Live
    }
}
fn page_variant(body: &api::PageChange) -> &'static str {
    if body.owners.as_ref().is_some_and(|owners| {
        owners.first.is_some()
            || owners
                .add
                .as_ref()
                .is_some_and(|holders| !holders.is_empty())
            || owners
                .remove
                .as_ref()
                .is_some_and(|entries| !entries.is_empty())
    }) {
        "PermissionPageUpdate.owners"
    } else if body.whole.is_some()
        || body.rows.as_ref().is_some_and(|rows| {
            rows.iter()
                .any(|row| row.inherit.is_some() || row.locked.is_some())
        })
    {
        "PermissionPageUpdate.page"
    } else {
        "PermissionPageUpdate.holders"
    }
}

pub(super) fn update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    body: api::PageChange,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let thing = if defaults(ctx) {
            Thing::Node(ctx.id("id")?)
        } else {
            subject::named(ctx)?.thing
        };
        // Reuse the native path identity then wire translation. Notification
        // identifiers are intrinsic inputs, not independent privilege grants.
        let (_, people) = crate::routes::notifications::person::page_change(&body)?;
        if defaults(ctx) && !people.is_empty() {
            return Err(super::invalid(
                "notify",
                "is available on a thing's own permission page",
            ));
        }
        let (graph, site) = things::site(conn, thing).await.map_err(authz_error)?;
        if let Thing::Other(object) = thing {
            if object.kind != site.kind {
                return Err(ApiError::not_found());
            }
        }
        let target = targets::from_site(&graph, &site)?;
        let mut plan = OperationPlan::new(
            ctx.route,
            if defaults(ctx) {
                ctx.route.name
            } else {
                page_variant(&body)
            },
        )?;
        for index in [0, 1] {
            plan.bind(index, target.object.kind, std::slice::from_ref(&target))?;
        }
        let selection = plan.variant().operation.clone();
        Ok(plan.with_data(PreparedTarget {
            thing,
            kind: site.kind,
            selection,
        }))
    })
}
fn authz_error(error: orch_authz::Error) -> ApiError {
    match error {
        orch_authz::Error::NotFound(_) => ApiError::not_found(),
        other => authorization::restoration_error(other),
    }
}

/// Hold exactly the ordinary page target, then refresh the complete original HTTP
/// context once. All page/giver/policy/body decisions still run in the native writer.
pub(crate) async fn action(ctx: &Ctx) -> ApiResult<(Ctx, Tx, Guard)> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let prepared = initial.operation_data::<PreparedTarget>()?;
    if defaults(ctx) && prepared.kind == ObjectKind::Thread {
        // Preserve the original permission check then unsupported-kind refusal, without
        // adding a row wait to a request that never reaches the native page writer.
        let current = ctx
            .for_operation_checkpoint(initial.user().clone(), initial.now().get())
            .with_admitted_caller(initial.principal().caller().clone())?;
        return Ok((current, tx, initial));
    }
    let target = match prepared.thing {
        Thing::Node(id) => which(ctx).target(id),
        thing => thing.target(),
    };
    orch_authz::pages::lock_change_target(&mut tx, target)
        .await
        .map_err(authz_error)?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let fresh = guard.operation_data::<PreparedTarget>()?;
    if fresh.thing != prepared.thing || fresh.kind != prepared.kind {
        return Err(DbError::Retry.into());
    }
    Ok((current, tx, guard))
}

pub(crate) async fn projected<T>(
    ctx: Ctx,
    work: impl Future<Output = ApiResult<T>>,
) -> ApiResult<T> {
    authorization::projected_result(ctx, work, false).await
}

/// Capture only the native writer's actual accepted identity/revision while its
/// transaction still owns the effect. Later metadata failure needs no guessed revision.
pub(crate) fn acknowledged(ctx: &Ctx, revision: i64) -> ApiResult<api::PermissionPageWriteReceipt> {
    let prepared = ctx.operation_data::<PreparedTarget>()?;
    Ok(api::PermissionPageWriteReceipt {
        projection: ReceiptProjection::Receipt,
        object_id: prepared.thing.id(),
        kind: prepared.kind.as_str().to_owned(),
        revision,
        defaults: defaults(ctx),
    })
}

/// Only optional metadata follows a completed modern page change. The shared helper
/// maps trusted database retry to receipt and never retries this snapshot or writer.
pub(crate) async fn reply(
    ctx: &Ctx,
    principal: &Principal,
    receipt: api::PermissionPageWriteReceipt,
) -> ApiResult<api::PermissionPageWriteResponse> {
    if ctx.key_receipt_required()
        && ctx.disclosure() == crate::key_operations::resolver::Disclosure::Denied
    {
        return Ok(api::PermissionPageWriteResponse::Receipt(receipt));
    }
    let work = async {
        let mut tx = ctx.app.db.begin_snapshot().await?;
        let value = current_with_identity(ctx, &mut tx, principal).await?;
        tx.commit().await.map_err(DbError::from)?;
        match value {
            Some((page, identity)) => {
                identity.check_deadline(&ctx.app)?;
                Ok(Some(page))
            }
            None => Ok(None),
        }
    };
    let value = crate::key_operations::completed_projection::run(ctx, async {
        match work.await {
            Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(None),
            other => other,
        }
    })
    .await?;
    Ok(match value {
        Some(page) => api::PermissionPageWriteResponse::Full(Box::new(page)),
        None => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            api::PermissionPageWriteResponse::Receipt(receipt)
        }
    })
}

/// The same optional current page read is also used by an uncommitted stale refusal.
/// A stale refusal has no completed effect, so database retry keeps its normal transaction path.
pub(crate) async fn current(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Option<api::PermissionPage>> {
    let Some((page, identity)) = current_with_identity(ctx, conn, principal).await? else {
        return Ok(None);
    };
    if identity.check_deadline(&ctx.app).is_err() {
        ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
        return Ok(None);
    }
    Ok(Some(page))
}

async fn current_with_identity(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Option<(api::PermissionPage, transaction_guard::CurrentHttpIdentity)>> {
    if ctx.key_receipt_required()
        && ctx.disclosure() == crate::key_operations::resolver::Disclosure::Denied
    {
        return Ok(None);
    }
    match prepare_current(ctx, conn, principal).await {
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            Ok(None)
        }
        Ok(None) => {
            ctx.complete_disclosure(crate::key_operations::resolver::Disclosure::Denied);
            Ok(None)
        }
        other => other,
    }
}

async fn prepare_current(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
) -> ApiResult<Option<(api::PermissionPage, transaction_guard::CurrentHttpIdentity)>> {
    let prepared = ctx.operation_data::<PreparedTarget>()?;
    let Some(identity) = crate::key_operations::admission::response_caller(
        ctx,
        conn,
        principal.caller(),
        &prepared.selection,
        principal.now(),
    )
    .await?
    else {
        return Ok(None);
    };
    let current = ctx.for_operation_checkpoint(identity.user.clone(), identity.now.get());
    let (graph, site) = things::site(conn, prepared.thing)
        .await
        .map_err(authz_error)?;
    let policy = subject::policy_of(conn, prepared.thing, &site).await?;
    let mut plan = OperationPlan::new(ctx.route, &prepared.selection)?;
    let target = targets::from_site(&graph, &site)?;
    for index in [0, 1] {
        plan.bind(index, target.object.kind, std::slice::from_ref(&target))?;
    }
    let Some(reader) = response_principal(
        &current,
        conn,
        &identity,
        &plan,
        1,
        target.object.kind,
        &target,
    )
    .await?
    else {
        return Ok(None);
    };
    let current = current.with_admitted_caller(reader.caller().clone())?;
    subject::require_page(conn, &reader, prepared.thing, policy).await?;
    let (plan, projection) = operations::prepare_page_projection(
        &current,
        conn,
        &reader,
        prepared.thing,
        &site,
        policy,
        defaults(ctx),
        plan,
    )
    .await?;
    let value = plan
        .read_response(&current, conn, reader.caller(), move |conn, reader| {
            Box::pin(async move { operations::project_page(conn, reader, &projection).await })
        })
        .await?;
    Ok(value.map(|page| (page, identity)))
}

/// Prepare only the consuming operation's actual primary response Read. Identity
/// restoration deliberately carries no admission; completed Manage is never re-admitted.
pub(crate) async fn response_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    identity: &transaction_guard::CurrentHttpIdentity,
    plan: &OperationPlan,
    index: usize,
    family: orch_access_contract::key_scope::KeyTargetKind,
    target: &orch_access_contract::key_scope::KeyResolvedTarget,
) -> ApiResult<Option<Principal>> {
    use orch_access_contract::key_scope::KeyAction;
    use orch_api_contract::operations::FacetUsage;
    if !plan.matches_route(&ctx.route) {
        return Err(ApiError::internal("response Read changed its operation"));
    }
    let metadata = plan
        .variant()
        .facets
        .get(index)
        .ok_or_else(|| ApiError::internal("response Read slot is undeclared"))?;
    if metadata.usage != FacetUsage::ResponseContent || metadata.action != KeyAction::Read {
        return Err(ApiError::internal(
            "response preparation is not a declared Read",
        ));
    }
    if !plan.permits_binding(
        &identity.caller,
        index,
        family,
        std::slice::from_ref(target),
    )? {
        return Ok(None);
    }
    let facts = orch_engine::operation_scope::facets(
        &plan.variant().operation,
        metadata,
        family,
        std::slice::from_ref(target),
    )
    .map_err(crate::key_operations::admission::binding_error)?;
    let caller = identity
        .caller
        .admit_facets(&facts)
        .map_err(|_| ApiError::internal("response Read admission changed"))?
        .into_caller();
    let reader = authorization::load_principal(conn, caller, identity.now).await?;
    Ok(Some(
        crate::sign_in::second_step::hold(conn, &identity.user, reader).await?,
    ))
}

#[cfg(test)]
mod tests;
