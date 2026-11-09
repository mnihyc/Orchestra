//! Completed fork identities use the existing native job/batch result projectors.

use super::*;

use crate::key_operations::publication;

use crate::middleware::idempotency::Creation;

use futures::future::BoxFuture;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::fork_sources;

use orch_api_contract::forks::ForkBatchCreateReceipt;

use orch_api_contract::forks::ForkBatchCreateResponse;

use orch_api_contract::forks::ForkCreateReceipt;

use orch_api_contract::forks::ForkCreateResponse;

use orch_api_contract::forks::ForkNowReceipt;

use orch_api_contract::forks::ForkNowResponse;

use orch_db::idempotency::creation::Created;

use orch_types::Json;

use serde_json::json;
pub(super) fn confirmation() -> Creation<(), ForkCreateBody, ForkCreateResponse> {
    Creation {
        kind: "thread_fork",
        replay: confirm,
        identity: confirm_identity,
    }
}

pub(super) fn immediate() -> Creation<(), ForkNowBody, ForkNowResponse> {
    Creation {
        kind: "thread_fork",
        replay: now,
        identity: now_identity,
    }
}

pub(super) fn batch() -> Creation<(), ForkCreateBody, ForkBatchCreateResponse> {
    Creation {
        kind: "fork_batch",
        replay: batch_replay,
        identity: batch_identity,
    }
}

fn confirm_identity(value: &ForkCreateResponse) -> ApiResult<Created> {
    let id = match value {
        ForkCreateResponse::Full(value) => value.id,
        ForkCreateResponse::Receipt(value) => value.job_id,
    };
    Ok(Created {
        kind: "thread_fork".into(),
        id,
        data: Json::Null,
    })
}

fn now_identity(value: &ForkNowResponse) -> ApiResult<Created> {
    let (id, thread) = match value {
        ForkNowResponse::Full(value) => (value.job.id, value.thread_id),
        ForkNowResponse::Receipt(value) => (value.job_id, value.thread_id),
    };
    Ok(Created {
        kind: "thread_fork".into(),
        id,
        data: json!({"reserved_thread":thread}),
    })
}

fn batch_identity(value: &ForkBatchCreateResponse) -> ApiResult<Created> {
    let (id, queued) = match value {
        ForkBatchCreateResponse::Full(value) => (value.batch_id, value.jobs.len()),
        ForkBatchCreateResponse::Receipt(value) => (value.batch_id, value.queued),
    };
    Ok(Created {
        kind: "fork_batch".into(),
        id,
        data: json!({"queued":queued}),
    })
}

async fn job(ctx: &Ctx, id: Id, now: bool) -> ApiResult<Option<super::job_reply::Outcome>> {
    let row = {
        let mut conn = ctx.app.db.acquire().await?;
        orch_db::queries::jobs::job(&mut conn, id).await?
    };
    let Some(row) = row else { return Ok(None) };
    if row.kind != orch_engine::jobs::fork::KIND || row.created_by != Some(ctx.user()?.id()) {
        return Ok(None);
    }
    let recipe: orch_engine::jobs::fork::Recipe =
        serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
    let declaration = if now {
        &fork_sources::NOW
    } else {
        super::confirmation::declaration(recipe.result)
    };
    Ok(Some(super::job_reply::Outcome {
        parent: recipe.source_thread,
        actor: recipe.by,
        row,
        declaration,
        copied: None,
    }))
}

fn confirm(
    ctx: Ctx,
    (): (),
    _body: ForkCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<ForkCreateResponse>> {
    Box::pin(async move {
        let receipt = ForkCreateResponse::Receipt(ForkCreateReceipt {
            projection: ReceiptProjection::Receipt,
            job_id: created.id,
        });
        publication::receipt(&ctx, &receipt)?;
        let Some(input) = job(&ctx, created.id, false).await? else {
            return Ok(receipt);
        };
        match super::job_reply::render_replay(&ctx, input).await {
            Ok(Some(job)) => Ok(ForkCreateResponse::Full(Box::new(job))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

fn now(
    ctx: Ctx,
    (): (),
    _body: ForkNowBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<ForkNowResponse>> {
    Box::pin(async move {
        let thread = created
            .data
            .get("reserved_thread")
            .and_then(Json::as_str)
            .and_then(|id| id.parse().ok())
            .ok_or_else(|| {
                ApiError::internal("the accepted immediate fork has no reserved Thread")
            })?;
        let receipt = ForkNowResponse::Receipt(ForkNowReceipt {
            projection: ReceiptProjection::Receipt,
            job_id: created.id,
            thread_id: thread,
        });
        publication::receipt(&ctx, &receipt)?;
        let Some(input) = job(&ctx, created.id, true).await? else {
            return Ok(receipt);
        };
        match super::job_reply::render_replay(&ctx, input).await {
            Ok(Some(job)) => Ok(ForkNowResponse::Full(Box::new(ForkNowReply {
                job,
                thread_id: thread,
            }))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}

fn batch_replay(
    ctx: Ctx,
    (): (),
    _body: ForkCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<ForkBatchCreateResponse>> {
    Box::pin(async move {
        let queued = created
            .data
            .get("queued")
            .and_then(Json::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .ok_or_else(|| ApiError::internal("the accepted batch has no recorded count"))?;
        let receipt = ForkBatchCreateResponse::Receipt(ForkBatchCreateReceipt {
            projection: ReceiptProjection::Receipt,
            batch_id: created.id,
            queued,
        });
        publication::receipt(&ctx, &receipt)?;
        let rows = {
            let mut conn = ctx.app.db.acquire().await?;
            orch_db::queries::jobs::forks::of_batch(&mut conn, created.id).await?
        };
        if rows.is_empty() {
            return Ok(receipt);
        }
        let roots =
            serde_json::from_value(created.data.get("roots").cloned().ok_or_else(|| {
                ApiError::internal("the accepted batch has no recorded native governors")
            })?)
            .map_err(ApiError::internal)?;
        let input = super::batch_reply::Outcome {
            batch: created.id,
            rows,
            roots,
            actor: ctx.user()?.id(),
            replay: true,
        };
        match super::batch_reply::render_replay(&ctx, input).await {
            Ok(Some(batch)) => Ok(ForkBatchCreateResponse::Full(Box::new(batch))),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
