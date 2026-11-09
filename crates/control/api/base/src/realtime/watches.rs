//! Who may follow the ephemeral feeds of an environment, decided by the environment's own page: its processes and its load by `terminal.view`, a directory of its workspace by `files.view`, and none of them through a thread; and diagnostics.
//!
//! Each environment feed asks its own permission page and is rechecked on authorization changes
//! and before the earliest end of the caller's grants or roles.
//! The transient coordinator runs its measurements and directory watches; browser and display
//! parts publish their complete lists through the same machinery.

use crate::realtime::subscriptions::Access;

use crate::realtime::subscriptions::principal_of;

use crate::realtime::Fail;

use crate::realtime::Inner;

use orch_access_contract::decisions::Decision;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_authz::Policy;

use orch_authz::resources;

use orch_realtime_contract::feeds::Feed;

use orch_types::Id;

use orch_types::Time;
/// What following `feed` of an environment needs of the environment, and which environment it is:
/// the processes and the load need `terminal.view`, a directory `files.view`. Nothing for any other
/// feed.
#[must_use]
pub(crate) fn needs(feed: &Feed) -> Option<(Id, Permission)> {
    match feed {
        Feed::EnvProcs(environment) | Feed::EnvStats(environment) => {
            Some((*environment, Permission::TerminalView))
        }
        Feed::EnvBrowsers(environment) => Some((*environment, Permission::BrowserView)),
        Feed::EnvTerminals(environment) => Some((*environment, Permission::TerminalManage)),
        Feed::EnvPorts(environment) => Some((*environment, Permission::PortsView)),
        Feed::EnvGit { environment_id, .. } => Some((*environment_id, Permission::FilesView)),
        Feed::EnvDisplays(environment) => Some((*environment, Permission::VncView)),
        Feed::EnvDir { environment_id, .. } => Some((*environment_id, Permission::FilesView)),
        _ => None,
    }
}

// Deadlines of cached feed decisions, including permissions used to project private fields.

/// The earliest future end that can change this person's authority or a feed's projection.
///
/// The cut is conservative across the person's pages and roles, since a feed can also show
/// references and jobs whose visibility comes from another thing. Extra rechecks are safe;
/// overlooking such a deadline could retain a private projection after its grant ended.
pub(crate) async fn earliest_end(
    inner: &Inner,
    connection: &crate::realtime::server::Connection,
) -> Result<Option<Time>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) =
        super::subscriptions::principal_for_connection(inner, &mut conn, connection).await?
    else {
        return Ok(None);
    };
    principal.read_deadline(&mut conn).await.map_err(Fail::from)
}

/// Whether `user` may follow a feed of the environment that needs `permission` there.
pub(crate) async fn decide(
    inner: &Inner,
    user: Id,
    environment: Id,
    permission: Permission,
) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    for_principal(&mut conn, &principal, environment, permission).await
}

/// The environment permission rule shared by every machine-backed live reader.
pub(crate) async fn for_principal(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    environment: Id,
    permission: Permission,
) -> Result<Access, Fail> {
    let object = ObjectRef {
        kind: ObjectKind::Environment,
        id: environment,
    };
    match resources::check(conn, principal, object, permission, Policy::default()).await {
        Ok(Decision::Allowed { .. }) => Ok(Access::Allowed),
        Ok(Decision::Refused(_)) => Ok(Access::Forbidden),
        Err(orch_authz::Error::NotFound(_)) => Ok(Access::NotFound),
        Err(orch_authz::Error::Suspended { .. } | orch_authz::Error::Archived { .. }) => {
            Ok(Access::Forbidden)
        }
        Err(error) => Err(error.into()),
    }
}

// Current credentials and shared authority at frame delivery.

/// Checks the current socket identity before its frame is projected. No transaction is kept
/// through the projection: it reads on connections of its own, and a frame held a pooled connection
/// idle in a transaction while it waited for those (the pool stalled under a team's live feeds).
/// Authority is checked again at the delivery boundary (`recheck`), credentials included.
pub(crate) async fn delivery_guard(
    inner: &Inner,
    connection: &crate::realtime::server::Connection,
) -> Result<Option<()>, Fail> {
    use orch_api_contract::common::ErrorCode;
    use orch_realtime_contract::version::CloseCode;
    if !connection.authenticated_at(inner.clock.time()) {
        return Ok(None);
    }
    let before = connection.credential();
    let app = inner
        .app()
        .ok_or_else(|| Fail::other("the realtime application is unavailable"))?;
    let mut tx = inner.db.begin().await?;
    let identity = match super::server::current_identity(&app, &mut tx, &before).await {
        Ok(identity) => identity,
        Err(error)
            if matches!(
                error.code(),
                ErrorCode::AuthInvalidToken | ErrorCode::AuthRequired | ErrorCode::AuthSuspended
            ) =>
        {
            if connection.still_current(&before) {
                let code = if error.code() == ErrorCode::AuthSuspended {
                    CloseCode::Suspended
                } else {
                    CloseCode::Unauthenticated
                };
                connection.close(code, "the socket credential is no longer current");
            }
            return Ok(None);
        }
        Err(error) => return Err(Fail::other(error)),
    };
    tx.commit().await.map_err(orch_db::DbError::from)?;
    if identity.check_deadline(&app).is_err() {
        if connection.still_current(&before) {
            connection.close(CloseCode::Unauthenticated, "the credential ran out");
        }
        return Ok(None);
    }
    if !connection.update_current(&before, &identity) {
        // A real reauth replaced this origin while its rows were being read.
        return Ok(None);
    }
    Ok(Some(()))
}

/// Rechecks the exact current session/key even if the page decision's epoch did not move.
pub(crate) async fn credential_current(
    inner: &Inner,
    connection: &crate::realtime::server::Connection,
) -> Result<bool, Fail> {
    Ok(delivery_guard(inner, connection).await?.is_some())
}

/// Rechecks one recipient before a presence or typing frame is serialized, without keeping a transaction.
pub(crate) async fn reader_guard(
    inner: &std::sync::Arc<Inner>,
    sub: &std::sync::Arc<crate::realtime::subscriptions::Subscription>,
) -> Result<Option<()>, Fail> {
    let Some(identity) = delivery_guard(inner, sub.connection()).await? else {
        return Ok(None);
    };
    // Presence/typing has no equivalent HTTP read yet. Until its shared logical catalogue
    // binding is installed, a content-read key is not implicitly granted these extra events.
    if sub.connection().credential().current.grant().is_some() {
        return Ok(None);
    }

    let epoch = super::sync::read_epoch(inner).await?;
    let mut st = sub.state.lock().await;
    if !inner.recheck(sub, &mut st, epoch).await? || !sub.is_live() {
        return Ok(None);
    }
    Ok(Some(identity))
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;

    use orch_types::new_id;
    #[cfg(any(test, feature = "test-support"))]
    fn the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
    {
        let environment = new_id();
        assert_eq!(
            needs(&Feed::EnvProcs(environment)),
            Some((environment, Permission::TerminalView))
        );
        assert_eq!(
            needs(&Feed::EnvStats(environment)),
            Some((environment, Permission::TerminalView))
        );
        assert_eq!(
            needs(&Feed::EnvDir {
                environment_id: environment,
                path: "src".to_owned()
            }),
            Some((environment, Permission::FilesView))
        );
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
     {
        self::the_processes_and_the_load_need_terminal_view_and_a_directory_files_view_on_the_environment()
    }

    #[cfg(any(test, feature = "test-support"))]
    fn no_other_feed_is_an_environments_to_decide_by_its_rows() {
        let id = new_id();
        for feed in [
            Feed::Thread(id),
            Feed::ThreadRaw(id),
            Feed::Env(id),
            Feed::User,
        ] {
            assert_eq!(needs(&feed), None, "{feed}");
        }
    }
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_no_other_feed_is_an_environments_to_decide_by_its_rows() {
        self::no_other_feed_is_an_environments_to_decide_by_its_rows()
    }
}
