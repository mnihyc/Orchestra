//! Actual lifecycle preparation sources in the single durable lifecycle command family.
use crate::{
    Engine, EngineError,
    access::ResourceAccess,
    commands::{CommandAuthority, Outgoing},
};
use orch_authz::{Caller, CapturedOperation};
use orch_db::{
    Conn,
    queries::link::{CommandRow, NewCommand},
};
use orch_types::{Clock, Hash, Id, Time};
use serde::{Deserialize, Serialize};

pub mod execution;

mod origin;
mod physical;
mod scope;
pub use origin::{Kind, Origin};
pub use scope::{Scope, scope};
pub(crate) use scope::{checkpoint, command, count, is_active, select_destinations};
fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}

/// No endpoint or raw command can construct a preparation from a label or a guessed job.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "source",
    content = "evidence",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum CommandEvidence {
    Native(Box<NativeCommand>),
    /// A separate real parent origin may fill this closed domain delegation port.
    Delegated(Delegation),
}
/// Reserved domain-owned parent port; an absent producer cannot manufacture authority.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Delegation {}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeCommand {
    origin: Origin,
    id: Id,
    machine: Id,
    kind: orch_link_contract::commands::CommandKind,
    key: Option<String>,
    body: Hash,
    deadline: Time,
    measurement: Option<Measurement>,
}
impl std::fmt::Debug for NativeCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("NativeLifecycleCommand([private])")
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Measurement {
    job: Id,
    phase: crate::harnesses::preparation::MeasurementPhase,
    request: Hash,
}
impl Measurement {
    async fn current(
        &self,
        conn: &mut Conn,
        origin: &Origin,
        command: &Outgoing,
    ) -> Result<bool, EngineError> {
        let Some(row) = orch_db::queries::jobs::job(conn, self.job).await? else {
            return Ok(false);
        };
        if row.kind != crate::jobs::measurement::KIND
            || !row.is_open()
            || row.cancel_requested()
            || row.created_by != Some(origin.captured().user())
        {
            return Ok(false);
        }
        let hash = crate::jobs::command_identity::hash(&serde_json::json!([
            row.plan["machine"],
            row.plan["command_kind"],
            row.plan["body"],
            row.plan["subject"],
            row.created_by
        ]))?;
        if hash != self.request {
            return Ok(false);
        }
        let work = command.body.get("work").and_then(|v| {
            serde_json::from_value::<orch_link_contract::commands::WorkReport>(v.clone()).ok()
        });
        Ok(work.is_some_and(|w| w.job_id == self.job && w.phase == self.phase.as_str()))
    }
}
impl NativeCommand {
    async fn new(
        conn: &mut Conn,
        origin: &Origin,
        clock: &dyn Clock,
        target: crate::harnesses::preparation::Target,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.authority.is_some()
            || !origin
                .physical(
                    conn,
                    command.machine,
                    command.kind,
                    &command.body,
                    clock.time(),
                )
                .await?
        {
            return Err(closed());
        }
        origin.checkpoint(conn, clock).await?.check(clock)?;
        physical::lock(conn, command.machine, command.kind, &command.body).await?;
        let check = origin.checkpoint(conn, clock).await?;
        if !origin
            .physical(
                conn,
                command.machine,
                command.kind,
                &command.body,
                clock.time(),
            )
            .await?
        {
            return Err(closed());
        }
        let origin = origin.accepted(
            CapturedOperation::capture(check.caller()).map_err(EngineError::unreadable)?,
            check.until(),
        )?;
        let measurement = match target {
            crate::harnesses::preparation::Target::Measurement {
                job: Some(job),
                machine,
                kind,
                phase: Some(phase),
                ..
            } if machine == command.machine && kind == command.kind => {
                let row = orch_db::queries::jobs::job(conn, job)
                    .await?
                    .ok_or_else(closed)?;
                let request = crate::jobs::command_identity::hash(&serde_json::json!([
                    row.plan["machine"],
                    row.plan["command_kind"],
                    row.plan["body"],
                    row.plan["subject"],
                    row.created_by
                ]))?;
                Some(Measurement {
                    job,
                    phase,
                    request,
                })
            }
            // Direct native probes are not durable measurement jobs.
            crate::harnesses::preparation::Target::Measurement {
                job: None,
                machine,
                kind,
                phase: None,
                ..
            } if machine == command.machine && kind == command.kind => None,
            _ => return Err(closed()),
        };
        let native = Self {
            origin,
            id: command.id,
            machine: command.machine,
            kind: command.kind,
            key: command.key.wire(),
            body: crate::jobs::command_identity::hash(&command.body)?,
            deadline: command.deadline.ok_or_else(closed)?,
            measurement,
        };
        if let Some(owner) = &native.measurement
            && !owner.current(conn, &native.origin, command).await?
        {
            return Err(closed());
        }
        check.check(clock)?;
        Ok(native)
    }
    fn matches(
        &self,
        id: Id,
        machine: Id,
        kind: &str,
        key: Option<&str>,
        body: &orch_types::Json,
        deadline: Time,
    ) -> Result<bool, EngineError> {
        Ok(self.id == id
            && self.machine == machine
            && self.kind.as_str() == kind
            && self.key.as_deref() == key
            && self.body == crate::jobs::command_identity::hash(body)?
            && self.deadline == deadline)
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: &Outgoing,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if clock.time() >= self.deadline
            || !self
                .origin
                .physical(
                    conn,
                    command.machine,
                    command.kind,
                    &command.body,
                    clock.time(),
                )
                .await?
        {
            return Ok(None);
        }
        if let Some(owner) = &self.measurement
            && !owner.current(conn, &self.origin, command).await?
        {
            return Ok(None);
        }
        physical::lock(conn, command.machine, command.kind, &command.body).await?;
        match self.origin.checkpoint(conn, clock).await {
            Ok(check) => {
                if !self
                    .origin
                    .physical(
                        conn,
                        command.machine,
                        command.kind,
                        &command.body,
                        clock.time(),
                    )
                    .await?
                {
                    return Ok(None);
                }
                let caller = check.caller().clone();
                let until = check.until().into_iter().chain(Some(self.deadline)).min();
                check.check(clock)?;
                Ok(Some(vec![(Some(caller), until)]))
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}
impl CommandEvidence {
    fn native(&self) -> &NativeCommand {
        match self {
            Self::Native(n) => n,
            Self::Delegated(d) => match *d {},
        }
    }
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Native(n) => n.kind.as_str(),
            Self::Delegated(d) => match *d {},
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Native(n) => {
                n.origin.lock_source(conn).await?;
                if let Some(owner) = &n.measurement
                    && let Some(row) = orch_db::queries::jobs::job(conn, owner.job).await?
                {
                    let body = if n.kind
                        == orch_link_contract::commands::CommandKind::NodeMountFingerprint
                    {
                        let pair: crate::jobs::measurement::FingerprintInput =
                            serde_json::from_value(row.plan["body"].clone())?;
                        if owner.phase
                            == crate::harnesses::preparation::MeasurementPhase::FingerprintTarget
                        {
                            serde_json::to_value(pair.target)?
                        } else {
                            serde_json::to_value(pair.source)?
                        }
                    } else {
                        row.plan["body"].clone()
                    };
                    physical::lock(conn, n.machine, n.kind, &body).await?;
                }
                Ok(())
            }
            Self::Delegated(d) => match *d {},
        }
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        _: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let n = self.native();
        if !n.matches(
            command.id,
            command.machine_id,
            command.kind,
            command.serial_key,
            command.body,
            Time::new(command.deadline_at),
        )? {
            return Ok(None);
        }
        let actual = Outgoing {
            authority: None,
            id: command.id,
            machine: command.machine_id,
            kind: command.kind.parse().map_err(|_| closed())?,
            key: orch_link_contract::commands::SerialKey::None,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
        };
        n.current(conn, clock, &actual).await
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        _: &dyn ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let n = self.native();
        if !n.matches(
            command.id,
            command.machine_id,
            &command.kind,
            command.serial_key(),
            &command.body,
            Time::new(command.deadline_at),
        )? {
            return Ok(None);
        }
        let actual = Outgoing {
            authority: None,
            id: command.id,
            machine: command.machine_id,
            kind: command.kind.parse().map_err(|_| closed())?,
            key: orch_link_contract::commands::SerialKey::None,
            body: command.body.clone(),
            deadline: Some(Time::new(command.deadline_at)),
        };
        n.current(conn, clock, &actual).await
    }
    pub async fn reprepare_measurement(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        _: &dyn ResourceAccess,
        origin: &CapturedOperation,
        until: Option<Time>,
        command: &Outgoing,
    ) -> Result<(CommandAuthority, CapturedOperation, Option<Time>), EngineError> {
        let n = self.native();
        if n.measurement.is_none()
            || n.id != command.id
            || n.machine != command.machine
            || n.kind != command.kind
            || n.key != command.key.wire()
            || n.body != crate::jobs::command_identity::hash(&command.body)?
        {
            return Err(closed());
        }
        let current = n.origin.accepted(origin.clone(), until)?;
        if !current
            .physical(
                conn,
                command.machine,
                command.kind,
                &command.body,
                clock.time(),
            )
            .await?
            || !n
                .measurement
                .as_ref()
                .ok_or_else(closed)?
                .current(conn, &current, command)
                .await?
        {
            return Err(closed());
        }
        let check = current.checkpoint(conn, clock).await?;
        let accepted = current.accepted(
            CapturedOperation::capture(check.caller()).map_err(EngineError::unreadable)?,
            check.until(),
        )?;
        let mut next = n.clone();
        next.origin = accepted;
        next.deadline = command.deadline.ok_or_else(closed)?;
        let origin = next.origin.captured().clone();
        let until = next.origin.until();
        check.check(clock)?;
        Ok((
            CommandAuthority::lifecycle_preparation(Self::Native(Box::new(next))),
            origin,
            until,
        ))
    }
}
