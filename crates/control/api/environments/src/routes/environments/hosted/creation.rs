//! Standalone native creation, its real sources and the shared singleton readback.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::resource_reply;

use crate::key_operations::native::resource_reply::ResourceProjection;

use crate::key_operations::publication;

use crate::key_operations::targets;

use crate::routes::access::operations::NameRoles;

use crate::routes::resources::projection::single;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::environment_sources::CREATE;

use orch_api_contract::environment_sources::CreateSource as Source;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_db::queries::environments::routes::EnvironmentFull;
pub(super) struct Create;

pub(super) struct Ready {
    input: CreationInput,
    native: orch_engine::environments::PreparedCreation,
}

pub(super) struct Created {
    environment: Id,
    job: Id,
    revision: i64,
    owner: Id,
    home: Id,
    template: Id,
    host: Id,
    mounts: Vec<Id>,
    initial: Option<(Option<Id>, Id)>,
}

pub(super) enum ReplyInput {
    First(Created),
    Replay(Id),
}

impl ReplyInput {
    fn environment(&self) -> Id {
        match self {
            Self::First(created) => created.environment,
            Self::Replay(id) => *id,
        }
    }
}

pub(super) fn sparse(caller: &Caller, error: ApiError) -> ApiError {
    if native::modern(caller) {
        authorization::problem_without_content(error)
    } else {
        error
    }
}

pub(super) async fn prefix(
    conn: &mut Conn,
    home: Id,
    initial: Option<(Option<Id>, Id)>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let (threads, environments) = if let Some((thread, environment)) = initial {
        (
            match thread {
                Some(thread) => vec![(
                    KeyTargetKind::Object(ObjectKind::Thread),
                    vec![targets::node_identity(conn, thread).await?],
                )],
                None => Vec::new(),
            },
            vec![(
                KeyTargetKind::Object(ObjectKind::Environment),
                vec![
                    targets::resource_identity(
                        conn,
                        ObjectRef {
                            kind: ObjectKind::Environment,
                            id: environment,
                        },
                    )
                    .await?,
                ],
            )],
        )
    } else {
        (Vec::new(), Vec::new())
    };
    Ok(vec![
        native::Facts::one(
            Source::Home,
            KeyTargetKind::Object(ObjectKind::Environment),
            targets::node_identity(conn, home).await?,
        ),
        native::Facts::collection(Source::InitialThreads, threads),
        native::Facts::collection(Source::InitialEnvironments, environments),
    ])
}

pub(super) async fn complete(
    conn: &mut Conn,
    home: Id,
    template: Id,
    host: Id,
    mounts: &[Id],
    initial: Option<(Option<Id>, Id)>,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = prefix(conn, home, initial).await?;
    for (source, kind, id) in [
        (Source::Template, ObjectKind::Template, template),
        (Source::Host, ObjectKind::Host, host),
    ] {
        facts.push(native::Facts::one(
            source,
            KeyTargetKind::Object(kind),
            targets::resource_identity(conn, ObjectRef { kind, id }).await?,
        ));
    }
    let mut actual = Vec::new();
    for id in mounts {
        actual.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Mount,
                    id: *id,
                },
            )
            .await?,
        );
    }
    facts.push(native::Facts::collection(
        Source::Mounts,
        if actual.is_empty() {
            Vec::new()
        } else {
            vec![(KeyTargetKind::Object(ObjectKind::Mount), actual)]
        },
    ));
    Ok(facts)
}

impl effects::Effect for Create {
    type Source = Source;
    type Query = ();
    type Body = NewEnvironment;
    type Value = Ready;
    type Output = Created;
    fn declaration(_: &Ready) -> &'static Declaration<Source> {
        &CREATE
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: NewEnvironment,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let neutral = Principal::load(conn, caller.clone(), Time::new(ctx.app.clock.now()))
                .await
                .map_err(ApiError::internal)?;
            let home = match body.home_node_id {
                Some(home) => home,
                None => own_domain(&neutral)?,
            };
            let source = match &body.files {
                Some(files) => Some(
                    orch_engine::initial_files::source_identity(conn, files)
                        .await
                        .map_err(initial_failed)?,
                ),
                None => None,
            };
            let known = native::bind_known(ctx, &CREATE, &prefix(conn, home, source).await?)?;
            let principal = known.preparation_principal(ctx, conn, caller).await?;
            let input = prepare_input(ctx, conn, &principal, body, None)
                .await
                .map_err(|error| sparse(caller, error))?;
            let source = input
                .initial
                .as_ref()
                .map(|initial| (initial.source_thread(), initial.source_environment()));
            let known = native::bind_known(ctx, &CREATE, &prefix(conn, input.home, source).await?)?;
            let native = match orch_engine::environments::prepare_creation_bounded(
                conn,
                ctx.app.clock.now(),
                &principal,
                &input.request(&principal),
                Some(known.shared()),
            )
            .await
            {
                Ok(native) => native,
                Err(error) => return Err(sparse(caller, creation_failed(conn, error).await?)),
            };
            let facts = complete(
                conn,
                native.home(),
                native.template(),
                native.host(),
                native.mounts(),
                source,
            )
            .await?;
            Ok((Ready { input, native }, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            authorization::require(
                conn,
                principal,
                value.native.home(),
                PermissionName::OfNode(NodePermission::Interact),
                Act::Use,
                Policy::default(),
            )
            .await
            .map(|_| ())
        })
    }
}

impl effects::Apply for Create {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Created>>> {
        Box::pin(async move {
            let deadline = before.deadline();
            let made = match orch_engine::environments::apply_creation(
                conn,
                ctx.app.clock.now(),
                &value.native,
                move || {
                    before
                        .check()
                        .map_err(|error| orch_engine::EngineError::BeforeEffect(Box::new(error)))
                },
            )
            .await
            {
                Ok(made) => made,
                Err(error) => {
                    return Err(sparse(
                        principal.caller(),
                        creation_failed(conn, error).await?,
                    ));
                }
            };
            finish_input(ctx, conn, principal, &value.input, &made).await?;
            let origin =
                CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
            orch_engine::environment_creation::retain(
                conn,
                &origin,
                &made,
                value.input.home,
                principal.now(),
            )
            .await
            .map_err(ApiError::engine)?;
            if let Some(initial) = &value.input.initial {
                let mut initial = initial.clone();
                initial
                    .capture_environment_creator(principal.caller(), made.job.id, made.id, 0)
                    .map_err(ApiError::engine)?;
                orch_db::queries::jobs::merge_plan(
                    conn,
                    made.job.id,
                    &json!({"initial_files":initial}),
                    ctx.app.clock.now(),
                )
                .await?;
            }
            let row = queries::environments(conn, &[made.id])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            orch_db::idempotency::creation::observe_with(
                conn,
                Some(principal.user()),
                CREATION,
                made.id,
                json!({"job_id":made.job.id,"revision":row.revision}),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Created {
                environment: made.id,
                job: made.job.id,
                revision: row.revision,
                owner: row.owner_id,
                home: row.home_node_id,
                template: value.native.template(),
                host: value.native.host(),
                mounts: value.native.mounts().to_vec(),
                initial: value
                    .input
                    .initial
                    .as_ref()
                    .map(|initial| (initial.source_thread(), initial.source_environment())),
            }))
        })
    }
}

const CREATION: &str = "environment";

pub(super) fn binding()
-> crate::middleware::idempotency::Creation<(), NewEnvironment, api::EnvironmentCreateResponse> {
    crate::middleware::idempotency::Creation {
        kind: CREATION,
        identity: |value| {
            let id = match value {
                api::EnvironmentCreateResponse::Full(value) => value.base.id,
                api::EnvironmentCreateResponse::Receipt(value) => value.environment_id,
            };
            // The atomic observer retains the richer accepted job/revision tuple.
            Ok(orch_db::idempotency::creation::Created {
                kind: CREATION.into(),
                id,
                data: serde_json::Value::Null,
            })
        },
        replay: |ctx, (), _, created| {
            Box::pin(async move {
                if created.kind != CREATION {
                    return Err(ApiError::internal("the accepted environment kind changed"));
                }
                let receipt = api::EnvironmentCreateReceipt {
                    projection: orch_api_contract::common::ReceiptProjection::Receipt,
                    environment_id: created.id,
                    job_id: serde_json::from_value(created.data["job_id"].clone())
                        .map_err(ApiError::internal)?,
                    revision: serde_json::from_value(created.data["revision"].clone())
                        .map_err(ApiError::internal)?,
                };
                crate::key_operations::publication::receipt(&ctx, &receipt)?;
                let account = ctx.authenticated_user()?.id();
                let full = crate::key_operations::completed_projection::run(
                    &ctx,
                    resource_reply::with_input_completed_replay::<Create>(
                        &ctx,
                        account,
                        ReplyInput::Replay(created.id),
                    ),
                )
                .await;
                match full {
                    Ok(Some(orch_api_contract::resources::Resource::Environment(value))) => {
                        Ok(api::EnvironmentCreateResponse::Full(Box::new(value)))
                    }
                    Ok(None) => Ok(api::EnvironmentCreateResponse::Receipt(receipt)),
                    Ok(Some(_)) => Err(ApiError::internal(
                        "the native creator returned another resource kind",
                    )),
                    Err(error)
                        if matches!(error.code(), ErrorCode::NotFound | ErrorCode::AuthzDenied) =>
                    {
                        Ok(api::EnvironmentCreateResponse::Receipt(receipt))
                    }
                    Err(error) => Err(error),
                }
            })
        },
    }
}

pub(super) async fn create(
    ctx: Ctx,
    (): (),
    _: NewEnvironment,
) -> ApiResult<api::EnvironmentCreateResponse> {
    let (mut tx, _) = crate::middleware::transaction_guard::begin(&ctx).await?;
    let done = effects::apply::<Create>(&ctx, &mut tx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let receipt = api::EnvironmentCreateReceipt {
        projection: orch_api_contract::common::ReceiptProjection::Receipt,
        environment_id: done.output.environment,
        job_id: done.output.job,
        revision: done.output.revision,
    };
    publication::receipt(&done.ctx, &receipt)?;
    ctx.app.engine.kick();
    let full = match resource_reply::with_input::<Create>(
        &done.ctx,
        &done.accepted,
        ReplyInput::First(done.output),
    )
    .await
    {
        Ok(full) => full,
        Err(error) if matches!(error.code(), ErrorCode::AuthzDenied | ErrorCode::NotFound) => None,
        Err(error) => return Err(error),
    };
    match full {
        Some(orch_api_contract::resources::Resource::Environment(value)) => {
            Ok(api::EnvironmentCreateResponse::Full(Box::new(value)))
        }
        Some(_) => Err(ApiError::internal(
            "the native creator returned another resource kind",
        )),
        None => Ok(api::EnvironmentCreateResponse::Receipt(receipt)),
    }
}

impl ResourceProjection for Create {
    type Source = Source;
    type Value = EnvironmentFull;
    type Input = ReplyInput;
    const READ_SOURCE: Source = Source::Environment;
    const ROLES: &'static single::Roles = &single::Roles {
        sources: "environment_created_sources",
        names: NameRoles {
            groups: Some("environment_created_groups"),
            nodes: [
                (ObjectKind::Org, "environment_created_orgs"),
                (ObjectKind::Project, "environment_created_projects"),
                (ObjectKind::Topic, "environment_created_topics"),
                (ObjectKind::Thread, "environment_created_threads"),
            ],
            containers: None,
        },
    };
    fn declaration(_: &Ctx, _: &ReplyInput) -> ApiResult<&'static Declaration<Source>> {
        Ok(&CREATE)
    }
    fn load<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a ReplyInput,
    ) -> BoxFuture<'a, ApiResult<(EnvironmentFull, Vec<native::Facts<Source>>)>> {
        Box::pin(async move {
            let row = queries::environments(conn, &[input.environment()])
                .await?
                .pop()
                .ok_or_else(ApiError::not_found)?;
            let mut facts = if let ReplyInput::First(input) = input {
                if row.owner_id != input.owner
                    || row.home_node_id != input.home
                    || row.template_id != Some(input.template)
                    || row.placement != "managed"
                {
                    return Err(ApiError::not_found());
                }
                complete(
                    conn,
                    input.home,
                    input.template,
                    input.host,
                    &input.mounts,
                    input.initial,
                )
                .await?
            } else {
                Vec::new()
            };
            facts.push(native::Facts::one(
                Source::Environment,
                KeyTargetKind::Object(ObjectKind::Environment),
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: row.id,
                    },
                )
                .await?,
            ));
            Ok((row, facts))
        })
    }
    fn object(row: &EnvironmentFull) -> ObjectRef {
        ObjectRef {
            kind: ObjectKind::Environment,
            id: row.id,
        }
    }
    fn require_read<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        row: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            super::super::reading::decide(conn, principal, row.id)
                .await
                .map(|_| ())
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        opened: &'a EnvironmentFull,
        fresh: &'a EnvironmentFull,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(opened.id == fresh.id
                && opened.owner_id == fresh.owner_id
                && opened.home_node_id == fresh.home_node_id
                && opened.template_id == fresh.template_id)
        })
    }
}
