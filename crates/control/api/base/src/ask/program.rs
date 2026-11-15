//! The built-in program uses ordinary thread input and Stop for permanent threads.

use super::*;

use crate::registry::Answer;

use orch_api_contract::threads::Delivery;

use orch_api_contract::threads::ThreadInputCreateBody;

use orch_api_contract::threads::ThreadInputCreateReply;

use orch_api_contract::threads::ThreadInputCreateResponse;

use orch_api_contract::threads::ThreadInputReceipt;

use orch_api_contract::threads::ThreadInterruptBody;

/// Temporary conversations keep personal governance; permanent threads use their native page.
pub(super) async fn opening(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    lock: bool,
) -> ApiResult<rows::Conversation> {
    let row = rows::get(conn, id, lock)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.temporary {
        let (row, inspecting, _) = super::opening_for(
            ctx,
            conn,
            principal,
            id,
            false,
            &[Permission::ThreadView, Permission::ThreadInteract],
        )
        .await?;
        if inspecting || row.user_id != principal.caller().user() {
            return Err(denied("Only the person may direct their Ask conversation"));
        }
        return Ok(row);
    }
    let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
    root_access(conn, &ordinary, row.root_org_id).await?;
    let policy = crate::routes::nodes::policy_at(conn, id).await?;
    for permission in [Permission::ThreadView, Permission::ThreadInteract] {
        authorization::require(
            conn,
            &ordinary,
            id,
            authorization::exact(permission),
            Act::Use,
            policy,
        )
        .await?;
    }
    let thread = threads::thread(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if thread.archived_at.is_some() {
        return Err(ApiError::new(
            ErrorCode::NodeArchived,
            "This thread is archived",
        ));
    }
    Ok(row)
}

pub async fn input(
    ctx: Ctx,
    body: ThreadInputCreateBody,
) -> ApiResult<Answer<ThreadInputCreateResponse>> {
    if body.plan || body.native_action.is_some() || body.bypass.is_some() {
        return Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "This read-only program uses questions without native actions, plan mode or limit bypass",
        ));
    }
    if body.delivery != Delivery::Queue {
        return Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "This program takes one question at a time; stop its current answer before sending another",
        ));
    }
    let accepted = super::runner::accept(
        ctx.clone(),
        api::AskMessageBody {
            client_id: body.client_id,
            content: body.content,
            context: None,
            model: None,
        },
    )
    .await?;
    let receipt = &accepted.receipt;
    let thread = receipt.conversation_id;
    let input = accepted.input;
    let fallback = ThreadInputCreateResponse::Receipt(ThreadInputReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        thread_id: thread,
        input_id: input,
        seq: receipt.seq,
    });
    crate::key_operations::publication::receipt(&ctx, &fallback)?;
    let projected = input_reply::project(&ctx, &accepted).await?;
    let response = match projected {
        Some(input) => ThreadInputCreateResponse::full(ThreadInputCreateReply {
            input,
            seq: receipt.seq,
        }),
        None => fallback,
    };
    let answer = Answer::from(response);
    Ok(if accepted.created {
        answer
    } else {
        answer.status(http::StatusCode::OK)
    })
}

/// Retry uses the already guarded native retained-input producer and the same control-plane writer.
pub async fn retry_in(
    ctx: Ctx,
    mut tx: orch_db::Tx,
    guard: transaction_guard::Guard,
    body: orch_api_contract::threads::ThreadTurnRetryBody,
    content: Vec<orch_api_contract::threads::ContentPart>,
) -> ApiResult<Answer<ThreadInputCreateReply>> {
    if body.bypass.is_some() {
        return Err(ApiError::new(
            ErrorCode::CapabilityUnsupported,
            "This read-only program uses current caps without a native limit bypass",
        ));
    }
    let pending = super::runner::accept_in(
        &ctx,
        &mut tx,
        &guard,
        ctx.id("id")?,
        api::AskMessageBody {
            client_id: body.client_id,
            content,
            context: None,
            model: None,
        },
    )
    .await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    let accepted = pending.start(&ctx.app);
    let input = input_reply::project(&ctx, &accepted)
        .await?
        .ok_or_else(|| denied("The accepted retry's input is no longer readable"))?;
    let answer = Answer::from(ThreadInputCreateReply {
        input,
        seq: accepted.receipt.seq,
    });
    Ok(if accepted.created {
        answer
    } else {
        answer.status(http::StatusCode::OK)
    })
}

pub async fn interrupt(
    ctx: Ctx,
    body: ThreadInterruptBody,
) -> ApiResult<orch_api_contract::threads::ThreadStop> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = opening(&ctx, &mut tx, guard.principal(), ctx.id("id")?, true).await?;
    let turn = row
        .turn
        .filter(|turn| body.turn_id == Some(*turn))
        .ok_or_else(|| {
            ApiError::new(ErrorCode::ThreadNotRunning, "That turn is not running")
                .with("current_turn_id", row.turn)
        })?;
    let calls = rows::calls(&mut tx, turn).await?;
    guard.before_effect(&ctx).map_err(ApiError::engine)?;
    let seq = orch_db::sequences::next(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    threads::mark_interrupted_by(&mut tx, turn, guard.user().id(), seq).await?;
    threads::finish_turn(&mut tx, turn, "interrupted", None, ctx.now, seq).await?;
    threads::settle_items_of_turn(&mut tx, turn, "cancelled", ctx.now, seq).await?;
    rows::finish(&mut tx, row.id, turn).await?;
    emit_settled(&ctx, &mut tx, row.id, turn).await?;
    let current = threads::turn(&mut tx, turn)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let people =
        orch_engine::views::People::load(&mut tx, [current.interrupted_by].into_iter().flatten())
            .await
            .map_err(ApiError::engine)?;
    let projected = orch_engine::views::turn_view(&current, &current.model, &people)
        .map_err(ApiError::engine)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    ctx.app
        .engine
        .cancel_ask_calls(&calls)
        .await
        .map_err(ApiError::engine)?;
    Ok(orch_api_contract::threads::ThreadStop {
        state: orch_api_contract::threads::StopState::Stopped,
        requested_at: orch_types::Time::new(ctx.now),
        turn: Some(projected),
    })
}

#[path = "program/input_reply.rs"]
pub mod input_reply;
