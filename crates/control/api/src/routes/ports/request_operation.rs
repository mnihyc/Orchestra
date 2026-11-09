//! Thread requests keep their native writer and share the opening's current response projector.
use super::*;
use crate::key_operations::{
    native::{self, reply},
    targets,
};
use crate::middleware::idempotency::ObservedCreation;
use crate::registry::OperationFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::port_sources::{self as sources, Source};
use orch_authz::Caller;
use serde_json::json;

const KIND: &str = "thread_port_request";
pub(crate) fn resolve<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    _caller: &'a Caller,
    (): (),
    _body: PortOpeningRequest,
) -> OperationFuture<'a> {
    Box::pin(async move {
        let target = targets::node(conn, ctx.id("id")?).await?;
        if target.object.kind != KeyTargetKind::Object(ObjectKind::Thread) {
            return Err(ApiError::not_found());
        }
        let fact = native::Facts::one(Source::RequestThread, target.object.kind, target);
        native::bind_required(ctx, &sources::THREAD_REQUEST, &[fact])
    })
}

pub(crate) async fn observe(
    conn: &mut Conn,
    user: Id,
    thread: Id,
    row: &rows::Opening,
    created: bool,
) -> ApiResult<()> {
    orch_db::idempotency::creation::observe_with(conn, Some(user), KIND, row.id, json!({
        "environment":row.environment_id,"revision":row.revision,"created":created,"thread":thread,"request_id":row.approval_id,
    })).await?;
    Ok(())
}

pub(crate) fn binding() -> ObservedCreation<(), PortOpeningRequest, PortOpenResponse> {
    ObservedCreation {
        kind: KIND,
        replay: |ctx, (), body: PortOpeningRequest, created| {
            Box::pin(async move {
                let env: Id = serde_json::from_value(created.data["environment"].clone())
                    .map_err(ApiError::internal)?;
                let revision = serde_json::from_value(created.data["revision"].clone())
                    .map_err(ApiError::internal)?;
                let fresh = serde_json::from_value(created.data["created"].clone())
                    .map_err(ApiError::internal)?;
                let thread: Id = serde_json::from_value(created.data["thread"].clone())
                    .map_err(ApiError::internal)?;
                let request_id = serde_json::from_value(created.data["request_id"].clone())
                    .map_err(ApiError::internal)?;
                if created.kind != KIND || ctx.id("id")? != thread {
                    return Err(ApiError::not_found());
                }
                let receipt = PortOpenResponse::Receipt(PortOpenReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    opening_id: created.id,
                    environment_id: env,
                    revision,
                    created: fresh,
                    request_id,
                    link: None,
                });
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                let row = rows::opening(&mut *ctx.app.db.acquire().await?, created.id)
                    .await?
                    .filter(|row| row.environment_id == env && row.port == i32::from(body.port));
                let Some(row) = row else {
                    return Ok(receipt);
                };
                project(
                    &ctx,
                    None,
                    create_operation::Accepted::requested(row, thread, fresh),
                    receipt,
                )
                .await
            })
        },
    }
}

pub(crate) async fn first(
    ctx: &Ctx,
    accepted: &orch_authz::Caller,
    value: PortOpenReply,
    created: bool,
) -> ApiResult<PortOpenResponse> {
    let receipt = PortOpenResponse::Receipt(PortOpenReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        opening_id: value.opening.id,
        environment_id: value.opening.environment_id,
        revision: value.opening.revision,
        created,
        request_id: value.request.as_ref().map(|request| request.id),
        link: None,
    });
    crate::key_operations::publication::receipt(ctx, &receipt)?;
    let Some(row) = rows::opening(&mut *ctx.app.db.acquire().await?, value.opening.id).await?
    else {
        return Ok(receipt);
    };
    project(
        ctx,
        Some(accepted),
        create_operation::Accepted::requested(row, ctx.id("id")?, created),
        receipt,
    )
    .await
}

async fn project(
    ctx: &Ctx,
    accepted: Option<&orch_authz::Caller>,
    input: create_operation::Accepted,
    receipt: PortOpenResponse,
) -> ApiResult<PortOpenResponse> {
    let full = if let Some(accepted) = accepted {
        reply::render::<create_operation::projection::Opening>(ctx, accepted, input).await
    } else {
        reply::render_completed_replay::<create_operation::projection::Opening>(
            ctx,
            ctx.user()?.id(),
            input,
        )
        .await
    };
    match full {
        Ok(Some(value)) => Ok(PortOpenResponse::Full(Box::new(value))),
        Ok(None) => Ok(receipt),
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(receipt),
        Err(error) => Err(error),
    }
}
