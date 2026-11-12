//! The same provider creation projection reads only the accepted native identity on replay.

use super::*;

use crate::middleware::idempotency::Creation;

use futures::future::BoxFuture;

use orch_api_contract::resources::MachineProviderCreateResponse;

use orch_api_contract::resources::ProviderCreateResponse;

use orch_api_contract::resources::ProviderWriteReply;

use orch_db::idempotency::creation::Created;
const KIND: &str = "provider";

#[derive(serde::Deserialize)]
struct Accepted {
    revision: i64,
    #[serde(default)]
    warning: Option<String>,
}

pub(in crate::routes::resources) fn provider_binding()
-> Creation<(), CreateProvider, ProviderCreateResponse> {
    Creation {
        kind: KIND,
        identity: |value| {
            let (id, warning) = match value {
                ProviderCreateResponse::Full(value) => {
                    (value.provider.base.id, value.warning.clone())
                }
                ProviderCreateResponse::Receipt(value) => {
                    (value.provider_id, value.warning.clone())
                }
            };
            Ok(Created {
                kind: KIND.into(),
                id,
                data: serde_json::json!({"warning":warning}),
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                let (receipt, provider) = replay(&ctx, created).await?;
                Ok(match provider {
                    Some(provider) => ProviderCreateResponse::full(ProviderWriteReply {
                        provider,
                        warning: receipt.warning,
                    }),
                    None => ProviderCreateResponse::Receipt(receipt),
                })
            })
        },
    }
}

pub(in crate::routes::resources) fn machine_binding()
-> Creation<(), MachineProviderCreateBody, MachineProviderCreateResponse> {
    Creation {
        kind: KIND,
        identity: |value| {
            let (id, warning) = match value {
                MachineProviderCreateResponse::Full(value) => (value.base.id, None),
                MachineProviderCreateResponse::Receipt(value) => {
                    (value.provider_id, value.warning.clone())
                }
            };
            Ok(Created {
                kind: KIND.into(),
                id,
                data: serde_json::json!({"warning":warning}),
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                let (receipt, provider) = replay(&ctx, created).await?;
                Ok(match provider {
                    Some(provider) => MachineProviderCreateResponse::full(provider),
                    None => MachineProviderCreateResponse::Receipt(receipt),
                })
            })
        },
    }
}

pub(in crate::routes::resources) async fn observed(
    ctx: &Ctx,
    conn: &mut Conn,
    receipt: &ProviderCreateReceipt,
    operands: &Operands,
) -> ApiResult<()> {
    if operands.duplicate.is_some() {
        return super::duplicate::observed(ctx, conn, receipt.provider_id, operands).await;
    }
    orch_db::idempotency::creation::observe_with(
        conn,
        Some(ctx.user()?.id()),
        KIND,
        receipt.provider_id,
        serde_json::json!({"revision":receipt.revision}),
    )
    .await?;
    let identity = transaction_guard::current_http_identity(
        ctx,
        conn,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    identity.check_deadline(&ctx.app)
}

async fn replay(
    ctx: &Ctx,
    created: Created,
) -> ApiResult<(ProviderCreateReceipt, Option<Provider>)> {
    if created.kind != KIND {
        return Err(ApiError::internal("the accepted provider kind changed"));
    }
    let accepted: Accepted = serde_json::from_value(created.data).map_err(ApiError::internal)?;
    let receipt = ProviderCreateReceipt {
        projection: ReceiptProjection::Receipt,
        provider_id: created.id,
        revision: accepted.revision,
        warning: accepted.warning,
    };
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    let current = authorization::caller_of(ctx.user()?)?;
    let reading = ctx.for_completed_replay_projection();
    let provider = crate::key_operations::native::completed(
        &reading,
        &current,
        move |ctx, conn, principal| -> BoxFuture<'_, ApiResult<Option<Provider>>> {
            Box::pin(async move {
                let plan = Prepared::new(ctx.route, ctx.route.name)?;
                super::project(
                    ctx,
                    conn,
                    principal.caller(),
                    plan,
                    created.id,
                    super::super::provider_projection::Purpose::Replay,
                )
                .await
            })
        },
    )
    .await?;
    Ok((receipt, provider))
}
