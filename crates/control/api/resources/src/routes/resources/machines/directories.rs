//! One on-intent native directory page, with original registration authority across the wait.

use super::{MachineRow, live_machine, registrable_machine_on, require_online};
use crate::{
    key_operations::{
        native::{self, Stored},
        targets,
    },
    middleware::{
        requests::{ApiError, ApiResult},
        transaction_guard,
    },
    registry::Ctx,
};
use futures::future::BoxFuture;
use orch_access_contract::{key_scope::KeyTargetKind, permissions::ObjectKind};
use orch_api_contract::{
    common::ErrorCode,
    machine_sources::{self as sources, Source},
    operation_sources::Declaration,
    resources::{MachineDirectoryListQuery, MachineDirectoryPage},
};
use orch_authz::{CapturedOperation, Principal};
use orch_db::{Conn, DbError};
use orch_engine::{
    commands::{MachineDirectoryAuthority, Outgoing},
    operation_scope::http_command_source::CommandEvidence,
};
use orch_link_contract::commands::{MachineDirectoryList, SerialKey};
use orch_types::new_id;

pub(super) struct Directories;
impl Stored for Directories {
    type Source = Source;
    type Value = MachineRow;
    const ROWS: &'static [Declaration<Source>] = &[sources::DIRECTORIES];
    fn load<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        _: &'static Declaration<Source>,
    ) -> BoxFuture<'a, ApiResult<(MachineRow, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = live_machine(conn, ctx.id("id")?).await?;
            if !row.is_active() {
                return Err(ApiError::not_found());
            }
            let subject = targets::machine_subject(conn, row.id).await?;
            Ok((
                row,
                vec![native::Facts::one(
                    Source::Governor,
                    KeyTargetKind::Object(ObjectKind::Machine),
                    subject,
                )],
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a MachineRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            registrable_machine_on(conn, principal, value.id)
                .await
                .map(|_| ())
        })
    }
}

fn ended() -> ApiError {
    ApiError::new(
        ErrorCode::AuthzDenied,
        "The original directory reader no longer permits this read",
    )
}

pub(super) async fn list(
    ctx: Ctx,
    query: MachineDirectoryListQuery,
    (): (),
) -> ApiResult<MachineDirectoryPage> {
    let id = ctx.id("id")?;
    let (credential, raw_until) = transaction_guard::native_origin(&ctx).await?;
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    let row = registrable_machine_on(&mut tx, guard.principal(), id).await?;
    if !row.is_active() {
        return Err(ApiError::not_found());
    }
    require_online(&ctx, &row)?;
    let limits =
        orch_engine::environments::workspace_limits_at(&mut tx, orch_settings::Place::System)
            .await
            .map_err(ApiError::engine)?;
    let read_kind = query.read_kind;
    let command = Outgoing::new(new_id(), id, SerialKey::Machine, &body(query, limits)?)
        .map_err(ApiError::internal)?;
    let source = MachineDirectoryAuthority::capture(
        &mut tx,
        guard.principal().caller(),
        credential,
        raw_until,
        &command,
        ctx.app.clock.as_ref(),
    )
    .await
    .map_err(ApiError::engine)?;
    let origin =
        CapturedOperation::capture(guard.principal().caller()).map_err(ApiError::internal)?;
    let http = native::http_origin::capture_operation(&ctx, &mut tx, &origin).await?;
    let authority = native::http_origin::bind_source(
        &http,
        &command,
        CommandEvidence::MachineDirectory(Box::new(source.clone())),
    )?;
    guard.check_deadline(&ctx)?;
    tx.commit().await.map_err(DbError::from)?;
    let outcome = request(&ctx, &guard, command, authority).await;
    current(&ctx, &source).await?;
    let reply = outcome.map_err(|error| match error {
        crate::dispatch::RunError::Command(orch_link::commands::CommandFailure::Failed(error))
            if error.code.as_str() == "policy.root_not_allowed" =>
        {
            ApiError::invalid("path", &error.message)
        }
        other => crate::routes::workspace::live::command_failure(other)
            .problem(crate::routes::workspace::problems::Doing::Reading),
    })?;
    if reply.read_kind != read_kind {
        return Err(ApiError::internal(
            "The native directory answer belongs to another read kind",
        ));
    }
    Ok(reply)
}

fn body(
    query: MachineDirectoryListQuery,
    limits: orch_link_contract::commands::WorkspaceLimits,
) -> ApiResult<MachineDirectoryList> {
    let limit = query
        .limit
        .unwrap_or(u32::try_from(limits.directory_entries).unwrap_or(u32::MAX));
    if limit == 0 {
        return Err(ApiError::invalid("limit", "must be at least 1"));
    }
    let recent: Vec<String> = query
        .recent
        .as_deref()
        .map(serde_json::from_str)
        .transpose()
        .map_err(|_| ApiError::invalid("recent", "must be a JSON array of directory names"))?
        .unwrap_or_default();
    Ok(MachineDirectoryList {
        read_kind: query.read_kind,
        root: query.root,
        path: query.path,
        prefix: query.prefix,
        recent,
        cursor: query.cursor,
        limit,
        page_bytes: limits.directory_page_bytes,
    })
}

async fn request(
    ctx: &Ctx,
    guard: &transaction_guard::Guard,
    command: Outgoing,
    authority: orch_engine::commands::CommandAuthority,
) -> Result<MachineDirectoryPage, crate::dispatch::RunError> {
    let dispatcher = ctx.app.link.dispatcher();
    dispatcher
        .issue_stored_before(
            crate::dispatch::StoredCommand {
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().into(),
                key: command.key.wire(),
                deadline: command.deadline,
                body: command.body,
                authority: Some(authority),
            },
            Box::new(|| {
                guard
                    .check_deadline(ctx)
                    .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
            }),
        )
        .await?;
    dispatcher
        .wait_for::<MachineDirectoryList>(command.id)
        .await
}

async fn current(ctx: &Ctx, source: &MachineDirectoryAuthority) -> ApiResult<()> {
    // Even a native refusal is withheld when the original HTTP reader ended during the wait.
    let (mut tx, _) = transaction_guard::begin(ctx).await?;
    let (ctx, guard) = transaction_guard::after_wait(ctx, &mut tx).await?;
    let (caller, until) = source
        .current(&mut tx, ctx.app.clock.as_ref())
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ended)?;
    guard.check_deadline(&ctx)?;
    if !caller.key_deadline_allows(ctx.app.clock.as_ref())
        || until.is_some_and(|until| until.get() <= ctx.app.clock.now())
    {
        return Err(ended());
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
