//! Native copy runs once outside the HTTP transaction; its actual queue source owns admission.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::effects::ApplyOwned;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::native::reply;

use crate::key_operations::targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::environment::ThroughThreadQuery;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::FileCopyReceipt;

use orch_api_contract::workspace::FileCopyResponse;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::Source;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::commands::CommandAuthority;

use orch_engine::commands::FileCopyCommand;

use orch_engine::commands::FileCopyInput;

use orch_engine::commands::FileCopySource;

use orch_engine::commands::Outgoing;

use orch_engine::operation_scope::continuing;

use orch_engine::operation_scope::continuing::Source as NativeSource;

pub(super) use creation::binding;
pub(super) struct Copy;

pub(super) struct Prepared {
    env: orch_db::queries::environments::workspace::EnvironmentRow,
    input: FileCopyInput,
    facts: Vec<native::Facts<Source>>,
}

pub(super) struct Copied {
    input: FileCopyInput,
    stat: FileStat,
    facts: Vec<native::Facts<Source>>,
}

fn error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the native file copy source no longer permits the operation",
        ),
        continuing::Error::Native(error) => ApiError::engine(error),
        continuing::Error::Binding(error) => crate::key_operations::admission::binding_error(error),
    }
}

async fn facts(conn: &mut Conn, id: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id,
        },
    )
    .await?;
    let family = KeyTargetKind::Object(ObjectKind::Environment);
    Ok(vec![
        native::Facts::one(Source::CopyEnvironment, family, target.clone()),
        native::Facts::one(Source::CopyContent, family, target),
    ])
}

impl Effect for Copy {
    type Source = Source;
    type Query = ThroughThreadQuery;
    type Body = FileFromToBody;
    type Value = Prepared;
    type Output = Copied;
    fn declaration(_: &Prepared) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_COPY
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        query: ThroughThreadQuery,
        body: FileFromToBody,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let id = ctx.id("id")?;
            let facts = facts(conn, id).await?;
            let plan = native::bind_required(ctx, &sources::ENVIRONMENT_FILE_COPY, &facts)?;
            let actor = native::preparation_principal(ctx, conn, caller, &plan).await?;
            crate::middleware::authorization::require_resource(
                conn,
                &actor,
                ResourceKind::Environment,
                id,
                Permission::FilesManage,
                Policy::default(),
            )
            .await?;
            let (from, to) = two_paths(&body)?;
            let door = door::open_with_principal(ctx, conn, id, query.thread, actor).await?;
            door.may_write()?;
            let machine = live::require_live(ctx, conn, &door.env).await?;
            let input = FileCopyInput::new(&door.env, machine, query.thread, from, to);
            Ok((
                Prepared {
                    env: door.env,
                    input,
                    facts: facts.clone(),
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
            FileCopySource::require(conn, principal, &value.input, &value.env)
                .await
                .map_err(error)
        })
    }
}

fn receipt(input: &FileCopyInput) -> FileCopyResponse {
    FileCopyResponse::Receipt(FileCopyReceipt {
        projection: ReceiptProjection::Receipt,
        environment_id: input.environment(),
        path: input.destination().to_owned(),
    })
}

impl ApplyOwned for Copy {
    fn apply_owned<'a>(
        ctx: &'a Ctx,
        tx: orch_db::Tx,
        principal: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<Outcome<Copied>>> {
        Box::pin(async move {
            let body = value.input.command();
            let command = Outgoing::new(
                orch_types::new_id(),
                value.input.machine(),
                value.input.serial(),
                &body,
            )
            .map_err(ApiError::internal)?;
            let session = match ctx.user()?.via {
                crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
                crate::middleware::authentication::Via::Token { .. }
                | crate::middleware::authentication::Via::OwnerRead { .. } => None,
            };
            let authority = FileCopyCommand::capture(
                principal.caller(),
                session,
                value.input.clone(),
                &command,
            )
            .map_err(ApiError::engine)?;
            // The actual command transaction repeats the original/current source after its own
            // native waits. No authority transaction is held while the filesystem works.
            tx.commit().await.map_err(DbError::from)?;
            let dispatcher = ctx.app.link.dispatcher();
            dispatcher
                .issue_stored_before(
                    crate::dispatch::StoredCommand {
                        id: Some(command.id),
                        machine: command.machine,
                        kind: command.kind.as_str().to_owned(),
                        key: command.key.wire(),
                        deadline: command.deadline,
                        body: command.body,
                        authority: Some(CommandAuthority::file_copy(authority)),
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
                        .problem(Doing::Copying),
                })?;
            let stat = match dispatcher.wait_for::<FsCopy>(command.id).await {
                Ok(stat) => stat,
                Err(failure) => {
                    let original = live::command_failure(failure).problem(Doing::Copying);
                    let sparse =
                        crate::middleware::authorization::problem_without_content(original.clone());
                    let failed = Failed {
                        input: value.input.clone(),
                        original,
                        facts: value.facts.clone(),
                    };
                    return Err(native::refusal::project::<FailureProjection>(
                        ctx,
                        principal.caller(),
                        failed,
                        sparse,
                    )
                    .await?);
                }
            };
            // The native result proves completion. Optional readback and audit waits cannot
            // turn that completed copy into an unperformed operation or repeat its command.
            crate::key_operations::publication::receipt(ctx, &receipt(&value.input))?;
            let mut accounting = ctx.app.db.begin().await?;
            audited_on(ctx,&mut accounting,&value.env,orch_db::audit::action!(AuditAction::WorkspaceFileCopied),&said(ctx,&format!("copied {} to {}",body.from,body.to))?,serde_json::json!({"from":body.from,"to":body.to,"size":stat.entry.size,"hash":stat.hash})).await?;
            orch_db::queries::environments::thread_engine::touch_activity(
                &mut accounting,
                value.env.id,
                ctx.now,
            )
            .await?;
            accounting.commit().await.map_err(DbError::from)?;
            Ok(Outcome::Applied(Copied {
                input: value.input.clone(),
                stat,
                facts: value.facts.clone(),
            }))
        })
    }
}

type Environment = orch_db::queries::environments::workspace::EnvironmentRow;

async fn response_source(
    conn: &mut Conn,
    input: &FileCopyInput,
    known: &[native::Facts<Source>],
) -> ApiResult<(Environment, Vec<native::Facts<Source>>)> {
    let env = input
        .current_environment(conn)
        .await
        .map_err(ApiError::engine)?
        .ok_or_else(ApiError::not_found)?;
    let mut current = known
        .iter()
        .filter(|fact| fact.source != Source::CopyContent)
        .cloned()
        .collect::<Vec<_>>();
    current.extend(
        facts(conn, env.id)
            .await?
            .into_iter()
            .filter(|fact| fact.source == Source::CopyContent),
    );
    Ok((env, current))
}

pub(super) async fn response_require(
    conn: &mut Conn,
    principal: &Principal,
    env: &Environment,
) -> ApiResult<()> {
    let read = crate::middleware::authorization::require_resource(
        conn,
        principal,
        ResourceKind::Environment,
        env.id,
        Permission::FilesView,
        Policy::default(),
    )
    .await;
    match read {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            crate::middleware::authorization::require_resource(
                conn,
                principal,
                ResourceKind::Environment,
                env.id,
                Permission::FilesManage,
                Policy::default(),
            )
            .await
        }
        Err(error) => Err(error),
    }
}

struct Projection;

impl reply::Projection for Projection {
    type Source = Source;
    type Input = Copied;
    type Subject = orch_db::queries::environments::workspace::EnvironmentRow;
    type Value = ();
    type Reply = FileStat;
    const READ_SOURCE: Source = Source::CopyContent;
    fn declaration(_: &Copied) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_COPY
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Copied,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(response_source(conn, &input.input, &input.facts))
    }

    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Copied,
        env: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(response_require(conn, principal, env))
    }

    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Copied,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Copied,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<FileStat>> {
        Box::pin(async move { Ok(input.stat.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Copied,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.generation == fresh.subject.generation
                && old.subject.machine_id == fresh.subject.machine_id
                && old.subject.workspace_root == fresh.subject.workspace_root)
        })
    }
}

struct Failed {
    input: FileCopyInput,
    original: ApiError,
    facts: Vec<native::Facts<Source>>,
}

struct FailureProjection;

impl reply::Projection for FailureProjection {
    type Source = Source;
    type Input = Failed;
    type Subject = Environment;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Source = Source::CopyContent;
    fn declaration(_: &Failed) -> &'static Declaration<Source> {
        &sources::ENVIRONMENT_FILE_COPY
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Failed,
    ) -> BoxFuture<'a, ApiResult<(Environment, Vec<native::Facts<Source>>)>> {
        Box::pin(response_source(conn, &input.input, &input.facts))
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Failed,
        env: &'a Environment,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(response_require(conn, principal, env))
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
            // The public detail is catalogued at serialization. Preserve the native
            // explanation only in this already Read-admitted rich refusal projection.
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
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.subject.id == fresh.subject.id
                && old.subject.generation == fresh.subject.generation
                && old.subject.machine_id == fresh.subject.machine_id
                && old.subject.workspace_root == fresh.subject.workspace_root)
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    _: ThroughThreadQuery,
    _: FileFromToBody,
) -> ApiResult<FileCopyResponse> {
    let tx = ctx.app.db.begin().await?;
    let completed = effects::apply_owned::<Copy>(&ctx, tx).await?;
    let fallback = receipt(&completed.output.input);
    Ok(
        match reply::render::<Projection>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(stat) => FileCopyResponse::Full(Box::new(stat)),
            None => fallback,
        },
    )
}

#[path = "copy/creation.rs"]
pub mod creation;
