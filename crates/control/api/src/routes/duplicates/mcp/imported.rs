//! The existing owned transaction and native import ACK own stored-snapshot duplication.
use super::*;
use crate::{
    dispatch::StoredCommand,
    middleware::{
        idempotency::{self, Creation, Registered, commands::CreationCommand},
        transaction_guard,
    },
};
use orch_api_contract::duplicates::McpDuplicateBody;
use orch_engine::{
    commands::{CommandAuthority, Outgoing},
    environment_mcp::duplicating::{Command, Stored},
};
use orch_link_contract::commands::{CommandKind, McpImport};
use orch_types::Json;

pub(super) fn source_error(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            orch_api_contract::common::ErrorCode::AuthzDenied,
            "the original stored MCP duplicate is unavailable",
        ),
    }
}
pub(super) fn binding<F: CreationIdentity, B>() -> Registered<(), B, DuplicateReceipt> {
    Registered::from(Creation {
        kind: F::CREATED_REPLAY,
        identity: |reply: &DuplicateReceipt| {
            Ok(orch_db::idempotency::creation::Created {
                kind: F::CREATED_REPLAY.into(),
                id: reply.id,
                data: Json::Null,
            })
        },
        replay: |ctx, (), _body, created| {
            Box::pin(async move {
                if created.kind != F::CREATED_REPLAY {
                    return Err(ApiError::internal("the native MCP copy identity changed"));
                }
                if !created.data.is_null() {
                    let mut conn = ctx.app.db.acquire().await?;
                    let native = idempotency::commands::load(&ctx, &mut conn, &created).await?;
                    if let Some(native) = native {
                        let input: McpImport =
                            serde_json::from_value(native.body).map_err(ApiError::internal)?;
                        if input.server_id != created.id || input.stored.is_none() {
                            return Err(ApiError::internal(
                                "the native MCP duplicate belongs to another accepted copy",
                            ));
                        }
                    }
                }
                let reply = receipt(DuplicateKind::McpServer, created.id);
                crate::key_operations::publication::receipt(&ctx, &reply)?;
                Ok(reply)
            })
        },
    })
    .command(CreationCommand {
        kind: CommandKind::McpImport,
        identity: |row| {
            let input: McpImport =
                serde_json::from_value(row.body.clone()).map_err(ApiError::internal)?;
            if matches!(
                input.transfer,
                Some(orch_link_contract::commands::McpSnapshotTransfer::Issue { .. })
            ) {
                return Ok(None);
            }
            if input.stored.is_none() || input.environment_id.is_some() {
                return Err(ApiError::internal(
                    "the creating command is not a stored MCP duplicate",
                ));
            }
            Ok(Some(orch_db::idempotency::creation::Created {
                kind: F::CREATED_REPLAY.into(),
                id: input.server_id,
                data: Json::Null,
            }))
        },
    })
}
impl effects::ApplyOwned for Presets {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let New::Imported(input) = &value.new else {
                let deadline = before.deadline();
                let effects::Outcome::Applied(id) =
                    Self::apply_sql(ctx, &mut tx, principal, value, before).await?
                else {
                    return Err(ApiError::internal(
                        "a fresh duplicate did not create its native identity",
                    ));
                };
                orch_db::idempotency::creation::observe(
                    &mut tx,
                    Some(principal.user()),
                    if value.delete.is_some() {
                        Moving::CREATED_REPLAY
                    } else {
                        Self::CREATED_REPLAY
                    },
                    id,
                )
                .await?;
                deadline.check()?;
                current(ctx, principal.caller())?;
                tx.commit().await.map_err(DbError::from)?;
                return Ok(effects::Outcome::Applied(id));
            };
            let input = input.as_ref();
            ctx.retain_source(input.clone())?;
            let outgoing =
                Outgoing::new(input.id(), input.machine(), input.serial(), input.command())
                    .map_err(ApiError::internal)?;
            let origin = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            let raw = native::http_origin::capture_operation(ctx, &mut tx, &origin).await?;
            let native = Command::capture(
                &mut tx,
                principal.caller(),
                input.clone(),
                &outgoing,
                &raw,
                ctx.now.into(),
            )
            .await
            .map_err(ApiError::engine)?;
            let authority = native::http_origin::bind(
                &raw,
                &outgoing,
                CommandAuthority::mcp_duplicate_import(native.clone()),
            )?;
            tx.rollback().await.map_err(DbError::from)?;
            let issued = ctx
                .app
                .link
                .dispatcher()
                .issue_stored_before(
                    StoredCommand {
                        id: Some(outgoing.id),
                        machine: outgoing.machine,
                        kind: outgoing.kind.as_str().into(),
                        key: outgoing.key.wire(),
                        deadline: outgoing.deadline,
                        body: outgoing.body,
                        authority: Some(authority.clone()),
                    },
                    Box::new(move || {
                        before.check().map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                    }),
                )
                .await
                .map_err(|error| ApiError::internal(error.to_string()))?;
            ctx.app
                .link
                .dispatcher()
                .wait_for::<McpImport>(issued.id)
                .await
                .map_err(|_| {
                    ApiError::new(
                        orch_api_contract::common::ErrorCode::SourceOffline,
                        "the native machine could not copy this saved MCP definition",
                    )
                })?;
            let completed = if input.is_issue() {
                let mut tx = ctx.app.db.begin().await?;
                let received = native.received(&mut tx).await.map_err(ApiError::engine)?;
                let (raw, _) =
                    native::http_origin::delivery(ctx, &mut tx, &authority, issued.id).await?;
                let (caller, first_effect) = if received.moving() {
                    let frame = raw
                        .prepare::<orch_engine::environment_mcp::duplicating::Moving>(
                            &mut tx,
                            ctx.app.clock.as_ref(),
                            received.clone(),
                        )
                        .await
                        .map_err(source_error)?;
                    let parts = frame.into_parts();
                    (parts.principal.caller().clone(), parts.first_effect)
                } else {
                    let frame = raw
                        .prepare::<Stored>(&mut tx, ctx.app.clock.as_ref(), received.clone())
                        .await
                        .map_err(source_error)?;
                    let parts = frame.into_parts();
                    (parts.principal.caller().clone(), parts.first_effect)
                };
                let outgoing = Outgoing::new(
                    received.id(),
                    received.machine(),
                    received.serial(),
                    received.command(),
                )
                .map_err(ApiError::internal)?;
                let copy = Command::capture(
                    &mut tx,
                    &caller,
                    received,
                    &outgoing,
                    &raw,
                    ctx.app.clock.time(),
                )
                .await
                .map_err(ApiError::engine)?;
                let frame = raw
                    .command(
                        &outgoing,
                        CommandAuthority::mcp_duplicate_import(copy.clone()),
                    )
                    .map_err(ApiError::engine)?;
                tx.commit().await.map_err(DbError::from)?;
                let clock = ctx.app.clock.clone();
                let created = ctx
                    .app
                    .link
                    .dispatcher()
                    .issue_stored_before(
                        StoredCommand {
                            id: Some(outgoing.id),
                            machine: outgoing.machine,
                            kind: outgoing.kind.as_str().into(),
                            key: outgoing.key.wire(),
                            deadline: outgoing.deadline,
                            body: outgoing.body,
                            authority: Some(frame.clone()),
                        },
                        Box::new(move || {
                            first_effect.check(clock.as_ref()).map_err(|error| {
                                Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                            })
                        }),
                    )
                    .await
                    .map_err(|error| ApiError::internal(error.to_string()))?;
                ctx.app
                    .link
                    .dispatcher()
                    .wait_for::<McpImport>(created.id)
                    .await
                    .map_err(|_| {
                        ApiError::new(
                            orch_api_contract::common::ErrorCode::SourceOffline,
                            "the destination machine could not keep this saved MCP definition",
                        )
                    })?;
                let mut tx = ctx.app.db.begin().await?;
                let done = copy
                    .reconcile(&mut tx, ctx.app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?;
                tx.commit().await.map_err(DbError::from)?;
                (done, frame, created.id)
            } else {
                let mut tx = ctx.app.db.begin().await?;
                let done = native
                    .reconcile(&mut tx, ctx.app.clock.as_ref())
                    .await
                    .map_err(ApiError::engine)?;
                tx.commit().await.map_err(DbError::from)?;
                (done, authority, issued.id)
            };
            let (completed, authority, command) = completed;
            // The native copy is already reconciled and committed. Current delivery
            // controls optional content; it cannot erase that completed outcome.
            let _ = native::completed(ctx, principal.caller(), move |ctx, conn, _| {
                Box::pin(async move {
                    let (_, current) =
                        native::http_origin::delivery(ctx, conn, &authority, command).await?;
                    native::http_origin::check(ctx, &current)?;
                    Ok(Some(()))
                })
            })
            .await?;
            Ok(effects::Outcome::Applied(completed.server_id))
        })
    }
}
pub(super) async fn run(ctx: Ctx, (): (), _body: McpDuplicateBody) -> ApiResult<DuplicateReceipt> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Presets>(&ctx, tx).await?;
    let reply = receipt(DuplicateKind::McpServer, completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &reply)?;
    Ok(reply)
}

impl effects::ApplyOwned for Moving {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        <Presets as effects::ApplyOwned>::apply_owned(ctx, tx, principal, value, before)
    }
}
pub(super) async fn run_move(
    ctx: Ctx,
    (): (),
    _: orch_api_contract::duplicates::McpMoveOutBody,
) -> ApiResult<DuplicateReceipt> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Moving>(&ctx, tx).await?;
    completed.ctx.app.engine.kick();
    let reply = receipt(DuplicateKind::McpServer, completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &reply)?;
    Ok(reply)
}
