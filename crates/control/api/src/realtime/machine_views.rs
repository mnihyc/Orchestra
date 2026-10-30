//! Current machine-backed view projections with the same permissions and personal cuts as their HTTP reads.

use std::collections::HashMap;
use std::sync::Arc;

use super::server::Connection;
use super::subscriptions::{self, Access};
use super::{Fail, Inner};
use crate::middleware::authentication::{Authed, Caller, Via};
use crate::registry::Ctx;
use crate::routes::{browsers, displays, ports, terminals};
use orch_access_contract::permissions::Permission;
use orch_api_contract::ports::{EnvironmentPortList, EnvironmentPortListQuery, PortSnapshot};
use orch_api_contract::routes::Endpoint;
use orch_api_contract::terminals::{EnvironmentTerminalListQuery, TerminalListState};
use orch_db::Conn;
use orch_realtime_contract::feeds::{Feed, TerminalsData};
use orch_types::{Id, Json};

/// Commit notifications cover server-side terminal and port changes as well as connector reports.
pub(crate) const CHANNEL: &str = "orch_machine_views";

/// A Git feed's actual current environment and directory; thread bindings are never cached authority.
pub(crate) async fn git_target(conn: &mut Conn, feed: &Feed) -> Result<Option<(Id, String)>, Fail> {
    match feed {
        Feed::EnvGit {
            environment_id,
            path,
        } => Ok(Some((*environment_id, path.clone()))),
        Feed::ThreadGit(thread) => Ok(orch_db::queries::environments::workspace::thread_place(
            conn, *thread,
        )
        .await?
        .filter(|row| row.side_of.is_none())
        .and_then(|row| {
            row.environment_id
                .map(|environment| (environment, row.workspace_dir))
        })),
        _ => Ok(None),
    }
}

/// A thread contributes context only: its viewer must also hold the matching Git GET's Files View.
pub(crate) async fn decide_git(inner: &Inner, user: Id, feed: &Feed) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = subscriptions::principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    let Some((environment, _)) = git_target(&mut conn, feed).await? else {
        return Ok(Access::NotFound);
    };
    if let Feed::ThreadGit(thread) = feed {
        let access = subscriptions::may_view(&mut conn, &principal, *thread).await?;
        if access != Access::Allowed {
            return Ok(access);
        }
    }
    super::watches::for_principal(&mut conn, &principal, environment, Permission::FilesView).await
}

/// Progress follows the same job/batch read, including maker-only exports and redacted progress.
pub(crate) async fn decide_progress(inner: &Inner, user: Id, feed: &Feed) -> Result<Access, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) = subscriptions::principal_of(inner, &mut conn, user).await? else {
        return Ok(Access::Forbidden);
    };
    match progress_for(&mut conn, &principal, feed).await {
        Ok(_) => Ok(Access::Allowed),
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::NotFound => {
            Ok(Access::NotFound)
        }
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::Internal => {
            Err(Fail::other(error))
        }
        Err(_) => Ok(Access::Forbidden),
    }
}

async fn progress_for(
    conn: &mut Conn,
    principal: &orch_authz::Principal,
    feed: &Feed,
) -> crate::middleware::requests::ApiResult<Json> {
    match feed {
        Feed::Job(id) => Ok(
            serde_json::json!({"job": crate::routes::environments::jobs::read_for(conn, principal, *id).await?}),
        ),
        Feed::Fork(id) => Ok(
            serde_json::json!({"batch": crate::routes::forks::read_batch_for(conn, principal, *id).await?}),
        ),
        _ => Err(crate::middleware::requests::ApiError::not_found()),
    }
}

/// A committed change is projected once; deletion and a changed maker close rather than poll a job.
pub(crate) async fn progress(
    inner: &Arc<Inner>,
    connection: &Connection,
    feed: &Feed,
) -> Result<Result<Json, Access>, Fail> {
    let mut conn = inner.db.acquire().await?;
    let Some(principal) =
        subscriptions::principal_for_connection(inner, &mut conn, connection).await?
    else {
        return Ok(Err(Access::Forbidden));
    };
    match progress_for(&mut conn, &principal, feed).await {
        Ok(data) => Ok(Ok(data)),
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::NotFound => {
            Ok(Err(Access::NotFound))
        }
        Err(error) if error.code() == orch_api_contract::common::ErrorCode::Internal => {
            Err(Fail::other(error))
        }
        Err(_) => Ok(Err(Access::Forbidden)),
    }
}

/// Feeds whose wire rows are always projected for their particular recipient.
pub(crate) fn projected(feed: &Feed) -> bool {
    matches!(
        feed,
        Feed::EnvTerminals(_)
            | Feed::EnvPorts(_)
            | Feed::EnvBrowsers(_)
            | Feed::EnvDisplays(_)
            | Feed::ThreadGit(_)
            | Feed::EnvGit { .. }
            | Feed::Job(_)
            | Feed::Fork(_)
    )
}

/// Reads use a single nontransactional connection. In particular, no authority transaction spans
/// the existing shell GET's initial native observation, and later reports supply it directly.
pub(crate) async fn project(
    inner: &Arc<Inner>,
    connection: &Connection,
    feed: &Feed,
    data: &Json,
) -> Result<Result<Option<Json>, Access>, Fail> {
    let projected = project_rows(inner, connection, feed, data).await;
    #[cfg(test)]
    super::epoch_handoff_tests::after_projection(super::epoch_handoff_tests::Cut::MachineReady)
        .await;
    if let Some(environment) = super::environment_watches::environment(feed) {
        // The native reader can lose its object between its permission read
        // and later metadata/settings statements, even without an auth epoch.
        // Retire only that gone source; an error on a still-live source remains
        // an error. This is a delivery check, never a polling reader.
        let mut conn = inner.db.acquire().await?;
        let live: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM environments WHERE id=$1 AND deleted_at IS NULL)",
        )
        .bind(environment)
        .fetch_one(&mut *conn)
        .await
        .map_err(orch_db::DbError::from)?;
        if !live {
            return Ok(Err(Access::NotFound));
        }
    }
    projected.map(Ok)
}

async fn project_rows(
    inner: &Arc<Inner>,
    connection: &Connection,
    feed: &Feed,
    data: &Json,
) -> Result<Option<Json>, Fail> {
    if data.get("offline") == Some(&Json::Bool(true)) {
        return Ok(Some(data.clone()));
    }
    let Some(app) = inner.app() else {
        return Ok(None);
    };
    let mut conn = inner.db.acquire().await?;
    let Some(principal) =
        subscriptions::principal_for_connection(inner, &mut conn, connection).await?
    else {
        return Ok(None);
    };
    if matches!(feed, Feed::ThreadGit(_) | Feed::EnvGit { .. }) {
        let Some((environment, path)) = git_target(&mut conn, feed).await? else {
            return Ok(None);
        };
        // A late report of a superseded thread binding must never be shown as its new Git state.
        if data["environment_id"] != environment.to_string() || data["path"] != path {
            return Ok(None);
        }
        if super::watches::for_principal(&mut conn, &principal, environment, Permission::FilesView)
            .await?
            != Access::Allowed
        {
            return Ok(None);
        }
        if let Feed::ThreadGit(thread) = feed
            && subscriptions::may_view(&mut conn, &principal, *thread).await? != Access::Allowed
        {
            return Ok(None);
        }
        return Ok(Some(data.clone()));
    }
    let environment = match feed {
        Feed::EnvTerminals(id)
        | Feed::EnvPorts(id)
        | Feed::EnvBrowsers(id)
        | Feed::EnvDisplays(id) => *id,
        _ => return Ok(None),
    };
    let Some((_, permission)) = super::watches::needs(feed) else {
        return Ok(None);
    };
    if super::watches::for_principal(&mut conn, &principal, environment, permission).await?
        != Access::Allowed
    {
        return Ok(None);
    }
    #[cfg(test)]
    super::epoch_handoff_tests::after_projection(super::epoch_handoff_tests::Cut::MachineAdmitted)
        .await;
    let Some(user) = orch_db::queries::identity::user_by_id(&mut conn, connection.user).await?
    else {
        return Ok(None);
    };
    let ctx = Ctx::for_tool(
        app,
        Caller::User(Arc::new(Authed {
            as_admin: principal.caller().as_admin(),
            user,
            via: Via::Session {
                session_id: connection.session(),
            },
        })),
        EnvironmentPortList::ROUTE,
        environment,
    );
    let view = match feed {
        Feed::EnvTerminals(_) => {
            // The first native snapshot arrives as soon as watch.set starts; never turn a live
            // recipient's projection into an extra machine request.
            if data.get("terminals").is_none() {
                return Ok(None);
            }
            let observed = data
                .get("terminals")
                .map(|items| {
                    serde_json::from_value::<Vec<orch_link_contract::commands::PtyStatusEntry>>(
                        items.clone(),
                    )
                    .map(|rows| {
                        rows.into_iter()
                            .map(|row| (row.terminal_id, row.busy))
                            .collect::<HashMap<_, _>>()
                    })
                })
                .transpose()
                .map_err(Fail::other)?;
            let open = terminals::list_for(
                &ctx,
                &mut conn,
                &principal,
                environment,
                EnvironmentTerminalListQuery {
                    state: Some(TerminalListState::Open),
                    limit: Some(200),
                    ..Default::default()
                },
                observed.as_ref(),
            )
            .await
            .map_err(Fail::other)?;
            let closed = terminals::list_for(
                &ctx,
                &mut conn,
                &principal,
                environment,
                EnvironmentTerminalListQuery {
                    state: Some(TerminalListState::Closed),
                    limit: Some(10),
                    ..Default::default()
                },
                observed.as_ref(),
            )
            .await
            .map_err(Fail::other)?;
            serde_json::to_value(TerminalsData { open, closed }).map_err(Fail::other)?
        }
        Feed::EnvPorts(_) => {
            let env = ports::environment(&mut conn, environment)
                .await
                .map_err(Fail::other)?;
            if let Some(snapshot) = data.get("snapshot") {
                let snapshot: PortSnapshot =
                    serde_json::from_value(snapshot.clone()).map_err(Fail::other)?;
                if snapshot.environment_id != environment
                    || snapshot.generation != i64::from(env.generation)
                {
                    return Ok(None);
                }
                ctx.app.cache.ports().observe(snapshot);
            }
            serde_json::to_value(
                ports::list_view(
                    &ctx,
                    &mut conn,
                    &principal,
                    &env,
                    EnvironmentPortListQuery {
                        limit: Some(200),
                        ..Default::default()
                    },
                )
                .await
                .map_err(Fail::other)?,
            )
            .map_err(Fail::other)?
        }
        Feed::EnvBrowsers(_) if data.get("browsers").is_some() => {
            browsers::filter_list(&mut conn, &principal, environment, data.clone())
                .await
                .map_err(Fail::other)?
        }
        Feed::EnvBrowsers(_) => {
            let value = browsers::current(&ctx.app, &mut conn, Some(&principal), environment)
                .await
                .map_err(Fail::other)?;
            serde_json::json!({"browsers":value.browsers,"profiles":value.profiles})
        }
        Feed::EnvDisplays(_) if data.get("displays").is_some() => {
            displays::filter_list(&mut conn, &principal, environment, data.clone())
                .await
                .map_err(Fail::other)?
        }
        Feed::EnvDisplays(_) => {
            let value = displays::current(&ctx.app, &mut conn, Some(&principal), environment)
                .await
                .map_err(Fail::other)?;
            serde_json::json!({"displays":value.displays})
        }
        _ => return Ok(None),
    };
    Ok(Some(view))
}
