//! Current readback of the actual created Mount through the existing stored singleton factory.

use super::*;

use crate::middleware::idempotency::Creation;

use orch_db::idempotency::creation::Created as Recorded;
pub(in crate::routes::resources::mounts) fn binding()
-> Creation<(), api::MountCreateBody, MountCreateResponse> {
    Creation {
        kind: "mount",
        replay: |ctx, (), _, created| Box::pin(replay(ctx, created)),
        identity: |value| {
            Ok(Recorded {
                kind: "mount".into(),
                id: match value {
                    MountCreateResponse::Full(value) => value.base.id,
                    MountCreateResponse::Receipt(value) => value.mount_id,
                },
                data: serde_json::Value::Null,
            })
        },
    }
}

async fn replay(ctx: Ctx, recorded: Recorded) -> ApiResult<MountCreateResponse> {
    let sparse = receipt(recorded.id);
    crate::key_operations::publication::receipt(&ctx, &sparse)?;
    let full = async {
        // Hydrate only the actual current native slot; no original Home Write is replayed.
        let row = {
            let mut conn = ctx.app.db.acquire().await?;
            rows::mounts(&mut conn, &[recorded.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?
        };
        ctx.retain_source(Created {
            id: recorded.id,
            home: row.home_id,
        })?;
        native::resource_reply::stored_completed_replay::<Mounts>(&ctx, ctx.user()?.id()).await
    }
    .await;
    match full {
        Ok(Some(Resource::Mount(value))) => Ok(MountCreateResponse::Full(Box::new(value))),
        Ok(None) => Ok(sparse),
        Ok(Some(_)) => Err(ApiError::internal(
            "a Mount creation replay returned another resource",
        )),
        Err(error)
            if matches!(
                error.code(),
                orch_api_contract::common::ErrorCode::NotFound
                    | orch_api_contract::common::ErrorCode::AuthzDenied
            ) =>
        {
            Ok(sparse)
        }
        Err(error) => Err(error),
    }
}
