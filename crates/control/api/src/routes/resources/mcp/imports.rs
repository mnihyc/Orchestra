//! The two stored-definition requests share one actual native source, copy and completion.

use super::*;
use crate::routes::{access::operations::NameRoles, resources::projection::single};
use crate::{
    dispatch::StoredCommand,
    key_operations::{
        native::{self, effects},
        publication,
    },
    middleware::{
        authentication::Via,
        transaction_guard::{self, HttpIdentityMode},
    },
};
use futures::future::BoxFuture;
use orch_access_contract::{decisions::ObjectRef, permissions::ObjectKind};
use orch_api_contract::{
    common::ReceiptProjection,
    mcp_sources::{self as sources, Source},
    operation_sources::Declaration,
    resources::{McpServerImportReceipt, McpServerImportResponse, Resource},
};
use orch_authz::Caller;
use orch_engine::operation_scope::continuing::Source as _;
use orch_engine::{
    commands::{CommandAuthority, Outgoing},
    environment_mcp::existing::{self, Input, Intent, Kind, Stored},
};
use orch_link_contract::commands::McpImport;
use orch_types::Time;
use std::marker::PhantomData;

const ROLES: single::Roles = single::Roles {
    names: NameRoles {
        groups: Some(sources::GROUPS),
        nodes: [
            (ObjectKind::Org, sources::NODES[0]),
            (ObjectKind::Project, sources::NODES[1]),
            (ObjectKind::Topic, sources::NODES[2]),
            (ObjectKind::Thread, sources::NODES[3]),
        ],
        containers: None,
    },
    sources: sources::REFERENCES,
};
pub(super) trait Request: Kind {
    type Body: Send + 'static;
    fn intent(ctx: &Ctx, body: Self::Body) -> ApiResult<Intent>;
}
impl Request for existing::Reimport {
    type Body = ();
    fn intent(ctx: &Ctx, (): ()) -> ApiResult<Intent> {
        Ok(Intent::Reimport {
            server: ctx.id("id")?,
        })
    }
}
impl Request for existing::Offering {
    type Body = api::McpOfferingUseBody;
    fn intent(ctx: &Ctx, body: Self::Body) -> ApiResult<Intent> {
        Ok(Intent::Offering {
            server: ctx.id("id")?,
            machine: ctx.id("machine_id")?,
            expected_revision: body.expected_revision,
            expected_fingerprint: body.expected_fingerprint,
        })
    }
}
pub(super) struct Imports<K>(PhantomData<K>);
fn selector_error(error: existing::Error) -> ApiError {
    match error {
        existing::Error::Missing => ApiError::not_found(),
        existing::Error::NotImported => {
            invalid("id", "this server was not imported from a machine")
        }
        existing::Error::NoOffering => ApiError::new(
            ErrorCode::SourceOffline,
            "No machine offers this imported server",
        ),
        existing::Error::Review => ApiError::new(
            ErrorCode::PlanBlocked,
            "Review the different definition and choose Use this machine's",
        ),
        existing::Error::StaleRevision => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The server changed; review its offerings again",
        ),
        existing::Error::StaleFingerprint => ApiError::new(
            ErrorCode::ConflictStaleRevision,
            "The reported definition changed; review it again",
        ),
        existing::Error::NoEnvironment => ApiError::new(
            ErrorCode::AuthzDenied,
            "no environment of this machine permits this caller to reimport",
        ),
        existing::Error::Import(error) => match error {
            orch_engine::environment_mcp::Error::Invalid { field, message } => {
                invalid(field, message)
            }
            orch_engine::environment_mcp::Error::NotFound => ApiError::not_found(),
            orch_engine::environment_mcp::Error::Offline => ApiError::new(
                ErrorCode::SourceOffline,
                "the environment has no connected machine",
            ),
            orch_engine::environment_mcp::Error::Internal(error) => ApiError::engine(error),
        },
        existing::Error::Native(error) => ApiError::engine(error),
    }
}
fn source_error(error: orch_engine::operation_scope::continuing::Error) -> ApiError {
    match error {
        orch_engine::operation_scope::continuing::Error::Native(error) => ApiError::engine(error),
        _ => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original stored MCP copy source is unavailable",
        ),
    }
}
impl<K: Request> effects::Effect for Imports<K> {
    type Source = Source;
    type Query = ();
    type Body = K::Body;
    type Value = Input;
    type Output = existing::Completion;
    fn declaration(_input: &Input) -> &'static Declaration<Source> {
        K::DECLARATION
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let intent = K::intent(ctx, body)?;
            let row = rows::servers(conn, &[intent.server()])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let mut known = existing::facts(conn, &row, None)
                .await
                .map_err(ApiError::engine)?;
            // Copy operands are unresolved until the actual selector chooses one.
            known.retain(|fact| {
                !matches!(
                    fact.source,
                    Source::ImportEnvironment | Source::ImportMachine
                )
            });
            let known = native::bind_known(ctx, K::DECLARATION, &known)?;
            let principal = native::preparation_principal(ctx, conn, caller, &known).await?;
            require_row(conn, &principal, &row, true).await?;
            let input = match ctx.optional_retained_source::<Input>()? {
                Some(input) => {
                    if input.intent() != &intent {
                        return Err(ApiError::internal(
                            "the native import request changed its retained source",
                        ));
                    }
                    (*input).clone()
                }
                None => {
                    let selected = existing::select::<K>(conn, caller, Time::new(ctx.now), &intent)
                        .await
                        .map_err(selector_error)?;
                    Input::capture(intent, orch_types::new_id(), &selected)
                        .map_err(ApiError::engine)?
                }
            };
            let (selected, facts) = Stored::<K>::prepare(conn, caller, &input, Time::new(ctx.now))
                .await
                .map_err(source_error)?;
            let plan = native::bind_required(ctx, K::DECLARATION, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                Stored::<K>::locks(conn, &principal, &input, &selected)
                    .await
                    .map_err(source_error)?;
            }
            let (_, facts) = Stored::<K>::prepare(conn, caller, &input, Time::new(ctx.now))
                .await
                .map_err(source_error)?;
            Ok((input, facts))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let (selected, _) =
                Stored::<K>::prepare(conn, principal.caller(), input, Time::new(ctx.now))
                    .await
                    .map_err(source_error)?;
            Stored::<K>::require(conn, principal, input, &selected)
                .await
                .map_err(source_error)
        })
    }
}
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
fn rpc_error(error: crate::dispatch::RunError) -> ApiError {
    if let crate::dispatch::RunError::Command(orch_link::commands::CommandFailure::Failed(
        refused,
    )) = &error
    {
        if refused.code == "policy.mcp_import_disabled" {
            return ApiError::new(
                ErrorCode::PolicyMcpImportDisabled,
                "the machine now permits no MCP imports",
            );
        }
    }
    ApiError::new(
        ErrorCode::SourceOffline,
        "the machine could not copy this MCP definition",
    )
}
impl<K: Request> effects::ApplyOwned for Imports<K> {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        input: &'a Input,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Self::Output>>> {
        Box::pin(async move {
            ctx.retain_source(input.clone())?;
            let identity =
                transaction_guard::current_http_identity(ctx, &mut tx, HttpIdentityMode::Read)
                    .await?;
            let session = match &identity.user.via {
                Via::Session { session_id } => Some(*session_id),
                Via::Token { .. } => None,
            };
            let command =
                Outgoing::new(input.id(), input.machine(), input.serial(), input.command())
                    .map_err(ApiError::internal)?;
            let copy = existing::Command::capture(
                &mut tx,
                principal.caller(),
                session,
                identity.expires_at,
                input.clone(),
                &command,
                Time::new(ctx.now),
            )
            .await
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
                        authority: Some(CommandAuthority::mcp_existing_import(copy.clone())),
                    },
                    Box::new(move || {
                        before.check().map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                    }),
                )
                .await;
            if let Err(error) = issued {
                return Err(
                    failed(ctx, rpc_error(crate::dispatch::RunError::Dispatch(error))).await,
                );
            }
            if let Err(error) = ctx
                .app
                .link
                .dispatcher()
                .wait_for::<McpImport>(command.id)
                .await
            {
                return Err(failed(ctx, rpc_error(error)).await);
            }
            // The same reconciler normally ran atomically with the native acknowledgment.
            // This idempotent path also serves the existing database-backed scripted peer.
            let mut tx = ctx.app.db.begin().await?;
            let completed = copy
                .reconcile(&mut tx, ctx.app.clock.as_ref())
                .await
                .map_err(ApiError::engine)?;
            tx.commit().await.map_err(DbError::from)?;
            Ok(effects::Outcome::Applied(completed))
        })
    }
}
struct Saved<K>(PhantomData<K>);
impl<K: Kind> native::Stored for Saved<K> {
    type Source = Source;
    type Value = McpRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::REIMPORT, sources::OFFERING_USE];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _declaration: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(McpRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = rows::servers(conn, &[ctx.id("id")?])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let facts = existing::facts(conn, &row, None)
                .await
                .map_err(ApiError::engine)?;
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require_row(conn, principal, row, true))
    }
}
impl<K: Kind> native::resource_reply::StoredResource for Saved<K> {
    const READ_SOURCE: Source = Source::Subject;
    const ROLES: &'static single::Roles = &ROLES;
    fn object(row: &McpRow) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::McpServer,
            id: row.id,
        }
    }
    fn require_read<'a>(
        _ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require_row(conn, principal, row, false).await {
                Ok(()) => Ok(()),
                Err(error)
                    if native::modern(principal.caller())
                        || !matches!(error.problem.status, 403 | 404) =>
                {
                    Err(error)
                }
                Err(_) => require_row(conn, principal, row, true).await,
            }
        })
    }
    fn check_opened<'a>(
        _ctx: &'a Ctx,
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        old: &'a McpRow,
        fresh: &'a McpRow,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.id == fresh.id
                && old.home_id == fresh.home_id
                && old.org_id == fresh.org_id
                && old.environment_id == fresh.environment_id
                && old.machine_id == fresh.machine_id
                && old.imported == fresh.imported)
        })
    }
}
fn receipt(outcome: &existing::Completion) -> McpServerImportResponse {
    McpServerImportResponse::Receipt(McpServerImportReceipt {
        projection: ReceiptProjection::Receipt,
        server_id: outcome.server_id,
        revision: outcome.revision,
        selected: outcome.selected,
        cleanup_queued: outcome.cleanup_queued,
    })
}
async fn run<K: Request>(ctx: Ctx) -> ApiResult<McpServerImportResponse> {
    let (tx, _) = transaction_guard::begin(&ctx).await?;
    let completed = effects::apply_owned::<Imports<K>>(&ctx, tx).await?;
    let restricted = receipt(&completed.output);
    publication::receipt(&completed.ctx, &restricted)?;
    if completed.output.revision.is_none() {
        return Ok(restricted);
    }
    Ok(
        match native::resource_reply::stored::<Saved<K>>(&completed.ctx, &completed.accepted)
            .await?
        {
            Some(Resource::McpServer(server)) => McpServerImportResponse::full(server),
            None => restricted,
            Some(_) => {
                return Err(ApiError::internal(
                    "the native imported MCP response has another kind",
                ));
            }
        },
    )
}
pub(super) async fn reimport(ctx: Ctx, (): (), (): ()) -> ApiResult<McpServerImportResponse> {
    run::<existing::Reimport>(ctx).await
}
pub(super) async fn offering(
    ctx: Ctx,
    (): (),
    _body: api::McpOfferingUseBody,
) -> ApiResult<McpServerImportResponse> {
    run::<existing::Offering>(ctx).await
}
