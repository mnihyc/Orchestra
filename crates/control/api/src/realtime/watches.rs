//! Who may follow the ephemeral feeds of an environment, decided by the environment's own page: its processes and its load by `terminal.view`, a directory of its workspace by `files.view`, and none of them through a thread; and diagnostics.
//!
//! Each environment feed asks its own permission page and is rechecked on authorization changes
//! and before the earliest end of the caller's grants or roles.
//! The transient coordinator runs its measurements and directory watches; browser and display
//! parts publish their complete lists through the same machinery.

use crate::realtime::subscriptions::{Access, principal_of};
use crate::realtime::{Fail, Inner};
use orch_access_contract::decisions::{Decision, ObjectRef};
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_authz::{Policy, resources};
use orch_realtime_contract::feeds::Feed;
use orch_types::{Id, Time};

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
pub(crate) async fn earliest_end(inner: &Inner, user: Id) -> Result<Option<Time>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = principal_of(inner, &mut conn, user).await? else {
        return Ok(None);
    };
    let now: chrono::DateTime<chrono::Utc> = principal.now().into();
    let at: Option<chrono::DateTime<chrono::Utc>> = sqlx::query_scalar(
        "WITH reaching_roles AS (
             SELECT expires_at FROM node_roles
              WHERE removed_at IS NULL AND (expires_at IS NULL OR expires_at > $4)
                AND ((subject_kind = 'user' AND subject_id = $1)
                  OR (subject_kind = 'group' AND subject_id = ANY($2))
                  OR (subject_kind = 'members' AND subject_id = ANY($3)))
         ), ends AS (
             SELECT expires_at FROM permission_entries
              WHERE removed_at IS NULL AND expires_at > $4
                AND (holder_kind = 'everyone'
                  OR (holder_kind = 'user' AND holder_id = $1)
                  OR (holder_kind = 'group' AND holder_id = ANY($2))
                  OR holder_kind = 'role')
             UNION ALL SELECT expires_at FROM reaching_roles
             UNION ALL SELECT expires_at FROM chat_members WHERE user_id=$1 AND left_at IS NULL AND expires_at>$4
         ) SELECT min(expires_at) FROM ends",
    )
    .bind(user)
    .bind(principal.groups())
    .bind(principal.member_of())
    .bind(now)
    .fetch_one(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    Ok(at.map(Time::new))
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
    use orch_db::queries::transaction_guard::{self as guard, Credential};
    use orch_realtime_contract::version::CloseCode;
    if !connection.authenticated_at(inner.clock.time()) {
        return Ok(None);
    }
    let (session, hash, _) = connection.credential();
    let mut tx = inner.db.begin().await?;
    let identity = guard::lock_identity(
        &mut tx,
        connection.user,
        Credential::Session {
            id: session,
            hash: &hash,
        },
        false,
    )
    .await?;
    let Some(identity) = identity else {
        connection.close(CloseCode::Unauthenticated, "the session is no longer valid");
        return Ok(None);
    };
    if identity
        .expires_at
        .is_some_and(|until| until <= inner.clock.now())
    {
        connection.close(
            CloseCode::Unauthenticated,
            "the access token or session ran out",
        );
        return Ok(None);
    }
    if identity.user.suspended() {
        connection.close(CloseCode::Suspended, "this user is suspended");
        return Ok(None);
    }
    if let Some(until) = identity.expires_at {
        connection.update_expiry(session, &hash, until);
    }
    connection.update_admin_mode(session, &hash, identity.as_admin);
    tx.commit().await.map_err(orch_db::DbError::from)?;
    Ok(Some(()))
}

/// Checks the socket's exact session token even when its cached page decision is unchanged.
pub(crate) async fn credential_current(
    inner: &Inner,
    connection: &crate::realtime::server::Connection,
) -> Result<bool, Fail> {
    use orch_realtime_contract::version::CloseCode;
    if !connection.authenticated_at(inner.clock.time()) {
        return Ok(false);
    }
    let (session, hash, _) = connection.credential();
    let mut conn = inner.db.acquire().await?;
    let identity: Option<(String, chrono::DateTime<chrono::Utc>, bool)> = sqlx::query_as(
        "SELECT u.status, LEAST(a.expires_at, s.expires_at), s.as_admin
           FROM users u JOIN sessions s ON s.user_id = u.id
           JOIN access_tokens a ON a.session_id = s.id
          WHERE u.id = $1 AND s.id = $2 AND a.token_hash = $3 AND s.revoked_at IS NULL",
    )
    .bind(connection.user)
    .bind(session)
    .bind(&hash)
    .fetch_optional(&mut *conn)
    .await
    .map_err(orch_db::DbError::from)?;
    match identity {
        Some((status, _, _)) if status == "suspended" => {
            connection.close(CloseCode::Suspended, "this user is suspended");
            Ok(false)
        }
        Some((_, until, as_admin)) if until > inner.clock.now() => {
            connection.update_expiry(session, &hash, until);
            connection.update_admin_mode(session, &hash, as_admin);
            Ok(true)
        }
        _ => {
            connection.close(
                CloseCode::Unauthenticated,
                "the access token or session is no longer valid",
            );
            Ok(false)
        }
    }
}

/// The earliest end of the exact browser credential, including its session's own end.
pub(crate) async fn session_until(
    conn: &mut orch_db::Conn,
    user: Id,
    session: Id,
    hash: &[u8],
    now: chrono::DateTime<chrono::Utc>,
) -> Result<Option<chrono::DateTime<chrono::Utc>>, orch_db::DbError> {
    Ok(sqlx::query_scalar(
        "SELECT LEAST(a.expires_at, s.expires_at)
           FROM access_tokens a JOIN sessions s ON s.id = a.session_id
          WHERE a.token_hash = $1 AND s.id = $2 AND s.user_id = $3
            AND a.expires_at > $4 AND s.expires_at > $4 AND s.revoked_at IS NULL",
    )
    .bind(hash)
    .bind(session)
    .bind(user)
    .bind(now)
    .fetch_optional(conn)
    .await?)
}

/// Rechecks one recipient before a presence or typing frame is serialized, without keeping a transaction.
pub(crate) async fn reader_guard(
    inner: &std::sync::Arc<Inner>,
    sub: &std::sync::Arc<crate::realtime::subscriptions::Subscription>,
) -> Result<Option<()>, Fail> {
    let Some(identity) = delivery_guard(inner, sub.connection()).await? else {
        return Ok(None);
    };
    let epoch = super::sync::read_epoch(inner).await?;
    let mut st = sub.state.lock().await;
    if !inner.recheck(sub, &mut st, epoch).await? || !sub.is_live() {
        return Ok(None);
    }
    Ok(Some(identity))
}

#[cfg(test)]
mod tests {
    use super::*;
    use orch_types::new_id;

    #[test]
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

    #[test]
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
}
