//! Who may read an environment and how much of it: whoever holds a permission on it reads all of it, and whoever holds none and may view a thread that runs there reads its name, owner, place, state and the job under way, and nothing of what is behind it.
//!
//! This is the one place that says it, for the request that reads an environment and for whatever
//! else shows the same view of one. [`decide`] decides, by the access engine's own answers: a
//! permission on the environment itself (its owner, an admin of its place, either level, any of its
//! four doors) is the whole of it, and a thread bound to it that the caller may view is the cut-down
//! view, which [`through_a_thread`] makes of a whole one. Somebody who has neither is refused with what
//! they lack to look at it and who can give it.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::routes::resources::require;
use orch_access_contract::decisions::ObjectRef;
use orch_access_contract::permissions::{NodePermission, Permission, PermissionName, ResourceKind};
use orch_api_contract::environment::{Environment, EnvironmentExec, EnvironmentSupport};
use orch_authz::{Act, Policy, Principal, nodes, resources};
use orch_db::Conn;
use orch_db::queries::environments as env_rows;
use orch_db::queries::environments::workspace as environment_rows;
use orch_types::Id;
use serde_json::json;
use std::collections::{HashMap, HashSet};

/// Both the reading cut and the exact holdings used by every batched environment projection.
pub(crate) struct Allowed {
    pub(crate) reading: Reading,
    pub(crate) effective: orch_authz::resources::Effective,
}

/// The environments among an explicit set that this caller reaches, through their own page or a visible bound thread.
pub(crate) async fn among(
    conn: &mut Conn,
    principal: &Principal,
    ids: &[Id],
) -> ApiResult<HashMap<Id, Allowed>> {
    let holdings = resources::effective_among(conn, principal, ids, Policy::default())
        .await
        .map_err(ApiError::internal)?;
    let bound = env_rows::threads_in(conn, ids).await?;
    let threads: Vec<Id> = bound.iter().map(|thread| thread.thread_id).collect();
    let visible: HashSet<Id> = nodes::holding(
        conn,
        principal,
        &threads,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?
    .into_iter()
    .map(|node| node.id)
    .collect();
    let through: HashSet<Id> = bound
        .iter()
        .filter(|thread| visible.contains(&thread.thread_id))
        .map(|thread| thread.environment_id)
        .collect();
    Ok(holdings
        .into_iter()
        .filter_map(|effective| {
            let id = effective.object.id;
            let reading = if effective.held.iter().any(|held| {
                !matches!(
                    held.permission,
                    Permission::AccessManage | Permission::AccessExternal
                )
            }) {
                Reading::Whole
            } else if through.contains(&id) {
                Reading::ThroughAThread
            } else {
                return None;
            };
            Some((id, Allowed { reading, effective }))
        })
        .collect())
}

/// Topic summaries use exactly the same binding set and reading door as full environment cards.
pub(crate) async fn topic_summaries(
    conn: &mut Conn,
    principal: &Principal,
    topics: &[Id],
    now: orch_types::Time,
) -> ApiResult<HashMap<Id, Vec<orch_api_contract::environment::EnvironmentRef>>> {
    let mut bindings = env_rows::hosting::environments_under_many(conn, topics).await?;
    for (node, default) in orch_settings::resolver::get_at_nodes::<Id>(
        conn,
        topics,
        orch_settings_contract::registry::keys::ENV_DEFAULT_ID,
    )
    .await
    .map_err(ApiError::internal)?
    {
        if let Some(id) = default
            && !bindings.contains(&(node, id))
        {
            bindings.push((node, id));
        }
    }
    let mut ids: Vec<Id> = bindings.iter().map(|(_, id)| *id).collect();
    ids.sort_unstable();
    ids.dedup();
    let allowed = among(conn, principal, &ids).await?;
    ids.retain(|id| allowed.contains_key(id));
    let refs: HashMap<Id, orch_api_contract::environment::EnvironmentRef> =
        crate::routes::resources::view::environment_refs(conn, &ids, &[], now)
            .await?
            .into_iter()
            .map(|(_, environment)| (environment.id, environment))
            .collect();
    let mut summaries: HashMap<Id, Vec<orch_api_contract::environment::EnvironmentRef>> =
        HashMap::new();
    for (topic, id) in bindings {
        if let Some(environment) = refs.get(&id) {
            summaries
                .entry(topic)
                .or_default()
                .push(environment.clone());
        }
    }
    Ok(summaries)
}

/// How much of an environment a caller may read, which is how they reach it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Reading {
    /// They hold a permission on the environment: all of it, and its configuration and the names of
    /// its secrets with `environment.manage`.
    Whole,
    /// They hold nothing there and may view a thread that runs in it: what [`through_a_thread`] leaves.
    ThroughAThread,
}

/// Whether `principal` may read the environment `id`, and how much of it: not found where there is
/// none, [`Reading::Whole`] for whoever holds something on it, [`Reading::ThroughAThread`] for whoever
/// holds nothing and may view a thread bound to it (`node.view` on one of them), and else a refusal
/// naming what is lacking on the environment and who can give it.
pub(crate) async fn decide(conn: &mut Conn, principal: &Principal, id: Id) -> ApiResult<Reading> {
    environment_rows::environment(conn, id)
        .await?
        .ok_or_else(ApiError::not_found)?;
    let object = ObjectRef {
        kind: ResourceKind::Environment.into(),
        id,
    };
    let holding = match resources::effective(conn, principal, object, Policy::default()).await {
        Ok(effective) => effective.held.iter().any(|held| {
            !matches!(
                held.permission,
                Permission::AccessManage | Permission::AccessExternal
            )
        }),
        Err(orch_authz::Error::NotFound(_)) => return Err(ApiError::not_found()),
        Err(error) => return Err(refusals::problem(conn, error).await),
    };
    if holding {
        return Ok(Reading::Whole);
    }
    let bound: Vec<Id> = env_rows::threads_in(conn, &[id])
        .await?
        .into_iter()
        .map(|thread| thread.thread_id)
        .collect();
    let viewable = nodes::holding(
        conn,
        principal,
        &bound,
        PermissionName::OfNode(NodePermission::View),
        Act::Use,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    if !viewable.is_empty() {
        return Ok(Reading::ThroughAThread);
    }
    // Holding nothing and seeing no thread of it: the refusal says what the caller lacks to look at
    // it, and who can give it.
    require(
        conn,
        principal,
        ResourceKind::Environment,
        id,
        Permission::EnvironmentView,
    )
    .await?;
    Ok(Reading::Whole)
}

/// What a whole view of an environment is left with for somebody who sees it only through a thread:
/// its name, owner, place, state and the job under way, with the threads of it that they may view and
/// how many more there are, and nothing of its configuration, its facts, its mounts, its machine and
/// host, its switches, what it was found to have, or when anything happened in it. A member that has
/// nothing to say is empty, off or null.
pub(crate) fn through_a_thread(view: Environment) -> Environment {
    Environment {
        can_declare_lost: false,
        temporary: None,
        workspace_incarnation: view.workspace_incarnation,
        data_sources: None,
        runtime_changes: None,
        removal: None,
        inspection: None,
        base: view.base,
        org_id: view.org_id,
        machine: None,
        // Whether its network is restricted and in force, without the places its list comes from.
        network: super::network::cut(view.network),
        home: view.home,
        default_for: view.default_for,
        config_trust: Vec::new(),
        threads: view.threads,
        threads_hidden: view.threads_hidden,
        reach: 0,
        workspace_root: String::new(),
        workspace_storage: None,
        config: json!({}),
        host: None,
        mounts: Vec::new(),
        exec: EnvironmentExec {
            offers: Vec::new(),
            orch_tools: true,
            machine_offers: None,
        },
        mcp_found: Vec::new(),
        index: view.index,
        index_reason: view.index_reason,
        template: None,
        generation: 0,
        updates: Vec::new(),
        update_required: false,
        connector_update: None,
        remote_write: false,
        terminal: false,
        browser: false,
        vnc: false,
        support: EnvironmentSupport::nothing_found(),
        granted_capabilities: Vec::new(),
        status_detail: None,
        facts: json!({}),
        // How it is held in is part of its configuration: nothing of it either.
        isolation: None,
        manifest_at: None,
        last_activity_at: None,
        idle_stop_at: None,
        unused_since: None,
        remove_at: None,
        remove_reason: None,
        job: view.job,
        seq: view.seq,
        revision: view.revision,
        access: view.access,
    }
}
