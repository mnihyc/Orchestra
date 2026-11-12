//! Original-operation sources of native workspace measurement commands.

use crate::EngineError;
use crate::operation_scope::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::{Decision, ObjectRef},
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    workspace::{ThreadSnapshotCreate, ThreadSnapshotPreflight},
    workspace_sources::{self as sources, Source as Role},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::{
        environment_facts,
        environments::{hosting, workspace as rows},
        link::{CommandRow, NewCommand},
        resources,
    },
};
use orch_link_contract::commands::{FsMeasure, Selection, SerialKey};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

const ENVIRONMENT: KeyTargetKind = KeyTargetKind::Object(ObjectKind::Environment);

pub(crate) mod snapshots;
pub use snapshots::{SnapshotCommand, SnapshotRequest};
mod copy;
pub use copy::{FileCopyCommand, FileCopyInput, FileCopySource};
mod mkdir;
pub use mkdir::{FileMkdirCommand, FileMkdirInput, FileMkdirSource};

/// The existing write door's native associations, shared by copy and mkdir.
async fn prepare_write(
    conn: &mut Conn,
    env: &rows::EnvironmentRow,
    machine: Id,
    thread: Option<Id>,
) -> Result<(), Error> {
    if let Some(thread) = thread {
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
        || !physical(conn, env, machine).await?
    {
        return Err(Error::Refused);
    }
    Ok(())
}

async fn require_write(
    conn: &mut Conn,
    principal: &Principal,
    env: &rows::EnvironmentRow,
    thread: Option<Id>,
) -> Result<(), Error> {
    if !env.remote_write
        || environment_facts::refuses(conn, env.id, "environment.remote_write").await?
        || !orch_authz::resources::check(
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
    if let Some(thread) = thread
        && !orch_authz::nodes::check(
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
    Ok(())
}

async fn lock_write(
    conn: &mut Conn,
    environment: Id,
    machine: Id,
    thread: Option<Id>,
) -> Result<(), Error> {
    if let Some(thread) = thread {
        sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR SHARE")
            .bind(thread)
            .execute(&mut *conn)
            .await
            .map_err(DbError::from)?;
    }
    sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
        .bind(environment)
        .execute(&mut *conn)
        .await
        .map_err(DbError::from)?;
    sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
        .bind(machine)
        .execute(conn)
        .await
        .map_err(DbError::from)?;
    Ok(())
}

pub(crate) async fn physical(
    conn: &mut Conn,
    env: &rows::EnvironmentRow,
    machine: Id,
) -> Result<bool, EngineError> {
    let attached = if environment_facts::of(conn, env.id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        hosting::connected(conn, env.id)
            .await?
            .is_some_and(|directory| directory.attached())
    } else {
        env.status == "running"
    };
    Ok(attached
        && resources::machine(conn, machine)
            .await?
            .is_some_and(|machine| machine.is_active()))
}

/// Actual selected native operands, never serialized admission or a user-supplied actor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMeasureInput {
    thread: Id,
    environment: Id,
    generation: i32,
    machine: Id,
    root: String,
    default_directory: Option<String>,
    selection: Selection,
}
impl SnapshotMeasureInput {
    /// Called by the native selector after its actual environment/path and ordinary gate.
    pub fn new(
        thread: Id,
        env: &rows::EnvironmentRow,
        machine: Id,
        default_directory: Option<String>,
        selection: Selection,
    ) -> Self {
        Self {
            thread,
            environment: env.id,
            generation: env.generation,
            machine,
            root: env.workspace_root.clone(),
            default_directory,
            selection,
        }
    }
    /// Reopen the same physical source, preserving the native current Thread binding.
    pub async fn current(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<rows::EnvironmentRow>, EngineError> {
        let Some(place) = rows::thread_place(conn, self.thread).await? else {
            return Ok(None);
        };
        if place.environment_id != Some(self.environment)
            || self
                .default_directory
                .as_ref()
                .is_some_and(|directory| directory != &place.workspace_dir)
        {
            return Ok(None);
        }
        let Some(env) = rows::environment(conn, self.environment).await? else {
            return Ok(None);
        };
        if env.machine_id != Some(self.machine)
            || env.generation != self.generation
            || env.workspace_root != self.root
        {
            return Ok(None);
        }
        if !physical(conn, &env, self.machine).await? {
            return Ok(None);
        }
        Ok(Some(env))
    }
    /// The exact bounded selection the connector measures, without re-resolving its paths.
    pub fn command(&self) -> FsMeasure {
        FsMeasure {
            environment_id: self.environment,
            selection: self.selection.clone(),
        }
    }
    /// The original physical recipient.
    pub fn machine(&self) -> Id {
        self.machine
    }
    /// The same native Environment queue lane.
    pub fn serial(&self) -> SerialKey {
        SerialKey::Environment(self.environment)
    }
    /// The original selection used by the ordinary measurement renderer.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
}

/// The factual source provider consumed by HTTP and by actual queue/send checkpoints.
pub struct SnapshotSource<M>(std::marker::PhantomData<M>);
/// The measured preview's native mode.
pub struct MeasureMode;
/// A snapshot creator's independent native mode.
pub struct CreateMode;
/// Canonical native source metadata; all modes keep the same actual Files gate.
pub trait SnapshotMode: Send + Sync + 'static {
    /// The native operation's canonical route.
    const ROUTE: Route;
    /// Its factual role declaration beside the contract.
    const ROW: &'static Declaration<Role>;
    /// The original native ordinary gate, independent of scope labels.
    const PERMISSION: Permission;
}
impl SnapshotMode for MeasureMode {
    const ROUTE: Route = ThreadSnapshotPreflight::ROUTE;
    const ROW: &'static Declaration<Role> = &sources::THREAD_SNAPSHOT_PREFLIGHT;
    const PERMISSION: Permission = Permission::FilesManage;
}
impl SnapshotMode for CreateMode {
    const ROUTE: Route = ThreadSnapshotCreate::ROUTE;
    const ROW: &'static Declaration<Role> = &sources::THREAD_SNAPSHOT_CREATE;
    const PERMISSION: Permission = Permission::FilesManage;
}
/// The actual original preflight's current native source.
pub type SnapshotMeasureSource = SnapshotSource<MeasureMode>;
/// The actual original creator's current measured native source.
pub type SnapshotCreateSource = SnapshotSource<CreateMode>;
impl<M: SnapshotMode> Source for SnapshotSource<M> {
    type Role = Role;
    type Input = SnapshotMeasureInput;
    type Value = rows::EnvironmentRow;
    const ROUTE: Route = M::ROUTE;
    const DECLARATION: &'static Declaration<Role> = M::ROW;

    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a Self::Input,
        _: Time,
    ) -> BoxFuture<'a, Result<(Self::Value, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let env = input.current(conn).await?.ok_or(Error::Refused)?;
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
                vec![Facts::one(Role::SnapshotEnvironment, ENVIRONMENT, target)],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a Self::Input,
        value: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            match orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: value.id,
                },
                M::PERMISSION,
                Policy::default(),
            )
            .await?
            {
                Decision::Allowed { .. } => Ok(()),
                Decision::Refused(_) => Err(Error::Refused),
            }
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _: &'a Principal,
        input: &'a Self::Input,
        _: &'a Self::Value,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT node_id FROM threads WHERE node_id=$1 FOR SHARE")
                .bind(input.thread)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
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
        input: &'a Self::Input,
        old: &'a Self::Value,
        fresh: &'a Self::Value,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.id == fresh.id
                && old.generation == fresh.generation
                && old.machine_id == fresh.machine_id
                && old.workspace_root == fresh.workspace_root
                && fresh.id == input.environment)
        })
    }
}

fn checked_caller<F: Source>(
    frame: continuing::Frame<F>,
    clock: &dyn Clock,
) -> Result<Caller, Error> {
    let parts = frame.into_parts();
    parts.first_effect.check(clock)?;
    Ok(parts.principal.caller().clone())
}

/// One exact canonical command plus its genuine original operation/source association.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkspaceCommand {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    source: SnapshotMeasureInput,
    canonical: Json,
}
impl std::fmt::Debug for WorkspaceCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WorkspaceCommand([private])")
    }
}
impl WorkspaceCommand {
    /// Captures only this actual measured preflight; another operation cannot relabel it.
    pub fn capture(
        origin: &CapturedOperation,
        session: Option<Id>,
        source: SnapshotMeasureInput,
        command: &super::Outgoing,
    ) -> Result<Self, EngineError> {
        if !matches!(
            origin.operation(),
            "ThreadSnapshotPreflight" | "ThreadSnapshotCreate"
        ) || command.machine != source.machine
            || command.kind.as_str() != "fs.measure"
            || command.key != source.serial()
            || command.body != serde_json::to_value(source.command())?
        {
            return Err(EngineError::unreadable(
                "the workspace command does not match its native source",
            ));
        }
        Ok(Self {
            origin: origin.clone(),
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
    pub(super) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.kind != "fs.measure"
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
        let Some(session) =
            super::authority::session_state(conn, &self.origin, self.session, clock.time()).await?
        else {
            return Ok(None);
        };
        let checked = match self.origin.operation() {
            "ThreadSnapshotPreflight" => continuing::prepare::<SnapshotMeasureSource>(
                conn,
                clock,
                &self.origin,
                self.source.clone(),
            )
            .await
            .and_then(|frame| checked_caller(frame, clock)),
            "ThreadSnapshotCreate" => continuing::prepare::<SnapshotCreateSource>(
                conn,
                clock,
                &self.origin,
                self.source.clone(),
            )
            .await
            .and_then(|frame| checked_caller(frame, clock)),
            _ => return Ok(None),
        };
        let caller = match checked {
            Ok(caller) => caller,
            Err(Error::Refused) => return Ok(None),
            Err(error) => return Err(EngineError::unreadable(error)),
        };
        if self.session.is_some() && caller.grant().is_some() {
            return Ok(None);
        }
        if caller.as_admin() && self.session.is_some() && !session.is_some_and(|(admin, _)| admin) {
            return Ok(None);
        }
        if !caller.key_deadline_allows(clock) {
            return Ok(None);
        }
        Ok(Some(vec![(
            Some(caller.clone()),
            session.map(|(_, until)| until),
        )]))
    }
    pub(super) async fn current_callers(
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
