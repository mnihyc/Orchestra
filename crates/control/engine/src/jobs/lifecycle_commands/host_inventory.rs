//! Initial Host inventory is native preparation, never a complete cascade or business origin.
use super::*;
use crate::operation_scope::{
    continuing, http_origin,
    sources::{self, Facts},
    targets,
};
use orch_access_contract::{
    decisions::ObjectRef,
    key_scope::{KeyAction, KeyTargetKind},
    permissions::{ObjectKind, Permission},
};
use orch_api_contract::{host_delete_sources as host, loss::HostDeletePreflight, routes::Endpoint};
use orch_authz::http_origin::HttpPreparation;
use orch_db::queries::resources::{self, hosting as hosts};
use orch_link_contract::commands::{CommandKind, SerialKey};
use std::sync::{Arc, Mutex, PoisonError};

type InventoryFrame = (crate::harnesses::preparation::Checkpoint, HttpPreparation);
type CurrentCallers = Option<Vec<(Option<Caller>, Option<Time>)>>;

fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(continuing::Error::Refused))
}
fn raw(error: orch_authz::http_origin::Error) -> EngineError {
    match http_origin::mapped(error) {
        continuing::Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}
fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Real native preparation input and original raw caller, without an admitted operation.
#[derive(Clone)]
pub struct Preparation {
    host: Id,
    machine: Id,
    initial: HttpPreparation,
    retained: Arc<Mutex<HttpPreparation>>,
}
impl Preparation {
    pub async fn new(
        conn: &mut Conn,
        origin: HttpPreparation,
        host: Id,
        machine: Id,
        clock: &dyn Clock,
    ) -> Result<Self, EngineError> {
        let source = Self {
            host,
            machine,
            initial: origin.clone(),
            retained: Arc::new(Mutex::new(origin)),
        };
        let (check, _) = source.checkpoint(conn, clock).await?;
        check.check(clock)?;
        Ok(source)
    }
    pub fn host(&self) -> Id {
        self.host
    }
    pub fn retained(&self) -> HttpPreparation {
        lock(&self.retained).clone()
    }
    pub(crate) fn original(&self) -> &HttpPreparation {
        &self.initial
    }
    pub fn request(&self) -> jobs::measurement::Request {
        jobs::measurement::Request {
            machine: self.machine,
            kind: CommandKind::NodeResourcesList,
            body: serde_json::json!({}),
            subject: serde_json::json!({"host_id":self.host}),
            by: self.initial.user(),
            reuse_unsupported: false,
        }
    }
    pub fn measurement(&self) -> Result<jobs::measurement::Preparation, EngineError> {
        jobs::measurement::Preparation::from_host_inventory(self.clone())
    }
    pub(crate) fn association(&self) -> Result<orch_types::Hash, EngineError> {
        command_identity::hash(&serde_json::json!([self.host, self.machine, self.initial]))
    }
    pub(crate) fn matches_request(&self, request: &jobs::measurement::Request) -> bool {
        request.machine == self.machine
            && request.kind == CommandKind::NodeResourcesList
            && request.body == serde_json::json!({})
            && request.subject == serde_json::json!({"host_id":self.host})
            && request.by == self.initial.user()
            && !request.reuse_unsupported
    }
    pub(crate) fn accepted(&self, origin: HttpPreparation) -> Result<Self, EngineError> {
        let origin = self.retained().intersect(&origin).map_err(raw)?;
        Ok(Self {
            initial: self.initial.clone(),
            retained: Arc::new(Mutex::new(origin)),
            ..self.clone()
        })
    }
    pub(crate) fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<InventoryFrame, EngineError>> {
        Box::pin(async move {
            let origin = self.retained();
            if origin.operation() != host::PREFLIGHT.operation {
                return Err(refused());
            }
            origin.lock_identity(conn).await.map_err(raw)?;
            resources::lock_resource(conn, "host", self.host).await?;
            let current = origin.current(conn, clock).await.map_err(raw)?;
            hosts::host(conn, self.host)
                .await?
                .filter(|row| row.machine_id == self.machine)
                .ok_or_else(refused)?;
            let facts = [Facts::one(
                host::Source::Host,
                KeyTargetKind::Object(ObjectKind::Host),
                targets::resource_identity(
                    conn,
                    ObjectRef {
                        kind: ObjectKind::Host,
                        id: self.host,
                    },
                )
                .await
                .map_err(EngineError::unreadable)?,
            )];
            let common = sources::common::Common::bind_preparation(
                HostDeletePreflight::ROUTE,
                &[host::PREFLIGHT],
                &facts,
            )
            .map_err(EngineError::unreadable)?;
            let caller = common
                .preparation_caller(current.caller())
                .map_err(|_| refused())?;
            let principal = Principal::load(conn, caller, clock.time()).await?;
            let known = crate::operation_scope::sources::bind_known(
                HostDeletePreflight::ROUTE,
                &host::PREFLIGHT,
                &facts,
            )
            .map_err(EngineError::unreadable)?
            .known_required_facets();
            let principal = continuing::hold_principal(
                conn,
                current.user(),
                origin.session(),
                principal,
                &known,
            )
            .await
            .map_err(|error| match error {
                continuing::Error::Native(error) => error,
                error => EngineError::BeforeEffect(Box::new(error)),
            })?;
            crate::operation_access::review(
                conn,
                &principal,
                ObjectRef {
                    kind: ObjectKind::Host,
                    id: self.host,
                },
                Permission::HostManage,
            )
            .await
            .map_err(|_| refused())?;
            if !prefix_caller(principal.caller()) {
                return Err(refused());
            }
            current.check(clock).map_err(raw)?;
            let accepted = origin
                .accepted(principal.caller(), current.until())
                .map_err(raw)?;
            let retained = self.retained().intersect(&accepted).map_err(raw)?;
            *lock(&self.retained) = retained;
            let tail = current;
            let check = crate::harnesses::preparation::Checkpoint::from_check(
                principal.caller().clone(),
                accepted.until(),
                Box::new(move |clock| tail.check(clock).map_err(raw)),
            );
            Ok((check, accepted))
        })
    }
    pub(crate) async fn command_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        job: &rows::JobRow,
        request: &jobs::measurement::Request,
        command: &Outgoing,
    ) -> Result<(CommandAuthority, HttpPreparation), EngineError> {
        if !self.matches_request(request)
            || job.state != "queued"
            || job.cancel_requested()
            || job.kind != jobs::measurement::KIND
            || job.created_by != Some(self.initial.user())
            || job.plan["subject"] != request.subject
            || job.plan["body"] != request.body
            || job.plan["command_kind"] != serde_json::json!("node.resources.list")
            || job.plan["machine"] != serde_json::json!(self.machine)
            || command.kind != CommandKind::NodeResourcesList
            || command.machine != self.machine
            || command.key != SerialKey::None
            || command.body != serde_json::json!({})
        {
            return Err(refused());
        }
        let inserted:bool=sqlx::query_scalar("SELECT xmin=((pg_current_xact_id()::text::numeric % 4294967296)::text)::xid FROM jobs WHERE id=$1")
            .bind(job.id).fetch_one(&mut *conn).await.map_err(orch_db::DbError::from)?;
        if !inserted {
            return Err(refused());
        }
        let (check, origin) = self.checkpoint(conn, clock).await?;
        let packet = Packet {
            host: self.host,
            machine: self.machine,
            job: job.id,
            timeout_secs: job.plan["timeout_secs"].as_u64().ok_or_else(refused)?,
            origin: origin.clone(),
            canonical: command_identity::canonical(
                command.id,
                command.machine,
                None,
                command.kind.as_str(),
                &command.body,
            )?,
        };
        check.check(clock)?;
        Ok((
            CommandAuthority::Lifecycle(Box::new(super::Evidence::Inventory(Box::new(packet)))),
            origin,
        ))
    }
}

/// Known Host Manage only; no exact/full/empty admission or effect/capture handoff.
pub(crate) fn prefix_caller(caller: &Caller) -> bool {
    caller.admitted_operation().is_none()
        && caller.admitted_required_roles().is_none()
        && caller.admitted_empty_facets().is_none()
        && caller.admitted_facets().is_some_and(|facts| {
            !facts.is_empty()
                && facts.iter().all(|fact| {
                    fact.operation == host::PREFLIGHT.operation
                        && fact.action == KeyAction::Manage
                        && fact.resource_kind == KeyTargetKind::Object(ObjectKind::Host)
                })
        })
}

/// Private packet in the existing lifecycle command family and actual measurement job.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    host: Id,
    machine: Id,
    job: Id,
    timeout_secs: u64,
    origin: HttpPreparation,
    canonical: Json,
}
impl Packet {
    pub fn command_kind(&self) -> &'static str {
        "node.resources.list"
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.origin.lock_identity(conn).await.map_err(raw)?;
        resources::lock_resource(conn, "host", self.host).await?;
        Ok(())
    }
    async fn current(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<CurrentCallers, EngineError> {
        if command.serial_key.is_some()
            || self.canonical
                != command_identity::canonical(
                    command.id,
                    command.machine_id,
                    command.serial_key,
                    command.kind,
                    command.body,
                )?
        {
            return Ok(None);
        }
        let Some(row) = rows::lock(conn, self.job).await? else {
            return Ok(None);
        };
        if row.kind != jobs::measurement::KIND
            || row.environment_id.is_some()
            || row.provider_id.is_some()
            || row.state != "running"
            || row.plan["timeout_secs"].as_u64() != Some(self.timeout_secs)
            || row.cancel_requested()
            || jobs::runner::abort_reason(&row)?.is_some()
            || jobs::runner::read_progress(&row).0.phase != "measure"
        {
            return Ok(None);
        }
        let original =
            jobs::measurement::preparation::inventory_origin(&row)?.ok_or_else(refused)?;
        let origin = self.origin.intersect(&original).map_err(raw)?;
        let source = Preparation {
            host: self.host,
            machine: self.machine,
            initial: origin.clone(),
            retained: Arc::new(Mutex::new(origin)),
        };
        if row.created_by != Some(source.initial.user())
            || !source.matches_request(&jobs::measurement::preparation::request_of(&row)?)
        {
            return Ok(None);
        }
        let expected = jobs::measurement::preparation::build_command(
            &row,
            jobs::measurement::preparation::MeasurementPhase::Measure,
            clock.now(),
        )?;
        if expected.id != command.id
            || command.deadline_at > jobs::measurement::preparation::inventory_deadline(&row)?.get()
            || clock.now() >= command.deadline_at
        {
            return Ok(None);
        }
        let (check, _) = source.checkpoint(conn, clock).await?;
        let result = vec![(Some(check.caller().clone()), check.until())];
        check.check(clock)?;
        Ok(Some(result))
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
    ) -> Result<CurrentCallers, EngineError> {
        match self.current(conn, command, clock).await {
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            other => other,
        }
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
    ) -> Result<CurrentCallers, EngineError> {
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
