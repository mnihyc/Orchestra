//! Actual recorded restore sources and original operations in the existing private channel.

use super::super::runner::{Failure, Job, Stop};
use super::super::source::{Action, Plan};
use crate::EngineError;
use crate::commands::{CommandAuthority, Outgoing, Stage};
use crate::operation_scope::{
    continuing::{self, Error, Source as NativeSource},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    forks::ActiveTurn,
    operation_sources::Declaration,
    restore_sources::{self as declarations, Source as Role},
    routes::{Endpoint, Route},
    source::{EnvironmentFilesRestore, EnvironmentRestorePreflight},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        environment_facts,
        environments::workspace as environments,
        jobs,
        link::{CommandRow, NewCommand},
        storage, threads,
    },
};
use orch_link_contract::{
    commands::{ProcList, ProcResumeOwned, ProcSuspendOwned, SerialKey},
    source::{FsRecord, FsRestore, FsRestorePlan, RecordKind},
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

const REQUEST: &str = "restore_operation";
const JOURNAL: &str = "restore_operation_commands";
const ISSUED: &str = "restore_issued_commands";

const ENV: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

/// Immutable factual source of the measured native plan; it contains no admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    snapshot: Id,
    witness: crate::archive::snapshot_authority::ReadWitness,
    until: Time,
    command: FsRestorePlan,
}

/// Fresh native rows, never serializable caller authority.
pub struct Native {
    /// The real current workspace source/destination.
    pub environment: environments::EnvironmentRow,
    /// The real retained recorded source.
    pub snapshot: storage::SnapshotRow,
}
impl Anchor {
    /// Capture the native selector's actual stored association and already resolved policy.
    pub fn new(
        environment: &environments::EnvironmentRow,
        snapshot: &storage::SnapshotRow,
        machine: Id,
        turn: Option<Id>,
        record_days: i64,
        max_bytes: i64,
    ) -> Result<Self, EngineError> {
        if snapshot.environment_id != environment.id
            || snapshot.storage != "environment"
            || snapshot.state != "complete"
            || environment.machine_id != Some(machine)
        {
            return Err(EngineError::unreadable(
                "the recorded restore source changed",
            ));
        }
        Ok(Self {
            environment: environment.id,
            generation: environment.generation,
            machine,
            root: environment.workspace_root.clone(),
            snapshot: snapshot.id,
            witness: crate::archive::snapshot_authority::ReadWitness::from_snapshot(snapshot),
            until: snapshot.expires_at.map(Time::new).ok_or_else(|| {
                EngineError::unreadable("the recorded state has no keeping period")
            })?,
            command: FsRestorePlan {
                environment_id: environment.id,
                snapshot_id: snapshot.id,
                directory: snapshot
                    .directory
                    .clone()
                    .ok_or_else(|| EngineError::unreadable("the recorded directory is unknown"))?,
                turn_id: turn,
                record_days: Some(record_days),
                max_bytes: Some(max_bytes),
            },
        })
    }
    /// The original native owning Environment.
    pub fn environment(&self) -> Id {
        self.environment
    }
    /// The actual original machine recipient.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The original exact physical workspace root.
    pub fn root(&self) -> &str {
        &self.root
    }
    /// The native complete recorded state.
    pub fn snapshot(&self) -> Id {
        self.snapshot
    }
    /// The exact native measurement body, without authority fields.
    pub fn command(&self) -> &FsRestorePlan {
        &self.command
    }
    /// The original source keeping bound, never extended by a later observation.
    pub fn until(&self) -> Time {
        self.until
    }

    /// Reopen only the actual metadata association, independently of completed management.
    pub async fn metadata(&self, conn: &mut Conn) -> Result<Option<Native>, EngineError> {
        let Some(environment) = environments::environment(conn, self.environment)
            .await?
            .filter(|row| {
                row.generation == self.generation
                    && row.machine_id == Some(self.machine)
                    && row.workspace_root == self.root
            })
        else {
            return Ok(None);
        };
        let Some(snapshot) = storage::snapshot(conn, self.snapshot)
            .await?
            .filter(|row| self.witness.matches(row))
        else {
            return Ok(None);
        };
        Ok(Some(Native {
            environment,
            snapshot,
        }))
    }

    async fn current(&self, conn: &mut Conn, now: Time) -> Result<Native, Error> {
        let native = self.metadata(conn).await?.ok_or(Error::Refused)?;
        if self.until <= now
            || native.snapshot.state != "complete"
            || native
                .snapshot
                .expires_at
                .is_none_or(|until| until <= now.get())
            || !crate::commands::workspace_physical(conn, &native.environment, self.machine).await?
        {
            return Err(Error::Refused);
        }
        Ok(native)
    }
}

/// Known native source roles; actual resolved target identities always come from the graph.
pub async fn environment_facts(
    conn: &mut Conn,
    environment: Id,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let target = targets::resource_identity(
        conn,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
    )
    .await
    .map_err(EngineError::unreadable)?;
    Ok(vec![
        Facts::one(Role::Environment, ENV, target.clone()),
        Facts::one(Role::Files, ENV, target.clone()),
        Facts::one(Role::Content, ENV, target),
    ])
}

pub(super) async fn permission(
    conn: &mut Conn,
    actor: &Principal,
    environment: Id,
    permission: Permission,
) -> Result<(), Error> {
    if orch_authz::resources::check(
        conn,
        actor,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        permission,
        Policy::default(),
    )
    .await?
    .is_allowed()
    {
        Ok(())
    } else {
        Err(Error::Refused)
    }
}

async fn native_gate(conn: &mut Conn, actor: &Principal, native: &Native) -> Result<(), Error> {
    permission(conn, actor, native.environment.id, Permission::FilesManage).await?;
    if !native.environment.remote_write
        || environment_facts::refuses(conn, native.environment.id, "environment.remote_write")
            .await?
    {
        return Err(Error::Refused);
    }
    Ok(())
}

/// The same native source/ordinary write ceiling for an already scoped API confirmation.
pub async fn require_target(
    conn: &mut Conn,
    actor: &Principal,
    anchor: &Anchor,
    now: Time,
) -> Result<(), Error> {
    let native = anchor.current(conn, now).await?;
    native_gate(conn, actor, &native).await
}

/// Actual review input. A running continuation supplies only its real native job association.
#[derive(Clone)]
pub struct ReviewInput {
    anchor: Anchor,
    job: Option<Id>,
}
/// The native review's factual factory, reused by API and durable measurement admission.
pub struct ReviewSource;
impl NativeSource for ReviewSource {
    type Role = Role;
    type Input = ReviewInput;
    type Value = Native;
    const ROUTE: Route = EnvironmentRestorePreflight::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::RESTORE_PREFLIGHT;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a ReviewInput,
        now: Time,
    ) -> BoxFuture<'a, Result<(Native, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let native = input.anchor.current(conn, now).await?;
            if jobs::changes::environment_change_hold(conn, native.environment.id)
                .await?
                .is_some_and(|row| {
                    row.kind == jobs::kind::WORKSPACE_RESTORE && Some(row.id) != input.job
                })
            {
                return Err(Error::Refused);
            }
            let facts = environment_facts(conn, native.environment.id).await?;
            Ok((native, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        actor: &'a Principal,
        _: &'a ReviewInput,
        value: &'a Native,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(native_gate(conn, actor, value))
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a ReviewInput,
        value: &'a Native,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if let Some(thread) = value.snapshot.thread_id {
                sqlx::query("SELECT id FROM nodes WHERE id=$1 FOR KEY SHARE")
                    .bind(thread)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(input.anchor.environment)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR SHARE")
                .bind(input.anchor.snapshot)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.anchor.machine)
                .execute(conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        input: &'a ReviewInput,
        old: &'a Native,
        fresh: &'a Native,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(input.anchor.witness.matches(&old.snapshot)
                && input.anchor.witness.matches(&fresh.snapshot)
                && old.environment.generation == fresh.environment.generation
                && old.environment.machine_id == fresh.environment.machine_id
                && old.environment.workspace_root == fresh.environment.workspace_root)
        })
    }
}

/// One genuine saved review operation, not a caller reconstructed from its creator id.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Review {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    anchor: Anchor,
}
impl std::fmt::Debug for Review {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestoreReview([private])")
    }
}
impl Review {
    /// Retain the actual admitted operation and raw browser-session association.
    pub fn capture(
        caller: &Caller,
        session: Option<Id>,
        anchor: Anchor,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        if origin.operation() != declarations::RESTORE_PREFLIGHT.operation
            || (session.is_some() && caller.grant().is_some())
            || ![
                orch_access_contract::key_scope::KeyAction::Read,
                orch_access_contract::key_scope::KeyAction::Manage,
            ]
            .iter()
            .all(|action| {
                admitted.iter().any(|fact| {
                    fact.operation == declarations::RESTORE_PREFLIGHT.operation
                        && fact.action == *action
                        && fact.resource_kind == ENV
                        && fact.target.object.kind == ENV
                        && fact.target.object.id == Some(anchor.environment)
                })
            })
        {
            return Err(EngineError::unreadable(
                "the restore review has no matching native source admission",
            ));
        }
        Ok(Self {
            origin,
            session,
            anchor,
        })
    }
    /// Actual immutable native source.
    pub fn anchor(&self) -> &Anchor {
        &self.anchor
    }
    /// The native account who actually prepared this plan.
    pub fn user(&self) -> Id {
        self.origin.user()
    }
    /// Recheck the actual original/current operation on this same native transaction.
    pub async fn prepare(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        job: Option<Id>,
        incoming: Option<&Caller>,
    ) -> Result<ReviewFrame, Error> {
        if let Some(job) = job {
            let row = jobs::job(conn, job).await?.ok_or(Error::Refused)?;
            let (request, _) = structural(&row).map_err(|_| Error::Refused)?;
            if !row.is_open() || row.cancel_requested() || request.review.as_ref() != Some(self) {
                return Err(Error::Refused);
            }
        }
        let input = ReviewInput {
            anchor: self.anchor.clone(),
            job,
        };
        let frame = match incoming {
            Some(caller) => {
                continuing::prepare_with_caller::<ReviewSource>(
                    conn,
                    clock,
                    &self.origin,
                    caller,
                    input,
                )
                .await?
            }
            None => continuing::prepare::<ReviewSource>(conn, clock, &self.origin, input).await?,
        };
        let raw = raw_session(
            conn,
            &self.origin,
            self.session,
            frame.principal().caller(),
            clock.time(),
        )
        .await?
        .ok_or(Error::Refused)?;
        let until = frame
            .value()
            .snapshot
            .expires_at
            .map(Time::new)
            .ok_or(Error::Refused)?
            .min(self.anchor.until);
        Ok(ReviewFrame {
            frame,
            until: raw.map_or(until, |raw| until.min(raw)),
        })
    }
}

async fn raw_session(
    conn: &mut Conn,
    origin: &CapturedOperation,
    session: Option<Id>,
    caller: &Caller,
    now: Time,
) -> Result<Option<Option<Time>>, EngineError> {
    let Some(session) = session else {
        return Ok(Some(None));
    };
    if caller.grant().is_some() {
        return Ok(None);
    }
    Ok(
        orch_db::queries::tokens::lock_session_state(conn, session, origin.user())
            .await?
            .filter(|(admin, until)| *until > now.get() && (!caller.as_admin() || *admin))
            .map(|(_, until)| Some(Time::new(until))),
    )
}

/// Current source/actor plus its actual raw-session and recorded-source lifetime.
pub struct ReviewFrame {
    frame: continuing::Frame<ReviewSource>,
    until: Time,
}
impl ReviewFrame {
    /// The actual current ordinary actor.
    pub fn principal(&self) -> &Principal {
        self.frame.principal()
    }
    /// The actual native operands.
    pub fn value(&self) -> &Native {
        self.frame.value()
    }
    /// The earliest genuine non-key source lifetime on this checkpoint.
    pub fn until(&self) -> Time {
        self.until
    }
    /// Consume at this transaction's real native first effect, without releasing its locks.
    pub fn into_parts(self) -> ReviewParts {
        let parts = self.frame.into_parts();
        ReviewParts {
            principal: parts.principal,
            value: parts.value,
            before: parts.first_effect,
            until: self.until,
        }
    }
}
/// The native owner consumes the linear cut immediately before its real first effect.
pub struct ReviewParts {
    /// The current original ordinary actor.
    pub principal: Principal,
    /// The current actual stored operands.
    pub value: Arc<Native>,
    before: continuing::FirstEffect,
    until: Time,
}
impl ReviewParts {
    /// Keep the real source/raw lifetime at the same synchronous native cut.
    pub fn check(self, clock: &dyn Clock) -> Result<(Principal, Arc<Native>), Error> {
        if clock.time() >= self.until {
            return Err(Error::Refused);
        }
        self.before.check(clock)?;
        Ok((self.principal, self.value))
    }
}

/// Only native restore commands can inhabit this private authority packet.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "stage",
    content = "source",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum RestoreCommand {
    /// The once-only measured source and its exact original operation.
    Review { review: Review, canonical: Json },
    /// One exact actual Job command, never source admission in its wire body.
    Job {
        request: Request,
        canonical: Json,
        stage: CommandStage,
    },
}
impl std::fmt::Debug for RestoreCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestoreCommand([private])")
    }
}
impl RestoreCommand {
    /// Capture only the native measured request of this same actual review.
    pub fn measurement(review: Review, command: &Outgoing) -> Result<Self, EngineError> {
        if command.machine != review.anchor.machine
            || command.key != SerialKey::Environment(review.anchor.environment)
            || command.kind.as_str() != "fs.restore.plan"
            || command.body != serde_json::to_value(review.anchor.command())?
        {
            return Err(EngineError::unreadable(
                "the native restore measurement changed its source",
            ));
        }
        Ok(Self::Review {
            review,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    /// The native typed command kind.
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Review { .. } => "fs.restore.plan",
            Self::Job { canonical, .. } => match canonical.get("kind").and_then(Json::as_str) {
                Some("proc.list") => "proc.list",
                Some("proc.suspend.owned") => "proc.suspend.owned",
                Some("proc.resume.owned") => "proc.resume.owned",
                Some("fs.restore") => "fs.restore",
                Some("fs.record") => "fs.record",
                _ => "unknown",
            },
        }
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let canonical = match self {
            Self::Review { canonical, .. } | Self::Job { canonical, .. } => canonical,
        };
        if command.kind != self.command_kind()
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != *canonical
        {
            return Ok(None);
        }
        if let Self::Job { request, stage, .. } = self {
            return match current_job_callers(conn, clock, request, command, *stage).await {
                Ok(value) => Ok(Some(value)),
                Err(Stop::Failed(failure)) if failure.code != super::super::runner::INTERNAL => {
                    Ok(None)
                }
                Err(Stop::Cancelled | Stop::Lost) => Ok(None),
                Err(error) => Err(EngineError::unreadable(format!(
                    "the recorded restore checkpoint failed: {error:?}"
                ))),
            };
        }
        let Self::Review { review, .. } = self else {
            return Ok(None);
        };
        let frame = match review.prepare(conn, clock, None, None).await {
            Ok(frame) => frame,
            Err(Error::Refused) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        let until = frame.until();
        let (actor, _) = frame
            .into_parts()
            .check(clock)
            .map_err(EngineError::unreadable)?;
        Ok(Some(vec![(Some(actor.caller().clone()), Some(until))]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue_callers(
            conn,
            &NewCommand {
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

/// The control branch selected from the native account authority, never a key's label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// Waiting needs no interruption; otherwise the native Environment manager controls it.
    Environment,
    /// The native Thread Interact alternative controls the actual reviewed writers.
    Threads,
}
impl Mode {
    /// One canonical native confirmation selection.
    pub fn declaration(self) -> &'static Declaration<Role> {
        match self {
            Self::Environment => &declarations::RESTORE_ENVIRONMENT,
            Self::Threads => &declarations::RESTORE_THREADS,
        }
    }
}

fn refused() -> Stop {
    Failure::new(
        "access.denied",
        "The original recorded restore no longer permits this work.",
    )
    .into()
}

/// Native confirmation authority and the exact materialized Job recipe.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    job: Id,
    review: Option<Review>,
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    anchor: Anchor,
    mode: Mode,
    plan: Json,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecordedRestore([private])")
    }
}
impl Request {
    /// Retain only the actual newly inserted native Job and independently admitted confirmation.
    pub async fn retain(
        conn: &mut Conn,
        row: &jobs::JobRow,
        caller: &Caller,
        session: Option<Id>,
        review: Option<Review>,
        anchor: Anchor,
        mode: Mode,
        now: Time,
    ) -> Result<Self, EngineError> {
        let plan: Plan = serde_json::from_value(row.plan.clone())?;
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let expected = crate::operation_scope::sources::bind_required(
            EnvironmentFilesRestore::ROUTE,
            mode.declaration(),
            &confirmation_facts(conn, &plan, mode).await?,
        )
        .map_err(EngineError::unreadable)?
        .required_facets()
        .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        let same = expected.iter().all(|required| {
            admitted.iter().any(|fact| {
                fact.operation == required.operation
                    && fact.action == required.action
                    && fact.resource_kind == required.resource_kind
                    && fact.target.object == required.target.object
            })
        });
        if row.kind != jobs::kind::WORKSPACE_RESTORE
            || row.state != "queued"
            || row.created_by != Some(caller.user())
            || row.environment_id != Some(anchor.environment)
            || row.node_id != plan.thread
            || plan.caller != caller.user()
            || origin.operation() != mode.declaration().operation
            || !same
            || (session.is_some() && caller.grant().is_some())
            || review
                .as_ref()
                .is_some_and(|review| review.user() != caller.user() || review.anchor != anchor)
            || (review.is_none()
                && caller
                    .grant()
                    .is_some_and(|grant| grant.operation_scope().is_some()))
            || row.plan.get(REQUEST).is_some()
            || !recipe(&plan, row.id, &anchor)
        {
            return Err(EngineError::unreadable(
                "the recorded restore is not its actual admitted native Job",
            ));
        }
        let request = Self {
            job: row.id,
            review,
            origin,
            session,
            anchor,
            mode,
            plan: serde_json::to_value(&plan)?,
        };
        jobs::merge_plan(
            conn,
            row.id,
            &serde_json::json!({REQUEST:request}),
            now.get(),
        )
        .await?;
        Ok(request)
    }
    /// Present malformed provenance is a refusal, never a legacy fallback.
    pub fn from_job(row: &jobs::JobRow) -> Result<Option<Self>, EngineError> {
        row.plan
            .get(REQUEST)
            .map(|value| serde_json::from_value(value.clone()).map_err(EngineError::from))
            .transpose()
    }
}
pub(crate) fn restricted(row: &jobs::JobRow) -> bool {
    row.plan.get(REQUEST).is_some() || row.plan.get(JOURNAL).is_some()
}
fn recipe(plan: &Plan, job: Id, anchor: &Anchor) -> bool {
    if plan.environment != anchor.environment
        || plan.generation != anchor.generation
        || plan.machine != anchor.machine
        || plan.creator_origin.is_some()
        || plan.scheduled_input.is_some()
    {
        return false;
    }
    matches!(&plan.action, Action::Restore { body, directory, record_days, max_bytes, .. }
        if body.job_id == job && body.environment_id == anchor.environment && body.snapshot_id == anchor.snapshot && body.generation == i64::from(anchor.generation)
            && body.undo_snapshot_id == crate::ids::derived("restore.undo", &[job.as_bytes()]) && body.work.is_none() && body.recovery_only.is_none()
            && directory == &anchor.command.directory && Some(*record_days) == anchor.command.record_days && Some(*max_bytes) == anchor.command.max_bytes)
}
fn structural(row: &jobs::JobRow) -> Result<(Request, Plan), Stop> {
    let request = Request::from_job(row)?.ok_or_else(refused)?;
    let plan: Plan = serde_json::from_value(row.plan.clone()).map_err(EngineError::from)?;
    if request.job != row.id
        || row.kind != jobs::kind::WORKSPACE_RESTORE
        || row.created_by != Some(request.origin.user())
        || row.created_by != Some(plan.caller)
        || row.node_id != plan.thread
        || row.environment_id != Some(plan.environment)
        || row.provider_id.is_some()
        || request.origin.operation() != request.mode.declaration().operation
        || request.plan != serde_json::to_value(&plan).map_err(EngineError::from)?
        || request
            .review
            .as_ref()
            .is_some_and(|review| review.user() != plan.caller || review.anchor != request.anchor)
        || !recipe(&plan, row.id, &request.anchor)
    {
        return Err(refused());
    }
    Ok((request, plan))
}

/// Actual retained reviewed writers that still belong to this workspace or its live turn.
pub async fn writers(conn: &mut Conn, plan: &Plan) -> Result<Vec<Id>, EngineError> {
    let Action::Restore { writers, .. } = &plan.action else {
        return Err(EngineError::unreadable(
            "the native restore has no writer recipe",
        ));
    };
    let mut actual = Vec::new();
    for id in writers {
        let Some(row) = threads::thread(conn, *id).await? else {
            continue;
        };
        let live_here: bool = match row.current_turn_id {
            Some(turn) => sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM turns t JOIN segments s ON s.id=t.segment_id WHERE t.id=$1 AND t.thread_id=$2 AND s.environment_id=$3 AND s.env_generation=$4)")
                .bind(turn).bind(id).bind(plan.environment).bind(plan.generation).fetch_one(&mut *conn).await.map_err(DbError::from)?,
            None => false,
        };
        if row.environment_id == Some(plan.environment) || live_here {
            actual.push(*id);
        }
    }
    actual.sort();
    actual.dedup();
    Ok(actual)
}
/// Current factual confirmation roles from the native frozen recipe.
pub async fn confirmation_facts(
    conn: &mut Conn,
    plan: &Plan,
    mode: Mode,
) -> Result<Vec<Facts<Role>>, EngineError> {
    let mut facts = environment_facts(conn, plan.environment).await?;
    if mode == Mode::Threads {
        let mut selected = Vec::new();
        for id in writers(conn, plan).await? {
            selected.push(
                targets::node(conn, id)
                    .await
                    .map_err(EngineError::unreadable)?,
            );
        }
        facts.push(Facts::collection(
            Role::Writers,
            if selected.is_empty() {
                Vec::new()
            } else {
                vec![(KeyTargetKind::Object(ObjectKind::Thread), selected)]
            },
        ));
    }
    Ok(facts)
}
/// The unchanged native file, writer-control and process-control prerequisites.
pub async fn require_confirmation(
    conn: &mut Conn,
    actor: &Principal,
    plan: &Plan,
    mode: Mode,
) -> Result<(), Error> {
    permission(conn, actor, plan.environment, Permission::FilesManage).await?;
    let Action::Restore {
        options, suspend, ..
    } = &plan.action
    else {
        return Err(Error::Refused);
    };
    if *suspend {
        permission(conn, actor, plan.environment, Permission::TerminalManage).await?;
    }
    if options.active_turn == ActiveTurn::Interrupt && !writers(conn, plan).await?.is_empty() {
        match mode {
            Mode::Environment => {
                permission(conn, actor, plan.environment, Permission::EnvironmentManage).await?
            }
            Mode::Threads => {
                for id in writers(conn, plan).await? {
                    if !orch_authz::nodes::check(
                        conn,
                        actor,
                        id,
                        Permission::ThreadInteract,
                        orch_authz::Act::Use,
                        Policy::default(),
                    )
                    .await?
                    .is_allowed()
                    {
                        return Err(Error::Refused);
                    }
                }
            }
        }
    }
    Ok(())
}

struct RuntimeInput {
    request: Request,
    focus: Option<(Id, Id)>,
}
struct RuntimeValue {
    native: Native,
    row: jobs::JobRow,
    plan: Plan,
}
struct RuntimeSource;
impl NativeSource for RuntimeSource {
    type Role = Role;
    type Input = RuntimeInput;
    type Value = RuntimeValue;
    const ROUTE: Route = EnvironmentFilesRestore::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &declarations::RESTORE_ENVIRONMENT;
    fn declaration(input: &RuntimeInput, _: &RuntimeValue) -> &'static Declaration<Role> {
        input.request.mode.declaration()
    }
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a RuntimeInput,
        now: Time,
    ) -> BoxFuture<'a, Result<(RuntimeValue, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.request.job)
                .await?
                .ok_or(Error::Refused)?;
            let (request, plan) = structural(&row).map_err(|_| Error::Refused)?;
            if request != input.request || !row.is_open() || row.cancel_requested() {
                return Err(Error::Refused);
            }
            let native = request.anchor.current(conn, now).await?;
            if let Some((thread, turn)) = input.focus {
                let current = threads::active_turn(conn, thread)
                    .await?
                    .ok_or(Error::Refused)?;
                if current.id != turn || !writers(conn, &plan).await?.contains(&thread) {
                    return Err(Error::Refused);
                }
            }
            let facts = confirmation_facts(conn, &plan, request.mode).await?;
            Ok((RuntimeValue { native, row, plan }, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        actor: &'a Principal,
        input: &'a RuntimeInput,
        value: &'a RuntimeValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            native_gate(conn, actor, &value.native).await?;
            require_confirmation(conn, actor, &value.plan, input.request.mode).await
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        actor: &'a Principal,
        input: &'a RuntimeInput,
        value: &'a RuntimeValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            jobs::lock(conn, value.row.id)
                .await?
                .ok_or(Error::Refused)?;
            if let Some((thread, _)) = input.focus {
                threads::lock(conn, thread).await?.ok_or(Error::Refused)?;
            }
            ReviewSource::locks(
                conn,
                actor,
                &ReviewInput {
                    anchor: input.request.anchor.clone(),
                    job: Some(input.request.job),
                },
                &value.native,
            )
            .await
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a RuntimeInput,
        old: &'a RuntimeValue,
        fresh: &'a RuntimeValue,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id
                && serde_json::to_value(&old.plan).map_err(EngineError::from)?
                    == serde_json::to_value(&fresh.plan).map_err(EngineError::from)?)
        })
    }
}

struct RuntimeFrame {
    confirmation: continuing::Frame<RuntimeSource>,
    review: Option<ReviewFrame>,
    until: Time,
}
impl RuntimeFrame {
    fn check(self, clock: &dyn Clock) -> Result<Vec<(Option<Caller>, Option<Time>)>, Error> {
        if self.until <= clock.time() {
            return Err(Error::Refused);
        }
        let mut callers = Vec::new();
        if let Some(review) = self.review {
            let until = review.until();
            let (actor, _) = review.into_parts().check(clock)?;
            callers.push((Some(actor.caller().clone()), Some(until)));
        }
        let parts = self.confirmation.into_parts();
        parts.first_effect.check(clock)?;
        callers.push((Some(parts.principal.caller().clone()), Some(self.until)));
        Ok(callers)
    }
}
async fn runtime(
    conn: &mut Conn,
    clock: &dyn Clock,
    request: &Request,
    focus: Option<(Id, Id)>,
) -> Result<RuntimeFrame, Error> {
    let confirmation = continuing::prepare::<RuntimeSource>(
        conn,
        clock,
        &request.origin,
        RuntimeInput {
            request: request.clone(),
            focus,
        },
    )
    .await?;
    let raw = raw_session(
        conn,
        &request.origin,
        request.session,
        confirmation.principal().caller(),
        clock.time(),
    )
    .await?
    .ok_or(Error::Refused)?;
    let until = confirmation
        .value()
        .native
        .snapshot
        .expires_at
        .map(Time::new)
        .ok_or(Error::Refused)?
        .min(request.anchor.until);
    let review = match &request.review {
        Some(review) => Some(review.prepare(conn, clock, Some(request.job), None).await?),
        None => None,
    };
    Ok(RuntimeFrame {
        confirmation,
        review,
        until: raw.map_or(until, |raw| until.min(raw)),
    })
}

/// Closed native continuation modes; cleanup cannot turn into another forward operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommandStage {
    /// A genuinely new forward read/control/filesystem command.
    Forward,
    /// Only the same existing native filesystem journal can run.
    Recovery,
    /// Only the original existing Undo state's metadata can be read.
    UndoMetadata,
    /// Only the actual native job's existing held process identities can be released.
    Resume,
}

fn command_stage(command: &Outgoing) -> Result<CommandStage, Stop> {
    Ok(match command.kind.as_str() {
        "proc.resume.owned" => CommandStage::Resume,
        "fs.record" => CommandStage::UndoMetadata,
        "fs.restore" => {
            let body: FsRestore =
                serde_json::from_value(command.body.clone()).map_err(EngineError::from)?;
            if body.recovery_only == Some(true) {
                CommandStage::Recovery
            } else {
                CommandStage::Forward
            }
        }
        "proc.list" | "proc.suspend.owned" => CommandStage::Forward,
        _ => return Err(refused()),
    })
}
fn body_matches(
    request: &Request,
    kind: &str,
    body: &Json,
    stage: CommandStage,
) -> Result<bool, EngineError> {
    let plan: Plan = serde_json::from_value(request.plan.clone())?;
    let Action::Restore {
        body: expected,
        directory,
        record_days,
        max_bytes,
        suspend,
        ..
    } = &plan.action
    else {
        return Ok(false);
    };
    Ok(match (kind, stage) {
        ("proc.list", CommandStage::Forward) => {
            !*suspend
                && serde_json::from_value::<ProcList>(body.clone())?
                    == ProcList {
                        environment_id: plan.environment,
                    }
        }
        ("proc.suspend.owned", CommandStage::Forward) => {
            *suspend
                && serde_json::from_value::<ProcSuspendOwned>(body.clone())?
                    == ProcSuspendOwned {
                        environment_id: plan.environment,
                        owner_id: request.job,
                    }
        }
        ("proc.resume.owned", CommandStage::Resume) => {
            *suspend
                && serde_json::from_value::<ProcResumeOwned>(body.clone())?
                    == ProcResumeOwned {
                        environment_id: plan.environment,
                        owner_id: request.job,
                    }
        }
        ("fs.record", CommandStage::UndoMetadata) => {
            serde_json::from_value::<FsRecord>(body.clone())?
                == FsRecord {
                    environment_id: plan.environment,
                    snapshot_id: expected.undo_snapshot_id,
                    directory: directory.clone(),
                    kind: RecordKind::Restore,
                    record_days: *record_days,
                    max_bytes: *max_bytes,
                }
        }
        ("fs.restore", CommandStage::Forward | CommandStage::Recovery) => {
            let mut actual: FsRestore = serde_json::from_value(body.clone())?;
            let valid_work = actual
                .work
                .as_ref()
                .is_none_or(|work| work.job_id == request.job);
            let valid_stage = match stage {
                CommandStage::Forward => actual.recovery_only.is_none(),
                CommandStage::Recovery => actual.recovery_only == Some(true),
                _ => false,
            };
            actual.work = None;
            actual.recovery_only = None;
            valid_work && valid_stage && actual == *expected
        }
        _ => false,
    })
}
impl RestoreCommand {
    /// Capture from the actual immutable Job and exact already prepared native command.
    pub(crate) fn job(
        row: &jobs::JobRow,
        expected: &Request,
        command: &Outgoing,
    ) -> Result<Self, Stop> {
        let (request, plan) = structural(row)?;
        let stage = command_stage(command)?;
        if request != *expected
            || command.machine != plan.machine
            || command.key != SerialKey::Environment(plan.environment)
            || !body_matches(&request, command.kind.as_str(), &command.body, stage)?
        {
            return Err(refused());
        }
        Ok(Self::Job {
            request,
            stage,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
}

async fn queued_original(
    conn: &mut Conn,
    row: &jobs::JobRow,
    request: &Request,
    wanted: &str,
) -> Result<bool, Stop> {
    let Some(journal) = row.plan.get(JOURNAL).and_then(Json::as_object) else {
        return Ok(false);
    };
    for (id, recorded) in journal {
        let id = id.parse::<Id>().map_err(|_| refused())?;
        let Some(saved) = orch_db::queries::link::command_with_authority(conn, id).await? else {
            if recorded.get("kind").and_then(Json::as_str) == Some(wanted)
                && row
                    .plan
                    .get(ISSUED)
                    .and_then(|issued| issued.get(id.to_string()))
                    == Some(
                        &serde_json::json!({"canonical":recorded,"stage":CommandStage::Forward}),
                    )
            {
                return Ok(true);
            }
            continue;
        };
        let command = &saved.command;
        if command.kind != wanted {
            continue;
        }
        let canonical = crate::jobs::command_identity::canonical(
            id,
            command.machine_id,
            command.serial_key(),
            &command.kind,
            &command.body,
        )?;
        if *recorded != canonical {
            return Err(refused());
        }
        let authority: CommandAuthority =
            serde_json::from_value(saved.authority.ok_or_else(refused)?)
                .map_err(EngineError::from)?;
        let CommandAuthority::WorkspaceRestore(authority) = authority else {
            return Err(refused());
        };
        let RestoreCommand::Job {
            request: original,
            canonical: admitted,
            stage: CommandStage::Forward,
        } = authority.as_ref()
        else {
            continue;
        };
        if original != request
            || *admitted != canonical
            || !body_matches(request, &command.kind, &command.body, CommandStage::Forward)?
        {
            return Err(refused());
        }
        // The native owned primitive proves actual acceptance itself. A queued/terminal state
        // alone is never an Accepted receipt and cannot make a normal command cleanup.
        return Ok(true);
    }
    Ok(false)
}

/// Durable factual queue acknowledgement, recorded only after the real issue call returned.
/// It keeps owned reconciliation possible after ordinary completed-command retention.
pub(crate) async fn issued(job: &Job, id: Id) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let (request, _) = structural(&row)?;
    let saved = orch_db::queries::link::command_with_authority(&mut tx, id)
        .await?
        .ok_or_else(refused)?;
    let command = &saved.command;
    let canonical = crate::jobs::command_identity::canonical(
        id,
        command.machine_id,
        command.serial_key(),
        &command.kind,
        &command.body,
    )?;
    if row
        .plan
        .get(JOURNAL)
        .and_then(|journal| journal.get(id.to_string()))
        != Some(&canonical)
    {
        return Err(refused());
    }
    let authority: CommandAuthority =
        serde_json::from_value(saved.authority.ok_or_else(refused)?).map_err(EngineError::from)?;
    let CommandAuthority::WorkspaceRestore(authority) = authority else {
        return Err(refused());
    };
    let RestoreCommand::Job {
        request: accepted,
        canonical: actual,
        stage,
    } = authority.as_ref()
    else {
        return Err(refused());
    };
    if *accepted != request
        || *actual != canonical
        || !body_matches(&request, &command.kind, &command.body, *stage)?
    {
        return Err(refused());
    }
    let mut issued = row
        .plan
        .get(ISSUED)
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    issued.as_object_mut().ok_or_else(refused)?.insert(
        id.to_string(),
        serde_json::json!({"canonical":canonical,"stage":stage}),
    );
    jobs::merge_plan(
        &mut tx,
        row.id,
        &serde_json::json!({ISSUED:issued}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}
async fn physical_cleanup(conn: &mut Conn, request: &Request) -> Result<(), Stop> {
    let Some(env) = environments::environment(conn, request.anchor.environment).await? else {
        return Err(refused());
    };
    if env.generation != request.anchor.generation
        || env.machine_id != Some(request.anchor.machine)
        || env.workspace_root != request.anchor.root
    {
        return Err(refused());
    }
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
        .bind(env.id)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
        .bind(request.anchor.machine)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    let current = environments::environment(conn, env.id)
        .await?
        .ok_or_else(refused)?;
    if current.generation != env.generation
        || current.machine_id != env.machine_id
        || current.workspace_root != env.workspace_root
    {
        return Err(refused());
    }
    Ok(())
}
async fn current_job_callers(
    conn: &mut Conn,
    clock: &dyn Clock,
    expected: &Request,
    command: &NewCommand<'_>,
    stage: CommandStage,
) -> Result<Vec<(Option<Caller>, Option<Time>)>, Stop> {
    let row = jobs::lock(conn, expected.job).await?.ok_or_else(refused)?;
    let (request, _) = structural(&row)?;
    if request != *expected
        || command.machine_id != request.anchor.machine
        || command.serial_key
            != SerialKey::Environment(request.anchor.environment)
                .wire()
                .as_deref()
        || !body_matches(&request, command.kind, command.body, stage)?
        || row
            .plan
            .get(JOURNAL)
            .and_then(|journal| journal.get(command.id.to_string()))
            != Some(&crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )?)
    {
        return Err(refused());
    }
    if stage == CommandStage::Forward {
        return runtime(conn, clock, &request, None)
            .await
            .map_err(|_| refused())?
            .check(clock)
            .map_err(|_| refused());
    }
    physical_cleanup(conn, &request).await?;
    let original = if stage == CommandStage::Resume {
        "proc.suspend.owned"
    } else {
        "fs.restore"
    };
    if !queued_original(conn, &row, &request, original).await? || command.deadline_at <= clock.now()
    {
        return Err(refused());
    }
    Ok(vec![(None, Some(Time::new(command.deadline_at)))])
}

/// Same canonical Job command after every runner/retained-envelope wait, before native issue.
pub(crate) async fn decorate(
    job: &Job,
    command: &mut Outgoing,
    respect_cancel: bool,
) -> Result<(), Stop> {
    let expected = match &command.authority {
        Some(CommandAuthority::WorkspaceRestore(authority)) => match authority.as_ref() {
            RestoreCommand::Job { request, .. } => request.clone(),
            _ => return Err(refused()),
        },
        _ => return Err(refused()),
    };
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if respect_cancel && let Some(stop) = super::super::runner::abort_reason(&row)? {
        return Err(stop);
    }
    let authority = RestoreCommand::job(&row, &expected, command)?;
    let stage = command_stage(command)?;
    let saved = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(EngineError::from)?;
    let known = saved
        .as_ref()
        .is_some_and(|status| matches!(status.stage, Stage::Accepted | Stage::Over));
    if !known {
        if stage == CommandStage::Forward {
            runtime(&mut tx, job.engine().inner.clock.as_ref(), &expected, None)
                .await
                .map_err(|_| refused())?
                .check(job.engine().inner.clock.as_ref())
                .map_err(|_| refused())?;
        } else {
            physical_cleanup(&mut tx, &expected).await?;
            let original = if stage == CommandStage::Resume {
                "proc.suspend.owned"
            } else {
                "fs.restore"
            };
            if !queued_original(&mut tx, &row, &expected, original).await? {
                return Err(refused());
            }
        }
    }
    crate::jobs::command_identity::remember(&mut tx, &row, JOURNAL, command, Time::new(job.now()))
        .await?;
    tx.commit().await.map_err(DbError::from)?;
    command.authority = Some(CommandAuthority::workspace_restore(authority));
    Ok(())
}

/// New forward stages use both exact current operations; existing native reconciliation does not.
pub(crate) async fn check(job: &Job, plan: &Plan, expected: &Request) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    let (request, current) = structural(&row)?;
    if row.cancel_requested() {
        return Err(Stop::Cancelled);
    }
    if request != *expected
        || serde_json::to_value(plan).map_err(EngineError::from)?
            != serde_json::to_value(&current).map_err(EngineError::from)?
    {
        return Err(refused());
    }
    runtime(&mut tx, job.engine().inner.clock.as_ref(), &request, None)
        .await
        .map_err(|_| refused())?
        .check(job.engine().inner.clock.as_ref())
        .map_err(|_| refused())?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// The actual native point-of-no-cancel/recovery intent, guarded on its own existing Job row.
pub(crate) async fn before_apply(
    job: &Job,
    expected: &Request,
    recovering: bool,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let (request, _) = structural(&row)?;
    if request != *expected {
        return Err(refused());
    }
    if !recovering {
        if let Some(stop) = super::super::runner::abort_reason(&row)? {
            return Err(stop);
        }
        runtime(&mut tx, job.engine().inner.clock.as_ref(), &request, None)
            .await
            .map_err(|_| refused())?
            .check(job.engine().inner.clock.as_ref())
            .map_err(|_| refused())?;
        if !jobs::pass_commit(&mut tx, row.id, job.now()).await? {
            return Err(Stop::Cancelled);
        }
    } else {
        physical_cleanup(&mut tx, &request).await?;
        if !queued_original(&mut tx, &row, &request, "fs.restore").await? {
            return Err(refused());
        }
    }
    jobs::merge_plan(
        &mut tx,
        row.id,
        &serde_json::json!({"restore_recovery_required":true}),
        job.now(),
    )
    .await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// An actual absent queue insert proves that this job never started a native filesystem act.
pub(crate) async fn not_issued(job: &Job, expected: &Request) -> Result<bool, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    let (request, _) = structural(&row)?;
    if request != *expected {
        return Err(refused());
    }
    let issued = queued_original(&mut tx, &row, &request, "fs.restore").await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(!issued)
}

pub(crate) async fn has_suspension(job: &Job, expected: &Request) -> Result<bool, Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = jobs::job(&mut tx, job.id()).await?.ok_or(Stop::Lost)?;
    let (request, _) = structural(&row)?;
    if request != *expected {
        return Err(refused());
    }
    let has = queued_original(&mut tx, &row, &request, "proc.suspend.owned").await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(has)
}
pub(crate) async fn active_turn(
    job: &Job,
    plan: &Plan,
    thread: Id,
) -> Result<Option<threads::TurnRow>, Stop> {
    let mut conn = job.engine().db().acquire().await?;
    let Some(turn) = threads::active_turn(&mut conn, thread).await? else {
        return Ok(None);
    };
    let here: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM segments WHERE id=$1 AND environment_id=$2 AND env_generation=$3)")
        .bind(turn.segment_id).bind(plan.environment).bind(plan.generation).fetch_one(&mut *conn).await.map_err(DbError::from)?;
    Ok(here.then_some(turn))
}
pub(crate) async fn reconcile(job: &Job, expected: &Request) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let (request, _) = structural(&row)?;
    if request != *expected || !queued_original(&mut tx, &row, &request, "fs.restore").await? {
        return Err(refused());
    }
    physical_cleanup(&mut tx, &request).await?;
    tx.commit().await.map_err(DbError::from)?;
    Ok(())
}

/// Stamp only the actual current writer of this physical Environment, on the same guarded Unit.
pub(crate) async fn interrupt(
    job: &Job,
    plan: &Plan,
    request: &Request,
    thread: Id,
) -> Result<(), Stop> {
    let mut tx = job.engine().db().begin().await?;
    let owner = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let (current, native) = structural(&owner)?;
    if current != *request
        || serde_json::to_value(&native).map_err(EngineError::from)?
            != serde_json::to_value(plan).map_err(EngineError::from)?
    {
        return Err(refused());
    }
    threads::lock(&mut tx, thread).await?;
    let active = threads::active_turn(&mut tx, thread).await?;
    let Some(turn) = active else { return Ok(()) };
    sqlx::query("SELECT id FROM turns WHERE id=$1 FOR UPDATE")
        .bind(turn.id)
        .execute(&mut *tx)
        .await
        .map_err(DbError::from)?;
    let turn = threads::turn(&mut tx, turn.id).await?.ok_or_else(refused)?;
    let here: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM segments WHERE id=$1 AND environment_id=$2 AND env_generation=$3)")
        .bind(turn.segment_id).bind(plan.environment).bind(plan.generation).fetch_one(&mut *tx).await.map_err(DbError::from)?;
    if !here {
        return Ok(());
    }
    let frame = runtime(
        &mut tx,
        job.engine().inner.clock.as_ref(),
        request,
        Some((thread, turn.id)),
    )
    .await
    .map_err(|_| refused())?;
    let until = frame.until.min(
        frame
            .review
            .as_ref()
            .map_or(frame.until, ReviewFrame::until),
    );
    let mut deadline_callers = vec![frame.confirmation.principal().caller().clone()];
    if let Some(review) = &frame.review {
        deadline_callers.push(review.principal().caller().clone());
    }
    let clock = job.engine().inner.clock.clone();
    let mut unit = crate::unit::Unit::with_first_effect(&mut tx, job.now(), move || {
        frame
            .check(clock.as_ref())
            .map(|_| ())
            .map_err(EngineError::unreadable)
    });
    super::super::fork::interrupt_for_change_in(&mut unit, thread, &turn, plan.caller).await?;
    let finished = unit.finish().await?;
    if until <= job.engine().inner.clock.time()
        || deadline_callers
            .iter()
            .any(|caller| !caller.key_deadline_allows(job.engine().inner.clock.as_ref()))
    {
        return Err(refused());
    }
    job.engine().commit(tx, finished).await?;
    job.engine().drive_interrupt(thread).await?;
    Ok(())
}
