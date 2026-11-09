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

mod recycle_operation;
mod update_creation;

mod update_operation;

/// Binds fleet recycling and batch update requests.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(
            api::NodeEnvironmentUpdateCreate,
            update_operation::confirmation::binding(),
        )
        .creation(
            api::EnvironmentUpdateBatchCreate,
            update_creation::binding(),
        )
        .on_with_operation(
            api::NodeEnvironmentRecycle,
            crate::key_operations::native::effects::prepare::<recycle_operation::Recycle>,
            recycle_operation::recycle,
        )
        .on_with_initial_operation(
            api::NodeEnvironmentUpdatePreflight,
            crate::key_operations::native::effects::initial::<update_operation::Preflight>,
            crate::key_operations::native::effects::prepare::<update_operation::Preflight>,
            update_operation::preflight,
        )
        .on_with_initial_operation(
            api::NodeEnvironmentUpdateCreate,
            crate::key_operations::native::effects::initial::<
                update_operation::confirmation::Confirm,
            >,
            crate::key_operations::native::effects::prepare::<
                update_operation::confirmation::Confirm,
            >,
            update_operation::confirmation::create,
        )
        .on_with_initial_operation(
            api::EnvironmentUpdateBatchPreflight,
            crate::key_operations::native::effects::initial::<update_operation::Preflight<true>>,
            crate::key_operations::native::effects::prepare::<update_operation::Preflight<true>>,
            update_operation::global_preflight,
        )
        .on_with_initial_operation(
            api::EnvironmentUpdateBatchCreate,
            crate::key_operations::native::effects::initial::<update_creation::Create>,
            crate::key_operations::native::effects::prepare::<update_creation::Create>,
            update_creation::create,
        );
    registry
        .creation(
            api::NodeEnvironmentMoveCreate,
            environments::moves::batch::confirmation::binding(),
        )
        .on_with_initial_operation(
            api::NodeEnvironmentMovePreflight,
            crate::key_operations::native::effects::initial::<environments::moves::batch::Preview>,
            crate::key_operations::native::effects::prepare::<environments::moves::batch::Preview>,
            environments::moves::batch::preflight,
        )
        .on_with_initial_operation(
            api::NodeEnvironmentMoveCreate,
            crate::key_operations::native::effects::initial::<
                environments::moves::batch::confirmation::Confirm,
            >,
            crate::key_operations::native::effects::prepare::<
                environments::moves::batch::confirmation::Confirm,
            >,
            environments::moves::batch::confirmation::create,
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
