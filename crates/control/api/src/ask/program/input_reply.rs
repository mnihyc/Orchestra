//! The accepted built-in input has its own completed response Read, never another write.
use super::*;
use crate::key_operations::{admission::ResponseProjection, native, publication, targets};
use crate::registry::OperationPlan;
use orch_api_contract::threads::ThreadInput;
use orch_db::queries::threads::InputRow;
use orch_types::Hash;

#[derive(PartialEq, Eq)]
struct Source {
    thread: Id,
    input: Id,
    continuation: Id,
    author: Option<Id>,
    run_as: Option<Id>,
    turn: Option<Id>,
    content: Hash,
}

impl Source {
    fn of(row: &InputRow) -> ApiResult<Self> {
        Ok(Self {
            thread: row.thread_id,
            input: row.id,
            continuation: row.continuation_id,
            author: row.author_id,
            run_as: row.run_as,
            turn: row.turn_id,
            content: Hash::of(&serde_json::to_vec(&row.content).map_err(ApiError::internal)?),
        })
    }
}

/// Bind only this operation's actual output Read. Required history/attachment reads and
/// effects were admitted before acceptance; they are not replayed to publish its result.
async fn read_row(
    ctx: &Ctx,
    conn: &mut Conn,
    current: &Principal,
    thread: Id,
    input: Id,
) -> ApiResult<Option<InputRow>> {
    let target = targets::node_identity(conn, thread).await?;
    let selection = current
        .caller()
        .admitted_operation()
        .unwrap_or(ctx.route.name);
    let mut plan = OperationPlan::new(ctx.route, selection)?;
    orch_engine::inputs::operation::bind_thread(plan.shared_mut(), &target)
        .map_err(crate::key_operations::admission::binding_error)?;
    plan.read_response_projection(
        ctx,
        conn,
        current.caller(),
        ResponseProjection::CompletedContent,
        move |conn, reader| {
            Box::pin(async move {
                let policy = crate::routes::nodes::policy_at(conn, thread).await?;
                authorization::require(
                    conn,
                    reader,
                    thread,
                    authorization::exact(Permission::ThreadView),
                    Act::Use,
                    policy,
                )
                .await?;
                threads::input(conn, input)
                    .await?
                    .filter(|row| row.thread_id == thread)
                    .ok_or_else(ApiError::not_found)
            })
        },
    )
    .await
}

pub(super) async fn project(
    ctx: &Ctx,
    accepted: &super::super::runner::Accepted,
) -> ApiResult<Option<ThreadInput>> {
    let thread = accepted.receipt.conversation_id;
    let input = accepted.input;
    let projected = native::completed(ctx, &accepted.caller, move |ctx, conn, reader| {
        Box::pin(async move {
            let Some(row) = read_row(ctx, conn, reader, thread, input).await? else {
                return Ok(None);
            };
            let source = Source::of(&row)?;
            let people = orch_engine::views::People::load(
                conn,
                [row.author_id, row.run_as].into_iter().flatten(),
            )
            .await
            .map_err(ApiError::engine)?;
            let view = orch_engine::views::input_view(&row, &people).map_err(ApiError::engine)?;
            Ok(Some((view, source)))
        })
    })
    .await?;
    let Some((view, source)) = projected else {
        return Ok(None);
    };
    publication::retain(
        ctx,
        &accepted.caller,
        source,
        |ctx, conn, reader, source| {
            Box::pin(async move {
                let row = read_row(ctx, conn, reader, source.thread, source.input)
                    .await?
                    .ok_or_else(|| denied("The accepted input is no longer readable"))?;
                if Source::of(&row)? != *source {
                    return Err(ApiError::not_found());
                }
                Ok(())
            })
        },
    )?;
    Ok(Some(view))
}
