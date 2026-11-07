//! One native prepared deletion and its atomically recorded copied-file cleanup.

use super::*;
use crate::key_operations::{
    native::{self, effects},
    targets,
};
use crate::middleware::transaction_guard;
use futures::future::BoxFuture;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    mcp_actions::{self, Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_engine::commands::{CommandAuthority, Outgoing};
use orch_engine::environment_mcp::ForgetCommand;
use orch_link_contract::commands::{CommandBody, McpForget, SerialKey};
use orch_types::Time;

pub(super) struct Delete;
pub(super) struct Value {
    row: McpRow,
    copies: Vec<rows::Offering>,
}
pub(super) struct Output {
    commands: Vec<Outgoing>,
}

async fn subject_fact(conn: &mut Conn, subject: ObjectRef) -> ApiResult<native::Facts<Source>> {
    Ok(native::Facts::one(
        Source::ManagedServer,
        KeyTargetKind::Object(ResourceKind::McpServer.into()),
        targets::page_object(conn, subject).await?,
    ))
}
async fn copies(conn: &mut Conn, server: Id) -> ApiResult<Vec<rows::Offering>> {
    let mut selected = Vec::new();
    for offering in rows::offerings(conn, &[server]).await? {
        if orch_db::queries::resources::machine(conn, offering.machine_id)
            .await?
            .is_some_and(|machine| machine.online && machine.is_active())
        {
            selected.push(offering);
        }
    }
    Ok(selected)
}

impl effects::Effect for Delete {
    type Source = Source;
    type Query = ();
    type Body = ();
    type Value = Value;
    type Output = Output;
    fn declaration(_value: &Value) -> &'static Declaration<Source> {
        &mcp_actions::DELETE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        (): (),
    ) -> BoxFuture<'a, ApiResult<(Value, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let subject = rows::subject(conn, id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            let governor = match subject.environment_id {
                Some(id) => ObjectRef {
                    kind: ResourceKind::Environment.into(),
                    id,
                },
                None => ObjectRef {
                    kind: ResourceKind::McpServer.into(),
                    id,
                },
            };
            let known = native::bind_known(
                ctx,
                &mcp_actions::DELETE,
                &[subject_fact(conn, governor).await?],
            )?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_subject(conn, &principal, id, subject.environment_id, true).await?;
            let writing = orch_db::queries::transaction_guard::in_transaction(conn);
            if writing {
                rows::lock_definition(conn, id).await?;
                // Keep the shared definition fence first, then own parent before child/name rows.
                if let Some(environment) = subject.environment_id {
                    let parent: Option<Id> =
                        sqlx::query_scalar("SELECT id FROM environments WHERE id=$1 FOR UPDATE")
                            .bind(environment)
                            .fetch_optional(&mut *conn)
                            .await
                            .map_err(DbError::from)?;
                    if parent.is_none() {
                        return Err(ApiError::not_found());
                    }
                }
            }
            let row = if writing {
                rows::lock(conn, id).await?
            } else {
                rows::servers(conn, &[id]).await?.pop()
            }
            .ok_or_else(ApiError::not_found)?;
            if row.environment_id != subject.environment_id {
                return Err(ApiError::new(
                    ErrorCode::ConflictStaleRevision,
                    "the server's owner changed; send the deletion again",
                ));
            }
            let mut selected = copies(conn, id).await?;
            if writing {
                let machine_ids: Vec<Id> = sqlx::query_scalar("SELECT machine_id FROM mcp_server_offerings WHERE server_id=$1 ORDER BY machine_id FOR UPDATE")
                    .bind(id).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                let _ =
                    sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
                        .bind(machine_ids)
                        .fetch_all(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                let _ = sqlx::query("SELECT name FROM secrets WHERE owner_kind='mcp_server' AND owner_id=$1 ORDER BY name FOR UPDATE")
                    .bind(id).fetch_all(&mut *conn).await.map_err(DbError::from)?;
                selected = copies(conn, id).await?;
            }
            let ids: Vec<Id> = selected
                .iter()
                .map(|offering| offering.machine_id)
                .collect();
            let mut actual = targets::machine_subjects(conn, &ids).await?;
            let targets = ids
                .iter()
                .map(|id| actual.remove(id).ok_or_else(ApiError::not_found))
                .collect::<ApiResult<Vec<_>>>()?;
            let mut facts = vec![subject_fact(conn, projection::subject(&row)).await?];
            facts.push(native::Facts::collection(
                Source::ForgottenCopies,
                if targets.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ResourceKind::Machine.into()), targets)]
                },
            ));
            Ok((
                Value {
                    row,
                    copies: selected,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_row(conn, principal, &value.row, true))
    }
}

impl effects::ApplyOwned for Delete {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Value,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let mut commands = Vec::new();
            for offering in &value.copies {
                let mut command = Outgoing::new(
                    orch_types::new_id(),
                    offering.machine_id,
                    SerialKey::Machine,
                    &McpForget {
                        server_id: value.row.id,
                    },
                )
                .map_err(ApiError::internal)?;
                let cleanup = ForgetCommand::capture_recorded_deletion(
                    &mut tx,
                    principal.caller(),
                    offering,
                    Time::new(ctx.now),
                    &command,
                )
                .await
                .map_err(ApiError::internal)?;
                command.authority = Some(CommandAuthority::mcp_forget(cleanup));
                commands.push(command);
            }
            before.check()?;
            rows::delete(&mut tx, value.row.id, ctx.now).await?;
            record(
                ctx,
                &mut tx,
                orch_db::audit::action!(AuditAction::McpDeleted),
                value.row.id,
                home(&value.row),
                "Deleted an MCP server",
                json!({"server":value.row.id,"name":value.row.name}),
            )
            .await?;
            for command in &mut commands {
                deadline.check()?;
                command.deadline = Some(Time::new(
                    ctx.app.clock.now()
                        + chrono::Duration::seconds(i64::from(McpForget::KIND.deadline_secs())),
                ));
                let authority = command
                    .authority
                    .as_ref()
                    .map(serde_json::to_value)
                    .transpose()
                    .map_err(ApiError::internal)?;
                orch_db::queries::link::insert_command_with_authority(
                    &mut tx,
                    &orch_db::queries::link::NewCommand {
                        id: command.id,
                        machine_id: command.machine,
                        kind: command.kind.as_str(),
                        body: &command.body,
                        serial_key: command.key.wire().as_deref(),
                        deadline_at: command
                            .deadline
                            .ok_or_else(|| {
                                ApiError::internal("the native cleanup has no deadline")
                            })?
                            .get(),
                    },
                    ctx.now,
                    authority.as_ref(),
                )
                .await?;
            }
            deadline.check()?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(Output { commands }))
        })
    }
}

pub(super) async fn run(ctx: Ctx, (): (), (): ()) -> ApiResult<()> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Delete>(&ctx, tx).await?;
    // These commands are already durable in the deletion's committed transaction.
    // Reopening the dispatcher merely starts their existing journal; no new act is admitted.
    let mut issued = Vec::new();
    for command in completed.output.commands {
        let dispatcher = ctx.app.link.dispatcher();
        if dispatcher
            .issue_stored(crate::dispatch::StoredCommand {
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().into(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body,
                authority: command.authority,
            })
            .await
            .is_ok()
        {
            issued.push(command.id);
        }
    }
    for id in issued {
        let _ = ctx.app.link.dispatcher().wait_for::<McpForget>(id).await;
    }
    Ok(())
}
