//! Proposed and retained input operands use the same current native classifier as direct sends.

use super::bind_bypass;

use super::bind_native;

use super::destination;

use crate::key_operations::blob_sources;

use crate::key_operations::targets;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::OperationInitialFuture;

use crate::registry::OperationPlan;

use orch_access_contract::key_scope::KeyResolvedTarget;

use orch_access_contract::permissions::Permission;

use orch_api_contract::input_schedules::NativeInvocation;

use orch_api_contract::threads::ContentPart;

use orch_api_contract::threads::Delivery;

use orch_api_contract::threads::ThreadInputUpdateBody;

use orch_api_contract::threads::ThreadTurnRetryBody;

use orch_api_contract::threads::TurnState;

use orch_authz::Act;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::threads;

use orch_db::queries::threads::InputRow;

use orch_db::queries::threads::ThreadRow;

use orch_engine::inputs::InputError;

use orch_engine::inputs::InputSourceOperands;
/// Actual source content and proof retained by the guarded retry producer.
pub(in crate::routes::threads) struct TurnRetryOperands {
    pub content: Vec<ContentPart>,
    pub sources: InputSourceOperands,
}

async fn base(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(ThreadRow, KeyResolvedTarget, OperationPlan, Principal)> {
    let id = ctx.id("id")?;
    let thread = threads::thread(conn, id)
        .await?
        .filter(|thread| thread.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    let target = targets::node(conn, id).await?;
    let plan = destination(ctx, &target)?;
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
    if thread.archived_at.is_some() {
        return Err(super::super::input_failed_projected(
            conn,
            id,
            InputError::Archived,
            ctx.key_receipt_required(),
        )
        .await);
    }
    Ok((thread, target, plan, principal))
}

async fn retained(ctx: &Ctx, conn: &mut Conn, thread: &ThreadRow) -> ApiResult<InputRow> {
    threads::input(conn, ctx.id("input_id")?)
        .await?
        .filter(|input| input.thread_id == thread.node_id)
        .ok_or_else(ApiError::not_found)
}

fn selection(input: &InputRow) -> ApiResult<Option<NativeInvocation>> {
    input
        .native_action
        .clone()
        .map(serde_json::from_value)
        .transpose()
        .map_err(ApiError::internal)
}

fn delivery(input: &InputRow) -> Delivery {
    match input.delivery.as_str() {
        "steer" => Delivery::Steer,
        "interrupt" => Delivery::Interrupt,
        _ => Delivery::Queue,
    }
}

fn update_selection(
    input: &InputRow,
    body: &ThreadInputUpdateBody,
) -> ApiResult<Option<NativeInvocation>> {
    match &body.native_action {
        Some(selected) => Ok(selected.clone()),
        None => selection(input),
    }
}

fn update_uses_native(body: &ThreadInputUpdateBody) -> bool {
    // edit_input_with validates the selected action only when selection or delivery
    // is supplied. Reordering and content-only edits retain the existing selector.
    body.native_action.is_some() || body.delivery.is_some()
}

fn update_author(caller: &Caller, input: &InputRow, body: &ThreadInputUpdateBody) -> ApiResult<()> {
    if (body.content.is_some() || body.native_action.is_some())
        && input.author_id != Some(caller.user())
    {
        return Err(super::super::author_only(input.thread_id));
    }
    Ok(())
}

fn retry_author(ctx: &Ctx, caller: &Caller, thread: &ThreadRow, input: &InputRow) -> ApiResult<()> {
    if ctx.route.name == "ThreadInputRetry"
        && input.author_id.unwrap_or(thread.owner_id) != caller.user()
    {
        return Err(super::super::author_only(input.thread_id));
    }
    Ok(())
}

async fn bind_selected(
    plan: &mut OperationPlan,
    conn: &mut Conn,
    target: &KeyResolvedTarget,
    thread: orch_types::Id,
    selected: Option<&NativeInvocation>,
    delivery: Delivery,
) -> ApiResult<()> {
    let permission = match selected {
        Some(selected) => {
            match orch_engine::native_actions::selected_action(conn, thread, selected, delivery)
                .await
            {
                Ok(action) => Some(action.permission),
                Err(error) => return Err(super::super::native_failed(conn, error).await),
            }
        }
        None => None,
    };
    bind_native(plan, target, permission)
}

pub(in crate::routes::threads) fn initial_update<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    body: ThreadInputUpdateBody,
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        let refresh = {
            let mut conn = ctx.app.db.acquire().await?;
            let (thread, _, _, principal) = base(ctx, &mut conn, caller).await?;
            let input = retained(ctx, &mut conn, &thread).await?;
            update_author(caller, &input, &body)?;
            if update_uses_native(&body) && update_selection(&input, &body)?.is_some() {
                Some((thread.node_id, principal.caller().clone()))
            } else {
                None
            }
        };
        if let Some((thread, caller)) = refresh {
            let initial = ctx.clone().with_admitted_caller(caller)?;
            super::super::native::refresh_for_input(&initial, thread).await?;
        }
        Ok(())
    })
}

pub(in crate::routes::threads) fn initial_retained<'a>(
    ctx: &'a Ctx,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationInitialFuture<'a> {
    Box::pin(async move {
        let refresh = {
            let mut conn = ctx.app.db.acquire().await?;
            let (thread, _, _, principal) = base(ctx, &mut conn, caller).await?;
            let input = retained(ctx, &mut conn, &thread).await?;
            retry_author(ctx, caller, &thread, &input)?;
            selection(&input)?
                .is_some()
                .then(|| (thread.node_id, principal.caller().clone()))
        };
        if let Some((thread, caller)) = refresh {
            let initial = ctx.clone().with_admitted_caller(caller)?;
            super::super::native::refresh_for_input(&initial, thread).await?;
        }
        Ok(())
    })
}

pub(in crate::routes::threads) fn prepare_update<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ThreadInputUpdateBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (thread, target, mut plan, _) = base(ctx, conn, caller).await?;
        let input = retained(ctx, conn, &thread).await?;
        update_author(caller, &input, &body)?;
        if update_uses_native(&body) {
            let selected = update_selection(&input, &body)?;
            bind_selected(
                &mut plan,
                conn,
                &target,
                thread.node_id,
                selected.as_ref(),
                body.delivery.unwrap_or_else(|| delivery(&input)),
            )
            .await?;
        } else {
            bind_native(&mut plan, &target, None)?;
        }
        // Only newly supplied words can introduce copied bytes. Reorder/delivery/native-only
        // changes do not acquire fabricated source reads from the unchanged stored content.
        let hashes = body
            .content
            .as_deref()
            .map(orch_chat::following::content_blobs)
            .unwrap_or_default();
        let sources = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
        blob_sources::bind(&mut plan, &sources.bindings)?;
        Ok(plan)
    })
}

pub(in crate::routes::threads) fn prepare_retained<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (thread, target, mut plan, _) = base(ctx, conn, caller).await?;
        let input = retained(ctx, conn, &thread).await?;
        retry_author(ctx, caller, &thread, &input)?;
        let selected = selection(&input)?;
        bind_selected(
            &mut plan,
            conn,
            &target,
            thread.node_id,
            selected.as_ref(),
            delivery(&input),
        )
        .await?;
        Ok(plan)
    })
}

pub(in crate::routes::threads) fn prepare_turn_retry<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: ThreadTurnRetryBody,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let (thread, target, mut plan, _) = base(ctx, conn, caller).await?;
        let turn = threads::turn(conn, ctx.id("turn_id")?)
            .await?
            .filter(|turn| turn.thread_id == thread.node_id)
            .ok_or_else(ApiError::not_found)?;
        if !matches!(
            serde_json::from_value(serde_json::Value::String(turn.state)),
            Ok(TurnState::Failed | TurnState::Interrupted)
        ) {
            return Err(ApiError::invalid(
                "turn_id",
                "only a turn that failed or was stopped is tried again",
            ));
        }
        let input = threads::input(conn, turn.input_id)
            .await?
            .filter(|input| input.thread_id == thread.node_id)
            .ok_or_else(ApiError::not_found)?;
        let content: Vec<ContentPart> =
            serde_json::from_value(input.content).map_err(ApiError::internal)?;
        // The existing retry copies normal content, not the original native invocation.
        bind_native(&mut plan, &target, None)?;
        let replayed =
            threads::input_by_client(conn, thread.node_id, Some(caller.user()), &body.client_id)
                .await?;
        let bypass_subjects = if replayed.is_some() {
            Vec::new()
        } else {
            match body.bypass.as_ref() {
                Some(asked) => match ctx
                    .app
                    .engine
                    .input_bypass_subjects(conn, &thread, caller.user(), asked, ctx.app.clock.now())
                    .await
                {
                    Ok(homes) => homes,
                    Err(error) => {
                        return Err(super::super::input_failed_projected(
                            conn,
                            thread.node_id,
                            error,
                            ctx.key_receipt_required(),
                        )
                        .await);
                    }
                },
                None => Vec::new(),
            }
        };
        bind_bypass(&mut plan, conn, &bypass_subjects).await?;
        let hashes = if replayed.is_some() {
            Vec::new()
        } else {
            orch_chat::following::content_blobs(&content)
        };
        let sources = blob_sources::prepare(ctx, conn, caller, &plan, &hashes, &[]).await?;
        blob_sources::bind(&mut plan, &sources.bindings)?;
        Ok(plan.with_data(TurnRetryOperands {
            content,
            sources: InputSourceOperands {
                blobs: sources.selected,
                replayed: replayed.map(|input| input.id),
                bypass_subjects,
                native_permission: None,
            },
        }))
    })
}
