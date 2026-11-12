//! Stored native Snapshot/Job provenance and its actual queued command source.

use super::*;
use crate::jobs::snapshot::Asked;
use orch_api_contract::operations::FacetUsage;
use orch_db::queries::{jobs, storage};
use orch_link_contract::commands::FsSnapshot;

pub(crate) const ORIGIN: &str = "snapshot_origin";

/// A native accepted source recipe, not target admission or an execution actor rebuilt from an id.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotRequest {
    origin: CapturedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    job: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    snapshot: Option<Id>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    session: Option<Id>,
    environment: Id,
    generation: i32,
    machine: Option<Id>,
    root: String,
    thread: Option<Id>,
    selection: Selection,
}
impl std::fmt::Debug for SnapshotRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SnapshotRequest([private])")
    }
}
impl SnapshotRequest {
    /// Capture only the actual creator admitted at this Environment by the shared source row.
    pub fn capture(
        caller: &Caller,
        session: Option<Id>,
        env: &rows::EnvironmentRow,
        asked: &Asked,
    ) -> Result<Self, EngineError> {
        let origin = CapturedOperation::capture(caller).map_err(EngineError::unreadable)?;
        let operation = if asked.thread.is_some() {
            "ThreadSnapshotCreate"
        } else {
            "EnvironmentFilesExport"
        };
        let metadata = crate::operation_scope::Prepared::for_selection(operation)
            .map_err(EngineError::unreadable)?;
        let admitted = caller.admitted_facets().unwrap_or(&[]);
        if origin.operation() != operation
            || asked.environment != env.id
            || asked.generation != env.generation
            || asked.created_by != Some(origin.user())
            || (session.is_some() && caller.grant().is_some())
            || !metadata
                .variant()
                .facets
                .iter()
                .filter(|facet| facet.usage == FacetUsage::Required)
                .all(|facet| {
                    admitted.iter().any(|fact| {
                        fact.operation == operation
                            && fact.action == facet.action
                            && fact.resource_kind == ENVIRONMENT
                            && fact.target.object.kind == ENVIRONMENT
                            && fact.target.object.id == Some(env.id)
                    })
                })
        {
            return Err(EngineError::unreadable(
                "the snapshot request is not its actual admitted source",
            ));
        }
        Ok(Self {
            origin,
            job: None,
            snapshot: None,
            session,
            environment: env.id,
            generation: env.generation,
            machine: env.machine_id,
            root: env.workspace_root.clone(),
            thread: asked.thread,
            selection: Selection {
                include: asked.include.clone(),
                exclude: asked.exclude.clone(),
            },
        })
    }
    /// The first native INSERT must consume exactly the prepared source, not rebuilt operands.
    pub fn matches(&self, asked: &Asked) -> bool {
        self.environment == asked.environment
            && self.generation == asked.generation
            && self.thread == asked.thread
            && asked.created_by == Some(self.origin.user())
            && self.selection.include == asked.include
            && self.selection.exclude == asked.exclude
    }
    /// Reopen the original physical source for optional response content, not forward work.
    pub async fn current(
        &self,
        conn: &mut Conn,
    ) -> Result<Option<rows::EnvironmentRow>, EngineError> {
        Ok(rows::environment(conn, self.environment)
            .await?
            .filter(|env| {
                env.generation == self.generation
                    && env.workspace_root == self.root
                    && self
                        .machine
                        .is_none_or(|machine| env.machine_id == Some(machine))
            }))
    }
    /// The original native Environment identity.
    pub fn environment(&self) -> Id {
        self.environment
    }
    /// The actual accepted child identity, for its own native completion/cleanup only.
    pub fn snapshot_for(&self, job: Id) -> Result<Id, EngineError> {
        if self.job != Some(job) {
            return Err(EngineError::unreadable(
                "the snapshot origin changed Job identity",
            ));
        }
        self.snapshot
            .ok_or_else(|| EngineError::unreadable("the accepted snapshot identity is absent"))
    }
    /// Frozen native paths; later source lookup never substitutes another selection.
    pub fn selection(&self) -> &Selection {
        &self.selection
    }
    /// The native physical source retained by the accepted creator, if already assigned.
    pub fn machine(&self) -> Option<Id> {
        self.machine
    }
    /// Load genuine provenance; malformed present metadata never becomes old ordinary work.
    pub fn from_job(row: &jobs::JobRow) -> Result<Option<Self>, EngineError> {
        row.plan
            .get(ORIGIN)
            .map(|value| serde_json::from_value(value.clone()).map_err(EngineError::from))
            .transpose()
    }
}

async fn structural(
    conn: &mut Conn,
    job: &jobs::JobRow,
) -> Result<(SnapshotRequest, storage::SnapshotRow), EngineError> {
    let request: SnapshotRequest =
        serde_json::from_value(job.plan.get(ORIGIN).cloned().unwrap_or(Json::Null))?;
    let snapshot = job
        .plan
        .get("snapshot_id")
        .and_then(Json::as_str)
        .and_then(|id| id.parse::<Id>().ok())
        .ok_or_else(|| EngineError::unreadable("the original snapshot identity is missing"))?;
    let row = storage::snapshot(conn, snapshot)
        .await?
        .ok_or_else(|| EngineError::unreadable("the original snapshot is missing"))?;
    let operation = if request.thread.is_some() {
        "ThreadSnapshotCreate"
    } else {
        "EnvironmentFilesExport"
    };
    if request.job != Some(job.id)
        || request.snapshot != Some(snapshot)
        || job.kind != jobs::kind::SNAPSHOT
        || job.environment_id != Some(request.environment)
        || job.node_id != request.thread
        || job.provider_id.is_some()
        || job.created_by != Some(request.origin.user())
        || request.origin.operation() != operation
        || row.thread_id != request.thread
        || row.environment_id != request.environment
        || row.env_generation != request.generation
        || row.created_by != job.created_by
        || row.created_at != job.created_at
        || job.plan["environment_id"] != serde_json::to_value(request.environment)?
        || job.plan["thread_id"] != serde_json::to_value(request.thread)?
        || job.plan["selection"] != serde_json::to_value(&request.selection)?
        || row.selection != serde_json::to_value(&request.selection)?
    {
        return Err(EngineError::unreadable(
            "the native snapshot no longer matches its original recipe",
        ));
    }
    Ok((request, row))
}

/// Only genuine pre-provenance jobs retain their existing ordinary native path.
pub(crate) fn has_origin(row: &jobs::JobRow) -> bool {
    row.kind == jobs::kind::SNAPSHOT && row.plan.get(ORIGIN).is_some()
}

/// Native command association, after the runner has added its canonical WorkReport.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotCommand {
    job: Id,
    request: SnapshotRequest,
    canonical: Json,
}
impl std::fmt::Debug for SnapshotCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SnapshotCommand([private])")
    }
}
impl SnapshotCommand {
    pub(crate) async fn capture(
        conn: &mut Conn,
        row: &jobs::JobRow,
        expected: &SnapshotRequest,
        command: &crate::commands::Outgoing,
    ) -> Result<Self, EngineError> {
        let (request, _) = structural(conn, row).await?;
        let body: FsSnapshot = serde_json::from_value(command.body.clone())?;
        if request != *expected
            || command.kind.as_str() != "fs.snapshot"
            || command.key != SerialKey::Environment(request.environment)
            || body.environment_id != request.environment
            || body.job_id != row.id
            || body.selection != request.selection
            || !body.work.as_ref().is_some_and(|work| work.job_id == row.id)
            || request
                .machine
                .is_some_and(|machine| machine != command.machine)
        {
            return Err(EngineError::unreadable(
                "the snapshot command does not match its original native recipe",
            ));
        }
        Ok(Self {
            job: row.id,
            request,
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
        if command.kind != "fs.snapshot"
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
            &self.request.origin,
            self.request.session,
            clock.time(),
        )
        .await?
        else {
            return Ok(None);
        };
        let input = JobInput {
            job: self.job,
            request: self.request.clone(),
            command: command.id,
            machine: command.machine_id,
        };
        let checked = match self.request.origin.operation() {
            "ThreadSnapshotCreate" => continuing::prepare::<JobSource<CreateMode>>(
                conn,
                clock,
                &self.request.origin,
                input,
            )
            .await
            .and_then(|frame| checked_caller(frame, clock)),
            "EnvironmentFilesExport" => continuing::prepare::<JobSource<ExportMode>>(
                conn,
                clock,
                &self.request.origin,
                input,
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
        if (self.request.session.is_some() && caller.grant().is_some())
            || (caller.as_admin()
                && self.request.session.is_some()
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

struct ExportMode;
impl SnapshotMode for ExportMode {
    const ROUTE: Route = orch_api_contract::workspace::EnvironmentFilesExport::ROUTE;
    const ROW: &'static Declaration<Role> = &sources::ENVIRONMENT_FILES_EXPORT;
    const PERMISSION: Permission = Permission::EnvironmentManage;
}
struct JobInput {
    job: Id,
    request: SnapshotRequest,
    command: Id,
    machine: Id,
}
struct JobValue {
    row: jobs::JobRow,
    snapshot: storage::SnapshotRow,
    env: rows::EnvironmentRow,
}
struct JobSource<M>(std::marker::PhantomData<M>);
impl<M: SnapshotMode> Source for JobSource<M> {
    type Role = Role;
    type Input = JobInput;
    type Value = JobValue;
    const ROUTE: Route = M::ROUTE;
    const DECLARATION: &'static Declaration<Role> = M::ROW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a JobInput,
        _: Time,
    ) -> BoxFuture<'a, Result<(JobValue, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job).await?.ok_or(Error::Refused)?;
            let (request, snapshot) = structural(conn, &row).await?;
            if request != input.request
                || !row.is_open()
                || row.cancel_requested()
                || snapshot.state != storage::snapshot_state::RUNNING
                || row
                    .plan
                    .pointer("/active_command/id")
                    .and_then(Json::as_str)
                    != Some(input.command.to_string().as_str())
            {
                return Err(Error::Refused);
            }
            let env = rows::environment(conn, request.environment)
                .await?
                .ok_or(Error::Refused)?;
            if env.generation != request.generation
                || env.workspace_root != request.root
                || env.machine_id != Some(input.machine)
                || request
                    .machine
                    .is_some_and(|machine| machine != input.machine)
            {
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
                JobValue { row, snapshot, env },
                vec![Facts::one(Role::SnapshotEnvironment, ENVIRONMENT, target)],
            ))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a JobInput,
        value: &'a JobValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            match orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: value.env.id,
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
        input: &'a JobInput,
        value: &'a JobValue,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            sqlx::query("SELECT id FROM jobs WHERE id=$1 FOR UPDATE")
                .bind(input.job)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM snapshots WHERE id=$1 FOR SHARE")
                .bind(value.snapshot.id)
                .execute(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(value.env.id)
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
        _: &'a JobInput,
        old: &'a JobValue,
        fresh: &'a JobValue,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id
                && old.snapshot.id == fresh.snapshot.id
                && old.env.id == fresh.env.id
                && old.env.generation == fresh.env.generation
                && old.env.machine_id == fresh.env.machine_id
                && old.env.workspace_root == fresh.env.workspace_root)
        })
    }
}

/// Called at the existing runner's final canonical issue seam; no policy or IO is replayed here.
pub(crate) async fn decorate(
    job: &crate::jobs::runner::Job,
    command: &mut crate::commands::Outgoing,
) -> Result<(), crate::jobs::runner::Stop> {
    let row = job.row().await?;
    let expected = match &command.authority {
        Some(crate::commands::CommandAuthority::SnapshotCreation(authority)) => {
            Some(authority.request.clone())
        }
        _ => None,
    };
    if !has_origin(&row) && expected.is_none() {
        return Ok(());
    }
    let expected = expected
        .ok_or_else(|| EngineError::unreadable("the snapshot runner lost its original source"))?;
    let mut conn = job
        .engine()
        .db()
        .acquire()
        .await
        .map_err(EngineError::from)?;
    let authority = SnapshotCommand::capture(&mut conn, &row, &expected, command).await?;
    command.authority = Some(crate::commands::CommandAuthority::snapshot_creation(
        authority,
    ));
    Ok(())
}

/// The actual native creator consumes this source recipe in its first-effect transaction.
pub(crate) fn with_origin(
    plan: &mut Json,
    asked: &Asked,
    origin: Option<&SnapshotRequest>,
    snapshot: Id,
    job: Id,
) -> Result<(), EngineError> {
    if let Some(origin) = origin {
        if origin.job.is_some() || origin.snapshot.is_some() || !origin.matches(asked) {
            return Err(EngineError::unreadable(
                "the snapshot creator changed its prepared operands",
            ));
        }
        let mut accepted = origin.clone();
        accepted.job = Some(job);
        accepted.snapshot = Some(snapshot);
        plan[ORIGIN] = serde_json::to_value(accepted)?;
    }
    Ok(())
}
