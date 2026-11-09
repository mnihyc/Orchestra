//! One actual directory creator, its current write door and immutable native command.

use super::*;
use orch_api_contract::workspace::EnvironmentFileMkdir;
use orch_link_contract::commands::{CommandKind, FsMkdir};

/// Native validated path and physical workspace, without captured permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMkdirInput {
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    thread: Option<Id>,
    path: String,
}
impl FileMkdirInput {
    /// Capture the existing native door after its path validation.
    pub fn new(env: &rows::EnvironmentRow, machine: Id, thread: Option<Id>, path: String) -> Self {
        Self {
            environment: env.id,
            generation: env.generation,
            machine,
            root: env.workspace_root.clone(),
            thread,
            path,
        }
    }
    /// The actual workspace Environment.
    pub fn environment(&self) -> Id {
        self.environment
    }
    /// The original physical recipient.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The selected native Thread door, when supplied.
    pub fn through_thread(&self) -> Option<Id> {
        self.thread
    }
    /// Factual requested path for an accepted acknowledgement, never a write permit.
    pub fn path(&self) -> &str {
        &self.path
    }
    /// The existing native Environment serialization lane.
    pub fn serial(&self) -> SerialKey {
        SerialKey::Environment(self.environment)
    }
    /// The canonical native body, carrying no authority.
    pub fn command(&self) -> FsMkdir {
        FsMkdir {
            environment_id: self.environment,
            path: self.path.clone(),
        }
    }
    /// Reopen only the same physical source; the caller still needs current Read.
    pub async fn current_environment(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<rows::EnvironmentRow>, EngineError> {
        Ok(rows::environment(conn, self.environment)
            .await?
            .filter(|env| {
                env.generation == self.generation
                    && env.machine_id == Some(self.machine)
                    && env.workspace_root == self.root
            }))
    }
}

/// The same current native write gate used by HTTP preparation and queue/send.
pub struct FileMkdirSource;
impl Source for FileMkdirSource {
    type Role = Role;
    type Input = FileMkdirInput;
    type Value = rows::EnvironmentRow;
    const ROUTE: Route = EnvironmentFileMkdir::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &sources::ENVIRONMENT_FILE_MKDIR;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a FileMkdirInput,
        _: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let env = input
                .current_environment(conn)
                .await?
                .ok_or(Error::Refused)?;
            prepare_write(conn, &env, input.machine, input.thread).await?;
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: env.id,
                },
            )
            .await
            .map_err(|error| match error {
                targets::TargetError::Missing => Error::Refused,
                targets::TargetError::Db(error) => error.into(),
                targets::TargetError::Authz(error) => error.into(),
            })?;
            Ok((
                env,
                vec![Facts::one(Role::MkdirEnvironment, ENVIRONMENT, target)],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a FileMkdirInput,
        env: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(require_write(conn, principal, env, input.thread))
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a FileMkdirInput,
        _: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(lock_write(
            conn,
            input.environment,
            input.machine,
            input.thread,
        ))
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a FileMkdirInput,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.id == fresh.id
                && old.generation == fresh.generation
                && old.machine_id == fresh.machine_id
                && old.workspace_root == fresh.workspace_root)
        })
    }
}

/// Server-owned native source inside the existing original HTTP command envelope.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileMkdirCommand {
    origin: CapturedOperation,
    input: FileMkdirInput,
    canonical: Json,
    deadline: Time,
}
impl std::fmt::Debug for FileMkdirCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileMkdirCommand([private])")
    }
}
impl FileMkdirCommand {
    /// The original fully admitted mkdir and its exact native proposal are required.
    pub fn capture(
        caller: &Caller,
        input: FileMkdirInput,
        command: &crate::commands::Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let same = caller.admitted_facets().is_some_and(|facts| {
            facts.iter().any(|fact| {
                fact.operation == sources::ENVIRONMENT_FILE_MKDIR.operation
                    && fact.action == orch_access_contract::key_scope::KeyAction::Write
                    && fact.resource_kind == ENVIRONMENT
                    && fact.target.object.kind == ENVIRONMENT
                    && fact.target.object.id == Some(input.environment)
            })
        });
        if !same
            || origin.operation() != sources::ENVIRONMENT_FILE_MKDIR.operation
            || command.authority.is_some()
            || command.kind != CommandKind::FsMkdir
            || command.machine != input.machine
            || command.key != input.serial()
            || command.body != serde_json::to_value(input.command())?
        {
            return Err(EngineError::unreadable(
                "the directory command changed its native source",
            ));
        }
        let deadline = command
            .deadline
            .ok_or_else(|| EngineError::unreadable("the directory command lost its deadline"))?;
        Ok(Self {
            origin,
            input,
            deadline,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    /// Accepted immutable coordinates only; this accessor cannot restore the creator.
    pub fn receipt_input(&self) -> FileMkdirInput {
        self.input.clone()
    }
    /// The sole command kind this producer can issue.
    pub fn command_kind(&self) -> &'static str {
        "fs.mkdir"
    }
    /// Keep the original write door's native lock order before current admission.
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        lock_write(
            conn,
            self.input.environment,
            self.input.machine,
            self.input.thread,
        )
        .await
        .map_err(|error| match error {
            Error::Native(error) => error,
            error => EngineError::BeforeEffect(Box::new(error)),
        })
    }
    /// Recheck this original source and exact proposal before a new native insert.
    pub async fn current_issue(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != self.command_kind()
            || Time::new(command.deadline_at) > self.deadline
            || clock.time() >= Time::new(command.deadline_at)
            || crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key,
                command.kind,
                command.body,
            )? != self.canonical
        {
            return Ok(None);
        }
        match continuing::prepare::<FileMkdirSource>(conn, clock, &self.origin, self.input.clone())
            .await
        {
            Ok(frame) => {
                frame
                    .check_deadline(clock)
                    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
                Ok(Some(vec![(Some(frame.principal().caller().clone()), None)]))
            }
            Err(Error::Refused) => Ok(None),
            Err(Error::Native(error)) => Err(error),
            Err(error) => Err(EngineError::BeforeEffect(Box::new(error))),
        }
    }
    /// Recheck the same native frame at its actual send boundary.
    pub async fn current_stored(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        self.current_issue(
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

    pub(crate) fn matches_receipt(&self, command: &CommandRow) -> Result<bool, EngineError> {
        Ok(command.kind == self.command_kind()
            && Time::new(command.deadline_at) <= self.deadline
            && crate::jobs::command_identity::canonical(
                command.id,
                command.machine_id,
                command.serial_key(),
                &command.kind,
                &command.body,
            )? == self.canonical)
    }
}
