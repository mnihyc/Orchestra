//! Authenticated model APIs use the provider's ordinary native key keeper and one memory exchange.

use crate::key_operations::{direct, native::http_origin};
use crate::middleware::{
    requests::{ApiError, ApiResult},
    transaction_guard,
};
use crate::registry::{Ctx, Registry};
use axum::{
    body::{Body, to_bytes},
    response::{IntoResponse, Response},
};
use bytes::Bytes;
use orch_api_contract::{common::ErrorCode, resources::*};
use orch_db::{Conn, DbError, queries::provider_relay as rows};
use orch_engine::{
    operation_scope::{continuing, provider_relay::CallSource},
    provider_relay::Selection,
};
use orch_model_contract::protocol::{
    ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_EMBEDDINGS, OPENAI_RESPONSES,
};
use orch_provider_contract::models::ProviderRelayOptions;
use orch_types::{Id, Json};
use std::{sync::Arc, time::Duration};

mod call;
mod exchange;
mod native_command;
mod options;

pub(super) fn mount(registry: &mut Registry) {
    registry
        .on_raw_query_with_optional_operation(ProviderRelayResponses, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayCompact, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayInputTokens, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayMessages, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayMessageCount, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayChat, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayEmbeddings, prepare, call)
        .on_raw_query_with_optional_operation(ProviderRelayInteractions, prepare, call)
        .on_raw_read_with_operation(ProviderRelayModels, direct::declared, models)
        .on_raw_read_with_operation(ProviderRelayMessageModels, direct::declared, models);
}

fn prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a orch_authz::Caller,
    (): (),
    (): (),
) -> crate::registry::OptionalOperationFuture<'a> {
    Box::pin(async move { direct::declared(ctx, conn, caller, (), ()).await.map(Some) })
}
fn unavailable(detail: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::ProviderRelayUnavailable, detail)
}
fn denied() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "Relay is not granted for this model and its options.",
    )
}
fn invalid(detail: impl Into<String>) -> ApiError {
    ApiError::new(ErrorCode::RequestInvalid, detail)
}
fn mapped(error: orch_engine::provider_relay::Error) -> ApiError {
    use orch_engine::provider_relay::Error;
    match error {
        Error::Denied => denied(),
        Error::Db(error) => error.into(),
        Error::Authz(error) => ApiError::internal(error),
        Error::Internal(error) => ApiError::engine(error),
        Error::Options(detail) => invalid(detail),
        error => unavailable(error.to_string()),
    }
}
fn continuing_error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Native(error) => ApiError::engine(error),
        _ => denied(),
    }
}
fn interface(ctx: &Ctx) -> ApiResult<(&'static str, &'static str)> {
    Ok(match ctx.route.name {
        "ProviderRelayResponses" => (OPENAI_RESPONSES, "/responses"),
        "ProviderRelayCompact" => (OPENAI_RESPONSES, "/responses/compact"),
        "ProviderRelayInputTokens" => (OPENAI_RESPONSES, "/responses/input_tokens"),
        "ProviderRelayMessages" => (ANTHROPIC_MESSAGES, "/v1/messages"),
        "ProviderRelayMessageCount" => (ANTHROPIC_MESSAGES, "/v1/messages/count_tokens"),
        "ProviderRelayChat" => (OPENAI_CHAT, "/chat/completions"),
        "ProviderRelayEmbeddings" => (OPENAI_EMBEDDINGS, "/embeddings"),
        "ProviderRelayInteractions" => (
            orch_model_contract::protocol::GOOGLE_INTERACTIONS,
            "/interactions",
        ),
        _ => {
            return Err(ApiError::internal(
                "the model call has no declared interface",
            ));
        }
    })
}
async fn configuration(conn: &mut Conn, provider: Id) -> ApiResult<ProviderRelayOptions> {
    let row = orch_db::queries::resources::thread_engine::provider(conn, provider)
        .await?
        .ok_or_else(|| unavailable("The provider is unavailable."))?;
    if row.kind != "api" {
        return Err(unavailable("Subscriptions run through a normal thread."));
    }
    let options: ProviderRelayOptions = serde_json::from_value(
        rows::configuration(conn, provider)
            .await?
            .ok_or_else(|| unavailable("The provider is unavailable."))?,
    )
    .map_err(ApiError::internal)?;
    if !options.enabled {
        return Err(unavailable("Provider Relay is off."));
    }
    if row.status == "disabled" || row.deleted_at.is_some() {
        return Err(unavailable("The provider is disabled or removed."));
    }
    Ok(options)
}
async fn call(ctx: Ctx, (): (), body: Body) -> ApiResult<Response> {
    let (format, path) = interface(&ctx)?;
    let provider = ctx.id("id")?;
    // Ordinary Relay authority is checked before reading private configuration or allocating input.
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    configuration(&mut tx, provider).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let options = options::Options::read(&ctx).await?;
    let relay = ctx
        .app
        .blobs
        .model_relays()
        .create(options.concurrency, u64::MAX)
        .await;
    let mut exchange = exchange::Exchange::new(ctx.clone(), Arc::clone(&relay), options);
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    configuration(&mut tx, provider).await?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let limit = usize::try_from(options.request_bytes).unwrap_or(usize::MAX);
    let bytes = tokio::time::timeout(
        Duration::from_secs(options.idle_secs),
        to_bytes(body, limit),
    )
    .await
    .map_err(|_| {
        unavailable("The model request stopped arriving before the relay idle allowance.")
    })?
    .map_err(|_| {
        ApiError::new(
            ErrorCode::RequestTooLarge,
            "The model request exceeds providers.relay_request_bytes.",
        )
        .with("limit", options.request_bytes)
    })?;
    let payload: Json = serde_json::from_slice(&bytes)
        .map_err(|_| invalid("The model request must be a JSON object."))?;
    let model = payload
        .get("model")
        .and_then(Json::as_str)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| invalid("The model request names its model."))?;
    let bytes = Bytes::from(serde_json::to_vec(&payload).map_err(ApiError::internal)?);
    call::run(
        &ctx,
        &mut exchange,
        call::Request {
            provider,
            model,
            format,
            path,
            payload: &payload,
            input: bytes,
        },
    )
    .await?;
    exchange.response().await
}
async fn models(ctx: Ctx, (): (), _body: Body) -> ApiResult<Response> {
    let provider = ctx.id("id")?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let messages = ctx.route.name == "ProviderRelayMessageModels";
    let all = orch_engine::provider_relay::models(
        &mut tx,
        guard.principal(),
        provider,
        messages.then_some(ANTHROPIC_MESSAGES),
    )
    .await
    .map_err(mapped)?;
    let shown:Vec<_> = all.into_iter().map(|model| if messages {
        serde_json::json!({"id":model.model_name,"type":"model","display_name":model.display_name})
    } else { serde_json::json!({"id":model.model_name,"object":"model","owned_by":"provider"}) }).collect();
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let reply = if messages {
        serde_json::json!({"data":shown,"has_more":false,"first_id":shown.first().and_then(|v|v.get("id")),"last_id":shown.last().and_then(|v|v.get("id"))})
    } else {
        serde_json::json!({"object":"list","data":shown})
    };
    Ok(axum::Json(reply).into_response())
}
