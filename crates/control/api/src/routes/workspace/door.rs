//! How a request gets into an environment's workspace: by the permission of the environment's own page, whichever thread the caller works beside.
//!
//! The files and the shells of an environment are given on its page, for all of it: `files.view`
//! and `files.manage` list, read and write anywhere in the workspace, a thread's directory being
//! where its agent starts and its panel opens, and no fence. The request's declaration names the
//! permission, and the server holds the caller to it on the environment before a handler runs;
//! whoever manages the environment holds it without being named. A thread named in a request gives
//! nothing: it says beside which thread the caller works, which is where a path or a search starts
//! when none is given. A caller who holds everything on a thread and nothing on its environment is
//! refused, with what is missing named on the environment.

use crate::middleware::authorization::{self, VIEW};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::common::ErrorCode;
use orch_authz::{Act, Policy, Principal};
use orch_db::Conn;
use orch_db::queries::environments::workspace::{self as rows, EnvironmentRow};
use orch_types::Id;

/// A caller who may reach into an environment.
#[derive(Debug)]
pub struct Door {
    /// The physical machine's own ceiling, separate from the environment's narrowing.
    pub machine_write: bool,
    /// The environment.
    pub env: EnvironmentRow,
    /// The caller, as the access engine knows them.
    pub principal: Principal,
    /// The thread the caller works beside, when they named one.
    pub thread: Option<Id>,
    /// That thread's directory in the workspace.
    pub directory: Option<String>,
}

impl Door {
    /// Requires the environment's switch for writes from outside to be on.
    pub fn may_write(&self) -> ApiResult<()> {
        if !self.machine_write {
            return Err(ApiError::new(
                ErrorCode::PolicyRemoteWriteDisabled,
                "environment.remote_write: this machine keeps file writes through Orch off",
            ));
        }
        if self.env.remote_write {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::WorkspaceRemoteWriteDisabled,
                "files are not written into this environment from here",
            ))
        }
    }
}

/// The thread `thread` that the caller works beside, in the environment `environment`: where its
/// agent starts. It is a thread that runs there and that the caller may view, and none else: a thread
/// that does not exist for the caller is not found. It gives nothing.
pub async fn beside(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    thread: Id,
) -> ApiResult<String> {
    let place = rows::thread_place(conn, thread)
        .await?
        .filter(|place| place.environment_id == Some(environment) && place.side_of.is_none())
        .ok_or_else(ApiError::not_found)?;
    match authorization::require(conn, principal, thread, VIEW, Act::Use, Policy::default()).await {
        Ok(()) => Ok(place.workspace_dir),
        Err(error) if error.code() == ErrorCode::Internal => Err(error),
        Err(_) => Err(ApiError::not_found()),
    }
}

/// Opens the environment the path names (`{id}`) to the caller, who holds what the request needs
/// there: that was decided before the handler ran. The thread `thread`, when one is named, is the
/// one they work beside.
pub async fn open(ctx: &Ctx, conn: &mut Conn, thread: Option<Id>) -> ApiResult<Door> {
    let environment = ctx.id("id")?;
    let principal = authorization::principal(ctx, conn).await?;
    let env = rows::environment(conn, environment)
        .await?
        .ok_or_else(ApiError::not_found)?;
    if matches!(
        ctx.route.method.as_str(),
        "POST" | "PUT" | "PATCH" | "DELETE"
    ) && let Some(job) =
        orch_db::queries::jobs::changes::environment_change_hold(conn, environment).await?
        && job.kind == orch_db::queries::jobs::kind::WORKSPACE_RESTORE
    {
        return Err(ApiError::new(
            ErrorCode::ConflictExists,
            "recorded files are being restored; remote file writes wait",
        )
        .with("job", orch_engine::locks::job_ref(&job)));
    }
    let directory = match thread {
        Some(thread) => Some(beside(conn, &principal, env.id, thread).await?),
        None => None,
    };
    Ok(Door {
        // A machine's own ceiling refuses only where a machine is known to hold the environment.
        machine_write: !orch_db::queries::environment_facts::refuses(
            conn,
            environment,
            "environment.remote_write",
        )
        .await?,
        env,
        principal,
        thread,
        directory,
    })
}
