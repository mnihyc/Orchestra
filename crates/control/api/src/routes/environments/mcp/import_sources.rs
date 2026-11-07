//! One original offering, once-only native copy and guarded filing with optional resource Read.

use super::*;
use crate::{
    dispatch::{RunError, StoredCommand},
    key_operations::native::{
        self,
        effects::{ApplyOwned, Deadline, Effect, FirstEffect, Outcome},
    },
    middleware::{authentication::Via, transaction_guard},
};
use futures::future::BoxFuture;
use orch_api_contract::{
    environment_sources::{MCP_IMPORT, McpCreateSource as Source},
    operation_sources::Declaration,
};
use orch_authz::Caller;
use orch_db::Tx;
use orch_engine::{
    commands::{CommandAuthority, Outgoing},
    environment_mcp::{
        self, CopyCommand, ForgetCommand, ImportSelection,
        importing::{Input, Offering},
    },
    operation_scope::continuing::{self, Source as NativeSource},
};
use orch_link_contract::commands::McpImport;

pub(super) struct Imports;
pub(super) struct Prepared {
    input: Input,
    selected: ImportSelection,
    name: String,
    enable: bool,
}
pub(super) struct Filed {
    pub projection: create_sources::Created,
    pub accepted: Caller,
}

fn source_error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused
        | continuing::Error::Binding(orch_engine::operation_scope::BindingError::Refused) => {
            ApiError::new(
                ErrorCode::AuthzDenied,
                "the original MCP import source is no longer current",
            )
        }
        continuing::Error::Binding(error) => ApiError::internal(error),
        continuing::Error::Native(error) => {
            crate::key_operations::snapshot_read::engine_error(error)
        }
    }
}
fn rpc_error(error: RunError) -> ApiError {
    if let RunError::Command(orch_link::commands::CommandFailure::Failed(refused)) = &error
        && refused.code == "policy.mcp_import_disabled"
    {
        return ApiError::new(
            ErrorCode::PolicyMcpImportDisabled,
            "the machine now permits no MCP imports",
        );
    }
    ApiError::new(
        ErrorCode::SourceOffline,
        "the machine could not copy this MCP definition",
    )
}

async fn require_import(conn: &mut Conn, principal: &Principal, environment: Id) -> ApiResult<()> {
    let gate = check_import(conn, principal, environment).await;
    if native::modern(principal.caller()) {
        gate.map_err(authorization::problem_without_content)
    } else {
        gate
    }
}

impl Effect for Imports {
    type Source = Source;
    type Query = ();
    type Body = api::EnvironmentMcpImportCreateBody;
    type Value = Prepared;
    type Output = Filed;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &MCP_IMPORT
    }

    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let name = resources::name(body.as_name.as_deref().unwrap_or(&body.name))?;
            let facts = create_sources::parent_facts(conn, environment).await?;
            let plan = native::bind_required(ctx, &MCP_IMPORT, &facts)?;
            let principal = plan.preparation_principal(ctx, conn, caller).await?;
            require_import(conn, &principal, environment).await?;
            environments::lock_environment(conn, environment)
                .await?
                .ok_or_else(ApiError::not_found)?;
            require_import(conn, &principal, environment).await?;
            let scope = serde_json::to_value(body.scope).map_err(ApiError::internal)?;
            let input = match ctx.optional_retained_source::<Input>()? {
                Some(input) => {
                    if !input.matches_request(
                        environment,
                        &body.source,
                        &scope,
                        body.directory.as_deref(),
                        &body.name,
                    ) {
                        return Err(ApiError::not_found());
                    }
                    (*input).clone()
                }
                None => {
                    let selected = environment_mcp::select_import(
                        conn,
                        environment,
                        &body.source,
                        scope,
                        body.directory,
                        &body.name,
                    )
                    .await
                    .map_err(policy_error)?;
                    Input::capture(conn, &selected, orch_types::new_id())
                        .await
                        .map_err(ApiError::engine)?
                }
            };
            let (selected, facts) = Offering::prepare(conn, caller, &input, ctx.app.clock.time())
                .await
                .map_err(source_error)?;
            Ok((
                Prepared {
                    input,
                    selected,
                    name,
                    enable: body.enable.unwrap_or(false),
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            require_import(conn, principal, value.input.environment()).await?;
            Offering::require(conn, principal, &value.input, &value.selected)
                .await
                .map_err(source_error)
        })
    }
}

/// The action and this immutable original source share this one fresh filing transaction.
async fn file(
    ctx: &Ctx,
    value: &Prepared,
    copy: &CopyCommand,
    reply: McpImportResult,
    original: &Deadline<'_>,
) -> ApiResult<Filed> {
    let mut tx = ctx.app.db.begin().await?;
    let outcome = async {
        let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
        let frame = continuing::prepare_with_caller::<Offering>(
            &mut tx,
            ctx.app.clock.as_ref(),
            copy.origin(),
            guard.principal().caller(),
            value.input.clone(),
        )
        .await
        .map_err(source_error)?;
        let parts = frame.into_parts();
        let row = McpRow {
            imported: true,
            id: value.input.server(),
            name: value.name.clone(),
            description: String::new(),
            org_id: Some(parts.value.environment().org_id),
            home_id: Some(parts.value.environment().org_id),
            owner_id: Some(value.input.owner(parts.principal.caller())),
            machine_id: Some(value.input.machine()),
            environment_id: None,
            spec: import_spec(ctx, &parts.value, reply)?,
            enabled: true,
            revision: 1,
        };
        let projection = create_sources::Created::imported(value.input.environment(), &row)?;
        original.check()?;
        guard.check_deadline(&current)?;
        parts
            .first_effect
            .check(ctx.app.clock.as_ref())
            .map_err(source_error)?;
        file_import(
            &current,
            &mut tx,
            &parts.principal,
            Filing {
                environment: value.input.environment(),
                row: &row,
                enable: value.enable,
                command_line: false,
            },
        )
        .await
        .map_err(FilingError::problem)?;
        original.check()?;
        guard.check_deadline(&current)?;
        if !parts
            .principal
            .caller()
            .key_deadline_allows(ctx.app.clock.as_ref())
        {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the original import key has ended",
            ));
        }
        Ok(Filed {
            projection,
            accepted: parts.principal.caller().clone(),
        })
    }
    .await;
    match outcome {
        Ok(filed) => {
            tx.commit().await.map_err(DbError::from)?;
            Ok(filed)
        }
        Err(error) => {
            tx.rollback().await.map_err(DbError::from)?;
            Err(error)
        }
    }
}

/// A failed filing owns only its completed private copy; no user authority is renewed.
async fn forget(ctx: &Ctx, copy: CopyCommand) {
    let result = async {
        let command = Outgoing::new(
            orch_types::new_id(),
            copy.input().machine(),
            SerialKey::Machine,
            &McpForget {
                server_id: copy.input().server(),
            },
        )
        .map_err(ApiError::internal)?;
        let evidence = ForgetCommand::capture(
            &mut *ctx.app.db.acquire().await?,
            ctx.app.clock.as_ref(),
            copy,
            &command,
        )
        .await
        .map_err(ApiError::engine)?;
        ctx.app
            .link
            .dispatcher()
            .issue_stored(StoredCommand {
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().to_owned(),
                key: command.key.wire(),
                deadline: Some(evidence.deadline()),
                body: command.body,
                authority: Some(CommandAuthority::mcp_forget(evidence)),
            })
            .await
            .map_err(ApiError::internal)?;
        Ok::<(), ApiError>(())
    }
    .await;
    if result.is_err() {
        tracing::warn!("an unfiled native MCP copy could not be queued for cleanup");
    }
}

/// The shared action resolver supplies current credential errors after an unsuccessful copy.
async fn failed(ctx: &Ctx, error: ApiError) -> ApiError {
    let current = async {
        let mut tx = ctx.app.db.begin().await?;
        let (current, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
        guard.check_deadline(&current)?;
        tx.rollback().await.map_err(DbError::from)?;
        Ok::<(), ApiError>(())
    }
    .await;
    current.err().unwrap_or(error)
}

impl ApplyOwned for Imports {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Filed>>> {
        Box::pin(async move {
            ctx.retain_source(value.input.clone())?;
            let original = before.deadline();
            let command = Outgoing::new(
                orch_types::new_id(),
                value.input.machine(),
                value.input.serial(),
                &value.input.command(),
            )
            .map_err(ApiError::internal)?;
            let session = match &ctx.user()?.via {
                Via::Session { session_id } => Some(*session_id),
                Via::Token { .. } => None,
            };
            let copy =
                CopyCommand::capture(principal.caller(), session, value.input.clone(), &command)
                    .map_err(ApiError::engine)?;
            tx.rollback().await.map_err(DbError::from)?;
            let issued = ctx
                .app
                .link
                .dispatcher()
                .issue_stored_before(
                    StoredCommand {
                        id: Some(command.id),
                        machine: command.machine,
                        kind: command.kind.as_str().to_owned(),
                        key: command.key.wire(),
                        deadline: command.deadline,
                        body: command.body,
                        authority: Some(CommandAuthority::mcp_import(copy.clone())),
                    },
                    Box::new(move || {
                        before.check().map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                    }),
                )
                .await;
            if let Err(error) = issued {
                return Err(failed(ctx, rpc_error(RunError::Dispatch(error))).await);
            }
            let reply = match ctx
                .app
                .link
                .dispatcher()
                .wait_for::<McpImport>(command.id)
                .await
            {
                Ok(reply) => reply,
                Err(error) => return Err(failed(ctx, rpc_error(error)).await),
            };
            match file(ctx, value, &copy, reply, &original).await {
                Ok(filed) => Ok(Outcome::Applied(filed)),
                Err(error) => {
                    forget(ctx, copy).await;
                    Err(error)
                }
            }
        })
    }
}
