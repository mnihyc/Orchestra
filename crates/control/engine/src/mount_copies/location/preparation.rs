//! Native copy suppliers retain their own raw request, memberships and exact commands.
use super::{Review, operation::Preview};
use crate::{
    Engine, EngineError,
    commands::{CommandAuthority, Outgoing},
    harnesses::preparation as shared,
    jobs::{command_identity, measurement},
    operation_scope::{continuing::Error, http_origin::HttpOrigin},
};
use futures::future::BoxFuture;
use orch_authz::{Caller, CapturedOperation};
use orch_db::{
    Conn, DbError,
    queries::{
        jobs,
        link::{CommandRow, NewCommand},
    },
};
use orch_link_contract::commands::{CommandKind, NodeMountMeasure, NodeVolumeCarryCheck};
use orch_types::{Clock, Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::{Arc, Mutex, PoisonError};

const MINIMUM: &str = "mount_copy_preparation";
type Callers = Vec<(Option<Caller>, Option<Time>)>;
type PreparedMeasurement = (CommandAuthority, CapturedOperation, Option<Time>);
fn closed() -> EngineError {
    EngineError::BeforeEffect(Box::new(Error::Refused))
}
fn mapped(error: Error) -> EngineError {
    match error {
        Error::Native(error) => error,
        error => EngineError::BeforeEffect(Box::new(error)),
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Minimum {
    review: Review,
    origin: HttpOrigin,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    job: Option<Id>,
    machine: Id,
    kind: CommandKind,
    identity: Hash,
    phase: Option<shared::MeasurementPhase>,
}
impl Target {
    fn of(target: shared::Target) -> Result<Self, EngineError> {
        match target {
            shared::Target::Measurement {
                job,
                machine,
                kind,
                identity,
                phase,
            } => Ok(Self {
                job,
                machine,
                kind,
                identity,
                phase,
            }),
            _ => Err(closed()),
        }
    }
    async fn current(
        &self,
        conn: &mut Conn,
        review: &Review,
        actor: Id,
    ) -> Result<(), EngineError> {
        if ![review.reader.machine, review.writer.machine].contains(&self.machine)
            || !matches!(
                self.kind,
                CommandKind::NodeVolumeCarryCheck | CommandKind::NodeMountMeasure
            )
        {
            return Err(closed());
        }
        if let Some(job) = self.job {
            let row = jobs::job(conn, job).await?.ok_or_else(closed)?;
            if row.kind != measurement::KIND
                || row.created_by != Some(actor)
                || row.plan["machine"] != json!(self.machine)
                || row.plan["command_kind"] != json!(self.kind.as_str())
                || self.kind != CommandKind::NodeMountMeasure
                || self
                    .phase
                    .is_some_and(|phase| phase != shared::MeasurementPhase::Measure)
                || self.phase.is_some() && (!row.is_open() || row.cancel_requested())
                || command_identity::hash(&json!([
                    row.plan["machine"],
                    row.plan["command_kind"],
                    row.plan["body"],
                    row.plan["subject"],
                    row.created_by,
                    row.plan["measurement_optional_home"] == true
                ]))? != self.identity
            {
                return Err(closed());
            }
        } else if self.phase.is_some() {
            return Err(closed());
        }
        Ok(())
    }
}

/// One request-local shared supplier; only its typed command packet survives a restart.
#[derive(Clone)]
pub struct Preparation {
    review: Review,
    initial: HttpOrigin,
    retained: Arc<Mutex<HttpOrigin>>,
}
tokio::task_local! { static ACTIVE: Preparation; }
pub(crate) fn active() -> Option<Preparation> {
    ACTIVE.try_with(Clone::clone).ok()
}
impl Preparation {
    pub fn new(review: Review, origin: HttpOrigin) -> Result<Self, EngineError> {
        if origin.origin().operation() != "MountCopyPreflight" {
            return Err(closed());
        }
        Ok(Self {
            review,
            initial: origin.clone(),
            retained: Arc::new(Mutex::new(origin)),
        })
    }
    pub async fn scope<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        ACTIVE.scope(self.clone(), future).await
    }
    pub fn retained(&self) -> HttpOrigin {
        self.retained
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
    fn retain(&self, next: &HttpOrigin) -> Result<(), EngineError> {
        let mut current = self.retained.lock().unwrap_or_else(PoisonError::into_inner);
        *current = current.intersect(next).map_err(mapped)?;
        Ok(())
    }
    pub(crate) fn measurement(&self) -> Result<measurement::Preparation, EngineError> {
        Ok(measurement::Preparation::new(
            command_identity::hash(&json!([self.review, self.initial]))?,
            shared::Preparation::new(self.clone()),
        ))
    }
    pub(crate) async fn accept_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
    ) -> Result<shared::Checkpoint, EngineError> {
        let frame = self
            .retained()
            .prepare::<Preview>(conn, clock, self.review.clone())
            .await
            .map_err(mapped)?;
        let next = frame.http_origin().cloned().ok_or_else(closed)?;
        self.retain(&next)?;
        let parts = frame.into_parts();
        Ok(shared::Checkpoint::from_check(
            parts.principal.caller().clone(),
            next.until(),
            Box::new(move |clock| parts.first_effect.check(clock).map_err(mapped)),
        ))
    }
    async fn checkpoint_on(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        target: &Target,
    ) -> Result<(shared::Checkpoint, HttpOrigin), EngineError> {
        target
            .current(conn, &self.review, self.initial.origin().user())
            .await?;
        let mut origin = self.retained();
        if let Some(job) = target.job {
            let row = jobs::job(conn, job).await?.ok_or_else(closed)?;
            if let Some(value) = row.plan.get(MINIMUM) {
                let minimum: Minimum = serde_json::from_value(value.clone())?;
                if minimum.review != self.review {
                    return Err(closed());
                }
                origin = origin.intersect(&minimum.origin).map_err(mapped)?;
            } else {
                let made: bool = sqlx::query_scalar(
                    "SELECT xmin=pg_current_xact_id()::xid FROM jobs WHERE id=$1",
                )
                .bind(job)
                .fetch_one(&mut *conn)
                .await
                .map_err(DbError::from)?;
                if !made {
                    return Err(closed());
                }
            }
        }
        let frame = origin
            .prepare::<Preview>(conn, clock, self.review.clone())
            .await
            .map_err(mapped)?;
        let next = frame.http_origin().cloned().ok_or_else(closed)?;
        self.retain(&next)?;
        let parts = frame.into_parts();
        let check = shared::Checkpoint::from_check(
            parts.principal.caller().clone(),
            next.until(),
            Box::new(move |clock| parts.first_effect.check(clock).map_err(mapped)),
        );
        Ok((check, next))
    }
}
impl shared::Source for Preparation {
    fn origin(&self) -> &CapturedOperation {
        self.initial.origin()
    }
    fn until(&self) -> Option<Time> {
        self.initial.until()
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: shared::Target,
    ) -> BoxFuture<'a, Result<shared::Checkpoint, EngineError>> {
        Box::pin(async move {
            Ok(self
                .checkpoint_on(conn, clock, &Target::of(target)?)
                .await?
                .0)
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn shared::Source>, EngineError> {
        let mut accepted = self.clone();
        accepted.initial = self.retained();
        if accepted.initial.origin() != &origin || accepted.initial.until() != until {
            return Err(closed());
        }
        Ok(Arc::new(accepted))
    }
    fn command<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: shared::Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            let target = Target::of(target)?;
            let (check, origin) = self.checkpoint_on(conn, clock, &target).await?;
            let packet = Packet::new(self.review.clone(), origin.clone(), target.clone(), command)?;
            check.check(clock)?;
            if let Some(job) = target.job {
                jobs::merge_plan(
                    conn,
                    job,
                    &json!({MINIMUM:Minimum{review:self.review.clone(),origin:origin.clone()}}),
                    clock.now(),
                )
                .await?;
            }
            let authority = packet.authority();
            if target.job.is_some() {
                Ok(authority)
            } else {
                origin.command(command, authority)
            }
        })
    }
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
enum Version {
    #[serde(rename = "mount_copy_preparation_v1")]
    One,
}
/// Canonical supplier command in the one private lifecycle evidence channel.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Packet {
    version: Version,
    review: Review,
    origin: HttpOrigin,
    target: Target,
    canonical: Json,
}
impl Packet {
    fn new(
        review: Review,
        origin: HttpOrigin,
        target: Target,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.machine != target.machine || command.kind != target.kind {
            return Err(closed());
        }
        let maker = if command.machine == review.reader.machine {
            &review.reader
        } else if command.machine == review.writer.machine {
            &review.writer
        } else {
            return Err(closed());
        };
        match command.kind {
            CommandKind::NodeVolumeCarryCheck if target.job.is_none() => {
                let body: NodeVolumeCarryCheck = serde_json::from_value(command.body.clone())?;
                if body.driver != maker.driver {
                    return Err(closed());
                }
            }
            CommandKind::NodeMountMeasure if target.job.is_some() => {
                let body: NodeMountMeasure = serde_json::from_value(command.body.clone())?;
                let location = if command.machine == review.reader.machine {
                    &review.source.location
                } else {
                    &review.target.as_ref().ok_or_else(closed)?.location
                };
                if body.driver != maker.driver
                    || body.mount_id != review.mount.id
                    || body.data != super::super::DataRef::of(&location.row()).link()
                    || body.work.as_ref().is_none_or(|work| {
                        Some(work.job_id) != target.job || work.phase != "measure"
                    })
                {
                    return Err(closed());
                }
            }
            _ => return Err(closed()),
        }
        Ok(Self {
            version: Version::One,
            review,
            origin,
            target,
            canonical: command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )?,
        })
    }
    fn authority(&self) -> CommandAuthority {
        CommandAuthority::Lifecycle(Box::new(
            crate::jobs::lifecycle_commands::Evidence::MountCopyPreparation(Box::new(self.clone())),
        ))
    }
    pub fn command_kind(&self) -> &'static str {
        self.target.kind.as_str()
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.origin.lock_identity(conn).await
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        command: &NewCommand<'_>,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command_identity::canonical(
            command.id,
            command.machine_id,
            command.serial_key,
            command.kind,
            command.body,
        )? != self.canonical
        {
            return Ok(None);
        }
        let preparation = Preparation::new(self.review.clone(), self.origin.clone())?;
        match preparation.checkpoint_on(conn, clock, &self.target).await {
            Ok((check, origin)) => {
                let caller = check.caller().clone();
                let until = check.until();
                check.check(clock)?;
                if let Some(job) = self.target.job {
                    jobs::merge_plan(
                        conn,
                        job,
                        &json!({MINIMUM:Minimum{review:self.review.clone(),origin}}),
                        clock.now(),
                    )
                    .await?;
                }
                Ok(Some(vec![(Some(caller), until)]))
            }
            Err(EngineError::BeforeEffect(_)) => Ok(None),
            Err(error) => Err(error),
        }
    }
    pub fn current_issue_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a NewCommand<'_>,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Option<Callers>, EngineError>> {
        Box::pin(self.current(conn, clock, command))
    }
    pub fn current_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a CommandRow,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Option<Callers>, EngineError>> {
        Box::pin(async move {
            self.current(
                conn,
                clock,
                &NewCommand {
                    content_transit: command.content_transit,
                    native_target: None,
                    id: command.id,
                    machine_id: command.machine_id,
                    kind: &command.kind,
                    body: &command.body,
                    serial_key: command.serial_key(),
                    deadline_at: command.deadline_at,
                },
            )
            .await
        })
    }
    pub fn reprepare_measurement<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        origin: &'a CapturedOperation,
        until: Option<Time>,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<PreparedMeasurement, EngineError>> {
        Box::pin(async move {
            let proposed = NewCommand {
                content_transit: command.content_transit,
                native_target: None,
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: None,
                deadline_at: command.deadline.ok_or_else(closed)?.get(),
            };
            if command.key != orch_link_contract::commands::SerialKey::None {
                return Err(closed());
            }
            let callers = self
                .current(conn, clock, &proposed)
                .await?
                .ok_or_else(closed)?;
            let (origin, until) = measurement::preparation::retain_current_callers(
                conn, clock, origin, until, &callers,
            )
            .await?;
            Ok((self.authority(), origin, until))
        })
    }
}

pub(crate) fn observe(
    engine: &Engine,
    mut command: Outgoing,
) -> BoxFuture<'_, Result<Outgoing, EngineError>> {
    Box::pin(async move {
        let Some(source) = active() else {
            return Ok(command);
        };
        command.deadline.get_or_insert(Time::new(
            engine.inner.clock.now()
                + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
        ));
        let target = shared::Target::Measurement {
            job: None,
            machine: command.machine,
            kind: command.kind,
            identity: command_identity::hash(&command.body)?,
            phase: None,
        };
        shared::Preparation::new(source)
            .command(engine, target, command)
            .await
    })
}
