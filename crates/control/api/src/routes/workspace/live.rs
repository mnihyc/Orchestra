//! Whether an environment can be asked now, asking it, and asking its machine for its measurements once for each session.
//!
//! An environment is asked when it stands attached in the session its machine has: the engine attaches
//! an environment in each session of its machine (a directory on a person's machine by the schedule of
//! environments, one made from a template by the job that makes it) and says so in the environment's
//! row, and nothing here sends `env.attach`. Where it does not stand attached, or its machine has no
//! session, what is asked of it is answered from what the system kept, or refused as offline. A machine
//! that holds an environment reports measurements only while a visible live subscription requests
//! them; the API keeps those samples for six hours. A directory observation may temporarily share
//! native registration with an initial live view; it starts no periodic read or retained HTTP watch.

use super::problems::{Doing, Failure};
use crate::dispatch::{DispatchError, RunError};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use crate::routes::resources::view::is_online;
use orch_api_contract::common::ErrorCode;
use orch_db::Conn;
use orch_db::queries::environments::hosting as standing;
use orch_db::queries::environments::workspace::EnvironmentRow;
use orch_db::queries::resources as machines;
use orch_link::commands::CommandFailure;
use orch_link_contract::commands::{CommandBody, SerialKey};
use orch_types::Id;

/// The machine that answers for the environment now, if there is one: the environment stands attached
/// in the session its machine has (a directory on a person's machine says so in its row; a unit made
/// from a template, by running), the machine's row says it is active, and it has a session.
pub async fn machine_of(ctx: &Ctx, conn: &mut Conn, env: &EnvironmentRow) -> ApiResult<Option<Id>> {
    let Some(id) = env.machine_id else {
        return Ok(None);
    };
    let attached = if orch_db::queries::environment_facts::of(conn, env.id)
        .await?
        .is_some_and(|facts| facts.attaches())
    {
        standing::connected(conn, env.id)
            .await?
            .is_some_and(|directory| directory.attached())
    } else {
        env.status == "running"
    };
    if !attached {
        return Ok(None);
    }
    let row = machines::machine(conn, id)
        .await?
        .filter(machines::MachineRow::is_active);
    Ok(row
        .filter(|row| is_online(&ctx.app, row.id, row.online))
        .map(|row| row.id))
}

/// The answer to a request that needs the environment to be asked while it cannot be.
#[must_use]
pub fn offline() -> ApiError {
    ApiError::new(
        ErrorCode::SourceOffline,
        "the environment is offline; only what was kept of it can be read",
    )
}

/// Runs a command of the catalogue at a machine and waits for its result.
pub async fn run<B: CommandBody>(
    ctx: &Ctx,
    machine: Id,
    key: SerialKey,
    body: &B,
) -> Result<B::Result, Failure> {
    ctx.app
        .link
        .dispatcher()
        .run(machine, &key, body)
        .await
        .map_err(command_failure)
}

/// The same command failure mapping for HTTP reads and their live directory watches.
pub(crate) fn command_failure(error: RunError) -> Failure {
    match error {
        RunError::Command(CommandFailure::Failed(error)) => Failure::Machine(error),
        RunError::Command(CommandFailure::Expired | CommandFailure::Cancelled) => {
            Failure::Away("the machine did not take the command up".to_owned())
        }
        RunError::Command(CommandFailure::Unreadable(why)) => Failure::Other(why),
        RunError::Dispatch(
            DispatchError::UnknownMachine | DispatchError::Revoked | DispatchError::Stopping,
        ) => Failure::Away("the machine cannot be reached".to_owned()),
        RunError::Dispatch(other) => Failure::Other(other.to_string()),
    }
}

/// How many identifiers are tried for one thing that is asked of a machine, one for each time the
/// machine failed it: after more failures than this the next ask has an identifier of no pattern,
/// which no earlier failure covers.
const ASKED_IDS: u32 = 16;

/// The identifier a thing is asked of a machine under: the first of `make(0)`, `make(1)`, and so on
/// whose command has not ended badly. People who ask at the same time share one command, and a
/// command that failed is not waited for again by those who ask after it: they ask anew.
pub async fn id_to_ask(
    dispatcher: &crate::dispatch::Dispatcher,
    make: impl Fn(u32) -> Id,
) -> ApiResult<Id> {
    for attempt in 0..ASKED_IDS {
        let id = make(attempt);
        match dispatcher.status(id).await.map_err(ApiError::internal)? {
            Some(row) if row.is_over() && row.state != SUCCEEDED => {}
            _ => return Ok(id),
        }
    }
    Ok(orch_types::new_id())
}

/// The word a command's row keeps once the machine answered it.
const SUCCEEDED: &str = "succeeded";

/// The machine that answers for the environment, which stands attached to it; or the refusal that the
/// source is offline.
pub async fn require_live(ctx: &Ctx, conn: &mut Conn, env: &EnvironmentRow) -> ApiResult<Id> {
    let machine = machine_of(ctx, conn, env).await?.ok_or_else(offline)?;
    Ok(machine)
}

/// The failure of asking a machine about files, as the problem for `doing`.
pub fn refused(failure: Failure, doing: Doing) -> ApiError {
    failure.problem(doing)
}
