//! Listings of environments and provider runtimes, recycle, and batch moves and updates.
//!
//! A batch plans each visible unit independently and queues only plans without blockers. The jobs
//! already hold their units while queued; the runner limits how many execute at a time. Recycling
//! uses the same eligibility checks in the preview and immediately before an ordinary stop.

use crate::middleware::requests::{ApiError, ApiResult};
use crate::middleware::{authorization, transaction_guard};
use crate::registry::{Ctx, Registry};
use crate::routes::environments::{self, jobs as shown_jobs, updates as plans};
use orch_api_contract::{environment as api, forks::ForkCreateBody};
use orch_authz::Principal;
use orch_db::queries::environments::hosting as envs;
use orch_db::{Conn, DbError};
use orch_engine::updates::{self, Target, UpdateError};
use orch_types::{Id, Time, new_id};

/// Binds fleet recycling and batch update requests.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::NodeEnvironmentRecycle, recycle)
        .on(api::NodeEnvironmentUpdatePreflight, update_preflight)
        .on(api::NodeEnvironmentUpdateCreate, update_create)
        .on(api::EnvironmentUpdateBatchPreflight, update_all_preflight)
        .on(api::EnvironmentUpdateBatchCreate, update_all_create);
    registry
        .on(
            api::NodeEnvironmentMovePreflight,
            environments::moves::batch_preflight,
        )
        .on(
            api::NodeEnvironmentMoveCreate,
            environments::moves::batch_create,
        );
}

/// A candidate in the fleet's common shape, with only thread counts disclosed here.
pub(crate) async fn entry(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    manageable: bool,
) -> ApiResult<api::FleetEntry> {
    let shown = environments::show(ctx, conn, principal, environment).await?;
    Ok(api::FleetEntry {
        environment: shown.base,
        home: Some(shown.home),
        threads: shown.threads.len() as i64 + shown.threads_hidden,
        host_name: shown.host.map(|h| h.name),
        template: shown.template.map(|t| api::TemplateRef {
            id: t.id,
            name: t.name,
        }),
        generation: shown.generation,
        updates: shown.updates.len() as i64,
        last_activity_at: shown.last_activity_at,
        idle_minutes: shown
            .last_activity_at
            .map(|t| (ctx.now - t.get()).num_minutes().max(0)),
        idle_stop_at: shown.idle_stop_at,
        active_turns: envs::active_turns(conn, environment).await?,
        open_shells: envs::open_shells(conn, environment).await?,
        processes: None,
        cpu_percent: None,
        memory_bytes: None,
        memory_limit_bytes: None,
        disk_used_bytes: None,
        manageable,
        holds: Vec::new(),
    })
}

/// The same recycle operation serves a node's list and a host manager's list.
pub(crate) async fn recycle_ids(
    ctx: &Ctx,
    ids: &[Id],
    body: &api::RecycleBody,
    host: Option<Id>,
) -> ApiResult<api::RecycleReply> {
    let (ctx, tx, principal) = crate::routes::nodes::action(ctx).await?;
    tx.commit().await.map_err(DbError::from)?;
    let ctx = &ctx;
    let mut reply = api::RecycleReply {
        candidates: Vec::new(),
        stopped: 0,
        skipped_no_permission: 0,
        skipped_active: 0,
    };
    for id in ids {
        let mut conn = ctx.app.db.acquire().await?;
        if host.is_none()
            && !updates::may_manage(&mut conn, &principal, Target::Environment(*id))
                .await
                .map_err(ApiError::internal)?
        {
            reply.skipped_no_permission += 1;
            continue;
        }
        if !ctx
            .app
            .engine
            .recyclable(
                *id,
                body.idle_minutes,
                body.include_with_processes.unwrap_or(false),
            )
            .await
            .map_err(ApiError::internal)?
        {
            reply.skipped_active += 1;
            continue;
        }
        let mut candidate = entry(ctx, &mut conn, &principal, *id, true).await?;
        drop(conn);
        if !body.dry_run {
            // A process probe and an earlier candidate's stop may take time. Ask this act's
            // credential, page and clock again before accepting each next stop.
            let (current, mut action, principal) = crate::routes::nodes::action(ctx).await?;
            if host.is_none() {
                let node = current.id("id")?;
                authorization::require(
                    &mut action,
                    &principal,
                    node,
                    authorization::VIEW,
                    orch_authz::Act::Use,
                    orch_authz::Policy::default(),
                )
                .await?;
                if !updates::may_manage(&mut action, &principal, Target::Environment(*id))
                    .await
                    .map_err(ApiError::internal)?
                {
                    reply.skipped_no_permission += 1;
                    continue;
                }
                if !envs::environments_under(&mut action, node)
                    .await?
                    .contains(id)
                {
                    reply.skipped_active += 1;
                    continue;
                }
            }
            candidate = entry(&current, &mut action, &principal, *id, true).await?;
            match ctx
                .app
                .engine
                .recycle_environment_in(
                    action,
                    &principal,
                    &crate::sign_in::origin(&current),
                    *id,
                    body.idle_minutes,
                    host,
                )
                .await
            {
                Ok(true) => reply.stopped += 1,
                Ok(false) => {
                    reply.skipped_active += 1;
                    continue;
                }
                Err(
                    orch_engine::environments::StopError::Busy(_)
                    | orch_engine::environments::StopError::Unavailable(_),
                ) => {
                    reply.skipped_active += 1;
                    continue;
                }
                Err(error) => {
                    return Err(ApiError::new(
                        orch_api_contract::common::ErrorCode::EnvironmentUnavailable,
                        error.to_string(),
                    ));
                }
            }
        }
        reply.candidates.push(candidate);
    }
    Ok(reply)
}

async fn recycle(ctx: Ctx, (): (), body: api::RecycleBody) -> ApiResult<api::RecycleReply> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let node = ctx.id("id")?;
    authorization::require(
        &mut conn,
        &principal,
        node,
        authorization::VIEW,
        orch_authz::Act::Use,
        orch_authz::Policy::default(),
    )
    .await?;
    let ids = envs::environments_under(&mut conn, node).await?;
    drop(conn);
    recycle_ids(&ctx, &ids, &body, None).await
}

async fn update_preflight(
    ctx: Ctx,
    (): (),
    body: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<api::EnvironmentUpdateBatchPlan> {
    let node = ctx.id("id")?;
    plan_updates(ctx, body, Some(node)).await
}

async fn update_all_preflight(
    ctx: Ctx,
    (): (),
    body: api::NodeEnvironmentUpdatePreflightBody,
) -> ApiResult<api::EnvironmentUpdateBatchPlan> {
    plan_updates(ctx, body, None).await
}

async fn plan_updates(
    mut ctx: Ctx,
    body: api::NodeEnvironmentUpdatePreflightBody,
    node: Option<Id>,
) -> ApiResult<api::EnvironmentUpdateBatchPlan> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    let mut ids = body.environment_ids;
    ids.sort_unstable();
    ids.dedup();
    if let Some(node) = node {
        authorization::require(
            &mut tx,
            &principal,
            node,
            authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await?;
        let under = envs::environments_under(&mut tx, node).await?;
        if ids.iter().any(|id| !under.contains(id)) {
            return Err(ApiError::not_found());
        }
    }
    let views = environments::environments_of(&ctx, &mut tx, &principal, &ids).await?;
    let id = new_id();
    let mut recipes = Vec::new();
    let mut plans_shown = Vec::new();
    let mut skipped = Vec::new();
    for view in views {
        let draft = match ctx
            .app
            .engine
            .draft_update(
                &mut tx,
                &principal,
                Target::Environment(view.base.id),
                api::UpdatePreflightReason::Update,
                None,
            )
            .await
        {
            Ok(draft) => draft,
            Err(UpdateError::Denied) => {
                skipped.push(api::EnvironmentUpdateSkipped {
                    environment: view.base,
                    reason: api::UpdateSkipReason::NoPermission,
                });
                continue;
            }
            Err(UpdateError::Blocked(_)) | Err(UpdateError::NotFound) => {
                skipped.push(api::EnvironmentUpdateSkipped {
                    environment: view.base,
                    reason: api::UpdateSkipReason::NotUpdatable,
                });
                continue;
            }
            Err(error) => return Err(plans::refused(error)),
        };
        if draft.updates.is_empty() {
            skipped.push(api::EnvironmentUpdateSkipped {
                environment: view.base,
                reason: api::UpdateSkipReason::UpToDate,
            });
            continue;
        }
        plans_shown.push(plans::environment_plan(&ctx, &mut tx, &principal, id, &draft).await?);
        if draft.blockers.is_empty() {
            recipes.push(draft.recipe);
        }
    }
    let place = node.map_or(orch_settings::Place::System, orch_settings::Place::Node);
    let options = updates::options(&mut tx, place, None)
        .await
        .map_err(plans::refused)?;
    let parallel = orch_settings::get::<i64>(
        &mut tx,
        place,
        orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(updates::DEFAULT_BATCH_PARALLEL);
    let estimate = orch_engine::jobs::estimates::together(
        &plans_shown
            .iter()
            .map(|p| p.estimate.clone())
            .collect::<Vec<_>>(),
        parallel,
    );
    plans::keep_batch(&ctx, &mut tx, id, node, &recipes, false).await?;
    let executable = recipes.len() as i64;
    let affected: Vec<_> = recipes
        .iter()
        .flat_map(|recipe| recipe.threads.iter().copied())
        .collect();
    let notify = Some(plans::audience(&ctx, &mut tx, &principal, &affected).await?);
    tx.commit().await.map_err(DbError::from)?;
    Ok(api::EnvironmentUpdateBatchPlan {
        notify,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        plans: plans_shown,
        skipped,
        executable,
        options,
        estimate,
    })
}

async fn update_create(
    ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<api::EnvironmentBatch> {
    let node = ctx.id("id")?;
    create_updates(ctx, body, Some(node), query).await
}

async fn update_all_create(
    ctx: Ctx,
    query: orch_api_contract::forks::UpdateConfirmQuery,
    body: ForkCreateBody,
) -> ApiResult<api::EnvironmentBatch> {
    create_updates(ctx, body, None, query).await
}

async fn create_updates(
    mut ctx: Ctx,
    body: ForkCreateBody,
    node: Option<Id>,
    query: orch_api_contract::forks::UpdateConfirmQuery,
) -> ApiResult<api::EnvironmentBatch> {
    let (mut tx, guard) = transaction_guard::begin(&ctx).await?;
    ctx.now = guard.now().get();
    let principal = guard.into_principal();
    if let Some(node) = node {
        authorization::require(
            &mut tx,
            &principal,
            node,
            authorization::VIEW,
            orch_authz::Act::Use,
            orch_authz::Policy::default(),
        )
        .await?;
    }
    let drafts = plans::batch_drafts(&ctx, &mut tx, &principal, node, false, &body).await?;
    if let Some(node) = node {
        let under = envs::environments_under(&mut tx, node).await?;
        if drafts
            .iter()
            .any(|draft| !under.contains(&draft.recipe.target.id()))
        {
            return Err(ApiError::not_found());
        }
    }
    let batch_id = new_id();
    let affected: Vec<_> = drafts
        .iter()
        .flat_map(|draft| draft.recipe.threads.iter().copied())
        .collect();
    let mut jobs = Vec::new();
    for draft in drafts {
        let environment = environments::show(&ctx, &mut tx, &principal, draft.recipe.target.id())
            .await?
            .base;
        let row = orch_engine::jobs::recreate::queue(&mut tx, draft, Some(batch_id), ctx.now)
            .await
            .map_err(plans::refused)?;
        jobs.push(api::EnvironmentBatchJob {
            environment,
            job: shown_jobs::view(&mut tx, &row).await?,
        });
    }
    if query.notify.unwrap_or(true) {
        plans::tell_update(&ctx, &mut tx, &affected, batch_id).await?;
    }
    orch_db::queries::jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(api::EnvironmentBatch { batch_id, jobs })
}
