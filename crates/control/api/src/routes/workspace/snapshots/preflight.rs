//! One native measurement outside retries, then the ordinary watched source projection.

use super::*;
use crate::key_operations::{
    native::{
        self,
        effects::{self, Effect, Initial},
    },
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef, key_scope::KeyTargetKind, permissions::ObjectKind,
};
use orch_api_contract::{
    operation_sources::Declaration,
    workspace_sources::{self as sources, Source},
};
use orch_authz::{Caller, CapturedOperation};
use orch_db::Conn;
use orch_engine::{
    commands::{
        CommandAuthority, Outgoing, SnapshotMeasureInput, SnapshotMeasureSource, WorkspaceCommand,
    },
    operation_scope::continuing::{self, Source as NativeSource},
};

pub(super) struct Measure;
pub(super) struct Input {
    source: SnapshotMeasureInput,
    origin: CapturedOperation,
}
pub(super) struct Fetched {
    source: SnapshotMeasureInput,
    origin: CapturedOperation,
    measured: SnapshotPreflight,
}

pub(super) fn error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original snapshot measurement source is no longer available",
        ),
        continuing::Error::Native(error) => snapshot_read::engine_error(error),
        continuing::Error::Binding(error) => crate::key_operations::admission::binding_error(error),
    }
}

async fn current(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Arc<Fetched>, Vec<native::Facts<Source>>)> {
    let fetched = effects::initial_value::<Measure>(ctx)?;
    let (_, facts) =
        SnapshotMeasureSource::prepare(conn, caller, &fetched.source, ctx.app.clock.time())
            .await
            .map_err(error)?;
    Ok((fetched, facts))
}
async fn require(ctx: &Ctx, conn: &mut Conn, principal: &Principal) -> ApiResult<()> {
    let fetched = effects::initial_value::<Measure>(ctx)?;
    let env = fetched
        .source
        .current(conn)
        .await
        .map_err(snapshot_read::engine_error)?
        .ok_or_else(ApiError::not_found)?;
    require_snapshot(conn, principal, env.id).await
}

impl Effect for Measure {
    type Source = Source;
    type Query = ();
    type Body = ThreadSnapshotPreflightBody;
    type Value = Arc<Fetched>;
    type Output = SnapshotPreflight;
    fn declaration(_: &Self::Value) -> &'static Declaration<Source> {
        &sources::THREAD_SNAPSHOT_PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        _: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        Box::pin(current(ctx, conn, caller))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal))
    }
}

impl Initial for Measure {
    type Input = Input;
    type Fetched = Fetched;
    fn declaration(_: &Input) -> &'static Declaration<Source> {
        &sources::THREAD_SNAPSHOT_PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        body: ThreadSnapshotPreflightBody,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (env, directory) = snapshot_environment(ctx, conn).await?;
            let target = targets::page_object(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: env.id,
                },
            )
            .await?;
            let facts = vec![native::Facts::one(
                Source::SnapshotEnvironment,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            )];
            let known = native::bind_required(ctx, &sources::THREAD_SNAPSHOT_PREFLIGHT, &facts)?;
            let admitted = known
                .shared()
                .admit(caller, false)
                .map_err(crate::key_operations::admission::binding_error)?;
            let principal = native::preparation_principal(ctx, conn, &admitted, &known).await?;
            require_snapshot(conn, &principal, env.id).await?;
            let (machine, selection) =
                snapshot_paths(ctx, conn, &env, &directory, body.selection.as_ref()).await?;
            let source = SnapshotMeasureInput::new(
                ctx.id("id")?,
                &env,
                machine,
                body.selection.is_none().then_some(directory),
                selection,
            );
            let origin = CapturedOperation::capture(&admitted).map_err(ApiError::internal)?;
            Ok((Input { source, origin }, facts))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let env = input
                .source
                .current(conn)
                .await
                .map_err(snapshot_read::engine_error)?
                .ok_or_else(ApiError::not_found)?;
            require_snapshot(conn, principal, env.id).await
        })
    }
    fn fetch<'a>(ctx: &'a Ctx, _: &'a Caller, input: Input) -> BoxFuture<'a, ApiResult<Fetched>> {
        Box::pin(async move {
            let measured = measure_source(ctx, &input.source, &input.origin).await?;
            Ok(Fetched {
                measured,
                source: input.source,
                origin: input.origin,
            })
        })
    }
}

/// The one native once-only measurement supplier, shared with actual snapshot creation.
pub(super) async fn measure_source(
    ctx: &Ctx,
    source: &SnapshotMeasureInput,
    origin: &CapturedOperation,
) -> ApiResult<SnapshotPreflight> {
    let command = Outgoing::new(
        orch_types::new_id(),
        source.machine(),
        source.serial(),
        &source.command(),
    )
    .map_err(ApiError::internal)?;
    let session = match ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
        crate::middleware::authentication::Via::Token { .. }
        | crate::middleware::authentication::Via::OwnerRead { .. } => None,
    };
    let authority = WorkspaceCommand::capture(origin, session, source.clone(), &command)
        .map_err(ApiError::engine)?;
    let dispatcher = ctx.app.link.dispatcher();
    dispatcher
        .issue_stored(crate::dispatch::StoredCommand {
            id: Some(command.id),
            machine: command.machine,
            kind: command.kind.as_str().to_owned(),
            key: command.key.wire(),
            deadline: command.deadline,
            body: command.body,
            authority: Some(CommandAuthority::workspace(authority)),
        })
        .await
        .map_err(|error| {
            live::command_failure(crate::dispatch::RunError::Dispatch(error))
                .problem(super::super::problems::Doing::Reading)
        })?;
    let measured = dispatcher
        .wait_for::<orch_link_contract::commands::FsMeasure>(command.id)
        .await
        .map_err(|error| {
            live::command_failure(error).problem(super::super::problems::Doing::Reading)
        })?;
    measurement_view(source.selection(), measured)
}

impl native::Selected for Measure {
    type Source = Source;
    type Query = ((), ThreadSnapshotPreflightBody);
    type Value = Arc<Fetched>;
    type Reply = SnapshotPreflight;
    const DECLARATION: &'static Declaration<Source> = &sources::THREAD_SNAPSHOT_PREFLIGHT;
    fn origin<'a>(_: &Self::Query, value: &'a Self::Value) -> Option<&'a CapturedOperation> {
        Some(&value.origin)
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: &'a Self::Query,
    ) -> BoxFuture<'a, ApiResult<(Self::Value, Vec<native::Facts<Source>>)>> {
        <Self as Effect>::prepare(ctx, conn, caller, (), query.1.clone())
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(ctx, conn, principal))
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<SnapshotPreflight>> {
        Box::pin(async move { Ok(value.measured.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Self::Query,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(old.source == fresh.source && old.origin == fresh.origin) })
    }
}
