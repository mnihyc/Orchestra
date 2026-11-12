//! The ordinary trial, stored fingerprint and native move stay on one guarded transaction.

use super::*;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::middleware::transaction_guard;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::thread_move::ThreadMovePreviewResponse;

use orch_api_contract::thread_move::ThreadMoveResponse;

use orch_api_contract::thread_move::ThreadMoveReviewReceipt;
fn boxed(error: ApiError) -> orch_engine::EngineError {
    orch_engine::EngineError::BeforeEffect(Box::new(error))
}
orch_api_base::__api_bind_3d9ed592e3!();

pub(super) fn review_receipt(plan: &ThreadMovePlan) -> ThreadMoveReviewReceipt {
    ThreadMoveReviewReceipt {
        projection: ReceiptProjection::Receipt,
        thread_id: plan.movement.node.id,
        parent_id: plan.movement.to.id,
        preflight_id: plan.movement.preflight_id,
        expected_revision: plan.expected_revision,
        expires_at: plan.movement.expires_at,
        blocker_codes: plan
            .movement
            .blockers
            .iter()
            .map(|blocker| blocker.code.clone())
            .collect(),
    }
}

async fn locked(
    ctx: &Ctx,
    target: Id,
    preflight: Option<Id>,
) -> ApiResult<(orch_db::Tx, transaction_guard::Guard)> {
    let (mut tx, initial) = transaction_guard::begin(ctx).await?;
    let row = ordinary(&mut tx, initial.principal(), ctx.id("id")?).await?;
    may_move(
        &mut tx,
        initial.principal(),
        NodeKind::Thread,
        row.node_id,
        target,
    )
    .await?;
    initial.check_deadline(ctx)?;
    locks(&mut tx, row.org_id, row.node_id, target).await?;
    if let Some(id) = preflight {
        jobs::changes::lock_plan(&mut tx, id)
            .await?
            .ok_or_else(|| {
                ApiError::new(
                    ErrorCode::PreflightExpired,
                    "the stored thread move review is no longer available",
                )
            })?;
    }
    Ok((tx, initial))
}

pub(super) async fn preview(
    ctx: Ctx,
    (): (),
    body: ThreadMovePreviewBody,
) -> ApiResult<ThreadMovePreviewResponse> {
    let result = preview_in(&ctx, body).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}

async fn preview_in(
    ctx: &Ctx,
    body: ThreadMovePreviewBody,
) -> ApiResult<ThreadMovePreviewResponse> {
    let (mut tx, initial) = locked(ctx, body.parent_id, None).await?;
    let completed = effects::apply::<operation::Preview>(ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = review_receipt(&completed.output.plan);
    publication::receipt(&completed.ctx, &receipt)?;
    match native::reply::render::<projection::Review>(
        &completed.ctx,
        &completed.accepted,
        completed.output,
    )
    .await?
    {
        Some(value) => Ok(ThreadMovePreviewResponse::Full(Box::new(value))),
        None => Ok(ThreadMovePreviewResponse::Receipt(receipt)),
    }
}

pub(super) async fn move_thread(
    ctx: Ctx,
    (): (),
    body: ThreadMoveBody,
) -> ApiResult<ThreadMoveResponse> {
    let result = move_in(&ctx, body).await;
    result.map_err(|error| {
        if ctx.key_receipt_required() {
            error.without_content()
        } else {
            error
        }
    })
}

async fn move_in(ctx: &Ctx, body: ThreadMoveBody) -> ApiResult<ThreadMoveResponse> {
    let (mut tx, initial) = locked(ctx, body.parent_id, Some(body.preflight_id)).await?;
    let completed = effects::apply::<operation::Move>(ctx, &mut tx).await?;
    initial.check_deadline(&completed.ctx)?;
    if let Some(original) = &completed.output.original {
        original.check(&completed.ctx)?;
    }
    tx.commit().await.map_err(DbError::from)?;
    publication::receipt(&completed.ctx, &completed.output.receipt)?;
    let receipt = completed.output.receipt.clone();
    match native::reply::render::<projection::Moved>(
        &completed.ctx,
        &completed.accepted,
        completed.output,
    )
    .await?
    {
        Some(value) => Ok(ThreadMoveResponse::Full(Box::new(value))),
        None => Ok(ThreadMoveResponse::Receipt(receipt)),
    }
}
