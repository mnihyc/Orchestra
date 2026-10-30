//! Confirmed full-history source/caller, independent of later presentation or selection.

use super::*;
use crate::jobs::upload_watch;
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_authz::{Act, Principal, TokenGrant, nodes};
use orch_db::queries::{jobs, thread_history_cuts as cuts};

const SOURCE: &str = "confirmed_history_source";

/// How much of the history phase the export of a native archive is, while its bytes cross the line: half.
/// The file crosses twice, up from the machine that holds the session and down to the one that opens
/// it, and the opening that follows imports what came; the other half is left to those.
const EXPORT_SHARE: f64 = 0.5;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct Source {
    pub cut_id: Id,
    pub thread: Id,
    pub view: Id,
    pub execution_revision: i64,
    pub queue_revision: i64,
    pub segment: Option<Id>,
    pub native_ref: Option<String>,
    pub range: Option<(i32, i32)>,
    pub accepted_turn: Option<Id>,
    pub reach: crate::fork_plan::Reach,
    pub topic: Id,
    pub name: String,
    pub links: Json,
    pub arrangement: Option<String>,
    pub own_params: Option<Json>,
    pub model: Option<String>,
    pub token: Option<Id>,
    pub scope: Option<crate::continuations::recipe::AuthorizationScope>,
}

pub(super) async fn confirmed(
    conn: &mut orch_db::Conn,
    thread: &threads::ThreadRow,
    caller: &Caller,
    recipe: &Recipe,
    cut_id: Id,
    reach: crate::fork_plan::Reach,
) -> Result<Source, EngineError> {
    let native_ref = match thread.current_segment_id {
        Some(id) => threads::segment(conn, id)
            .await?
            .and_then(|segment| segment.native_session_ref),
        None => None,
    };
    let range = crate::fork_plan::history_range_view(
        conn,
        thread.node_id,
        thread.active_continuation_id,
        &recipe.request,
    )
    .await
    .map_err(|error| EngineError::unreadable(error.to_string()))?;
    let waits = recipe.result == ForkResult::Replace
        && !recipe.request.history.as_ref().is_some_and(|history| {
            history.mode == orch_api_contract::forks::HistoryMode::Nothing
                || history.to_turn.is_some()
        });
    let own_params = orch_settings::resolve(
        conn,
        orch_settings::Place::Node(thread.node_id),
        orch_settings_contract::registry::keys::AGENT_PARAMS,
    )
    .await?
    .own
    .map(|own| own.value);
    let model = crate::resolved::load(conn, thread.node_id).await?.model;
    Ok(Source {
        cut_id,
        thread: thread.node_id,
        view: thread.active_continuation_id,
        execution_revision: thread.execution_revision,
        queue_revision: thread.queue_revision,
        segment: thread.current_segment_id,
        native_ref,
        range,
        accepted_turn: thread.current_turn_id.filter(|_| waits),
        reach,
        topic: recipe.request.topic_id.unwrap_or(thread.topic_id),
        name: thread.name.clone(),
        links: thread.links.clone(),
        arrangement: thread.arrangement.clone(),
        own_params,
        model,
        token: caller.grant().and_then(|grant| grant.token()),
        scope: caller
            .grant()
            .map(|grant| crate::continuations::recipe::AuthorizationScope {
                scopes: grant.scopes().to_strings(),
                subtree: grant.subtree(),
            }),
    })
}

pub(super) fn retained(plan: &Json) -> Result<Source, Stop> {
    serde_json::from_value(plan.get(SOURCE).cloned().ok_or_else(|| {
        failed(
            "history_source_unconfirmed",
            "This older operation has no confirmed history source; a new source is not guessed.",
        )
    })?)
    .map_err(|_| {
        failed(
            "history_source_unconfirmed",
            "The confirmed history source cannot be read.",
        )
    })
}

async fn allowed(
    conn: &mut orch_db::Conn,
    principal: &Principal,
    node: Id,
    permission: Permission,
) -> Result<bool, Stop> {
    Ok(matches!(
        nodes::check(
            conn,
            principal,
            node,
            permission,
            Act::Use,
            orch_authz::Policy::default()
        )
        .await
        .map_err(EngineError::from)?,
        Decision::Allowed { .. }
    ))
}

async fn environment_manager(
    conn: &mut orch_db::Conn,
    job: &Job,
    principal: &Principal,
    environment: Id,
    work: Option<Id>,
) -> Result<bool, Stop> {
    if !principal.caller().permits(Permission::EnvironmentManage) {
        return Ok(false);
    }
    if principal.caller().grant().is_some() {
        let path: Option<Vec<Id>> = sqlx::query_scalar(
            "SELECT n.path FROM environments e JOIN nodes n
            ON n.id=COALESCE($2,e.home_node_id) WHERE e.id=$1",
        )
        .bind(environment)
        .bind(work)
        .fetch_optional(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if !principal.caller().reaches(path.as_deref()) {
            return Ok(false);
        }
    }
    Ok(job
        .engine()
        .inner
        .access
        .environment_manage(conn, principal.user(), environment)
        .await
        .map_err(EngineError::from)?
        .is_allowed())
}

/// Called in the native/file admission and commit transactions. Passive captured
/// reads do not grant fresh execution, and a frozen token never widens a new one.
pub(super) async fn authorize(
    conn: &mut orch_db::Conn,
    job: &Job,
    recipe: &Recipe,
    source: &Source,
    target: Option<&threads::ThreadRow>,
) -> Result<Caller, Stop> {
    let row = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    job.check()?;
    if source.cut_id != job.id()
        || source.thread != recipe.source_thread
        || source.segment != recipe.source_segment
    {
        return Err(failed(
            "history_source_changed",
            "The confirmed source belongs to another operation lane.",
        ));
    }
    let thread = threads::thread(conn, source.thread)
        .await?
        .filter(|thread| thread.deleted_at.is_none() && thread.archived_at.is_none())
        .ok_or_else(|| {
            failed(
                "source_unavailable",
                "The confirmed source is no longer readable.",
            )
        })?;
    if recipe.result == ForkResult::Replace
        && (thread.active_continuation_id != source.view
            || thread.execution_revision != source.execution_revision
            || thread.current_segment_id != source.segment)
    {
        return Err(failed(
            "history_source_changed",
            "Another execution now owns the source; this operation cannot replace it.",
        ));
    }
    if recipe.result == ForkResult::Replace
        && let Some(segment) = source.segment
    {
        let actual = threads::segment(conn, segment).await?.ok_or_else(|| {
            failed(
                "history_source_changed",
                "The captured original segment is unavailable.",
            )
        })?;
        let current = environment_rows::environment(conn, actual.environment_id)
            .await?
            .ok_or_else(|| {
                failed(
                    "history_source_changed",
                    "The captured original environment is unavailable.",
                )
            })?;
        if actual.native_session_ref != source.native_ref
            || actual.env_generation != current.generation
            || thread.agent_id != Some(actual.agent_id)
            || thread.provider_id != Some(actual.provider_id)
            || thread.environment_id != Some(actual.environment_id)
            || thread.arrangement.as_deref() != Some(actual.arrangement.as_str())
        {
            return Err(failed(
                "history_source_changed",
                "The actual original native binding or environment generation changed.",
            ));
        }
    }
    let principal =
        super::super::source::load_principal(conn, job, recipe.by, source.token).await?;
    let current = principal.caller().clone();
    let mut principals = vec![principal];
    if let Some(scope) = &source.scope {
        principals.push(
            Principal::load(
                conn,
                Caller::token(
                    recipe.by,
                    TokenGrant::from_stored(&scope.scopes, scope.subtree)
                        .map_err(EngineError::unreadable)?,
                ),
                Time::new(job.now()),
            )
            .await
            .map_err(EngineError::from)?,
        );
    }
    for principal in principals {
        let environment_reach = source.reach == crate::fork_plan::Reach::Environment
            && recipe.result == ForkResult::Replace;
        if environment_reach {
            if !environment_manager(conn, job, &principal, recipe.from_environment, None).await? {
                return Err(failed(
                    "authz.denied",
                    "Current environment management no longer permits this accepted batch member.",
                ));
            }
        } else if !allowed(conn, &principal, source.thread, Permission::ThreadView).await? {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits reading the confirmed source.",
            ));
        }
        if recipe.result != ForkResult::Replace
            && !allowed(conn, &principal, source.thread, Permission::ThreadManage).await?
            && !allowed(conn, &principal, source.thread, Permission::ThreadFork).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits this fork.",
            ));
        }
        if !environment_reach
            && recipe.result == ForkResult::Replace
            && (recipe.forkplan.from.agent.id != recipe.forkplan.to.agent.id
                || recipe.forkplan.from.provider.id != recipe.forkplan.to.provider.id
                || serde_json::to_value(recipe.forkplan.to.arrangement)
                    .map_err(EngineError::from)?
                    != serde_json::to_value(&source.arrangement).map_err(EngineError::from)?)
            && !allowed(conn, &principal, source.thread, Permission::ThreadManage).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits replacing this binding.",
            ));
        }
        if recipe.result != ForkResult::Replace
            && !allowed(conn, &principal, source.topic, Permission::TopicInteract).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits creating the target thread here.",
            ));
        }
        let target_environment = recipe.to_environment.or_else(|| {
            row.plan
                .get(made::ENVIRONMENT)
                .and_then(Json::as_str)
                .and_then(|id| id.parse::<Id>().ok())
        });
        let moving = matches!(recipe.forkplan.to.environment, ForkPlanTarget::Made { .. })
            || target_environment.is_some_and(|to| to != recipe.from_environment);
        let directory_changes = recipe.forkplan.to.directory != thread.workspace_dir;
        if recipe.result != ForkResult::Replace
            && !moving
            && !environment_manager(
                conn,
                job,
                &principal,
                recipe.from_environment,
                Some(source.topic),
            )
            .await?
        {
            return Err(failed(
                "authz.denied",
                "Current rights no longer permit placing this new thread in the source environment.",
            ));
        }
        if recipe.result == ForkResult::Replace && (moving || directory_changes)
            || recipe.result != ForkResult::Replace && moving
        {
            let decision = orch_authz::environments::check_move(
                conn,
                &principal,
                &orch_authz::environments::Moving {
                    thread: (recipe.result == ForkResult::Replace).then_some(source.thread),
                    from: (recipe.result == ForkResult::Replace).then_some(recipe.from_environment),
                    to: target_environment,
                    all_of_the_environment: environment_reach,
                },
                orch_authz::Policy::default(),
            )
            .await
            .map_err(EngineError::from)?;
            if !matches!(decision, Decision::Allowed { .. }) {
                return Err(failed(
                    "authz.denied",
                    "Current rights no longer permit this confirmed runtime placement.",
                ));
            }
        }
        if recipe.result != ForkResult::Replace
            && let Some(orch_api_contract::access::ThreadAccess::As(page)) = &recipe.request.access
            && page.source != source.thread
            && !allowed(conn, &principal, page.source, Permission::ThreadManage).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits carrying the requested permission page.",
            ));
        }
        if source.range.is_none_or(|(first, _)| first == 1)
            && let Some(parent) = thread.side_of
            && !allowed(conn, &principal, parent, Permission::ThreadView).await?
        {
            return Err(failed(
                "authz.denied",
                "The side's captured parent prefix is no longer readable.",
            ));
        }
        // Opening history authors no new input. Keep the ordinary fork/manage
        // contract rather than inventing ThreadInteract on a read-only source.
        let provider_required = !environment_reach
            && (recipe.result != ForkResult::Replace
                || recipe.forkplan.from.provider.id != recipe.forkplan.to.provider.id);
        if provider_required {
            if !principal.caller().permits(Permission::ProviderUse) {
                return Err(failed(
                    "authz.denied",
                    "The accepted credential no longer includes provider.use.",
                ));
            }
            let answer = job
                .engine()
                .inner
                .access
                .provider_use(
                    conn,
                    &crate::access::ProviderUse {
                        person: recipe.by,
                        node: target.map_or(
                            if recipe.result == ForkResult::Replace {
                                source.thread
                            } else {
                                source.topic
                            },
                            |thread| thread.node_id,
                        ),
                        provider: recipe.forkplan.to.provider.id,
                        model: Some(&recipe.forkplan.to.model),
                        effort: recipe.forkplan.to.effort.as_deref(),
                    },
                )
                .await
                .map_err(EngineError::from)?;
            match answer {
                crate::access::Answer::Refused(_) => {
                    return Err(failed(
                        "authz.denied",
                        "The accepted credential no longer permits the target provider, model or effort.",
                    ));
                }
                crate::access::Answer::Allowed {
                    arrangements: Some(allowed),
                } => {
                    let chosen: orch_agent_contract::descriptor::ArrangementType =
                        crate::sessions::convert(&recipe.forkplan.to.arrangement)?;
                    // One machine that has everything takes nobody's consent: no entry narrows it.
                    if chosen.needs_consent() && !allowed.contains(&chosen) {
                        return Err(failed(
                            "authz.denied",
                            "The current provider entry no longer permits this confirmed arrangement.",
                        ));
                    }
                }
                _ => {}
            }
        }
        let profile_required = !environment_reach
            && (recipe.result != ForkResult::Replace
                || recipe.forkplan.from.agent.id != recipe.forkplan.to.agent.id);
        if profile_required
            && (!principal.caller().permits(Permission::AgentUse)
                || !job
                    .engine()
                    .inner
                    .access
                    .agent_use(
                        conn,
                        recipe.by,
                        target.map_or(
                            if recipe.result == ForkResult::Replace {
                                source.thread
                            } else {
                                source.topic
                            },
                            |thread| thread.node_id,
                        ),
                        recipe.forkplan.to.agent.id,
                    )
                    .await
                    .map_err(EngineError::from)?
                    .is_allowed())
        {
            return Err(failed(
                "authz.denied",
                "Current and accepted rights no longer permit selecting this target agent profile.",
            ));
        }
    }
    // Birth suppliers take one Caller. Supply the actual intersection, rather
    // than silently upgrading a stored token back to its person's session.
    let Some(scope) = &source.scope else {
        return Ok(current);
    };
    let accepted = orch_access_contract::decisions::Scopes::parse(&scope.scopes)
        .map_err(EngineError::unreadable)?;
    let permissions = Permission::ALL
        .iter()
        .copied()
        .filter(|permission| accepted.allow(*permission) && current.permits(*permission))
        .collect();
    let subtree = match (
        scope.subtree,
        current.grant().and_then(|grant| grant.subtree()),
    ) {
        (Some(a), Some(b)) if a != b => {
            let a_under_b: bool = sqlx::query_scalar(
                "SELECT COALESCE((SELECT $2=ANY(path) FROM nodes WHERE id=$1),false)",
            )
            .bind(a)
            .bind(b)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            let b_under_a: bool = sqlx::query_scalar(
                "SELECT COALESCE((SELECT $2=ANY(path) FROM nodes WHERE id=$1),false)",
            )
            .bind(b)
            .bind(a)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
            if a_under_b {
                Some(a)
            } else if b_under_a {
                Some(b)
            } else {
                return Err(failed(
                    "authz.denied",
                    "Current and accepted credential subtrees no longer intersect.",
                ));
            }
        }
        (Some(a), _) | (_, Some(a)) => Some(a),
        _ => None,
    };
    let mut grant = TokenGrant::new(
        orch_access_contract::decisions::Scopes::Only(permissions),
        subtree,
    );
    if let Some(token) = source.token {
        grant = grant.for_token(token);
    }
    Ok(Caller::token(recipe.by, grant))
}

pub(super) async fn capture(
    conn: &mut orch_db::Conn,
    id: Id,
    source: &Source,
    range: Option<(i32, i32)>,
    at: DateTime<Utc>,
) -> Result<cuts::Cut, EngineError> {
    let cut = cuts::capture(
        conn,
        &cuts::Capture {
            id,
            thread: source.thread,
            view: source.view,
            job: Some(id),
            owner_thread: None,
            segment: source.segment,
            execution_revision: source.execution_revision,
            range,
            at,
        },
    )
    .await?;
    if cut.source_native_ref != source.native_ref {
        return Err(EngineError::unreadable(
            "The captured native source identity changed.",
        ));
    }
    Ok(cut)
}

impl Forking {
    pub(super) async fn source_cut(&self, job: &Job) -> Result<cuts::Cut, Stop> {
        let mut tx = job.engine().db().begin().await?;
        let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        let source = retained(&owned.plan)?;
        let thread = threads::lock(&mut tx, source.thread)
            .await?
            .ok_or_else(|| failed("source_unavailable", "The source is gone."))?;
        authorize(&mut tx, job, &self.recipe, &source, None).await?;
        if let Some(cut) = cuts::cut(&mut tx, job.id()).await? {
            if cut.thread_id != source.thread
                || cut.continuation_id != source.view
                || cut.execution_revision != source.execution_revision
                || cut.source_segment_id != source.segment
                || cut.source_native_ref != source.native_ref
            {
                return Err(failed(
                    "history_source_changed",
                    "The stored history cut has another confirmed source.",
                ));
            }
            // Existing cuts were captured at confirmation. Publish their own
            // carried boundary too, including an inherited side prefix; never
            // infer it again from a source that may already have continued.
            let range =
                (cut.last_turn >= cut.first_turn).then_some((cut.first_turn, cut.last_turn));
            let boundary = json!(range);
            if owned.plan.get(made::HISTORY_RANGE) != Some(&boundary) {
                jobs::merge_plan(
                    &mut tx,
                    job.id(),
                    &json!({made::HISTORY_RANGE:boundary}),
                    job.now(),
                )
                .await?;
            }
            tx.commit().await.map_err(orch_db::DbError::from)?;
            return Ok(cut);
        }
        if source.accepted_turn.is_none() {
            return Err(failed(
                "history_source_unconfirmed",
                "The history cut captured at confirmation is unavailable; an absent cut is not an empty history.",
            ));
        }
        let range = if let Some(accepted) = source.accepted_turn {
            let turn = threads::turn(&mut tx, accepted)
                .await?
                .filter(|turn| {
                    turn.thread_id == source.thread && Some(turn.segment_id) == source.segment
                })
                .ok_or_else(|| {
                    failed(
                        "history_source_changed",
                        "The accepted waiting turn belongs to another source.",
                    )
                })?;
            let (native_end_confirmed,pending,owner):(bool,bool,Id)=sqlx::query_as("SELECT native_end_confirmed,
                end_work IS NOT NULL OR files_end_due OR files_work_pending,continuation_id FROM turns WHERE id=$1")
                .bind(accepted).fetch_one(&mut *tx).await.map_err(orch_db::DbError::from)?;
            if owner != source.view {
                return Err(failed(
                    "history_source_changed",
                    "The accepted waiting turn belongs to another continuation.",
                ));
            }
            if matches!(turn.state.as_str(), "starting" | "running")
                || !native_end_confirmed
                || pending
                || thread.current_turn_id.is_some()
            {
                return Err(failed(
                    "history_source_busy",
                    "The accepted source turn has not completed its native/terminal work.",
                ));
            }
            let range = crate::fork_plan::history_range_view(
                &mut tx,
                source.thread,
                source.view,
                &self.recipe.request,
            )
            .await
            .map_err(|_| {
                failed(
                    "history_source_changed",
                    "The accepted source range is no longer available.",
                )
            })?;
            if range.is_some_and(|(_, through)| through > turn.ordinal) {
                return Err(failed(
                    "history_source_changed",
                    "The cut extends beyond the accepted waiting turn.",
                ));
            }
            range
        } else {
            source.range
        };
        let captured = capture(&mut tx, job.id(), &source, range, job.now()).await?;
        let range = (captured.last_turn >= captured.first_turn)
            .then_some((captured.first_turn, captured.last_turn));
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({made::HISTORY_RANGE:range}),
            job.now(),
        )
        .await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        Ok(captured)
    }
}

pub(super) fn attach(plan: &mut Json, source: &Source) -> Result<(), EngineError> {
    plan[SOURCE] = serde_json::to_value(source)?;
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct Target {
    pub thread: Id,
    pub segment: Id,
    pub selected: Id,
    pub execution_revision: i64,
    pub machine: Id,
    pub environment: Id,
    pub generation: i32,
    pub environment_machine: Option<Id>,
    pub directory: String,
    pub arrangement: String,
}

pub(super) async fn freeze_target(
    job: &Job,
    recipe: &Recipe,
    snapshot: &crate::sessions::StartSnapshot,
) -> Result<(), Stop> {
    let target = Target {
        thread: snapshot.thread.node_id,
        segment: snapshot.segment.id,
        selected: snapshot.thread.active_continuation_id,
        execution_revision: snapshot.thread.execution_revision,
        machine: snapshot.key.machine,
        environment: snapshot.environment.id,
        generation: snapshot.environment.generation,
        environment_machine: snapshot.environment.machine_id,
        directory: snapshot.directory(),
        arrangement: snapshot.thread.arrangement.clone().unwrap_or_default(),
    };
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let source = retained(&owned.plan)?;
    let thread = threads::thread(&mut tx, target.thread).await?;
    authorize(&mut tx, job, recipe, &source, thread.as_ref()).await?;
    if let Some(old) = owned.plan.get("history_target") {
        if serde_json::from_value::<Target>(old.clone()).map_err(EngineError::from)? != target {
            return Err(failed(
                "history_target_changed",
                "The retained native target changed; its original command and outcome are kept.",
            ));
        }
    } else {
        jobs::merge_plan(
            &mut tx,
            job.id(),
            &json!({"history_target":target}),
            job.now(),
        )
        .await?;
    }
    check_target(&mut tx, recipe, &target).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

pub(super) async fn check_target(
    conn: &mut orch_db::Conn,
    recipe: &Recipe,
    target: &Target,
) -> Result<(), Stop> {
    let environment = environment_rows::environment(conn, target.environment)
        .await?
        .ok_or_else(|| {
            failed(
                "history_target_changed",
                "The original target environment is unavailable.",
            )
        })?;
    let provider = resource_rows::provider(conn, recipe.forkplan.to.provider.id)
        .await?
        .ok_or_else(|| {
            failed(
                "history_target_changed",
                "The original target provider is unavailable.",
            )
        })?;
    let thread = threads::thread(conn, target.thread).await?.ok_or_else(|| {
        failed(
            "history_target_changed",
            "The original target thread is unavailable.",
        )
    })?;
    let machine = resource_rows::machine_place(conn, target.machine)
        .await?
        .ok_or_else(|| {
            failed(
                "history_target_changed",
                "The original native target machine is unavailable.",
            )
        })?;
    if environment.deleted_at.is_some()
        || environment.remove_at.is_some()
        || environment.status != "running"
        || provider.status != "ready"
        || machine.status != "active"
        || machine.deleted_at.is_some()
        || !machine.online
        || !machine.roles.iter().any(|role| role == "agent")
        || environment.generation != target.generation
        || environment.machine_id != target.environment_machine
        || harnesses::machine_of(Some(&target.arrangement), &provider, environment.machine_id)
            != Some(target.machine)
        || thread.active_continuation_id != target.selected
        || thread.execution_revision != target.execution_revision
        || recipe.new_thread.is_some()
            && (thread.current_segment_id != Some(target.segment)
                || thread.environment_id != Some(target.environment)
                || thread.workspace_dir != recipe.forkplan.to.directory
                || thread.agent_id != Some(recipe.forkplan.to.agent.id)
                || thread.provider_id != Some(recipe.forkplan.to.provider.id))
    {
        return Err(failed(
            "history_target_changed",
            "The original native target binding or environment generation changed.",
        ));
    }
    Ok(())
}

/// Freeze body, budget and actual durable queue admission under the same current
/// authority cut. V3's final dispatcher check cannot resurrect a withdrawn row.
pub(super) async fn save(
    job: &Job,
    recipe: &Recipe,
    slot: &str,
    command: Outgoing,
    attempt: Option<&Attempt>,
) -> Result<Outgoing, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let source = retained(&owned.plan)?;
    let target =
        threads::thread(&mut tx, recipe.new_thread.unwrap_or(recipe.source_thread)).await?;
    authorize(&mut tx, job, recipe, &source, target.as_ref()).await?;
    let admitted_target: Target =
        serde_json::from_value(owned.plan.get("history_target").cloned().ok_or_else(|| {
            failed(
                "history_target_unconfirmed",
                "The native target has no confirmed preparation identity.",
            )
        })?)
        .map_err(EngineError::from)?;
    check_target(&mut tx, recipe, &admitted_target).await?;
    super::recovery::intent(&mut tx, job, recipe, &owned.plan, &command).await?;
    let old = owned
        .plan
        .get("history_wire")
        .and_then(|wire| wire.get(slot));
    let fresh = old.is_none();
    let mut saved = if let Some(old) = old {
        let old: Outgoing = serde_json::from_value(old.clone()).map_err(EngineError::from)?;
        if old.id != command.id
            || old.machine != command.machine
            || old.key != command.key
            || old.kind != command.kind
            || old.body != command.body
        {
            return Err(failed(
                "history_command_changed",
                "The retained native command belongs to another preparation stage.",
            ));
        }
        old
    } else {
        command
    };
    let existing = orch_db::queries::link::command(&mut tx, saved.id).await?;
    let deadline = match (saved.deadline, existing.as_ref()) {
        (Some(at), _) => at,
        (None, Some(row)) => Time::new(row.deadline_at),
        (None, None) if fresh => {
            Time::new(job.now() + chrono::Duration::seconds(i64::from(saved.kind.deadline_secs())))
        }
        _ => {
            return Err(failed(
                "history_native_unconfirmed",
                "The original native command budget is missing; another admission is not guessed.",
            ));
        }
    };
    saved.deadline = Some(deadline);
    let key = saved.key.wire();
    orch_db::queries::link::insert_command(
        &mut tx,
        &orch_db::queries::link::NewCommand {
            id: saved.id,
            machine_id: saved.machine,
            kind: saved.kind.as_str(),
            body: &saved.body,
            serial_key: key.as_deref(),
            deadline_at: deadline.get(),
        },
        job.now(),
    )
    .await?;
    let row = orch_db::queries::link::command(&mut tx, saved.id)
        .await?
        .ok_or_else(|| {
            failed(
                "history_native_unconfirmed",
                "The admitted native command is absent.",
            )
        })?;
    if row.machine_id != saved.machine
        || row.kind != saved.kind.as_str()
        || row.body != saved.body
        || row.serial_key() != key.as_deref()
        || Time::new(row.deadline_at) != deadline
    {
        return Err(failed(
            "history_command_changed",
            "The native command differs from its original admitted identity or budget.",
        ));
    }
    cuts::hold_command(&mut tx, source.cut_id, saved.id, saved.machine).await?;
    if matches!(
        saved.kind,
        orch_link_contract::commands::CommandKind::SessionOpenFenced
            | orch_link_contract::commands::CommandKind::SessionContinuationResume
    ) {
        sqlx::query(
            "INSERT INTO thread_native_guards(thread_id) VALUES($1) ON CONFLICT DO NOTHING",
        )
        .bind(recipe.new_thread.unwrap_or(recipe.source_thread))
        .execute(&mut *tx)
        .await
        .map_err(orch_db::DbError::from)?;
    }
    let mut wire = owned
        .plan
        .get("history_wire")
        .cloned()
        .unwrap_or_else(|| json!({}));
    wire[slot] = serde_json::to_value(&saved).map_err(EngineError::from)?;
    // The longest implemented fallback path is export + three close/open
    // attempts + one handoff + one successful-child readback. Refuse unexpected stages rather than grow a job's
    // retained native body store through arbitrary retries.
    if wire.as_object().is_none_or(|wire| wire.len() > 9) {
        return Err(failed(
            "history_command_changed",
            "This preparation contains more native stages than its declared fallback path.",
        ));
    }
    let mut patch = json!({"history_wire":wire});
    if let Some(attempt) = attempt {
        let mut attempts = owned
            .plan
            .get("history_attempts")
            .cloned()
            .unwrap_or_else(|| json!({}));
        if let Some(old) = attempts.get(slot) {
            let old: Attempt = serde_json::from_value(old.clone()).map_err(EngineError::from)?;
            if old.method != attempt.method
                || old.mode != attempt.mode
                || old.body != attempt.body
                || old.model_inputs != attempt.model_inputs
                || old.instruction_sources != attempt.instruction_sources
            {
                return Err(failed(
                    "history_command_changed",
                    "The retained native opening has another history method.",
                ));
            }
        } else {
            attempts[slot] = serde_json::to_value(attempt).map_err(EngineError::from)?;
        }
        patch["history_attempts"] = attempts;
    }
    jobs::merge_plan(&mut tx, job.id(), &patch, job.now()).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(saved)
}

pub(super) async fn run(job: &Job, command: Outgoing) -> Result<Json, Stop> {
    let state = {
        let mut conn = job.engine().db().acquire().await?;
        orch_db::queries::link::command(&mut conn, command.id)
            .await?
            .map(|row| row.state)
    };
    if matches!(state.as_deref(), Some("cancelled" | "expired")) {
        return Err(failed(
            "history_native_unconfirmed",
            "The original native admission was withdrawn; its outcome must be reconciled without another child.",
        ));
    }
    let result = if command.kind == orch_link_contract::commands::CommandKind::SessionExport {
        // The archive of a native export is the size of the history and crosses the line for as long
        // as that takes: the person sees how far it has come, and a stop says what arrived.
        let watched = upload_watch::watching(
            job,
            command.machine,
            command.id,
            EXPORT_SHARE,
            job.command(command.clone()),
            Result::is_ok,
        )
        .await;
        match (watched.result, watched.reached) {
            (Err(Stop::Failed(mut failure)), Some(reached)) if failure.code == "timeout" => {
                if let Some(told) = reached.after_a_stop() {
                    failure.message = format!("{} {told}", failure.message);
                }
                Err(Stop::Failed(failure))
            }
            (result, _) => result,
        }
    } else {
        job.command(command.clone()).await
    };
    if let Some(outcome) = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .and_then(|info| info.outcome)
    {
        let mut conn = job.engine().db().acquire().await?;
        let error;
        let ending = match &outcome {
            crate::commands::Outcome::Succeeded(value) => {
                orch_db::queries::link::Ending::Succeeded(value)
            }
            crate::commands::Outcome::Failed(value) => {
                error = serde_json::to_value(value).map_err(EngineError::from)?;
                orch_db::queries::link::Ending::Failed(&error)
            }
            crate::commands::Outcome::Cancelled => orch_db::queries::link::Ending::Cancelled,
            crate::commands::Outcome::Expired => orch_db::queries::link::Ending::Expired,
        };
        orch_db::queries::link::finish_command(&mut conn, command.id, &ending, job.now()).await?;
        if matches!(&outcome, crate::commands::Outcome::Succeeded(_))
            || matches!(&outcome,
            crate::commands::Outcome::Failed(error) if error.code=="session.segment_changed"
                && command.kind==orch_link_contract::commands::CommandKind::SessionCloseFenced)
            || matches!(&outcome,
            crate::commands::Outcome::Failed(error) if error.code=="unsupported" && matches!(command.kind,
                orch_link_contract::commands::CommandKind::SessionOpenFenced|orch_link_contract::commands::CommandKind::SessionContinuationResume))
        {
            cuts::settle_command(&mut conn, job.id(), command.id).await?;
        }
    }
    result
}

/// Retaining an already produced result is factual housekeeping. Starting its
/// producer or admitting a native consumer still needs current/frozen authority.
pub(super) async fn artifact(
    job: &Job,
    recipe: &Recipe,
    name: &str,
    value: &Json,
    factual: bool,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owned = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let source = retained(&owned.plan)?;
    if !factual {
        let target =
            threads::thread(&mut tx, recipe.new_thread.unwrap_or(recipe.source_thread)).await?;
        authorize(&mut tx, job, recipe, &source, target.as_ref()).await?;
    }
    let cut = cuts::cut(&mut tx, job.id())
        .await?
        .ok_or_else(|| failed("history_source_unconfirmed", "The confirmed cut is absent."))?;
    if cut.thread_id != source.thread
        || cut.continuation_id != source.view
        || cut.job_id != Some(job.id())
    {
        return Err(failed(
            "history_source_changed",
            "This artifact belongs to another confirmed source.",
        ));
    }
    if !cuts::keep_artifact(&mut tx, job.id(), name, value, &[]).await? {
        return Err(failed(
            "history_artifact_changed",
            "This preparation stage already owns a different immutable result.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// A native export is an admitted read, not another child. Pin its actual
/// uploaded identity before any close/import admission; a missing/collected
/// upload refuses here while the original lane is still untouched.
pub(super) async fn exported(
    job: &Job,
    name: &str,
    archive: &SessionExportResult,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let blob = orch_db::queries::storage::blob(&mut tx, &archive.blob)
        .await?
        .ok_or_else(|| {
            failed(
                "history_unavailable",
                "The native export's uploaded bytes are unavailable; no import was admitted.",
            )
        })?;
    if blob.size != archive.size {
        return Err(failed(
            "history_native_identity",
            "The native export's uploaded size changed.",
        ));
    }
    if !cuts::keep_artifact(
        &mut tx,
        job.id(),
        name,
        &serde_json::to_value(archive).map_err(EngineError::from)?,
        &[archive.blob],
    )
    .await?
    {
        return Err(failed(
            "history_native_identity",
            "The native export already retained another uploaded identity.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Save an actual successful native child before any further verifier, current
/// authorization check or cancellable metadata publication can fail.
pub(super) async fn child_result(
    job: &Job,
    command: &Outgoing,
    result: &SessionOpenResult,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let name = format!("native_result_{}", command.id);
    if !cuts::keep_artifact(
        &mut tx,
        job.id(),
        &name,
        &json!({"command":command,"result":result}),
        &[],
    )
    .await?
    {
        return Err(failed(
            "history_native_identity",
            "The original creation already retained another native child.",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
