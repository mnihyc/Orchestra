//! The accepted native birth and history import replay as current thread content or their receipt.
use super::*;
use crate::middleware::idempotency::ObservedCreation;
use orch_api_contract::common::{ErrorCode, ReceiptProjection};
use orch_api_contract::nodes::{ThreadAdoptReceipt, ThreadAdoptResponse};
use orch_db::idempotency::creation::Created;

const KIND: &str = "adopted_thread";

pub(crate) fn binding() -> ObservedCreation<(), ThreadAdoptBody, ThreadAdoptResponse> {
    ObservedCreation { kind: KIND, replay }
}

pub(super) async fn observed(
    conn: &mut Conn,
    account: Id,
    topic: Id,
    outcome: &orch_engine::adoption::Adopted,
) -> ApiResult<()> {
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(account),
        KIND,
        outcome.created.thread,
        serde_json::json!({
            "topic": topic,
            "segment": outcome.created.segment,
            "history_items": outcome.history_items,
        }),
    )
    .await?;
    Ok(())
}

fn replay(
    ctx: Ctx,
    (): (),
    _: ThreadAdoptBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<ThreadAdoptResponse>> {
    Box::pin(async move {
        if created.kind != KIND {
            return Err(ApiError::internal("the accepted adoption has another kind"));
        }
        let receipt = ThreadAdoptResponse::Receipt(ThreadAdoptReceipt {
            projection: ReceiptProjection::Receipt,
            thread_id: created.id,
            topic_id: serde_json::from_value(created.data["topic"].clone())
                .map_err(ApiError::internal)?,
            segment_id: serde_json::from_value(created.data["segment"].clone())
                .map_err(ApiError::internal)?,
            history_items: serde_json::from_value(created.data["history_items"].clone())
                .map_err(ApiError::internal)?,
        });
        crate::key_operations::publication::receipt(&ctx, &receipt)?;
        match readback::completed(&ctx, created.id).await {
            Ok(Some(thread)) => Ok(ThreadAdoptResponse::full(thread)),
            Ok(None) => Ok(receipt),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(receipt)
            }
            Err(error) => Err(error),
        }
    })
}
