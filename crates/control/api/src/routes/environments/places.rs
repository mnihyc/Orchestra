//! Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage.
//!
//! A host is listed with what it has, what the units on it declare they need, what is left, whether
//! it can be chosen and why not, what it warns about, and whether it is what the scheduler would
//! choose; the figures come from the engine ([`orch_engine::places`]), which also decides what a
//! request that makes an environment does with them. The environments are listed as places a thread
//! could be put into: those the caller manages, with what they run on and who works there.

use super::clean_config;
use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use crate::routes::resources::LIST_LIMIT;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::environment::{
    Environment, EnvironmentReason, EnvironmentStatus, PlaceEnvironment, PlaceFacts, PlaceHost,
    PlaceListQuery,
};
use orch_authz::nodes::Page;
use orch_authz::{Policy, resources};
use orch_db::queries::resources::hosting as host_rows;
use orch_engine::places::{self, Ask, IDLE_MINUTES_DEFAULT};
use orch_settings::{Place as SettingsPlace, get as setting};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json, Time};
use serde_json::json;

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    crate::routes::resources::environment_places::mount(registry);
}

/// Host choices from stored reports and the caller's existing connection, without listing environments.
pub(crate) async fn host_choices(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    query: &PlaceListQuery,
    now: Time,
) -> ApiResult<Vec<PlaceHost>> {
    let request = prepare_request(conn, home, query).await?;
    let hosts = prepare_hosts(conn, principal, home, &request, now).await?;
    render_hosts(conn, hosts).await
}

/// The existing typed configuration/template input, before scope or host selection.
pub(crate) struct Request {
    pub own: Json,
    pub template: host_rows::TemplateState,
}

pub(crate) async fn prepare_request(
    conn: &mut orch_db::Conn,
    home: Id,
    query: &PlaceListQuery,
) -> ApiResult<Request> {
    let own: Json = match &query.config {
        Some(text) => serde_json::from_str(text)
            .map_err(|_| invalid("config", "the environment's own values, as JSON text"))?,
        None => json!({}),
    };
    clean_config("config", &own)?;
    let template_id: Id = match query.template_id {
        Some(id) => id,
        None => setting(conn, SettingsPlace::Node(home), keys::ENV_TEMPLATE_ID)
            .await
            .map_err(ApiError::internal)?
            .ok_or_else(|| invalid("template_id", "name a template: none is set for this place"))?,
    };
    let template = host_rows::template(conn, template_id)
        .await?
        .filter(|template| template.deleted_at.is_none())
        .ok_or_else(ApiError::not_found)?;
    Ok(Request { own, template })
}

/// Native resolved planner operands, shared by all host-choice readers.
pub(crate) struct HostChoiceInput {
    pub wanted: places::Wanted,
    pub minutes_idle: i64,
}

pub(crate) async fn prepare_host_choices(
    conn: &mut orch_db::Conn,
    home: Id,
    query: &PlaceListQuery,
) -> ApiResult<HostChoiceInput> {
    let request = prepare_request(conn, home, query).await?;
    resolve_request(conn, home, &request).await
}

async fn resolve_request(
    conn: &mut orch_db::Conn,
    home: Id,
    request: &Request,
) -> ApiResult<HostChoiceInput> {
    let wanted = places::wanted(conn, home, request.template.clone(), &request.own)
        .await
        .map_err(ApiError::internal)?;
    let minutes_idle =
        places::resolved_for_new(conn, home, &request.own, keys::ENV_RECYCLE_IDLE_MINUTES)
            .await
            .map_err(ApiError::internal)?
            .and_then(|value| value.as_i64())
            .unwrap_or(IDLE_MINUTES_DEFAULT);
    Ok(HostChoiceInput {
        wanted,
        minutes_idle,
    })
}

pub(crate) async fn prepare_hosts(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    request: &Request,
    now: Time,
) -> ApiResult<Vec<places::HostPlace>> {
    prepare_hosts_inner(conn, principal, home, request, now, None).await
}

pub(crate) async fn prepare_hosts_among(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    request: &Request,
    now: Time,
    selected: &[Id],
) -> ApiResult<Vec<places::HostPlace>> {
    prepare_hosts_inner(conn, principal, home, request, now, Some(selected)).await
}

async fn prepare_hosts_inner(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    request: &Request,
    now: Time,
    selected: Option<&[Id]>,
) -> ApiResult<Vec<places::HostPlace>> {
    let input = resolve_request(conn, home, request).await?;
    select_hosts(
        conn,
        principal,
        home,
        &input,
        now,
        selected.map(HostChoiceSet::Window),
    )
    .await
}

pub(crate) async fn select_host_choices(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    input: &HostChoiceInput,
    now: Time,
    selected: Option<&[Id]>,
) -> ApiResult<Vec<places::HostPlace>> {
    select_hosts(
        conn,
        principal,
        home,
        input,
        now,
        selected.map(HostChoiceSet::Capped),
    )
    .await
}

enum HostChoiceSet<'a> {
    Capped(&'a [Id]),
    Window(&'a [Id]),
}

async fn select_hosts(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    input: &HostChoiceInput,
    now: Time,
    selected: Option<HostChoiceSet<'_>>,
) -> ApiResult<Vec<places::HostPlace>> {
    let ask = Ask {
        home,
        wanted: &input.wanted,
        current: None,
        minutes_idle: input.minutes_idle,
        now: now.into(),
    };
    match selected {
        Some(HostChoiceSet::Capped(ids)) => {
            places::host_places_selected(conn, principal, &ask, ids).await
        }
        Some(HostChoiceSet::Window(ids)) => {
            places::host_places_among(conn, principal, &ask, ids).await
        }
        None => places::host_places(conn, principal, &ask).await,
    }
    .map_err(ApiError::internal)
}

pub(crate) async fn render_hosts(
    conn: &mut orch_db::Conn,
    mut hosts: Vec<places::HostPlace>,
) -> ApiResult<Vec<PlaceHost>> {
    name_host_choices(conn, &mut hosts, None).await?;
    Ok(hosts.into_iter().map(|host| host.place).collect())
}

pub(crate) async fn name_host_choices(
    conn: &mut orch_db::Conn,
    hosts: &mut [places::HostPlace],
    bounds: Option<&crate::routes::access::page::NameBounds>,
) -> ApiResult<()> {
    let host_ids: Vec<Id> = hosts.iter().map(|place| place.place.host.id).collect();
    let addresses =
        crate::routes::resources::view::addresses_with_names(conn, &host_ids, bounds).await?;
    let owners: Vec<Id> = hosts.iter().filter_map(|place| place.owner).collect();
    let people = refusals::user_refs(conn, &owners).await?;
    for place in hosts {
        place.place.host.address = addresses
            .get(&place.place.host.id)
            .cloned()
            .unwrap_or_default();
        place.place.owner = place.owner.and_then(|owner| people.get(&owner).cloned());
    }
    Ok(())
}

/// The original manageable set with an optional factual selection before its native bound.
pub(crate) async fn manageable_environment_ids(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    selected: Option<&[Id]>,
) -> ApiResult<Vec<Id>> {
    if let Some(ids) = selected {
        let mut found = resources::effective_among(conn, principal, ids, Policy::default())
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter(|each| {
                each.held
                    .iter()
                    .any(|held| held.permission == Permission::EnvironmentManage)
            })
            .map(|each| each.object.id)
            .collect::<Vec<_>>();
        found.sort_unstable();
        found.truncate(usize::try_from(LIST_LIMIT).unwrap_or(usize::MAX));
        return Ok(found);
    }
    let seen = resources::visible(
        conn,
        principal,
        ResourceKind::Environment,
        Page {
            after: None,
            limit: LIST_LIMIT,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let managed: Vec<Id> = seen
        .items
        .iter()
        .filter(|each| {
            each.held
                .iter()
                .any(|held| held.permission == Permission::EnvironmentManage)
        })
        .map(|each| each.resource.id)
        .collect();
    Ok(managed)
}

/// An environment the caller manages, as a place.
pub(crate) fn place_of(view: Environment) -> PlaceEnvironment {
    let mut reasons = Vec::new();
    if matches!(
        view.base.status,
        EnvironmentStatus::Offline
            | EnvironmentStatus::Failed
            | EnvironmentStatus::Lost
            | EnvironmentStatus::Removed
    ) {
        reasons.push(EnvironmentReason::Offline);
    }
    if view.base.status == EnvironmentStatus::Disabled {
        reasons.push(EnvironmentReason::Disabled);
    }
    if view.remove_at.is_some() {
        reasons.push(EnvironmentReason::Removing);
    }
    let mut topics: Vec<orch_api_contract::nodes::NodeRef> = Vec::new();
    for thread in &view.threads {
        if !topics.iter().any(|known| known.id == thread.topic.id) {
            topics.push(thread.topic.clone());
        }
    }
    PlaceEnvironment {
        facts: PlaceFacts {
            os: view
                .facts
                .get("os")
                .and_then(Json::as_str)
                .unwrap_or_default()
                .to_owned(),
            cpus: view.facts.get("cpus").and_then(Json::as_i64),
            memory_mb: view.facts.get("memory_mb").and_then(Json::as_i64),
        },
        threads: i64::try_from(view.threads.len()).unwrap_or(i64::MAX) + view.threads_hidden,
        topics,
        topics_hidden: 0,
        mounts: Vec::new(),
        usable: reasons.is_empty(),
        reasons,
        current: false,
        host: view.host,
        machine: view.machine,
        environment: view.base,
    }
}

pub(crate) struct EnvironmentSource {
    pub environment: Id,
    pub host: Option<Id>,
    pub machine: Option<Id>,
    pub topics: std::collections::BTreeSet<Id>,
}

pub(crate) struct PreparedEnvironments {
    inventory: super::PreparedInventory,
    pub sources: Vec<EnvironmentSource>,
    pub disclosure: crate::routes::nodes::view::ReferenceDisclosure,
}

/// The original managed choices, with actual per-row reference associations before rendering.
pub(crate) async fn prepare_environments(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    seen: &[resources::Seen],
) -> ApiResult<PreparedEnvironments> {
    let inventory = super::prepare_inventory(ctx, conn, principal, seen).await?;
    let mut nodes = std::collections::BTreeSet::new();
    for thread in &inventory.threads {
        nodes.extend([thread.thread_id, thread.topic_id]);
    }
    let disclosure = crate::routes::nodes::view::reference_disclosure(
        conn,
        principal,
        &nodes.iter().copied().collect::<Vec<_>>(),
        Policy::default(),
    )
    .await?;
    let live = orch_db::queries::resources::routes::live_machine_ids(
        conn,
        inventory.physical_machine_ids(),
    )
    .await?;
    let sources = inventory
        .rows
        .iter()
        .map(|row| EnvironmentSource {
            environment: row.id,
            host: inventory
                .hosted
                .iter()
                .find(|(environment, _, _)| *environment == row.id)
                .map(|(_, host, _)| *host),
            machine: inventory
                .machine_facts
                .get(&row.id)
                .and_then(|facts| facts.machine_id)
                .filter(|id| live.contains(id)),
            topics: inventory
                .threads
                .iter()
                .filter(|thread| {
                    thread.environment_id == row.id
                        && disclosure.names.contains(&thread.thread_id)
                        && disclosure.names.contains(&thread.topic_id)
                })
                .map(|thread| thread.topic_id)
                .collect(),
        })
        .collect();
    Ok(PreparedEnvironments {
        inventory,
        sources,
        disclosure,
    })
}

impl PreparedEnvironments {
    pub(crate) async fn render(
        &self,
        ctx: &Ctx,
        conn: &mut orch_db::Conn,
        principal: &orch_authz::Principal,
        limited: bool,
        hosts: &std::collections::BTreeSet<Id>,
        machines: &std::collections::BTreeSet<Id>,
        topics: &std::collections::BTreeSet<Id>,
    ) -> ApiResult<Vec<PlaceEnvironment>> {
        let ids = self
            .sources
            .iter()
            .flat_map(|source| source.topics.iter().copied())
            .collect::<Vec<_>>();
        let refs = if limited {
            crate::routes::nodes::view::refs_with_name_sources(
                conn,
                principal,
                &ids,
                Policy::default(),
                topics,
            )
            .await?
        } else {
            crate::routes::nodes::view::refs(conn, principal, &ids, Policy::default()).await?
        };
        let mut result = Vec::new();
        for view in super::show_inventory_prepared(ctx, conn, principal, &self.inventory).await? {
            let id = view.base.id;
            let mut place = place_of(view);
            let source = self
                .sources
                .iter()
                .find(|source| source.environment == id)
                .ok_or_else(ApiError::not_found)?;
            if limited {
                if source.host.is_some_and(|host| !hosts.contains(&host)) {
                    place.host = None;
                }
                if source
                    .machine
                    .is_some_and(|machine| !machines.contains(&machine))
                {
                    place.machine = None;
                }
                for topic in &mut place.topics {
                    *topic = refs
                        .get(&topic.id)
                        .cloned()
                        .ok_or_else(ApiError::not_found)?;
                }
            }
            result.push(place);
        }
        Ok(result)
    }

    pub(crate) fn preserves(
        &self,
        fresh: &Self,
        hosts: &std::collections::BTreeSet<Id>,
        machines: &std::collections::BTreeSet<Id>,
    ) -> bool {
        self.sources.iter().all(|old| {
            fresh
                .sources
                .iter()
                .find(|now| now.environment == old.environment)
                .is_some_and(|now| {
                    old.host
                        .filter(|host| hosts.contains(host))
                        .is_none_or(|host| now.host == Some(host))
                        && old
                            .machine
                            .filter(|machine| machines.contains(machine))
                            .is_none_or(|machine| now.machine == Some(machine))
                        && old.topics.is_subset(&now.topics)
                })
        })
    }
}
