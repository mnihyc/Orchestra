//! Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage.
//!
//! A host is listed with what it has, what the units on it declare they need, what is left, whether
//! it can be chosen and why not, what it warns about, and whether it is what the scheduler would
//! choose; the figures come from the engine ([`orch_engine::places`]), which also decides what a
//! request that makes an environment does with them. The environments are listed as places a thread
//! could be put into: those the caller manages, with what they run on and who works there.

use super::{clean_config, environments_of};
use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::nodes::invalid;
use crate::routes::resources::LIST_LIMIT;
use crate::routes::resources::view::addresses;
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_api_contract::environment::{
    self as api, Environment, EnvironmentReason, EnvironmentStatus, Place, PlaceEnvironment,
    PlaceFacts, PlaceHost, PlaceListQuery,
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
    registry.on(api::NodePlaceList, node_places);
}

/// Where an environment made for an org or a project could be, for the caller.
async fn node_places(ctx: Ctx, query: PlaceListQuery, (): ()) -> ApiResult<Vec<Place>> {
    let node = ctx.id("id")?;
    places_for(&ctx, node, &query).await
}

/// Where an environment made for `home` could be, for the caller: the hosts they may use, then the
/// environments they manage.
pub(crate) async fn places_for(
    ctx: &Ctx,
    home: Id,
    query: &PlaceListQuery,
) -> ApiResult<Vec<Place>> {
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(ctx, &mut conn).await?;
    let hosts = host_choices(&mut conn, &principal, home, query, ctx.now.into()).await?;
    let mut found: Vec<Place> = hosts.into_iter().map(Place::Host).collect();
    found.extend(
        manageable_environments(ctx, &mut conn, &principal)
            .await?
            .into_iter()
            .map(Place::Environment),
    );
    Ok(found)
}

/// Host choices from stored reports and the caller's existing connection, without listing environments.
pub(crate) async fn host_choices(
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
    home: Id,
    query: &PlaceListQuery,
    now: Time,
) -> ApiResult<Vec<PlaceHost>> {
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
    let wanted = places::wanted(conn, home, template, &own)
        .await
        .map_err(ApiError::internal)?;
    let minutes_idle = places::resolved_for_new(conn, home, &own, keys::ENV_RECYCLE_IDLE_MINUTES)
        .await
        .map_err(ApiError::internal)?
        .and_then(|value| value.as_i64())
        .unwrap_or(IDLE_MINUTES_DEFAULT);
    let mut hosts = places::host_places(
        conn,
        principal,
        &Ask {
            home,
            wanted: &wanted,
            current: None,
            minutes_idle,
            now: now.into(),
        },
    )
    .await
    .map_err(ApiError::internal)?;
    // The people and the addresses are read by whoever can read them: the engine only knows ids.
    let host_ids: Vec<Id> = hosts.iter().map(|place| place.place.host.id).collect();
    let addresses = addresses(conn, &host_ids).await?;
    let owners: Vec<Id> = hosts.iter().filter_map(|place| place.owner).collect();
    let people = refusals::user_refs(conn, &owners).await?;
    for place in &mut hosts {
        place.place.host.address = addresses
            .get(&place.place.host.id)
            .cloned()
            .unwrap_or_default();
        place.place.owner = place.owner.and_then(|owner| people.get(&owner).cloned());
    }
    Ok(hosts.into_iter().map(|place| place.place).collect())
}

/// The environments the caller manages, as places a thread could be put into.
async fn manageable_environments(
    ctx: &Ctx,
    conn: &mut orch_db::Conn,
    principal: &orch_authz::Principal,
) -> ApiResult<Vec<PlaceEnvironment>> {
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
    let views = environments_of(ctx, conn, principal, &managed).await?;
    Ok(views.into_iter().map(place_of).collect())
}

/// An environment the caller manages, as a place.
fn place_of(view: Environment) -> PlaceEnvironment {
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
