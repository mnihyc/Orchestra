//! Queue the actual native upload source through the existing raw HTTP identity envelope.

use super::*;
use crate::key_operations::native::http_origin;
use orch_db::queries::uploads::targets::{State, Target};
use orch_engine::commands::Outgoing;
use orch_engine::operation_scope::{
    continuing,
    http_command_source::CommandEvidence,
    upload_session::{Prepared, SessionSource},
};

pub(super) fn mapped(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original native upload source is no longer current",
        ),
    }
}

async fn issue_inner<F>(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
    state: &State,
    target: &Target,
    command: Outgoing,
) -> ApiResult<Id>
where
    F: continuing::Source<Input = SessionSource, Value = Prepared>,
{
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let source = SessionSource::capture(
        &mut tx,
        current.route,
        guard.principal().caller(),
        state,
        target,
        &command,
    )
    .await
    .map_err(ApiError::engine)?;
    let raw = http_origin::capture_operation(&current, &mut tx, source.origin()).await?;
    let frame = raw
        .prepare::<F>(&mut tx, current.app.clock.as_ref(), source.clone())
        .await
        .map_err(mapped)?;
    let raw = frame
        .http_origin()
        .cloned()
        .ok_or_else(|| ApiError::internal("the upload lost its original HTTP source"))?;
    let authority =
        http_origin::bind_source(&raw, &command, CommandEvidence::UploadSession(source))?;
    tx.commit().await.map_err(DbError::from)?;
    let id = command.id;
    current
        .app
        .link
        .dispatcher()
        .issue_stored_before(
            crate::dispatch::StoredCommand {
                content_transit: false,
                id: Some(id),
                machine: command.machine,
                kind: command.kind.as_str().to_owned(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body,
                authority: Some(authority),
            },
            Box::new(|| {
                frame
                    .check_deadline(current.app.clock.as_ref())
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            }),
        )
        .await
        .map_err(ApiError::internal)?;
    Ok(id)
}

pub(super) async fn issue<F>(
    ctx: &Ctx,
    tx: orch_db::Tx,
    state: &State,
    target: &Target,
    command: Outgoing,
) -> ApiResult<Id>
where
    F: continuing::Source<Input = SessionSource, Value = Prepared>,
{
    use orch_api_contract::routes::Endpoint;
    if ctx.route.name == orch_api_contract::workspace::EnvironmentFileContentPut::ROUTE.name {
        issue_inner::<orch_engine::operation_scope::upload_session::Editor>(
            ctx, tx, state, target, command,
        )
        .await
    } else {
        issue_inner::<F>(ctx, tx, state, target, command).await
    }
}
