//! One native cancellation per commit, with current authority and truthful partial outcomes.

use super::*;
use crate::key_operations::native::reply::{Prepared, Projection};
use crate::key_operations::{native, publication};
use crate::middleware::transaction_guard::Guard;
use futures::future::BoxFuture;
use orch_api_contract::fork_sources::{self as sources, Source};
use orch_api_contract::forks::{ForkBatchCancelReceipt, ForkBatchCancelResponse};
use orch_api_contract::operation_sources::Declaration;
use orch_authz::Caller;
use orch_engine::jobs::batch::{BatchCancellation, BatchMemberCancellation};

pub(super) struct Cancel;

async fn snapshot(ctx: &Ctx, conn: &mut Conn) -> ApiResult<BatchCancellation> {
    let value = ctx
        .app
        .engine
        .preparing_batch_cancellation(conn, ctx.id("batch_id")?)
        .await
        .map_err(ApiError::engine)?;
    if value.rows().is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(value)
}

async fn remaining(conn: &mut Conn, value: &BatchCancellation) -> ApiResult<native::Facts<Source>> {
    let open = value
        .rows()
        .iter()
        .filter(|row| row.is_open())
        .cloned()
        .collect::<Vec<_>>();
    batch_read::subjects(conn, Source::PendingBatchJobs, &open).await
}

fn creator(principal: &Principal, value: &BatchCancellation) -> ApiResult<()> {
    if value
        .rows()
        .first()
        .is_some_and(|row| row.created_by == Some(principal.user()))
    {
        Ok(())
    } else {
        Err(ApiError::not_found())
    }
}

impl native::Stored for Cancel {
    type Source = Source;
    type Value = BatchCancellation;
    const ROWS: &'static [Declaration<Source>] = &[sources::BATCH_CANCEL];

    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(BatchCancellation, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = snapshot(ctx, conn).await?;
            let facts = vec![remaining(conn, &value).await?];
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a BatchCancellation,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { creator(principal, value) })
    }
}

async fn prepared(ctx: &Ctx, conn: &mut Conn, guard: &Guard) -> ApiResult<BatchCancellation> {
    let value = if native::modern(guard.principal().caller()) {
        guard
            .operation_data::<native::Prepared<BatchCancellation>>()?
            .value
            .clone()
    } else {
        snapshot(ctx, conn).await?
    };
    creator(guard.principal(), &value)?;
    Ok(value)
}

async fn member(
    ctx: &Ctx,
    original: &BatchCancellation,
    id: Id,
) -> ApiResult<(Caller, BatchMemberCancellation)> {
    let (mut tx, first) = transaction_guard::begin(ctx).await?;
    first.check_deadline(ctx)?;
    orch_db::queries::jobs::lock(&mut tx, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let value = prepared(&current, &mut tx, &guard).await?;
    if !original.preserves(&value) {
        return Err(ApiError::new(
            ErrorCode::AuthzDenied,
            "the original batch membership changed",
        ));
    }
    let row = value
        .rows()
        .iter()
        .find(|row| row.id == id)
        .ok_or_else(ApiError::not_found)?;
    let accepted = guard.principal().caller().clone();
    let result = current
        .app
        .engine
        .cancel_batch_member_before(&mut tx, row, accepted.user(), || {
            guard.before_effect(&current)
        })
        .await
        .map_err(ApiError::engine)?;
    // Every SQL effect is still rollbackable here, including the native command cancellation.
    // A lock wait cannot commit a write after its actual key/session lifetime ended.
    let (last, final_guard) = transaction_guard::after_wait(&current, &mut tx).await?;
    guard.check_deadline(&last)?;
    final_guard.check_deadline(&last)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok((accepted, result))
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<ForkBatchCancelResponse> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let original = prepared(&ctx, &mut tx, &initial).await?;
    let mut accepted = initial.principal().caller().clone();
    initial.check_deadline(&ctx)?;
    tx.rollback().await.map_err(orch_db::DbError::from)?;
    let mut receipt = ForkBatchCancelReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        batch_id: ctx.id("batch_id")?,
        cancelled: Vec::new(),
        requested: Vec::new(),
        complete: true,
    };
    for row in original.rows().iter().filter(|row| row.is_open()) {
        let (actor, result) = match member(&ctx, &original, row.id).await {
            Ok(result) => result,
            Err(error)
                if (!receipt.cancelled.is_empty() || !receipt.requested.is_empty())
                    && matches!(error.problem.status, 401 | 403 | 404 | 410) =>
            {
                receipt.complete = false;
                break;
            }
            Err(error) => return Err(error),
        };
        accepted = actor;
        if result.ended() {
            receipt.cancelled.push(result.id());
        }
        if result.asked() {
            receipt.requested.push(result.id());
        }
        // This is owned completion of precisely the committed member. It cannot start
        // a new cancellation after authority closes, and does not wait for a runner ACK.
        ctx.app
            .engine
            .finish_batch_member(&result)
            .await
            .map_err(ApiError::engine)?;
    }
    publication::receipt(&ctx, &receipt)?;
    if !receipt.complete {
        return Ok(ForkBatchCancelResponse::Receipt(receipt));
    }
    match native::reply::render::<BatchReply>(&ctx, &accepted, original).await? {
        Some(value) => Ok(ForkBatchCancelResponse::Full(Box::new(value))),
        None => Ok(ForkBatchCancelResponse::Receipt(receipt)),
    }
}

struct BatchReply;
impl Projection for BatchReply {
    type Source = Source;
    type Input = BatchCancellation;
    type Subject = BatchCancellation;
    type Value = batch_read::Selected;
    type Reply = ForkBatch;
    const READ_SOURCE: Source = Source::BatchJobs;
    fn declaration(_input: &BatchCancellation) -> &'static Declaration<Source> {
        &sources::BATCH_CANCEL
    }

    fn subject<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a BatchCancellation,
    ) -> BoxFuture<'a, ApiResult<(BatchCancellation, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = snapshot(ctx, conn).await?;
            if !input.preserves(&value) {
                return Err(ApiError::not_found());
            }
            let facts = vec![
                remaining(conn, &value).await?,
                batch_read::subjects(conn, Source::BatchJobs, value.rows()).await?,
            ];
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a BatchCancellation,
        subject: &'a BatchCancellation,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move { creator(principal, subject) })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a BatchCancellation,
        subject: &'a BatchCancellation,
    ) -> BoxFuture<'a, ApiResult<(batch_read::Selected, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            batch_read::prepare_view(
                ctx,
                conn,
                principal,
                ctx.id("batch_id")?,
                subject.rows().to_vec(),
                &sources::BATCH_CANCEL,
            )
            .await
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a BatchCancellation,
        prepared: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ForkBatch>> {
        <batch_read::Batch as native::Selected>::render(ctx, conn, principal, &prepared.value)
    }
    fn check_opened<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _input: &'a BatchCancellation,
        old: &'a Prepared<Self>,
        fresh: &'a Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            if !old.subject.preserves(&fresh.subject) {
                return Ok(false);
            }
            <batch_read::Batch as native::Selected>::check_opened(
                ctx,
                conn,
                principal,
                &(),
                &old.value,
                &fresh.value,
            )
            .await
        })
    }
}
