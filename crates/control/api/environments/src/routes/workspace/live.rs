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

use super::problems::Doing;

use super::problems::Failure;

use crate::dispatch::DispatchError;

use crate::dispatch::RunError;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::routes::resources::view::is_online;

use orch_api_contract::common::ErrorCode;

use orch_db::Conn;

use orch_db::queries::environments::hosting as standing;

use orch_db::queries::environments::workspace::EnvironmentRow;

use orch_db::queries::resources as machines;

use orch_link::commands::CommandFailure;

use orch_types::Id;

use orch_engine::commands::Outgoing;
use orch_link_contract::commands::{CommandBody, SerialKey};

/// Bind every immediate human file command to its original HTTP identity and current native scope.
pub async fn issue(ctx: &Ctx, mut command: Outgoing) -> Result<crate::dispatch::Issued, Failure> {
    if matches!(
        ctx.user().map(|user| &user.via),
        Ok(crate::middleware::authentication::Via::OwnerRead { .. })
    ) {
        if !orch_engine::operation_scope::file_commands::FileCommand::root_service(
            command.kind.as_str(),
            &command.body,
        )
        .map_err(|error| Failure::Other(error.to_string()))?
        {
            return Err(Failure::Current(Box::new(ApiError::new(
                ErrorCode::FileReachDenied,
                "this reader is confined to the environment roots",
            ))));
        }
        return ctx
            .app
            .link
            .dispatcher()
            .issue_stored(crate::dispatch::StoredCommand {
                native_target: command.native_target,
                id: Some(command.id),
                machine: command.machine,
                kind: command.kind.as_str().to_owned(),
                key: command.key.wire(),
                deadline: command.deadline,
                content_transit: command.content_transit,
                body: command.body,
                authority: None,
            })
            .await
            .map_err(|error| command_failure(RunError::Dispatch(error)));
    }
    async {
        let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
        let source = orch_engine::operation_scope::file_commands::FileCommand::capture(
            &mut tx,
            guard.principal().caller(),
            ctx.route,
            &command,
        )
        .await
        .map_err(ApiError::engine)?;
        let raw = crate::key_operations::native::http_origin::capture_operation(
            ctx,
            &mut tx,
            source.origin(),
        )
        .await?;
        command.deadline.get_or_insert_with(|| {
            orch_types::Time::new(
                ctx.app.clock.now()
                    + chrono::Duration::seconds(i64::from(command.kind.deadline_secs())),
            )
        });
        let authority = crate::key_operations::native::http_origin::bind_source(
            &raw,
            &command,
            orch_engine::operation_scope::http_command_source::CommandEvidence::FilePath(Box::new(
                source,
            )),
        )?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        guard.check_deadline(ctx)?;
        ctx.app
            .link
            .dispatcher()
            .issue_stored_before(
                crate::dispatch::StoredCommand {
                    native_target: command.native_target,
                    id: Some(command.id),
                    machine: command.machine,
                    kind: command.kind.as_str().to_owned(),
                    key: command.key.wire(),
                    deadline: command.deadline,
                    content_transit: command.content_transit,
                    body: command.body,
                    authority: Some(authority),
                },
                Box::new(move || {
                    guard.before_effect(ctx).map_err(|error| {
                        Box::new(error) as Box<dyn std::error::Error + Send + Sync>
                    })
                }),
            )
            .await
            .map_err(|error| match error {
                DispatchError::BeforeEffect(error) => ApiError::internal(error),
                other => command_failure(RunError::Dispatch(other)).problem(Doing::Reading),
            })
    }
    .await
    .map_err(|error| Failure::Current(Box::new(error)))
}

/// Internal work keeps its existing root producer; human file commands retain their current actor.
pub async fn run<B: CommandBody>(
    ctx: &Ctx,
    machine: Id,
    key: SerialKey,
    body: &B,
) -> Result<B::Result, Failure> {
    if !orch_engine::operation_scope::file_commands::FileCommand::applicable(B::KIND.as_str())
        || matches!(
            ctx.user().map(|user| &user.via),
            Ok(crate::middleware::authentication::Via::OwnerRead { .. })
        )
    {
        return orch_api_base::routes::workspace::live::run(ctx, machine, key, body).await;
    }
    let command = Outgoing::new(orch_types::new_id(), machine, key, body)
        .map_err(|error| Failure::Other(error.to_string()))?;
    let id = command.id;
    issue(ctx, command).await?;
    ctx.app
        .link
        .dispatcher()
        .wait_for::<B>(id)
        .await
        .map_err(command_failure)
}
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
orch_api_base::__api_bind_0a904728c5!();

/// The answer to a request that needs the environment to be asked while it cannot be.
#[must_use]
pub fn offline() -> ApiError {
    ApiError::new(
        ErrorCode::SourceOffline,
        "the environment is offline; only what was kept of it can be read",
    )
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
orch_api_base::__api_bind_30aad6c07a!();

/// Finds a shared attempt for this file version. A completed fetch can be reused while its
/// answer still names available content. Concurrent readers can also join a pending attempt.
pub async fn id_to_ask(
    dispatcher: &crate::dispatch::Dispatcher,
    make: impl Fn(u32) -> Id,
) -> ApiResult<Id> {
    let usable =
        async |attempt| {
            let row = dispatcher
                .status(make(attempt))
                .await
                .map_err(ApiError::internal)?;
            Ok::<_, ApiError>(row.is_none_or(|row| {
                !row.is_over() || (row.state == SUCCEEDED && row.result.is_some())
            }))
        };
    if usable(0).await? {
        return Ok(make(0));
    }
    // Attempts occupy successive slots. Locate the end of the spent prefix in logarithmic
    // reads so repeatedly opening one unchanged file after eviction stays inexpensive.
    let (mut spent, mut next) = (0u32, 1u32);
    while !usable(next).await? {
        if next == u32::MAX {
            return Ok(orch_types::new_id());
        }
        spent = next;
        next = next.saturating_mul(2);
    }
    while next - spent > 1 {
        let mid = spent + (next - spent) / 2;
        if usable(mid).await? {
            next = mid;
        } else {
            spent = mid;
        }
    }
    Ok(make(next))
}

/// The word a command's row keeps once the machine answered it.
const SUCCEEDED: &str = "succeeded";

/// The machine that answers for the environment, which stands attached to it; or the refusal that the
/// source is offline.
pub async fn require_live(ctx: &Ctx, conn: &mut Conn, env: &EnvironmentRow) -> ApiResult<Id> {
    let machine = machine_of(ctx, conn, env).await?.ok_or_else(offline)?;
    Ok(machine)
}
orch_api_base::__api_bind_190ad4e921!();

/// The failure of asking a machine about files, as the problem for `doing`.
pub fn refused(failure: Failure, doing: Doing) -> ApiError {
    failure.problem(doing)
}
orch_api_base::__api_bind_c89aa3c2c0!();
