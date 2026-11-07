//! The actual original request and native unit at update-planning catalogue and file-estimate boundaries.

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
    environment::NodeEnvironmentUpdatePreflight,
    environment_sources::update_batch::{NODE_PREFLIGHT, Source as Role},
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
use orch_link_contract::commands::{CommandKind, NodeKeepEstimate, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

const OP: &str = "NodeEnvironmentUpdatePreflight";

/// A known native request source that closed never becomes an ordinary caller.
#[derive(Debug, thiserror::Error)]
#[error("the original update planning request no longer permits this work")]
pub struct Ended;
/// The current original caller may not manage this candidate; other candidates stay independent.
#[derive(Debug, thiserror::Error)]
#[error("the current update planning caller may not manage this candidate")]
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
    runtime_machine: Option<Id>,
    generation: i32,
    runtime: Option<Id>,
    host: Option<Id>,
}
impl Association {
    async fn load(conn: &mut Conn, environment: Id) -> Result<Self, EngineError> {
        let state = envs::environment(conn, environment)
            .await?
            .filter(|state| state.deleted_at.is_none())
            .ok_or_else(|| EngineError::BeforeEffect(Box::new(Ended)))?;
        let runtime = envs::live_runtime(conn, environment).await?;
        let runtime_machine = match &runtime {
            Some(runtime) => orch_db::queries::resources::hosting::host(conn, runtime.host_id)
                .await?
                .map(|host| host.machine_id),
            None => None,
        };
        Ok(Self {
            environment,
            owner: state.owner_id,
            home: state.home_node_id,
            machine: state.machine_id,
            runtime_machine,
            generation: state.generation,
            runtime: runtime.as_ref().map(|runtime| runtime.id),
            host: runtime.as_ref().map(|runtime| runtime.host_id),
        })
    }
}

async fn lock_rows(
    conn: &mut Conn,
    association: &Association,
    node: Id,
) -> Result<(), EngineError> {
    let _ = envs::lock_environment(conn, association.environment).await?;
    // A node's selection includes both home membership and threads bound under that node.
    sqlx::query(
        "SELECT t.node_id FROM threads t JOIN nodes n ON n.id=t.node_id
        WHERE t.environment_id=$1 AND n.deleted_at IS NULL ORDER BY t.node_id FOR SHARE OF t,n",
    )
    .bind(association.environment)
    .execute(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    let mut nodes = vec![node, association.home];
    nodes.sort_unstable();
    nodes.dedup();
    sqlx::query("SELECT id FROM nodes WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(nodes)
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    if let Some(runtime) = association.runtime {
        sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
            .bind(runtime)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    if let Some(host) = association.host {
        sqlx::query("SELECT id FROM hosts WHERE id=$1 FOR SHARE")
            .bind(host)
            .execute(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?;
    }
    let mut machines = association
        .machine
        .into_iter()
        .chain(association.runtime_machine)
        .collect::<Vec<_>>();
    machines.sort_unstable();
    machines.dedup();
    sqlx::query("SELECT id FROM machines WHERE id=ANY($1) ORDER BY id FOR SHARE")
        .bind(machines)
        .execute(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
    Ok(())
}

/// Immutable real candidate/credential association, never reusable admission or a fake session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    node: Id,
    association: Association,
}
impl std::fmt::Debug for Input {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UpdatePlanningInput([private])")
    }
}
impl Input {
    /// The trusted HTTP/native supplier binds its actual current candidate and admitted operation.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        node: Id,
        environment: Id,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(OP) || !credential.matches(caller) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            node,
            association: Association::load(conn, environment).await?,
        })
    }
    /// The exact admitted minimum accepted at the native observation boundaries.
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    /// Earliest real raw-credential expiry retained by those boundaries.
    pub fn until(&self) -> Option<Time> {
        self.until
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
        let frame = continuing::prepare_with_caller::<Native>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.clone(),
        )
        .await
        .map_err(|error| match error {
            Error::Refused | Error::Binding(crate::operation_scope::BindingError::Refused) => {
                EngineError::BeforeEffect(Box::new(Denied))
            }
            error => engine(error),
        })?;
        let check = Checkpoint {
            frame,
            until,
            input: self.clone(),
        };
        check.check(clock)?;
        Ok(check)
    }
    /// Validate the retained native source without IO, rendering or a new authority claim.
    pub async fn matches(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        Ok(
            Association::load(conn, self.environment()).await? == self.association
                && envs::environments_under(conn, self.node)
                    .await?
                    .contains(&self.environment()),
        )
    }

    /// Pin native membership/runtime inputs before a pure action replan on its real connection.
    pub async fn lock(&self, conn: &mut Conn) -> Result<(), EngineError> {
        lock_rows(conn, &self.association, self.node).await?;
        if !self.matches(conn).await? {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(())
    }

    /// Refresh the real original operation before and after external catalogue waits.
    pub async fn current(&mut self, engine: &crate::Engine) -> Result<(), EngineError> {
        let mut tx = engine.db().begin().await?;
        let check = self
            .checkpoint(&mut tx, engine.inner.clock.as_ref())
            .await?;
        if !check.matches() {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        check.check(engine.inner.clock.as_ref())?;
        let accepted = check.accepted_input()?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        *self = accepted;
        Ok(())
    }

    /// Issue one bounded native observation; guarded replanning never calls this supplier.
    pub(super) async fn estimate_files(
        &mut self,
        engine: &crate::Engine,
        machine: Id,
        runtime: Id,
    ) -> Result<Option<orch_driver_contract::kept::KeepEstimate>, EngineError> {
        let command = Outgoing::new(
            orch_types::new_id(),
            machine,
            SerialKey::Runtime(runtime),
            &NodeKeepEstimate {
                runtime_id: runtime,
            },
        )?;
        let mut tx = engine.db().begin().await?;
        let Some((command, mut accepted)) = self
            .seal_probe(&mut tx, engine.inner.clock.as_ref(), command)
            .await?
        else {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        let result = engine.ask_bounded(command).await?;
        accepted.current(engine).await?;
        *self = accepted;
        Ok(match result {
            Some(crate::commands::Outcome::Succeeded(value)) => serde_json::from_value(value).ok(),
            _ => None,
        })
    }

    /// Seal only the original file estimate for this real physical unit.
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
        if command.kind != CommandKind::NodeKeepEstimate
            || Some(command.key.clone()) != self.association.runtime.map(SerialKey::Runtime)
            || !evidence.native()
        {
            return Err(EngineError::unreadable(
                "the file estimate differs from its native update source",
            ));
        }
        check.check(clock)?;
        let parts = check.frame.into_parts();
        parts.first_effect.check(clock).map_err(engine)?;
        command.authority = Some(CommandAuthority::update_planning_probe(evidence));
        Ok(Some((command, input)))
    }
}

/// Current native candidate and its real raw-credential lifetime, on the caller's transaction.
pub struct Checkpoint {
    /// The native source frame; its first-effect cut is consumed by the native observation supplier.
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
    /// The original physical unit still supplies this file observation.
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
    association: Association,
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
    const ROUTE: Route = NodeEnvironmentUpdatePreflight::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &NODE_PREFLIGHT;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        _: Time,
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
            let association = Association::load(conn, state.id).await?;
            let matches = association == input.association;
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
                    association,
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
            lock_rows(conn, &value.association, input.node).await?;
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
            Ok(opened.association == fresh.association
                && opened.home == fresh.home
                && opened.machine == fresh.machine
                && opened.runtime == fresh.runtime
                && (!opened.under || fresh.under)
                && (!opened.matches || fresh.matches))
        })
    }
}

/// Private evidence for the exact file estimate built by the native update planner.
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
        f.write_str("UpdatePlanningProbe([private])")
    }
}
impl CommandEvidence {
    /// Take the factual actor/unit fence before the command row, without granting authority.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ =
            orch_db::queries::transaction_guard::lock_native_actor(conn, self.input.origin.user())
                .await?;
        if envs::environment(conn, self.input.environment())
            .await?
            .is_some()
        {
            let current = Association::load(conn, self.input.environment()).await?;
            lock_rows(conn, &current, self.input.node).await?;
        }
        Ok(())
    }
    fn native(&self) -> bool {
        self.input.association.runtime_machine == Some(self.machine)
            && self.input.association.runtime.is_some_and(|runtime| {
                self.serial == SerialKey::Runtime(runtime).wire()
                    && serde_json::to_value(NodeKeepEstimate {
                        runtime_id: runtime,
                    })
                    .is_ok_and(|body| body == self.body)
            })
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
            || command.kind != "node.runtime.keep-estimate"
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
