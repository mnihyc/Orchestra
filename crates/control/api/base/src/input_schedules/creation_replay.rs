//! Completed schedule creation keeps its accepted definition authority and current native Read.

use super::*;

use crate::key_operations::native;

use crate::key_operations::publication;

use crate::middleware::idempotency::Creation;

use futures::future::BoxFuture;

use orch_db::idempotency::creation::Created;
pub(super) fn binding() -> Creation<(), InputScheduleCreateBody, InputScheduleWriteResponse> {
    Creation {
        kind: "input_schedule",
        replay,
        identity: |value| {
            Ok(Created {
                kind: "input_schedule".into(),
                id: match value {
                    InputScheduleWriteResponse::Full(row) => row.id,
                    InputScheduleWriteResponse::Receipt(row) => row.schedule_id,
                },
                data: Json::Null,
            })
        },
    }
}

fn replay(
    ctx: Ctx,
    (): (),
    _: InputScheduleCreateBody,
    created: Created,
) -> BoxFuture<'static, ApiResult<InputScheduleWriteResponse>> {
    Box::pin(async move {
        let topic: Id = decode(created.data["topic"].clone())?;
        if ctx.id("id")? != topic {
            return Err(ApiError::not_found());
        }
        let safe = InputScheduleWriteResponse::Receipt(InputScheduleReceipt {
            projection: orch_api_contract::common::ReceiptProjection::Receipt,
            schedule_id: created.id,
            revision: decode(created.data["revision"].clone())?,
            topic_seq: decode(created.data["topic_seq"].clone())?,
        });
        publication::receipt(&ctx, &safe)?;
        let ctx = ctx.for_completed_replay_projection();
        let caller = authorization::caller_of(ctx.user()?)?;
        let id = created.id;
        let full = native::completed(&ctx, &caller, move |ctx, conn, reader| {
            Box::pin(async move {
                let row = rows::schedule(conn, id, false)
                    .await?
                    .filter(|row| row.topic_id == topic)
                    .ok_or_else(ApiError::not_found)?;
                operation::replay_full(ctx, conn, reader.caller(), row).await
            })
        })
        .await;
        match full {
            Ok(Some((value, source))) => {
                operation::retain_replay(&ctx, &caller, source)?;
                Ok(InputScheduleWriteResponse::full(value))
            }
            Ok(None) => Ok(safe),
            Err(error) if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) => {
                Ok(safe)
            }
            Err(error) => Err(error),
        }
    })
}
