//! Exact native temporary-file sources of the shared browser upload operations.

use super::{
    continuing::{self, Error, Source as NativeSource},
    sources::Facts,
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyIdentityKind, KeyObjectRef, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    threads::{UploadSessionComplete, UploadSessionDelete, UploadSessionPut},
    upload_sources::{self, Source as Role},
    workspace::EnvironmentFileContentPut,
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        environments::workspace,
        uploads::targets::{self as rows, Destination, State, Target},
    },
};
use orch_link_contract::commands::{
    CommandKind, FsRelayAbort, FsRelayCommit, FsRelayPrepare, FsRelayReceive, FsRelayVerify,
    SerialKey,
};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum Operation {
    Piece,
    Verify,
    Cleanup,
    Editor,
}

/// Server-owned immutable operands; neither the browser nor a native command can manufacture them.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionSource {
    origin: CapturedOperation,
    operation: Operation,
    session: Id,
    owner: Id,
    destination: Destination,
    command: Id,
    kind: CommandKind,
    expires_at: Time,
}
impl std::fmt::Debug for SessionSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SessionSource([private])")
    }
}

pub struct Prepared {
    pub state: State,
    pub target: Target,
}
pub struct Piece;
pub struct Verify;
pub struct Cleanup;
pub struct Editor;

async fn prepare(
    conn: &mut Conn,
    input: &SessionSource,
    now: Time,
) -> Result<(Prepared, Vec<Facts<Role>>), Error> {
    let state = rows::state(conn, input.session, false)
        .await?
        .ok_or(Error::Refused)?;
    let target = rows::of(conn, input.session).await?.ok_or(Error::Refused)?;
    if state.session.user_id != input.owner
        || (input.operation != Operation::Cleanup
            && (state.cancelled_at.is_some() || state.completed_at.is_some()))
        || !target.destination.relayed
        || target.destination != input.destination
    {
        return Err(Error::Refused);
    }
    if input.operation != Operation::Cleanup
        && state.session.expires_at <= now.get()
        && !(input.kind == CommandKind::FsRelayVerify
            && verification_active(conn, &state, &target, now).await?)
    {
        return Err(Error::Refused);
    }
    let active = match input.kind {
        CommandKind::FsRelayReceive => target.receiving_command,
        CommandKind::FsRelayVerify => target.verifying_command,
        CommandKind::FsRelayAbort => target.aborting_command,
        CommandKind::FsRelayPrepare | CommandKind::FsRelayCommit
            if input.operation == Operation::Editor =>
        {
            target.receiving_command
        }
        _ => return Err(Error::Refused),
    };
    if active != Some(input.command) {
        return Err(Error::Refused);
    }
    if input.operation != Operation::Cleanup {
        let env = workspace::environment(conn, target.destination.environment_id)
            .await?
            .ok_or(Error::Refused)?;
        if env.machine_id != target.destination.machine_id
            || env.generation != target.destination.generation
            || env.workspace_root != target.destination.workspace_root
        {
            return Err(Error::Refused);
        }
        let attached = if env.placement == "connected" {
            orch_db::queries::environments::hosting::connected(conn, env.id)
                .await?
                .is_some_and(|directory| directory.attached())
        } else {
            env.status == "running"
        };
        let machine = match env.machine_id {
            Some(machine) => orch_db::queries::resources::machine(conn, machine).await?,
            None => None,
        };
        if !attached || machine.is_none_or(|machine| !machine.is_active()) {
            return Err(Error::Refused);
        }
    }
    let mut facts = Vec::new();
    if input.operation != Operation::Editor {
        let mut upload = targets::account(conn, input.owner)
            .await
            .map_err(EngineError::unreadable)?;
        upload.object = KeyObjectRef {
            kind: KeyTargetKind::Identity(KeyIdentityKind::Upload),
            id: Some(input.session),
        };
        facts.push(Facts::one(
            Role::Session,
            KeyTargetKind::Identity(KeyIdentityKind::Upload),
            upload,
        ));
    }
    if input.operation != Operation::Cleanup {
        let environment = targets::resource_identity(
            conn,
            ObjectRef {
                kind: ObjectKind::Environment,
                id: input.destination.environment_id,
            },
        )
        .await
        .map_err(EngineError::unreadable)?;
        if input.operation == Operation::Editor {
            facts.push(Facts::one(
                Role::EditorEnvironment,
                KeyTargetKind::Object(ObjectKind::Environment),
                environment,
            ));
        } else {
            facts.push(Facts::collection(
                Role::SessionEnvironment,
                vec![(
                    KeyTargetKind::Object(ObjectKind::Environment),
                    vec![environment],
                )],
            ));
        }
    }
    Ok((Prepared { state, target }, facts))
}

async fn require(
    conn: &mut Conn,
    principal: &Principal,
    input: &SessionSource,
    value: &Prepared,
) -> Result<(), Error> {
    if principal.user() != value.state.session.user_id {
        return Err(Error::Refused);
    }
    if input.operation == Operation::Cleanup {
        return if value.state.cancelled_at.is_some() {
            Ok(())
        } else {
            Err(Error::Refused)
        };
    }
    if !orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: input.destination.environment_id,
        },
        Permission::FilesManage,
        Policy::default(),
    )
    .await?
    .is_allowed()
    {
        return Err(Error::Refused);
    }
    let env = workspace::environment(conn, input.destination.environment_id)
        .await?
        .ok_or(Error::Refused)?;
    if !env.remote_write
        || orch_db::queries::environment_facts::refuses(conn, env.id, "environment.remote_write")
            .await?
    {
        return Err(Error::Refused);
    }
    if let Some(thread) = input.destination.thread_id {
        let place = workspace::thread_place(conn, thread)
            .await?
            .ok_or(Error::Refused)?;
        if place.environment_id != Some(env.id) || place.side_of.is_some() {
            return Err(Error::Refused);
        }
    }
    Ok(())
}

macro_rules! reader {
    ($name:ident, $route:ty, $row:ident) => {
        impl NativeSource for $name {
            type Role = Role;
            type Input = SessionSource;
            type Value = Prepared;
            const ROUTE: Route = <$route>::ROUTE;
            const DECLARATION: &'static Declaration<Role> = &upload_sources::$row;
            fn prepare<'a>(
                conn: &'a mut Conn,
                _: &'a Caller,
                input: &'a SessionSource,
                now: Time,
            ) -> BoxFuture<'a, Result<(Prepared, Vec<Facts<Role>>), Error>> {
                Box::pin(prepare(conn, input, now))
            }
            fn require<'a>(
                conn: &'a mut Conn,
                principal: &'a Principal,
                input: &'a SessionSource,
                value: &'a Prepared,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(require(conn, principal, input, value))
            }
            fn locks<'a>(
                conn: &'a mut Conn,
                _: &'a Principal,
                input: &'a SessionSource,
                _: &'a Prepared,
            ) -> BoxFuture<'a, Result<(), Error>> {
                Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
            }
            fn check_opened<'a>(
                _: &'a mut Conn,
                _: &'a Principal,
                _: &'a SessionSource,
                old: &'a Prepared,
                fresh: &'a Prepared,
            ) -> BoxFuture<'a, Result<bool, Error>> {
                Box::pin(async move {
                    Ok(old.state.session.id == fresh.state.session.id
                        && old.state.session.user_id == fresh.state.session.user_id
                        && old.state.session.size == fresh.state.session.size
                        && old.state.session.created_at == fresh.state.session.created_at
                        && old.target.destination == fresh.target.destination)
                })
            }
        }
    };
}
reader!(Piece, UploadSessionPut, SESSION_PUT);
reader!(Verify, UploadSessionComplete, SESSION_COMPLETE);
reader!(Cleanup, UploadSessionDelete, SESSION_DELETE);
reader!(Editor, EnvironmentFileContentPut, EDITOR_SAVE);

impl SessionSource {
    pub fn capture(
        caller: &Caller,
        state: &State,
        target: &Target,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        let operation = match caller.admitted_operation() {
            Some("UploadSessionPut") => Operation::Piece,
            Some("UploadSessionComplete") => Operation::Verify,
            Some("UploadSessionDelete") => Operation::Cleanup,
            Some("EnvironmentFileContentPut") => Operation::Editor,
            _ => {
                return Err(EngineError::unreadable(
                    "the native upload changed its original operation",
                ));
            }
        };
        let to = &target.destination;
        if !to.relayed
            || (operation != Operation::Cleanup
                && (state.cancelled_at.is_some() || state.completed_at.is_some()))
            || caller.user() != state.session.user_id
            || command.machine
                != to
                    .machine_id
                    .ok_or_else(|| EngineError::unreadable("the upload has no native machine"))?
            || command.key
                != if command.kind == CommandKind::FsRelayCommit {
                    SerialKey::Environment(to.environment_id)
                } else {
                    SerialKey::None
                }
        {
            return Err(EngineError::unreadable(
                "the upload changed its physical source",
            ));
        }
        match command.kind {
            CommandKind::FsRelayReceive if operation != Operation::Cleanup => {
                let body: FsRelayReceive = serde_json::from_value(command.body.clone())?;
                if body.environment_id != to.environment_id
                    || body.path != to.path
                    || body.session_id != state.session.id
                    || body.size != state.session.size
                    || body.end < 0
                    || body.end > body.size
                    || body.relay_id.is_nil()
                    || body.expires_at.get() > state.session.expires_at
                    || body.idle_secs == 0
                    || body.retry_ms == 0
                    || i64::try_from(body.retention_secs).ok() != Some(state.session.idle_secs)
                    || target.receiving_command != Some(command.id)
                    || (operation == Operation::Verify && body.end != 0)
                {
                    return Err(EngineError::unreadable(
                        "the native receiver changed its upload operands",
                    ));
                }
            }
            CommandKind::FsRelayVerify
                if matches!(operation, Operation::Verify | Operation::Editor) =>
            {
                let body: FsRelayVerify = serde_json::from_value(command.body.clone())?;
                if body.environment_id != to.environment_id
                    || body.receive_path != to.path
                    || body.session_id != state.session.id
                    || target.verifying_command != Some(command.id)
                    || i64::try_from(body.retention_secs).ok() != Some(state.session.idle_secs)
                {
                    return Err(EngineError::unreadable(
                        "the verifier changed its native upload",
                    ));
                }
            }
            CommandKind::FsRelayPrepare if operation == Operation::Editor => {
                let body: FsRelayPrepare = serde_json::from_value(command.body.clone())?;
                if body.environment_id != to.environment_id
                    || body.receive_path != to.path
                    || body.path != to.path
                    || body.session_id != state.session.id
                    || target.receiving_command != Some(command.id)
                {
                    return Err(EngineError::unreadable(
                        "the editor preparation changed its native target",
                    ));
                }
            }
            CommandKind::FsRelayCommit if operation == Operation::Editor => {
                let body: FsRelayCommit = serde_json::from_value(command.body.clone())?;
                if body.environment_id != to.environment_id
                    || body.receive_path != to.path
                    || body.path != to.path
                    || body.session_id != state.session.id
                    || body.size != state.session.size
                    || target.verified()? != Some(body.hash)
                    || target.receiving_command != Some(command.id)
                {
                    return Err(EngineError::unreadable(
                        "the editor publication changed its verified native source",
                    ));
                }
            }
            CommandKind::FsRelayAbort if operation == Operation::Editor => {
                let body: FsRelayAbort = serde_json::from_value(command.body.clone())?;
                if body.environment_id != to.environment_id
                    || body.receive_path != to.path
                    || body.session_id != state.session.id
                    || target.aborting_command != Some(command.id)
                {
                    return Err(EngineError::unreadable(
                        "the editor cleanup changed its native source",
                    ));
                }
            }
            CommandKind::FsRelayAbort if operation == Operation::Cleanup => {
                let body: FsRelayAbort = serde_json::from_value(command.body.clone())?;
                if state.cancelled_at.is_none()
                    || target.aborting_command != Some(command.id)
                    || body.environment_id != to.environment_id
                    || body.receive_path != to.path
                    || body.session_id != state.session.id
                {
                    return Err(EngineError::unreadable(
                        "the cleanup changed its accepted native upload",
                    ));
                }
            }
            _ => {
                return Err(EngineError::unreadable(
                    "the upload cannot issue this command",
                ));
            }
        }
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            operation,
            session: state.session.id,
            owner: state.session.user_id,
            destination: to.clone(),
            command: command.id,
            kind: command.kind,
            expires_at: Time::new(state.session.expires_at),
        })
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = rows::state(conn, self.session, true).await?;
        let _ = rows::lock(conn, self.session).await?;
        if self.operation == Operation::Cleanup {
            return Ok(());
        }
        sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
            .bind(self.destination.environment_id)
            .fetch_optional(conn)
            .await
            .map_err(DbError::from)?;
        Ok(())
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let result = match self.operation {
            Operation::Piece => {
                continuing::prepare::<Piece>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| {
                        (
                            frame.principal().caller().clone(),
                            frame.check_deadline(clock),
                        )
                    })
            }
            Operation::Verify => {
                continuing::prepare::<Verify>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| {
                        (
                            frame.principal().caller().clone(),
                            frame.check_deadline(clock),
                        )
                    })
            }
            Operation::Cleanup => {
                continuing::prepare::<Cleanup>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| {
                        (
                            frame.principal().caller().clone(),
                            frame.check_deadline(clock),
                        )
                    })
            }
            Operation::Editor => {
                continuing::prepare::<Editor>(conn, clock, &self.origin, self.clone())
                    .await
                    .map(|frame| {
                        (
                            frame.principal().caller().clone(),
                            frame.check_deadline(clock),
                        )
                    })
            }
        };
        match result {
            Ok((caller, checked)) => {
                checked.map_err(|e| EngineError::BeforeEffect(Box::new(e)))?;
                Ok(Some(vec![(Some(caller), None)]))
            }
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(error)) => Err(error),
        }
    }
}

/// An actual unfinished verification keeps its own captured command lifetime, not an idle cutoff.
pub async fn verification_active(
    conn: &mut Conn,
    state: &State,
    target: &Target,
    now: Time,
) -> Result<bool, DbError> {
    let Some(id) = target.verifying_command else {
        return Ok(false);
    };
    let Some(command) = orch_db::queries::link::command(conn, id).await? else {
        return Ok(false);
    };
    Ok(target.destination.relayed
        && command.kind == CommandKind::FsRelayVerify.as_str()
        && Some(command.machine_id) == target.destination.machine_id
        && command.created_at <= state.session.expires_at
        && !command.is_over()
        && command.deadline_at > now.get())
}

/// The real native acknowledgement is a durable fact even when its initiating HTTP reader left.
/// Current completion/write readers still need their own session and environment authority.
pub async fn retain_verified(
    conn: &mut Conn,
    machine: Id,
    id: Id,
    result: &orch_types::Json,
    at: chrono::DateTime<chrono::Utc>,
) -> Result<(), DbError> {
    let Some(command) = orch_db::queries::link::command(conn, id)
        .await?
        .filter(|command| {
            command.machine_id == machine && command.kind == CommandKind::FsRelayVerify.as_str()
        })
    else {
        return Ok(());
    };
    let Ok(body) = serde_json::from_value::<FsRelayVerify>(command.body) else {
        return Ok(());
    };
    let Ok(verified) =
        serde_json::from_value::<orch_link_contract::commands::FsRelayVerified>(result.clone())
    else {
        return Ok(());
    };
    let Some(state) = rows::state(conn, body.session_id, true).await? else {
        return Ok(());
    };
    let Some(target) = rows::lock(conn, body.session_id).await? else {
        return Ok(());
    };
    if state.cancelled_at.is_some()
        || state.completed_at.is_some()
        || !target.destination.relayed
        || target.destination.machine_id != Some(machine)
        || target.destination.environment_id != body.environment_id
        || target.destination.path != body.receive_path
        || target.verifying_command != Some(id)
        || verified.file.size != state.session.size
    {
        return Ok(());
    }
    if rows::verified(conn, body.session_id, id, &verified.file.hash).await? {
        rows::progress(
            conn,
            body.session_id,
            verified.file.size,
            at,
            verified.expires_at.get(),
        )
        .await?;
    }
    Ok(())
}
