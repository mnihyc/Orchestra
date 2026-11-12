//! One reviewed batch independently moves, re-creates or switches the selected environments.

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::environment::EnvironmentBatch;

use orch_api_contract::environment::EnvironmentBatchJob;

use orch_api_contract::environment::EnvironmentMovePreflightBody;

use orch_api_contract::forks::ForkBatchPreflightBody;

use orch_api_contract::forks::ForkCreateBody;

use orch_api_contract::forks::PlanBlocker;

use orch_api_contract::loss as api;

use orch_db::DbError;

use orch_db::queries::jobs;

use orch_engine::fork_plan::StoredBatch;

use orch_engine::jobs::environment_move::MoveRecipe;

use orch_engine::recovery;

use orch_engine::updates;

use orch_engine::updates::UpdateError;

use orch_types::Id;

use orch_types::Time;

use orch_types::new_id;

use serde::Deserialize;

use serde::Serialize;
/// Binds independent recovery planning and execution.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(api::EnvironmentRecoverCreate, confirmation::binding())
        .on_with_initial_operation(
            api::EnvironmentRecoverPreflight,
            crate::key_operations::native::effects::initial::<operation::Preview>,
            super::lifecycle::prepare::<operation::Preview>,
            operation::preflight,
        )
        .on_with_initial_operation(
            api::EnvironmentRecoverCreate,
            crate::key_operations::native::effects::initial::<confirmation::Confirm>,
            super::lifecycle::prepare::<confirmation::Confirm>,
            confirmation::create,
        )
        .on_snapshot_read_with_optional_operation(
            api::EnvironmentRecoverGet,
            crate::key_operations::native::selected::<read_sources::Recovered>,
            crate::key_operations::native::selected_read::<read_sources::Recovered>,
        );
}

#[derive(Clone, Serialize, Deserialize)]
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

#[derive(Clone, Serialize, Deserialize)]
struct Stored {
    notification: api::OperationNotificationPlan,
    admin_grants: Vec<api::OperationAdminGrant>,
    affected: Vec<Id>,
    body: api::EnvironmentRecoverPreflightBody,
    members: Vec<Member>,
    switches: Option<StoredBatch>,
}

/// The same native affected set that supplied this saved recovery audience.
pub(crate) fn recipient_subjects(
    plan: &orch_types::Json,
) -> ApiResult<Vec<orch_access_contract::decisions::ObjectRef>> {
    let stored: Stored = serde_json::from_value(plan.clone()).map_err(ApiError::internal)?;
    Ok(stored
        .affected
        .into_iter()
        .map(|id| orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id,
        })
        .collect())
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
        UpdateError::Internal(error @ orch_engine::EngineError::BeforeEffect(_)) => {
            Err(super::lifecycle::error(error))
        }
        other => Err(super::updates::refused(other)),
    }
}

async fn observe_preflight(
    ctx: Ctx,
    caller: &orch_authz::Caller,
    scope: &orch_engine::jobs::environment_move::preparation::Scope,
    body: api::EnvironmentRecoverPreflightBody,
) -> ApiResult<operation::Observed> {
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
    let principal =
        authorization::load_principal(&mut conn, caller.clone(), ctx.app.clock.time()).await?;
    let caller = principal.caller().clone();
    drop(conn);
    let id = new_id();
    let mut rows = Vec::new();
    let mut others = 0;
    let mut admin_grants = Vec::new();
    let mut members = Vec::new();
    let mut switches: Option<StoredBatch> = None;
    let mut content = Vec::new();
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
                            content.extend(draft.recipe.threads.iter().map(|id| {
                                (orch_access_contract::permissions::ObjectKind::Thread, *id)
                            }));
                            content.extend(draft.mounts.iter().map(|m| {
                                (
                                    orch_access_contract::permissions::ObjectKind::Mount,
                                    m.mount.id,
                                )
                            }));
                            content.extend(draft.mcp.iter().filter(|s| !s.own && !s.imported).map(
                                |s| {
                                    (
                                        orch_access_contract::permissions::ObjectKind::McpServer,
                                        s.id,
                                    )
                                },
                            ));
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
                            content.extend(draft.recipe.replacement.threads.iter().map(|id| {
                                (orch_access_contract::permissions::ObjectKind::Thread, *id)
                            }));
                            content.extend(draft.mounts.iter().map(|m| {
                                (
                                    orch_access_contract::permissions::ObjectKind::Mount,
                                    m.mount.id,
                                )
                            }));
                            content.extend(draft.mcp.iter().filter(|s| !s.own && !s.imported).map(
                                |s| {
                                    (
                                        orch_access_contract::permissions::ObjectKind::McpServer,
                                        s.id,
                                    )
                                },
                            ));
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
                            content.extend(draft.disclosure.iter().map(|o| (o.kind, o.id)));
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
    let current = if scope.origin().await.environments().next().is_some() {
        scope
            .current(&ctx.app.engine)
            .await
            .map_err(super::lifecycle::error)?
    } else {
        caller.clone()
    };
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::load_principal(&mut conn, current, ctx.app.clock.time()).await?;
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
    let named = content
        .iter()
        .filter_map(|(kind, id)| {
            (*kind == orch_access_contract::permissions::ObjectKind::Thread).then_some(*id)
        })
        .collect::<Vec<_>>();
    let visible = super::lifecycle::visible_threads(&mut conn, &principal, &named).await?;
    Ok(operation::Observed {
        origin: scope.origin().await,
        visible,
        content,
        stored: Stored {
            notification: notification.clone(),
            admin_grants: admin_grants.clone(),
            affected,
            body,
            members,
            switches,
        },
        plan: api::EnvironmentRecoveryPlan {
            others,
            admin_grants,
            notification,
            preflight_id: id,
            expires_at: Time::new(updates::expires(ctx.now)),
            rows,
            executable,
        },
        audience,
    })
}

fn stale() -> ApiError {
    ApiError::new(
        orch_api_contract::common::ErrorCode::ConflictStaleRevision,
        "The source or destination changed; review recovery again",
    )
}

#[path = "recovery/confirmation.rs"]
pub mod confirmation;

#[path = "recovery/operation.rs"]
pub mod operation;

#[path = "recovery/read_sources.rs"]
pub mod read_sources;
