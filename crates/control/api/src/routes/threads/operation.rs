//! Direct-input source facts resolved beside the existing input/native/bypass producers.

mod retained;
pub(super) use retained::{
    TurnRetryOperands, initial_retained, initial_update, prepare_retained, prepare_turn_retry,
    prepare_update,
};

use crate::key_operations::{blob_sources, targets};
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, OperationFuture, OperationInitialFuture, OperationPlan};
use orch_access_contract::key_scope::KeyResolvedTarget;
use orch_access_contract::permissions::Permission;
use orch_api_contract::native::NativePermission;
use orch_api_contract::threads::ThreadInputCreateBody;
use orch_authz::caps::Home;
use orch_authz::{Act, Caller, Policy};
use orch_db::Conn;
use orch_db::queries::threads;
use orch_engine::inputs::{InputError, InputSourceOperands};

fn destination(ctx: &Ctx, target: &KeyResolvedTarget) -> ApiResult<OperationPlan> {
    let mut plan = OperationPlan::new(ctx.route, ctx.route.name)?;
    orch_engine::inputs::operation::bind_thread(plan.shared_mut(), target)
        .map_err(crate::key_operations::admission::binding_error)?;
    Ok(plan)
}

fn bind_native(
    plan: &mut OperationPlan,
    target: &KeyResolvedTarget,
    permission: Option<NativePermission>,
) -> ApiResult<()> {
    orch_engine::inputs::operation::bind_native(plan.shared_mut(), target, permission)
        .map_err(crate::key_operations::admission::binding_error)
}

async fn bind_bypass(plan: &mut OperationPlan, conn: &mut Conn, homes: &[Home]) -> ApiResult<()> {
    use orch_engine::inputs::operation::InputOperationError;
    use orch_engine::operation_scope::targets::TargetError;
    match orch_engine::inputs::operation::bind_bypass(plan.shared_mut(), conn, homes).await {
        Ok(()) => Ok(()),
        Err(InputOperationError::Binding(error)) => {
            Err(crate::key_operations::admission::binding_error(error))
        }
        Err(InputOperationError::Target(TargetError::Missing)) => Err(ApiError::not_found()),
        Err(InputOperationError::Target(TargetError::Db(error))) => Err(error.into()),
        Err(InputOperationError::Target(TargetError::Authz(error))) => {
            Err(crate::middleware::authorization::refusals::problem(conn, error).await)
        }
    }
}

/// Refreshes an actual native selector only after its known input authority is admitted.
/// Registry invokes this phase outside every mutation transaction; its connection is dropped
/// before the existing native refresh can issue RPCs. Replay never inspects replacement effects.
pub(super) fn initial<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    body: ThreadInputCreateBody,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        if body.native_action.is_none() {
            return Ok(());
        }
        let id = ctx.id("id")?;
        let prepared_ctx = {
            let mut conn = ctx.app.db.acquire().await?;
            let target = targets::node(&mut conn, id).await?;
            let base = destination(ctx, &target)?;
            let principal = base.preparation_principal(ctx, &mut conn, caller).await?;
            authorization::require(
                &mut conn,
                &principal,
                id,
                authorization::exact(Permission::ThreadInteract),
                Act::Use,
                Policy::default(),
            )
            .await?;
            if threads::input_by_client(&mut conn, id, Some(caller.user()), &body.client_id)
                .await?
                .is_some()
            {
                return Ok(());
            }
            ctx.clone()
                .with_admitted_caller(principal.caller().clone())?
        };
        // The original helper also rechecks ordinary Interact, releases its short connection,
        // then refreshes the existing native session. Final planning and its guard run afterwards.
        super::native::refresh_for_input(&prepared_ctx, id).await
    })
}

pub(super) fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ThreadInputCreateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let row = threads::thread(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if row.deleted_at.is_some() {
            return Err(ApiError::not_found());
        }
        let target = targets::node(conn, id).await?;
        let mut plan = destination(ctx, &target)?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        authorization::require(
            conn,
            &principal,
            id,
            authorization::exact(Permission::ThreadInteract),
            Act::Use,
            Policy::default(),
        )
        .await?;
        if row.archived_at.is_some() {
            return Err(super::input_failed_projected(
                conn,
                id,
                InputError::Archived,
                ctx.key_receipt_required(),
            )
            .await);
        }
        // The ordinary writer finds prior acceptance before source checks. Replacement body blobs
        // and requested bypasses perform no fresh effect on this path.
        if let Some(previous) =
            threads::input_by_client(conn, id, Some(caller.user()), &body.client_id).await?
        {
            bind_native(&mut plan, &target, None)?;
            bind_bypass(&mut plan, conn, &[]).await?;
            blob_sources::bind(&mut plan, &[])?;
            return Ok(plan.with_data(InputSourceOperands {
                blobs: Vec::new(),
                replayed: Some(previous.id),
                bypass_subjects: Vec::new(),
                native_permission: None,
            }));
        }
        let native_permission = match body.native_action.as_ref() {
            Some(selected) => match orch_engine::native_actions::selected_action(
                conn,
                id,
                selected,
                body.delivery,
            )
            .await
            {
                Ok(action) => Some(action.permission),
                Err(error) => return Err(super::native_failed(conn, error).await),
            },
            None => None,
        };
        bind_native(&mut plan, &target, native_permission)?;
        let bypass_subjects = match body.bypass.as_ref() {
            Some(asked) => match ctx
                .app
                .engine
                .input_bypass_subjects(conn, &row, caller.user(), asked, ctx.app.clock.now())
                .await
            {
                Ok(homes) => homes,
                Err(error) => {
                    return Err(super::input_failed_projected(
                        conn,
                        id,
                        error,
                        ctx.key_receipt_required(),
                    )
                    .await);
                }
            },
            None => Vec::new(),
        };
        bind_bypass(&mut plan, conn, &bypass_subjects).await?;
        let hashes = orch_chat::following::content_blobs(&body.content);
        let prepared = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
        blob_sources::bind(&mut plan, &prepared.bindings)?;
        Ok(plan.with_data(InputSourceOperands {
            blobs: prepared.selected,
            replayed: None,
            bypass_subjects,
            native_permission,
        }))
    })
}

/// Direct Plan decisions reuse the same native policy/typed Thread subject as deferred input.
pub(super) fn prepare_plan<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    _body: orch_api_contract::threads::ThreadPlanDecideBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let (shared, permission) =
            match orch_engine::inputs::operation::plan_decision(conn, id).await {
                Ok(value) => value,
                Err(error) => {
                    return Err(super::input_failed_projected(
                        conn,
                        id,
                        error,
                        ctx.key_receipt_required(),
                    )
                    .await);
                }
            };
        let plan = OperationPlan::from_shared(shared);
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        authorization::require(
            conn,
            &principal,
            id,
            authorization::exact(permission),
            Act::Use,
            Policy::default(),
        )
        .await?;
        Ok(plan)
    })
}

/// Actual applicable/reached cap homes, selected by the same native bypass producer.
pub(super) fn prepare_bypass<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: orch_api_contract::threads::LimitBypassRequest,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let id = ctx.id("id")?;
        let thread = threads::thread(conn, id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let target = targets::node(conn, id).await?;
        let mut plan = destination(ctx, &target)?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        authorization::require(
            conn,
            &principal,
            id,
            authorization::exact(Permission::ThreadInteract),
            Act::Use,
            Policy::default(),
        )
        .await?;
        let input = threads::input(conn, ctx.id("input_id")?)
            .await?
            .filter(|row| row.thread_id == id)
            .ok_or_else(ApiError::not_found)?;
        let homes = match input.author_id {
            Some(author) => match ctx
                .app
                .engine
                .input_bypass_subjects(conn, &thread, author, &body, ctx.app.clock.now())
                .await
            {
                Ok(homes) => homes,
                Err(error) => {
                    return Err(super::input_failed_projected(
                        conn,
                        id,
                        error,
                        ctx.key_receipt_required(),
                    )
                    .await);
                }
            },
            None => Vec::new(),
        };
        bind_bypass(&mut plan, conn, &homes).await?;
        Ok(plan.with_data(homes))
    })
}
