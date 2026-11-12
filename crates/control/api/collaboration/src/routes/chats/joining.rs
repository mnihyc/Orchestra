//! Stored public-group self join on the same current native action transaction.

use super::*;

use crate::key_operations::native;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_api_contract::chat_sources as sources;

use orch_api_contract::chat_sources::Source;

use orch_api_contract::operation_sources::Declaration;
struct Groups;

impl native::Stored for Groups {
    type Source = Source;
    type Value = ChatRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::JOIN];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(ChatRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::chat(conn, ctx.id("id")?)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let target = targets::chat(conn, row.id).await?;
            Ok((
                row,
                vec![native::Facts::one(Source::Chat, target.object.kind, target)],
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a ChatRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let work = Work {
                projection_ctx: ctx.clone(),
                principal: principal.clone(),
                origin: origin(ctx),
                now: principal.now().get(),
            };
            let result = chats::check_join(conn, &work.acting(), row).await;
            work.refuse(conn, Some(row.id), result).await
        })
    }
}

pub(super) fn mount(registry: &mut Registry) {
    registry.on_with_optional_operation(
        orch_api_contract::chats::ChatJoin,
        native::stored::<Groups, (), ()>,
        join,
    );
}

async fn join(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (mut tx, initial) = transaction_guard::begin(&ctx).await?;
    let row = rows::chat(&mut tx, ctx.id("id")?)
        .await?
        .ok_or_else(ApiError::not_found)?;
    <Groups as native::Stored>::require(&ctx, &mut tx, initial.principal(), &row).await?;
    rows::lock(&mut tx, row.id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let (ctx, guard) = transaction_guard::after_wait(&ctx, &mut tx).await?;
    let row = if native::modern(guard.principal().caller()) {
        guard
            .operation_data::<native::Prepared<ChatRow>>()?
            .value
            .clone()
    } else {
        rows::chat(&mut tx, row.id)
            .await?
            .ok_or_else(ApiError::not_found)?
    };
    <Groups as native::Stored>::require(&ctx, &mut tx, guard.principal(), &row).await?;
    let work = Work {
        projection_ctx: ctx.clone(),
        principal: guard.principal().clone(),
        origin: origin(&ctx),
        now: guard.principal().now().get(),
    };
    guard.check_deadline(&ctx)?;
    let result =
        chats::join_public_before(&mut tx, &work.acting(), &row, || guard.check_deadline(&ctx))
            .await;
    match result {
        Ok(()) => {}
        Err(orch_authz::pages::EffectFailure::BeforeEffect(error)) => return Err(error),
        Err(orch_authz::pages::EffectFailure::Native(error)) => {
            return work.refuse(&mut tx, Some(row.id), Err(error)).await;
        }
    }
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
