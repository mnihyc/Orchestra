//! Workspace duplication uses the native creator and its original durable provision authority.

use super::super::machine_create;

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::targets;

use crate::routes::duplicates;

use crate::routes::duplicates::Configuration;

use futures::future::BoxFuture;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_api_contract::binding::NewEnvironmentFiles;

use orch_api_contract::duplicates::DuplicateBody;

use orch_api_contract::duplicates::DuplicateKind;

use orch_api_contract::duplicates::EnvironmentDuplicateBody;

use orch_api_contract::duplicates::EnvironmentMoveOut;

use orch_api_contract::duplicates::EnvironmentMoveOutBody;

use orch_api_contract::duplicates::EnvironmentMoveOutPreview;

use orch_api_contract::duplicates::MoveOutDeleteEffect;

use orch_api_contract::duplicates::MoveOutDeletePlan;

use orch_api_contract::environment_sources::CreateSource as Source;

use orch_api_contract::environment_sources::duplicating as declarations;

use orch_api_contract::environment_sources::duplicating::CONNECTED;

use orch_api_contract::environment_sources::duplicating::MANAGED;

use orch_api_contract::operation_sources::Declaration;

use orch_authz::Caller;

use orch_authz::CapturedOperation;

use orch_db::queries::environments::routes::EnvironmentFull;

use std::collections::BTreeMap;

use std::collections::BTreeSet;
pub(super) struct Environments;

struct Moving;

struct Preview;

impl Configuration for Moving {
    const KIND: DuplicateKind = DuplicateKind::Environment;
    const REPLAY: &'static str = "environment_move_out";
    const MOVING: bool = true;
}

pub(super) fn mount_moves(registry: &mut Registry) {
    duplicates::register::<Moving, _>(registry, EnvironmentMoveOut);
    registry.on_snapshot_body_read_with_optional_operation(
        EnvironmentMoveOutPreview,
        preview_prepare,
        preview,
    );
}

impl Configuration for Environments {
    const KIND: DuplicateKind = DuplicateKind::Environment;
    const REPLAY: &'static str = "environment_duplicate";
}

pub(super) struct Ready {
    source: EnvironmentFull,
    review: Option<orch_engine::environments::ending::Review>,
    delete: Option<MoveOutDeletePlan>,
    preview: bool,
    home: Id,
    config: serde_json::Value,
    placement: Placement,
    tags: Vec<Id>,
    secrets: BTreeMap<String, String>,
}

enum Placement {
    Managed {
        input: Box<CreationInput>,
        native: Box<orch_engine::environments::PreparedCreation>,
    },
    Connected {
        registration: Box<machine_create::Prepared>,
        files: Box<orch_engine::initial_files::Plan>,
    },
}

impl Ready {
    fn files(&self) -> ApiResult<&orch_engine::initial_files::Plan> {
        match &self.placement {
            Placement::Managed { input, .. } => input
                .initial
                .as_ref()
                .ok_or_else(|| ApiError::internal("the copy lost its workspace source")),
            Placement::Connected { files, .. } => Ok(&**files),
        }
    }
}

async fn connected_facts(
    conn: &mut Conn,
    home: Id,
    source: Id,
    machine: Id,
    config: &serde_json::Value,
) -> ApiResult<Vec<native::Facts<Source>>> {
    let mut facts = creation::prefix(conn, home, Some((None, source))).await?;
    facts.push(native::Facts::one(
        Source::Machine,
        KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Machine),
        targets::machine_subject(conn, machine).await?,
    ));
    let mounts = orch_engine::places::mount_ids_for_new(conn, home, config)
        .await
        .map_err(ApiError::engine)?;
    let mut actual = Vec::new();
    for id in mounts {
        actual.push(
            targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ResourceKind::Mount.into(),
                    id,
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
            vec![(KeyTargetKind::Object(ResourceKind::Mount.into()), actual)]
        },
    ));
    Ok(facts)
}

async fn require_creation(conn: &mut Conn, principal: &Principal, home: Id) -> ApiResult<()> {
    authorization::require(
        conn,
        principal,
        home,
        PermissionName::OfNode(NodePermission::Interact),
        Act::Use,
        Policy::default(),
    )
    .await
    .map(|_| ())
}

impl effects::Effect for Environments {
    type Source = Source;
    type Query = ();
    type Body = EnvironmentDuplicateBody;
    type Value = Ready;
    type Output = Id;
    fn declaration(value: &Ready) -> &'static Declaration<Source> {
        declaration(
            matches!(&value.placement, Placement::Connected { .. }),
            value.review.is_some(),
            value.preview,
        )
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: EnvironmentDuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(ctx, conn, caller, Intent::Copy(body)))
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            duplicates::copy_to(
                conn,
                principal,
                duplicates::subject(DuplicateKind::Environment, value.source.id),
                Some(value.home),
            )
            .await?;
            require_creation(conn, principal, value.home).await?;
            if let Placement::Connected { registration, .. } = &value.placement {
                let current = machine_create::resolve_for_machine(
                    conn,
                    principal,
                    registration.machine.id,
                    registration.body.clone(),
                )
                .await?;
                if !machine_create::same_preparation(registration, &current) {
                    return Err(DbError::Retry.into());
                }
            }
            if let Some(review) = &value.review {
                if !duplicates::owns(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ResourceKind::Environment.into(),
                        id: value.source.id,
                    },
                )
                .await?
                {
                    return Err(ApiError::new(
                        ErrorCode::AuthzDenied,
                        "the environment owner changed",
                    ));
                }
                crate::routes::resources::require(
                    conn,
                    principal,
                    ResourceKind::Environment,
                    value.source.id,
                    orch_access_contract::permissions::Permission::EnvironmentManage,
                )
                .await?;
                if orch_engine::environments::ending::review(conn, &value.source)
                    .await
                    .map_err(super::super::ending_error)?
                    != *review
                {
                    return Err(duplicates::moving::stale());
                }
            }
            let files = value.files()?;
            // This checks its true Environment source, not a thread's directory or another key.
            orch_engine::initial_files::require_source(conn, principal, files)
                .await
                .map_err(ApiError::engine)
        })
    }
}

impl effects::Apply for Environments {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        Box::pin(async move {
            let origin =
                CapturedOperation::capture(principal.caller()).map_err(ApiError::internal)?;
            let raw = native::http_origin::capture_operation(ctx, conn, &origin).await?;
            let deadline = before.deadline();
            let made = match &value.placement {
                Placement::Managed { input, native } => {
                    let made = match orch_engine::environments::apply_creation(
                        conn,
                        ctx.app.clock.now(),
                        native,
                        move || {
                            before.check().map_err(|error| {
                                orch_engine::EngineError::BeforeEffect(Box::new(error))
                            })
                        },
                    )
                    .await
                    {
                        Ok(made) => made,
                        Err(error) => {
                            return Err(creation::sparse(
                                principal.caller(),
                                creation_failed(conn, error).await?,
                            ));
                        }
                    };
                    finish_input(ctx, conn, principal, input, &made).await?;
                    made
                }
                Placement::Connected {
                    registration,
                    files,
                } => {
                    let created = machine_create::insert_native(
                        ctx,
                        conn,
                        principal,
                        registration,
                        move || before.check(),
                    )
                    .await?;
                    sqlx::query("UPDATE environments SET config=$2 WHERE id=$1")
                        .bind(created.environment)
                        .bind(&value.config)
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                    let state = orch_db::queries::environments::hosting::environment(
                        conn,
                        created.environment,
                    )
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                    let job = orch_engine::jobs::provision::queue(
                        conn,
                        ctx.app.clock.now(),
                        &state,
                        orch_engine::jobs::provision::Mode::Copy,
                        Some(principal.user()),
                    )
                    .await
                    .map_err(ApiError::internal)?;
                    orch_db::queries::jobs::merge_plan(
                        conn,
                        job.id,
                        &json!({"initial_files":files}),
                        ctx.app.clock.now(),
                    )
                    .await?;
                    orch_engine::environments::Made {
                        id: created.environment,
                        job,
                    }
                }
            };
            sqlx::query(
                "UPDATE environments SET remote_write=$2,terminal=$3,browser=$4,vnc=$5 WHERE id=$1",
            )
            .bind(made.id)
            .bind(value.source.remote_write)
            .bind(value.source.terminal)
            .bind(value.source.browser)
            .bind(value.source.vnc)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
            if !value.tags.is_empty() {
                orch_db::queries::resources::grouping::set_tags(
                    conn,
                    ResourceKind::Environment,
                    made.id,
                    &value.tags,
                    ctx.app.clock.now(),
                )
                .await?;
            }
            if !value.secrets.is_empty() {
                let master = super::super::master_key()?;
                for (name, value) in &value.secrets {
                    orch_settings::secrets::put(
                        conn,
                        master,
                        &orch_settings::secrets::Put {
                            actor: orch_db::audit::Actor::User(principal.user()),
                            owner: orch_settings::secrets::Owner::Environment(made.id),
                            name,
                            value,
                            origin: &crate::sign_in::origin(ctx),
                        },
                        ctx.app.clock.now(),
                    )
                    .await
                    .map_err(ApiError::internal)?;
                    deadline.check()?;
                }
            }
            if let Some(review) = &value.review {
                orch_db::queries::jobs::merge_plan(
                    conn,
                    made.job.id,
                    &json!({"environment_move_out":review}),
                    ctx.app.clock.now(),
                )
                .await?;
            }
            orch_engine::environment_creation::retain_duplicate(
                conn,
                &origin,
                &raw,
                &made,
                value.home,
                principal.now(),
            )
            .await
            .map_err(ApiError::engine)?;
            let mut files = value.files()?.clone();
            files
                .capture_environment_creator(principal.caller(), made.job.id, made.id, 0)
                .map_err(ApiError::engine)?;
            orch_db::queries::jobs::merge_plan(
                conn,
                made.job.id,
                &json!({"initial_files":files}),
                ctx.app.clock.now(),
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(made.id))
        })
    }
}

enum Intent {
    Copy(EnvironmentDuplicateBody),
    Preview(EnvironmentDuplicateBody),
    Move(EnvironmentMoveOutBody),
}

fn declaration(connected: bool, moving: bool, preview: bool) -> &'static Declaration<Source> {
    match (connected, moving, preview) {
        (true, true, true) => &declarations::MOVE_CONNECTED_PREVIEW,
        (false, true, true) => &declarations::MOVE_MANAGED_PREVIEW,
        (true, true, false) => &declarations::MOVE_CONNECTED,
        (false, true, false) => &declarations::MOVE_MANAGED,
        (true, false, _) => &CONNECTED,
        (false, false, _) => &MANAGED,
    }
}

async fn add_move_facts(
    conn: &mut Conn,
    facts: &mut Vec<native::Facts<Source>>,
    source: Id,
    moving: bool,
    preview: bool,
) -> ApiResult<()> {
    if moving {
        let target = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: source,
            },
        )
        .await?;
        facts.push(native::Facts::one(
            Source::MoveSubject,
            KeyTargetKind::Object(ResourceKind::Environment.into()),
            target.clone(),
        ));
        if preview {
            facts.push(native::Facts::one(
                Source::MovePreview,
                KeyTargetKind::Object(ResourceKind::Environment.into()),
                target,
            ));
        }
    }
    Ok(())
}

async fn prepare_native(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    intent: Intent,
) -> ApiResult<(Ready, Vec<native::Facts<Source>>)> {
    let preview = matches!(&intent, Intent::Preview(_));
    let moving = !matches!(&intent, Intent::Copy(_));
    let (body, confirmation) = match intent {
        Intent::Copy(body) | Intent::Preview(body) => (body, None),
        Intent::Move(body) => (body.copy, Some(body.delete_plan)),
    };
    let id = ctx.id("id")?;
    sqlx::query(if preview {
        "SELECT id FROM environments WHERE id=$1 AND deleted_at IS NULL"
    } else {
        "SELECT id FROM environments WHERE id=$1 AND deleted_at IS NULL FOR SHARE"
    })
    .bind(id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(DbError::from)?
    .ok_or_else(ApiError::not_found)?;
    let source = queries::environments(conn, &[id])
        .await?
        .pop()
        .ok_or_else(ApiError::not_found)?;
    let generic = DuplicateBody {
        name: body.name.clone(),
        destination: body.destination.clone(),
        expected_revision: body.expected_revision,
    };
    let home = duplicates::destination(&generic, Some(source.home_node_id))
        .ok_or_else(|| invalid("destination", "an environment needs a hierarchy home"))?;
    if body.managed.is_some() && body.machine_id.is_some() {
        return Err(invalid(
            "machine_id",
            "choose one managed or connected placement",
        ));
    }
    let connected =
        body.managed.is_none() && (body.machine_id.is_some() || source.placement == "connected");
    let machine = if connected {
        Some(
            body.machine_id
                .or(source.machine_id)
                .ok_or_else(|| invalid("machine_id", "choose a destination machine"))?,
        )
    } else {
        None
    };
    let mut prefix = creation::prefix(conn, home, Some((None, source.id))).await?;
    if let Some(machine) = machine {
        prefix.push(native::Facts::one(
            Source::Machine,
            KeyTargetKind::Object(ResourceKind::Machine.into()),
            targets::machine_subject(conn, machine).await?,
        ));
    }
    add_move_facts(conn, &mut prefix, source.id, moving, preview).await?;
    let row = declaration(connected, moving, preview);
    let known = native::bind_known(ctx, row, &prefix)?;
    let principal = known.preparation_principal(ctx, conn, caller).await?;
    crate::routes::resources::require(
        conn,
        &principal,
        ResourceKind::Environment,
        source.id,
        orch_access_contract::permissions::Permission::EnvironmentView,
    )
    .await?;
    crate::routes::resources::require(
        conn,
        &principal,
        ResourceKind::Environment,
        source.id,
        orch_access_contract::permissions::Permission::FilesView,
    )
    .await?;
    duplicates::revision(&generic, source.revision)?;
    require_creation(conn, &principal, home).await?;
    let destination_org = duplicates::copy_to(
        conn,
        &principal,
        duplicates::subject(DuplicateKind::Environment, source.id),
        Some(home),
    )
    .await?;
    let (review, delete) = if moving {
        if destination_org == Some(source.org_id) {
            return Err(invalid(
                "destination",
                "use the existing Move within this organization",
            ));
        }
        if !duplicates::owns(
            conn,
            &principal,
            ObjectRef {
                kind: ResourceKind::Environment.into(),
                id: source.id,
            },
        )
        .await?
        {
            return Err(ApiError::new(
                ErrorCode::AuthzDenied,
                "only an actual owner can move this environment into another organization",
            ));
        }
        crate::routes::resources::require(
            conn,
            &principal,
            ResourceKind::Environment,
            source.id,
            orch_access_contract::permissions::Permission::EnvironmentManage,
        )
        .await?;
        let review = orch_engine::environments::ending::review(conn, &source)
            .await
            .map_err(super::super::ending_error)?;
        let delete = duplicates::moving::plan(
            orch_api_contract::duplicates::CopySubject {
                kind: orch_api_contract::duplicates::CopySubjectKind::Environment,
                id: source.id,
            },
            orch_api_contract::duplicates::DuplicateDestination::Node { id: home },
            source.revision,
            principal.user(),
            &generic,
            json!({"native_source":review.stamp()}),
            vec![
                MoveOutDeleteEffect::StopBoundWork,
                MoveOutDeleteEffect::RemoveSource,
            ],
        )?;
        if let Some(confirmation) = &confirmation {
            duplicates::moving::confirm(&delete, confirmation)?;
        }
        (Some(review), Some(delete))
    } else {
        (None, None)
    };
    let mut config = source.config.as_object().cloned().ok_or_else(|| {
        ApiError::internal("the source environment configuration is not an object")
    })?;
    // Copying configuration grants neither an earlier machine-program consent
    // nor a second reference to the original workspace or signed-in home.
    config.remove(orch_settings_contract::registry::keys::EXEC_VERSION_OVERRIDES);
    config.remove(orch_settings_contract::registry::keys::ENV_DATA_SOURCES);
    if body.managed.is_some() || connected {
        config.remove(orch_settings_contract::registry::keys::ENV_HOST_ID);
        config.remove(orch_settings_contract::registry::keys::ENV_TEMPLATE_ID);
    }
    let mut name = duplicates::display_name(&generic, &source.name);
    if body.name.is_none() {
        let mut number = 2_u64;
        while queries::named(conn, principal.user(), source.org_id, &name)
            .await?
            .is_some()
        {
            duplicates::current(ctx, principal.caller())?;
            let suffix = format!(" copy {number}");
            name = format!(
                "{}{}",
                source
                    .name
                    .chars()
                    .take(80 - suffix.len())
                    .collect::<String>(),
                suffix
            );
            number = number
                .checked_add(1)
                .ok_or_else(|| ApiError::internal("the copy name space is exhausted"))?;
        }
    }
    config.insert(
        orch_settings_contract::registry::keys::ENV_DATA_SOURCES.into(),
        serde_json::json!(orch_driver_contract::runtime::EnvironmentDataSources::default()),
    );
    let config = serde_json::Value::Object(config);
    clean_config("config", &config)?;
    orch_engine::environments::check_initial_program_consent(&config)
        .map_err(|why| invalid("config", why))?;
    let (placement, facts) = if let Some(machine) = machine {
        let mut facts = connected_facts(conn, home, source.id, machine, &config).await?;
        add_move_facts(conn, &mut facts, source.id, moving, preview).await?;
        let complete = native::bind(ctx, row, &facts)?;
        let principal = complete.preparation_principal(ctx, conn, caller).await?;
        let graph = orch_authz::engine::Graph::load(conn, &[home])
            .await
            .map_err(ApiError::internal)?;
        let namespace = graph
            .nodes()
            .filter(|node| {
                node.kind == orch_access_contract::roles::NodeKind::Org
                    && graph.get(home).is_some_and(|at| at.path.contains(&node.id))
            })
            .max_by_key(|node| node.path.len())
            .map(|node| node.id)
            .ok_or_else(ApiError::not_found)?;
        let request = api::MachineEnvironmentCreateBody {
            name,
            home_id: Some(namespace),
            home_node_id: Some(home),
            root: String::new(),
            data_sources: Some(Default::default()),
            remote_write: Some(source.remote_write),
            terminal: Some(source.terminal),
            access: None,
        };
        let mut registration =
            machine_create::resolve_for_machine(conn, &principal, machine, request.clone())
                .await
                .map_err(|error| {
                    if error.code() == ErrorCode::NotFound {
                        invalid(
                            "machine_id",
                            "choose a machine whose Use permits this owner and place",
                        )
                    } else {
                        creation::sparse(caller, error)
                    }
                })?;
        if !preview && orch_db::queries::transaction_guard::in_transaction(conn) {
            orch_db::queries::transaction_guard::share_machine(conn, machine).await?;
            crate::routes::resources::creation_home::lock(conn, Some(namespace)).await?;
            sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR SHARE")
                .bind(home)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            if !orch_authz::environments::reserve_capacity(conn, &registration.created)
                .await
                .map_err(ApiError::internal)?
            {
                return Err(invalid(
                    "machine_id",
                    "the selected Use entry has no remaining environment capacity",
                ));
            }
            let fresh = machine_create::resolve_for_machine(conn, &principal, machine, request)
                .await
                .map_err(|error| creation::sparse(caller, error))?;
            if !machine_create::same_preparation(&registration, &fresh) {
                return Err(DbError::Retry.into());
            }
            registration = fresh;
        }
        let files = orch_engine::initial_files::plan(
            conn,
            &principal,
            &NewEnvironmentFiles::FromEnvironment {
                from_environment: source.id,
                selection: None,
            },
        )
        .await
        .map_err(initial_failed)?;
        // Mount use is the same native placement check; facts cannot grant it.
        let mounts = orch_engine::places::mount_ids_for_new(conn, home, &config)
            .await
            .map_err(ApiError::engine)?;
        let policies = orch_engine::access::resource_policies(
            conn,
            registration.machine.host_id.unwrap_or(machine),
        )
        .await
        .map_err(ApiError::engine)?;
        match orch_authz::environments::check_creation(
            conn,
            &principal,
            &orch_authz::environments::NewEnvironment {
                home,
                place: orch_authz::environments::Place::Machine { machine },
                mounts,
            },
            &policies,
        )
        .await
        .map_err(ApiError::internal)?
        {
            orch_authz::environments::Creating::Allowed(_) => {}
            _ => {
                return Err(invalid(
                    "machine_id",
                    "choose a machine whose Use permits this owner and place",
                ));
            }
        }
        (
            Placement::Connected {
                registration: Box::new(registration),
                files: Box::new(files),
            },
            facts,
        )
    } else {
        let template_id = body
            .managed
            .as_ref()
            .map_or(source.template_id, |choice| choice.template_id);
        let host_id = body.managed.as_ref().map(|choice| choice.host_id);
        let requested = NewEnvironment {
            name: Some(name),
            home_node_id: Some(home),
            template_id,
            host_id,
            config: Some(config.clone()),
            access: None,
            files: Some(NewEnvironmentFiles::FromEnvironment {
                from_environment: source.id,
                selection: None,
            }),
            data_sources: Some(Default::default()),
        };
        let input = prepare_input(ctx, conn, &principal, requested, None)
            .await
            .map_err(|error| creation::sparse(caller, error))?;
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
            Err(error) => {
                return Err(creation::sparse(
                    caller,
                    creation_failed(conn, error).await?,
                ));
            }
        };
        let mut facts = creation::complete(
            conn,
            home,
            native.template(),
            native.host(),
            native.mounts(),
            Some((None, source.id)),
        )
        .await?;
        add_move_facts(conn, &mut facts, source.id, moving, preview).await?;
        (
            Placement::Managed {
                input: Box::new(input),
                native: Box::new(native),
            },
            facts,
        )
    };
    let tags =
        duplicates::copy_tags(conn, ResourceKind::Environment, source.id, Some(home)).await?;
    let secrets = if duplicates::owns(
        conn,
        &principal,
        ObjectRef {
            kind: ResourceKind::Environment.into(),
            id: source.id,
        },
    )
    .await?
    {
        let names = orch_settings::secrets::list(
            conn,
            orch_settings::secrets::Owner::Environment(source.id),
        )
        .await
        .map_err(ApiError::internal)?
        .into_iter()
        .filter(|info| info.node.is_none())
        .map(|info| info.name)
        .collect::<BTreeSet<_>>();
        duplicates::secret_values(
            conn,
            orch_db::queries::settings::owner::ENVIRONMENT,
            source.id,
            &names,
        )
        .await?
    } else {
        BTreeMap::new()
    };
    Ok((
        Ready {
            source,
            review,
            delete,
            preview,
            home,
            config,
            placement,
            tags,
            secrets,
        },
        facts,
    ))
}

impl effects::Effect for Moving {
    type Source = Source;
    type Query = ();
    type Body = EnvironmentMoveOutBody;
    type Value = Ready;
    type Output = Id;
    fn declaration(value: &Ready) -> &'static Declaration<Source> {
        declaration(
            matches!(&value.placement, Placement::Connected { .. }),
            true,
            false,
        )
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: EnvironmentMoveOutBody,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(ctx, conn, caller, Intent::Move(body)))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<()>> {
        <Environments as effects::Effect>::require(ctx, conn, principal, value)
    }
}

impl effects::Apply for Moving {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Id>>> {
        <Environments as effects::Apply>::apply(ctx, conn, principal, value, before)
    }
}

fn preview_prepare<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    body: EnvironmentDuplicateBody,
) -> crate::registry::OptionalOperationFuture<'a> {
    native::selected::<Preview>(ctx, conn, caller, body, ())
}

async fn preview(ctx: Ctx, (): (), body: EnvironmentDuplicateBody) -> ApiResult<MoveOutDeletePlan> {
    native::selected_read::<Preview>(ctx, body, ()).await
}

impl native::Selected for Preview {
    type Source = Source;
    type Query = EnvironmentDuplicateBody;
    type Value = Ready;
    type Reply = MoveOutDeletePlan;
    const MOVES: native::Moves = native::Moves::Repeat;
    const DECLARATION: &'static Declaration<Source> = &declarations::MOVE_MANAGED_PREVIEW;
    const WATCH_ORDINARY: bool = true;
    fn declaration(_: &EnvironmentDuplicateBody, value: &Ready) -> &'static Declaration<Source> {
        declaration(
            matches!(&value.placement, Placement::Connected { .. }),
            true,
            true,
        )
    }
    fn select<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        body: &'a EnvironmentDuplicateBody,
    ) -> BoxFuture<'a, ApiResult<(Ready, Vec<native::Facts<Source>>)>> {
        Box::pin(prepare_native(
            ctx,
            conn,
            caller,
            Intent::Preview(body.clone()),
        ))
    }
    fn require<'a>(
        ctx: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            ctx.user()?;
            Ok(())
        })
    }
    fn render<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        principal: &'a Principal,
        value: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<MoveOutDeletePlan>> {
        Box::pin(async move {
            <Environments as effects::Effect>::require(ctx, conn, principal, value).await?;
            value
                .delete
                .clone()
                .ok_or_else(|| ApiError::internal("the native environment has no Delete plan"))
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a EnvironmentDuplicateBody,
        old: &'a Ready,
        fresh: &'a Ready,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(old.review == fresh.review && old.delete == fresh.delete && old.home == fresh.home)
        })
    }
}
