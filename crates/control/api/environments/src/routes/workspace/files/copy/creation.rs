//! The accepted native copy result is read through the same current optional result projection.

use super::*;

use crate::middleware::idempotency::ObservedCreation;

use crate::middleware::idempotency::Registered;

use crate::middleware::idempotency::commands;

use orch_db::idempotency::creation::Created;

use orch_db::queries::link;
const KIND: &str = "workspace_file_copy";

pub(crate) fn binding() -> Registered<ThroughThreadQuery, FileFromToBody, FileCopyResponse> {
    Registered::from(ObservedCreation {
        kind: KIND,
        replay: |ctx, query: ThroughThreadQuery, body: FileFromToBody, created| {
            Box::pin(async move {
                let environment = ctx.id("id")?;
                let (from, to) = two_paths(&body)?;
                if created.kind != KIND || created.data["environment_id"] != serde_json::json!(environment)
                    || created.data["path"] != serde_json::json!(to) {
                    return Err(ApiError::internal("the accepted file-copy association changed"));
                }
                let fallback = FileCopyResponse::Receipt(FileCopyReceipt {
                    projection: ReceiptProjection::Receipt,
                    environment_id: environment,
                    path: to.clone(),
                });
                crate::key_operations::publication::receipt(&ctx, &fallback)?;
                let copied = copied(&ctx, &created, environment, query.thread, &from, &to).await?;
                let Some(copied) = copied else { return Ok(fallback); };
                match reply::render_completed_replay::<Projection>(&ctx, ctx.user()?.id(), copied).await {
                    Ok(Some(stat)) => Ok(FileCopyResponse::Full(Box::new(stat))),
                    Ok(None) => Ok(fallback),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(fallback),
                    Err(error) => Err(error),
                }
            })
        },
    }).command(commands::CreationCommand {
        kind: CommandKind::FsCopy,
        identity: |row| {
            let command: FsCopy = serde_json::from_value(row.body.clone()).map_err(ApiError::internal)?;
            Ok(Some(Created { kind: KIND.into(), id: row.id,
                data: serde_json::json!({"environment_id":command.environment_id,"path":command.to}) }))
        },
    })
}

async fn copied(
    ctx: &Ctx,
    created: &Created,
    environment: Id,
    thread: Option<Id>,
    from: &str,
    to: &str,
) -> ApiResult<Option<Copied>> {
    let mut conn = ctx.app.db.acquire().await?;
    let Some(row) = commands::load(ctx, &mut conn, created)
        .await?
        .filter(|row| row.id == created.id && row.state == "succeeded")
    else {
        return Ok(None);
    };
    let Some(stored) = link::command_with_authority(&mut conn, row.id).await? else {
        return Ok(None);
    };
    if stored.command != row {
        return Ok(None);
    }
    let Some(authority) = stored.authority else {
        return Ok(None);
    };
    let authority: CommandAuthority =
        serde_json::from_value(authority).map_err(ApiError::internal)?;
    let Some(input) = authority
        .file_copy_receipt(&row)
        .map_err(ApiError::engine)?
    else {
        return Ok(None);
    };
    let command = input.command();
    if input.environment() != environment
        || input.through_thread() != thread
        || command.from != from
        || command.to != to
    {
        return Ok(None);
    }
    let Some(result) = row.result else {
        return Ok(None);
    };
    let stat: FileStat = serde_json::from_value(result).map_err(ApiError::internal)?;
    Ok(Some(Copied {
        input,
        stat,
        facts: facts(&mut conn, environment).await?,
    }))
}
