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

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::Ctx;
use orch_api_contract::common::ErrorCode;
use orch_authz::{Policy, Principal};
use orch_db::Conn;
use orch_db::queries::environments::workspace as rows;
use orch_db::queries::environments::workspace::EnvironmentRow;
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

orch_api_base::api_family_callback! {
    bind __api_bind_f1c62865f5, __api_install_f1c62865f5, __api_callback_f1c62865f5;
    slot __ApiCallbackf1c62865f5, __API_CALLBACK_F1C62865F5, __api_set_f1c62865f5;
    base orch_api_base::routes::workspace::door;

    async fn beside<'__api>(arg0: &'__api mut orch_db::Conn, arg1: &'__api orch_authz::Principal, arg2: orch_types::Id, arg3: orch_types::Id) -> orch_api_base::middleware::requests::ApiResult<String>;
}

/// The same native door for a producer that already obtained its current preparation actor.
pub async fn open_with_principal(
    ctx: &Ctx,
    conn: &mut Conn,
    environment: Id,
    thread: Option<Id>,
    principal: Principal,
) -> ApiResult<Door> {
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

/// The same current Files door and remote-write ceilings for raw saves and targeted sessions.
pub async fn writing(
    ctx: &Ctx,
    conn: &mut Conn,
    environment: Id,
    thread: Option<Id>,
    principal: Principal,
) -> ApiResult<(Door, Id)> {
    let door = open_with_principal(ctx, conn, environment, thread, principal).await?;
    authorization::require_resource(
        conn,
        &door.principal,
        orch_access_contract::permissions::ResourceKind::Environment,
        environment,
        orch_access_contract::permissions::Permission::FilesManage,
        Policy::default(),
    )
    .await?;
    door.may_write()?;
    let machine = crate::routes::workspace::live::require_live(ctx, conn, &door.env).await?;
    Ok((door, machine))
}
