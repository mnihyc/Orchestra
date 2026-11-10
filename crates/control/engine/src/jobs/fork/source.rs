//! Confirmed full-history source/caller, independent of later presentation or selection.

use super::*;
use crate::jobs::upload_watch;
use orch_access_contract::{decisions::Decision, permissions::Permission};
use orch_authz::{Act, Principal, TokenGrant, nodes};
use orch_db::queries::{jobs, thread_history_cuts as cuts};

const SOURCE: &str = "confirmed_history_source";
fn managed_error(error: EngineError) -> Stop {
    match error {
        EngineError::BeforeEffect(_) => failed(
            "authz.denied",
            "The original native operation is no longer permitted.",
        ),
        error => error.into(),
    }
}

/// How much of the history phase the export of a native archive is, while its bytes cross the line: half.
/// The file crosses twice, up from the machine that holds the session and down to the one that opens
/// it, and the opening that follows imports what came; the other half is left to those.
const EXPORT_SHARE: f64 = 0.5;

/// Ordinary confirmations have one caller; automatic input recovery retains every origin.
pub(super) enum Authority<'a> {
    Direct(&'a Caller),
    Managed {
        caller: &'a Caller,
        origin: &'a crate::fork_plan::operation::Origin,
    },
    Agent {
        caller: &'a Caller,
        thread: Id,
        turn: Id,
    },
    Recovery {
        input_id: Id,
        callers: &'a crate::inputs::InputCallers,
    },
}

impl Authority<'_> {
    pub(super) fn actor(&self) -> Id {
        match self {
            Self::Direct(caller) | Self::Agent { caller, .. } | Self::Managed { caller, .. } => {
                caller.user()
            }
            Self::Recovery { callers, .. } => callers.user(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct RecoveryInputAuthority {
    input_id: Id,
    captured: crate::inputs::InputAuthority,
}

pub(super) enum Authorized {
    Direct(Caller),
    Managed {
        principal: Principal,
        origin: Box<crate::fork_plan::operation::Origin>,
    },
    Lifecycle {
        principal: Principal,
        until: Option<Time>,
    },
    Recovery,
}

impl Authorized {
    pub(super) fn check(&self, clock: &dyn orch_types::Clock) -> Result<(), EngineError> {
        if let Self::Managed { principal, origin } = self {
            origin.deadline(clock, principal.caller())?;
        }
        if let Self::Lifecycle { principal, until } = self
            && (!principal.caller().key_deadline_allows(clock)
                || until.is_some_and(|until| clock.time() >= until))
        {
            return Err(EngineError::BeforeEffect(Box::new(
                crate::operation_scope::continuing::Error::Refused,
            )));
        }
        Ok(())
    }
    /// The bounded recovery producer never creates a thread or environment. Do not
    /// collapse its origins to satisfy an unrelated single-caller birth interface.
    pub(super) fn creation_caller(&self) -> Result<Caller, Stop> {
        match self {
            Self::Direct(caller) => Ok(caller.clone()),
            Self::Managed { principal, .. } | Self::Lifecycle { principal, .. } => {
                Ok(principal.caller().clone())
            }
            Self::Recovery => Err(failed(
                "recovery_shape_changed",
                "An input recovery cannot create a new thread or environment.",
            )),
        }
    }
}

fn recovery_shape_matches(
    recipe: &Recipe,
    thread: &threads::ThreadRow,
    reach: crate::fork_plan::Reach,
) -> bool {
    recipe.result == ForkResult::Replace
        && recipe.request.result == ForkResult::Replace
        && recipe.forkplan.result == ForkResult::Replace
        && recipe.new_thread.is_none()
        && recipe.batch_id.is_none()
        && reach == crate::fork_plan::Reach::Seen
        && matches!(
            recipe.request.environment,
            None | Some(ForkEnvironment::Same { same: true })
        )
        && recipe.request.topic_id.is_none()
        && recipe.request.name.is_none()
        && recipe.request.access.is_none()
        && recipe.request.agent_access.is_none()
        && recipe.request.directory.is_none()
        && recipe.request.cwd.is_none()
        && recipe.request.mounts.is_none()
        && recipe.request.worktree.is_none()
        && recipe.request.files_at.is_none()
        && recipe.forkplan.files.is_none()
        && recipe.forkplan.worktree.is_none()
        && recipe.forkplan.files_at.is_none()
        && recipe.source_execution.environment().is_some()
        && recipe.to_environment == recipe.source_execution.environment()
        && matches!(&recipe.forkplan.to.environment, ForkPlanTarget::Existing(environment) if Some(environment.id) == recipe.source_execution.environment())
        && recipe.forkplan.to.directory == thread.workspace_dir
}

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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_input: Option<RecoveryInputAuthority>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_thread: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<crate::fork_plan::operation::Origin>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    operation_version: Option<u8>,
}

pub(super) async fn confirmed(
    conn: &mut orch_db::Conn,
    thread: &threads::ThreadRow,
    authority: &Authority<'_>,
    recipe: &Recipe,
    cut_id: Id,
    reach: crate::fork_plan::Reach,
) -> Result<Source, EngineError> {
    let (token, scope, recovery_input) = match authority {
        Authority::Managed { caller, origin } => {
            if origin.captured().user() != caller.user()
                || !origin.matches_job(thread.node_id, &recipe.request, reach, recipe.batch_id)
            {
                return Err(EngineError::unreadable(
                    "the confirmed native operation has another source",
                ));
            }
            (None, None, None)
        }
        Authority::Direct(caller) | Authority::Agent { caller, .. } => (
            caller.grant().and_then(|grant| grant.token()),
            caller
                .grant()
                .map(|grant| crate::continuations::recipe::AuthorizationScope {
                    scopes: grant.scopes().to_strings(),
                    subtree: grant.subtree(),
                }),
            None,
        ),
        Authority::Recovery { input_id, callers } => {
            if !recovery_shape_matches(recipe, thread, reach) {
                return Err(EngineError::unreadable(
                    "the input recovery changed its bounded switch shape",
                ));
            }
            (
                None,
                None,
                Some(RecoveryInputAuthority {
                    input_id: *input_id,
                    captured: callers.captured().clone(),
                }),
            )
        }
    };
    let native_ref = match (
        recipe.source_execution.environment(),
        thread.current_segment_id,
    ) {
        (Some(_), Some(id)) => threads::segment(conn, id)
            .await?
            .and_then(|segment| segment.native_session_ref),
        _ => None,
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
    let model = match &recipe.forkplan.from {
        orch_api_contract::forks::ForkPlanFrom::Program { model, .. } => Some(model.clone()),
        _ => crate::resolved::load(conn, thread.node_id).await?.model,
    };
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
        token,
        scope,
        recovery_input,
        agent_thread: match authority {
            Authority::Agent { thread, .. } => Some(*thread),
            _ => None,
        },
        operation: match authority {
            Authority::Managed { origin, .. } => Some((**origin).clone()),
            _ => None,
        },
        operation_version: matches!(authority, Authority::Managed { .. }).then_some(1),
    })
}

pub(super) fn retained(plan: &Json) -> Result<Source, Stop> {
    if plan
        .get(SOURCE)
        .is_some_and(|value| value.get("operation").is_some_and(Json::is_null))
    {
        return Err(failed(
            "authz.denied",
            "The retained native operation source is malformed.",
        ));
    }
    let retained: Source = serde_json::from_value(plan.get(SOURCE).cloned().ok_or_else(|| {
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
    })?;
    if !matches!(
        (&retained.operation, retained.operation_version),
        (None, None) | (Some(_), Some(1))
    ) {
        return Err(failed(
            "authz.denied",
            "The retained native operation source is incomplete.",
        ));
    }
    Ok(retained)
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
    access: &dyn crate::access::ResourceAccess,
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
    Ok(access
        .environment_manage(conn, principal.user(), environment)
        .await
        .map_err(EngineError::from)?
        .is_allowed())
}

pub(super) async fn lock_operation_actor(
    conn: &mut orch_db::Conn,
    id: Id,
) -> Result<(), EngineError> {
    if let Some(row) = jobs::job(conn, id).await? {
        crate::jobs::lifecycle_commands::environment::lock_source(conn, &row).await?;
    }
    if let Some(value) = jobs::job(conn, id).await?.and_then(|row| {
        row.plan
            .get(SOURCE)
            .and_then(|source| source.get("operation"))
            .cloned()
    }) {
        let origin: crate::fork_plan::operation::Origin = serde_json::from_value(value)?;
        orch_db::queries::transaction_guard::lock_native_actor(conn, origin.captured().user())
            .await?;
    }
    Ok(())
}

/// Called in the native/file admission and commit transactions. Passive captured
/// reads do not grant fresh execution, and a frozen token never widens a new one.
pub(super) async fn authorize(
    conn: &mut orch_db::Conn,
    job: &Job,
    recipe: &Recipe,
    source: &Source,
    target: Option<&threads::ThreadRow>,
) -> Result<Authorized, Stop> {
    lock_operation_actor(conn, job.id()).await?;
    let row = job.lock_owned(conn).await?.ok_or(Stop::Lost)?;
    if source.operation.is_some() {
        threads::lock(conn, source.thread)
            .await?
            .ok_or(Stop::Lost)?;
    }
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    job.check()?;
    let authorized = authorize_on(
        conn,
        &row,
        recipe,
        source,
        target,
        job.engine().inner.clock.as_ref(),
        job.engine().inner.access.as_ref(),
    )
    .await?;
    if let Authorized::Managed { principal, origin } = &authorized {
        retain_operation(
            conn,
            &row,
            principal.caller(),
            origin,
            job.engine().inner.clock.as_ref(),
        )
        .await?;
    }
    Ok(authorized)
}

pub(super) async fn retain_operation(
    conn: &mut orch_db::Conn,
    row: &JobRow,
    caller: &Caller,
    origin: &crate::fork_plan::operation::Origin,
    clock: &dyn orch_types::Clock,
) -> Result<(), Stop> {
    origin.deadline(clock, caller).map_err(managed_error)?;
    let mut current = retained(&row.plan)?;
    current.operation = Some(origin.clone());
    jobs::merge_plan(conn, row.id, &json!({(SOURCE):current}), clock.now()).await?;
    origin.deadline(clock, caller).map_err(managed_error)?;
    Ok(())
}

/// The same native authority gate on an executor's actual row, clock and resource access.
#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "one existing native authority gate keeps all of its original prerequisites and runtime ports"
)]
pub(super) async fn authorize_on(
    conn: &mut orch_db::Conn,
    row: &JobRow,
    recipe: &Recipe,
    source: &Source,
    target: Option<&threads::ThreadRow>,
    clock: &dyn orch_types::Clock,
    access: &dyn crate::access::ResourceAccess,
) -> Result<Authorized, Stop> {
    if source.cut_id != row.id
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
        && recipe.source_execution.environment().is_some()
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
    if let ExecutionSource::Program {
        program,
        root_org_id,
    } = &recipe.source_execution
    {
        let current = if recipe.result == ForkResult::New
            && let Some(segment) = source.segment
        {
            crate::fork_plan::program::execution_at(conn, thread.node_id, segment).await
        } else {
            crate::fork_plan::program::execution(conn, &thread).await
        }
        .map_err(|_| {
            failed(
                "history_source_changed",
                "The captured program binding is unavailable",
            )
        })?;
        let Some((current, provider, model)) = current else {
            return Err(failed(
                "history_source_changed",
                "The source is no longer the confirmed built-in program",
            ));
        };
        if current.id != *program
            || current.root_org_id != *root_org_id
            || provider.id != recipe.forkplan.from.provider().id
            || source.model.as_ref() != Some(&model)
        {
            return Err(failed(
                "history_source_changed",
                "The captured program, provider or model changed",
            ));
        }
    }
    if source.agent_thread != recipe.by_agent {
        return Err(failed(
            "authz.denied",
            "The accepted agent fork authority changed.",
        ));
    }
    if let Some(agent_thread) = source.agent_thread {
        if recipe.result != ForkResult::New || source.recovery_input.is_some() {
            return Err(failed(
                "authz.denied",
                "An agent confirmation permits only a new fork.",
            ));
        }
        let has_origin = if let Some(turn) = recipe.by_agent_turn {
            sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM turns WHERE id=$1 AND thread_id=$2)",
            )
            .bind(turn)
            .bind(agent_thread)
            .fetch_one(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
        } else {
            false
        };
        if !has_origin {
            return Err(failed(
                "authz.denied",
                "The accepted agent fork's original turn is unavailable.",
            ));
        }
        let agent = orch_authz::reach::Agent::of(conn, agent_thread)
            .await
            .map_err(EngineError::from)?
            .ok_or_else(|| {
                failed(
                    "authz.denied",
                    "The agent that confirmed this fork is unavailable.",
                )
            })?;
        let reach = orch_authz::reach::Reach::load_current(
            conn,
            agent,
            clock.time(),
            &orch_authz::reach::Things {
                nodes: vec![source.thread],
                ..Default::default()
            },
        )
        .await
        .map_err(EngineError::from)?;
        if !reach.thread_act_by(
            source.thread,
            thread.side_of.is_some(),
            Permission::ThreadFork,
            recipe.by,
        ) {
            return Err(failed(
                "authz.denied",
                "The accepted giver no longer supplies thread.fork to this agent.",
            ));
        }
    }
    let mut managed = None;
    let mut lifecycle = None;
    let (principals, current) = if crate::jobs::lifecycle_commands::environment::present(row) {
        if source.operation.is_some()
            || source.recovery_input.is_some()
            || source.agent_thread.is_some()
        {
            return Err(failed(
                "authz.denied",
                "The lifecycle child has another native authority.",
            ));
        }
        let checked =
            crate::jobs::lifecycle_commands::environment::current(conn, row, clock, access, None)
                .await
                .map_err(managed_error)?;
        let caller = checked.principal.caller().clone();
        lifecycle = Some(checked.until);
        (vec![checked.principal], Some(caller))
    } else if let Some(operation) = &source.operation {
        if source.token.is_some()
            || source.scope.is_some()
            || source.recovery_input.is_some()
            || source.agent_thread.is_some()
            || !operation.matches_job(
                source.thread,
                &recipe.request,
                source.reach,
                recipe.batch_id,
            )
            || operation.captured().user() != recipe.by
        {
            return Err(failed(
                "authz.denied",
                "The retained native operation changed its source.",
            ));
        }
        let current = retained(&row.plan)?.operation.ok_or_else(|| {
            failed(
                "authz.denied",
                "The original native operation source is missing.",
            )
        })?;
        let (principal, origin) = operation
            .current_act(conn, clock, &current)
            .await
            .map_err(managed_error)?;
        let caller = principal.caller().clone();
        managed = Some(origin);
        (vec![principal], Some(caller))
    } else if let Some(recovery) = &source.recovery_input {
        if source.token.is_some()
            || source.scope.is_some()
            || row.plan.get("copy_worktree").and_then(Json::as_bool) != Some(false)
            || !recovery_shape_matches(recipe, &thread, source.reach)
        {
            return Err(failed(
                "recovery_shape_changed",
                "The retained input recovery changed its bounded switch shape.",
            ));
        }
        let input = threads::input(conn, recovery.input_id)
            .await?
            .filter(|input| input.thread_id == source.thread && input.author_id == Some(recipe.by))
            .ok_or_else(|| {
                failed(
                    "origin_authority",
                    "The accepted recovery input is unavailable.",
                )
            })?;
        let current_authority = crate::inputs::saved_authority(
            input.submission_authority.as_ref(),
        )?
        .ok_or_else(|| {
            failed(
                "origin_authority",
                "The recovery input's current authority is unavailable.",
            )
        })?;
        let captured = recovery
            .captured
            .retaining_current(recipe.by, &current_authority)?;
        let authority = {
            let mut unit = Unit::new(conn, clock.now());
            crate::inputs::input_authority_callers(
                &mut unit, &thread, &input, recipe.by, &captured, clock,
            )
            .await?
            .ok_or_else(|| {
                failed(
                    "origin_authority",
                    "The accepted recovery credentials are no longer available.",
                )
            })?
        };
        let mut principals = Vec::with_capacity(authority.callers().len());
        for caller in authority.callers() {
            principals.push(
                Principal::load(conn, caller.clone(), clock.time())
                    .await
                    .map_err(EngineError::from)?,
            );
        }
        (principals, None)
    } else {
        if let Some(token) = source.token {
            let scoped: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM api_tokens WHERE id=$1 AND operation_scope IS NOT NULL)")
                .bind(token).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
            if scoped {
                return Err(failed(
                    "authz.denied",
                    "This scoped native operation has no retained original source.",
                ));
            }
        }
        let principal =
            super::super::source::load_principal_at(conn, recipe.by, source.token, clock.time())
                .await?;
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
                    clock.time(),
                )
                .await
                .map_err(EngineError::from)?,
            );
        }
        (principals, Some(current))
    };
    for principal in &principals {
        if recipe.forkplan.files.is_some()
            || recipe.forkplan.files_at.is_some()
            || recipe.forkplan.worktree.is_some()
        {
            // Files are copied only from a source with a real workspace; a program source has none.
            let from_environment = recipe
                .source_execution
                .environment()
                .ok_or_else(|| failed("authz.denied", "The file source is unavailable."))?;
            let destination = match recipe.to_environment.or_else(|| {
                row.plan
                    .get(made::ENVIRONMENT)
                    .and_then(Json::as_str)
                    .and_then(|id| id.parse::<Id>().ok())
            }) {
                Some(id) => {
                    orch_db::queries::environments::thread_engine::environment(conn, id)
                        .await?
                        .ok_or_else(|| {
                            failed("authz.denied", "The file destination is unavailable.")
                        })?
                        .home_node_id
                }
                None => match &recipe.request.environment {
                    Some(ForkEnvironment::New { new }) => new.home_node_id.unwrap_or(source.topic),
                    _ => {
                        orch_db::queries::environments::thread_engine::environment(
                            conn,
                            from_environment,
                        )
                        .await?
                        .ok_or_else(|| failed("authz.denied", "The file source is unavailable."))?
                        .home_node_id
                    }
                },
            };
            match crate::copy_out::authorize(
                conn,
                principal,
                crate::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Environment,
                    id: from_environment,
                }),
                Some(destination),
            )
            .await
            {
                Ok(_) => {}
                Err(crate::copy_out::Error::Native(error)) => return Err(error.into()),
                Err(_) => {
                    return Err(failed(
                        "authz.denied",
                        "The source environment no longer permits files to be copied into this organization.",
                    ));
                }
            }
        }
        if recipe.result != ForkResult::Replace {
            match crate::copy_out::authorize(
                conn,
                principal,
                crate::copy_out::Subject::Object(orch_access_contract::decisions::ObjectRef {
                    kind: orch_access_contract::permissions::ObjectKind::Thread,
                    id: source.thread,
                }),
                Some(source.topic),
            )
            .await
            {
                Ok(_) => {}
                Err(crate::copy_out::Error::Native(error)) => return Err(error.into()),
                Err(_) => {
                    return Err(failed(
                        "authz.denied",
                        "The source no longer permits a copy into this organization.",
                    ));
                }
            }
        }
        let environment_reach = source.reach == crate::fork_plan::Reach::Environment
            && recipe.result == ForkResult::Replace;
        if environment_reach {
            let environment = recipe.source_execution.environment().ok_or_else(|| {
                failed(
                    "authz.denied",
                    "A program has no source environment management authority",
                )
            })?;
            if !environment_manager(conn, access, principal, environment, None).await? {
                return Err(failed(
                    "authz.denied",
                    "Current environment management no longer permits this accepted batch member.",
                ));
            }
        } else if !allowed(conn, principal, source.thread, Permission::ThreadView).await? {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits reading the confirmed source.",
            ));
        }
        if recipe.result != ForkResult::Replace
            && !allowed(conn, principal, source.thread, Permission::ThreadManage).await?
            && !allowed(conn, principal, source.thread, Permission::ThreadFork).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits this fork.",
            ));
        }
        if !environment_reach
            && recipe.result == ForkResult::Replace
            && (recipe.forkplan.from.agent().map(|agent| agent.id)
                != Some(recipe.forkplan.to.agent.id)
                || recipe.forkplan.from.provider().id != recipe.forkplan.to.provider.id
                || serde_json::to_value(recipe.forkplan.to.arrangement)
                    .map_err(EngineError::from)?
                    != serde_json::to_value(&source.arrangement).map_err(EngineError::from)?)
            && !allowed(conn, principal, source.thread, Permission::ThreadManage).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits replacing this binding.",
            ));
        }
        if recipe.result != ForkResult::Replace
            && !allowed(conn, principal, source.topic, Permission::TopicStartThreads).await?
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
            || target_environment
                .is_some_and(|to| Some(to) != recipe.source_execution.environment());
        let directory_changes = recipe.forkplan.to.directory != thread.workspace_dir;
        if recipe.result != ForkResult::Replace
            && !moving
            && !environment_manager(
                conn,
                access,
                principal,
                recipe.source_execution.environment().ok_or_else(|| {
                    failed(
                        "authz.denied",
                        "A program has no source environment to place another thread in",
                    )
                })?,
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
                principal,
                &orch_authz::environments::Moving {
                    thread: (recipe.result == ForkResult::Replace).then_some(source.thread),
                    from: if recipe.result == ForkResult::Replace {
                        recipe.source_execution.environment()
                    } else {
                        None
                    },
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
            && !allowed(conn, principal, page.source, Permission::ThreadManage).await?
        {
            return Err(failed(
                "authz.denied",
                "The accepted credential no longer permits carrying the requested permission page.",
            ));
        }
        if source.range.is_none_or(|(first, _)| first == 1)
            && let Some(parent) = thread.side_of
            && !allowed(conn, principal, parent, Permission::ThreadView).await?
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
                || matches!(recipe.source_execution, ExecutionSource::Program { .. })
                || recipe.forkplan.from.provider().id != recipe.forkplan.to.provider.id);
        if provider_required {
            if !principal.caller().permits(Permission::ProviderUse) {
                return Err(failed(
                    "authz.denied",
                    "The accepted credential no longer includes provider.use.",
                ));
            }
            let answer = access
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
                || recipe.forkplan.from.agent().map(|agent| agent.id)
                    != Some(recipe.forkplan.to.agent.id));
        if profile_required
            && (!principal.caller().permits(Permission::AgentUse)
                || !access
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
    let Some(current) = current else {
        return Ok(Authorized::Recovery);
    };
    if let Some(until) = lifecycle {
        let principal = principals.into_iter().next().ok_or(Stop::Lost)?;
        let authorized = Authorized::Lifecycle { principal, until };
        authorized.check(clock).map_err(managed_error)?;
        return Ok(authorized);
    }
    if let Some(origin) = managed {
        origin.deadline(clock, &current).map_err(managed_error)?;
        let principal = principals.into_iter().next().ok_or(Stop::Lost)?;
        return Ok(Authorized::Managed {
            principal,
            origin: Box::new(origin),
        });
    }
    // Birth suppliers take one Caller. Supply the actual intersection, rather
    // than silently upgrading a stored token back to its person's session.
    let Some(scope) = &source.scope else {
        return Ok(Authorized::Direct(current));
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
    Ok(Authorized::Direct(Caller::token(recipe.by, grant)))
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
        lock_operation_actor(&mut tx, job.id()).await?;
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
            // A control-plane program has no native harness to acknowledge its end.
            // Its settled turn and cleared current execution are the source boundary.
            let needs_native_end = !matches!(
                &self.recipe.source_execution,
                ExecutionSource::Program { .. }
            );
            if matches!(turn.state.as_str(), "starting" | "running")
                || needs_native_end && !native_end_confirmed
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
    lock_operation_actor(&mut tx, job.id()).await?;
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
        || !crate::providers::request_status_available(&provider.status)
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
    let command = job.prepare_command(command).await?;
    let mut tx = job.engine().db().begin().await?;
    lock_operation_actor(&mut tx, job.id()).await?;
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
    if !fresh && source.operation.is_some() && saved.authority.is_none() {
        return Err(failed(
            "authz.denied",
            "The known native command lost its original source.",
        ));
    }
    let current = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    saved = super::command::history(&current, slot, saved)?;
    let private = saved
        .authority
        .as_ref()
        .map(serde_json::to_value)
        .transpose()
        .map_err(EngineError::from)?;
    let budget = super::operation::Preparation::from_job(&current)?;
    let accepted = match &budget {
        Some(source) => Some(
            source
                .current(
                    &mut tx,
                    job.engine().inner.clock.as_ref(),
                    job.engine().inner.access.as_ref(),
                )
                .await
                .map_err(managed_error)?,
        ),
        None => None,
    };
    if let Some(accepted) = &accepted {
        accepted
            .check(job.engine().inner.clock.as_ref())
            .map_err(managed_error)?;
    }
    let key = saved.key.wire();
    orch_db::queries::link::insert_command_with_authority(
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
        private.as_ref(),
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
    if orch_db::queries::link::command_authority(&mut tx, saved.id).await? != private {
        return Err(failed(
            "authz.denied",
            "The native command retained another original source.",
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
    if let Some(accepted) = &accepted {
        accepted
            .check(job.engine().inner.clock.as_ref())
            .map_err(managed_error)?;
    }
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
    lock_operation_actor(&mut tx, job.id()).await?;
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
    lock_operation_actor(&mut tx, job.id()).await?;
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
    lock_operation_actor(&mut tx, job.id()).await?;
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
