//! The actual original request and native unit at a recycling probe or stop boundary.

use super::Candidate;
use crate::commands::{CommandAuthority, Outgoing};
use crate::operation_scope::{
    continuing::{self, Error, Source},
    request_origin::{self, Credential},
    sources::Facts,
    targets,
};
use crate::{EngineError, updates};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{NodePermission, ObjectKind},
};
use orch_api_contract::{
    environment::NodeEnvironmentRecycle,
    environment_sources::{RECYCLE, RecycleSource as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::{Act, Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn,
    queries::{
        environments::hosting as envs,
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{CommandKind, ProcList, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

const OP: &str = "NodeEnvironmentRecycle";

/// A known native request source that closed never becomes an ordinary caller.
#[derive(Debug, thiserror::Error)]
#[error("the original recycling request no longer permits this work")]
pub struct Ended;
/// The current original caller may not manage this candidate; other candidates stay independent.
#[derive(Debug, thiserror::Error)]
#[error("the current recycling caller may not manage this candidate")]
pub struct Denied;
fn engine(error: Error) -> EngineError {
    match error {
        Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused) => {
            EngineError::BeforeEffect(Box::new(Ended))
        }
        Error::Native(error) => error,
        error => EngineError::unreadable(error),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Association {
    environment: Id,
    owner: Id,
    home: Id,
    machine: Option<Id>,
    generation: i32,
    runtime: Option<Id>,
    host: Option<Id>,
}
impl Association {
    fn of(candidate: &Candidate) -> Self {
        Self {
            environment: candidate.state.id,
            owner: candidate.state.owner_id,
            home: candidate.state.home_node_id,
            machine: candidate.state.machine_id,
            generation: candidate.state.generation,
            runtime: candidate.runtime.as_ref().map(|runtime| runtime.id),
            host: candidate.runtime.as_ref().map(|runtime| runtime.host_id),
        }
    }
}

/// Immutable real candidate/credential association, never reusable admission or a fake session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    node: Id,
    idle_minutes: u32,
    association: Association,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecycleInput([private])")
    }
}
impl Input {
    /// The trusted HTTP/native supplier binds its actual current candidate and admitted operation.
    pub fn capture(
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        node: Id,
        idle_minutes: u32,
        candidate: &Candidate,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(OP) || !credential.matches(caller) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            node,
            idle_minutes,
            association: Association::of(candidate),
        })
    }
    /// Actual selected Environment; this id supplies no permission by itself.
    pub fn environment(&self) -> Id {
        self.association.environment
    }
    /// Check both the actual raw credential and the original/current native source after waits.
    pub async fn checkpoint(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Checkpoint, EngineError> {
        let (caller, until) =
            request_origin::current(conn, clock, &self.origin, &self.credential, self.until, OP)
                .await
                .map_err(engine)?;
        let frame = match continuing::prepare_with_caller::<Native>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.clone(),
        )
        .await
        {
            Ok(frame) => frame,
            Err(Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused)) => {
                // The native locks may have waited past this exact request's lifetime.
                // Only a still-current credential may skip an independently denied candidate.
                request_origin::current(
                    conn,
                    clock,
                    &self.origin,
                    &self.credential,
                    self.until,
                    OP,
                )
                .await
                .map_err(engine)?;
                return Err(EngineError::BeforeEffect(Box::new(Denied)));
            }
            Err(error) => return Err(engine(error)),
        };
        let check = Checkpoint {
            frame,
            until,
            input: self.clone(),
        };
        check.check(clock)?;
        Ok(check)
    }
    /// Seal only the original native process query for this real physical unit.
    pub async fn seal_probe(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        mut command: Outgoing,
    ) -> Result<Option<(Outgoing, Input)>, EngineError> {
        let check = self.checkpoint(conn, clock).await?;
        if !check.matches() {
            return Ok(None);
        }
        let input = check.accepted_input()?;
        let evidence = CommandEvidence {
            input: input.clone(),
            id: command.id,
            machine: command.machine,
            serial: command.key.wire(),
            body: command.body.clone(),
        };
        if command.kind != CommandKind::ProcList
            || command.key != SerialKey::Environment(self.environment())
            || !evidence.native()
        {
            return Err(EngineError::unreadable(
                "the recycling probe differs from its native source",
            ));
        }
        check.check(clock)?;
        let parts = check.frame.into_parts();
        parts.first_effect.check(clock).map_err(engine)?;
        command.authority = Some(CommandAuthority::recycle_probe(evidence));
        Ok(Some((command, input)))
    }
}

/// Current native candidate and its real raw-credential lifetime, on the caller's transaction.
pub struct Checkpoint {
    /// The native source frame; its first-effect cut is consumed by the actual stop writer.
    pub frame: continuing::Frame<Native>,
    /// The current/original minimum of this actual raw credential.
    pub until: Option<Time>,
    input: Input,
}
impl Checkpoint {
    /// Retain this actual accepted source's caller and raw minimum for its next native boundary.
    pub fn accepted_input(&self) -> Result<Input, EngineError> {
        let mut input = self.input.clone();
        input.origin = CapturedOperation::capture(self.frame.principal().caller())
            .map_err(EngineError::unreadable)?;
        input.until = match (input.until, self.until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(input)
    }
    /// The original physical unit still supplies this process observation.
    pub fn matches(&self) -> bool {
        self.frame.value().matches && self.frame.value().under
    }
    /// Recheck the exact raw/key lifetime without issuing or accepting any work.
    pub fn check(&self, clock: &dyn Clock) -> Result<(), EngineError> {
        self.frame.check_deadline(clock).map_err(engine)?;
        if self.until.is_some_and(|end| clock.time() >= end) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(())
    }
}

/// The same native ordinary gates and source facts on HTTP and command dispatch.
pub struct Native;
/// Current factual native operands, with no IO or formatted reply.
pub struct Value {
    /// Current real idle candidate, when native eligibility still holds.
    pub candidate: Option<Candidate>,
    /// The original node still selects this Environment through the native membership query.
    pub under: bool,
    /// Current native unit/registration agrees with the original observation.
    pub matches: bool,
    home: Id,
    machine: Option<Id>,
    runtime: Option<Id>,
}
impl Source for Native {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = NodeEnvironmentRecycle::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &RECYCLE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let state = envs::environment(conn, input.environment())
                .await?
                .filter(|state| state.deleted_at.is_none())
                .ok_or(Error::Refused)?;
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: state.id,
                },
            )
            .await
            .map_err(|error| Error::Native(EngineError::unreadable(error)))?;
            let candidate = super::candidate_in(conn, state.id, input.idle_minutes, now).await?;
            let matches = candidate
                .as_ref()
                .is_some_and(|candidate| Association::of(candidate) == input.association);
            let under = envs::environments_under(conn, input.node)
                .await?
                .contains(&state.id);
            let runtime = envs::live_runtime(conn, state.id)
                .await?
                .map(|runtime| runtime.id);
            Ok((
                Value {
                    home: state.home_node_id,
                    machine: state.machine_id,
                    runtime,
                    candidate,
                    under,
                    matches,
                },
                vec![Facts::collection(
                    Role::Environments,
                    vec![(KeyTargetKind::Object(ObjectKind::Environment), vec![target])],
                )],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        _: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let user = orch_db::queries::identity::user_by_id(conn, principal.user())
                .await?
                .ok_or(Error::Refused)?;
            let hold = crate::operation_scope::account_policy::session_hold(
                conn,
                &user,
                input.credential.browser_session(),
            )
            .await?;
            let principal = match hold {
                Some(hold) => principal.clone().held_for_second_step(hold),
                None => principal.clone(),
            };
            let graph = orch_authz::engine::Graph::load(conn, &[input.node]).await?;
            let node = graph.get(input.node).ok_or(Error::Refused)?;
            let permission = NodePermission::View.on(node.kind).ok_or(Error::Refused)?;
            if !orch_authz::nodes::check(
                conn,
                &principal,
                input.node,
                permission,
                Act::Use,
                Policy::default(),
            )
            .await?
            .is_allowed()
                || !updates::may_manage(
                    conn,
                    &principal,
                    updates::Target::Environment(input.environment()),
                )
                .await?
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Input,
        value: &'a Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            envs::lock_environment(conn, input.environment())
                .await?
                .ok_or(Error::Refused)?;
            if let Some(runtime) = value.runtime {
                sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
                    .bind(runtime)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            if let Some(machine) = value.machine {
                sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                    .bind(machine)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
            let mut nodes = vec![input.node, value.home];
            nodes.sort_unstable();
            nodes.dedup();
            sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
                .bind(nodes)
                .execute(&mut *conn)
                .await
                .map_err(orch_db::DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a Input,
        opened: &'a Value,
        fresh: &'a Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(opened.home == fresh.home
                && opened.machine == fresh.machine
                && opened.runtime == fresh.runtime
                && (!opened.under || fresh.under)
                && (!opened.matches || fresh.matches))
        })
    }
}

/// Private evidence for only the native proc.list built by the real recycling supplier.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandEvidence {
    input: Input,
    id: Id,
    machine: Id,
    serial: Option<String>,
    body: Json,
}
impl std::fmt::Debug for CommandEvidence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RecycleProbe([private])")
    }
}
impl CommandEvidence {
    fn native(&self) -> bool {
        self.input.association.machine == Some(self.machine)
            && self.serial == SerialKey::Environment(self.input.environment()).wire()
            && serde_json::to_value(ProcList {
                environment_id: self.input.environment(),
            })
            .is_ok_and(|body| body == self.body)
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: CommandView<'_>,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if !self.native()
            || command.id != self.id
            || command.machine != self.machine
            || command.kind != "proc.list"
            || command.serial != self.serial.as_deref()
            || *command.body != self.body
        {
            return Ok(None);
        }
        let check = match self.input.checkpoint(conn, clock).await {
            Ok(check) => check,
            Err(EngineError::BeforeEffect(error))
                if error.is::<Ended>() || error.is::<Denied>() =>
            {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if !check.matches() || !check.frame.value().under {
            return Ok(None);
        }
        check.check(clock)?;
        Ok(Some(vec![(
            Some(check.frame.principal().caller().clone()),
            check.until,
        )]))
    }
    /// The real queue proposal, not a fabricated stored command.
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                kind: command.kind,
                serial: command.serial_key,
                body: command.body,
            },
        )
        .await
    }
    /// Refresh actual unaccepted dispatch; accepted command reconciliation remains native.
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current(
            conn,
            clock,
            CommandView {
                id: command.id,
                machine: command.machine_id,
                kind: &command.kind,
                serial: command.serial_key(),
                body: &command.body,
            },
        )
        .await
    }
}
struct CommandView<'a> {
    id: Id,
    machine: Id,
    kind: &'a str,
    serial: Option<&'a str>,
    body: &'a Json,
}
