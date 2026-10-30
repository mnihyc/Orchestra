//! What the agents of an environment can reach, over HTTP: one read-only overview cut by the caller's own rights.
//!
//! The overview is made of what the system already knows, read by the readers that already show each part: the
//! environment's network as its own view shows it, the names its secret setting resolves to, the mounts attached to
//! it, the arrangement of each thread of it the caller may view, and the browser profiles the environment was last
//! found to keep. Nothing is fetched to make it and no key, secret value, cookie or site is in it. Each part is shown
//! only to whoever already reads it elsewhere: the secret names to whoever manages the environment, the mounts to
//! whoever reads its files, the profiles to whoever sees its browsers; another caller gets `null` there. A caller who
//! holds nothing on the environment and sees it only through a thread gets what that thread lets them see: whether
//! its network is restricted and in force, and the lines of the threads they may view.

use crate::middleware::authorization;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::environments::reading::{self, Reading};
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{ObjectKind, Permission};
use orch_api_contract::environment::{
    self as api, AgentReachOverview, AgentReachOverviewBrowserProfile,
    AgentReachOverviewCredential, AgentReachOverviewMount, AgentReachOverviewSecrets,
};
use orch_api_contract::mounts::EnvMountState;
use orch_authz::{Policy, Principal};
use orch_db::{Conn, DbError};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Time};

/// The word of a thread that has no arrangement yet: no key is placed for it until its first turn.
const UNBOUND: &str = "unbound";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(api::EnvironmentAgentReachGet, get);
}

/// Whether the caller holds `permission` on the environment, without refusing them: a part they may not read is
/// `null`, never an error for the whole.
async fn holds(
    conn: &mut Conn,
    principal: &Principal,
    environment: Id,
    permission: Permission,
) -> ApiResult<bool> {
    Ok(orch_authz::resources::check(
        conn,
        principal,
        ObjectRef {
            kind: ObjectKind::Environment,
            id: environment,
        },
        permission,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .is_allowed())
}

/// The arrangement of each thread of `threads`, and when the copy of a key it last had runs out.
async fn arrangements(
    conn: &mut Conn,
    threads: &[Id],
) -> Result<Vec<(Id, Option<String>, Option<Time>)>, DbError> {
    let rows: Vec<(Id, Option<String>, Option<chrono::DateTime<chrono::Utc>>)> = sqlx::query_as(
        "SELECT t.node_id, t.arrangement, \
           (SELECT l.copy_expires_at FROM leases l \
             WHERE l.thread_id = t.node_id AND l.mode = 'shared' \
             ORDER BY l.opened_at DESC LIMIT 1) \
         FROM threads t WHERE t.node_id = ANY($1)",
    )
    .bind(threads)
    .fetch_all(&mut *conn)
    .await
    .map_err(DbError::from)?;
    Ok(rows
        .into_iter()
        .map(|(thread, arrangement, expires)| (thread, arrangement, expires.map(Time::new)))
        .collect())
}

/// The line of one thread: its arrangement, and the expiry of a copy where the arrangement keeps a copy.
fn credential_line(
    arrangement: Option<&str>,
    copy_expires_at: Option<Time>,
) -> (String, Option<Time>) {
    match arrangement {
        Some(arrangement) => (
            arrangement.to_owned(),
            (arrangement == "credential_shared")
                .then_some(copy_expires_at)
                .flatten(),
        ),
        None => (UNBOUND.to_owned(), None),
    }
}

/// What the agents of the environment can reach, for this caller.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<AgentReachOverview> {
    let environment = ctx.id("id")?;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let reading = reading::decide(&mut conn, &principal, environment).await?;
    let whole = super::show(&ctx, &mut conn, &principal, environment).await?;
    let view = match reading {
        Reading::Whole => whole,
        Reading::ThroughAThread => reading::through_a_thread(whole),
    };
    let whole_reader = matches!(reading, Reading::Whole);
    let manages = whole_reader
        && holds(
            &mut conn,
            &principal,
            environment,
            Permission::EnvironmentManage,
        )
        .await?;
    let reads_files =
        whole_reader && holds(&mut conn, &principal, environment, Permission::FilesView).await?;
    let sees_browsers =
        whole_reader && holds(&mut conn, &principal, environment, Permission::BrowserView).await?;

    let secrets = if manages {
        let mut names: Vec<String> = orch_settings::get(
            &mut conn,
            orch_settings::Place::Environment(environment),
            keys::ENV_SECRET_NAMES,
        )
        .await
        .map_err(ApiError::internal)?
        .unwrap_or_default();
        names.sort();
        names.dedup();
        AgentReachOverviewSecrets {
            names: Some(names),
            source: Some(view.home.clone()),
        }
    } else {
        AgentReachOverviewSecrets {
            names: None,
            source: None,
        }
    };

    let mounts = reads_files.then(|| {
        view.mounts
            .iter()
            .filter(|mount| mount.state == EnvMountState::Attached)
            .filter_map(|mount| {
                Some(AgentReachOverviewMount {
                    mount: mount.mount.clone(),
                    path: mount.path.clone()?,
                    mode: serde_json::to_value(mount.mode).ok()?.as_str()?.to_owned(),
                })
            })
            .collect()
    });

    // One line for each thread the caller may view; the view already leaves the others out.
    let threads: Vec<Id> = view.threads.iter().map(|thread| thread.thread.id).collect();
    let found = arrangements(&mut conn, &threads).await?;
    let credentials = view
        .threads
        .iter()
        .map(|thread| {
            let (arrangement, expires) = found
                .iter()
                .find(|(id, _, _)| *id == thread.thread.id)
                .map_or((None, None), |(_, arrangement, expires)| {
                    (arrangement.as_deref(), *expires)
                });
            let (placement, expires_at) = credential_line(arrangement, expires);
            AgentReachOverviewCredential {
                thread: thread.thread.clone(),
                placement,
                expires_at,
            }
        })
        .collect();

    let browser_profiles = sees_browsers.then(|| {
        ctx.app
            .engine
            .browsers()
            .memory()
            .profiles(environment)
            .unwrap_or_default()
            .into_iter()
            .map(|profile| AgentReachOverviewBrowserProfile { name: profile.name })
            .collect()
    });

    let home_path = if whole_reader {
        crate::routes::nodes::view::ways(&mut conn, &principal, &[view.home.id], Policy::default())
            .await?
            .remove(&view.home.id)
    } else {
        None
    };
    Ok(AgentReachOverview {
        home_path,
        network: view.network,
        secrets,
        mounts,
        credentials,
        browser_profiles,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry() {
        let at: Time = "2026-10-15T00:00:00.000Z".parse().unwrap();
        assert_eq!(
            credential_line(Some("credential_shared"), Some(at)),
            ("credential_shared".to_owned(), Some(at))
        );
        assert_eq!(
            credential_line(Some("key_separated"), Some(at)),
            ("key_separated".to_owned(), None),
            "a forwarder token is no copy of the key"
        );
        assert_eq!(credential_line(None, None), ("unbound".to_owned(), None));
    }
}
