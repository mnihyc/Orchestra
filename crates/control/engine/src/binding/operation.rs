//! The actual empty-history binding source through descriptor startup and its first write.
use super::*;
use crate::{
    harnesses::preparation as startup,
    operation_scope::{continuing, request_origin, sources::Facts, targets},
};
use futures::future::BoxFuture;
use orch_api_contract::{
    binding::ThreadBindingSet,
    binding::ThreadBindingSetBody,
    binding_sources::{self as rows, Source as Role},
    operation_sources::Declaration,
    routes::{Endpoint, Route},
};
use orch_authz::CapturedOperation;
use orch_types::{Clock, Hash};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    thread: Id,
    topic: Id,
    view: Id,
    segment: Option<Id>,
    revision: i64,
    agent: Option<Id>,
    provider: Option<Id>,
    environment: Id,
    generation: i32,
    machine: Option<Id>,
    root: String,
    directory: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    snapshot: Snapshot,
    body: ThreadBindingSetBody,
}
impl Input {
    pub async fn select(
        conn: &mut orch_db::Conn,
        thread: Id,
        body: ThreadBindingSetBody,
    ) -> Result<Self, EngineError> {
        let row = threads::thread(conn, thread)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(refused)?;
        let environment = environments::environment(conn, row.environment_id.ok_or_else(refused)?)
            .await?
            .filter(|row| row.deleted_at.is_none())
            .ok_or_else(refused)?;
        Ok(Self {
            snapshot: Snapshot {
                thread,
                topic: row.topic_id,
                view: row.active_continuation_id,
                segment: row.current_segment_id,
                revision: row.execution_revision,
                agent: row.agent_id,
                provider: row.provider_id,
                environment: environment.id,
                generation: environment.generation,
                machine: environment.machine_id,
                root: environment.workspace_root,
                directory: row.workspace_dir,
            },
            body,
        })
    }
    pub fn thread(&self) -> Id {
        self.snapshot.thread
    }
    pub fn environment(&self) -> Id {
        self.snapshot.environment
    }
    pub fn body(&self) -> &ThreadBindingSetBody {
        &self.body
    }
    pub fn request(&self) -> Result<Rebind, EngineError> {
        Ok(Rebind {
            agent: self.body.agent_id,
            provider: self.body.provider_id,
            arrangement: crate::sessions::convert(&self.body.arrangement)?,
            model: self.body.model.clone(),
            effort: self.body.effort.clone(),
        })
    }
    pub async fn facts(&self, conn: &mut orch_db::Conn) -> Result<Vec<Facts<Role>>, EngineError> {
        if Self::select(conn, self.thread(), self.body.clone()).await? != *self {
            return Err(refused());
        }
        let target = targets::node(conn, self.thread())
            .await
            .map_err(|error| match error {
                targets::TargetError::Missing => refused(),
                targets::TargetError::Db(error) => error.into(),
                targets::TargetError::Authz(error) => error.into(),
            })?;
        if target.object.kind
            != orch_access_contract::key_scope::KeyTargetKind::Object(ObjectKind::Thread)
        {
            return Err(refused());
        }
        Ok(vec![Facts::one(Role::Thread, target.object.kind, target)])
    }
    async fn target(
        &self,
        conn: &mut orch_db::Conn,
        target: startup::Target,
    ) -> Result<(), EngineError> {
        match target {
            startup::Target::Descriptor {
                provider,
                agent,
                environment,
            } if provider == self.body.provider_id
                && agent == self.body.agent_id
                && environment == Some(self.environment()) =>
            {
                Ok(())
            }
            startup::Target::Harness(key)
                if key.provider == self.body.provider_id && key.agent == self.body.agent_id =>
            {
                let provider = resources::provider(conn, key.provider)
                    .await?
                    .ok_or_else(refused)?;
                if leases::machine_of(&provider) == Some(key.machine)
                    || self.snapshot.machine == Some(key.machine)
                {
                    Ok(())
                } else {
                    Err(refused())
                }
            }
            _ => Err(refused()),
        }
    }
}
pub struct Work;
impl continuing::Source for Work {
    type Role = Role;
    type Input = Input;
    type Value = Input;
    const ROUTE: Route = ThreadBindingSet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &rows::REBIND.phase;
    fn prepare<'a>(
        conn: &'a mut orch_db::Conn,
        _caller: &'a Caller,
        input: &'a Input,
        _now: Time,
    ) -> BoxFuture<'a, Result<(Input, Vec<Facts<Role>>), continuing::Error>> {
        Box::pin(async move {
            let facts = input.facts(conn).await?;
            Ok((input.clone(), facts))
        })
    }
    fn require<'a>(
        _conn: &'a mut orch_db::Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        _value: &'a Input,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async { Ok(()) })
    }
    fn locks<'a>(
        conn: &'a mut orch_db::Conn,
        _principal: &'a Principal,
        input: &'a Input,
        _value: &'a Input,
    ) -> BoxFuture<'a, Result<(), continuing::Error>> {
        Box::pin(async move {
            threads::lock(conn, input.thread())
                .await?
                .ok_or(continuing::Error::Refused)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _conn: &'a mut orch_db::Conn,
        _principal: &'a Principal,
        _input: &'a Input,
        old: &'a Input,
        new: &'a Input,
    ) -> BoxFuture<'a, Result<bool, continuing::Error>> {
        Box::pin(async move { Ok(old == new) })
    }
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Origin {
    origin: CapturedOperation,
    input: Input,
    credential: request_origin::Credential,
    until: Option<Time>,
}
impl std::fmt::Debug for Origin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BindingOrigin([private])")
    }
}
fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(continuing::Error::Refused))
}
fn scope_error(error: continuing::Error) -> EngineError {
    match error {
        continuing::Error::Native(error) => error,
        _ => refused(),
    }
}
impl Origin {
    pub fn capture(
        caller: &Caller,
        input: Input,
        credential: request_origin::Credential,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some("ThreadBindingSet") || !credential.matches(caller) {
            return Err(refused());
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            input,
            credential,
            until,
        })
    }
    pub fn input(&self) -> &Input {
        &self.input
    }
    pub fn captured(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn deadline(&self, clock: &dyn Clock, caller: &Caller) -> Result<(), EngineError> {
        if caller.key_deadline_allows(clock) && self.until.is_none_or(|until| clock.time() < until)
        {
            Ok(())
        } else {
            Err(refused())
        }
    }
    async fn frame(
        &self,
        conn: &mut orch_db::Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<(continuing::preparation::Frame<Work>, Option<Time>), EngineError> {
        let (caller, until) = request_origin::current(
            conn,
            clock,
            &self.origin,
            &self.credential,
            self.until,
            "ThreadBindingSet",
        )
        .await
        .map_err(scope_error)?;
        let frame = continuing::preparation::prepare_with_caller::<Work>(
            conn,
            clock,
            &self.origin,
            &caller,
            self.input.clone(),
        )
        .await
        .map_err(scope_error)?;
        match rows::REBIND.destination {
            rows::NativeDestination::Binding => {
                match select_rebind_with_access(
                    conn,
                    access,
                    frame.principal(),
                    self.input.thread(),
                    &self.input.request()?,
                    false,
                )
                .await
                {
                    Ok(_) | Err(RebindError::AgentNotReported) => {}
                    Err(RebindError::Internal(error)) => return Err(error),
                    Err(_) => return Err(refused()),
                }
            }
        }
        frame.check_deadline(clock).map_err(scope_error)?;
        self.deadline(clock, frame.principal().caller())?;
        Ok((frame, until))
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Self, EngineError> {
        if origin.user() != self.origin.user() || origin.operation() != self.origin.operation() {
            return Err(refused());
        }
        Ok(Self {
            origin,
            until,
            ..self.clone()
        })
    }
}
#[derive(Clone)]
pub struct Preparation {
    original: Origin,
    minimum: Arc<Mutex<Origin>>,
    access: Arc<dyn crate::access::ResourceAccess>,
    clock: Arc<dyn Clock>,
}
impl Engine {
    pub fn binding_preparation(&self, origin: Origin) -> Preparation {
        Preparation {
            original: origin.clone(),
            minimum: Arc::new(Mutex::new(origin)),
            access: self.inner.access.clone(),
            clock: self.inner.clock.clone(),
        }
    }
}
impl Preparation {
    pub fn startup(&self) -> startup::Preparation {
        startup::Preparation::new(self.clone())
    }
    pub fn retained(&self) -> Result<Origin, EngineError> {
        self.minimum
            .lock()
            .map(|origin| origin.clone())
            .map_err(|_| refused())
    }
    fn remember(&self, origin: Origin) -> Result<(), EngineError> {
        let mut current = self.minimum.lock().map_err(|_| refused())?;
        let until = current.until.into_iter().chain(origin.until).min();
        *current = Origin { until, ..origin };
        Ok(())
    }
}
impl startup::Source for Preparation {
    fn origin(&self) -> &CapturedOperation {
        &self.original.origin
    }
    fn until(&self) -> Option<Time> {
        self.original.until
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut orch_db::Conn,
        clock: &'a dyn Clock,
        target: startup::Target,
    ) -> BoxFuture<'a, Result<startup::Checkpoint, EngineError>> {
        Box::pin(async move {
            let origin = self.retained()?;
            let (frame, until) = origin.frame(conn, clock, self.access.as_ref()).await?;
            origin.input.target(conn, target).await?;
            let caller = frame.principal().caller().clone();
            let until = origin
                .until
                .into_iter()
                .chain(until)
                .chain(caller.grant().and_then(|grant| grant.expires_at()))
                .min();
            let accepted = origin.accepted(
                CapturedOperation::capture(&caller).map_err(EngineError::unreadable)?,
                until,
            )?;
            self.remember(accepted.clone())?;
            let actual_clock = self.clock.clone();
            Ok(startup::Checkpoint::from_preparation(
                frame,
                until,
                Box::new(move || accepted.deadline(actual_clock.as_ref(), &caller)),
            ))
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn startup::Source>, EngineError> {
        let original = self.original.accepted(origin, until)?;
        Ok(Arc::new(Self {
            original,
            ..self.clone()
        }))
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut orch_db::Conn,
        clock: &'a dyn Clock,
        target: startup::Target,
        command: &'a crate::commands::Outgoing,
    ) -> BoxFuture<'a, Result<crate::commands::CommandAuthority, EngineError>> {
        Box::pin(async move {
            let origin = self.retained()?;
            origin.input.target(conn, target).await?;
            Ok(crate::commands::CommandAuthority::RebindPreparation(
                Box::new(Command::capture(origin, target, command, clock)?),
            ))
        })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    origin: Origin,
    id: Id,
    machine: Id,
    kind: orch_link_contract::commands::CommandKind,
    #[serde(with = "crate::commands::RetainedKey")]
    serial: orch_link_contract::commands::SerialKey,
    body: Hash,
    deadline: Option<Time>,
    harness: harnesses::HarnessKey,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BindingCommand([private])")
    }
}
impl Command {
    fn capture(
        origin: Origin,
        target: startup::Target,
        command: &crate::commands::Outgoing,
        _clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        use orch_link_contract::commands::{CommandKind, SerialKey};
        let startup::Target::Harness(harness) = target else {
            return Err(refused());
        };
        let serial = if command.kind == CommandKind::AgentStart {
            SerialKey::Harness {
                provider_id: harness.provider,
                agent_id: harness.agent,
            }
        } else {
            SerialKey::Runtime(harness.provider)
        };
        if command.authority.is_some()
            || command.machine != harness.machine
            || command.key != serial
        {
            return Err(refused());
        }
        Ok(Self {
            origin,
            id: command.id,
            machine: command.machine,
            kind: command.kind,
            serial,
            body: crate::jobs::command_identity::hash(&command.body)?,
            deadline: command.deadline,
            harness,
        })
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub(crate) async fn lock_source(&self, conn: &mut orch_db::Conn) -> Result<(), EngineError> {
        orch_db::queries::transaction_guard::lock_native_actor(conn, self.origin.origin.user())
            .await?;
        threads::lock(conn, self.origin.input.thread()).await?;
        Ok(())
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut orch_db::Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.id
            || command.machine_id != self.machine
            || command.kind != self.kind.as_str()
            || command.serial_key != self.serial.wire().as_deref()
            || crate::jobs::command_identity::hash(command.body)? != self.body
            || self
                .deadline
                .is_some_and(|at| at.get() != command.deadline_at)
        {
            return Ok(None);
        }
        if let Some(row) = orch_db::queries::link::command(conn, self.id).await? {
            if row.deadline_at != command.deadline_at || row.body != *command.body {
                return Ok(None);
            }
        } else if command.deadline_at
            > clock.now() + chrono::Duration::seconds(i64::from(self.kind.deadline_secs()))
        {
            return Ok(None);
        }
        if command.deadline_at <= clock.now() {
            return Ok(None);
        }
        let (frame, until) = match self.origin.frame(conn, clock, access).await {
            Ok(value) => value,
            Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        match self
            .origin
            .input
            .target(conn, startup::Target::Harness(self.harness))
            .await
        {
            Ok(()) => {}
            Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        }
        let parts = frame.into_parts();
        parts.first_effect.check(clock).map_err(scope_error)?;
        self.origin.deadline(clock, parts.principal.caller())?;
        Ok(Some(vec![(
            Some(parts.principal.caller().clone()),
            Some(until.map_or(Time::new(command.deadline_at), |until| {
                until.min(Time::new(command.deadline_at))
            })),
        )]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut orch_db::Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let serial = command.serial_key().map(str::to_owned);
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: serial.as_deref(),
                deadline_at: command.deadline_at,
            },
            clock,
            access,
        )
        .await
    }
}
