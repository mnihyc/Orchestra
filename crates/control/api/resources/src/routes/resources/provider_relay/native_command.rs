//! The public model call retains its original HTTP credential through the ordinary command authority.
use super::*;
use orch_engine::{
    commands::Outgoing,
    operation_scope::{http_command_source::CommandEvidence, http_origin::HttpOrigin},
};

pub(super) async fn issue(
    ctx: &Ctx,
    mut tx: orch_db::Tx,
    call: &rows::Call,
    command: Outgoing,
) -> ApiResult<(CallSource, HttpOrigin)> {
    let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let source = CallSource::capture(guard.principal().caller(), call, &command)
        .map_err(ApiError::engine)?;
    let raw = http_origin::capture_operation(&current, &mut tx, source.origin()).await?;
    source
        .check(&mut tx, ctx.app.clock.as_ref(), &raw)
        .await
        .map_err(continuing_error)?;
    let authority = http_origin::bind_source(
        &raw,
        &command,
        CommandEvidence::ProviderRelay(source.clone()),
    )?;
    guard.check_deadline(&current)?;
    tx.commit().await.map_err(DbError::from)?;
    let before = raw.clone();
    current
        .app
        .link
        .dispatcher()
        .issue_stored_before(
            crate::dispatch::StoredCommand {
                native_target: command.native_target,
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().to_owned(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body,
                authority: Some(authority),
                content_transit: false,
            },
            Box::new(|| {
                // The queue restores the source under its own transaction; this last cut owns the same raw deadline.
                if before
                    .until()
                    .is_some_and(|until| current.app.clock.time() >= until)
                {
                    return Err(Box::new(std::io::Error::other(
                        "the original HTTP credential expired",
                    ))
                        as Box<dyn std::error::Error + Send + Sync>);
                }
                Ok(())
            }),
        )
        .await
        .map_err(ApiError::internal)?;
    Ok((source, raw))
}
