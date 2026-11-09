//! Native confirmation materializes one saved recipe; replay is a separate no-file-act variant.

use super::*;

use crate::key_operations::native;

use crate::key_operations::native::effects;

use crate::key_operations::native::reply;

use crate::key_operations::targets as api_targets;

use futures::future::BoxFuture;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::key_scope::KeyIdentityKind;

use orch_access_contract::key_scope::KeyTargetKind;

use orch_access_contract::permissions::ObjectKind;

use orch_api_contract::common::ReceiptProjection;

use orch_api_contract::operation_sources::Declaration;

use orch_api_contract::operations::FacetUsage;

use orch_api_contract::restore_sources as declarations;

use orch_api_contract::restore_sources::Source as Role;

use orch_api_contract::source::RestoreStartReceipt;

use orch_api_contract::source::RestoreStartResponse;

use orch_authz::Caller;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::jobs::restore::operation as source_scope;

use orch_engine::jobs::restore::operation::Anchor;

use orch_engine::jobs::restore::operation::Mode;

use orch_engine::jobs::restore::operation::Request;
pub(super) struct Start;

pub(super) enum Prepared {
    New {
        frozen: Box<Frozen>,
        anchor: Box<Anchor>,
        mode: Mode,
        facts: Vec<native::Facts<Role>>,
        until: Time,
    },
    Existing {
        row: Box<jobs::JobRow>,
        preflight: Id,
        facts: Vec<native::Facts<Role>>,
    },
}

pub(super) enum Output {
    New {
        row: Box<jobs::JobRow>,
        preflight: Id,
        undo: Id,
        anchor: Box<Anchor>,
        mode: Mode,
        facts: Vec<native::Facts<Role>>,
        until: Time,
        original: Option<Box<(Caller, Time)>>,
    },
    Existing {
        row: Box<jobs::JobRow>,
        preflight: Id,
        facts: Vec<native::Facts<Role>>,
    },
}

async fn root(conn: &mut Conn, environment: Id) -> ApiResult<Vec<native::Facts<Role>>> {
    Ok(source_scope::environment_facts(conn, environment)
        .await
        .map_err(ApiError::engine)?
        .into_iter()
        .filter(|fact| fact.source == Role::Environment)
        .collect())
}

async fn preliminary(ctx: &Ctx, conn: &mut Conn, caller: &Caller) -> ApiResult<Principal> {
    let facts = root(conn, ctx.id("id")?).await?;
    for declaration in [
        &declarations::RESTORE_EXISTING,
        &declarations::RESTORE_ENVIRONMENT,
        &declarations::RESTORE_THREADS,
    ] {
        let known = native::bind_known(ctx, declaration, &facts)?;
        match native::preparation_principal(ctx, conn, caller, &known).await {
            Ok(actor) => {
                require(conn, &actor, ctx.id("id")?, Permission::FilesManage).await?;
                return Ok(actor);
            }
            Err(error) if error.problem.status == 403 => {}
            Err(error) => return Err(error),
        }
    }
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "the native restore operation is outside this credential",
    ))
}

async fn selected_mode(
    ctx: &Ctx,
    conn: &mut Conn,
    caller: &Caller,
    plan: &Plan,
) -> ApiResult<(Mode, Vec<native::Facts<Role>>)> {
    for mode in [Mode::Environment, Mode::Threads] {
        let facts = source_scope::confirmation_facts(conn, plan, mode)
            .await
            .map_err(ApiError::engine)?;
        let known = native::bind_required(ctx, mode.declaration(), &facts)?;
        let actor = match native::preparation_principal(ctx, conn, caller, &known).await {
            Ok(actor) => actor,
            Err(error) if error.problem.status == 403 => continue,
            Err(error) => return Err(error),
        };
        match source_scope::require_confirmation(conn, &actor, plan, mode).await {
            Ok(()) => return Ok((mode, facts)),
            Err(orch_engine::operation_scope::continuing::Error::Refused) => {}
            Err(error) => return Err(preflight::error(error)),
        }
    }
    Err(ApiError::new(
        ErrorCode::AuthzDenied,
        "current authority does not permit controlling the reviewed writers",
    ))
}

impl effects::Effect for Start {
    type Source = Role;
    type Query = ();
    type Body = EnvironmentFilesRestoreBody;
    type Value = Prepared;
    type Output = Output;
    fn declaration(value: &Prepared) -> &'static Declaration<Role> {
        match value {
            Prepared::New { mode, .. } => mode.declaration(),
            Prepared::Existing { .. } => &declarations::RESTORE_EXISTING,
        }
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            preliminary(ctx, conn, caller).await?;
            let saved = jobs::changes::lock_plan(conn, body.preflight_id)
                .await?
                .filter(|row| {
                    row.kind == jobs::kind::WORKSPACE_RESTORE
                        && row.environment_id == Some(environment)
                        && row.created_by == caller.user()
                })
                .ok_or_else(ApiError::not_found)?;
            let existing: Option<Id> = sqlx::query_scalar("SELECT id FROM jobs WHERE kind='workspace_restore' AND plan->>'preflight_id'=$1 ORDER BY created_at LIMIT 1")
                .bind(body.preflight_id.to_string()).fetch_optional(&mut *conn).await.map_err(DbError::from)?;
            if let Some(id) = existing {
                let row = jobs::job(conn, id)
                    .await?
                    .filter(|row| row.environment_id == Some(environment))
                    .ok_or_else(ApiError::not_found)?;
                let facts = root(conn, environment).await?;
                return Ok((
                    Prepared::Existing {
                        row: Box::new(row),
                        preflight: body.preflight_id,
                        facts: facts.clone(),
                    },
                    facts,
                ));
            }
            let mut frozen: Frozen =
                serde_json::from_value(saved.plan).map_err(ApiError::internal)?;
            if saved.expires_at <= ctx.app.clock.now() {
                return Err(ApiError::new(
                    ErrorCode::PreflightExpired,
                    "the restore plan expired; make a new plan",
                ));
            }
            if !frozen.shown.blockers.is_empty() {
                return Err(ApiError::new(
                    ErrorCode::PlanBlocked,
                    "the restore plan has unresolved blockers",
                )
                .with("blockers", frozen.shown.blockers));
            }
            if let Some(touched) = frozen
                .review
                .as_ref()
                .and_then(|review| review.anchor().command().touched.as_ref())
                && frozen.shown.paths.iter().any(|path| {
                    path.later_change != Some(false) && !touched.overwrite.contains(&path.path)
                })
            {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceRestoreConflict,
                    "a touched file changed since the agent wrote it; choose overwrite explicitly",
                ));
            }
            // Existing native generation/error outcomes precede the deeper source frame.
            let actor = preliminary(ctx, conn, caller).await?;
            let door = super::super::door::open_with_principal(ctx, conn, environment, None, actor)
                .await?;
            door.may_write()?;
            if door.env.generation != frozen.recipe.generation
                || door.env.machine_id != Some(frozen.recipe.machine)
            {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceRestoreConflict,
                    "the environment changed; make a new plan",
                ));
            }
            let snapshot = storage::snapshot(conn, frozen.shown.snapshot.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if snapshot.environment_id != environment
                || snapshot.state != "complete"
                || snapshot
                    .expires_at
                    .is_none_or(|until| until <= ctx.app.clock.now())
            {
                return Err(unavailable("expired"));
            }
            let anchor = match &frozen.review {
                Some(review) => {
                    if review.user() != caller.user() {
                        return Err(ApiError::not_found());
                    }
                    review.anchor().clone()
                }
                None => {
                    if native::modern(caller) {
                        return Err(ApiError::new(
                            ErrorCode::PreflightExpired,
                            "make a new restore review under its original operation",
                        ));
                    }
                    let Action::Restore {
                        record_days,
                        max_bytes,
                        ..
                    } = &frozen.recipe.action
                    else {
                        return Err(ApiError::internal(
                            "the saved native restore recipe is missing",
                        ));
                    };
                    Anchor::new(
                        &door.env,
                        &snapshot,
                        frozen.recipe.machine,
                        None,
                        *record_days,
                        *max_bytes,
                    )
                    .map_err(ApiError::engine)?
                }
            };
            if orch_db::queries::transaction_guard::in_transaction(conn) {
                orch_db::queries::environments::hosting::lock_environment(conn, environment)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                if let Some(review) = &frozen.review {
                    review
                        .prepare(conn, ctx.app.clock.as_ref(), None, None)
                        .await
                        .map_err(preflight::error)?;
                }
                if let Some(thread) = snapshot.thread_id {
                    sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
                        .bind(thread)
                        .execute(&mut *conn)
                        .await
                        .map_err(DbError::from)?;
                }
                sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR SHARE")
                    .bind(snapshot.id)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
                orch_db::queries::uploads::lock_blobs(conn, &[frozen.plan_blob]).await?;
            }
            let current = anchor
                .metadata(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(|| {
                    ApiError::new(
                        ErrorCode::WorkspaceRestoreConflict,
                        "the recorded restore source changed; make a new plan",
                    )
                })?;
            let until = current
                .snapshot
                .expires_at
                .map(Time::new)
                .ok_or_else(|| unavailable("expired"))?
                .min(anchor.until());
            if current.snapshot.state != "complete" || until <= ctx.app.clock.time() {
                return Err(unavailable("expired"));
            }
            let identity = transaction_guard::current_http_identity(
                ctx,
                conn,
                transaction_guard::HttpIdentityMode::Read,
            )
            .await?;
            transaction_guard::guard_user(&identity.user)?;
            let caller = identity
                .caller
                .intersect_restrictions(caller)
                .map_err(ApiError::internal)?;
            frozen.recipe.caller = caller.user();
            frozen.recipe.token = caller.grant().and_then(|grant| grant.token());
            let (mode, facts) = selected_mode(ctx, conn, &caller, &frozen.recipe).await?;
            Ok((
                Prepared::New {
                    frozen: Box::new(frozen),
                    anchor: Box::new(anchor),
                    mode,
                    facts: facts.clone(),
                    until,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match value {
                Prepared::Existing { row, .. } => {
                    require(
                        conn,
                        actor,
                        row.environment_id.ok_or_else(ApiError::not_found)?,
                        Permission::FilesManage,
                    )
                    .await
                }
                Prepared::New {
                    frozen,
                    mode,
                    anchor,
                    ..
                } => {
                    source_scope::require_target(conn, actor, anchor, ctx.app.clock.time())
                        .await
                        .map_err(preflight::error)?;
                    source_scope::require_confirmation(conn, actor, &frozen.recipe, *mode)
                        .await
                        .map_err(preflight::error)
                }
            }
        })
    }
}

impl effects::Apply for Start {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Output>>> {
        Box::pin(async move {
            let Prepared::New {
                frozen,
                anchor,
                mode,
                facts,
                until,
            } = value
            else {
                let Prepared::Existing {
                    row,
                    preflight,
                    facts,
                } = value
                else {
                    return Err(ApiError::internal("the native confirmation has no outcome"));
                };
                return Ok(effects::Outcome::Unchanged(Output::Existing {
                    row: row.clone(),
                    preflight: *preflight,
                    facts: facts.clone(),
                }));
            };
            let original = match &frozen.review {
                Some(review) => Some(
                    review
                        .prepare(conn, ctx.app.clock.as_ref(), None, None)
                        .await
                        .map_err(preflight::error)?,
                ),
                None => None,
            };
            let original_clock = original
                .as_ref()
                .map(|frame| (frame.principal().caller().clone(), frame.until()));
            let deadline = before.deadline();
            let row = orch_engine::jobs::source::begin_before(
                conn,
                frozen.recipe.clone(),
                ctx.now,
                move || {
                    if let Some(frame) = original {
                        frame
                            .into_parts()
                            .check(ctx.app.clock.as_ref())
                            .map_err(|error| {
                                Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                            })?;
                    }
                    before.check().map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                },
            )
            .await
            .map_err(super::super::source::make_error)?;
            jobs::merge_plan(
                conn,
                row.id,
                &json!({"preflight_id":frozen.shown.preflight_id}),
                ctx.now,
            )
            .await?;
            let recipe: Plan =
                serde_json::from_value(row.plan.clone()).map_err(ApiError::internal)?;
            let Action::Restore {
                body, directory, ..
            } = &recipe.action
            else {
                return Err(ApiError::internal(
                    "the created restore lost its native recipe",
                ));
            };
            Request::retain(
                conn,
                &row,
                actor.caller(),
                source_scope::Confirmation {
                    session: preflight::session(ctx)?,
                    review: frozen.review.clone(),
                    anchor: anchor.as_ref().clone(),
                    mode: *mode,
                    now: ctx.app.clock.time(),
                },
            )
            .await
            .map_err(ApiError::engine)?;
            let selection = match &frozen.shown.form {
                orch_api_contract::source::RestoreForm::WholeDirectory => {
                    json!({"include":[directory],"exclude":[]})
                }
                orch_api_contract::source::RestoreForm::TouchedFiles { .. } => json!({
                    "include": frozen.shown.paths.iter().map(|path| {
                        if directory == "." { path.path.clone() } else { format!("{directory}/{}", path.path) }
                    }).collect::<Vec<_>>(),
                    "exclude": [], "restore_form": "touched"
                }),
            };
            source::begin_recorded(
                conn,
                &storage::NewSnapshot {
                    id: body.undo_snapshot_id,
                    thread_id: recipe.thread,
                    environment_id: recipe.environment,
                    env_generation: recipe.generation,
                    label: "Before restore",
                    selection: &selection,
                    created_by: Some(actor.user()),
                    at: ctx.now,
                },
                directory,
                "restore",
            )
            .await?;
            storage::pin_blobs(conn, &[frozen.plan_blob]).await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Output::New {
                row: Box::new(row),
                preflight: frozen.shown.preflight_id,
                undo: body.undo_snapshot_id,
                anchor: anchor.clone(),
                mode: *mode,
                facts: facts.clone(),
                until: *until,
                original: original_clock.map(Box::new),
            }))
        })
    }
}

fn receipt(output: &Output) -> RestoreStartResponse {
    let (row, preflight, undo) = match output {
        Output::New {
            row,
            preflight,
            undo,
            ..
        } => (row, *preflight, Some(*undo)),
        Output::Existing { row, preflight, .. } => (row, *preflight, None),
    };
    RestoreStartResponse::Receipt(RestoreStartReceipt {
        projection: ReceiptProjection::Receipt,
        job_id: row.id,
        preflight_id: preflight,
        undo_snapshot_id: undo,
    })
}

async fn response_gate(conn: &mut Conn, actor: &Principal, environment: Id) -> ApiResult<()> {
    match require(conn, actor, environment, Permission::FilesView).await {
        Ok(()) => Ok(()),
        Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
            require(conn, actor, environment, Permission::FilesManage).await
        }
        Err(error) => Err(error),
    }
}

struct NewProjection;

impl reply::Projection for NewProjection {
    type Source = Role;
    type Input = Output;
    type Subject = source_scope::Native;
    type Value = ();
    type Reply = Job;
    const READ_SOURCE: Role = Role::Content;
    fn declaration(input: &Output) -> &'static Declaration<Role> {
        match input {
            Output::New { mode, .. } => mode.declaration(),
            _ => &declarations::RESTORE_ENVIRONMENT,
        }
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Output,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let Output::New { anchor, facts, .. } = input else {
                return Err(ApiError::internal("a replay is not a new restore"));
            };
            let source = anchor
                .metadata(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            let mut current = source_scope::environment_facts(conn, source.environment.id)
                .await
                .map_err(ApiError::engine)?;
            current.extend(
                facts
                    .iter()
                    .filter(|fact| fact.source == Role::Writers)
                    .cloned(),
            );
            Ok((source, current))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        _: &'a Output,
        source: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(response_gate(conn, actor, source.environment.id))
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Output,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Role>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Output,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(async move {
            let Output::New { row, .. } = input else {
                return Err(ApiError::internal("the new restore outcome changed"));
            };
            crate::routes::environments::jobs::view(conn, row).await
        })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Output,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            Ok(
                old.subject.environment.generation == fresh.subject.environment.generation
                    && old.subject.environment.machine_id == fresh.subject.environment.machine_id
                    && old.subject.environment.workspace_root
                        == fresh.subject.environment.workspace_root
                    && old.subject.snapshot.id == fresh.subject.snapshot.id,
            )
        })
    }
}

struct ExistingProjection;

struct Named {
    names: Option<std::collections::HashMap<Id, String>>,
}

impl reply::Projection for ExistingProjection {
    type Source = Role;
    type Input = Output;
    type Subject = jobs::JobRow;
    type Value = Named;
    type Reply = Job;
    const READ_SOURCE: Role = Role::ExistingJob;
    fn declaration(_: &Output) -> &'static Declaration<Role> {
        &declarations::RESTORE_EXISTING
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Output,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let Output::Existing {
                row: old,
                preflight,
                facts,
            } = input
            else {
                return Err(ApiError::internal("a new restore is not a replay"));
            };
            let row = jobs::job(conn, old.id)
                .await?
                .ok_or_else(ApiError::not_found)?;
            if row.kind != old.kind
                || row.node_id != old.node_id
                || row.environment_id != old.environment_id
                || row.provider_id != old.provider_id
                || row.created_by != old.created_by
                || row.created_at != old.created_at
                || row.plan.get("preflight_id") != Some(&json!(preflight))
            {
                return Err(ApiError::not_found());
            }
            let environment = row.environment_id.ok_or_else(ApiError::not_found)?;
            let target = api_targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: environment,
                },
            )
            .await?;
            let identity =
                crate::routes::environments::jobs::subject::identity(&row, Some(&target));
            let mut facts = facts.clone();
            facts.push(native::Facts::one(
                Role::ExistingJob,
                KeyTargetKind::Identity(KeyIdentityKind::Job),
                identity,
            ));
            Ok((row, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        _: &'a Output,
        row: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            response_gate(
                conn,
                actor,
                row.environment_id.ok_or_else(ApiError::not_found)?,
            )
            .await
        })
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        _: &'a Output,
        row: &'a jobs::JobRow,
    ) -> BoxFuture<'a, ApiResult<(Named, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let mut named = std::collections::HashMap::new();
            let mut selected = Vec::new();
            if native::modern(actor.caller()) {
                let names = crate::routes::environments::jobs::machine_name_sources(
                    conn,
                    std::slice::from_ref(row),
                )
                .await?;
                let targets = api_targets::machine_identities(
                    conn,
                    &names.keys().copied().collect::<Vec<_>>(),
                )
                .await?;
                let plan = crate::registry::OperationPlan::new(
                    ctx.route,
                    declarations::RESTORE_EXISTING.operation,
                )?;
                for (id, name) in names {
                    if let Some(target) = targets.get(&id)
                        && native::permits_usage(
                            &plan,
                            &declarations::RESTORE_EXISTING,
                            actor.caller(),
                            &Role::MachineNames,
                            FacetUsage::ResponseContent,
                            KeyTargetKind::Object(ObjectKind::Machine),
                            target,
                        )?
                    {
                        named.insert(id, name);
                        selected.push(target.clone());
                    }
                }
            }
            let facts = native::Facts::collection(
                Role::MachineNames,
                if selected.is_empty() {
                    Vec::new()
                } else {
                    vec![(KeyTargetKind::Object(ObjectKind::Machine), selected)]
                },
            );
            Ok((
                Named {
                    names: native::modern(actor.caller()).then_some(named),
                },
                vec![facts],
            ))
        })
    }
    fn render<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Output,
        prepared: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<Job>> {
        Box::pin(
            crate::routes::environments::jobs::view_with_machine_sources(
                conn,
                &prepared.subject,
                prepared.value.names.as_ref(),
            ),
        )
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Output,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let names = match (&old.value.names, &fresh.value.names) {
                (Some(old_names), Some(new)) => {
                    old_names.keys().all(|id| new.contains_key(id))
                        && (old_names.is_empty()
                            || crate::routes::environments::jobs::waiting_machine(&old.subject)
                                == crate::routes::environments::jobs::waiting_machine(
                                    &fresh.subject,
                                ))
                }
                (None, _) => true,
                _ => false,
            };
            Ok(names
                && old.subject.id == fresh.subject.id
                && old.subject.kind == fresh.subject.kind
                && old.subject.node_id == fresh.subject.node_id
                && old.subject.environment_id == fresh.subject.environment_id
                && old.subject.created_by == fresh.subject.created_by
                && old.subject.created_at == fresh.subject.created_at)
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: EnvironmentFilesRestoreBody,
) -> ApiResult<RestoreStartResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<Start>(&ctx, &mut tx).await?;
    let identity = transaction_guard::current_http_identity(
        &completed.ctx,
        &mut tx,
        transaction_guard::HttpIdentityMode::Read,
    )
    .await?;
    transaction_guard::guard_user(&identity.user)?;
    identity.check_deadline(&ctx.app)?;
    if !completed
        .accepted
        .key_deadline_allows(ctx.app.clock.as_ref())
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the restore confirmation credential has expired",
        ));
    }
    if let Output::New {
        until, original, ..
    } = &completed.output
    {
        if *until <= ctx.app.clock.time() {
            return Err(unavailable("expired"));
        }
        if original.as_deref().is_some_and(|(caller, until)| {
            *until <= ctx.app.clock.time() || !caller.key_deadline_allows(ctx.app.clock.as_ref())
        }) {
            return Err(ApiError::new(
                ErrorCode::AuthInvalidToken,
                "the original restore review has expired",
            ));
        }
    }
    tx.commit().await.map_err(DbError::from)?;
    let fallback = receipt(&completed.output);
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    let content = match completed.output {
        output @ Output::New { .. } => {
            ctx.app.engine.kick();
            reply::render::<NewProjection>(&completed.ctx, &completed.accepted, output).await?
        }
        output @ Output::Existing { .. } => {
            reply::render::<ExistingProjection>(&completed.ctx, &completed.accepted, output).await?
        }
    };
    Ok(match content {
        Some(job) => RestoreStartResponse::Full(Box::new(job)),
        None => fallback,
    })
}
