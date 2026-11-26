//! Original request and physical destination of one accepted workspace upload job.

use super::*;
use crate::operation_scope::{
    continuing::{self, Error, Source},
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
    operation_sources::Declaration,
    routes::{Endpoint, Route},
    workspace::EnvironmentUploadWrite,
    workspace_sources::{self, Source as Role},
};
use orch_authz::{Caller, Policy, Principal};
use orch_db::{Conn, DbError, queries::jobs::JobRow};
use orch_link_contract::commands::CommandKind;
use orch_types::{Clock, Time};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandSource {
    job: Id,
    command: Id,
    kind: CommandKind,
    request: crate::operation_scope::http_origin::HttpOrigin,
}
impl std::fmt::Debug for CommandSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("UploadCommand([private])")
    }
}
struct Writer;
struct Prepared {
    row: JobRow,
    admission: Admission,
}
impl Source for Writer {
    type Role = Role;
    type Input = CommandSource;
    type Value = Prepared;
    const ROUTE: Route = EnvironmentUploadWrite::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &workspace_sources::ENVIRONMENT_UPLOAD_WRITE;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _: &'a Caller,
        input: &'a CommandSource,
        _: Time,
    ) -> BoxFuture<'a, Result<(Prepared, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = jobs::job(conn, input.job).await?.ok_or(Error::Refused)?;
            if row.kind != jobs::kind::DELIVERY
                || !row.is_open()
                || row.cancel_requested()
                || row.plan.get("workspace_upload").is_none()
            {
                return Err(Error::Refused);
            }
            let admission: Admission = serde_json::from_value(row.plan["workspace_upload"].clone())
                .map_err(EngineError::from)?;
            if admission.raw != input.request
                || row.created_by != Some(admission.by)
                || row.environment_id != Some(admission.write.environment_id)
            {
                return Err(Error::Refused);
            }
            let env = environments::workspace::environment(conn, admission.write.environment_id)
                .await?
                .ok_or(Error::Refused)?;
            if env.machine_id != Some(admission.machine)
                || env.generation != admission.generation
                || env.workspace_root != admission.workspace_root
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
            .map_err(EngineError::unreadable)?;
            let family = KeyTargetKind::Object(ObjectKind::Environment);
            let machine_names = targets::machine_identities(conn, &[admission.machine])
                .await
                .map_err(EngineError::unreadable)?
                .remove(&admission.machine)
                .ok_or(Error::Refused)?;
            let facts = vec![
                Facts::one(Role::UploadEnvironment, family, target.clone()),
                Facts::one(Role::UploadContent, family, target),
                Facts::collection(
                    Role::JobMachineNames,
                    vec![(
                        KeyTargetKind::Object(ObjectKind::Machine),
                        vec![machine_names],
                    )],
                ),
            ];
            Ok((Prepared { row, admission }, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        _: &'a CommandSource,
        value: &'a Prepared,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let admitted = &value.admission;
            if principal.user() != admitted.by
                || !orch_authz::resources::check(
                    conn,
                    principal,
                    ObjectRef {
                        kind: ObjectKind::Environment,
                        id: admitted.write.environment_id,
                    },
                    Permission::FilesManage,
                    Policy::default(),
                )
                .await?
                .is_allowed()
            {
                return Err(Error::Refused);
            }
            let env = environments::workspace::environment(conn, admitted.write.environment_id)
                .await?
                .ok_or(Error::Refused)?;
            let reach =
                orch_authz::file_reach::effective(conn, principal, env.id, Policy::default())
                    .await?;
            if !admitted
                .write
                .access
                .manage
                .is_some_and(|asked| reach.manage.is_some_and(|held| held >= asked))
            {
                return Err(Error::Refused);
            }
            if !env.remote_write
                || orch_db::queries::environment_facts::refuses(
                    conn,
                    env.id,
                    "environment.remote_write",
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
        input: &'a CommandSource,
        value: &'a Prepared,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            let _ = jobs::lock(conn, input.job).await?;
            sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
                .bind(value.admission.write.environment_id)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
            sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(value.admission.machine)
                .fetch_optional(conn)
                .await
                .map_err(DbError::from)?;
            Ok(())
        })
    }
    fn check_opened<'a>(
        _: &'a mut Conn,
        _: &'a Principal,
        _: &'a CommandSource,
        old: &'a Prepared,
        fresh: &'a Prepared,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(old.row.id == fresh.row.id
                && old.admission.machine == fresh.admission.machine
                && old.admission.generation == fresh.admission.generation
                && old.admission.workspace_root == fresh.admission.workspace_root)
        })
    }
}
impl CommandSource {
    pub fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = jobs::lock(conn, self.job).await?;
        Ok(())
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match continuing::prepare::<Writer>(conn, clock, self.request.origin(), self.clone()).await
        {
            Ok(frame) => {
                frame
                    .check_deadline(clock)
                    .map_err(|error| EngineError::BeforeEffect(Box::new(error)))?;
                Ok(Some(vec![(Some(frame.principal().caller().clone()), None)]))
            }
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(error)) => Err(error),
        }
    }
}

/// Called after the runner supplies the actual work report and before any forward queue effect.
pub(crate) async fn decorate(job: &Job, command: &mut Outgoing) -> Result<(), Stop> {
    if !matches!(
        command.kind,
        CommandKind::FsWrite | CommandKind::FsRelayPrepare | CommandKind::FsRelayCommit
    ) {
        return Ok(());
    }
    let mut conn = job
        .engine()
        .db()
        .acquire()
        .await
        .map_err(EngineError::from)?;
    let Some(row) = jobs::job(&mut conn, job.id())
        .await
        .map_err(EngineError::from)?
    else {
        return Err(Stop::Cancelled);
    };
    if row.kind != jobs::kind::DELIVERY || row.plan.get("workspace_upload").is_none() {
        return Ok(());
    }
    let admission: Admission =
        serde_json::from_value(row.plan["workspace_upload"].clone()).map_err(EngineError::from)?;
    drop(conn);
    if let Some(retained) = job
        .engine()
        .inner
        .commands
        .retained_command(command.id)
        .await
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?
    {
        super::super::runner::preserve_creator_report(command, &retained)?;
    }
    let expected_id = if command.kind == CommandKind::FsRelayPrepare {
        crate::ids::derived("workspace.upload.prepare", &[job.id().as_bytes()])
    } else {
        super::command_id(job.id())
    };
    if command.id != expected_id || command.machine != admission.machine {
        return Err(Stop::Failed(Failure::new(
            INTERNAL,
            "the upload command changed its job identity",
        )));
    }
    let valid = match command.kind {
        CommandKind::FsWrite if admission.relay.is_none() => {
            let body: FsWrite =
                serde_json::from_value(command.body.clone()).map_err(EngineError::from)?;
            let mut expected = admission.write.clone();
            expected.work = body.work.clone();
            body == expected
                && command.key == SerialKey::Environment(admission.write.environment_id)
        }
        CommandKind::FsRelayPrepare => {
            let body: FsRelayPrepare =
                serde_json::from_value(command.body.clone()).map_err(EngineError::from)?;
            admission.relay.as_ref().is_some_and(|relay| {
                body.environment_id == admission.write.environment_id
                    && body.path == admission.write.path
                    && body.receive_path == relay.receive_path
                    && body.session_id == relay.session_id
            }) && command.key == SerialKey::None
        }
        CommandKind::FsRelayCommit => {
            let body: FsRelayCommit =
                serde_json::from_value(command.body.clone()).map_err(EngineError::from)?;
            admission.relay.as_ref().is_some_and(|relay| {
                body.environment_id == admission.write.environment_id
                    && body.path == admission.write.path
                    && body.receive_path == relay.receive_path
                    && body.session_id == relay.session_id
                    && body.hash == admission.write.blob
                    && body.size == admission.size
                    && body.mode == admission.write.mode
                    && body.expected_hash == admission.write.expected_hash
            }) && command.key == SerialKey::Environment(admission.write.environment_id)
        }
        _ => false,
    };
    if !valid || command.body.pointer("/work/job_id") != Some(&serde_json::json!(job.id())) {
        return Err(Stop::Failed(Failure::new(
            INTERNAL,
            "the upload command changed its actual file operands",
        )));
    }
    let source = CommandSource {
        job: job.id(),
        command: command.id,
        kind: command.kind,
        request: admission.raw.clone(),
    };
    let mut tx = job.engine().db().begin().await.map_err(EngineError::from)?;
    let frame = admission
        .raw
        .prepare::<Writer>(&mut tx, job.engine().inner.clock.as_ref(), source.clone())
        .await
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
    let raw = frame
        .http_origin()
        .ok_or_else(|| EngineError::unreadable("the upload lost its admitted HTTP source"))?;
    command.authority = Some(raw.command_source(
        command,
        crate::operation_scope::http_command_source::CommandEvidence::UploadWrite(source),
    )?);
    tx.commit()
        .await
        .map_err(orch_db::DbError::from)
        .map_err(EngineError::from)?;
    frame
        .check_deadline(job.engine().inner.clock.as_ref())
        .map_err(|error| Stop::Failed(Failure::new(INTERNAL, error.to_string())))?;
    Ok(())
}
