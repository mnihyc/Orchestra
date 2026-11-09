//! One native measurement and content supplier, then the guarded saved-plan writer.

use super::*;
use crate::key_operations::native::{
    self,
    effects::{self, Effect, Initial},
    reply,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    common::ReceiptProjection,
    operation_sources::Declaration,
    restore_sources::{self as declarations, Source as Role},
    source::{RestorePreflightReceipt, RestorePreflightResponse},
};
use orch_authz::{Caller, Principal};
use orch_db::{Conn, DbError};
use orch_engine::{
    commands::{CommandAuthority, Outgoing},
    jobs::restore::operation::{self as source_scope, Anchor, RestoreCommand, Review},
    operation_scope::{continuing, targets},
};
use orch_link_contract::source::FsRestorePlanResult;

pub(super) struct ReviewPlan;
pub(super) struct Input {
    review: Review,
    target: Option<threads::TurnRow>,
    applied: AppliedChangeOptions,
    overwrite: Vec<String>,
}
pub(super) struct Measured {
    input: Input,
    measured: FsRestorePlanResult,
    private: Json,
    size: i64,
}
pub(super) struct Prepared {
    frozen: Frozen,
    size: i64,
    until: Time,
}
pub(super) struct Saved {
    frozen: Frozen,
    until: Time,
}

pub(super) fn error(error: continuing::Error) -> ApiError {
    match error {
        continuing::Error::Refused => ApiError::new(
            ErrorCode::AuthzDenied,
            "the original recorded restore source is no longer admitted",
        ),
        continuing::Error::Native(error) => ApiError::engine(error),
        continuing::Error::Binding(error) => crate::key_operations::admission::binding_error(error),
    }
}
pub(super) fn session(ctx: &Ctx) -> ApiResult<Option<Id>> {
    Ok(match ctx.user()?.via {
        crate::middleware::authentication::Via::Session { session_id } => Some(session_id),
        crate::middleware::authentication::Via::Token { .. }
        | crate::middleware::authentication::Via::OwnerRead { .. } => None,
    })
}
async fn facts(
    conn: &mut Conn,
    environment: Id,
    names: &[Id],
) -> ApiResult<Vec<native::Facts<Role>>> {
    let mut facts = source_scope::environment_facts(conn, environment)
        .await
        .map_err(ApiError::engine)?;
    let mut targets = Vec::new();
    for id in names {
        targets.push(
            targets::node(conn, *id)
                .await
                .map_err(|error| ApiError::engine(orch_engine::EngineError::unreadable(error)))?,
        );
    }
    facts.push(native::Facts::collection(
        Role::WriterNames,
        if targets.is_empty() {
            Vec::new()
        } else {
            vec![(
                KeyTargetKind::Object(orch_access_contract::permissions::ObjectKind::Thread),
                targets,
            )]
        },
    ));
    Ok(facts)
}

/// Native visibility plus this same operation's optional writer-name source.
async fn visible(
    ctx: &Ctx,
    conn: &mut Conn,
    actor: &Principal,
    environment: Id,
    thread: Id,
) -> ApiResult<bool> {
    if !native::modern(actor.caller()) {
        return match authorization::require(
            conn,
            actor,
            thread,
            authorization::VIEW,
            Act::Use,
            Policy::default(),
        )
        .await
        {
            Ok(()) => Ok(true),
            Err(error) if error.code() == ErrorCode::Internal => Err(error),
            Err(_) => Ok(false),
        };
    }
    let projection = ctx.for_effect_discovery(ctx.user()?.as_ref().clone(), actor.now().get());
    let plan = native::bind(
        &projection,
        &declarations::RESTORE_PREFLIGHT,
        &facts(conn, environment, &[thread]).await?,
    )?;
    let result = plan
        .read_response(&projection, conn, actor.caller(), move |conn, reader| {
            Box::pin(async move {
                authorization::require(
                    conn,
                    reader,
                    thread,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
                Ok(true)
            })
        })
        .await;
    match result {
        Ok(Some(true)) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) if matches!(error.problem.status, 401 | 403 | 404 | 410) => Ok(false),
        Err(error) => Err(error),
    }
}

/// Effect-free observation before a separate once-only immutable content read. No pool
/// lease survives into that supplier; its final plan/effect uses fresh source admission.
pub(super) async fn observed_writer(ctx: &Ctx, review: &Review, thread: Id) -> ApiResult<bool> {
    let mut tx = ctx.app.db.begin().await?;
    let frame = review
        .prepare(&mut tx, ctx.app.clock.as_ref(), None, None)
        .await
        .map_err(error)?;
    let actor = frame.principal().clone();
    let until = frame.until();
    let allowed = visible(ctx, &mut tx, &actor, review.anchor().environment(), thread).await?;
    tx.commit().await.map_err(DbError::from)?;
    if !actor.caller().key_deadline_allows(ctx.app.clock.as_ref()) || until <= ctx.app.clock.time()
    {
        return Err(ApiError::new(
            ErrorCode::AuthInvalidToken,
            "the recorded restore observation has ended",
        ));
    }
    Ok(allowed)
}

impl Effect for ReviewPlan {
    type Source = Role;
    type Query = ();
    type Body = EnvironmentRestorePreflightBody;
    type Value = Prepared;
    type Output = Saved;
    fn declaration(_: &Prepared) -> &'static Declaration<Role> {
        &declarations::RESTORE_PREFLIGHT
    }
    fn prepare<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Prepared, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let measured = effects::initial_value::<Self>(ctx)?;
            let frame = measured
                .input
                .review
                .prepare(conn, ctx.app.clock.as_ref(), None, Some(caller))
                .await
                .map_err(error)?;
            let until = frame.until();
            let actor = frame.principal();
            let native = frame.value();
            let anchor = measured.input.review.anchor();
            let environment = anchor.environment();
            if get::<i64>(
                conn,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_DAYS,
            )
            .await
            .map_err(ApiError::internal)?
                != anchor.command().record_days
                || get::<i64>(
                    conn,
                    Place::Environment(environment),
                    keys::WORKSPACE_RECORD_MAX_BYTES,
                )
                .await
                .map_err(ApiError::internal)?
                    != anchor.command().max_bytes
                || options(conn, environment, body.options.as_ref()).await?
                    != measured.input.applied
            {
                return Err(ApiError::new(
                    ErrorCode::WorkspaceRestoreConflict,
                    "the restore settings changed; make a new plan",
                ));
            }
            let directory = anchor.command().directory.clone();
            let ids = writer_ids(conn, environment, &directory).await?;
            let mut writers = Vec::new();
            let mut blockers = measured.measured.blockers.clone();
            let suspend = match require(conn, actor, environment, Permission::TerminalManage).await
            {
                Ok(()) => true,
                Err(error) if error.code() == ErrorCode::Internal => return Err(error),
                Err(_) => false,
            };
            for id in &ids {
                let row = threads::thread(conn, *id)
                    .await?
                    .ok_or_else(ApiError::not_found)?;
                let active = row.current_turn_id.is_some();
                let named = if visible(ctx, conn, actor, environment, *id).await? {
                    Some(NodeRef {
                        thread_display: None,
                        id: *id,
                        kind: NodeKind::Thread,
                        name: Some(row.name),
                        redacted: false,
                    })
                } else {
                    None
                };
                if active
                    && measured.input.applied.active_turn == ActiveTurn::Interrupt
                    && require(conn, actor, environment, Permission::EnvironmentManage)
                        .await
                        .is_err()
                    && authorization::require(
                        conn,
                        actor,
                        *id,
                        authorization::exact(Permission::ThreadInteract),
                        Act::Use,
                        Policy::default(),
                    )
                    .await
                    .is_err()
                {
                    blockers.push("interrupt_not_allowed".to_owned());
                }
                writers.push(RestoreWriter {
                    thread: named,
                    active,
                });
            }
            if measured.measured.writers_active && !writers.iter().any(|writer| writer.active) {
                writers.push(RestoreWriter {
                    thread: None,
                    active: true,
                });
            }
            let names = writers
                .iter()
                .filter_map(|writer| {
                    writer
                        .thread
                        .as_ref()
                        .map(|thread| (thread.id, thread.clone()))
                })
                .collect::<std::collections::HashMap<_, _>>();
            let mut paths = measured.measured.paths.clone();
            for path in &mut paths {
                path.later_writers = path
                    .later_writers
                    .iter()
                    .filter_map(|writer| names.get(&writer.id).cloned())
                    .collect();
            }
            if !measured.measured.undo_available {
                blockers.push("complete_undo_unavailable".to_owned());
            }
            let snapshot =
                super::super::snapshots::views(conn, vec![native.snapshot.clone()], actor.now())
                    .await?
                    .remove(0);
            let shown = RestorePlan {
                form: measured.measured.form.clone(),
                preflight_id: new_id(),
                expires_at: Time::new(
                    actor.now().get() + chrono::Duration::minutes(PREFLIGHT_VALID_MINUTES),
                ),
                environment: snapshot.environment.clone(),
                snapshot,
                generation: i64::from(native.environment.generation),
                directory: directory.clone(),
                namespace_fingerprint: decode(&measured.private, "namespace_fingerprint")?,
                ignore_fingerprint: decode(&measured.private, "ignore_fingerprint")?,
                paths,
                writers,
                ignored: vec![".git".to_owned()],
                undo_available: measured.measured.undo_available,
                options: measured.input.applied.clone(),
                blockers,
            };
            let recipe = Plan {
                environment,
                generation: native.environment.generation,
                machine: anchor.machine(),
                thread: native.snapshot.thread_id,
                caller: actor.user(),
                token: actor.caller().grant().and_then(|grant| grant.token()),
                creator_origin: None,
                scheduled_input: None,
                action: Action::Restore {
                    body: FsRestore {
                        work: None,
                        recovery_only: None,
                        environment_id: environment,
                        job_id: new_id(),
                        snapshot_id: anchor.snapshot(),
                        undo_snapshot_id: new_id(),
                        plan_blob: measured.measured.plan_blob,
                        generation: i64::from(native.environment.generation),
                    },
                    writers: ids,
                    options: measured.input.applied.clone(),
                    suspend,
                    directory,
                    record_days: decode(&measured.private["record_policy"], "record_days")?,
                    max_bytes: decode(&measured.private["record_policy"], "max_bytes")?,
                },
            };
            orch_db::queries::uploads::lock_blobs(conn, &[measured.measured.plan_blob]).await?;
            let facts = facts(
                conn,
                environment,
                &names.keys().copied().collect::<Vec<_>>(),
            )
            .await?;
            Ok((
                Prepared {
                    frozen: Frozen {
                        shown,
                        recipe,
                        plan_blob: measured.measured.plan_blob,
                        review: Some(measured.input.review.clone()),
                    },
                    size: measured.size,
                    until,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        value: &'a Prepared,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            actor,
            value.frozen.recipe.environment,
            Permission::FilesManage,
        ))
    }
}

impl Initial for ReviewPlan {
    type Input = Input;
    type Fetched = Measured;
    fn declaration(_: &Input) -> &'static Declaration<Role> {
        &declarations::RESTORE_PREFLIGHT
    }
    fn authorize<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        caller: &'a Caller,
        (): (),
        body: Self::Body,
    ) -> BoxFuture<'a, ApiResult<(Input, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let environment = ctx.id("id")?;
            let facts = source_scope::environment_facts(conn, environment)
                .await
                .map_err(ApiError::engine)?;
            let plan = native::bind_required(ctx, &declarations::RESTORE_PREFLIGHT, &facts)?;
            let admitted = plan
                .shared()
                .admit(caller, false)
                .map_err(crate::key_operations::admission::binding_error)?;
            let actor = native::preparation_principal(ctx, conn, &admitted, &plan).await?;
            require(conn, &actor, environment, Permission::FilesManage).await?;
            let door = super::super::door::open_with_principal(ctx, conn, environment, None, actor)
                .await?;
            door.may_write()?;
            let machine = super::super::live::require_live(ctx, conn, &door.env).await?;
            let snapshot = storage::snapshot(conn, body.snapshot_id)
                .await?
                .filter(|row| row.environment_id == environment)
                .ok_or_else(ApiError::not_found)?;
            if snapshot.storage != "environment" {
                return Err(unavailable("not_recorded"));
            }
            let record_days = get::<i64>(
                conn,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_DAYS,
            )
            .await
            .map_err(ApiError::internal)?
            .unwrap_or(0);
            let max_bytes = get::<i64>(
                conn,
                Place::Environment(environment),
                keys::WORKSPACE_RECORD_MAX_BYTES,
            )
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| unavailable("off"))?;
            let applied = options(conn, environment, body.options.as_ref()).await?;
            let turn: Option<Id> = sqlx::query_scalar(
                "SELECT id FROM turns WHERE before_snapshot_id=$1 ORDER BY ordinal DESC LIMIT 1",
            )
            .bind(snapshot.id)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
            let target = match turn {
                Some(id) => threads::turn(conn, id).await?,
                None => None,
            };
            let selected_undo = snapshot.kind == "restore"
                && snapshot.selection["restore_form"].as_str() == Some("touched");
            let reason = if record_days <= 0 && !selected_undo {
                Some("off".to_owned())
            } else if snapshot.state != "complete" {
                Some(
                    snapshot
                        .unavailable_reason
                        .clone()
                        .unwrap_or_else(|| "incomplete".into()),
                )
            } else if snapshot
                .expires_at
                .is_none_or(|until| until <= ctx.app.clock.now())
            {
                Some("expired".into())
            } else {
                None
            };
            let reason = reason.map(|reason| {
                if reason != "live_tree" {
                    return reason;
                }
                match door.env.facts["index_reason"].as_str() {
                    Some("entries") => door.env.facts["index_entry_limit"]
                        .as_u64()
                        .map_or_else(|| "entries".into(), |limit| format!("entries:{limit}")),
                    Some("chosen") => "chosen".into(),
                    Some("watches") => "watches".into(),
                    Some("scan_time") => "scan_time".into(),
                    _ => reason,
                }
            });
            let anchor = if let Some(reason) = reason {
                let target = target.as_ref().ok_or_else(|| unavailable("turn_unknown"))?;
                let mut touched = orch_engine::jobs::restore::touched::load(
                    conn,
                    environment,
                    target.thread_id,
                    target.id,
                    snapshot
                        .directory
                        .as_deref()
                        .ok_or_else(|| unavailable("directory_unknown"))?,
                    reason,
                )
                .await
                .map_err(ApiError::engine)?;
                if body
                    .overwrite
                    .iter()
                    .any(|path| !touched.paths.iter().any(|selected| selected.path == *path))
                {
                    return Err(ApiError::new(
                        ErrorCode::WorkspaceRestoreConflict,
                        "overwrite names a path outside this touched plan",
                    ));
                }
                touched.overwrite = body.overwrite.clone();
                Anchor::new_touched(
                    &door.env,
                    &snapshot,
                    machine,
                    touched,
                    record_days,
                    max_bytes,
                    door.principal.now(),
                )
                .map_err(ApiError::engine)?
            } else {
                Anchor::new(&door.env, &snapshot, machine, turn, record_days, max_bytes)
                    .map_err(ApiError::engine)?
            };
            let review =
                Review::capture(&admitted, session(ctx)?, anchor).map_err(ApiError::engine)?;
            Ok((
                Input {
                    review,
                    target,
                    applied,
                    overwrite: body.overwrite,
                },
                facts,
            ))
        })
    }
    fn require_initial<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        input: &'a Input,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(require(
            conn,
            actor,
            input.review.anchor().environment(),
            Permission::FilesManage,
        ))
    }
    fn fetch<'a>(
        ctx: &'a Ctx,
        accepted: &'a Caller,
        input: Input,
    ) -> BoxFuture<'a, ApiResult<Measured>> {
        Box::pin(async move {
            let mut input = input;
            let mut measured = loop {
                let anchor = input.review.anchor();
                let command = Outgoing::new(
                    new_id(),
                    anchor.machine(),
                    SerialKey::Environment(anchor.environment()),
                    anchor.command(),
                )
                .map_err(ApiError::internal)?;
                let authority = RestoreCommand::measurement(input.review.clone(), &command)
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
                        authority: Some(CommandAuthority::workspace_restore(authority)),
                    })
                    .await
                    .map_err(|error| {
                        super::super::live::command_failure(crate::dispatch::RunError::Dispatch(
                            error,
                        ))
                        .problem(super::super::problems::Doing::Reading)
                    })?;
                let measurement = dispatcher
                    .wait_for::<FsRestorePlan>(command.id)
                    .await
                    .map_err(|failure| {
                        let failure = super::super::live::command_failure(failure);
                        match failure.code() {
                            Some("changed" | "conflict") => ApiError::new(
                                ErrorCode::WorkspaceRestoreConflict,
                                "the recorded coverage or current files changed; make a new plan",
                            ),
                            Some("unsupported" | "too_large" | "not_found" | "ignored") => {
                                unavailable("store_unavailable")
                            }
                            _ => super::super::live::refused(
                                failure,
                                super::super::problems::Doing::Reading,
                            ),
                        }
                    });
                match measurement {
                    Ok(measured) => break measured,
                    Err(original)
                        if original.code() == ErrorCode::WorkspaceRestoreUnavailable
                            && input.review.anchor().command().touched.is_none()
                            && input.target.is_some() =>
                    {
                        let mut tx = ctx.app.db.begin().await?;
                        let frame = input
                            .review
                            .prepare(&mut tx, ctx.app.clock.as_ref(), None, Some(accepted))
                            .await
                            .map_err(error)?;
                        let native = frame.value();
                        let anchor = input.review.anchor();
                        let target = input
                            .target
                            .as_ref()
                            .ok_or_else(|| unavailable("turn_unknown"))?;
                        let mut touched = orch_engine::jobs::restore::touched::load(
                            &mut tx,
                            anchor.environment(),
                            target.thread_id,
                            target.id,
                            &anchor.command().directory,
                            "store_unavailable".into(),
                        )
                        .await
                        .map_err(ApiError::engine)?;
                        if input.overwrite.iter().any(|path| {
                            !touched.paths.iter().any(|selected| selected.path == *path)
                        }) {
                            return Err(ApiError::new(
                                ErrorCode::WorkspaceRestoreConflict,
                                "overwrite names a path outside this touched plan",
                            ));
                        }
                        touched.overwrite = input.overwrite.clone();
                        let next = Anchor::new_touched(
                            &native.environment,
                            &native.snapshot,
                            anchor.machine(),
                            touched,
                            anchor.command().record_days.unwrap_or(0),
                            anchor
                                .command()
                                .max_bytes
                                .ok_or_else(|| unavailable("off"))?,
                            ctx.app.clock.time(),
                        )
                        .map_err(ApiError::engine)?;
                        tx.commit().await.map_err(DbError::from)?;
                        input.review = Review::capture(accepted, session(ctx)?, next)
                            .map_err(ApiError::engine)?;
                        continue;
                    }
                    Err(original) => {
                        let sparse = authorization::problem_without_content(original.clone());
                        return Err(native::refusal::project::<FailureProjection>(
                            ctx,
                            accepted,
                            Failed {
                                review: input.review.clone(),
                                original,
                            },
                            sparse,
                        )
                        .await?);
                    }
                }
            };
            let anchor = input.review.anchor();
            // The supplier owns no connection across native upload or content-store IO.
            let bytes = ctx
                .app
                .blobs
                .read_whole(
                    &measured.plan_blob,
                    i64::try_from(orch_engine::manifest::MANIFEST_MAX_EXPANDED_BYTES)
                        .map_err(ApiError::internal)?,
                )
                .await
                .map_err(ApiError::internal)?;
            if Hash::of(&bytes) != measured.plan_blob {
                return Err(unavailable("store_changed"));
            }
            let private: Json = serde_json::from_slice(&bytes).map_err(ApiError::internal)?;
            let mut conn = ctx.app.db.acquire().await?;
            let current = anchor
                .metadata(&mut conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            if let Some(touched) = &anchor.command().touched {
                let expected = Hash::of(&serde_json::to_vec(touched).map_err(ApiError::internal)?);
                if decode::<Hash>(&private, "touched_fingerprint")? != expected {
                    return Err(unavailable("store_changed"));
                }
            }
            if (anchor.command().touched.is_none()
                && Some(decode::<Hash>(&private, "target_manifest")?)
                    != current.snapshot.manifest_blob)
                || decode::<Id>(&private, "snapshot_id")? != anchor.snapshot()
                || decode::<String>(&private, "directory")? != anchor.command().directory
                || decode::<i64>(&private, "generation")?
                    != i64::from(current.environment.generation)
            {
                return Err(unavailable("store_changed"));
            }
            let ids =
                writer_ids(&mut conn, anchor.environment(), &anchor.command().directory).await?;
            drop(conn);
            if anchor.command().touched.is_none() {
                later(
                    &mut measured.paths,
                    &private["current"],
                    input
                        .target
                        .as_ref()
                        .and_then(|turn| turn.files_end_manifest.as_ref()),
                );
            }
            let mut candidates = Vec::new();
            for id in ids {
                if observed_writer(ctx, &input.review, id).await? {
                    candidates.push(RestoreWriter {
                        thread: Some(NodeRef {
                            thread_display: None,
                            id,
                            kind: NodeKind::Thread,
                            name: None,
                            redacted: true,
                        }),
                        active: false,
                    });
                }
            }
            known_writers(
                ctx,
                &input.review,
                anchor.environment(),
                &anchor.command().directory,
                input.target.as_ref(),
                &candidates,
                &mut measured.paths,
            )
            .await?;
            Ok(Measured {
                input,
                measured,
                private,
                size: i64::try_from(bytes.len()).map_err(ApiError::internal)?,
            })
        })
    }
}

impl effects::Apply for ReviewPlan {
    fn apply<'a>(
        ctx: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        value: &'a Prepared,
        before: effects::FirstEffect<'a>,
    ) -> BoxFuture<'a, ApiResult<effects::Outcome<Saved>>> {
        Box::pin(async move {
            let review = value
                .frozen
                .review
                .as_ref()
                .ok_or_else(|| ApiError::internal("the original restore review is missing"))?;
            let frame = review
                .prepare(conn, ctx.app.clock.as_ref(), None, Some(actor.caller()))
                .await
                .map_err(error)?;
            let deadline = before.deadline();
            let (reviewer, current) = frame
                .into_parts()
                .check(ctx.app.clock.as_ref())
                .map_err(error)?;
            before.check()?;
            let now = ctx.app.clock.now();
            storage::register_blob(conn, &value.frozen.plan_blob, value.size, now).await?;
            storage::pin_blobs(conn, &[value.frozen.plan_blob]).await?;
            jobs::forks::save_preflight(
                conn,
                &jobs::forks::PreflightRow {
                    id: value.frozen.shown.preflight_id,
                    kind: jobs::kind::WORKSPACE_RESTORE.to_owned(),
                    node_id: current.snapshot.thread_id,
                    environment_id: Some(current.environment.id),
                    created_by: reviewer.user(),
                    plan: serde_json::to_value(&value.frozen).map_err(ApiError::internal)?,
                    expires_at: value.frozen.shown.expires_at.get(),
                    created_at: now,
                },
            )
            .await?;
            deadline.check()?;
            Ok(effects::Outcome::Applied(Saved {
                frozen: value.frozen.clone(),
                until: value.until,
            }))
        })
    }
}

fn receipt(id: Id) -> RestorePreflightResponse {
    RestorePreflightResponse::Receipt(RestorePreflightReceipt {
        projection: ReceiptProjection::Receipt,
        preflight_id: id,
    })
}
struct Projection;
impl reply::Projection for Projection {
    type Source = Role;
    type Input = Saved;
    type Subject = source_scope::Native;
    type Value = ();
    type Reply = RestorePlan;
    const READ_SOURCE: Role = Role::Content;
    fn declaration(_: &Saved) -> &'static Declaration<Role> {
        &declarations::RESTORE_PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Saved,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let review = input
                .frozen
                .review
                .as_ref()
                .ok_or_else(ApiError::not_found)?;
            let source = review
                .anchor()
                .metadata(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            let names = input
                .frozen
                .shown
                .writers
                .iter()
                .filter_map(|writer| writer.thread.as_ref().map(|thread| thread.id))
                .collect::<Vec<_>>();
            let facts = facts(conn, source.environment.id, &names).await?;
            Ok((source, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        input: &'a Saved,
        source: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require(conn, actor, source.environment.id, Permission::FilesView).await {
                Ok(()) => {}
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
                    require(conn, actor, source.environment.id, Permission::FilesManage).await?
                }
                Err(error) => return Err(error),
            }
            for thread in input
                .frozen
                .shown
                .writers
                .iter()
                .filter_map(|writer| writer.thread.as_ref())
            {
                authorization::require(
                    conn,
                    actor,
                    thread.id,
                    authorization::VIEW,
                    Act::Use,
                    Policy::default(),
                )
                .await?;
            }
            Ok(())
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Saved,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Role>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<RestorePlan>> {
        Box::pin(async move { Ok(input.frozen.shown.clone()) })
    }
    fn check_opened<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Saved,
        old: &'a reply::Prepared<Self>,
        fresh: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<bool>> {
        Box::pin(async move {
            let anchor = input
                .frozen
                .review
                .as_ref()
                .ok_or_else(ApiError::not_found)?
                .anchor();
            Ok(old.subject.snapshot.id == anchor.snapshot()
                && fresh.subject.snapshot.id == anchor.snapshot()
                && old.subject.environment.generation == fresh.subject.environment.generation
                && old.subject.environment.machine_id == fresh.subject.environment.machine_id
                && old.subject.environment.workspace_root
                    == fresh.subject.environment.workspace_root)
        })
    }
}

pub(super) async fn run(
    ctx: Ctx,
    (): (),
    _: EnvironmentRestorePreflightBody,
) -> ApiResult<RestorePreflightResponse> {
    let mut tx = ctx.app.db.begin().await?;
    let completed = effects::apply::<ReviewPlan>(&ctx, &mut tx).await?;
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
            "the restore review credential has expired",
        ));
    }
    if completed.output.until <= ctx.app.clock.time() {
        return Err(unavailable("expired"));
    }
    tx.commit().await.map_err(DbError::from)?;
    let fallback = receipt(completed.output.frozen.shown.preflight_id);
    crate::key_operations::publication::receipt(&completed.ctx, &fallback)?;
    Ok(
        match reply::render::<Projection>(&completed.ctx, &completed.accepted, completed.output)
            .await?
        {
            Some(plan) => RestorePreflightResponse::Full(Box::new(plan)),
            None => fallback,
        },
    )
}

struct Failed {
    review: Review,
    original: ApiError,
}
struct FailureProjection;
impl reply::Projection for FailureProjection {
    type Source = Role;
    type Input = Failed;
    type Subject = source_scope::Native;
    type Value = ();
    type Reply = ApiError;
    const READ_SOURCE: Role = Role::Content;
    fn declaration(_: &Failed) -> &'static Declaration<Role> {
        &declarations::RESTORE_PREFLIGHT
    }
    fn subject<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        input: &'a Failed,
    ) -> BoxFuture<'a, ApiResult<(Self::Subject, Vec<native::Facts<Role>>)>> {
        Box::pin(async move {
            let source = input
                .review
                .anchor()
                .metadata(conn)
                .await
                .map_err(ApiError::engine)?
                .ok_or_else(ApiError::not_found)?;
            let facts = facts(conn, source.environment.id, &[]).await?;
            Ok((source, facts))
        })
    }
    fn require<'a>(
        _: &'a Ctx,
        conn: &'a mut Conn,
        actor: &'a Principal,
        _: &'a Failed,
        source: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<()>> {
        Box::pin(async move {
            match require(conn, actor, source.environment.id, Permission::FilesView).await {
                Ok(()) => Ok(()),
                Err(error) if matches!(error.problem.status, 403 | 404 | 410) => {
                    require(conn, actor, source.environment.id, Permission::FilesManage).await
                }
                Err(error) => Err(error),
            }
        })
    }
    fn prepare<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Failed,
        _: &'a Self::Subject,
    ) -> BoxFuture<'a, ApiResult<((), Vec<native::Facts<Role>>)>> {
        Box::pin(async { Ok(((), Vec::new())) })
    }
    fn render<'a>(
        _: &'a Ctx,
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a Failed,
        _: &'a reply::Prepared<Self>,
    ) -> BoxFuture<'a, ApiResult<ApiError>> {
        Box::pin(async move { Ok(input.original.clone()) })
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
