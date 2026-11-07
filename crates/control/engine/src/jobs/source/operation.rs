//! Original-operation source preparation, using the shared factual binder and current native frame.

use super::*;
use crate::commands::{CommandAuthority, Stage};
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::{self as binding, Facts},
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::key_scope::KeyTargetKind;
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    source::EnvironmentSourcePrepare,
    source_sources::{self as declarations, Source as Role},
};
use orch_authz::Act;
use orch_types::Clock;

const REQUEST: &str = "source_operation";
const JOURNAL: &str = "source_operation_commands";
const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);
const THREAD: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Thread);

fn refusal() -> Stop {
    Failure::new(
        "access.denied",
        "The original source preparation no longer permits this work.",
    )
    .into()
}
pub(crate) fn is_restricted(row: &JobRow) -> bool {
    row.plan.get(REQUEST).is_some() || row.plan.get(JOURNAL).is_some()
}
/// Select only the actual native source action, never a credential's requested label.
pub fn declaration(plan: &Plan) -> &'static Declaration<Role> {
    declaration_for(matches!(plan.action, Action::Worktree { .. }))
}
/// The mode is the native stored choice, never an incoming credential label.
pub fn declaration_for(worktree: bool) -> &'static Declaration<Role> {
    if worktree {
        &declarations::SOURCE_WORKTREE
    } else {
        &declarations::SOURCE_PREPARE
    }
}

/// The actual accepted native recipe, not target admission or a caller rebuilt from an account id.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    job: Id,
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    root: String,
    plan: Json,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourcePreparation([private])")
    }
}
impl Request {
    /// Retain exactly the newly inserted source job on its actual first-effect transaction.
    pub async fn retain(
        conn: &mut Conn,
        row: &JobRow,
        caller: &Caller,
        session: Option<Id>,
        root: &str,
        now: Time,
    ) -> Result<Self, EngineError> {
        let plan: Plan = serde_json::from_value(row.plan.clone())?;
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let expected = binding::bind_required(
            EnvironmentSourcePrepare::ROUTE,
            declaration(&plan),
            &facts(conn, &plan).await?,
        )
        .map_err(EngineError::unreadable)?
        .required_facets()
        .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        let same = expected.iter().all(|required| {
            admitted.iter().any(|actual| {
                actual.operation == required.operation
                    && actual.action == required.action
                    && actual.resource_kind == required.resource_kind
                    && actual.target.object == required.target.object
            })
        });
        if row.kind != kind::WORKSPACE_PREPARE
            || row.state != "queued"
            || row.created_by != Some(caller.user())
            || row.node_id != plan.thread
            || row.environment_id != Some(plan.environment)
            || plan.thread.is_none()
            || plan.creator_origin.is_some()
            || plan.scheduled_input.is_some()
            || origin.operation() != declaration(&plan).operation
            || (session.is_some() && caller.grant().is_some())
            || is_restricted(row)
            || !same
        {
            return Err(EngineError::unreadable(
                "the source request is not its newly admitted native job",
            ));
        }
        let value = Self {
            job: row.id,
            origin,
            session,
            root: root.into(),
            plan: serde_json::to_value(&plan)?,
        };
        rows::merge_plan(conn, row.id, &json!({REQUEST:value}), now.get()).await?;
        Ok(value)
    }
    /// Present invalid provenance never selects a historical ordinary path.
    pub fn from_job(row: &JobRow) -> Result<Option<Self>, EngineError> {
        row.plan
            .get(REQUEST)
            .map(|value| serde_json::from_value(value.clone()).map_err(EngineError::from))
            .transpose()
    }
}
fn structural(row: &JobRow, plan: &Plan) -> Result<Request, Stop> {
    let request = Request::from_job(row)?.ok_or_else(refusal)?;
    if request.job != row.id
        || row.kind != kind::WORKSPACE_PREPARE
        || row.node_id != plan.thread
        || row.environment_id != Some(plan.environment)
        || row.provider_id.is_some()
        || row.created_by != Some(plan.caller)
        || request.origin.user() != plan.caller
        || request.origin.operation() != declaration(plan).operation
        || request.plan != json!(plan)
        || plan.creator_origin.is_some()
        || plan.scheduled_input.is_some()
    {
        return Err(refusal());
    }
    Ok(request)
}
/// Current factual operands of the existing native source recipe.
pub async fn facts(conn: &mut Conn, plan: &Plan) -> Result<Vec<Facts<Role>>, EngineError> {
    facts_for(
        conn,
        plan.environment,
        plan.thread
            .ok_or_else(|| EngineError::unreadable("the native source has no Thread"))?,
        matches!(plan.action, Action::Worktree { .. }),
    )
    .await
}
/// Factual native discovery before the existing typed planner has completed its operands.
pub async fn facts_for(
    conn: &mut Conn,
    environment: Id,
    thread: Id,
    worktree: bool,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let row = threads::thread(conn, thread)
        .await?
        .filter(|row| row.environment_id == Some(environment))
        .ok_or_else(|| EngineError::unreadable("the native source Thread changed"))?;
    let env = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    let target = targets::node(conn, row.node_id)
        .await
        .map_err(EngineError::unreadable)?;
    let mut facts = vec![
        Facts::one(Role::Environment, ENV, env.clone()),
        Facts::one(Role::Thread, THREAD, target),
        Facts::one(Role::ProblemEnvironment, ENV, env.clone()),
    ];
    if worktree {
        facts.push(Facts::one(Role::WorkspaceFiles, ENV, env));
    }
    Ok(facts)
}

async fn raw_session(
    conn: &mut Conn,
    request: &Request,
    caller: &Caller,
    now: Time,
) -> Result<Option<Option<Time>>, EngineError> {
    let Some(session) = request.session else {
        return Ok(Some(None));
    };
    if caller.grant().is_some() {
        return Ok(None);
    }
    let state =
        orch_db::queries::tokens::lock_session_state(conn, session, request.origin.user()).await?;
    Ok(state
        .filter(|(admin, end)| *end > now.get() && (!caller.as_admin() || *admin))
        .map(|(_, end)| Some(Time::new(end))))
}
/// Effect-free precursor on existing sampled-Time observer paths; native effects use Frame below.
pub(super) async fn principal_at(
    conn: &mut Conn,
    row: &JobRow,
    plan: &Plan,
    now: Time,
) -> Result<Principal, Stop> {
    let request = structural(row, plan)?;
    let caller = request
        .origin
        .restore(conn, now)
        .await
        .map_err(EngineError::from)?
        .ok_or_else(refusal)?;
    if raw_session(conn, &request, &caller, now).await?.is_none() {
        return Err(refusal());
    }
    let prepared = binding::bind_required(
        EnvironmentSourcePrepare::ROUTE,
        declaration(plan),
        &facts(conn, plan).await?,
    )
    .map_err(EngineError::unreadable)?;
    let caller = prepared.admit(&caller, false).map_err(|_| refusal())?;
    Principal::load(conn, caller, now)
        .await
        .map_err(EngineError::from)
        .map_err(Into::into)
}

struct Input {
    job: Id,
    request: Request,
    write: bool,
}
struct Value {
    row: JobRow,
    plan: Plan,
    env: environments::workspace::EnvironmentRow,
}
struct Native;
impl Source for Native {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = EnvironmentSourcePrepare::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::SOURCE_PREPARE;
    fn declaration(_: &Input, value: &Value) -> &'static Declaration<Role> {
        declaration(&value.plan)
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = rows::job(conn, input.job).await?.ok_or(Error::Refused)?;
            let plan: Plan = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
            let request = structural(&row, &plan).map_err(|_| Error::Refused)?;
            if request != input.request || !row.is_open() || row.cancel_requested() {
                return Err(Error::Refused);
            }
            let env = matching_target(conn, &plan, None)
                .await
                .map_err(|_| Error::Refused)?
                .filter(|env| env.workspace_root == request.root)
                .ok_or(Error::Refused)?;
            let facts = facts(conn, &plan).await?;
            Ok((Value { row, plan, env }, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            check_native_at(
                conn,
                value.row.id,
                &value.plan,
                input.write,
                None,
                Some(principal),
            )
            .await
            .map_err(|_| Error::Refused)?;
            let thread = value.plan.thread.ok_or(Error::Refused)?;
            if !orch_authz::nodes::check(
                conn,
                principal,
                thread,
                Permission::ThreadView,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            rows::lock(conn, value.row.id)
                .await?
                .ok_or(Error::Refused)?;
            if lock_attempt_at(conn, &value.row, &value.plan, None)
                .await?
                .is_none()
            {
                return Err(Error::Refused);
            }
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(value.env.id)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(value.plan.machine)
                .execute(conn)
                .await
                .map_err(orch_db::DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        old: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id
                && json!(old.plan) == json!(fresh.plan)
                && old.env.id == fresh.env.id
                && old.env.generation == fresh.env.generation
                && old.env.machine_id == fresh.env.machine_id
                && old.env.workspace_root == fresh.env.workspace_root)
        })
    }
}
async fn frame(
    conn: &mut Conn,
    clock: &dyn Clock,
    row: &JobRow,
    write: bool,
) -> Result<continuing::Frame<Native>, Stop> {
    let plan: Plan = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
    let request = structural(row, &plan)?;
    let origin = request.origin.clone();
    let frame = continuing::prepare::<Native>(
        conn,
        clock,
        &origin,
        Input {
            job: row.id,
            request: request.clone(),
            write,
        },
    )
    .await
    .map_err(|_| refusal())?;
    if raw_session(conn, &request, frame.principal().caller(), clock.time())
        .await?
        .is_none()
    {
        return Err(refusal());
    }
    Ok(frame)
}
pub(super) async fn before_mutation(
    conn: &mut Conn,
    job: &Job,
    plan: &Plan,
    write: bool,
) -> Result<(), Stop> {
    let row = rows::job(conn, job.id()).await?.ok_or_else(refusal)?;
    if !is_restricted(&row) {
        return Ok(());
    }
    if json!(plan)
        != serde_json::to_value(
            serde_json::from_value::<Plan>(row.plan.clone()).map_err(EngineError::from)?,
        )
        .map_err(EngineError::from)?
    {
        return Err(refusal());
    }
    frame(conn, job.engine().inner.clock.as_ref(), &row, write)
        .await?
        .into_parts()
        .first_effect
        .check(job.engine().inner.clock.as_ref())
        .map_err(|_| refusal())
}
/// Before an unaccepted native phase, restore the exact original operation on the real transaction.
pub(super) async fn check(job: &Job, plan: &Plan, write: bool) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    before_mutation(&mut tx, job, plan, write).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Exact current native command and its immutable original source job recipe.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceCommand {
    job: Id,
    request: Request,
    canonical: Json,
    write: bool,
}
impl std::fmt::Debug for SourceCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SourceCommand([private])")
    }
}
fn canonical(command: &Outgoing) -> Result<Json, EngineError> {
    crate::jobs::command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
}
impl SourceCommand {
    /// The actual native command family captured by the source producer.
    pub fn command_kind(&self) -> &'static str {
        match self.canonical.get("kind").and_then(Json::as_str) {
            Some("fs.stat") => "fs.stat",
            Some("fs.mkdir") => "fs.mkdir",
            Some("git.prepare") => "git.prepare",
            Some("fs.record") => "fs.record",
            Some("git.worktree") => "git.worktree",
            _ => "unknown",
        }
    }

    pub(crate) async fn capture(
        conn: &mut Conn,
        row: &JobRow,
        expected: &Request,
        command: &Outgoing,
    ) -> Result<Self, Stop> {
        let plan: Plan = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
        let request = structural(row, &plan)?;
        if request != *expected
            || command.machine != plan.machine
            || command.key != SerialKey::Environment(plan.environment)
            || !creator_body(&plan, row.id, command.kind.as_str(), &command.body)?
        {
            return Err(refusal());
        }
        let write = requires_write(&plan) || command.kind.as_str() != "fs.stat";
        Ok(Self {
            job: row.id,
            request,
            canonical: canonical(command)?,
            write,
        })
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if crate::jobs::command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? != self.canonical
        {
            return Ok(None);
        }
        let Some(row) = rows::job(conn, self.job).await? else {
            return Ok(None);
        };
        if row
            .plan
            .get(JOURNAL)
            .and_then(|value| value.get(command.id.to_string()))
            .filter(|value| **value == self.canonical)
            .is_none()
        {
            return Ok(None);
        }
        let current = match frame(conn, clock, &row, self.write).await {
            Ok(frame) => frame,
            Err(_) => return Ok(None),
        };
        if Request::from_job(&current.value().row)?.as_ref() != Some(&self.request) {
            return Ok(None);
        }
        let parts = current.into_parts();
        let raw = raw_session(conn, &self.request, parts.principal.caller(), clock.time()).await?;
        let Some(until) = raw else {
            return Ok(None);
        };
        parts
            .first_effect
            .check(clock)
            .map_err(EngineError::unreadable)?;
        Ok(Some(vec![(Some(parts.principal.caller().clone()), until)]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
        )
        .await
    }
}

pub(crate) async fn decorate(job: &Job, command: &mut Outgoing) -> Result<(), Stop> {
    let row = job.row().await?;
    let expected = match &command.authority {
        Some(CommandAuthority::SourcePreparation(value)) => Some(value.request.clone()),
        _ => None,
    };
    if !is_restricted(&row) && expected.is_none() {
        return Ok(());
    }
    let expected = expected.ok_or_else(refusal)?;
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let authority = SourceCommand::capture(&mut tx, &row, &expected, command).await?;
    let existing = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?;
    if !existing
        .as_ref()
        .is_some_and(|info| matches!(info.stage, Stage::Accepted | Stage::Over))
    {
        let current = frame(
            &mut tx,
            job.engine().inner.clock.as_ref(),
            &row,
            authority.write,
        )
        .await?;
        let parts = current.into_parts();
        parts
            .first_effect
            .check(job.engine().inner.clock.as_ref())
            .map_err(|_| refusal())?;
        let mut journal = row.plan.get(JOURNAL).cloned().unwrap_or_else(|| json!({}));
        if let Some(old) = journal.get(command.id.to_string())
            && old != &authority.canonical
        {
            return Err(refusal());
        }
        journal[command.id.to_string()] = authority.canonical.clone();
        rows::merge_plan(&mut tx, row.id, &json!({JOURNAL:journal}), job.now()).await?;
    } else if row
        .plan
        .get(JOURNAL)
        .and_then(|journal| journal.get(command.id.to_string()))
        != Some(&authority.canonical)
    {
        return Err(refusal());
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    command.authority = Some(CommandAuthority::source_preparation(authority));
    Ok(())
}
pub(super) async fn received(job: &Job, plan: &Plan) -> Result<bool, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = rows::job(&mut tx, job.id()).await?.ok_or_else(refusal)?;
    let accepted = received_at(&mut tx, &row, plan, false).await?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(accepted)
}
async fn received_at(
    conn: &mut Conn,
    row: &JobRow,
    plan: &Plan,
    completed: bool,
) -> Result<bool, Stop> {
    if !is_restricted(row) {
        return Ok(false);
    }
    let request = structural(row, plan)?;
    let result = row.plan.get("file_result");
    if completed && (result.is_none() || row.plan.get("file_source_attempt") != Some(&json!(plan)))
    {
        return Ok(false);
    }
    let Some(journal) = row.plan.get(JOURNAL).and_then(Json::as_object) else {
        return Ok(false);
    };
    for id in journal.keys() {
        let id = id.parse::<Id>().map_err(|_| refusal())?;
        let Some(private) = orch_db::queries::link::command_with_authority(conn, id).await? else {
            continue;
        };
        let command = &private.command;
        let canonical = crate::jobs::command_identity::canonical(
            id,
            command.machine_id,
            command.serial_key(),
            &command.kind,
            &command.body,
        )?;
        if journal.get(&id.to_string()) != Some(&canonical) {
            return Err(refusal());
        }
        let authority: CommandAuthority =
            serde_json::from_value(private.authority.ok_or_else(refusal)?)
                .map_err(|_| refusal())?;
        let CommandAuthority::SourcePreparation(authority) = authority else {
            return Err(refusal());
        };
        if authority.request != request
            || authority.job != row.id
            || authority.canonical != canonical
        {
            return Err(refusal());
        }
        if !matches!(
            command.state.as_str(),
            "accepted" | "succeeded" | "failed" | "cancelled" | "expired"
        ) {
            continue;
        }
        if !completed {
            return Ok(true);
        }
        if command.state != "succeeded" {
            continue;
        }
        let Some(value) = command.result.as_ref() else {
            continue;
        };
        let Some(result) = result else {
            continue;
        };
        if value == result {
            return Ok(true);
        }
        if matches!(command.kind.as_str(), "fs.stat" | "fs.mkdir")
            && value.pointer("/entry/kind").and_then(Json::as_str) == Some("dir")
            && result.get("kind").and_then(Json::as_str) == Some("directory")
        {
            let directory = match &plan.action {
                Action::Prepare { directory, .. } => directory,
                _ => continue,
            };
            if result.get("directory").and_then(Json::as_str) == Some(directory.as_str()) {
                return Ok(true);
            }
        }
    }
    Ok(false)
}
/// Completed native result publication uses only its exact owned receipt and original source.
pub(super) async fn reconcile_at(conn: &mut Conn, row: &JobRow, plan: &Plan) -> Result<bool, Stop> {
    received_at(conn, row, plan, true).await
}
