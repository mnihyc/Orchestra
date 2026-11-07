//! The reviewed native cascade is admitted as one prepared effect and committed once.

use super::*;
use crate::key_operations::{
    native::{self, effects, reply},
    publication,
};
use futures::future::BoxFuture;
use orch_api_contract::{
    loss_sources::{DECLARE, Source},
    operation_sources::Declaration,
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError, queries::jobs::forks};
use orch_engine::loss::{self, PreparedLossDeclaration};
use std::sync::Arc;

pub(super) struct Confirm;
pub(super) struct Ready {
    native: Arc<PreparedLossDeclaration>,
}
impl effects::Effect for Confirm {
    type Source = Source;
    type Query = ();
    type Body = api::LossDeclareBody;
    type Value = Ready;
    type Output = Arc<PreparedLossDeclaration>;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &DECLARE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = forks::preflight(conn, body.preflight_id)
                .await?
                .filter(|row| row.kind == "loss" && row.created_by == caller.user())
                .ok_or_else(ApiError::not_found)?;
            let target = loss::declaration_target(&row).map_err(refused)?;
            let known = vec![preflight::target_fact(conn, target).await?];
            let plan = native::bind_known(ctx, &DECLARE, &known)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            preflight::require_target(conn, &principal, target).await?;
            let prepared = ctx
                .app
                .engine
                .prepare_loss_declaration(conn, &principal, &body)
                .await
                .map_err(refused)?;
            let facts = vec![
                preflight::target_fact(conn, prepared.preview().target()).await?,
                preflight::affected_facts(conn, prepared.affected()).await?,
            ];
            Ok((
                Ready {
                    native: Arc::new(prepared),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let preview = value.native.preview();
            if preview.creator() != principal.user() {
                return Err(ApiError::not_found());
            }
            preflight::require_target(conn, principal, preview.target()).await
        })
    }
}
impl effects::Apply for Confirm {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            before.check()?;
            ctx.app
                .engine
                .apply_loss_declaration(conn, principal, &value.native)
                .await
                .map_err(refused)?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Arc::clone(&value.native)))
        })
    }
}
pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: api::LossDeclareBody,
) -> ApiResult<api::LossDeclareResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Confirm>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::LossDeclareReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        preflight_id: done.output.preview().plan().preflight_id,
    };
    publication::receipt(&done.ctx, &receipt)?;
    for machine in done.output.machines() {
        ctx.app.link.let_go(*machine).await;
    }
    ctx.app.engine.kick();
    let full = match reply::render::<preflight::Content>(
        &done.ctx,
        &done.accepted,
        preflight::Saved::declared(done.output),
    )
    .await
    {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    Ok(match full {
        Some(plan) => api::LossDeclareResponse::Full(Box::new(plan)),
        None => api::LossDeclareResponse::Receipt(receipt),
    })
}
