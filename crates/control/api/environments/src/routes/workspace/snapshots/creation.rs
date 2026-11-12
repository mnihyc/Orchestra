//! The native measured Snapshot creator and whole-workspace exporter share one effect/source path.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::effects::Apply;

use crate::key_operations::native::effects::Effect;

use crate::key_operations::native::effects::Initial;

use crate::key_operations::native::effects::Outcome;

use crate::key_operations::native::reply;

use crate::key_operations::targets;

use crate::middleware::transaction_guard;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::workspace::SnapshotStartReceipt;

use orch_api_contract::workspace::SnapshotStartResponse;

use orch_api_contract::workspace_sources as sources;

use orch_api_contract::workspace_sources::Source;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::commands::SnapshotCreateSource;

use orch_engine::commands::SnapshotMeasureInput;

use orch_engine::commands::SnapshotRequest;

use orch_engine::operation_scope::continuing::Source as NativeSource;

pub(super) use replay::binding;
pub(super) struct Create;

pub(super) struct Export;

pub(super) struct InitialInput {
    source: SnapshotMeasureInput,
    origin: CapturedOperation,
    label: String,
}

pub(super) struct Measured {
    source: SnapshotMeasureInput,
    label: String,
    measurement: SnapshotPreflight,
}

pub(super) struct Prepared {
    env: environment_rows::EnvironmentRow,
    asked: Asked,
    measurement: Option<SnapshotPreflight>,
    facts: Vec<native::Facts<Source>>,
}

pub(super) struct Started {
    begun: orch_engine::jobs::snapshot::Begun,
    request: SnapshotRequest,
    facts: Vec<native::Facts<Source>>,
}

fn declaration(thread: bool) -> &'static Declaration<Source> {
    if thread {
        &sources::THREAD_SNAPSHOT_CREATE
    } else {
        &sources::ENVIRONMENT_FILES_EXPORT
    }
}

fn session(ctx: &Ctx) -> ApiResult<Option<Id>> {
    Ok(match ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
        crate::middleware::authentication::Via::Token { .. }
        | crate::middleware::authentication::Via::OwnerRead { .. } => None,
    })
}

async fn facts(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Source>>> {
    let target = targets::page_object(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await?;
    let family = KeyTargetKind::Object(ObjectKind::Environment);
    Ok(vec![
        native::Facts::one(Source::SnapshotEnvironment, family, target.clone()),
        native::Facts::one(Source::SnapshotContent, family, target),
    ])
}

async fn require(conn: &mut Conn, principal: &Principal, prepared: &Prepared) -> ApiResult<()> {
    if prepared.asked.thread.is_some() {
        require_snapshot(conn, principal, prepared.env.id).await?;
    } else {
        authorization::require_resource(
            conn,
            principal,
            ResourceKind::Environment,
            prepared.env.id,
            Permission::EnvironmentManage,
            Policy::default(),
        )
        .await?;
    }
    if let Some(measured) = prepared
        .measurement
        .as_ref()
        .filter(|measured| !measured.allowed)
    {
        return Err(ApiError::new(
            ErrorCode::WorkspaceNotIndexed,
            measured
                .reason
                .as_deref()
                .unwrap_or("the selection could not be measured"),
        )
        .with("preflight", measured));
    }
    Ok(())
}

async fn prepared_create(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let measured = effects::initial_value::<Create>(ctx)?;
    let (env, known) =
        SnapshotCreateSource::prepare(conn, caller, &measured.source, ctx.app.clock.time())
            .await
            .map_err(preflight::error)?;
    let plan = native::bind_required(ctx, &sources::THREAD_SNAPSHOT_CREATE, &known)?;
    let actor = native::preparation_principal(ctx, conn, caller, &plan).await?;
    require_snapshot(conn, &actor, env.id).await?;
    let thread = ctx.id("id")?;
    orch_db::queries::threads::lock(conn, thread)
        .await?
        .ok_or_else(ApiError::not_found)?;
    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
        .bind(thread)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
        .bind(env.id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let (env, _) =
        SnapshotCreateSource::prepare(conn, caller, &measured.source, ctx.app.clock.time())
            .await
            .map_err(preflight::error)?;
    let facts = facts(conn, env.id).await?;
    let asked = Asked {
        thread: Some(thread),
        environment: env.id,
        generation: env.generation,
        label: measured.label.clone(),
        include: measured.source.selection().include.clone(),
        exclude: measured.source.selection().exclude.clone(),
        created_by: Some(caller.user()),
    };
    Ok((
        Prepared {
            env,
            asked,
            measurement: Some(measured.measurement.clone()),
            facts: facts.clone(),
        },
        facts,
    ))
}

async fn prepared_export(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
) -> ApiResult<(Prepared, Vec<native::Facts<Source>>)> {
    let id = ctx.id("id")?;
    let env = environment_rows::environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let known = facts(conn, env.id).await?;
    let plan = native::bind_required(ctx, &sources::ENVIRONMENT_FILES_EXPORT, &known)?;
    let actor = native::preparation_principal(ctx, conn, caller, &plan).await?;
    authorization::require_resource(
        conn,
        &actor,
        ResourceKind::Environment,
        id,
        Permission::EnvironmentManage,
        Policy::default(),
    )
    .await?;
    if !orch_db::queries::environments::temporary::try_lock(conn, id).await? {
        return Err(ApiError::new(
            ErrorCode::PlanBlocked,
            "The environment is changing or being removed. Try Export again when it finishes.",
        ));
    }
    let env = environment_rows::environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let facts = facts(conn, env.id).await?;
    let asked = Asked {
        thread: None,
        environment: id,
        generation: env.generation,
        label: env.name.clone(),
        include: vec!["**".to_owned()],
        exclude: Vec::new(),
        created_by: Some(caller.user()),
    };
    Ok((
        Prepared {
            env,
            asked,
            measurement: None,
            facts: facts.clone(),
        },
        facts,
    ))
}

macro_rules! effect {
    ($type:ty,$body:ty,$row:expr,$prepare:ident) => {
        impl Effect for $type {
            type Source = Source;
            type Query = ();
            type Body = $body;
            type Value = Prepared;
            type Output = Started;
            fn declaration(_: &Prepared) -> &'static Declaration<Source> {
                $row
            }
            fn prepare<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                caller: &'a Caller,
                _: (),
                _: Self::Body,
            ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Source>>)>> {
                Box::pin($prepare(ctx, conn, caller))
            }
            fn require<'a>(
                _: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
            ) -> BoxFuture<'a, ApiResult<()>> {
                Box::pin(require(conn, principal, value))
            }
        }
        impl Apply for $type {
            fn apply<'a>(
                ctx: &'a Ctx,
                conn: &'a mut Conn,
                principal: &'a Principal,
                value: &'a Prepared,
                before: effects::FirstEffect<'a>,
            ) -> BoxFuture<'a, ApiResult<Outcome<Started>>> {
                Box::pin(apply(ctx, conn, principal, value, before))
            }
        }
    };
}

effect!(
    Create,
    ThreadSnapshotCreateBody,
    &sources::THREAD_SNAPSHOT_CREATE,
    prepared_create
);

effect!(
    Export,
    (),
    &sources::ENVIRONMENT_FILES_EXPORT,
    prepared_export
);

impl Initial for Create {
    type Input = InitialInput;
    type Fetched = Measured;
    fn declaration(_: &InitialInput) -> &'static Declaration<Source> {
        &sources::THREAD_SNAPSHOT_CREATE
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        _: (),
        body: ThreadSnapshotCreateBody,
    ) -> BoxFuture<'a, ApiResult<(InitialInput, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let (env, directory) = snapshot_environment(ctx, conn).await?;
            let facts = facts(conn, env.id).await?;
            let plan = native::bind_required(ctx, &sources::THREAD_SNAPSHOT_CREATE, &facts)?;
            let admitted = plan
                .shared()
                .admit(caller, false)
                .map_err(crate::key_operations::admission::binding_error)?;
            let actor = native::preparation_principal(ctx, conn, &admitted, &plan).await?;
            require_snapshot(conn, &actor, env.id).await?;
            let label = label_of(ctx, body.label.as_deref())?;
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
            Ok((
                InitialInput {
                    source,
                    origin,
                    label,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a InitialInput,
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
    fn fetch<'a>(
        ctx: &'a Ctx,
        _: &'a Caller,
        input: InitialInput,
    ) -> BoxFuture<'a, ApiResult<Measured>> {
        Box::pin(async move {
            let measurement = preflight::measure_source(ctx, &input.source, &input.origin).await?;
            Ok(Measured {
                source: input.source,
                label: input.label,
                measurement,
            })
        })
    }
}

async fn apply(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    value: &Prepared,
    before: effects::FirstEffect<'_>,
) -> ApiResult<Outcome<Started>> {
    let request =
        SnapshotRequest::capture(principal.caller(), session(ctx)?, &value.env, &value.asked)
            .map_err(ApiError::engine)?;
    let begun = ctx
        .app
        .engine
        .begin_snapshot_before(conn, &value.asked, Some(&request), move || {
            before
                .check()
                .map_err(|error| Box::new(error) as Box<dyn std::error::Error + Send + Sync>)
        })
        .await
        .map_err(ApiError::engine)?;
    let thread = value.asked.thread;
    if thread.is_none() {
        orch_engine::temporary_environments::export_activity(conn, value.env.id, ctx.now)
            .await
            .map_err(ApiError::engine)?;
    }
    let action = orch_db::audit::action!(AuditAction::WorkspaceSnapshotCreated);
    let summary = if thread.is_some() {
        format!(
            "{} began a snapshot of {}",
            crate::routes::nodes::actor_name(ctx)?,
            value.asked.include.join(", ")
        )
    } else {
        format!("Workspace export of {} was requested", value.env.name)
    };
    let mut entry = AuditEntry::new(
        Actor::User(principal.user()),
        action.as_str(),
        "snapshot",
        Some(begun.snapshot),
        &summary,
    );
    entry.history = thread.is_some() && AuditAction::WorkspaceSnapshotCreated.is_history_line();
    entry.node_id = thread.or(Some(value.env.home_node_id));
    entry.org_id = Some(value.env.org_id);
    entry.environment_id = Some(value.env.id);
    entry.details = if thread.is_some() {
        serde_json::json!({"snapshot":begun.snapshot,"label":value.asked.label,"include":value.asked.include})
    } else {
        serde_json::json!({"snapshot":begun.snapshot,"environment":value.env.id,"whole_workspace":true})
    };
    audit::append(conn, &entry, &origin(ctx), ctx.now).await?;
    Ok(Outcome::Applied(Started {
        begun,
        request,
        facts: value.facts.clone(),
    }))
}

struct Projection;

impl reply::Projection for Projection {
    type Source = Source;
    type Input = Started;
    type Subject = environment_rows::EnvironmentRow;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Source = Source::SnapshotContent;
    fn declaration(input: &Started) -> &'static Declaration<Source> {
        declaration(input.begun.job.node_id.is_some())
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Started,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let env = input
                .request
                .current(conn)
                .await
                .map_err(snapshot_read::engine_error)?
                .ok_or_else(ApiError::not_found)?;
            let mut facts = input
                .facts
                .iter()
                .filter(|fact| fact.source != Source::SnapshotContent)
                .cloned()
                .collect::<Vec<_>>();
            facts.extend(
                super::creation::facts(conn, env.id)
                    .await?
                    .into_iter()
                    .filter(|fact| fact.source == Source::SnapshotContent),
            );
            Ok((env, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Started,
        env: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            let read = authorization::require_resource(
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
                    let permission = if input.begun.job.node_id.is_some() {
                        Permission::FilesManage
                    } else {
                        Permission::EnvironmentManage
                    };
                    authorization::require_resource(
                        conn,
                        principal,
                        ResourceKind::Environment,
                        env.id,
                        permission,
                        Policy::default(),
                    )
                    .await
                }
                Err(error) => Err(error),
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Started,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Source>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Started,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(
            async move { crate::routes::environments::jobs::view(conn, &input.begun.job).await },
        )
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Started,
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

async fn finish<F: Apply<Output = Started>>(ctx: Ctx) -> ApiResult<SnapshotStartResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<F>(&ctx, &mut tx).await?;
    // The once-only measurement/writer is not replayed after an awaited native audit write.
    let (current, guard) = transaction_guard::after_wait(&completed.ctx, &mut tx).await?;
    guard.check_deadline(&current)?;
    let accepted = guard.principal().caller().clone();
    tx.commit().await.map_err(DbError::from)?;
    let fallback = SnapshotStartResponse::Receipt(SnapshotStartReceipt {
        projection: ReceiptProjection::Receipt,
        job_id: completed.output.begun.job.id,
        snapshot_id: completed.output.begun.snapshot,
    });
    crate::key_operations::publication::receipt(&current, &fallback)?;
    current.app.engine.kick();
    Ok(
        match reply::render::<Projection>(&current, &accepted, completed.output).await? {
            Some(job) => SnapshotStartResponse::Full(Box::new(job)),
            None => fallback,
        },
    )
}

pub(super) async fn create(
    ctx: Ctx,
    _: (),
    _: ThreadSnapshotCreateBody,
) -> ApiResult<SnapshotStartResponse> {
    finish::<Create>(ctx).await
}

pub(super) async fn export(ctx: Ctx, _: (), _: ()) -> ApiResult<SnapshotStartResponse> {
    finish::<Export>(ctx).await
}

#[path = "creation/replay.rs"]
pub mod replay;
