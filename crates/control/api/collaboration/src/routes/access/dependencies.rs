//! Permission-page dependencies resolved from the same defaults, model settings and giving rules as work.

use super::page::NameBounds;

use crate::middleware::authorization::refusals;

use crate::middleware::requests::ApiError;

use crate::middleware::requests::ApiResult;

use orch_access_contract::decisions::ObjectRef;

use orch_access_contract::pages::PageKind;

use orch_access_contract::permissions::ObjectKind;

use orch_access_contract::permissions::Permission;

use orch_access_contract::permissions::ResourceKind;

use orch_api_contract::access::Dependency;

use orch_api_contract::access::DependencyObject;

use orch_api_contract::access::DependencyOpen;

use orch_authz::depends;

use orch_authz::depends::Open;

use orch_authz::depends::Starting;

use orch_authz::pages::PageView;

use orch_authz::pages::Target;

use orch_authz::resources::Resource;

use orch_authz::resources::read_into;

use orch_authz::resources::site_of;

use orch_authz::usable::Policies;

use orch_authz::usable::ResourcePolicy;

use orch_authz::Policy;

use orch_authz::Principal;

use orch_authz::requests;

use orch_db::Conn;

use orch_db::queries::resources;

use orch_db::queries::threads;

use orch_settings::Place;

use orch_settings::get;

use orch_settings_contract::registry::keys;

use orch_types::Id;

use std::collections::HashMap;
/// A configured choice can name a missing resource or one retained after removal.
/// Neither is a live access dependency to offer. This projection does not replace the
/// strict dependency checks used by work, copying or offers, or substitute another default.
async fn current_resources(conn: &mut Conn, ids: &[Id]) -> ApiResult<(Vec<Resource>, Policies)> {
    let mut graph = orch_authz::engine::Graph::default();
    let mut facts = read_into(conn, &mut graph, ids)
        .await
        .map_err(ApiError::internal)?;
    facts.retain(|resource| !resource.deleted && site_of(&graph, resource).is_some());
    let mut homes: Vec<_> = facts.iter().map(|row| row.home).collect();
    homes.sort_unstable();
    homes.dedup();
    let mut policies = Policies::default();
    for home in homes {
        let sharing = super::subject::policy_at_place(conn, home).await?;
        let shared_subscriptions = get(
            conn,
            home.map_or(Place::System, Place::Node),
            keys::PROVIDERS_SUBSCRIPTION_SHARING,
        )
        .await
        .map_err(ApiError::internal)?;
        let use_policy = ResourcePolicy {
            owners_name_outsiders: sharing.owners_name_outsiders,
            shared_subscriptions,
        };
        policies = match home {
            Some(home) => policies
                .at_home(home, use_policy)
                .with_sharing(Some(home), sharing),
            None => Policies::everywhere(use_policy).with_sharing(None, sharing),
        };
    }
    Ok((facts, policies))
}

fn current_choice(facts: &[Resource], id: Option<Id>, kind: ResourceKind) -> Option<Id> {
    id.filter(|id| {
        facts
            .iter()
            .any(|resource| resource.id == *id && resource.kind == kind)
    })
}

async fn id_setting(conn: &mut Conn, node: Id, key: &str) -> ApiResult<Option<Id>> {
    get(conn, Place::Node(node), key)
        .await
        .map_err(ApiError::internal)
}

/// The ordinary dependency decisions retained until the operation admits their sources.
/// Payload-bearing names stay private and are not logged or serialized by this carrier.
#[derive(Clone, Default)]
pub(crate) struct PreparedDependencies {
    found: Vec<depends::Dependency>,
    policies: Policies,
    homes: HashMap<Id, Option<Id>>,
}

impl PreparedDependencies {
    /// The real resources whose existing dependency payload will be shown.
    pub(crate) fn objects(&self) -> impl Iterator<Item = ObjectRef> + '_ {
        self.found.iter().map(|each| each.object)
    }

    /// The places used only for optional, ordinarily redacted dependency provenance.
    pub(crate) fn nodes(&self) -> impl Iterator<Item = Id> + '_ {
        self.found.iter().filter_map(|each| each.through)
    }
}

/// What a live page's permissions need elsewhere, with the ordinary projection unchanged.
pub(super) async fn of_page(
    conn: &mut Conn,
    principal: &Principal,
    page: &PageView,
) -> ApiResult<Vec<Dependency>> {
    let prepared = prepare_page(conn, principal, page).await?;
    project_page(conn, principal, &prepared, None).await
}

/// Resolves the actual configured dependencies before their public names are projected.
/// These are ordinary page-authorized decisions; preparing them grants no extra resource View.
pub(crate) async fn prepare_page(
    conn: &mut Conn,
    principal: &Principal,
    page: &PageView,
) -> ApiResult<PreparedDependencies> {
    if page.page != PageKind::Live {
        return Ok(PreparedDependencies::default());
    }
    let (found, policies) = match page.target {
        Target::Live(id) if page.kind == Some(orch_access_contract::roles::NodeKind::Thread) => {
            let Some(thread) = threads::thread(conn, id).await? else {
                return Ok(PreparedDependencies::default());
            };
            let settings = orch_engine::resolved::load(conn, id)
                .await
                .map_err(ApiError::internal)?;
            let Some((provider, model)) = thread.provider_id.zip(settings.model.as_deref()) else {
                return Ok(PreparedDependencies::default());
            };
            let (facts, policies) = current_resources(conn, &[provider]).await?;
            if current_choice(&facts, Some(provider), ResourceKind::Provider).is_none() {
                return Ok(PreparedDependencies::default());
            }
            let found = depends::of_thread(
                conn,
                principal,
                id,
                provider,
                model,
                settings.effort.as_deref(),
                &policies,
            )
            .await
            .map_err(ApiError::internal)?;
            (found, policies)
        }
        Target::Live(id) => {
            let how: Option<String> = get(conn, Place::Node(id), keys::ENV_THREAD_DEFAULT)
                .await
                .map_err(ApiError::internal)?;
            let environment = if how.as_deref() == Some("fresh") {
                None
            } else {
                id_setting(conn, id, keys::ENV_DEFAULT_ID).await?
            };
            let starting = Starting {
                provider: id_setting(conn, id, keys::AGENT_DEFAULT_PROVIDER_ID).await?,
                profile: id_setting(conn, id, keys::AGENT_DEFAULT_AGENT_ID).await?,
                host: if environment.is_none() {
                    id_setting(conn, id, keys::ENV_HOST_ID).await?
                } else {
                    None
                },
                template: if environment.is_none() {
                    id_setting(conn, id, keys::ENV_TEMPLATE_ID).await?
                } else {
                    None
                },
                environment,
            };
            let ids: Vec<_> = [
                starting.provider,
                starting.profile,
                starting.host,
                starting.template,
                starting.environment,
            ]
            .into_iter()
            .flatten()
            .collect();
            let (facts, policies) = current_resources(conn, &ids).await?;
            let starting = Starting {
                provider: current_choice(&facts, starting.provider, ResourceKind::Provider),
                profile: current_choice(&facts, starting.profile, ResourceKind::Agent),
                host: current_choice(&facts, starting.host, ResourceKind::Host),
                template: current_choice(&facts, starting.template, ResourceKind::Template),
                environment: current_choice(
                    &facts,
                    starting.environment,
                    ResourceKind::Environment,
                ),
            };
            let settings = orch_engine::resolved::load(conn, id)
                .await
                .map_err(ApiError::internal)?;
            let found = depends::of_place_with_model(
                conn,
                principal,
                id,
                &starting,
                settings.model.as_deref(),
                settings.effort.as_deref(),
                &policies,
            )
            .await
            .map_err(ApiError::internal)?;
            (found, policies)
        }
        Target::Resource(object) if object.kind == ObjectKind::Provider => {
            let Some(provider) = resources::thread_engine::provider(conn, object.id).await? else {
                return Ok(PreparedDependencies::default());
            };
            let Some(profile) = provider
                .agent_id
                .filter(|_| provider.kind == "subscription")
            else {
                return Ok(PreparedDependencies::default());
            };
            let (facts, policies) = current_resources(conn, &[object.id, profile]).await?;
            if current_choice(&facts, Some(object.id), ResourceKind::Provider).is_none()
                || current_choice(&facts, Some(profile), ResourceKind::Agent).is_none()
            {
                return Ok(PreparedDependencies::default());
            }
            let found = depends::of_provider(conn, principal, object.id, profile, &policies)
                .await
                .map_err(ApiError::internal)?;
            (found, policies)
        }
        _ => return Ok(PreparedDependencies::default()),
    };
    let mut ids: Vec<_> = found.iter().map(|each| each.object.id).collect();
    ids.sort_unstable();
    ids.dedup();
    let homes = if ids.is_empty() {
        HashMap::new()
    } else {
        resources::resource_facts(conn, &ids)
            .await?
            .into_iter()
            .map(|row| (row.id, row.home_id))
            .collect()
    };
    Ok(PreparedDependencies {
        found,
        policies,
        homes,
    })
}

/// Projects the prepared dependencies after their actual full resource sources are admitted.
/// Optional through-node names keep their ordinary visibility and the operation's name bound.
pub(crate) async fn project_page(
    conn: &mut Conn,
    principal: &Principal,
    prepared: &PreparedDependencies,
    bounds: Option<&NameBounds>,
) -> ApiResult<Vec<Dependency>> {
    let found = &prepared.found;
    if found.is_empty() {
        return Ok(Vec::new());
    }
    let owners: Vec<_> = found.iter().filter_map(|each| each.owner).collect();
    let names = refusals::user_refs(conn, &owners).await?;
    let through: Vec<_> = found.iter().filter_map(|each| each.through).collect();
    let places = match bounds {
        Some(bounds) => bounds.refs(conn, principal, &through).await?,
        None => {
            crate::routes::nodes::view::refs(conn, principal, &through, Policy::default()).await?
        }
    };
    let mut result = Vec::new();
    for each in found {
        let home = prepared.homes.get(&each.object.id).copied().flatten();
        let givers = requests::givers_for(
            conn,
            principal,
            each.object,
            &[each.permission],
            each.options.as_ref(),
            prepared.policies.at(home),
        )
        .await
        .map_err(ApiError::internal)?;
        let eligible_owner = each.owner.filter(|owner| {
            (each.can_give && *owner == principal.user())
                || givers.nearest.contains(owner)
                || givers.further.contains(owner)
        });
        result.push(Dependency {
            for_permission: each.for_permission.as_str().to_owned(),
            object: DependencyObject {
                kind: each.object.kind.as_str().to_owned(),
                id: each.object.id,
                name: each.name.clone(),
                owner: eligible_owner.and_then(|owner| names.get(&owner).cloned()),
            },
            permission: each.permission.as_str().to_owned(),
            choice: None,
            options: each
                .options
                .as_ref()
                .and_then(|options| serde_json::to_value(options).ok()),
            open: match each.open {
                Open::All => DependencyOpen::All,
                Open::Part => DependencyOpen::Part,
                Open::Nobody => DependencyOpen::Nobody,
            },
            through: each.through.and_then(|id| places.get(&id).cloned()),
            // Environments have no work-in-a-place holder; their managers are named people.
            can_give: each.can_give && each.permission != Permission::EnvironmentManage,
        });
    }
    Ok(result)
}
