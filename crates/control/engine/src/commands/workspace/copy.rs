//! The actual workspace copy source, native write ceiling and original command association.

use super::*;
use orch_api_contract::workspace::EnvironmentFileCopy;
use orch_link_contract::commands::FsCopy;

/// Native validated paths and the actual physical workspace; no serialized admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCopyInput {
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    thread: Option<Id>,
    from: String,
    to: String,
}
impl FileCopyInput {
    /// Capture exactly the existing native door and its already validated explicit paths.
    pub fn new(
        env: &rows::EnvironmentRow,
        machine: Id,
        thread: Option<Id>,
        from: String,
        to: String,
    ) -> Self {
        Self {
            environment: env.id,
            generation: env.generation,
            machine,
            root: env.workspace_root.clone(),
            thread,
            from,
            to,
        }
    }
    /// The actual Environment whose files were copied.
    pub fn environment(&self) -> Id {
        self.environment
    }
    /// The supplied destination, for the completed acknowledgement only.
    pub fn destination(&self) -> &str {
        &self.to
    }
    /// The exact native physical recipient.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The native serialized Environment lane.
    pub fn serial(&self) -> SerialKey {
        SerialKey::Environment(self.environment)
    }
    /// The same native copy body, without authority fields.
    pub fn command(&self) -> FsCopy {
        FsCopy {
            environment_id: self.environment,
            from: self.from.clone(),
            to: self.to.clone(),
        }
    }
    /// Optional result disclosure reopens only its actual physical Environment, not completed Write.
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

/// One current native copy factory for HTTP preparation and actual queue/send admission.
pub struct FileCopySource;
impl Source for FileCopySource {
    type Role = Role;
    type Input = FileCopyInput;
    type Value = rows::EnvironmentRow;
    const ROUTE: Route = EnvironmentFileCopy::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &sources::ENVIRONMENT_FILE_COPY;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a FileCopyInput,
        _: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let env = input
                .current_environment(conn)
                .await?
                .ok_or(Error::Refused)?;
            if let Some(thread) = input.thread {
                let place = rows::thread_place(conn, thread)
                    .await?
                    .ok_or(Error::Refused)?;
                if place.environment_id != Some(env.id) || place.side_of.is_some() {
                    return Err(Error::Refused);
                }
            }
            if orch_db::queries::jobs::changes::environment_change_hold(conn, env.id)
                .await?
                .is_some_and(|job| job.kind == orch_db::queries::jobs::kind::WORKSPACE_RESTORE)
            {
                return Err(Error::Refused);
            }
            if !physical(conn, &env, input.machine).await? {
                return Err(Error::Refused);
            }
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
                vec![Facts::one(Role::CopyEnvironment, ENVIRONMENT, target)],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a FileCopyInput,
        env: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !env.remote_write
                || environment_facts::refuses(conn, env.id, "environment.remote_write").await?
            {
                return Err(Error::Refused);
            }
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: env.id,
                },
                Permission::FilesManage,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            if let Some(thread) = input.thread {
                if !orch_authz::nodes::check(
                    conn,
                    principal,
                    thread,
                    Permission::ThreadView,
                    orch_authz::Act::Use,
                    Policy::default(),
                )
                .await?
                .is_allowed()
                {
                    return Err(Error::Refused);
                }
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a FileCopyInput,
        _: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if let Some(thread) = input.thread {
                sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR SHARE")
                    .bind(thread)
                    .execute(&mut *conn)
                    .await
                    .map_err(DbError::from)?;
            }
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(input.environment)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(input.machine)
                .execute(conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a FileCopyInput,
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

/// One genuine accepted copy operation and exact native command in the existing private channel.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCopyCommand {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    source: FileCopyInput,
    canonical: Json,
}
impl std::fmt::Debug for FileCopyCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileCopyCommand([private])")
    }
}
impl FileCopyCommand {
    /// Only the native copy producer's admitted caller and immutable operands can create this packet.
    pub fn capture(
        caller: &Caller,
        session: Option<Id>,
        source: FileCopyInput,
        command: &crate::commands::Outgoing,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let metadata = crate::operation_scope::Prepared::for_selection(
            sources::ENVIRONMENT_FILE_COPY.operation,
        )
        .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        let same = metadata
            .variant()
            .facets
            .iter()
            .filter(|facet| facet.usage == orch_api_contract::operations::FacetUsage::Required)
            .all(|facet| {
                admitted.iter().any(|fact| {
                    fact.operation == sources::ENVIRONMENT_FILE_COPY.operation
                        && fact.action == facet.action
                        && fact.resource_kind == ENVIRONMENT
                        && fact.target.object.kind == ENVIRONMENT
                        && fact.target.object.id == Some(source.environment)
                })
            });
        if !same
            || origin.operation() != sources::ENVIRONMENT_FILE_COPY.operation
            || (session.is_some() && caller.grant().is_some())
            || command.kind.as_str() != "fs.copy"
            || command.machine != source.machine
            || command.key != source.serial()
            || command.body != serde_json::to_value(source.command())?
        {
            return Err(EngineError::unreadable(
                "the file copy does not match its original native source",
            ));
        }
        Ok(Self {
            origin,
            session,
            source,
            canonical: crate::jobs::command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "fs.copy"
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
        let Some(session) = crate::commands::authority::session_state(
            conn,
            &self.origin,
            self.session,
            clock.time(),
        )
        .await?
        else {
            return Ok(None);
        };
        let caller = match continuing::prepare::<FileCopySource>(
            conn,
            clock,
            &self.origin,
            self.source.clone(),
        )
        .await
        .and_then(|frame| checked_caller(frame, clock))
        {
            Ok(caller) => caller,
            Err(Error::Refused) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        if (self.session.is_some() && caller.grant().is_some())
            || (caller.as_admin()
                && self.session.is_some()
                && !session.is_some_and(|(admin, _)| admin))
        {
            return Ok(None);
        }
        Ok(Some(vec![(Some(caller), session.map(|(_, until)| until))]))
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
