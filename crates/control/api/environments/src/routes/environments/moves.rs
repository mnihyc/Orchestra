//! Workspace move plans and confirmations, by data ownership and declared machine lifecycle.

use super::updates::refused;

use crate::middleware::authorization;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use orch_api_contract::environment as api;

use orch_api_contract::forks::EnvironmentMovePlan;

use orch_api_contract::forks::ForkCreateBody;

use orch_api_contract::forks::MoveThreadAction;

use orch_api_contract::forks::MovedThread;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::queries::jobs;

use orch_db::queries::jobs::forks::PreflightRow;

use orch_db::Conn;

use orch_db::DbError;

use orch_engine::jobs::environment_move::MoveDraft;

use orch_engine::jobs::environment_move::MoveRecipe;

use orch_engine::jobs::machine_move;

use orch_engine::updates;

use orch_types::Id;

use orch_types::Time;

use orch_types::new_id;

use serde::Deserialize;

use serde::Serialize;

use serde_json::json;
/// The native batch saved for confirmation and its private audience's factual subjects.
#[derive(Clone, Serialize, Deserialize)]
struct StoredMoveBatch {
    #[serde(default)]
    observations: orch_engine::jobs::measurement::SavedMeasurements,
    recipes: Vec<MoveRecipe>,
    #[serde(default)]
    portable_recipes: Vec<machine_move::Recipe>,
    request: api::EnvironmentMovePreflightBody,
    notification: orch_api_contract::loss::OperationNotificationPlan,
}

#[derive(Clone, PartialEq, Eq)]
struct MoveSourceAssociation {
    owner: Id,
    home: Id,
    generation: i32,
    machine: Option<Id>,
    runtime: Option<Id>,
}

#[derive(Clone)]
pub(in crate::routes) struct ObservedMoveBatch {
    node: Id,
    stored: StoredMoveBatch,
    plan: api::EnvironmentMoveBatchPlan,
    audience: Vec<orch_engine::notifications::OperationRecipient>,
    visible: Vec<Id>,
    managed: Vec<Id>,
    destinations: std::collections::BTreeSet<(orch_access_contract::permissions::ObjectKind, Id)>,
    content: std::collections::BTreeSet<(orch_access_contract::permissions::ObjectKind, Id)>,
    associations: std::collections::BTreeMap<Id, MoveSourceAssociation>,
}

pub(crate) fn recipient_subjects(
    plan: &orch_types::Json,
) -> ApiResult<Vec<orch_access_contract::decisions::ObjectRef>> {
    let stored: StoredMoveBatch =
        serde_json::from_value(plan.clone()).map_err(ApiError::internal)?;
    Ok(stored
        .recipes
        .into_iter()
        .map(|recipe| recipe.replacement.target.id())
        .chain(
            stored
                .portable_recipes
                .into_iter()
                .map(|recipe| recipe.environment),
        )
        .map(|id| orch_access_contract::decisions::ObjectRef {
            kind: orch_access_contract::permissions::ObjectKind::Environment,
            id,
        })
        .collect())
}

/// Binds the plan and execution of one environment move.
pub fn mount(registry: &mut Registry) {
    registry
        .creation(api::EnvironmentMoveCreate, confirmation::binding())
        .on(api::EnvironmentMovePlaceList, places)
        .on_with_initial_operation(
            api::EnvironmentMovePreflight,
            crate::key_operations::native::effects::initial::<operation::Preview>,
            super::lifecycle::prepare::<operation::Preview>,
            operation::preflight,
        )
        .on_with_initial_operation(
            api::EnvironmentMoveCreate,
            crate::key_operations::native::effects::initial::<confirmation::Confirm>,
            super::lifecycle::prepare::<confirmation::Confirm>,
            confirmation::create,
        );
}

async fn places(
    ctx: Ctx,
    query: api::PlaceListQuery,
    (): (),
) -> ApiResult<Vec<api::EnvironmentMovePlace>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let mut places = machine_move::places(
        &ctx.app.engine,
        &mut conn,
        &principal,
        ctx.id("id")?,
        query.template_id,
    )
    .await
    .map_err(refused)?;
    let subjects = places
        .iter()
        .map(|place| match place {
            api::EnvironmentMovePlace::Host(host) => ("host".to_owned(), host.host.id),
            api::EnvironmentMovePlace::Machine(machine) => {
                ("machine".to_owned(), machine.machine.id)
            }
        })
        .collect::<Vec<_>>();
    let grouping = crate::routes::resources::grouping::Prepared::select(&mut conn, &subjects)
        .await?
        .choices(&ctx.app, &mut conn, &principal, None, None)
        .await?;
    for place in &mut places {
        match place {
            api::EnvironmentMovePlace::Host(host) => {
                if let Some(meta) = grouping
                    .iter()
                    .find(|row| row.kind == "host" && row.id == host.host.id)
                {
                    // The planner already made the actual per-machine availability judgement.
                    let availability = std::mem::take(&mut host.grouping.availability);
                    host.grouping = meta.grouping.clone();
                    host.grouping.availability = availability;
                }
            }
            api::EnvironmentMovePlace::Machine(machine) => {
                if let Some(meta) = grouping
                    .iter()
                    .find(|row| row.kind == "machine" && row.id == machine.machine.id)
                {
                    let availability = std::mem::take(&mut machine.grouping.availability);
                    machine.grouping = meta.grouping.clone();
                    machine.grouping.availability = availability;
                }
            }
        }
    }
    Ok(places)
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
        kind: draft.recipe.transfer_kind(),
        source_has_unit: draft.recipe.source_has_unit(),
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
orch_api_base::__api_bind_4d665163fa!();

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
        kind: orch_api_contract::forks::EnvironmentTransferKind::Move,
        source_has_unit: true,
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
orch_api_base::__api_bind_8e01278771!();

/// Plans the selected visible environments of a place, without holding any unit.
async fn observe_batch(
    ctx: &Ctx,
    principal: &Principal,
    body: api::NodeEnvironmentMovePreflightBody,
    id: Id,
    allowed: &std::collections::BTreeSet<Id>,
) -> ApiResult<ObservedMoveBatch> {
    let (result, observations) = orch_engine::jobs::measurement::recording::record(
        observe_batch_native(ctx, principal, body, id, allowed),
    )
    .await;
    let mut observed = result?;
    observed.stored.observations = observations;
    Ok(observed)
}

async fn observe_batch_native(
    ctx: &Ctx,
    principal: &Principal,
    body: api::NodeEnvironmentMovePreflightBody,
    id: Id,
    allowed: &std::collections::BTreeSet<Id>,
) -> ApiResult<ObservedMoveBatch> {
    use orch_access_contract::permissions::ObjectKind;
    let mut tx = ctx.app.db.begin_snapshot().await?;
    let node = ctx.id("id")?;
    authorization::require(
        &mut tx,
        principal,
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
    let prepared = super::prepare_viewed(ctx, &mut tx, principal, &ids).await?;
    let visible = prepared.selected_ids();
    let views = super::show_inventory_prepared(ctx, &mut tx, principal, &prepared).await?;
    let mut content = prepared
        .content_sources(&mut tx)
        .await?
        .into_values()
        .flatten()
        .map(|object| (object.kind, object.id))
        .collect::<std::collections::BTreeSet<_>>();
    content.extend(visible.iter().map(|id| (ObjectKind::Environment, *id)));
    let mut associations = std::collections::BTreeMap::new();
    for id in &visible {
        let row = orch_db::queries::environments::hosting::environment(&mut tx, *id)
            .await?
            .ok_or_else(ApiError::not_found)?;
        let runtime = orch_db::queries::environments::hosting::live_runtime(&mut tx, *id)
            .await?
            .map(|runtime| runtime.id);
        associations.insert(
            *id,
            MoveSourceAssociation {
                owner: row.owner_id,
                home: row.home_node_id,
                generation: row.generation,
                machine: row.machine_id,
                runtime,
            },
        );
    }
    let mut managed = Vec::new();
    let mut destinations = std::collections::BTreeSet::new();
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
    for (environment, view) in visible.iter().copied().zip(views) {
        if !allowed.contains(&environment)
            || !updates::may_manage(
                &mut tx,
                principal,
                updates::Target::Environment(environment),
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
        managed.push(environment);
        if machine_move::needed(&mut tx, environment, &request)
            .await
            .map_err(ApiError::internal)?
        {
            // The measurements must not retain this batch's transaction or pool lease.
            tx.commit().await.map_err(DbError::from)?;
            let result = ctx
                .app
                .engine
                .draft_machine_move(principal, environment, &request, None)
                .await;
            tx = ctx.app.db.begin_snapshot().await?;
            match result {
                Ok(draft) => {
                    destinations.extend(
                        draft
                            .recipe
                            .destination_access()
                            .into_iter()
                            .map(|(object, _)| (object.kind, object.id)),
                    );
                    content.extend(
                        draft
                            .recipe
                            .threads
                            .iter()
                            .map(|id| (ObjectKind::Thread, *id)),
                    );
                    content.extend(
                        draft
                            .mounts
                            .iter()
                            .map(|mount| (ObjectKind::Mount, mount.mount.id)),
                    );
                    content.extend(
                        draft
                            .mcp
                            .iter()
                            .filter(|server| !server.own && !server.imported)
                            .map(|server| (ObjectKind::McpServer, server.id)),
                    );
                    plans.push(shown_machine(ctx, &mut tx, principal, id, &draft).await?);
                    if draft.blockers.is_empty() {
                        let mut recipe = draft.recipe;
                        recipe.token = None;
                        portable_recipes.push(recipe);
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
            .draft_move(&mut tx, principal, environment, &request, None)
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
        destinations.insert((ObjectKind::Host, draft.recipe.destination));
        destinations.insert((ObjectKind::Template, draft.recipe.replacement.template));
        content.extend(
            draft
                .recipe
                .replacement
                .threads
                .iter()
                .map(|id| (ObjectKind::Thread, *id)),
        );
        content.extend(
            draft
                .mounts
                .iter()
                .map(|mount| (ObjectKind::Mount, mount.mount.id)),
        );
        content.extend(
            draft
                .mcp
                .iter()
                .filter(|server| !server.own && !server.imported)
                .map(|server| (ObjectKind::McpServer, server.id)),
        );
        plans.push(shown(ctx, &mut tx, principal, id, &draft).await?);
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
        orch_engine::notifications::operation_audience(&mut tx, principal, &affected, &[])
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
    tx.rollback().await.map_err(DbError::from)?;
    content.extend(destinations.iter().copied());
    let stored = StoredMoveBatch {
        observations: Default::default(),
        recipes,
        portable_recipes,
        request,
        notification: notification.clone(),
    };
    Ok(ObservedMoveBatch {
        node,
        stored,
        audience,
        visible,
        managed,
        destinations,
        content,
        associations,
        plan: api::EnvironmentMoveBatchPlan {
            notification,
            preflight_id: id,
            expires_at: Time::new(updates::expires(ctx.now)),
            plans,
            skipped,
            executable,
            options,
            estimate,
        },
    })
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

#[path = "moves/batch.rs"]
pub mod batch;

#[path = "moves/confirmation.rs"]
pub mod confirmation;

#[path = "moves/operation.rs"]
pub mod operation;
