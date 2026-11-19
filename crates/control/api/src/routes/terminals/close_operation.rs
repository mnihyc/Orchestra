//! One stored native shell source, exact first queue effect, and completed optional readback.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, ApplyOwned, Effect, FirstEffect, Outcome},
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    operation_sources::Declaration,
    terminal_sources::{self as sources, Source},
    terminals::{TerminalCloseReceipt, TerminalCloseResponse},
};
use orch_authz::Caller;
use orch_engine::shells::Finishing;
use std::sync::Arc;

pub(super) struct Close;

#[derive(Clone)]
pub(super) struct Closed {
    id: Id,
    environment: Id,
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![native::Facts::one(
        Source::SessionEnvironment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    )])
}

async fn require(conn: &mut Conn, principal: &Principal, row: &SessionRow) -> ApiResult<()> {
    if !may_close_shell(
        conn,
        principal,
        &Shell {
            environment: row.environment_id,
            thread: row.thread_id,
            opened_by: row.opened_by,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    {
        return Err(ApiError::not_found());
    }
    Ok(())
}

impl Effect for Close {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Finishing;
    type Output = Closed;

    fn declaration(_value: &Finishing) -> &'static Declaration<Source> {
        &sources::CLOSE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Finishing, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let value = ctx
                .app
                .engine
                .shells()
                .preparing_finish(conn, ctx.id("terminal_id")?)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, value.session().environment_id).await?;
            Ok((value, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Finishing,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(conn, principal, value.session()))
    }
}

fn first_check<'a, 'guard: 'a>(
    ctx: &'a Ctx,
    id: Id,
    first: &'a mut Option<FirstEffect<'guard>>,
) -> orch_engine::commands::BeforeCommand<'a> {
    Box::new(move || {
        if let Some(before) = first.take() {
            before
                .check()
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)?;
            ctx.app
                .realtime
                .terminals()
                .tell(id, Told::Over(TerminalClosedReason::ClosedByUser));
        }
        Ok(())
    })
}

impl ApplyOwned for Close {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Finishing,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Closed>>> {
        Box::pin(async move {
            let row = value.session();
            let result = Closed {
                id: row.id,
                environment: row.environment_id,
            };
            if !row.is_live() {
                before.check()?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
                return Ok(Outcome::Unchanged(result));
            }
            let session = match ctx.user()?.via {
                crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
                crate::middleware::authentication::Via::Token { .. } => None,
            };
            let shells = ctx.app.engine.shells();
            let mut first = Some(before);
            let pending = shells
                .begin_finish_before(
                    value,
                    Some((principal.caller(), session)),
                    Some(first_check(ctx, row.id, &mut first)),
                )
                .await
                .map_err(|error| match error {
                    orch_engine::EngineError::Commands(orch_engine::commands::CommandsError::AuthorityMismatch) => ApiError::new(ErrorCode::ConflictExists, "the shell already has a close owned by another origin; let that close finish"),
                    other => ApiError::engine(other),
                })?;
            if pending.queued() {
                // The durable exact close intent is accepted. Release credential/authority rows
                // before the native answer; later key loss cannot strand accepted reconciliation.
                tx.commit().await.map_err(orch_db::DbError::from)?;
                shells
                    .finish_pending(pending, Ending::CLOSED, &origin(ctx))
                    .await
                    .map_err(ApiError::engine)?;
            } else {
                // No command was queued. Keep the ordinary local CAS in this actor's guard.
                let check = first
                    .is_some()
                    .then(|| first_check(ctx, row.id, &mut first));
                shells
                    .finish_local_in(&mut tx, &pending, Ending::CLOSED, &origin(ctx), check)
                    .await
                    .map_err(ApiError::engine)?;
                let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
                guard.check_deadline(&current)?;
                tx.commit().await.map_err(orch_db::DbError::from)?;
            }
            Ok(Outcome::Applied(result))
        })
    }
}

fn association(old: &SessionRow, current: &SessionRow) -> bool {
    old.id == current.id
        && old.environment_id == current.environment_id
        && old.thread_id == current.thread_id
        && old.opened_by == current.opened_by
        && old.env_generation == current.env_generation
        && !current.is_live()
}

async fn render(
    ctx: &Ctx,
    conn: &mut Conn,
    reader: &Principal,
    row: SessionRow,
) -> ApiResult<(TerminalSession, Arc<SessionRow>)> {
    require(conn, reader, &row).await?;
    let view = views(conn, &ctx.app.engine.shells(), std::slice::from_ref(&row))
        .await?
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::internal("a closed native shell has no metadata"))?;
    Ok((view, Arc::new(row)))
}

async fn full(
    ctx: &Ctx,
    accepted: &Caller,
    closed: Closed,
) -> ApiResult<Option<(TerminalSession, Arc<SessionRow>)>> {
    let original = accepted.clone();
    native::completed(ctx, accepted, move |ctx, conn, current| {
        Box::pin(async move {
            let row = rows::get(conn, closed.id)
                .await?
                .filter(|row| row.environment_id == closed.environment && !row.is_live())
                .ok_or_else(ApiError::not_found)?;
            if native::modern(current.caller()) {
                let response = native::bind(
                    ctx,
                    &sources::CLOSE,
                    &facts(conn, closed.environment).await?,
                )?;
                let project = ctx.clone();
                response
                    .read_response(ctx, conn, &original, move |conn, reader| {
                        Box::pin(async move { render(&project, conn, reader, row).await })
                    })
                    .await
            } else {
                render(ctx, conn, current, row).await.map(Some)
            }
        })
    })
    .await
}

pub(super) async fn run(ctx: Ctx) -> ApiResult<TerminalCloseResponse> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    guard.check_deadline(&ctx)?;
    rows::lock(&mut tx, ctx.id("terminal_id")?).await?;
    let completed = effects::apply_owned::<Close>(&ctx, tx).await?;
    let ctx = completed.ctx;
    let accepted = completed.accepted;
    let closed = completed.output;
    let receipt = TerminalCloseReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        terminal_id: closed.id,
        environment_id: closed.environment,
        closed: true,
    };
    crate::key_operations::publication::receipt(&ctx, &receipt)?;
    let Some((value, opened)) = full(&ctx, &accepted, closed).await? else {
        return Ok(TerminalCloseResponse::Receipt(receipt));
    };
    crate::key_operations::publication::retain(
        &ctx,
        &accepted,
        (opened, accepted.clone()),
        |ctx, conn, _current, (opened, accepted)| {
            Box::pin(async move {
                let row = rows::get(conn, opened.id)
                    .await?
                    .filter(|row| association(opened, row))
                    .ok_or_else(ApiError::not_found)?;
                let response = native::bind(
                    ctx,
                    &sources::CLOSE,
                    &facts(conn, row.environment_id).await?,
                )?;
                response
                    .read_response(ctx, conn, accepted, move |conn, reader| {
                        Box::pin(async move { require(conn, reader, &row).await })
                    })
                    .await?
                    .ok_or_else(ApiError::not_found)
            })
        },
    )?;
    Ok(TerminalCloseResponse::Full(Box::new(value)))
}
