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

use crate::middleware::authorization::VIEW;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use orch_api_contract::common::ErrorCode;

use orch_authz::Act;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_db::Conn;

use orch_db::queries::environments::workspace as rows;

pub use orch_api_base::routes::workspace::door::{Door, open_with_principal, writing};
use orch_types::Id;

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
orch_api_base::__api_bind_f1c62865f5!();

/// Opens the environment the path names (`{id}`) to the caller, who holds what the request needs
/// there: that was decided before the handler ran. The thread `thread`, when one is named, is the
/// one they work beside.
pub async fn open(ctx: &Ctx, conn: &mut Conn, thread: Option<Id>) -> ApiResult<Door> {
    let environment = ctx.id("id")?;
    let principal = authorization::principal(ctx, conn).await?;
    open_with_principal(ctx, conn, environment, thread, principal).await
}
/// Retained bytes and native answers are checked again after their waits, under the current actor.
pub async fn require_reach(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    minimum: orch_access_contract::permissions::FileReach,
    write: bool,
) -> ApiResult<()> {
    let reach =
        match orch_authz::file_reach::effective(conn, principal, environment, Policy::default())
            .await
        {
            Ok(reach) => reach,
            Err(error) => return Err(authorization::engine_problem(conn, principal, error).await),
        };
    let allowed = if write { reach.manage } else { reach.view };
    if allowed.is_some_and(|reach| reach >= minimum) {
        Ok(())
    } else {
        Err(ApiError::new(
            ErrorCode::FileReachDenied,
            "this target is outside your current file reach",
        ))
    }
}

/// A native fact can be retained after source work, but never beyond the current HTTP reader.
pub async fn finish_read<T>(
    ctx: &Ctx,
    environment: Id,
    minimum: orch_access_contract::permissions::FileReach,
    value: T,
) -> ApiResult<T> {
    if minimum == orch_access_contract::permissions::FileReach::Machine
        && matches!(
            ctx.user()?.via,
            crate::middleware::authentication::Via::OwnerRead { .. }
        )
    {
        return Err(ApiError::new(
            ErrorCode::FileReachDenied,
            "this reader is confined to the environment roots",
        ));
    }
    let (mut tx, guard) = crate::middleware::transaction_guard::begin(ctx).await?;
    require_reach(&mut tx, guard.principal(), environment, minimum, false).await?;
    guard.check_deadline(ctx)?;
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(value)
}

/// Unreported policy cannot certify a smaller source than the admitted native command.
#[must_use]
pub fn minimum(
    facts: Option<&orch_api_contract::workspace::FileSourceFacts>,
    admitted: orch_link_contract::commands::FsAccess,
    write: bool,
) -> orch_access_contract::permissions::FileReach {
    use orch_access_contract::permissions::FileReach;
    match facts.map(|facts| facts.policy.kind) {
        Some(orch_api_contract::workspace::FileSourceKind::Machine) => FileReach::Machine,
        Some(_) => FileReach::Roots,
        None => (if write {
            admitted.manage
        } else {
            admitted.view
        })
        .unwrap_or(FileReach::Roots),
    }
}
