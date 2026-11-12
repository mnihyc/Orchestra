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
    environment::{
        EnvironmentUpdateBatchCreate, EnvironmentUpdateBatchPreflight, NodeEnvironmentUpdateCreate,
        NodeEnvironmentUpdatePreflight,
    },
    environment_sources::update_batch::{
        GLOBAL_CREATE, GLOBAL_PREFLIGHT, NODE_CREATE, NODE_PREFLIGHT, Source as Role,
    },
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

const PREFLIGHT: &str = "NodeEnvironmentUpdatePreflight";
const CREATE: &str = "NodeEnvironmentUpdateCreate";
const GLOBAL_REVIEW: &str = "EnvironmentUpdateBatchPreflight";
const GLOBAL_CONFIRM: &str = "EnvironmentUpdateBatchCreate";

/// A known native request source that closed never becomes an ordinary caller.
#[derive(Debug, thiserror::Error)]
#[error("the original update planning request no longer permits this work")]
pub struct Ended;
/// A preparation review's expiry never becomes a deadline on an already accepted job.
#[derive(Debug, thiserror::Error)]
#[error("the native review expired before acceptance")]
pub struct ReviewExpired;
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
    node: Option<Id>,
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
    let mut nodes = std::iter::once(association.home)
        .chain(node)
        .collect::<Vec<_>>();
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

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewRef {
    id: Id,
    hash: orch_types::Hash,
    expires_at: Time,
}

/// Immutable real candidate/credential association, never reusable admission or a fake session.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    origin: CapturedOperation,
    credential: Credential,
    until: Option<Time>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    node: Option<Id>,
    association: Association,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    execution_template: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    review: Option<ReviewRef>,
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
        if !matches!(caller.admitted_operation(), Some(PREFLIGHT | CREATE))
            || !credential.matches(caller)
        {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            node: Some(node),
            association: Association::load(conn, environment).await?,
            execution_template: None,
            review: None,
        })
    }
    /// Capture this same source for a genuine global collection, never a node alias.
    pub async fn capture_global(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        environment: Id,
        create: bool,
    ) -> Result<Self, EngineError> {
        let operation = if create {
            GLOBAL_CONFIRM
        } else {
            GLOBAL_REVIEW
        };
        if caller.admitted_operation() != Some(operation) || !credential.matches(caller) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            credential,
            until,
            node: None,
            association: Association::load(conn, environment).await?,
            execution_template: None,
            review: None,
        })
    }
    /// Exact node creator over the same native source.
    pub async fn capture_node_create(
        conn: &mut Conn,
        caller: &Caller,
        credential: Credential,
        until: Option<Time>,
        node: Id,
        environment: Id,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some(CREATE) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Self::capture(conn, caller, credential, until, node, environment).await
    }
    fn validate(&self) -> Result<(), EngineError> {
        if !matches!(
            (self.origin.operation(), self.node),
            (PREFLIGHT | CREATE, Some(_)) | (GLOBAL_REVIEW | GLOBAL_CONFIRM, None)
        ) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(())
    }
    /// Real optional selector, without a dummy parent.
    pub fn node_scope(&self) -> Option<Id> {
        self.node
    }
    /// Actual raw browser identity for current native proof policy.
    pub fn browser_session(&self) -> Option<Id> {
        self.credential.browser_session()
    }
    /// Immutable source association of two independently checked update acts.
    pub fn same_source(&self, other: &Self) -> bool {
        self.node == other.node && self.association == other.association
    }
    /// Current raw origin used for completed Read, without re-admitting completed writes.
    pub async fn restore_current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(Caller, Option<Time>), EngineError> {
        self.validate()?;
        request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            self.origin.operation(),
        )
        .await
        .map_err(engine)
    }
    /// Current source and accepted minimum for a native boundary.
    pub async fn current_authority(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Current, EngineError> {
        let check = self.checkpoint(conn, clock).await?;
        if !check.matches() {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        let caller = check.principal().caller().clone();
        let until = check.until;
        let input = check.into_accepted(clock)?;
        Ok(Current {
            input,
            caller,
            until,
        })
    }
    /// Attach the actual native reviewed batch, including its expiry only during preparation.
    pub async fn reviewing(mut self, conn: &mut Conn, id: Id) -> Result<Self, EngineError> {
        let row = orch_db::queries::jobs::forks::preflight(conn, id)
            .await?
            .ok_or_else(|| EngineError::BeforeEffect(Box::new(Ended)))?;
        if !matches!(self.origin.operation(), CREATE | GLOBAL_CONFIRM)
            || row.kind != "update_batch"
            || row.node_id != self.node
            || row.created_by != self.origin.user()
            || row.plan["providers"] != false
        {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        let recipes: Vec<updates::Recipe> = serde_json::from_value(row.plan["recipes"].clone())?;
        if !recipes.iter().any(|recipe| {
            recipe.target == updates::Target::Environment(self.environment())
                && Some(recipe.old) == self.runtime()
                && recipe.by == self.origin.user()
        }) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        self.review = Some(ReviewRef {
            id,
            hash: crate::jobs::command_identity::hash(&row.plan)?,
            expires_at: Time::new(row.expires_at),
        });
        Ok(self)
    }

    /// Freeze the actual prepared destination when a native replacement accepts this source.
    pub fn for_execution(&self, template: Id) -> Self {
        let mut input = self.clone();
        input.execution_template = Some(template);
        input.review = None;
        input
    }
    /// Actual frozen native choice; a planning source is never an executable job origin.
    pub fn execution_template(&self) -> Option<Id> {
        self.execution_template
    }

    /// Actual runtime association for a stored native review, without authority.
    pub fn runtime(&self) -> Option<Id> {
        self.association.runtime
    }

    /// Compare only the immutable actual request/unit association, never grants or refreshed deadlines.
    pub fn same_request(&self, other: &Self) -> bool {
        self.origin.user() == other.origin.user()
            && self.origin.operation() == other.origin.operation()
            && self.credential == other.credential
            && self.node == other.node
            && self.association == other.association
            && self.execution_template == other.execution_template
            && self.review == other.review
    }

    /// Intersect another actual accepted frame; this can only narrow the original source.
    pub async fn narrow(
        &mut self,
        conn: &mut Conn,
        clock: &dyn Clock,
        caller: &Caller,
        until: Option<Time>,
    ) -> Result<(), EngineError> {
        let original = self
            .origin
            .restore_current(conn, clock)
            .await?
            .ok_or_else(|| EngineError::BeforeEffect(Box::new(Ended)))?;
        let caller = original
            .intersect_restrictions(caller)
            .map_err(EngineError::unreadable)?;
        let caller = match self.origin.operation() {
            PREFLIGHT => admit_minimum::<false, false>(conn, clock, &caller, self).await?,
            CREATE => admit_minimum::<true, false>(conn, clock, &caller, self).await?,
            GLOBAL_REVIEW => admit_minimum::<false, true>(conn, clock, &caller, self).await?,
            GLOBAL_CONFIRM => admit_minimum::<true, true>(conn, clock, &caller, self).await?,
            _ => return Err(EngineError::BeforeEffect(Box::new(Ended))),
        };
        self.origin = CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?;
        self.until = match (self.until, until) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (a, b) => a.or(b),
        };
        Ok(())
    }

    /// The same real raw/key minimum immediately before a native transaction commits.
    /// This does not reselect a source that the transaction has just legitimately advanced.
    pub async fn check_deadline(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<(), EngineError> {
        let (caller, until) = request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            self.origin.operation(),
        )
        .await
        .map_err(engine)?;
        if !caller.key_deadline_allows(clock) || until.is_some_and(|until| clock.time() >= until) {
            return Err(EngineError::BeforeEffect(Box::new(Ended)));
        }
        Ok(())
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
        self.validate()?;
        let (caller, until) = request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            self.origin.operation(),
        )
        .await
        .map_err(engine)?;
        let frame = match self.origin.operation() {
            PREFLIGHT => continuing::prepare_with_caller::<Native<false>>(
                conn,
                clock,
                &self.origin,
                &caller,
                self.clone(),
            )
            .await
            .map(|frame| NativeFrame::Preflight(Box::new(frame))),
            CREATE => continuing::prepare_with_caller::<Native<true>>(
                conn,
                clock,
                &self.origin,
                &caller,
                self.clone(),
            )
            .await
            .map(|frame| NativeFrame::Create(Box::new(frame))),
            GLOBAL_REVIEW => continuing::prepare_with_caller::<Native<false, true>>(
                conn,
                clock,
                &self.origin,
                &caller,
                self.clone(),
            )
            .await
            .map(|frame| NativeFrame::GlobalPreflight(Box::new(frame))),
            GLOBAL_CONFIRM => continuing::prepare_with_caller::<Native<true, true>>(
                conn,
                clock,
                &self.origin,
                &caller,
                self.clone(),
            )
            .await
            .map(|frame| NativeFrame::GlobalCreate(Box::new(frame))),
            _ => Err(Error::Refused),
        }
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
                && match self.node {
                    Some(node) => envs::environments_under(conn, node)
                        .await?
                        .contains(&self.environment()),
                    None => true,
                },
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
        check
            .frame
            .into_first_effect()
            .check(clock)
            .map_err(engine)?;
        command.authority = Some(CommandAuthority::update_planning_probe(evidence));
        Ok(Some((command, input)))
    }
}

// Re-admit actual native facts after intersect_restrictions deliberately discards admission.
// CapturedOperation keeps only provenance; every effect still obtains its full current frame.
async fn admit_minimum<const CREATE_OPERATION: bool, const GLOBAL: bool>(
    conn: &mut Conn,
    clock: &dyn Clock,
    caller: &Caller,
    input: &Input,
) -> Result<Caller, EngineError> {
    let (value, facts) =
        <Native<CREATE_OPERATION, GLOBAL> as Source>::prepare(conn, caller, input, clock.time())
            .await
            .map_err(engine)?;
    let declaration = <Native<CREATE_OPERATION, GLOBAL> as Source>::declaration(input, &value);
    let plan = crate::operation_scope::sources::bind_required(
        <Native<CREATE_OPERATION, GLOBAL> as Source>::ROUTE,
        declaration,
        &facts,
    )
    .map_err(|error| engine(Error::Binding(error)))?;
    plan.admit(caller, false)
        .map_err(|error| engine(Error::Binding(error)))
}

enum NativeFrame {
    Preflight(Box<continuing::Frame<Native<false>>>),
    Create(Box<continuing::Frame<Native<true>>>),
    GlobalPreflight(Box<continuing::Frame<Native<false, true>>>),
    GlobalCreate(Box<continuing::Frame<Native<true, true>>>),
}
impl NativeFrame {
    fn principal(&self) -> &Principal {
        match self {
            Self::Preflight(frame) => frame.principal(),
            Self::Create(frame) => frame.principal(),
            Self::GlobalPreflight(frame) => frame.principal(),
            Self::GlobalCreate(frame) => frame.principal(),
        }
    }
    fn value(&self) -> &Value {
        match self {
            Self::Preflight(frame) => frame.value(),
            Self::Create(frame) => frame.value(),
            Self::GlobalPreflight(frame) => frame.value(),
            Self::GlobalCreate(frame) => frame.value(),
        }
    }
    fn check_deadline(&self, clock: &dyn Clock) -> Result<(), Error> {
        match self {
            Self::Preflight(frame) => frame.check_deadline(clock),
            Self::Create(frame) => frame.check_deadline(clock),
            Self::GlobalPreflight(frame) => frame.check_deadline(clock),
            Self::GlobalCreate(frame) => frame.check_deadline(clock),
        }
    }
    fn into_first_effect(self) -> continuing::FirstEffect {
        match self {
            Self::Preflight(frame) => (*frame).into_parts().first_effect,
            Self::Create(frame) => (*frame).into_parts().first_effect,
            Self::GlobalPreflight(frame) => (*frame).into_parts().first_effect,
            Self::GlobalCreate(frame) => (*frame).into_parts().first_effect,
        }
    }
}

/// Current native candidate and its real raw-credential lifetime, on the caller's transaction.
pub struct Checkpoint {
    /// The native source frame; its first-effect cut is consumed by the native observation supplier.
    frame: NativeFrame,
    /// The current/original minimum of this actual raw credential.
    pub until: Option<Time>,
    input: Input,
}
impl Checkpoint {
    /// Consume the actual native first-effect cut while retaining its accepted minimum.
    pub fn into_accepted(self, clock: &dyn Clock) -> Result<Input, EngineError> {
        let input = self.accepted_input()?;
        self.check(clock)?;
        self.frame
            .into_first_effect()
            .check(clock)
            .map_err(engine)?;
        Ok(input)
    }
    /// The actual current original/current-intersected native actor.
    pub fn principal(&self) -> &Principal {
        self.frame.principal()
    }
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

/// Fresh source result, not a serialized or reusable permission.
pub struct Current {
    /// Same source retaining the accepted minimum.
    pub input: Input,
    /// Current native admitted actor.
    pub caller: Caller,
    /// Earliest real raw expiry.
    pub until: Option<Time>,
}
/// The same native ordinary gates and source facts on HTTP and command dispatch.
pub struct Native<const CREATE_OPERATION: bool = false, const GLOBAL: bool = false>;
/// Current factual native operands, with no IO or formatted reply.
pub struct Value {
    association: Association,
    template: Option<(Id, Id)>,
    /// The original node still selects this Environment through the native membership query.
    pub under: bool,
    /// Current native unit/registration agrees with the original observation.
    pub matches: bool,
    home: Id,
    machine: Option<Id>,
    runtime: Option<Id>,
}
impl<const CREATE_OPERATION: bool, const GLOBAL: bool> Source for Native<CREATE_OPERATION, GLOBAL> {
    type Role = Role;
    type Input = Input;
    type Value = Value;
    const ROUTE: Route = match (CREATE_OPERATION, GLOBAL) {
        (false, false) => NodeEnvironmentUpdatePreflight::ROUTE,
        (true, false) => NodeEnvironmentUpdateCreate::ROUTE,
        (false, true) => EnvironmentUpdateBatchPreflight::ROUTE,
        (true, true) => EnvironmentUpdateBatchCreate::ROUTE,
    };
    const DECLARATION: &'static Declaration<Role> = match (CREATE_OPERATION, GLOBAL) {
        (false, false) => &NODE_PREFLIGHT,
        (true, false) => &NODE_CREATE,
        (false, true) => &GLOBAL_PREFLIGHT,
        (true, true) => &GLOBAL_CREATE,
    };
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Input,
        now: Time,
    ) -> BoxFuture<'a, Result<(Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            input.validate().map_err(Error::Native)?;
            if input.origin.operation() != Self::DECLARATION.operation {
                return Err(Error::Refused);
            }
            if CREATE_OPERATION {
                match (&input.review, input.execution_template) {
                    (Some(review), None) => {
                        let row = orch_db::queries::jobs::forks::preflight(conn, review.id)
                            .await?
                            .ok_or(Error::Refused)?;
                        if row.kind != "update_batch"
                            || row.node_id != input.node
                            || row.created_by != input.origin.user()
                            || Time::new(row.expires_at) != review.expires_at
                            || crate::jobs::command_identity::hash(&row.plan)? != review.hash
                        {
                            return Err(Error::Refused);
                        }
                        if row.expires_at <= now.get() {
                            return Err(Error::Native(EngineError::BeforeEffect(Box::new(
                                ReviewExpired,
                            ))));
                        }
                    }
                    (None, Some(_)) => {}
                    _ => return Err(Error::Refused),
                }
            } else if input.review.is_some() || input.execution_template.is_some() {
                return Err(Error::Refused);
            }
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
            let under = match input.node {
                Some(node) => envs::environments_under(conn, node)
                    .await?
                    .contains(&state.id),
                None => true,
            };
            let runtime = envs::live_runtime(conn, state.id)
                .await?
                .map(|runtime| runtime.id);
            let template = match input.execution_template {
                Some(template) => {
                    match envs::runtime(conn, input.association.runtime.ok_or(Error::Refused)?)
                        .await?
                    {
                        Some(old) => Some((template, old.template_id)),
                        None => None,
                    }
                }
                None => updates::environment_update_template(conn, state.id).await?,
            };
            let mut facts = vec![Facts::collection(
                Role::Environments,
                vec![(KeyTargetKind::Object(ObjectKind::Environment), vec![target])],
            )];
            if CREATE_OPERATION {
                let selected = match template.filter(|(template, old)| template != old) {
                    Some((id, _)) => vec![
                        targets::resource_identity(
                            conn,
                            ObjectRef {
                                kind: ObjectKind::Template,
                                id,
                            },
                        )
                        .await
                        .map_err(|error| Error::Native(EngineError::unreadable(error)))?,
                    ],
                    None => Vec::new(),
                };
                facts.push(Facts::collection(
                    Role::Templates,
                    if selected.is_empty() {
                        Vec::new()
                    } else {
                        vec![(KeyTargetKind::Object(ObjectKind::Template), selected)]
                    },
                ));
            }
            Ok((
                Value {
                    home: state.home_node_id,
                    machine: state.machine_id,
                    runtime,
                    association,
                    template,
                    under,
                    matches,
                },
                facts,
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a Input,
        value: &'a Value,
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
            if let Some(node) = input.node {
                let graph = orch_authz::engine::Graph::load(conn, &[node]).await?;
                let row = graph.get(node).ok_or(Error::Refused)?;
                let permission = NodePermission::View.on(row.kind).ok_or(Error::Refused)?;
                if !orch_authz::nodes::check(
                    conn,
                    &principal,
                    node,
                    permission,
                    Act::Use,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            if !updates::may_manage(
                conn,
                &principal,
                updates::Target::Environment(input.environment()),
            )
            .await?
            {
                return Err(Error::Refused);
            }
            if CREATE_OPERATION && input.execution_template.is_some() {
                let Some((selected, old)) = value.template else {
                    return Err(Error::Refused);
                };
                if !updates::template_usable(conn, &principal, selected, old).await? {
                    return Err(Error::Refused);
                }
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
            if let Some(review) = &input.review {
                sqlx::query("SELECT id FROM preflights WHERE id=$1 FOR SHARE")
                    .bind(review.id)
                    .execute(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?;
            }
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
                && opened.template == fresh.template
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
                if error.is::<Ended>() || error.is::<Denied>() || error.is::<ReviewExpired>() =>
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
