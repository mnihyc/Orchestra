//! The actual queued environment update keeps its review and confirmation independently.
use super::*;
use crate::updates::operation::Input as Origin;
use orch_api_contract::environment_sources::update_batch as declarations;
use orch_authz::Principal;
use orch_types::{Clock, Hash, Time};
use serde::{Deserialize, Serialize};

/// Private native association; presence can never fall back to an ordinary replacement.
pub const BINDING: &str = "environment_update_binding";
pub(crate) const COMMANDS: &str = "environment_update_batch_commands";
pub(crate) const NOTICES: &str = "environment_update_batch_notice_commands";
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Accepted {
    review: Option<Origin>,
    confirmation: Origin,
    batch: Id,
    recipe: Hash,
    old_host: Id,
    old_machine: Option<Id>,
    old_generation: i32,
    old_template: Id,
    old_driver: String,
}
/// Recognized modern association, including malformed or partly missing metadata.
pub fn is_restricted(row: &jobs::JobRow) -> bool {
    [BINDING, COMMANDS, NOTICES]
        .iter()
        .any(|key| row.plan.get(*key).is_some())
}
fn read(row: &jobs::JobRow) -> Result<Option<Accepted>, EngineError> {
    row.plan
        .get(BINDING)
        .cloned()
        .map(serde_json::from_value)
        .transpose()
        .map_err(Into::into)
}
fn recipe_hash(recipe: &Recipe) -> Result<Hash, EngineError> {
    super::super::command_identity::hash(&serde_json::to_value(recipe)?)
}
fn valid_origins(accepted: &Accepted, recipe: &Recipe) -> bool {
    accepted.confirmation.origin().operation() == declarations::GLOBAL_CREATE.operation
        && accepted.confirmation.origin().user() == recipe.by
        && accepted.confirmation.environment() == recipe.target.id()
        && accepted.review.as_ref().is_none_or(|review| {
            review.origin().operation() == declarations::GLOBAL_PREFLIGHT.operation
                && review.origin().user() == recipe.by
                && review.environment() == recipe.target.id()
        })
}
/// Capture only a genuine newly queued member after its original review and confirmation passed.
pub async fn retain(
    conn: &mut Conn,
    row: &jobs::JobRow,
    batch: Id,
    review: Option<Origin>,
    confirmation: Origin,
    now: Time,
) -> Result<(), EngineError> {
    let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    let Target::Environment(environment) = recipe.target else {
        return Err(EngineError::unreadable("not an environment replacement"));
    };
    let old = envs::runtime(conn, recipe.old)
        .await?
        .ok_or_else(|| EngineError::unreadable("the reviewed runtime disappeared"))?;
    let accepted = Accepted {
        review,
        confirmation: confirmation.for_execution(recipe.template),
        batch,
        recipe: recipe_hash(&recipe)?,
        old_host: old.host_id,
        old_machine: old.machine_id,
        old_generation: old.generation,
        old_template: old.template_id,
        old_driver: old.driver,
    };
    if row.kind != "environment_recreate"
        || row.environment_id != Some(environment)
        || row.provider_id.is_some()
        || row.batch_id != Some(batch)
        || row.state != "queued"
        || row.created_by != Some(recipe.by)
        || recipe.auto_start
        || recipe.capability.is_some()
        || recipe.reason != UpdatePreflightReason::Update
        || !valid_origins(&accepted, &recipe)
        || !accepted
            .confirmation
            .matches(conn)
            .await
            .map_err(EngineError::unreadable)?
        || accepted
            .review
            .as_ref()
            .is_some_and(|review| !review.same_source(&accepted.confirmation))
    {
        return Err(EngineError::unreadable(
            "the native update member changed before retaining its origin",
        ));
    }
    jobs::merge_plan(conn, row.id, &json!({BINDING:accepted}), now.get())
        .await
        .map_err(Into::into)
}
/// The exact original old/candidate pair, not a terminal-state substitute for native receipt evidence.
pub(crate) async fn structure(
    conn: &mut Conn,
    row: &jobs::JobRow,
    forward: bool,
) -> Result<Option<(Recipe, envs::RuntimeFull, Option<envs::RuntimeFull>)>, EngineError> {
    let Some(accepted) = read(row)? else {
        return Ok(None);
    };
    let mut recipe: Recipe = serde_json::from_value(row.plan.clone())?;
    // The native replacement records its actual current host beside its original recipe.
    recipe.host = None;
    let Target::Environment(environment) = recipe.target else {
        return Ok(None);
    };
    if row.kind != "environment_recreate"
        || row.environment_id != Some(environment)
        || row.provider_id.is_some()
        || row.created_by != Some(recipe.by)
        || row.batch_id != Some(accepted.batch)
        || recipe_hash(&recipe)? != accepted.recipe
        || !valid_origins(&accepted, &recipe)
        || (forward
            && (!row.is_open()
                || row.cancel_requested()
                || crate::jobs::runner::abort_reason(row)?.is_some()))
    {
        return Ok(None);
    }
    envs::lock_environment(conn, environment)
        .await?
        .ok_or_else(|| EngineError::unreadable("the update environment disappeared"))?;
    if row
        .plan
        .get("host")
        .is_some_and(|host| !host.is_null() && *host != json!(accepted.old_host))
    {
        return Ok(None);
    }
    let next: Option<Id> = row
        .plan
        .get("runtime")
        .filter(|value| !value.is_null())
        .cloned()
        .map(serde_json::from_value)
        .transpose()?;
    let ids:Vec<Id>=sqlx::query_scalar("SELECT id FROM runtimes WHERE environment_id=$1 AND kind='environment' AND (id=$2 OR id=$3) ORDER BY id FOR UPDATE").bind(environment).bind(recipe.old).bind(next).fetch_all(&mut *conn).await.map_err(DbError::from)?;
    if !ids.contains(&recipe.old)
        || next.is_some_and(|next| next == recipe.old || !ids.contains(&next))
    {
        return Ok(None);
    }
    let Some(old) = envs::runtime(conn, recipe.old).await?.filter(|old| {
        old.environment_id == Some(environment)
            && old.provider_id.is_none()
            && old.kind == "environment"
            && old.host_id == accepted.old_host
            && old.machine_id == accepted.old_machine
            && old.generation == accepted.old_generation
            && old.template_id == accepted.old_template
            && old.driver == accepted.old_driver
    }) else {
        return Ok(None);
    };
    let candidate = match next {
        None => None,
        Some(next) => {
            let Some(candidate) = envs::runtime(conn, next).await?.filter(|candidate| {
                candidate.environment_id == Some(environment)
                    && candidate.provider_id.is_none()
                    && candidate.kind == "environment"
                    && candidate.successor_of == Some(old.id)
                    && candidate.generation == old.generation.saturating_add(1)
                    && candidate.host_id == old.host_id
                    && candidate.template_id == recipe.template
                    && candidate.driver == old.driver
                    && candidate.spec_hash == recipe.hash
                    && (!forward || !matches!(candidate.state.as_str(), "retired" | "removed"))
            }) else {
                return Ok(None);
            };
            if row.plan.get("host") != Some(&json!(old.host_id)) {
                return Ok(None);
            }
            Some(candidate)
        }
    };
    if forward
        && envs::live_runtime(conn, environment)
            .await?
            .is_none_or(|runtime| runtime.id != old.id)
    {
        return Ok(None);
    }
    Ok(Some((recipe, old, candidate)))
}
/// Actual current raw/key minima and ordinary management at the owned native effect boundary.
pub async fn current(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
    let Some((recipe, old, _)) = structure(conn, row, true).await? else {
        return Ok(None);
    };
    let Some(accepted) = read(row)? else {
        return Ok(None);
    };
    let mut callers = Vec::new();
    for origin in accepted
        .review
        .iter()
        .chain(std::iter::once(&accepted.confirmation))
    {
        let current = match origin.current_authority(conn, clock).await {
            Ok(current) => current,
            Err(EngineError::BeforeEffect(error))
                if error.is::<crate::updates::operation::Ended>()
                    || error.is::<crate::updates::operation::Denied>() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        let mut principal = Principal::load(conn, current.caller, clock.time()).await?;
        let user = orch_db::queries::identity::user_by_id(conn, principal.user())
            .await?
            .ok_or_else(|| EngineError::unreadable("the update actor disappeared"))?;
        if let Some(hold) = crate::operation_scope::account_policy::session_hold(
            conn,
            &user,
            origin.browser_session(),
        )
        .await?
        {
            principal = principal.held_for_second_step(hold);
        }

        if !crate::updates::may_manage(conn, &principal, recipe.target).await?
            || !crate::updates::template_usable(conn, &principal, recipe.template, old.template_id)
                .await?
        {
            return Ok(None);
        }
        let until = current.until;
        callers.push((Some(principal.caller().clone()), until));
    }
    if callers.iter().any(|(caller, until)| {
        caller
            .as_ref()
            .is_none_or(|caller| !caller.key_deadline_allows(clock))
            || until.is_some_and(|until| clock.time() >= until)
    }) {
        return Ok(None);
    }
    Ok(Some(callers))
}
/// Owned compensation verifies the real original/candidate pair without reviving forward permission.
pub(crate) async fn cleanup(job: &Job) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if structure(&mut tx, &row, false).await?.is_none() {
        return Err(fail(
            "access.denied",
            "the update cleanup no longer owns this native candidate",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The existing enrollment writer uses the same owned transaction as its private job result.
/// The secret never escapes a transaction whose original credential ends during preparation.
pub(crate) async fn enrollment(
    job: &Job,
    services: &dyn crate::hosting::HostServices,
    ask: &crate::hosting::EnrollmentAsk,
) -> Result<crate::hosting::Enrollment, Stop> {
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let Some(lives) = current(&mut tx, &row, job.engine().inner.clock.as_ref()).await? else {
        return Err(fail(
            "access.denied",
            "the original update no longer permits enrollment",
        ));
    };
    let Some((recipe, _, Some(candidate))) = structure(&mut tx, &row, true).await? else {
        return Err(fail(
            "access.denied",
            "the update enrollment candidate changed",
        ));
    };
    let state = envs::environment(&mut tx, recipe.target.id())
        .await?
        .ok_or(Stop::Lost)?;
    if ask.runtime != candidate.id
        || ask.host != candidate.host_id
        || ask.created_by != state.owner_id
        || ask.name_hint != format!("{}-enroll", candidate.unit_name)
    {
        return Err(fail(
            "access.denied",
            "the native enrollment differs from its update candidate",
        ));
    }
    let enrollment = match row.plan.get("enrollment") {
        Some(value) => serde_json::from_value(value.clone()).map_err(EngineError::from)?,
        None => {
            let enrollment = services
                .enrollment(&mut tx, ask)
                .await
                .map_err(|error| EngineError::unreadable(error))?;
            jobs::merge_plan(
                &mut tx,
                row.id,
                &json!({"enrollment":enrollment}),
                job.now(),
            )
            .await?;
            enrollment
        }
    };
    if lives.iter().any(|(caller, until)| {
        caller
            .as_ref()
            .is_none_or(|caller| !caller.key_deadline_allows(job.engine().inner.clock.as_ref()))
            || until.is_some_and(|until| job.engine().inner.clock.time() >= until)
    }) {
        return Err(fail(
            "access.denied",
            "the original update ended before enrollment committed",
        ));
    }
    tx.commit().await.map_err(DbError::from)?;
    Ok(enrollment)
}

/// Exact native command reference; accepted origins stay on the immutable owning job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    id: Id,
    kind: orch_link_contract::commands::CommandKind,
}
impl Command {
    /// The native envelope's actual type, never an incoming operation claim.
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    /// Native ownership precedes the command row in the existing queue's lock order.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        jobs::lock(conn, self.job)
            .await?
            .ok_or_else(|| EngineError::unreadable("the native update command lost its owner"))?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: CommandView<'_>,
    ) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.id || command.kind != self.kind.as_str() {
            return Ok(None);
        }
        let Some(row) = jobs::lock(conn, self.job).await? else {
            return Ok(None);
        };
        if !is_restricted(&row)
            || !super::super::command_identity::recorded_matches(
                &row,
                COMMANDS,
                command.id,
                command.machine,
                command.serial,
                command.kind,
                command.body,
            )?
        {
            return Ok(None);
        }
        super::batch_authority::command_current_callers(
            conn,
            command.id,
            command.machine,
            command.serial,
            command.kind,
            command.body,
            clock,
        )
        .await
    }
    /// Exact proposed insertion on the real writer connection.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                serial: command.serial_key,
                kind: command.kind,
                body: command.body,
            },
        )
        .await
    }
    /// Exact unaccepted forwarding, independently of an accepted command receipt.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<orch_authz::Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                serial: command.serial_key(),
                kind: &command.kind,
                body: &command.body,
            },
        )
        .await
    }
}
struct CommandView<'a> {
    id: Id,
    machine: Id,
    serial: Option<&'a str>,
    kind: &'a str,
    body: &'a Json,
}
/// Seal the trusted runner's actual post-augmentation command and pass its final check to the writer.
pub(crate) async fn before_issue(
    job: &Job,
    command: &mut Outgoing,
) -> Result<Option<crate::commands::BeforeCommand<'static>>, Stop> {
    let mut tx = job.engine().db().begin().await?;
    orch_db::queries::transaction_guard::lock_action(&mut tx).await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if !is_restricted(&row) {
        return Ok(None);
    }
    if !super::super::command_identity::recorded_matches(
        &row,
        COMMANDS,
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )? {
        return Err(fail(
            "access.denied",
            "the native update command differs from its retained envelope",
        ));
    }
    let authority = crate::commands::CommandAuthority::UpdateJob(Box::new(Command {
        job: row.id,
        id: command.id,
        kind: command.kind,
    }));
    if command
        .authority
        .as_ref()
        .is_some_and(|original| original != &authority)
    {
        return Err(fail(
            "access.denied",
            "the native update command already belongs to another origin",
        ));
    }
    command.authority = Some(authority);
    let accepted = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?
        .is_some_and(|info| {
            matches!(
                info.stage,
                crate::commands::Stage::Accepted | crate::commands::Stage::Over
            )
        });
    if accepted {
        tx.commit().await.map_err(DbError::from)?;
        return Ok(None);
    }
    let Some(lives) = current(&mut tx, &row, job.engine().inner.clock.as_ref()).await? else {
        let status = job
            .engine()
            .inner
            .commands
            .status(command.id)
            .await
            .map_err(EngineError::from)?;
        let retained = job
            .engine()
            .inner
            .commands
            .retained_command(command.id)
            .await
            .map_err(EngineError::from)?;
        if status.is_some_and(|info| {
            matches!(
                info.stage,
                crate::commands::Stage::Accepted | crate::commands::Stage::Over
            )
        }) && retained.as_ref().is_some_and(|retained| {
            super::super::command_identity::retained_matches(&row, COMMANDS, command, retained)
                .unwrap_or(false)
        }) {
            tx.commit().await.map_err(DbError::from)?;
            return Ok(None);
        }
        return Err(fail("access.denied", "the native update command has ended"));
    };
    tx.commit().await.map_err(DbError::from)?;
    let clock = std::sync::Arc::clone(&job.engine().inner.clock);
    Ok(Some(Box::new(move || {
        if lives.iter().any(|(caller, until)| {
            caller
                .as_ref()
                .is_none_or(|caller| !caller.key_deadline_allows(clock.as_ref()))
                || until.is_some_and(|until| clock.time() >= until)
        }) {
            return Err(Box::new(crate::operation_scope::continuing::Error::Refused)
                as Box<dyn std::error::Error + Send + Sync>);
        }
        Ok(())
    })))
}

/// Final native lifetime check without reselecting a source this transaction legitimately advanced.
pub(crate) async fn deadline(
    conn: &mut Conn,
    row: &jobs::JobRow,
    clock: &dyn Clock,
) -> Result<(), EngineError> {
    let accepted =
        read(row)?.ok_or_else(|| EngineError::unreadable("the batch update source is missing"))?;
    for source in accepted
        .review
        .iter()
        .chain(std::iter::once(&accepted.confirmation))
    {
        source.check_deadline(conn, clock).await?;
    }
    Ok(())
}
