//! One native directory writer, accepted command identity and current optional metadata.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::middleware::idempotency::ObservedCreation;

use crate::middleware::idempotency::Registered;

use crate::middleware::idempotency::commands;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::FileMkdirReceipt;

use orch_api_contract::workspace::FileMkdirResponse;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::Source;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_db::idempotency::creation::Created;

use orch_db::queries::link;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::FileMkdirCommand;

use orch_engine::commands::FileMkdirInput;

use orch_engine::commands::FileMkdirSource;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::continuing::Source as NativeSource;

use orch_engine::operation_scope::http_command_source::CommandEvidence;
const KIND: &str = "workspace_directory";

type Environment = orch_db::queries::environments::workspace::EnvironmentRow;

pub(super) struct Mkdir;

pub(super) struct Prepared {
    env: Environment,
    input: FileMkdirInput,
}

pub(super) struct Made {
    input: FileMkdirInput,
    stat: FileStat,
}

fn source_error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the native directory source no longer permits this operation",
        ),
        continuing::Error::Native(error) => ApiError::engine(error),
        continuing::Error::Binding(error) => crate::key_operations::admission::binding_error(error),
    }
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    Ok(vec![native::Facts::one(
        Source::MkdirEnvironment,
        KeyTargetKind::Object(ObjectKind::Environment),
        targets::page_object(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: environment,
            },
        )
        .await?,
    )])
}

impl effects::Effect for Mkdir {
    type Source = Source;
    type Query = ThroughThreadQuery;
    type Body = FilePathBody;
    type Value = Prepared;
    type Output = Made;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_MKDIR
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: ThroughThreadQuery,
        body: FilePathBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let facts = facts(conn, environment).await?;
            let plan = native::bind_required(ctx, &sources::ENVIRONMENT_FILE_MKDIR, &facts)?;
            let principal = native::preparation_principal(ctx, conn, caller, &plan).await?;
            crate::middleware::authorization::require_resource(
                conn,
                &principal,
                ResourceKind::Environment,
                environment,
                Permission::FilesManage,
                Policy::default(),
            )
            .await?;
            let door =
                door::open_with_principal(ctx, conn, environment, query.thread, principal).await?;
            door.may_write()?;
            let machine = live::require_live(ctx, conn, &door.env).await?;
            let path = paths::clean("path", &body.path, false)?;
            let input = FileMkdirInput::new(&door.env, machine, query.thread, path);
            Ok((
                Prepared {
                    env: door.env,
                    input,
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
            FileMkdirSource::require(conn, principal, &value.input, &value.env)
                .await
                .map_err(source_error)
        })
    }
}

impl effects::ApplyOwned for Mkdir {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        mut tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Made>>> {
        Box::pin(async move {
            let command = Outgoing::new(
                orch_types::new_id(),
                value.input.machine(),
                value.input.serial(),
                &value.input.command(),
            )
            .map_err(ApiError::internal)?;
            let origin = orch_authz::CapturedOperation::capture(principal.caller())
                .map_err(ApiError::internal)?;
            let http = native::http_origin::capture_operation(ctx, &mut tx, &origin).await?;
            let source =
                FileMkdirCommand::capture(principal.caller(), value.input.clone(), &command)
                    .map_err(ApiError::engine)?;
            let authority = native::http_origin::bind_source(
                &http,
                &command,
                CommandEvidence::FileMkdir(source),
            )?;
            tx.rollback().await.map_err(DbError::from)?;
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
                    Box::new(move || {
                        before.check().map_err(|error| {
                            Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                        })
                    }),
                )
                .await
                .map_err(|error| match error {
                    crate::dispatch::DispatchError::BeforeEffect(error) => {
                        ApiError::engine(orch_engine::EngineError::BeforeEffect(error))
                    }
                    other => live::command_failure(crate::dispatch::RunError::Dispatch(other))
                        .problem(Doing::Making),
                })?;
            let stat = match dispatcher.wait_for::<FsMkdir>(command.id).await {
                Ok(stat) => stat,
                Err(failure) => {
                    let original = live::command_failure(failure).problem(Doing::Making);
                    let sparse =
                        crate::middleware::authorization::problem_without_content(original.clone());
                    return Err(native::refusal::project::<FailureProjection>(
                        ctx,
                        principal.caller(),
                        Failed {
                            input: value.input.clone(),
                            original,
                        },
                        sparse,
                    )
                    .await?);
                }
            };
            Ok(effects::Outcome::Applied(Made {
                input: value.input.clone(),
                stat,
            }))
        })
    }
}

fn receipt(environment: Id, path: &str) -> FileMkdirResponse {
    FileMkdirResponse::Receipt(FileMkdirReceipt {
        projection: ReceiptProjection::Receipt,
        environment_id: environment,
        path: path.into(),
    })
}

async fn subject(
    conn: &mut Conn,
    input: &FileMkdirInput,
) -> ApiResult<(Environment, Vec<native::Facts<Source>>)> {
    let env = input
        .current_environment(conn)
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ApiError::not_found)?;
    let facts = facts(conn, env.id).await?;
    Ok((env, facts))
}

fn preserved(old: &Environment, current: &Environment) -> bool {
    old.id == current.id
        && old.generation == current.generation
        && old.machine_id == current.machine_id
        && old.workspace_root == current.workspace_root
}

struct Metadata;

impl reply::Projection for Metadata {
    type Source = Source;
    type Input = Made;
    type Subject = Environment;
    type Value = ();
    type Reply = FileStat;
    const READ_SOURCE: Source = Source::MkdirEnvironment;
    fn declaration(_: &Made) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_MKDIR
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Made,
    ) -> BoxFuture<'a, ApiResult<(Environment, Vec<native::Facts<Source>>)>> {
        Box::pin(subject(conn, &input.input))
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Made,
        env: &'a Environment,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::copy::response_require(conn, principal, env))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Made,
        _: &'a Environment,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Made,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<FileStat>> {
        Box::pin(async move { Ok(input.stat.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Made,
        old: &'a reply::Prepared<Self>,
        current: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(preserved(&old.subject, &current.subject)) })
    }
}

struct Failed {
    input: FileMkdirInput,
    original: ApiError,
}

struct FailureProjection;

impl reply::Projection for FailureProjection {
    type Source = Source;
    type Input = Failed;
    type Subject = Environment;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::MkdirEnvironment;
    fn declaration(_: &Failed) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_MKDIR
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Failed,
    ) -> BoxFuture<'a, ApiResult<(Environment, Vec<native::Facts<Source>>)>> {
        Box::pin(subject(conn, &input.input))
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Failed,
        env: &'a Environment,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(super::copy::response_require(conn, principal, env))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Failed,
        _: &'a Environment,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Failed,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move {
            Ok(input
                .original
                .clone()
                .with("diagnostic", input.original.problem.detail.clone()))
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Failed,
        old: &'a reply::Prepared<Self>,
        current: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move { Ok(preserved(&old.subject, &current.subject)) })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    _: ThroughThreadQuery,
    _: FilePathBody,
) -> ApiResult<FileMkdirResponse> {
    let tx = ctx.app.db.begin().await?;
    let done = effects::apply_owned::<Mkdir>(&ctx, tx).await?;
    let fallback = receipt(done.output.input.environment(), done.output.input.path());
    publication::receipt(&done.ctx, &fallback)?;
    Ok(
        match reply::render::<Metadata>(&done.ctx, &done.accepted, done.output).await? {
            Some(stat) => FileMkdirResponse::full(stat),
            None => fallback,
        },
    )
}

pub(super) fn binding() -> Registered<ThroughThreadQuery, FilePathBody, FileMkdirResponse> {
    Registered::from(ObservedCreation {
        kind: KIND,
        replay: |ctx, query: ThroughThreadQuery, body: FilePathBody, created| {
            Box::pin(async move {
                let environment = ctx.id("id")?;
                let path = paths::clean("path", &body.path, false)?;
                if created.kind != KIND
                    || created.data["environment_id"] != serde_json::json!(environment)
                    || created.data["path"] != serde_json::json!(path)
                {
                    return Err(ApiError::internal(
                        "the accepted directory association changed",
                    ));
                }
                let fallback = receipt(environment, &path);
                publication::receipt(&ctx, &fallback)?;
                let Some(made) =
                    completed(&ctx, &created, environment, query.thread, &path).await?
                else {
                    return Ok(fallback);
                };
                match reply::render_completed_replay::<Metadata>(&ctx, ctx.user()?.id(), made).await
                {
                    Ok(Some(stat)) => Ok(FileMkdirResponse::full(stat)),
                    Ok(None) => Ok(fallback),
                    Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => {
                        Ok(fallback)
                    }
                    Err(error) => Err(error),
                }
            })
        },
    })
    .command(commands::CreationCommand {
        kind: CommandKind::FsMkdir,
        identity: |row| {
            let input: FsMkdir =
                serde_json::from_value(row.body.clone()).map_err(ApiError::internal)?;
            Ok(Some(Created {
                kind: KIND.into(),
                id: row.id,
                data: serde_json::json!({"environment_id":input.environment_id,"path":input.path}),
            }))
        },
    })
}

async fn completed(
    ctx: &Ctx,
    created: &Created,
    environment: Id,
    thread: Option<Id>,
    path: &str,
) -> ApiResult<Option<Made>> {
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
        .file_mkdir_receipt(&row)
        .map_err(ApiError::engine)?
    else {
        return Ok(None);
    };
    if input.environment() != environment
        || input.through_thread() != thread
        || input.path() != path
    {
        return Ok(None);
    }
    let Some(result) = row.result else {
        return Ok(None);
    };
    let stat: FileStat = serde_json::from_value(result).map_err(ApiError::internal)?;
    Ok(Some(Made { input, stat }))
}
