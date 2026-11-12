//! Exact Environment source of a reference card's once-only live file requests.
use super::{
    continuing::{self, Error, Source},
    sources::Facts,
    targets,
};
use crate::{EngineError, commands::Outgoing};
use futures::future::BoxFuture;
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::KeyTargetKind,
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{
    documents::ReferencePreviewGet,
    operation_sources::Declaration,
    reference_sources::{self, Source as Role},
    routes::{Endpoint, Route},
};
use orch_authz::{Caller, CapturedOperation, Policy, Principal};
use orch_db::{
    Conn, DbError,
    queries::environments::workspace::{self, EnvironmentRow},
};
use orch_link_contract::commands::{CommandKind, FsFetch, FsStat, SerialKey};
use orch_types::{Clock, Id, Time};
use serde::{Deserialize, Serialize};

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileSource {
    origin: CapturedOperation,
    physical: Physical,
}
impl std::fmt::Debug for FileSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FileSource([private])")
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Physical {
    environment: Id,
    owner: Id,
    home: Id,
    org: Id,
    placement: String,
    generation: i32,
    machine: Option<Id>,
    root: String,
    path: String,
    attached: Option<Time>,
    runtime: Option<Id>,
}
async fn physical(
    conn: &mut Conn,
    row: &EnvironmentRow,
    path: &str,
) -> Result<Physical, EngineError> {
    let attached: Option<chrono::DateTime<chrono::Utc>> =
        sqlx::query_scalar("SELECT attached_session FROM environments WHERE id=$1")
            .bind(row.id)
            .fetch_one(&mut *conn)
            .await
            .map_err(DbError::from)?;
    let runtime = if row.placement == "managed" {
        sqlx::query_scalar("SELECT r.id FROM runtimes r JOIN environments e ON e.id=r.environment_id WHERE e.id=$1 AND r.kind='environment' AND r.generation=e.generation AND r.template_id=e.template_id ORDER BY r.created_at DESC,r.id DESC LIMIT 1").bind(row.id).fetch_optional(&mut *conn).await.map_err(DbError::from)?
    } else {
        None
    };
    Ok(Physical {
        environment: row.id,
        owner: row.owner_id,
        home: row.home_node_id,
        org: row.org_id,
        placement: row.placement.clone(),
        generation: row.generation,
        machine: row.machine_id,
        root: row.workspace_root.clone(),
        path: path.into(),
        attached: attached.map(Time::new),
        runtime,
    })
}
pub struct Reader;
impl Source for Reader {
    type Role = Role;
    type Input = FileSource;
    type Value = EnvironmentRow;
    const ROUTE: Route = ReferencePreviewGet::ROUTE;
    const DECLARATION: &'static Declaration<Role> = &reference_sources::PREVIEW;
    fn prepare<'a>(
        conn: &'a mut Conn,
        _caller: &'a Caller,
        input: &'a FileSource,
        _now: Time,
    ) -> BoxFuture<'a, Result<(EnvironmentRow, Vec<Facts<Role>>), Error>> {
        Box::pin(async move {
            let row = workspace::environment(conn, input.physical.environment)
                .await?
                .ok_or(Error::Refused)?;
            if physical(conn, &row, &input.physical.path).await? != input.physical {
                return Err(Error::Refused);
            }
            let target = targets::resource_identity(
                conn,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: row.id,
                },
            )
            .await
            .map_err(EngineError::unreadable)?;
            let mut facts = vec![Facts::one(
                Role::Target,
                KeyTargetKind::Object(ObjectKind::Environment),
                target,
            )];
            // A live Environment-file card has no foreign place, author or parent-name payload.
            facts.extend(
                reference_sources::NAMES
                    .iter()
                    .map(|(_, role)| Facts::collection(*role, Vec::new())),
            );
            Ok((row, facts))
        })
    }
    fn require<'a>(
        conn: &'a mut Conn,
        principal: &'a Principal,
        input: &'a FileSource,
        _value: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move {
            if !orch_authz::resources::check(
                conn,
                principal,
                ObjectRef {
                    kind: ObjectKind::Environment,
                    id: input.physical.environment,
                },
                Permission::FilesView,
                Policy::default(),
            )
            .await?
            .is_allowed()
            {
                return Err(Error::Refused);
            }
            Ok(())
        })
    }
    fn locks<'a>(
        conn: &'a mut Conn,
        _principal: &'a Principal,
        input: &'a FileSource,
        _value: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<(), Error>> {
        Box::pin(async move { input.lock_source(conn).await.map_err(Error::from) })
    }
    fn check_opened<'a>(
        _conn: &'a mut Conn,
        _principal: &'a Principal,
        _input: &'a FileSource,
        opened: &'a EnvironmentRow,
        fresh: &'a EnvironmentRow,
    ) -> BoxFuture<'a, Result<bool, Error>> {
        Box::pin(async move {
            Ok(opened.id == fresh.id
                && opened.machine_id == fresh.machine_id
                && opened.generation == fresh.generation
                && opened.workspace_root == fresh.workspace_root
                && opened.owner_id == fresh.owner_id
                && opened.home_node_id == fresh.home_node_id)
        })
    }
}
impl FileSource {
    /// Capture only actual native operands on the original admitted HTTP operation.
    pub async fn capture(
        conn: &mut Conn,
        caller: &Caller,
        environment: Id,
        path: &str,
    ) -> Result<Self, EngineError> {
        if caller.admitted_operation() != Some("ReferencePreviewGet")
            || !orch_documents::references::relative_path(path)
        {
            return Err(EngineError::unreadable(
                "the reference file source changed its operation or path",
            ));
        }
        let row = workspace::environment(conn, environment)
            .await?
            .ok_or_else(|| EngineError::unreadable("the reference environment disappeared"))?;
        Ok(Self {
            origin: CapturedOperation::capture(caller).map_err(EngineError::unreadable)?,
            physical: physical(conn, &row, path).await?,
        })
    }
    pub fn origin(&self) -> &CapturedOperation {
        &self.origin
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        let _ = sqlx::query("SELECT id FROM environments WHERE id=$1 FOR SHARE")
            .bind(self.physical.environment)
            .fetch_optional(&mut *conn)
            .await
            .map_err(DbError::from)?;
        if let Some(machine) = self.physical.machine {
            let _ = sqlx::query("SELECT id FROM machines WHERE id=$1 FOR SHARE")
                .bind(machine)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
        }
        if let Some(runtime) = self.physical.runtime {
            let _ = sqlx::query("SELECT id FROM runtimes WHERE id=$1 FOR SHARE")
                .bind(runtime)
                .fetch_optional(&mut *conn)
                .await
                .map_err(DbError::from)?;
        }
        Ok(())
    }
    pub fn command(
        &self,
        command: &Outgoing,
    ) -> Result<super::http_command_source::CommandEvidence, EngineError> {
        if command.machine
            != self
                .physical
                .machine
                .ok_or_else(|| EngineError::unreadable("the environment is offline"))?
            || command.key != SerialKey::None
        {
            return Err(EngineError::unreadable(
                "the reference command changed its native machine",
            ));
        }
        match command.kind {
            CommandKind::FsStat => {
                let body: FsStat = serde_json::from_value(command.body.clone())?;
                if body.environment_id != self.physical.environment
                    || body.mount.is_some()
                    || body.path != self.physical.path
                {
                    return Err(EngineError::unreadable(
                        "the reference stat changed its native source",
                    ));
                }
                Ok(super::http_command_source::CommandEvidence::ReferenceStat(
                    self.clone(),
                ))
            }
            CommandKind::FsFetch => {
                let body: FsFetch = serde_json::from_value(command.body.clone())?;
                if body.environment_id != self.physical.environment
                    || body.mount.is_some()
                    || body.path != self.physical.path
                    || body.expect.is_some()
                    || body.max_bytes.is_none_or(|bytes| bytes < 0)
                {
                    return Err(EngineError::unreadable(
                        "the reference fetch changed its native source",
                    ));
                }
                Ok(super::http_command_source::CommandEvidence::ReferenceFetch(
                    self.clone(),
                ))
            }
            _ => Err(EngineError::unreadable(
                "the reference source cannot send that command",
            )),
        }
    }
    pub async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match continuing::prepare::<Reader>(conn, clock, &self.origin, self.clone()).await {
            Ok(frame) => {
                let row = frame.value();
                let attached = if row.placement == "connected" {
                    orch_db::queries::environments::hosting::connected(conn, row.id)
                        .await?
                        .is_some_and(|directory| directory.attached())
                } else {
                    row.status == "running"
                };
                let machine = match row.machine_id {
                    Some(machine) => orch_db::queries::resources::machine(conn, machine).await?,
                    None => None,
                };
                if !attached || machine.is_none_or(|machine| !machine.is_active()) {
                    return Ok(None);
                }
                frame
                    .check_deadline(clock)
                    .map_err(|e| EngineError::BeforeEffect(Box::new(e)))?;
                Ok(Some(vec![(Some(frame.principal().caller().clone()), None)]))
            }
            Err(Error::Refused | Error::Binding(_)) => Ok(None),
            Err(Error::Native(e)) => Err(e),
        }
    }
}
