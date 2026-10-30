//! Permission-page dependencies resolved from the same defaults, model settings and giving rules as work.

use crate::middleware::authorization::refusals;
use crate::middleware::requests::{ApiError, ApiResult};
use orch_access_contract::pages::PageKind;
use orch_access_contract::permissions::{ObjectKind, Permission, ResourceKind};
use orch_api_contract::access::{Dependency, DependencyObject, DependencyOpen};
use orch_authz::depends::{self, Open, Starting};
use orch_authz::pages::{PageView, Target};
use orch_authz::resources::{Resource, read_into, site_of};
use orch_authz::usable::{Policies, ResourcePolicy};
use orch_authz::{Policy, Principal, requests};
use orch_db::Conn;
use orch_db::queries::{resources, threads};
use orch_settings::{Place, get};
use orch_settings_contract::registry::keys;
use orch_types::Id;

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

/// What a live page's permissions need elsewhere, with no invented defaults or ineligible decider names.
pub(super) async fn of_page(
    conn: &mut Conn,
    principal: &Principal,
    page: &PageView,
) -> ApiResult<Vec<Dependency>> {
    if page.page != PageKind::Live {
        return Ok(Vec::new());
    }
    let (found, policies) = match page.target {
        Target::Live(id) if page.kind == Some(orch_access_contract::roles::NodeKind::Thread) => {
            let Some(thread) = threads::thread(conn, id).await? else {
                return Ok(Vec::new());
            };
            let settings = orch_engine::resolved::load(conn, id)
                .await
                .map_err(ApiError::internal)?;
            let Some((provider, model)) = thread.provider_id.zip(settings.model.as_deref()) else {
                return Ok(Vec::new());
            };
            let (facts, policies) = current_resources(conn, &[provider]).await?;
            if current_choice(&facts, Some(provider), ResourceKind::Provider).is_none() {
                return Ok(Vec::new());
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
                return Ok(Vec::new());
            };
            let Some(profile) = provider
                .agent_id
                .filter(|_| provider.kind == "subscription")
            else {
                return Ok(Vec::new());
            };
            let (facts, policies) = current_resources(conn, &[object.id, profile]).await?;
            if current_choice(&facts, Some(object.id), ResourceKind::Provider).is_none()
                || current_choice(&facts, Some(profile), ResourceKind::Agent).is_none()
            {
                return Ok(Vec::new());
            }
            let found = depends::of_provider(conn, principal, object.id, profile, &policies)
                .await
                .map_err(ApiError::internal)?;
            (found, policies)
        }
        _ => return Ok(Vec::new()),
    };
    let owners: Vec<_> = found.iter().filter_map(|each| each.owner).collect();
    let names = refusals::user_refs(conn, &owners).await?;
    let through: Vec<_> = found.iter().filter_map(|each| each.through).collect();
    let places =
        crate::routes::nodes::view::refs(conn, principal, &through, Policy::default()).await?;
    let mut result = Vec::new();
    for each in found {
        let home = resources::resource_facts(conn, &[each.object.id])
            .await?
            .first()
            .and_then(|row| row.home_id);
        let givers = requests::givers_for(
            conn,
            principal,
            each.object,
            &[each.permission],
            each.options.as_ref(),
            policies.at(home),
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
                name: each.name,
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
