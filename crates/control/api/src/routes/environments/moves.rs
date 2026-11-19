//! Workspace move plans and confirmations, by data ownership and declared machine lifecycle.

use super::updates::refused;
use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use orch_api_contract::{
    environment as api,
    forks::{EnvironmentMovePlan, ForkCreateBody, MoveThreadAction, MovedThread},
    workspace::Job,
};
use orch_authz::{Policy, Principal};
use orch_db::queries::jobs::{self, forks::PreflightRow};
use orch_db::{Conn, DbError};
use orch_engine::jobs::environment_move::{MoveDraft, MoveRecipe};
use orch_engine::jobs::machine_move;
use orch_engine::updates;
use orch_types::{Id, Time, new_id};
use serde_json::json;

/// Binds the plan and execution of one environment move.
pub fn mount(registry: &mut Registry) {
    registry
        .on(api::EnvironmentMovePlaceList, places)
        .on(api::EnvironmentMovePreflight, preflight)
        .on(api::EnvironmentMoveCreate, create);
}

async fn places(
    ctx: Ctx,
    query: api::PlaceListQuery,
    (): (),
) -> ApiResult<Vec<api::EnvironmentMovePlace>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    machine_move::places(
        &ctx.app.engine,
        &mut conn,
        &principal,
        ctx.id("id")?,
        query.template_id,
    )
    .await
    .map_err(refused)
}

pub(crate) async fn shown_machine(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    draft: &machine_move::Draft,
) -> ApiResult<EnvironmentMovePlan> {
    let environment = super::show(ctx, conn, principal, draft.recipe.environment).await?;
    let source = orch_db::queries::environment_facts::of(conn, draft.recipe.environment)
        .await?
        .and_then(|facts| facts.machine_id)
        .unwrap_or(draft.recipe.source_machine);
    let source = orch_db::queries::resources::machine(conn, source)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let refs =
        crate::routes::nodes::view::refs(conn, principal, &draft.recipe.threads, Policy::default())
            .await?;
    let mut threads = Vec::new();
    for (id, action) in &draft.threads {
        threads.push(MovedThread {
            thread: refs
                .get(id)
                .filter(|reference| !reference.redacted)
                .cloned(),
            action: *action,
            turn_running: orch_db::queries::threads::active_turn(conn, *id)
                .await?
                .is_some(),
            note: match action {
                MoveThreadAction::Continue => {
                    "The harness and its conversation stay with the provider"
                }
                MoveThreadAction::Reimport if draft.recipe.files.home.is_some() => {
                    "The native session resumes from the copied home"
                }
                MoveThreadAction::Reimport => "Continues from Orch's record",
                MoveThreadAction::Blocked => {
                    "The destination has not declared the required executor"
                }
            }
            .into(),
        });
    }
    let files = draft.recipe.files.clone();
    let estimate = orch_engine::jobs::estimates::estimate(
        &orch_engine::jobs::estimates::Work {
            bytes_up: files.bytes_to_upload,
            bytes_down: files.bytes_to_download,
            files: files
                .file_count
                .saturating_add(files.home.as_ref().map_or(0, |home| home.file_count)),
            making: orch_engine::jobs::estimates::Making::Environment,
            archive_bytes: None,
        },
        &orch_engine::jobs::estimates::Rates::assumed(),
    );
    Ok(EnvironmentMovePlan {
        notification: orch_api_contract::loss::OperationNotificationPlan {
            preview_id: Some(id),
            people: orch_engine::notifications::operation_audience(
                conn,
                principal,
                &[environment.base.id],
                &[],
            )
            .await
            .map_err(ApiError::internal)?
            .len() as i64,
            default_on: orch_engine::notifications::operation_default(
                conn,
                orch_settings::Place::Environment(environment.base.id),
                !threads.is_empty(),
            )
            .await
            .map_err(ApiError::internal)?,
        },
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        environment: environment.base,
        from: environment.host,
        from_machine: Some(machine_move::machine_ref(&source).map_err(ApiError::internal)?),
        to: draft.place.clone(),
        files,
        network: draft.recipe.network(),
        threads,
        mounts: draft.mounts.clone(),
        mcp: draft.mcp.clone(),
        held_still: draft.recipe.held_still(),
        options: draft.recipe.options.clone(),
        estimate,
        blockers: draft.blockers.clone(),
    })
}

/// Fills names and thread visibility around the engine's move plan.
pub(crate) async fn shown(
    ctx: &Ctx,
    conn: &mut Conn,
    principal: &Principal,
    id: Id,
    draft: &MoveDraft,
) -> ApiResult<EnvironmentMovePlan> {
    let environment =
        super::show(ctx, conn, principal, draft.recipe.replacement.target.id()).await?;
    let from = environment.host.clone().ok_or_else(ApiError::not_found)?;
    let refs = crate::routes::nodes::view::refs(
        conn,
        principal,
        &draft.recipe.replacement.threads,
        Policy::default(),
    )
    .await?;
    let mut threads = Vec::new();
    for (id, action) in &draft.threads {
        threads.push(MovedThread {
            thread: refs.get(id).filter(|r| !r.redacted).cloned(),
            action: *action,
            turn_running: orch_db::queries::threads::active_turn(conn, *id)
                .await?
                .is_some(),
            note: match action {
                MoveThreadAction::Continue => {
                    "The harness and its conversation stay with the provider"
                }
                MoveThreadAction::Reimport if draft.recipe.files.home.is_some() => {
                    "The native session resumes from the copied home"
                }
                MoveThreadAction::Reimport => "Continues from Orch's record",
                MoveThreadAction::Blocked => "The target does not have its executor",
            }
            .to_owned(),
        });
    }
    let files = draft.recipe.files.clone();
    // A mount's data that is copied goes up from the old host and down to the new one, beside the workspace.
    let (copied_files, copied_bytes) = draft.recipe.mount_decisions.totals();
    let estimate = orch_engine::jobs::estimates::estimate(
        &orch_engine::jobs::estimates::Work {
            bytes_up: files.bytes_to_upload.saturating_add(copied_bytes),
            bytes_down: files.bytes_to_download.saturating_add(copied_bytes),
            files: files
                .file_count
                .saturating_add(files.home.as_ref().map_or(0, |home| home.file_count))
                .saturating_add(copied_files)
                .saturating_add(
                    files
                        .private_views
                        .iter()
                        .filter(|view| view.carried)
                        .map(|view| view.file_count)
                        .fold(0i64, i64::saturating_add),
                ),
            making: orch_engine::jobs::estimates::Making::Environment,
            archive_bytes: None,
        },
        &orch_engine::jobs::estimates::Rates::assumed(),
    );
    Ok(EnvironmentMovePlan {
        notification: orch_api_contract::loss::OperationNotificationPlan {
            preview_id: Some(id),
            people: orch_engine::notifications::operation_audience(
                conn,
                principal,
                &[environment.base.id],
                &[],
            )
            .await
            .map_err(ApiError::internal)?
            .len() as i64,
            default_on: orch_engine::notifications::operation_default(
                conn,
                orch_settings::Place::Environment(environment.base.id),
                !threads.is_empty(),
            )
            .await
            .map_err(ApiError::internal)?,
        },
        network: super::network::planned(conn, principal, &draft.recipe.replacement).await?,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        environment: environment.base,
        from: Some(from),
        from_machine: None,
        to: api::EnvironmentMovePlace::Host(Box::new(draft.destination.place.clone())),
        files,
        threads,
        mounts: draft.mounts.clone(),
        mcp: draft.mcp.clone(),
        held_still: draft.recipe.held_still,
        options: draft.recipe.replacement.options.clone(),
        estimate,
        blockers: draft.blockers.clone(),
    })
}

async fn preflight(
    ctx: Ctx,
    (): (),
    body: api::EnvironmentMovePreflightBody,
) -> ApiResult<EnvironmentMovePlan> {
    let environment = ctx.id("id")?;
    let (principal, portable) = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let portable = machine_move::needed(&mut conn, environment, &body)
            .await
            .map_err(ApiError::internal)?;
        (principal, portable)
    };
    if portable {
        let draft = ctx
            .app
            .engine
            .draft_machine_move(&principal, environment, &body, None)
            .await
            .map_err(refused)?;
        let id = new_id();
        let mut conn = ctx.app.db.acquire().await?;
        let plan = shown_machine(&ctx, &mut conn, &principal, id, &draft).await?;
        jobs::forks::save_preflight(
            &mut conn,
            &PreflightRow {
                id,
                kind: "environment_move".into(),
                node_id: None,
                environment_id: Some(environment),
                created_by: principal.user(),
                plan: json!({"portable":draft.recipe,"request":body,"notification":plan.notification}),
                expires_at: updates::expires(ctx.now),
                created_at: ctx.now,
            },
        )
        .await?;
        save_audience(&mut conn, &principal, environment, id).await?;
        return Ok(plan);
    }
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let environment = ctx.id("id")?;
    let draft = ctx
        .app
        .engine
        .draft_move(&mut tx, &principal, environment, &body, None)
        .await
        .map_err(refused)?;
    let id = new_id();
    let plan = shown(&ctx, &mut tx, &principal, id, &draft).await?;
    jobs::forks::save_preflight(
        &mut tx,
        &PreflightRow {
            id,
            kind: "environment_move".into(),
            node_id: None,
            environment_id: Some(environment),
            created_by: principal.user(),
            plan: json!({"recipe":draft.recipe,"request":body,"notification":plan.notification}),
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    save_audience(&mut tx, &principal, environment, id).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(plan)
}

async fn create(ctx: Ctx, (): (), body: ForkCreateBody) -> ApiResult<Job> {
    let environment = ctx.id("id")?;
    let (principal, original) = {
        let mut conn = ctx.app.db.acquire().await?;
        let principal = authorization::principal(&ctx, &mut conn).await?;
        let row = jobs::changes::lock_plan(&mut conn, body.preflight_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        (principal, row)
    };
    if original.plan.get("portable").is_some() {
        if original.kind != "environment_move"
            || original.environment_id != Some(environment)
            || original.created_by != principal.user()
        {
            return Err(ApiError::not_found());
        }
        if original.expires_at <= ctx.now {
            return Err(refused(updates::UpdateError::Expired));
        }
        let recipe: machine_move::Recipe =
            serde_json::from_value(original.plan["portable"].clone())
                .map_err(ApiError::internal)?;
        let request: api::EnvironmentMovePreflightBody =
            serde_json::from_value(original.plan["request"].clone()).map_err(ApiError::internal)?;
        let draft = ctx
            .app
            .engine
            .draft_machine_move(&principal, environment, &request, body.options.as_ref())
            .await
            .map_err(refused)?;
        if !recipe.same_choice(&draft.recipe) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                "The source, destination or promised data changed; review the move again.",
            ));
        }
        let mut tx = ctx.app.db.begin().await?;
        orch_db::queries::transaction_guard::lock_authority(&mut tx).await?;
        let principal = authorization::principal(&ctx, &mut tx).await?;
        let current = jobs::changes::lock_plan(&mut tx, body.preflight_id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        if current.plan != original.plan || current.expires_at <= ctx.now {
            return Err(refused(updates::UpdateError::Expired));
        }
        let operation = move_operation(
            &mut tx,
            &principal,
            environment,
            body.preflight_id,
            body.notify.unwrap_or(false),
        )
        .await?;
        let row = orch_engine::notifications::operation_scope(
            Some(operation.id),
            machine_move::queue(&mut tx, &principal, draft, None, ctx.now),
        )
        .await
        .map_err(refused)?;
        orch_engine::notifications::attach_operation(&mut tx, &[row.id], &operation, ctx.now)
            .await
            .map_err(ApiError::internal)?;
        orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
            .await
            .map_err(ApiError::internal)?;
        jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
        let job = super::jobs::view(&mut tx, &row).await?;
        tx.commit().await.map_err(DbError::from)?;
        ctx.app.engine.kick();
        return Ok(job);
    }
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let environment = ctx.id("id")?;
    let row = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.kind != "environment_move"
        || row.environment_id != Some(environment)
        || row.created_by != principal.user()
    {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.now {
        return Err(refused(updates::UpdateError::Expired));
    }
    let recipe: MoveRecipe =
        serde_json::from_value(row.plan["recipe"].clone()).map_err(ApiError::internal)?;
    let request: api::EnvironmentMovePreflightBody =
        serde_json::from_value(row.plan["request"].clone()).map_err(ApiError::internal)?;
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
        .map_err(refused)?;
    if draft.recipe.replacement.old != recipe.replacement.old {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "the unit was changed after this move was planned",
        ));
    }
    let operation = move_operation(
        &mut tx,
        &principal,
        environment,
        body.preflight_id,
        body.notify.unwrap_or(false),
    )
    .await?;
    let row = orch_engine::notifications::operation_scope(
        Some(operation.id),
        orch_engine::jobs::environment_move::queue(&mut tx, draft, None, ctx.now),
    )
    .await
    .map_err(refused)?;
    orch_engine::notifications::attach_operation(&mut tx, &[row.id], &operation, ctx.now)
        .await
        .map_err(ApiError::internal)?;
    orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    let job = super::jobs::view(&mut tx, &row).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(job)
}

/// Plans the selected visible environments of a place, without holding any unit.
pub(crate) async fn batch_preflight(
    ctx: Ctx,
    (): (),
    body: api::NodeEnvironmentMovePreflightBody,
) -> ApiResult<api::EnvironmentMoveBatchPlan> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let node = ctx.id("id")?;
    authorization::require(
        &mut tx,
        &principal,
        node,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await?;
    let under = orch_db::queries::environments::hosting::environments_under(&mut tx, node).await?;
    let mut ids = body.environment_ids;
    ids.sort_unstable();
    ids.dedup();
    if ids.iter().any(|id| !under.contains(id)) {
        return Err(ApiError::not_found());
    }
    let views = super::environments_of(&ctx, &mut tx, &principal, &ids).await?;
    let id = new_id();
    let request = api::EnvironmentMovePreflightBody {
        machine_id: body.machine_id,
        template_id: body.template_id,
        copy_directories: body.copy_directories,
        host_id: body.host_id,
        home: None,
        private_views: None,
        files: None,
        check_fingerprints: None,
        fingerprint_request_id: None,
        mounts: None,
    };
    let mut plans = Vec::new();
    let mut skipped = Vec::new();
    let mut recipes = Vec::new();
    let mut portable_recipes = Vec::new();
    for view in views {
        if !updates::may_manage(
            &mut tx,
            &principal,
            updates::Target::Environment(view.base.id),
        )
        .await
        .map_err(ApiError::internal)?
        {
            skipped.push(api::EnvironmentMoveSkipped {
                environment: view.base,
                reason: api::MoveSkipReason::NoPermission,
            });
            continue;
        }
        if machine_move::needed(&mut tx, view.base.id, &request)
            .await
            .map_err(ApiError::internal)?
        {
            // The measurements must not retain this batch's transaction or pool lease.
            tx.commit().await.map_err(DbError::from)?;
            let result = ctx
                .app
                .engine
                .draft_machine_move(&principal, view.base.id, &request, None)
                .await;
            tx = ctx.app.db.begin().await?;
            match result {
                Ok(draft) => {
                    plans.push(shown_machine(&ctx, &mut tx, &principal, id, &draft).await?);
                    if draft.blockers.is_empty() {
                        portable_recipes.push(draft.recipe);
                    }
                }
                Err(updates::UpdateError::Blocked(_)) => {
                    skipped.push(api::EnvironmentMoveSkipped {
                        environment: view.base,
                        reason: api::MoveSkipReason::NotMovable,
                    })
                }
                Err(error) => return Err(refused(error)),
            }
            continue;
        }
        let draft = match ctx
            .app
            .engine
            .draft_move(&mut tx, &principal, view.base.id, &request, None)
            .await
        {
            Ok(draft) => draft,
            Err(updates::UpdateError::Blocked(_)) => {
                skipped.push(api::EnvironmentMoveSkipped {
                    environment: view.base,
                    reason: api::MoveSkipReason::NotMovable,
                });
                continue;
            }
            Err(error) => return Err(refused(error)),
        };
        plans.push(shown(&ctx, &mut tx, &principal, id, &draft).await?);
        if draft.blockers.is_empty() {
            recipes.push(draft.recipe);
        }
    }
    let options = updates::options(&mut tx, orch_settings::Place::Node(node), None)
        .await
        .map_err(refused)?;
    let parallel = orch_settings::get::<i64>(
        &mut tx,
        orch_settings::Place::Node(node),
        orch_settings_contract::registry::keys::SWITCH_BATCH_PARALLEL,
    )
    .await
    .map_err(ApiError::internal)?
    .unwrap_or(updates::DEFAULT_BATCH_PARALLEL);
    let estimate = orch_engine::jobs::estimates::together(
        &plans.iter().map(|p| p.estimate.clone()).collect::<Vec<_>>(),
        parallel,
    );
    let executable = (recipes.len() + portable_recipes.len()) as i64;
    let affected = recipes
        .iter()
        .map(|recipe| recipe.replacement.target.id())
        .chain(portable_recipes.iter().map(|recipe| recipe.environment))
        .collect::<Vec<_>>();
    let audience =
        orch_engine::notifications::operation_audience(&mut tx, &principal, &affected, &[])
            .await
            .map_err(ApiError::internal)?;
    let notification = orch_api_contract::loss::OperationNotificationPlan {
        preview_id: Some(id),
        people: audience.len() as i64,
        default_on: orch_engine::notifications::operation_default(
            &mut tx,
            orch_settings::Place::Node(node),
            plans.iter().any(|plan| !plan.threads.is_empty()),
        )
        .await
        .map_err(ApiError::internal)?,
    };
    jobs::forks::save_preflight(
        &mut tx,
        &PreflightRow {
            id,
            kind: "move_batch".into(),
            node_id: Some(node),
            environment_id: None,
            created_by: principal.user(),
            plan: json!({"recipes":recipes,"portable_recipes":portable_recipes,"request":request,"notification":notification}),
            expires_at: updates::expires(ctx.now),
            created_at: ctx.now,
        },
    )
    .await?;
    orch_db::queries::notifications::operations::preview(
        &mut tx,
        id,
        &audience
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(api::EnvironmentMoveBatchPlan {
        notification,
        preflight_id: id,
        expires_at: Time::new(updates::expires(ctx.now)),
        plans,
        skipped,
        executable,
        options,
        estimate,
    })
}

/// Confirms every accepted member under a shared batch limit, after checking its scope again.
pub(crate) async fn batch_create(
    ctx: Ctx,
    (): (),
    body: ForkCreateBody,
) -> ApiResult<api::EnvironmentBatch> {
    let mut tx = ctx.app.db.begin().await?;
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let node = ctx.id("id")?;
    authorization::require(
        &mut tx,
        &principal,
        node,
        authorization::VIEW,
        orch_authz::Act::Use,
        Policy::default(),
    )
    .await?;
    let row = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if row.kind != "move_batch" || row.node_id != Some(node) || row.created_by != principal.user() {
        return Err(ApiError::not_found());
    }
    if row.expires_at <= ctx.now {
        return Err(refused(updates::UpdateError::Expired));
    }
    let recipes: Vec<MoveRecipe> =
        serde_json::from_value(row.plan["recipes"].clone()).map_err(ApiError::internal)?;
    let portable_recipes: Vec<machine_move::Recipe> = serde_json::from_value(
        row.plan
            .get("portable_recipes")
            .cloned()
            .unwrap_or_else(|| json!([])),
    )
    .map_err(ApiError::internal)?;
    let request: api::EnvironmentMovePreflightBody =
        serde_json::from_value(row.plan["request"].clone()).map_err(ApiError::internal)?;
    let under = orch_db::queries::environments::hosting::environments_under(&mut tx, node).await?;
    let mut drafts = Vec::new();
    let mut portable_drafts = Vec::new();
    for recipe in recipes {
        let environment = recipe.replacement.target.id();
        if !under.contains(&environment) {
            return Err(ApiError::not_found());
        }
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
            .map_err(refused)?;
        if draft.recipe.replacement.old != recipe.replacement.old {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                "a unit changed after this batch was planned",
            ));
        }
        drafts.push(draft);
    }
    for recipe in portable_recipes {
        let environment = recipe.environment;
        if !under.contains(&environment) {
            return Err(ApiError::not_found());
        }
        tx.commit().await.map_err(DbError::from)?;
        let result = ctx
            .app
            .engine
            .draft_machine_move(&principal, environment, &request, body.options.as_ref())
            .await;
        tx = ctx.app.db.begin().await?;
        let draft = result.map_err(refused)?;
        if !recipe.same_choice(&draft.recipe) {
            return Err(ApiError::new(
                orch_api_contract::common::ErrorCode::ConflictStaleRevision,
                "A source, destination or promised data changed; review the batch again.",
            ));
        }
        portable_drafts.push(draft);
    }
    let current = jobs::changes::lock_plan(&mut tx, body.preflight_id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if current.plan != row.plan || current.expires_at <= ctx.now {
        return Err(refused(updates::UpdateError::Expired));
    }
    let principal = authorization::principal(&ctx, &mut tx).await?;
    let current_under =
        orch_db::queries::environments::hosting::environments_under(&mut tx, node).await?;
    if drafts
        .iter()
        .any(|draft| !current_under.contains(&draft.recipe.replacement.target.id()))
        || portable_drafts
            .iter()
            .any(|draft| !current_under.contains(&draft.recipe.environment))
    {
        return Err(ApiError::not_found());
    }
    let batch_id = new_id();
    let affected = drafts
        .iter()
        .map(|draft| draft.recipe.replacement.target.id())
        .chain(portable_drafts.iter().map(|draft| draft.recipe.environment))
        .collect::<Vec<_>>();
    let audience =
        orch_engine::notifications::operation_audience(&mut tx, &principal, &affected, &[])
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
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "The people affected changed; review the batch again",
        ));
    }
    let operation = orch_engine::notifications::NotificationOperation {
        id: batch_id,
        kind: "move".into(),
        name: String::new(),
        notify: body.notify.unwrap_or(false),
        path: "/resources/environments".into(),
        subject: None,
        recipients: audience,
    };
    let mut jobs = Vec::new();
    for draft in drafts {
        let environment = super::show(
            &ctx,
            &mut tx,
            &principal,
            draft.recipe.replacement.target.id(),
        )
        .await?
        .base;
        let row = orch_engine::notifications::operation_scope(
            Some(batch_id),
            orch_engine::jobs::environment_move::queue(&mut tx, draft, Some(batch_id), ctx.now),
        )
        .await
        .map_err(refused)?;
        jobs.push(api::EnvironmentBatchJob {
            environment,
            job: super::jobs::view(&mut tx, &row).await?,
        });
    }
    for draft in portable_drafts {
        let environment = super::show(&ctx, &mut tx, &principal, draft.recipe.environment)
            .await?
            .base;
        let row = orch_engine::notifications::operation_scope(
            Some(batch_id),
            machine_move::queue(&mut tx, &principal, draft, Some(batch_id), ctx.now),
        )
        .await
        .map_err(refused)?;
        jobs.push(api::EnvironmentBatchJob {
            environment,
            job: super::jobs::view(&mut tx, &row).await?,
        });
    }
    orch_engine::notifications::attach_operation(
        &mut tx,
        &jobs.iter().map(|row| row.job.id).collect::<Vec<_>>(),
        &operation,
        ctx.now,
    )
    .await
    .map_err(ApiError::internal)?;
    orch_engine::notifications::notify_operation(&mut tx, &operation, "running", ctx.now)
        .await
        .map_err(ApiError::internal)?;
    jobs::forks::forget_preflight(&mut tx, body.preflight_id).await?;
    tx.commit().await.map_err(DbError::from)?;
    ctx.app.engine.kick();
    Ok(api::EnvironmentBatch { batch_id, jobs })
}

async fn move_operation(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    id: Id,
    notify: bool,
) -> ApiResult<orch_engine::notifications::NotificationOperation> {
    let state = orch_db::queries::environments::hosting::environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let recipients =
        orch_engine::notifications::operation_audience(conn, principal, &[environment], &[])
            .await
            .map_err(ApiError::internal)?;
    if !orch_db::queries::notifications::operations::matches_preview(
        conn,
        id,
        &recipients
            .iter()
            .map(|person| person.user)
            .collect::<Vec<_>>(),
    )
    .await?
    {
        return Err(ApiError::new(
            orch_api_contract::common::ErrorCode::ConflictStaleRevision,
            "The people affected changed; review the move again",
        ));
    }
    Ok(orch_engine::notifications::NotificationOperation {
        id,
        kind: "move".into(),
        name: state.name,
        notify,
        path: format!("/resources/environments/{environment}"),
        subject: Some(("environment".into(), environment)),
        recipients,
    })
}

async fn save_audience(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    environment: Id,
    id: Id,
) -> ApiResult<()> {
    let users =
        orch_engine::notifications::operation_audience(conn, principal, &[environment], &[])
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .map(|person| person.user)
            .collect::<Vec<_>>();
    orch_db::queries::notifications::operations::preview(conn, id, &users).await?;
    Ok(())
}
