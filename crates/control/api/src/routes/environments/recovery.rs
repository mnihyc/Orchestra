//! One reviewed batch independently moves, re-creates or switches the selected environments.

use crate::{
    middleware::{
        authorization,
        requests::{ApiError, ApiResult},
    },
    registry::{Ctx, Registry},
};
use orch_api_contract::{
    environment::{EnvironmentBatch, EnvironmentBatchJob, EnvironmentMovePreflightBody},
    forks::{ForkBatchPreflightBody, ForkCreateBody, PlanBlocker},
    loss as api,
};
use orch_db::{DbError, queries::jobs};
use orch_engine::{
    fork_plan::StoredBatch,
    jobs::environment_move::MoveRecipe,
    recovery,
    updates::{self, UpdateError},
};
use orch_types::{Id, Time, new_id};
use serde::{Deserialize, Serialize};

/// Binds independent recovery planning and execution.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentRecoverPreflight, preflight)
        .on(api::EnvironmentRecoverCreate, create)
        .on(api::EnvironmentRecoverGet, batch_get);
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
enum Member {
    Recreate {
        recipe: Box<recovery::Recipe>,
    },
    Move {
        recipe: Box<MoveRecipe>,
        request: EnvironmentMovePreflightBody,
    },
    Portable {
        recipe: Box<orch_engine::jobs::machine_move::Recipe>,
        request: EnvironmentMovePreflightBody,
    },
}
#[derive(Serialize, Deserialize)]
struct Stored {
    notification: api::OperationNotificationPlan,
    admin_grants: Vec<api::OperationAdminGrant>,
    affected: Vec<Id>,
    body: api::EnvironmentRecoverPreflightBody,
    members: Vec<Member>,
    switches: Option<StoredBatch>,
}

async fn blockers(error: UpdateError) -> ApiResult<Vec<PlanBlocker>> {
    match error {
        UpdateError::Blocked(blockers) => Ok(blockers),
        UpdateError::Denied => Ok(vec![updates::blocker(
            "access.denied",
            "Environment management is required",
        )]),
        UpdateError::Held(_) => Ok(vec![updates::blocker(
            "environment_busy",
            "Another operation holds this environment",
        )]),
        other => Err(super::updates::refused(other)),
    }
}

async fn preflight(
    ctx: Ctx,
    (): (),
    body: api::EnvironmentRecoverPreflightBody,
) -> ApiResult<api::EnvironmentRecoveryPlan> {
    if body.environment_ids.is_empty() {
        return Err(crate::routes::nodes::invalid(
            "environment_ids",
            "select at least one environment",
        ));
    }
    let mut ids = body.environment_ids.clone();
    ids.sort_unstable();
    ids.dedup();
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let caller = principal.caller().clone();
    drop(conn);
    let id = new_id();
    let mut rows = Vec::new();
    let mut others = 0;
    let mut admin_grants = Vec::new();
    let mut members = Vec::new();
    let mut switches: Option<StoredBatch> = None;
    let mut destination = body.environment.clone();
    for environment in ids {
        let mut conn = ctx.app.db.begin().await?;
        let source = orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id: environment,
        };
        match orch_engine::operation_access::review(
            &mut conn,
            &principal,
            source,
            orch_access_contract::permissions::Permission::EnvironmentManage,
        )
        .await
        {
            Ok(_) => {}
            Err(UpdateError::Denied) => {
                others += 1;
                continue;
            }
            Err(error) => return Err(super::updates::refused(error)),
        }
        let shown = super::show(&ctx, &mut conn, &principal, environment).await?;
        let choice = body
            .choices
            .iter()
            .find(|choice| choice.environment_id == environment);
        let lost = shown.base.status == orch_api_contract::environment::EnvironmentStatus::Lost;
        let action = choice.and_then(|choice| choice.action).unwrap_or(if lost {
            api::RecoveryAction::Recreate
        } else {
            api::RecoveryAction::Move
        });
        let mut row = api::EnvironmentRecoveryRow {
            environment: shown.base,
            action,
            host: None,
            files: None,
            listing_available: true,
            restored: false,
            restored_bytes: None,
            move_plan: None,
            switch_plan: None,
            blockers: Vec::new(),
        };
        let host = choice.and_then(|choice| choice.host_id).or(body.host_id);
        // These page additions are rolled back with this preview transaction; only selected grants are shown.
        let prospective = orch_engine::operation_access::preview_recovery(
            &mut conn,
            &principal,
            environment,
            host,
        )
        .await
        .map_err(super::updates::refused)?;
        match action {
            api::RecoveryAction::Recreate => {
                let prepared = ctx
                    .app
                    .engine
                    .prepare_recovery(&mut conn, &principal, environment, host)
                    .await;
                conn.rollback().await.map_err(DbError::from)?;
                let measured = match prepared {
                    Ok(prepared) => ctx.app.engine.measure_recovery(prepared).await,
                    Err(error) => Err(error),
                };
                match measured {
                    Ok(draft) => {
                        row.host = Some(draft.host);
                        row.files = draft.recipe.listing_available.then_some(draft.files);
                        row.listing_available = draft.recipe.listing_available;
                        row.restored = !draft.recipe.restored.is_empty();
                        row.restored_bytes = draft
                            .recipe
                            .restored
                            .values()
                            .try_fold(0_i64, |sum, value| {
                                value.map(|bytes| sum.saturating_add(bytes))
                            });
                        members.push(Member::Recreate {
                            recipe: Box::new(draft.recipe),
                        });
                    }
                    Err(error) => row.blockers = blockers(error).await?,
                }
            }
            api::RecoveryAction::Move => {
                let request = EnvironmentMovePreflightBody {
                    host_id: host,
                    machine_id: None,
                    template_id: None,
                    copy_directories: None,
                    home: None,
                    private_views: None,
                    files: None,
                    check_fingerprints: None,
                    fingerprint_request_id: None,
                    mounts: None,
                };
                if orch_engine::jobs::machine_move::needed(&mut conn, environment, &request)
                    .await
                    .map_err(ApiError::internal)?
                {
                    drop(conn);
                    match ctx
                        .app
                        .engine
                        .draft_machine_move(&principal, environment, &request, None)
                        .await
                    {
                        Ok(draft) => {
                            let mut conn = ctx.app.db.acquire().await?;
                            let plan = super::moves::shown_machine(
                                &ctx, &mut conn, &principal, id, &draft,
                            )
                            .await?;
                            row.blockers = plan.blockers.clone();
                            row.move_plan = Some(plan);
                            if row.blockers.is_empty() {
                                members.push(Member::Portable {
                                    recipe: Box::new(draft.recipe),
                                    request,
                                });
                            }
                        }
                        Err(error) => row.blockers = blockers(error).await?,
                    }
                } else {
                    match ctx
                        .app
                        .engine
                        .draft_move(&mut conn, &principal, environment, &request, None)
                        .await
                    {
                        Ok(draft) => {
                            let plan = super::moves::shown(&ctx, &mut conn, &principal, id, &draft)
                                .await?;
                            row.blockers = plan.blockers.clone();
                            row.move_plan = Some(plan);
                            if row.blockers.is_empty() {
                                members.push(Member::Move {
                                    recipe: Box::new(draft.recipe),
                                    request,
                                });
                            }
                        }
                        Err(error) => row.blockers = blockers(error).await?,
                    }
                }
            }
            api::RecoveryAction::SwitchThreads => {
                drop(conn);
                if destination.is_none() {
                    row.blockers.push(updates::blocker(
                        "environment_required",
                        "Choose a destination environment",
                    ));
                } else {
                    let request = ForkBatchPreflightBody {
                        thread_ids: None,
                        all_of_environment: Some(environment),
                        all_of_provider: None,
                        all_of_agent: None,
                        source_model_id: None,
                        to: None,
                        environment: destination.clone(),
                        options: None,
                    };
                    match ctx.app.engine.draft_batch(&caller, &request).await {
                        Ok(draft) => {
                            if switches.is_none() {
                                destination = draft
                                    .members
                                    .first()
                                    .and_then(|member| member.request.environment.clone())
                                    .or(destination);
                            }
                            row.blockers = if draft.plan.executable == 0 {
                                draft
                                    .plan
                                    .plans
                                    .iter()
                                    .flat_map(|plan| plan.blockers.clone())
                                    .collect()
                            } else {
                                Vec::new()
                            };
                            if draft.plan.executable == 0 && row.blockers.is_empty() {
                                row.blockers.push(updates::blocker(
                                    "no_threads",
                                    "No threads can be switched",
                                ));
                            }
                            row.switch_plan = Some(draft.plan.clone());
                            if let Some(all) = switches.as_mut() {
                                all.members.extend(draft.members);
                                all.plan.plans.extend(draft.plan.plans);
                                all.plan.executable += draft.plan.executable;
                                all.plan.skipped.extend(draft.plan.skipped);
                            } else {
                                let mut request = request;
                                request.environment = destination.clone();
                                switches = Some(StoredBatch {
                                    body: request,
                                    plan: draft.plan,
                                    members: draft.members,
                                });
                            }
                        }
                        Err(error) => row
                            .blockers
                            .push(updates::blocker("switch_unavailable", error.to_string())),
                    }
                }
            }
        }
        if row.blockers.is_empty() {
            let mut needed = vec![(
                source,
                orch_access_contract::permissions::Permission::EnvironmentManage,
            )];
            if let Some(member) = members.last() {
                use orch_access_contract::{
                    decisions::ObjectRef,
                    permissions::{ObjectKind, Permission},
                };
                match member {
                    Member::Recreate { recipe } if recipe.environment == environment => needed
                        .extend([
                            (
                                ObjectRef {
                                    kind: ObjectKind::Host,
                                    id: recipe.host,
                                },
                                Permission::HostUse,
                            ),
                            (
                                ObjectRef {
                                    kind: ObjectKind::Template,
                                    id: recipe.template,
                                },
                                Permission::TemplateUse,
                            ),
                        ]),
                    Member::Move { recipe, .. }
                        if recipe.replacement.target.id() == environment =>
                    {
                        needed.extend([
                            (
                                ObjectRef {
                                    kind: ObjectKind::Host,
                                    id: recipe.destination,
                                },
                                Permission::HostUse,
                            ),
                            (
                                ObjectRef {
                                    kind: ObjectKind::Template,
                                    id: recipe.replacement.template,
                                },
                                Permission::TemplateUse,
                            ),
                        ])
                    }
                    Member::Portable { recipe, .. } if recipe.environment == environment => {
                        needed.extend(recipe.destination_access())
                    }
                    _ => {}
                }
            }
            for grant in prospective {
                if needed.contains(&(grant.object, grant.permission))
                    && !admin_grants.contains(&grant)
                {
                    admin_grants.push(grant);
                }
            }
        }
        rows.push(row);
    }
    let affected = rows
        .iter()
        .filter(|row| row.blockers.is_empty())
        .map(|row| row.environment.id)
        .collect::<Vec<_>>();
    let (ctx, mut conn, principal) = crate::routes::nodes::action(&ctx).await?;
    for row in rows.iter().filter(|row| row.blockers.is_empty()) {
        orch_engine::operation_access::review(
            &mut conn,
            &principal,
            orch_access_contract::decisions::ObjectRef {
                kind: orch_access_contract::permissions::ObjectKind::Environment,
                id: row.environment.id,
            },
            orch_access_contract::permissions::Permission::EnvironmentManage,
        )
        .await
        .map_err(super::updates::refused)?;
    }
    for grant in &admin_grants {
        orch_engine::operation_access::review(
            &mut conn,
            &principal,
            grant.object,
            grant.permission,
        )
        .await
        .map_err(super::updates::refused)?;
    }
    let audience =
        orch_engine::notifications::operation_audience(&mut conn, &principal, &affected, &[])
            .await
            .map_err(ApiError::internal)?;
    let notification = api::OperationNotificationPlan {
        preview_id: Some(id),
        people: audience.len() as i64,
        default_on: orch_engine::notifications::operation_default(
            &mut conn,
            match rows.as_slice() {
                [row] => orch_settings::Place::Environment(row.environment.id),
                _ => orch_settings::Place::System,
            },
            rows.iter()
                .any(|row| row.blockers.is_empty() && row.action != api::RecoveryAction::Recreate),
        )
        .await
        .map_err(ApiError::internal)?,
    };
    let executable = affected.len() as i64;
    jobs::forks::save_preflight(
        &mut conn,
        &jobs::forks::PreflightRow {
            id,
            kind: "recover_batch".into(),
            node_id: None,
            environment_id: None,
            created_by: principal.user(),
            plan: serde_json::to_value(Stored {
                notification: notification.clone(),
                admin_grants: admin_grants.clone(),
                affected,
                body,
                members,
                switches,
            })
            .map_err(ApiError::internal)?,
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    orch_db::queries::notifications::operations::preview(
        &mut conn,
        id,
        &audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    conn.commit().await.map_err(DbError::from)?;
    Ok(api::EnvironmentRecoveryPlan {
        others,
        admin_grants,
        notification,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        rows,
        executable,
    })
}

enum Ready {
    Recreate(Box<recovery::Recipe>),
    Move(Box<orch_engine::jobs::environment_move::MoveDraft>),
    Portable(Box<orch_engine::jobs::machine_move::Draft>),
}

async fn create(ctx: Ctx, (): (), body: ForkCreateBody) -> ApiResult<EnvironmentBatch> {
    let (ctx, mut tx, principal) = crate::routes::nodes::action(&ctx).await?;
    let original = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .filter(|row| row.kind == "recover_batch" && row.created_by == principal.user())
        .ok_or_else(ApiError::not_found)?;
    if original.expires_at <= ctx.now {
        return Err(super::updates::refused(UpdateError::Expired));
    }
    let stored: Stored =
        serde_json::from_value(original.plan.clone()).map_err(ApiError::internal)?;
    tx.rollback().await.map_err(DbError::from)?;
    let batch = new_id();
    let mut ready = Vec::new();
    for member in stored.members {
        let environment = match &member {
            Member::Recreate { recipe } => recipe.environment,
            Member::Move { recipe, .. } => recipe.replacement.target.id(),
            Member::Portable { recipe, .. } => recipe.environment,
        };
        let mut tx = ctx.app.db.begin().await?;
        let named = match &member {
            Member::Recreate { recipe } => Some(recipe.host),
            Member::Move { recipe, .. } => Some(recipe.destination),
            Member::Portable { .. } => None,
        };
        orch_engine::operation_access::preview_recovery(&mut tx, &principal, environment, named)
            .await
            .map_err(super::updates::refused)?;
        let drafted = match member {
            Member::Recreate { recipe } => {
                let prepared = ctx
                    .app
                    .engine
                    .prepare_recovery(&mut tx, &principal, environment, Some(recipe.host))
                    .await
                    .map_err(super::updates::refused)?;
                tx.rollback().await.map_err(DbError::from)?;
                let draft = ctx
                    .app
                    .engine
                    .measure_recovery(prepared)
                    .await
                    .map_err(super::updates::refused)?;
                if draft.recipe != *recipe {
                    return Err(stale());
                }
                Ready::Recreate(recipe)
            }
            Member::Move { recipe, request } => {
                let draft = ctx
                    .app
                    .engine
                    .draft_move(
                        &mut tx,
                        &principal,
                        environment,
                        &request,
                        body.options.as_ref(),
                    )
                    .await
                    .map_err(super::updates::refused)?;
                if draft.recipe.replacement.old != recipe.replacement.old
                    || draft.recipe.destination != recipe.destination
                    || draft.recipe.manifest != recipe.manifest
                {
                    return Err(stale());
                }
                Ready::Move(Box::new(draft))
            }
            Member::Portable { recipe, request } => {
                tx.rollback().await.map_err(DbError::from)?;
                let draft = ctx
                    .app
                    .engine
                    .draft_machine_move(&principal, environment, &request, body.options.as_ref())
                    .await
                    .map_err(super::updates::refused)?;
                if !recipe.same_choice(&draft.recipe) {
                    return Err(stale());
                }
                Ready::Portable(Box::new(draft))
            }
        };
        ready.push((environment, drafted));
    }
    // Measurements precede acceptance. From here every hold and queued member commits together.
    let mut tx = ctx.app.db.begin().await?;
    let current = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current.plan != original.plan || current.expires_at <= ctx.now {
        return Err(stale());
    }
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let audience =
        orch_engine::notifications::operation_audience(&mut tx, &principal, &stored.affected, &[])
            .await
            .map_err(ApiError::internal)?;
    if !orch_db::queries::notifications::operations::matches_preview(
        &mut tx,
        body.preflight_id,
        &audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?
    {
        return Err(stale());
    }
    let operation = orch_engine::notifications::NotificationOperation {
        id: batch,
        kind: "recover".into(),
        name: String::new(),
        notify: body.notify.unwrap_or(false),
        path: "/resources/environments".into(),
        subject: None,
        recipients: audience,
    };
    orch_engine::operation_access::accept(
        &mut tx,
        &principal,
        &stored.admin_grants,
        &orch_db::audit::Origin::default(),
    )
    .await
    .map_err(super::updates::refused)?;
    let mut queued = Vec::new();
    for (environment, ready) in ready {
        if !updates::may_manage(
            &mut tx,
            &principal,
            updates::Target::Environment(environment),
        )
        .await
        .map_err(ApiError::internal)?
        {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::AuthzDenied,
                "Environment management is required",
            ));
        }
        let shown = super::show(&ctx, &mut tx, &principal, environment)
            .await?
            .base;
        let row = match ready {
            Ready::Recreate(recipe) => orch_engine::notifications::operation_scope(
                Some(operation.id),
                recovery::queue(&ctx.app.engine, &mut tx, &recipe, Some(batch), ctx.now),
            )
            .await
            .map_err(super::updates::refused)?,
            Ready::Move(draft) => orch_engine::notifications::operation_scope(
                Some(operation.id),
                orch_engine::jobs::environment_move::queue(&mut tx, *draft, Some(batch), ctx.now),
            )
            .await
            .map_err(super::updates::refused)?,
            Ready::Portable(draft) => orch_engine::notifications::operation_scope(
                Some(operation.id),
                orch_engine::jobs::machine_move::queue(
                    &mut tx,
                    &principal,
                    *draft,
                    Some(batch),
                    ctx.now,
                ),
            )
            .await
            .map_err(super::updates::refused)?,
        };
        queued.push(EnvironmentBatchJob {
            environment: shown,
            job: super::jobs::view(&mut tx, &row).await?,
        });
    }
    if let Some(switches) = stored.switches {
        let caller = principal.caller().clone();
        match orch_engine::notifications::operation_scope(
            Some(operation.id),
            ctx.app
                .engine
                .queue_batch_members(&mut tx, &caller, &switches, &body, batch),
        )
        .await
        {
            Ok(jobs) => {
                for job in jobs {
                    let environment = match job.node_id {
                        Some(thread) => orch_db::queries::threads::thread(&mut tx, thread)
                            .await?
                            .and_then(|row| row.environment_id),
                        None => None,
                    }
                    .or(job.environment_id);
                    if let Some(environment) = environment {
                        let environment = super::show(&ctx, &mut tx, &principal, environment)
                            .await?
                            .base;
                        queued.push(EnvironmentBatchJob {
                            environment,
                            job: super::jobs::view(&mut tx, &job).await?,
                        });
                    }
                }
            }
            Err(orch_engine::fork_plan::PlanError::NothingExecutable) => {}
            Err(error) => {
                return Err(ApiError::new(
                    orch_api_contract::common::ErrorCode::PlanBlocked,
                    error.to_string(),
                ));
            }
        }
    }
    if queued.is_empty() {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::PlanBlocked,
            "No recovery member can run",
        ));
    }
    orch_engine::notifications::attach_operation(
        &mut tx,
        &queued.iter().map(|row| row.job.id).collect::<Vec<_>>(),
        &operation,
        ctx.now,
    )
    .await
    .map_err(ApiError::internal)?;
    orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    for environment in &stored.affected {
        let mut entry = orch_db::audit::Entry::new(
            orch_db::audit::Actor::User(principal.user()),
            orch_db::audit::name!("environment.updated"),
            "environment",
            Some(*environment),
            "Requested reviewed recovery",
        );
        entry.environment_id = Some(*environment);
        entry.history = true;
        entry.r#override = !stored.admin_grants.is_empty();
        entry.details = serde_json::json!({"operation":batch,"admin_grants":stored.admin_grants});
        orch_db::audit::append(&mut tx, &entry, &orch_db::audit::Origin::default(), ctx.now)
            .await?;
    }
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(EnvironmentBatch {
        batch_id: batch,
        jobs: queued,
    })
}

fn stale() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "The source or destination changed; review recovery again",
    )
}

async fn batch_get(ctx: Ctx, (): (), (): ()) -> ApiResult<EnvironmentBatch> {
    let batch = ctx.id("batch_id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let rows = jobs::forks::of_batch(&mut conn, batch).await?;
    if rows.is_empty() {
        return Err(ApiError::not_found());
    }
    let mut shown = Vec::new();
    for row in rows {
        let environment = match row.environment_id {
            Some(id) => Some(id),
            None => match row.node_id {
                Some(thread) => orch_db::queries::threads::thread(&mut conn, thread)
                    .await?
                    .and_then(|row| row.environment_id),
                None => None,
            },
        };
        let Some(environment) = environment else {
            continue;
        };
        if let Ok(view) = super::show(&ctx, &mut conn, &principal, environment).await {
            shown.push(EnvironmentBatchJob {
                environment: view.base,
                job: super::jobs::read_for(&mut conn, &principal, row.id).await?,
            });
        }
    }
    if shown.is_empty() {
        return Err(ApiError::not_found());
    }
    Ok(EnvironmentBatch {
        batch_id: batch,
        jobs: shown,
    })
}
