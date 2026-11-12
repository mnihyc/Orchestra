//! Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage.
//!
//! A host is listed with what it has, what the units on it declare they need, what is left, whether
//! it can be chosen and why not, what it warns about, and whether it is what the scheduler would
//! choose; the figures come from the engine ([`orch_engine::places`]), which also decides what a
//! request that makes an environment does with them. The environments are listed as places a thread
//! could be put into: those the caller manages, with what they run on and who works there.

use super::clean_config;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use crate::registry::Ctx;

use crate::registry::Registry;

use crate::routes::nodes::invalid;

use crate::routes::resources::LIST_LIMIT;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::environment::Environment;

use orch_api_contract::environment::EnvironmentReason;

use orch_api_contract::environment::EnvironmentStatus;

use orch_api_contract::environment::PlaceEnvironment;

use orch_api_contract::environment::PlaceFacts;

use orch_api_contract::environment::PlaceHost;

use orch_api_contract::environment::PlaceListQuery;

use orch_authz::nodes::Page;

use orch_authz::Policy;

use orch_authz::resources;

use orch_db::queries::resources::hosting as host_rows;

use orch_engine::places;

use orch_engine::places::Ask;

use orch_engine::places::IDLE_MINUTES_DEFAULT;

use orch_settings::Place as SettingsPlace;

use orch_settings::get as setting;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use orch_types::Json;

use orch_types::Time;

use serde_json::json;

pub use orch_api_base::routes::environments::places::Request;

pub use orch_api_base::routes::environments::places::HostChoiceInput;

pub use orch_api_base::routes::environments::places::EnvironmentSource;

pub use orch_api_base::routes::environments::places::PreparedEnvironments;

pub use orch_api_base::routes::environments::places::EnvironmentNames;
/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    crate::routes::resources::environment_places::mount(registry);
}

/// Host choices from stored reports and the caller's existing connection, without listing environments.
pub(crate) async fn host_choices(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    query: &PlaceListQuery,
    now: Time,
) -> ApiResult<Vec<PlaceHost>> {
    let request = prepare_request(conn, home, query).await?;
    let mut hosts = prepare_hosts(conn, principal, home, &request, now).await?;
    group_host_choices(app, conn, principal, &mut hosts, None, None).await?;
    render_hosts(conn, hosts).await
}
orch_api_base::__api_bind_0165bcdf3c!();

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
orch_api_base::__api_bind_84d36c5654!();

pub(crate) async fn prepare_host_choices(
    conn: &mut orch_db::Conn,
    home: Id,
    query: &PlaceListQuery,
) -> ApiResult<HostChoiceInput> {
    let request = prepare_request(conn, home, query).await?;
    resolve_request(conn, home, &request).await
}
orch_api_base::__api_bind_a7a7119068!();

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
orch_api_base::__api_bind_08e2411db6!();

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
orch_api_base::__api_bind_804a7d87b5!();

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
orch_api_base::__api_bind_f09a39d7c1!();

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
orch_api_base::__api_bind_525d07a808!();

/// Add display metadata only after the native planner has chosen and judged each Host.
pub(crate) async fn group_host_choices(
    app: &crate::state::AppState,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    hosts: &mut [places::HostPlace],
    names: Option<&crate::routes::access::page::NameBounds>,
    machines: Option<&std::collections::BTreeSet<Id>>,
) -> ApiResult<()> {
    let subjects = hosts
        .iter()
        .map(|host| ("host".to_owned(), host.host.id))
        .collect::<Vec<_>>();
    let grouped = crate::routes::resources::grouping::Prepared::select(conn, &subjects)
        .await?
        .render(app, conn, principal, names, machines)
        .await?;
    for host in hosts {
        let mut grouping = grouped
            .get(&("host".to_owned(), host.host.id))
            .cloned()
            .unwrap_or_default();
        if machines.is_none_or(|ids| ids.contains(&host.host.machine_id)) {
            grouping.availability.push(
                orch_api_contract::resource_groups::ResourceMachineAvailability {
                    machine_id: host.host.machine_id,
                    available: host.place.usable,
                    reason: host
                        .place
                        .reasons
                        .first()
                        .map(serde_json::to_value)
                        .transpose()
                        .map_err(ApiError::internal)?
                        .and_then(|value| value.as_str().map(str::to_owned)),
                },
            );
        }
        host.place.grouping = grouping;
    }
    Ok(())
}
orch_api_base::__api_bind_1c0f1e698f!();

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
        found.truncate(LIST_LIMIT);
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
orch_api_base::__api_bind_e34cda7dae!();

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
        grouping: orch_api_contract::resource_groups::ResourceGrouping {
            home: Some(view.home.clone()),
            home_path: Vec::new(),
            origin_machine: view.machine.clone(),
            availability: view
                .machine
                .as_ref()
                .map(
                    |machine| orch_api_contract::resource_groups::ResourceMachineAvailability {
                        machine_id: machine.id,
                        available: reasons.is_empty(),
                        reason: reasons
                            .first()
                            .and_then(|reason| serde_json::to_value(reason).ok())
                            .and_then(|value| value.as_str().map(str::to_owned)),
                    },
                )
                .into_iter()
                .collect(),
        },
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
orch_api_base::__api_bind_3cd543019e!();

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
orch_api_base::__api_bind_75ad864d9c!();
