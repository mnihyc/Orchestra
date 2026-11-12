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

use super::network;

use super::reach_operation;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::OperationFuture;

use crate::registry::Registry;

use crate::routes::access::operations::NameRoles;

use crate::routes::access::operations::projection_names;

use crate::routes::access::page::NameBounds;

use crate::routes::access::page::NameSources;

use crate::routes::environments::reading;

use crate::routes::environments::reading::Reading;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_api_contract::environment as api;

use orch_api_contract::environment::AgentReachOverview;

use orch_api_contract::environment::AgentReachOverviewBrowserProfile;

use orch_api_contract::environment::AgentReachOverviewCredential;

use orch_api_contract::environment::AgentReachOverviewMount;

use orch_api_contract::environment::AgentReachOverviewSecrets;

use orch_api_contract::mounts::EnvMount;

use orch_api_contract::mounts::EnvMountState;

use orch_authz::Caller;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::engine::Graph;

use orch_db::queries::environments as rows;

use orch_db::Conn;

use orch_db::DbError;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Time;

use std::collections::BTreeSet;
const NAMES: NameRoles = NameRoles {
    groups: None,
    nodes: [
        (ObjectKind::Org, "agent_reach_names_orgs"),
        (ObjectKind::Project, "agent_reach_names_projects"),
        (ObjectKind::Topic, "agent_reach_names_topics"),
        (ObjectKind::Thread, "agent_reach_names_threads"),
    ],
    containers: None,
};

struct PreparedReach {
    home: Id,
    whole: bool,
    network: network::PreparedNetwork,
    secret_names: Option<Vec<String>>,
    mounts: Option<Vec<EnvMount>>,
    credentials: Vec<rows::ThreadArrangement>,
    profiles: Option<Vec<String>>,
    names: NameBounds,
}

/// The word of a thread that has no arrangement yet: no key is placed for it until its first turn.
const UNBOUND: &str = "unbound";

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on_with_operation(api::EnvironmentAgentReachGet, operation, get);
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

fn operation<'a>(
    ctx: &'a Ctx,
    conn: &'a mut Conn,
    caller: &'a Caller,
    (): (),
    (): (),
) -> OperationFuture<'a> {
    Box::pin(async move {
        let environment = ctx.id("id")?;
        let mut plan = reach_operation::root(ctx, conn, caller).await?;
        let principal = plan.preparation_principal(ctx, conn, caller).await?;
        let reading = reading::decide(conn, &principal, environment)
            .await
            .map_err(|problem| reach_operation::error(caller, problem))?;
        let row = super::queries::environments(conn, &[environment])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        let whole_reader = matches!(reading, Reading::Whole);
        let manages = whole_reader
            && holds(conn, &principal, environment, Permission::EnvironmentManage).await?;
        let reads_files =
            whole_reader && holds(conn, &principal, environment, Permission::FilesView).await?;
        let sees_browsers =
            whole_reader && holds(conn, &principal, environment, Permission::BrowserView).await?;
        let network = network::prepare_environments(conn, &[environment])
            .await?
            .remove(&environment)
            .ok_or_else(|| ApiError::internal("the Environment network was not resolved"))?;
        let secret_names = if manages {
            let mut names: Vec<String> = orch_settings::get(
                conn,
                orch_settings::Place::Environment(environment),
                keys::ENV_SECRET_NAMES,
            )
            .await
            .map_err(ApiError::internal)?
            .unwrap_or_default();
            names.sort();
            names.dedup();
            Some(names)
        } else {
            None
        };
        let mounts = if reads_files
            && super::list_settings::load(conn, std::slice::from_ref(&row))
                .await?
                .has_mounts(environment)
        {
            orch_engine::mounts::views(conn, environment, principal.now().get())
                .await
                .map_err(ApiError::internal)?
                .into_iter()
                .filter(|mount| mount.state == EnvMountState::Attached)
                .filter(|mount| mount.path.is_some())
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        let mount_ids = mounts
            .iter()
            .map(|mount| mount.mount.id)
            .collect::<BTreeSet<_>>();
        let mount_ids =
            reach_operation::mounts(conn, caller, &mut plan, "agent_reach_mounts", &mount_ids)
                .await?;
        let mounts = reads_files.then(|| {
            mounts
                .into_iter()
                .filter(|mount| mount_ids.contains(&mount.mount.id))
                .collect()
        });

        // Reuse the Environment view's actual ordinary Thread/Topic reference predicate.
        // Topic visibility is not an additional key Read when no Topic content is returned.
        let bound = rows::threads_in(conn, &[environment]).await?;
        let referenced = bound
            .iter()
            .flat_map(|thread| [thread.thread_id, thread.topic_id])
            .collect::<BTreeSet<_>>();
        let known = crate::routes::nodes::view::reference_visibility(
            conn,
            &principal,
            &referenced.into_iter().collect::<Vec<_>>(),
            Policy::default(),
        )
        .await?;
        let thread_ids = bound
            .iter()
            .filter(|thread| super::bound_thread_visible(thread, &known))
            .map(|thread| thread.thread_id)
            .collect::<BTreeSet<_>>();
        let thread_ids =
            reach_operation::threads(conn, caller, &mut plan, "agent_reach_threads", &thread_ids)
                .await?;
        let mut credentials =
            rows::thread_arrangements(conn, &thread_ids.iter().copied().collect::<Vec<_>>())
                .await?;
        // The old reply iterated Environment threads_in (ordered by Thread ID), then
        // looked up each arrangement. Keep that order independently of the batch query plan.
        credentials.sort_by_key(|row| row.thread);
        let mut name_sources = NameSources::default();
        if whole_reader {
            let graph = Graph::load(conn, &[row.home_node_id])
                .await
                .map_err(ApiError::internal)?;
            let home = graph
                .get(row.home_node_id)
                .ok_or_else(ApiError::not_found)?;
            name_sources.nodes.extend(home.path.iter().copied());
            name_sources.nodes.extend(network.name_sources());
        }
        let mut names = projection_names(conn, caller, &mut plan, name_sources, &NAMES).await?;
        // These exact Thread names/displays are already part of their admitted content Read;
        // they do not require a separate read of every parent used for ordinary visibility.
        names.nodes.extend(thread_ids);
        let profiles = sees_browsers.then(|| {
            ctx.app
                .engine
                .browsers()
                .memory()
                .profiles(environment)
                .unwrap_or_default()
                .into_iter()
                .map(|profile| profile.name)
                .collect()
        });
        Ok(plan.with_data(PreparedReach {
            home: row.home_node_id,
            whole: whole_reader,
            network,
            secret_names,
            mounts,
            credentials,
            profiles,
            names,
        }))
    })
}

/// What the agents of the Environment can reach, through the same current ordinary producer.
async fn get(ctx: Ctx, (): (), (): ()) -> ApiResult<AgentReachOverview> {
    crate::key_operations::snapshot_read::run(ctx, snapshot).await
}

async fn snapshot(ctx: Ctx) -> ApiResult<AgentReachOverview> {
    let mut conn = ctx.app.db.begin_snapshot().await?;
    let principal = ctx.operation_principal(&mut conn).await?;
    let prepared = ctx.operation_data::<PreparedReach>()?;
    let thread_ids = prepared
        .credentials
        .iter()
        .map(|row| row.thread)
        .collect::<Vec<_>>();
    let mut ids = thread_ids.clone();
    if prepared.secret_names.is_some() {
        ids.push(prepared.home);
    }
    let refs = prepared.names.refs(&mut conn, &principal, &ids).await?;
    let credentials = prepared
        .credentials
        .iter()
        .map(|row| {
            let (placement, expires_at) = credential_line(
                row.arrangement.as_deref(),
                row.copy_expires_at.map(Time::new),
            );
            Ok(AgentReachOverviewCredential {
                thread: refs
                    .get(&row.thread)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?,
                placement,
                expires_at,
            })
        })
        .collect::<ApiResult<Vec<_>>>()?;
    let secrets = AgentReachOverviewSecrets {
        names: prepared.secret_names.clone(),
        source: if prepared.secret_names.is_some() {
            Some(
                refs.get(&prepared.home)
                    .cloned()
                    .ok_or_else(ApiError::not_found)?,
            )
        } else {
            None
        },
    };
    let mounts = prepared.mounts.as_ref().map(|mounts| {
        mounts
            .iter()
            .filter_map(|mount| {
                Some(AgentReachOverviewMount {
                    mount: mount.mount.clone(),
                    path: mount.path.clone()?,
                    mode: serde_json::to_value(mount.mode).ok()?.as_str()?.to_owned(),
                })
            })
            .collect()
    });
    let browser_profiles = prepared.profiles.as_ref().map(|profiles| {
        profiles
            .iter()
            .map(|name| AgentReachOverviewBrowserProfile { name: name.clone() })
            .collect()
    });
    let home_path = if prepared.whole {
        let mut paths = if prepared.names.limited {
            crate::routes::nodes::view::ways_with_name_sources(
                &mut conn,
                &principal,
                &[prepared.home],
                Policy::default(),
                &prepared.names.nodes,
            )
            .await?
        } else {
            crate::routes::nodes::view::ways(
                &mut conn,
                &principal,
                &[prepared.home],
                Policy::default(),
            )
            .await?
        };
        paths.remove(&prepared.home)
    } else {
        None
    };
    let network = if prepared.whole {
        prepared
            .network
            .project(&mut conn, Some(&principal), Some(&prepared.names))
            .await?
    } else {
        prepared.network.through_thread()
    };
    let shown = AgentReachOverview {
        home_path,
        network,
        secrets,
        mounts,
        credentials,
        browser_profiles,
    };
    conn.commit().await.map_err(DbError::from)?;
    Ok(shown)
}

#[cfg(any(test, feature = "test-support"))]
pub mod tests {

    use super::*;
    #[cfg(any(test, feature = "test-support"))]
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
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    pub fn __api_run_each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry() {
        self::each_threads_line_names_its_arrangement_and_only_a_copy_has_an_expiry()
    }
}
