//! The native fork's startup and history commands retain one exact original source.
use super::*;
use crate::{commands::CommandAuthority, harnesses::preparation as startup};
use futures::future::BoxFuture;
use orch_authz::{Caller, CapturedOperation};
use orch_db::Conn;
use orch_types::{Clock, Hash};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
enum Purpose {
    Harness(HarnessKey),
    Provider { provider: Id, machine: Id },
    History { slot: String },
    Files { selected: files::Copy },
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Authority {
    source: operation::Preparation,
    purpose: Purpose,
    id: Id,
    machine: Id,
    kind: orch_link_contract::commands::CommandKind,
    #[serde(with = "crate::commands::RetainedKey")]
    serial: SerialKey,
    body: Hash,
    deadline: Option<Time>,
}
impl std::fmt::Debug for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ForkCommand([private])")
    }
}
fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(crate::operation_scope::continuing::Error::Refused))
}
fn stop(error: Stop) -> EngineError {
    match error {
        Stop::Failed(error) if error.code == INTERNAL => EngineError::unreadable(error.message),
        _ => refused(),
    }
}
impl Authority {
    fn capture(
        source: operation::Preparation,
        purpose: Purpose,
        command: &Outgoing,
    ) -> Result<Self, EngineError> {
        if command.authority.is_some() {
            return Err(refused());
        }
        Ok(Self {
            source,
            purpose,
            id: command.id,
            machine: command.machine,
            kind: command.kind,
            serial: command.key.clone(),
            body: crate::jobs::command_identity::hash(&command.body)?,
            deadline: command.deadline,
        })
    }
    pub(crate) fn command_kind(&self) -> &'static str {
        self.kind.as_str()
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        self.source.lock_source(conn).await?;
        if let Purpose::Files { selected } = &self.purpose {
            selected.lock(conn).await?;
        }
        Ok(())
    }
    async fn check_purpose(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
        body: &Json,
    ) -> Result<operation::Checkpoint, EngineError> {
        let checkpoint = match self.purpose {
            Purpose::Harness(key) => {
                self.source
                    .current_harness(conn, clock, access, key)
                    .await?
            }
            Purpose::Provider { provider, machine } => {
                self.source
                    .current_provider(conn, clock, access, provider, Some(machine))
                    .await?
            }
            Purpose::History { .. } | Purpose::Files { .. } => {
                self.source.current(conn, clock, access).await?
            }
        };
        let saved = match &self.purpose {
            Purpose::History { slot } => {
                let row = orch_db::queries::jobs::job(conn, self.source.job())
                    .await?
                    .ok_or_else(refused)?;
                let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
                let target: source::Target = serde_json::from_value(
                    row.plan
                        .get("history_target")
                        .cloned()
                        .ok_or_else(refused)?,
                )?;
                source::check_target(conn, &recipe, &target)
                    .await
                    .map_err(stop)?;
                Some(serde_json::from_value::<Outgoing>(
                    row.plan
                        .get("history_wire")
                        .and_then(|wire| wire.get(slot))
                        .cloned()
                        .ok_or_else(refused)?,
                )?)
            }
            Purpose::Files { selected } => {
                let row = orch_db::queries::jobs::job(conn, self.source.job())
                    .await?
                    .ok_or_else(refused)?;
                let recipe: Recipe = serde_json::from_value(row.plan.clone())?;
                selected
                    .current(conn, &row, &recipe, checkpoint.principal(), clock)
                    .await?;
                Some(serde_json::from_value::<Outgoing>(
                    row.plan
                        .get("fork_native_wire")
                        .and_then(|wire| wire.get(self.id.to_string()))
                        .cloned()
                        .ok_or_else(refused)?,
                )?)
            }
            _ => None,
        };
        if let Some(saved) = saved
            && (saved.id != self.id
                || saved.machine != self.machine
                || saved.kind != self.kind
                || saved.key != self.serial
                || saved.body != *body
                || saved.deadline != self.deadline
                || saved.authority != Some(CommandAuthority::ForkNative(Box::new(self.clone()))))
        {
            return Err(refused());
        }
        self.source.retain_on(conn, clock, &checkpoint).await?;
        checkpoint.check(clock)?;
        Ok(checkpoint)
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        if command.id != self.id
            || command.machine_id != self.machine
            || command.kind != self.kind.as_str()
            || command.serial_key != self.serial.wire().as_deref()
            || crate::jobs::command_identity::hash(command.body)? != self.body
            || self
                .deadline
                .is_some_and(|until| until.get() != command.deadline_at)
            || command.deadline_at <= clock.now()
        {
            return Ok(None);
        }
        if let Some(actual) = orch_db::queries::link::command(conn, self.id).await? {
            if actual.deadline_at != command.deadline_at || actual.body != *command.body {
                return Ok(None);
            }
        } else if self.deadline.is_none()
            && command.deadline_at
                > clock.now() + chrono::Duration::seconds(i64::from(self.kind.deadline_secs()))
        {
            return Ok(None);
        }
        let checkpoint = match self.check_purpose(conn, clock, access, command.body).await {
            Ok(checkpoint) => checkpoint,
            Err(EngineError::BeforeEffect(_)) => return Ok(None),
            Err(error) => return Err(error),
        };
        Ok(Some(vec![(
            Some(checkpoint.caller().clone()),
            Some(
                checkpoint
                    .until()
                    .map_or(Time::new(command.deadline_at), |until| {
                        until.min(Time::new(command.deadline_at))
                    }),
            ),
        )]))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &orch_db::queries::link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        let serial = command.serial_key().map(str::to_owned);
        self.current_issue_callers(
            conn,
            &orch_db::queries::link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: serial.as_deref(),
                deadline_at: command.deadline_at,
            },
            clock,
            access,
        )
        .await
    }
}
#[derive(Clone)]
struct Startup {
    source: operation::Preparation,
    access: Arc<dyn crate::access::ResourceAccess>,
}
impl Engine {
    pub(crate) fn fork_startup(
        &self,
        row: &JobRow,
    ) -> Result<Option<startup::Preparation>, EngineError> {
        Ok(operation::Preparation::from_job(row)?.map(|source| {
            startup::Preparation::new(Startup {
                source,
                access: self.inner.access.clone(),
            })
        }))
    }
}
impl startup::Source for Startup {
    fn origin(&self) -> &CapturedOperation {
        self.source.origin().captured()
    }
    fn until(&self) -> Option<Time> {
        self.source.origin().until()
    }
    fn checkpoint<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        target: startup::Target,
    ) -> BoxFuture<'a, Result<startup::Checkpoint, EngineError>> {
        Box::pin(async move {
            let checkpoint = match target {
                startup::Target::Harness(key) => {
                    self.source
                        .current_harness(conn, clock, self.access.as_ref(), key)
                        .await?
                }
                startup::Target::ProviderSelection { provider } => {
                    self.source
                        .current_provider(conn, clock, self.access.as_ref(), provider, None)
                        .await?
                }
                startup::Target::Provider { provider, machine } => {
                    self.source
                        .current_provider(
                            conn,
                            clock,
                            self.access.as_ref(),
                            provider,
                            Some(machine),
                        )
                        .await?
                }
                _ => return Err(refused()),
            };
            self.source.retain_on(conn, clock, &checkpoint).await?;
            Ok(startup::Checkpoint::from_check(
                checkpoint.caller().clone(),
                checkpoint.until(),
                Box::new(move |clock| checkpoint.check(clock)),
            ))
        })
    }
    fn accepted(
        &self,
        origin: CapturedOperation,
        until: Option<Time>,
    ) -> Result<Arc<dyn startup::Source>, EngineError> {
        Ok(Arc::new(Self {
            source: self.source.accepted_snapshot(origin, until)?,
            access: self.access.clone(),
        }))
    }
    fn command<'a>(
        &'a self,
        _conn: &'a mut Conn,
        _clock: &'a dyn Clock,
        target: startup::Target,
        command: &'a Outgoing,
    ) -> BoxFuture<'a, Result<CommandAuthority, EngineError>> {
        Box::pin(async move {
            let purpose = match target {
                startup::Target::Harness(key) if command.machine == key.machine => {
                    let expected =
                        if command.kind == orch_link_contract::commands::CommandKind::AgentStart {
                            SerialKey::Harness {
                                provider_id: key.provider,
                                agent_id: key.agent,
                            }
                        } else {
                            SerialKey::Runtime(key.provider)
                        };
                    if command.key != expected {
                        return Err(refused());
                    }
                    Purpose::Harness(key)
                }
                startup::Target::Provider { provider, machine }
                    if command.machine == machine
                        && command.key == SerialKey::Provider(provider) =>
                {
                    Purpose::Provider { provider, machine }
                }
                _ => return Err(refused()),
            };
            Ok(CommandAuthority::ForkNative(Box::new(Authority::capture(
                self.source.clone(),
                purpose,
                command,
            )?)))
        })
    }
}
/// The existing native history writer freezes provenance before its first queue insertion.
pub(super) fn history(
    row: &JobRow,
    slot: &str,
    command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let Some(source) = operation::Preparation::from_job(row)? else {
        return Ok(command);
    };
    if let Some(CommandAuthority::ForkNative(authority)) = &command.authority {
        if authority.source.job() != row.id
            || authority.id != command.id
            || authority.body != crate::jobs::command_identity::hash(&command.body)?
            || !matches!(&authority.purpose,Purpose::History {slot:actual} if actual==slot)
            || authority.serial != command.key
            || authority.deadline != command.deadline
        {
            return Err(refused());
        }
        return Ok(command);
    }
    let authority = Authority::capture(source, Purpose::History { slot: slot.into() }, &command)?;
    Ok(command.with_authority(CommandAuthority::ForkNative(Box::new(authority))))
}

/// The native job runner freezes file command provenance after its original work report.
/// Accepted receipts use this exact private frame without authorizing a new command.
pub(crate) async fn remember_forward(job: &Job, command: &mut Outgoing) -> Result<(), Stop> {
    let row = job.row().await?;
    if row.kind != KIND {
        return Ok(());
    }
    let Some(source) = operation::Preparation::from_job(&row)? else {
        return Ok(());
    };
    if let Some(CommandAuthority::ForkNative(authority)) = &command.authority {
        if authority.source.job() != row.id {
            return Err(refused().into());
        }
        return Ok(());
    }
    if command.authority.is_some() {
        return Err(refused().into());
    }
    let slot = command.id.to_string();
    let saved = row
        .plan
        .get("fork_native_wire")
        .and_then(|wire| wire.get(&slot))
        .cloned()
        .map(serde_json::from_value::<Outgoing>)
        .transpose()
        .map_err(EngineError::from)?;
    if let Some(saved) = &saved {
        let mut expected = saved.body.clone();
        let mut actual = command.body.clone();
        let old_work = expected
            .as_object_mut()
            .and_then(|body| body.remove("work"));
        let new_work = actual.as_object_mut().and_then(|body| body.remove("work"));
        if saved.id != command.id
            || saved.machine != command.machine
            || saved.kind != command.kind
            || saved.key != command.key
            || expected != actual
        {
            return Err(refused().into());
        }
        match (old_work, new_work) {
            (Some(old), Some(new)) if !old.is_null() && !new.is_null() => {
                let old: orch_link_contract::commands::WorkReport =
                    serde_json::from_value(old).map_err(EngineError::from)?;
                let mut new: orch_link_contract::commands::WorkReport =
                    serde_json::from_value(new).map_err(EngineError::from)?;
                new.timeout_secs = old.timeout_secs;
                if old != new {
                    return Err(refused().into());
                }
            }
            (old, new) if old == new => {}
            _ => return Err(refused().into()),
        }
        *command = saved.clone();
    }
    let accepted = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(|error| EngineError::unreadable(error.to_string()))?
        .is_some_and(|info| {
            matches!(
                info.stage,
                crate::commands::Stage::Accepted | crate::commands::Stage::Over
            )
        });
    if accepted {
        if saved.is_none() {
            return Err(refused().into());
        }
        return Ok(());
    }
    let clock = job.engine().inner.clock.as_ref();
    let mut tx = job.engine().db().begin().await?;
    let checkpoint = source
        .current(&mut tx, clock, job.engine().inner.access.as_ref())
        .await?;
    let current = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    let recipe: Recipe = serde_json::from_value(current.plan.clone()).map_err(EngineError::from)?;
    let selected = files::Copy::selected(&mut tx, &current, &recipe, clock).await?;
    selected
        .current(&mut tx, &current, &recipe, checkpoint.principal(), clock)
        .await?;
    if !selected.covers(&recipe, current.id, command)? {
        return Err(refused().into());
    }
    let retained = source.retain_on(&mut tx, clock, &checkpoint).await?;
    if let Some(saved) = saved {
        let Some(CommandAuthority::ForkNative(authority)) = &saved.authority else {
            return Err(refused().into());
        };
        if !matches!(&authority.purpose,Purpose::Files { selected:old } if old==&selected)
            || !authority.source.preserves(&retained)
        {
            return Err(refused().into());
        }
    } else {
        let authority = Authority::capture(retained, Purpose::Files { selected }, command)?;
        command.authority = Some(CommandAuthority::ForkNative(Box::new(authority)));
        let mut wire = current
            .plan
            .get("fork_native_wire")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let wire = wire.as_object_mut().ok_or_else(refused)?;
        if wire.contains_key(&slot) {
            return Err(refused().into());
        }
        wire.insert(
            slot,
            serde_json::to_value(&*command).map_err(EngineError::from)?,
        );
        checkpoint.check(clock)?;
        orch_db::queries::jobs::merge_plan(
            &mut tx,
            current.id,
            &json!({"fork_native_wire":wire}),
            clock.now(),
        )
        .await?;
    }
    checkpoint.check(clock)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(())
}
