//! Canonical native move and Host-parent proposals in the existing durable command channel.

use crate::{
    EngineError,
    commands::{CommandAuthority, Outgoing},
    jobs::{
        self, command_identity,
        runner::{Failure, Job, Stop},
    },
};
use futures::future::BoxFuture;
use orch_authz::{Caller, Principal};
use orch_db::{
    Conn,
    queries::{
        jobs as rows,
        link::{CommandRow, NewCommand},
    },
};
use orch_types::{Clock, Id, Json, Time};
use serde::{Deserialize, Serialize};

type CurrentCallers = Vec<(Option<Caller>, Option<Time>)>;

pub mod environment;
pub mod host_inventory;
pub use environment::retain as retain_environment_execution;

/// One durable lifecycle channel: accepted execution or native preparation.
/// Untagged execution keeps its existing private wire record exactly.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Evidence {
    Execution(Box<Command>),
    Preparation(Preparation),
    Reviewed(Box<planning::Packet>),
    Inventory(Box<host_inventory::Packet>),
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
enum PreparationVersion {
    #[serde(rename = "lifecycle_preparation_v1")]
    One,
}
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preparation {
    version: PreparationVersion,
    native: Box<jobs::environment_move::preparation::CommandEvidence>,
}
impl Evidence {
    pub(crate) fn preparation(
        native: jobs::environment_move::preparation::CommandEvidence,
    ) -> Self {
        Self::Preparation(Preparation {
            version: PreparationVersion::One,
            native: Box::new(native),
        })
    }
    pub fn command_kind(&self) -> &'static str {
        match self {
            Self::Execution(command) => command.command_kind(),
            Self::Preparation(command) => command.native.command_kind(),
            Self::Reviewed(command) => command.command_kind(),
            Self::Inventory(command) => command.command_kind(),
        }
    }
    pub async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        match self {
            Self::Execution(command) => command.lock_source(conn).await,
            Self::Preparation(command) => command.native.lock_source(conn).await,
            Self::Reviewed(command) => command.lock_source(conn).await,
            Self::Inventory(command) => command.lock_source(conn).await,
        }
    }
    pub async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Inventory(source) => source.current_issue_callers(conn, command, clock).await,
            Self::Reviewed(source) => {
                source
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::Execution(source) => {
                source
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
            Self::Preparation(source) => {
                source
                    .native
                    .current_issue_callers(conn, command, clock, access)
                    .await
            }
        }
    }
    pub async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<Vec<(Option<Caller>, Option<Time>)>>, EngineError> {
        match self {
            Self::Inventory(source) => source.current_callers(conn, command, clock).await,
            Self::Reviewed(source) => source.current_callers(conn, command, clock, access).await,
            Self::Execution(source) => source.current_callers(conn, command, clock, access).await,
            Self::Preparation(source) => {
                source
                    .native
                    .current_callers(conn, command, clock, access)
                    .await
            }
        }
    }
    pub(crate) async fn reprepare_measurement(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
        origin: &orch_authz::CapturedOperation,
        until: Option<Time>,
        command: &Outgoing,
    ) -> Result<
        (
            CommandAuthority,
            orch_authz::CapturedOperation,
            Option<Time>,
        ),
        EngineError,
    > {
        match self {
            // Preparation provenance cannot be converted to a captured business operation.
            Self::Inventory(_) => Err(EngineError::BeforeEffect(Box::new(
                crate::operation_scope::continuing::Error::Refused,
            ))),
            Self::Reviewed(source) => {
                source
                    .reprepare_measurement(conn, clock, access, origin, until, command)
                    .await
            }
            Self::Preparation(source) => {
                source
                    .native
                    .reprepare_measurement(conn, clock, access, origin, until, command)
                    .await
            }
            Self::Execution(source) => {
                // Preserve the old channel's current-proof path for genuine owned execution IO.
                // It still requires the real job, current recipe and exact journaled proposal.
                let serial = command.key.wire();
                let proposed = NewCommand {
                    id: command.id,
                    machine_id: command.machine,
                    kind: command.kind.as_str(),
                    body: &command.body,
                    serial_key: serial.as_deref(),
                    deadline_at: command
                        .deadline
                        .ok_or_else(|| {
                            EngineError::unreadable(
                                "The native lifecycle observation lost its allowance.",
                            )
                        })?
                        .get(),
                };
                let callers = source
                    .current_issue_callers(conn, &proposed, clock, access)
                    .await?
                    .ok_or_else(|| {
                        EngineError::BeforeEffect(Box::new(
                            crate::operation_scope::continuing::Error::Refused,
                        ))
                    })?;
                let (origin, until) = jobs::measurement::preparation::retain_current_callers(
                    conn, clock, origin, until, &callers,
                )
                .await?;
                Ok((
                    CommandAuthority::lifecycle((**source).clone()),
                    origin,
                    until,
                ))
            }
        }
    }
}

pub mod planning;

pub(crate) const JOURNAL: &str = "lifecycle_commands";
fn closed(error: &EngineError) -> bool {
    matches!(error,EngineError::BeforeEffect(error) if error.is::<jobs::environment_move::authority::Ended>() || error.is::<jobs::host_delete::authority::Ended>() || error.is::<jobs::environment_move::preparation::execution::Ended>() || error.is::<crate::operation_scope::continuing::Error>())
}
fn failed(error: impl std::fmt::Display) -> Stop {
    Failure::new("access.denied", error.to_string()).into()
}
/// Closed native owners; no client-supplied operation label or permission policy.
#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Owner {
    NodeMove,
    HostDelete,
    EnvironmentLifecycle,
}
fn owner(row: &rows::JobRow) -> Option<Owner> {
    if environment::present(row) {
        Some(Owner::EnvironmentLifecycle)
    } else if jobs::environment_move::authority::present(row) && row.kind == "environment_move" {
        Some(Owner::NodeMove)
    } else if jobs::host_delete::authority::present(row) && row.kind == jobs::host_delete::KIND {
        Some(Owner::HostDelete)
    } else {
        None
    }
}
pub(crate) fn applies(row: &rows::JobRow) -> bool {
    owner(row).is_some()
}
async fn principal(
    conn: &mut Conn,
    row: &rows::JobRow,
    owner: Owner,
    clock: &dyn Clock,
    access: &dyn crate::access::ResourceAccess,
) -> Result<Principal, EngineError> {
    match owner {
        Owner::NodeMove => jobs::environment_move::authority::principal(conn, row, clock).await,
        Owner::HostDelete => jobs::host_delete::authority::principal(conn, row, clock).await,
        Owner::EnvironmentLifecycle => Ok(environment::current(conn, row, clock, access, None)
            .await?
            .principal),
    }
}
/// One actual producer's canonical proposal, sealed only at the native runner's final handoff.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Command {
    job: Id,
    actor: Id,
    owner: Owner,
    frame: Json,
    previous: Option<Box<CommandAuthority>>,
    #[serde(default)]
    cleanup: bool,
}
impl std::fmt::Debug for Command {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LifecycleCommand([private])")
    }
}
struct Wire<'a> {
    id: Id,
    machine: Id,
    key: Option<&'a str>,
    kind: &'a str,
    body: &'a Json,
}
impl Command {
    /// Actual native kind retained by the proposal, never a key claim.
    pub fn command_kind(&self) -> &'static str {
        self.frame
            .get("kind")
            .and_then(Json::as_str)
            .and_then(|kind| {
                kind.parse::<orch_link_contract::commands::CommandKind>()
                    .ok()
            })
            .map_or("invalid", |kind| kind.as_str())
    }
    /// Original actor fence precedes the command-row fence.
    pub fn lock_source<'a>(&'a self, conn: &'a mut Conn) -> BoxFuture<'a, Result<(), EngineError>> {
        Box::pin(async move {
            orch_db::queries::transaction_guard::lock_native_actor(conn, self.actor).await?;
            if self.owner == Owner::EnvironmentLifecycle {
                let row = rows::job(conn, self.job).await?.ok_or_else(|| {
                    EngineError::BeforeEffect(Box::new(
                        crate::operation_scope::continuing::Error::Refused,
                    ))
                })?;
                environment::lock_source(conn, &row).await?;
            }
            if let Some(previous) = &self.previous {
                Box::pin(previous.lock_native_source(conn)).await?;
            }
            Ok(())
        })
    }
    async fn current(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
        wire: Wire<'_>,
    ) -> Result<Option<(Option<Caller>, Option<Time>)>, EngineError> {
        let Wire {
            id,
            machine,
            key,
            kind,
            body,
        } = wire;
        if command_identity::canonical(id, machine, key, kind, body)? != self.frame {
            return Ok(None);
        }
        let Some(initial) = rows::job(conn, self.job).await? else {
            return Ok(None);
        };
        if owner(&initial) != Some(self.owner) || initial.created_by != Some(self.actor) {
            return Ok(None);
        }
        if self.cleanup {
            let Some(row) = rows::lock(conn, self.job).await? else {
                return Ok(None);
            };
            if !matches!(self.owner, Owner::NodeMove | Owner::EnvironmentLifecycle)
                || owner(&row) != Some(self.owner)
                || row.created_by != Some(self.actor)
                || !row.is_open()
                || !command_identity::recorded_matches(&row, JOURNAL, id, machine, key, kind, body)?
                || !cleanup_matches(
                    conn,
                    &row,
                    self.owner,
                    Wire {
                        id,
                        machine,
                        key,
                        kind,
                        body,
                    },
                )
                .await?
            {
                return Ok(None);
            }
            return Ok(Some((None, None)));
        }
        match principal(conn, &initial, self.owner, clock, access).await {
            Ok(_) => {}
            Err(error) if closed(&error) => return Ok(None),
            Err(error) => return Err(error),
        }
        let Some(row) = rows::lock(conn, self.job).await? else {
            return Ok(None);
        };
        if owner(&row) != Some(self.owner)
            || row.created_by != Some(self.actor)
            || !row.is_open()
            || row.cancel_requested()
            || jobs::runner::abort_reason(&row)?.is_some()
            || !command_identity::recorded_matches(&row, JOURNAL, id, machine, key, kind, body)?
        {
            return Ok(None);
        }
        if !jobs::recreate::batch_authority::native_child_proposal(
            conn, &row, id, machine, key, kind, body,
        )
        .await?
        {
            return Ok(None);
        }
        let principal = match principal(conn, &row, self.owner, clock, access).await {
            Ok(principal) => principal,
            Err(error) if closed(&error) => return Ok(None),
            Err(error) => return Err(error),
        };
        let narrowed = rows::job(conn, self.job)
            .await?
            .ok_or_else(|| EngineError::unreadable("the native source disappeared"))?;
        let until = deadline(&narrowed, self.owner)?;
        Ok(Some((Some(principal.caller().clone()), until)))
    }
    /// Current full original/raw minimum before the first durable command insertion.
    pub fn current_issue_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a NewCommand<'_>,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            let Some((caller, until)) = self
                .current(
                    conn,
                    clock,
                    access,
                    Wire {
                        id: command.id,
                        machine: command.machine_id,
                        key: command.serial_key,
                        kind: command.kind,
                        body: command.body,
                    },
                )
                .await?
            else {
                return Ok(None);
            };
            let mut callers = vec![(caller, until)];
            if let Some(previous) = &self.previous {
                let Some(inner) =
                    Box::pin(previous.current_issue_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers.extend(inner);
            }
            Ok(Some(callers))
        })
    }
    /// The exact same retained proposal is checked again after every unaccepted send wait.
    pub fn current_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        command: &'a CommandRow,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            let Some((caller, until)) = self
                .current(
                    conn,
                    clock,
                    access,
                    Wire {
                        id: command.id,
                        machine: command.machine_id,
                        key: command.serial_key(),
                        kind: &command.kind,
                        body: &command.body,
                    },
                )
                .await?
            else {
                return Ok(None);
            };
            let mut callers = vec![(caller, until)];
            if let Some(previous) = &self.previous {
                let Some(inner) =
                    Box::pin(previous.current_callers(conn, command, clock, access)).await?
                else {
                    return Ok(None);
                };
                callers.extend(inner);
            }
            Ok(Some(callers))
        })
    }
}
async fn cleanup_matches(
    conn: &mut Conn,
    row: &rows::JobRow,
    owner: Owner,
    wire: Wire<'_>,
) -> Result<bool, EngineError> {
    match owner {
        Owner::NodeMove => {
            jobs::environment_move::authority::cleanup_matches(
                conn,
                row,
                wire.machine,
                wire.key,
                wire.kind,
                wire.body,
            )
            .await
        }
        Owner::EnvironmentLifecycle => {
            environment::cleanup_matches(conn, row, wire.machine, wire.key, wire.kind, wire.body)
                .await
        }
        Owner::HostDelete => Ok(false),
    }
}
fn deadline(row: &rows::JobRow, owner: Owner) -> Result<Option<Time>, EngineError> {
    match owner {
        Owner::NodeMove => jobs::environment_move::authority::deadline(row),
        Owner::HostDelete => jobs::host_delete::authority::deadline(row),
        Owner::EnvironmentLifecycle => environment::deadline(row),
    }
}
/// Decorate the final native proposal after WorkReport/retry-id construction, through the one runner.
/// Existing accepted receipts and native compensation retain their independent reconciliation path.
pub(crate) async fn decorate(
    job: &Job,
    command: &mut Outgoing,
    respect_cancel: bool,
) -> Result<(), Stop> {
    if !respect_cancel {
        return Ok(());
    }
    let initial = job.row().await?;
    let Some(native) = owner(&initial) else {
        return Ok(());
    };
    let prior = job
        .engine()
        .inner
        .commands
        .status(command.id)
        .await
        .map_err(failed)?;
    if prior.as_ref().is_some_and(|info| {
        matches!(
            info.stage,
            crate::commands::Stage::Accepted | crate::commands::Stage::Over
        )
    }) {
        return Ok(());
    }
    let mut tx = job.engine().db().begin().await?;
    let cleanup = matches!(native, Owner::NodeMove | Owner::EnvironmentLifecycle)
        && initial
            .plan
            .get("committed")
            .or_else(|| initial.plan.get("provision_committed"))
            .and_then(Json::as_bool)
            == Some(true);
    if cleanup {
        let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
        if !cleanup_matches(
            &mut tx,
            &row,
            native,
            Wire {
                id: command.id,
                machine: command.machine,
                key: command.key.wire().as_deref(),
                kind: command.kind.as_str(),
                body: &command.body,
            },
        )
        .await
        .map_err(failed)?
        {
            return Err(failed(
                "the proposal is not the committed move's owned cleanup",
            ));
        }
        command_identity::remember(&mut tx, &row, JOURNAL, command, Time::new(job.now()))
            .await
            .map_err(failed)?;
        let evidence = Command {
            job: row.id,
            actor: row
                .created_by
                .ok_or_else(|| failed("the cleanup actor disappeared"))?,
            owner: native,
            frame: command_identity::canonical(
                command.id,
                command.machine,
                command.key.wire().as_deref(),
                command.kind.as_str(),
                &command.body,
            )
            .map_err(failed)?,
            previous: command.authority.clone().map(Box::new),
            cleanup: true,
        };
        tx.commit().await.map_err(orch_db::DbError::from)?;
        command.authority = Some(CommandAuthority::lifecycle(evidence));
        return Ok(());
    }
    let current = principal(
        &mut tx,
        &initial,
        native,
        job.engine().inner.clock.as_ref(),
        job.engine().inner.access.as_ref(),
    )
    .await
    .map_err(failed)?;
    let row = job.lock_owned(&mut tx).await?.ok_or(Stop::Lost)?;
    if owner(&row) != Some(native)
        || row.cancel_requested()
        || jobs::runner::abort_reason(&row)?.is_some()
    {
        return Err(failed("the native lifecycle direction changed"));
    }
    let current = principal(
        &mut tx,
        &row,
        native,
        job.engine().inner.clock.as_ref(),
        job.engine().inner.access.as_ref(),
    )
    .await
    .map_err(failed)?
    .caller()
    .intersect_restrictions(current.caller())
    .map_err(failed)?;
    let narrowed = rows::job(&mut tx, row.id).await?.ok_or(Stop::Lost)?;
    if deadline(&narrowed, native)
        .map_err(failed)?
        .is_some_and(|until| job.engine().inner.clock.time() >= until)
        || !current.key_deadline_allows(job.engine().inner.clock.as_ref())
    {
        return Err(failed("the original lifecycle key expired"));
    }
    if !jobs::recreate::batch_authority::native_child_proposal(
        &mut tx,
        &row,
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
    .await
    .map_err(failed)?
    {
        return Err(failed("the native replacement operands changed"));
    }
    command_identity::remember(&mut tx, &row, JOURNAL, command, Time::new(job.now()))
        .await
        .map_err(failed)?;
    let frame = command_identity::canonical(
        command.id,
        command.machine,
        command.key.wire().as_deref(),
        command.kind.as_str(),
        &command.body,
    )
    .map_err(failed)?;
    let evidence = Command {
        job: row.id,
        actor: current.user(),
        owner: native,
        frame,
        previous: command.authority.clone().map(Box::new),
        cleanup: false,
    };
    if deadline(&narrowed, native)
        .map_err(failed)?
        .is_some_and(|until| job.engine().inner.clock.time() >= until)
        || !current.key_deadline_allows(job.engine().inner.clock.as_ref())
    {
        return Err(failed(
            "the original lifecycle credential expired before proposal commit",
        ));
    }
    tx.commit().await.map_err(orch_db::DbError::from)?;
    command.authority = Some(CommandAuthority::lifecycle(evidence));
    Ok(())
}

#[cfg(test)]
mod tests;
