//! Everything the page that adds a provider needs, for the caller, in one answer: the profiles whose harness can hold a sign-in with the ways to sign in, the protocols and what an endpoint of each is asked for, the presets, the machines that may keep a key, and what the policies there say.
//!
//! The page is rendered from what is declared and nothing else: the sign-in methods and the options
//! of a harness are the ones its adapter declared in the descriptor a harness started from the profile
//! last reported, the options of an endpoint are the ones its protocol declares, and the presets are
//! the setting that holds them. A harness the policy `providers.subscription_harnesses` leaves out is
//! listed apart, with the place that says so.

use crate::middleware::authorization::{self, refusals};
use crate::middleware::requests::{ApiError, ApiResult};
use crate::registry::{Ctx, Registry};
use crate::routes::access::page::same;
use crate::routes::nodes::{placed, policy_at, view as node_view};
use crate::routes::resources::LIST_LIMIT;
use crate::routes::resources::providers::{home_policies, refused_making};
use crate::routes::resources::upstreams::PROTOCOLS;
use crate::routes::resources::view::{agent_refs, machine_refs};
use orch_access_contract::permissions::{Permission, ResourceKind};
use orch_agent_contract::descriptor::AgentDescriptor;
use orch_api_contract::binding::ArrangementType;
use orch_api_contract::environment::TemplateRef;
use orch_api_contract::nodes::NodeKind;
use orch_api_contract::resources::{
    self as api, ArrangementConsent, ArrangementConsentForm, HarnessKindName, ProviderPreset,
    ProviderSetup, ProviderSetupPolicy, ProviderSetupProtocol, ProviderSetupRuntimeTemplate,
    ProviderSetupSubscription, Upstream,
};
use orch_authz::nodes::Page;
use orch_authz::ownership::{Basis, Making};
use orch_authz::{Policy, ownership, resources};
use orch_db::queries::environments::routes as env_queries;
use orch_db::queries::resources::routes as queries;
use orch_model_contract::endpoint::standard_options;
use orch_model_contract::protocol::{
    ANTHROPIC_MESSAGES, OPENAI_CHAT, OPENAI_EMBEDDINGS, OPENAI_RESPONSES,
};
use orch_settings::{Place, get as setting, resolve};
use orch_settings_contract::registry::keys;
use orch_types::{Id, Json};
use std::collections::BTreeMap;

/// What a protocol is called where a person chooses one.
fn protocol_title(name: &str) -> &'static str {
    match name {
        OPENAI_RESPONSES => "OpenAI Responses",
        OPENAI_CHAT => "OpenAI Chat Completions",
        ANTHROPIC_MESSAGES => "Anthropic Messages",
        OPENAI_EMBEDDINGS => "OpenAI Embeddings",
        _ => "Other",
    }
}

/// Binds this module's requests.
pub fn mount(registry: &mut Registry) {
    registry.on(api::ProviderSetupGet, setup);
}

/// The descriptor a profile last reported, with what the system sets over it; none when no
/// harness started from the profile has reported one yet.
fn descriptor_of(descriptor: &Json, overrides: &Json) -> Option<AgentDescriptor> {
    let mut merged = descriptor.as_object().cloned()?;
    if merged.is_empty() {
        return None;
    }
    if let Some(set) = overrides.as_object() {
        for (name, value) in set {
            merged.insert(name.clone(), value.clone());
        }
    }
    serde_json::from_value(Json::Object(merged)).ok()
}

/// The types a provider can allow, each with whether it is proposed and whether a harness there can
/// be placed this way.
fn consents(
    proposed: &[ArrangementType],
    placed_as: Option<&[ArrangementType]>,
) -> Vec<ArrangementConsent> {
    [
        (
            ArrangementType::Separated,
            "Nothing: the harness and its tools stay apart.",
        ),
        (
            ArrangementType::KeySeparated,
            "A token that works only at Orch's forwarder.",
        ),
        (
            ArrangementType::CredentialShared,
            "A copy of the credential.",
        ),
        (
            ArrangementType::HandedOver,
            "The credential itself, to one environment at a time.",
        ),
    ]
    .into_iter()
    .map(|(kind, puts)| {
        let leaves = matches!(
            kind,
            ArrangementType::CredentialShared | ArrangementType::HandedOver
        );
        ArrangementConsent {
            kind,
            // Subscription creation defaults to separated when the adapter proposes none;
            // expose that same default only when its descriptor actually supports it.
            allowed: proposed.contains(&kind)
                || proposed.is_empty()
                    && kind == ArrangementType::Separated
                    && placed_as.is_some_and(|types| types.contains(&kind)),
            available: placed_as.is_none_or(|types| types.contains(&kind)),
            puts: puts.to_owned(),
            form: leaves.then(|| ArrangementConsentForm {
                name: "credential".to_owned(),
                title: "Sign-in".to_owned(),
                lifetime_secs: None,
            }),
        }
    })
    .collect()
}

/// The presets of the setting that holds them: a name with its endpoints; a name set to `null`
/// is hidden.
fn presets_of(value: Option<Json>, home: Id) -> Vec<ProviderPreset> {
    let Some(Json::Object(entries)) = value else {
        return Vec::new();
    };
    entries
        .into_iter()
        .filter_map(|(name, entry)| {
            // An endpoint of a preset may leave its options out; the shape has them always.
            let endpoints: Vec<Json> = entry.get("upstreams")?.as_array()?.clone();
            let upstreams: Vec<Upstream> = endpoints
                .into_iter()
                .map(|mut endpoint| {
                    if let Some(members) = endpoint.as_object_mut() {
                        members
                            .entry("options")
                            .or_insert_with(|| serde_json::json!({}));
                    }
                    serde_json::from_value(endpoint).ok()
                })
                .collect::<Option<_>>()?;
            Some(ProviderPreset {
                name,
                note: entry.get("note").and_then(Json::as_str).map(str::to_owned),
                docs_url: entry
                    .get("docs_url")
                    .and_then(Json::as_str)
                    .map(str::to_owned),
                upstreams,
                template: entry
                    .get("template")
                    .cloned()
                    .and_then(|value| serde_json::from_value::<api::TemplateCreateBody>(value).ok())
                    .map(|mut recipe| {
                        recipe.home_id = Some(home);
                        recipe
                    }),
                model: entry
                    .get("model")
                    .cloned()
                    .and_then(|value| serde_json::from_value(value).ok()),
                dimensions: entry
                    .get("dimensions")
                    .and_then(Json::as_u64)
                    .and_then(|n| u32::try_from(n).ok()),
            })
        })
        .collect()
}

/// Gives everything the page that adds a provider needs, for the caller and for the org that
/// would be its home.
async fn setup(ctx: Ctx, query: api::ProviderSetupGetQuery, (): ()) -> ApiResult<ProviderSetup> {
    let org = query.org_id;
    let mut conn = ctx.app.db.acquire().await?;
    let principal = authorization::principal(&ctx, &mut conn).await?;
    let provider = if let Some(id) = query.provider_id {
        authorization::require_resource(
            &mut conn,
            &principal,
            ResourceKind::Provider,
            id,
            Permission::ProviderManage,
            Policy::default(),
        )
        .await?;
        let row = queries::providers(&mut conn, &[id])
            .await?
            .pop()
            .ok_or_else(ApiError::not_found)?;
        if row.home_id != org {
            return Err(crate::routes::nodes::invalid(
                "org_id",
                "is this provider's home org",
            ));
        }
        Some(row)
    } else {
        None
    };
    let (_, kind) = placed(&mut conn, org).await?;
    if kind != NodeKind::Org {
        return Err(ApiError::not_found());
    }
    let policies = home_policies(&mut conn, org).await?;
    let policy = policy_at(&mut conn, org).await?;
    if provider.is_none() {
        match ownership::may_make(
            &mut conn,
            &principal,
            ResourceKind::Provider,
            Some(org),
            &policies.members_make,
            policy,
        )
        .await
        {
            Ok(Making::Allowed(Basis::Member | Basis::Rules(_) | Basis::SystemAdmin)) => {}
            Ok(Making::Refused(why)) => return Err(refused_making(&mut conn, org, why).await),
            Err(error) => return Err(refusals::problem(&mut conn, error).await),
        }
    }
    // The profiles the caller may use, with what their adapters declare.
    let page = Page {
        after: None,
        limit: LIST_LIMIT,
    };
    let seen = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Agent,
        page,
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let usable: Vec<Id> = seen
        .items
        .iter()
        .filter(|each| {
            !each.resource.disabled
                && each.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::AgentUse | Permission::AgentManage
                    )
                })
        })
        .map(|each| each.resource.id)
        .collect();
    let rows = orch_db::queries::resources::routes::agents(&mut conn, &usable).await?;
    let refs = agent_refs(&ctx.app, &mut conn, &usable, &[]).await?;
    // Where the policies that govern subscriptions are set.
    let harnesses = resolve(
        &mut conn,
        Place::Node(org),
        keys::PROVIDERS_SUBSCRIPTION_HARNESSES,
    )
    .await
    .map_err(ApiError::internal)?;
    let sharing = resolve(
        &mut conn,
        Place::Node(org),
        keys::PROVIDERS_SUBSCRIPTION_SHARING,
    )
    .await
    .map_err(ApiError::internal)?;
    let scopes: Vec<Id> = [&harnesses, &sharing]
        .iter()
        .filter_map(|view| view.resolved.source.node)
        .collect();
    let scope_refs = node_view::refs(&mut conn, &principal, &scopes, Policy::default()).await?;
    let scope_of = |view: &orch_settings::SettingView| {
        view.resolved
            .source
            .node
            .and_then(|node| scope_refs.get(&node).cloned())
    };
    // The template a provider's runtime would be made from.
    let template_id: Id = setting::<Id>(&mut conn, Place::Node(org), keys::RUNTIME_TEMPLATE_ID)
        .await
        .map_err(ApiError::internal)?
        .unwrap_or(orch_db::seeds::PROVIDER_RUNTIME_TEMPLATE_ID);
    let named = env_queries::template_names(&mut conn, &[template_id]).await?;
    let template = TemplateRef {
        id: template_id,
        name: named.first().map_or_else(
            || "Provider runtime".to_owned(),
            |(_, name, _)| name.clone(),
        ),
    };
    let mut subscriptions = Vec::new();
    let mut unreported = Vec::new();
    let mut unavailable = Vec::new();
    let mut withheld = Vec::new();
    let mut unshared = Vec::new();
    let mut speaking: BTreeMap<String, Vec<api::AgentRef>> = BTreeMap::new();
    for row in &rows {
        let Some(agent) = refs
            .iter()
            .find(|(_, each)| each.id == row.id)
            .map(|(_, each)| each.clone())
        else {
            continue;
        };
        let Some(descriptor) = descriptor_of(&row.descriptor, &row.descriptor_overrides) else {
            unavailable.push(api::ProviderSetupUnavailableSubscription {
                harness: HarnessKindName {
                    kind: row.harness_kind.clone(),
                    name: agent.name.clone(),
                    names: crate::programs::shipped_name(&row.harness_kind, None).names,
                },
                agent: Some(agent.clone()),
                reason: api::ProviderSetupSubscriptionUnavailableReason::NotReported,
            });
            unreported.push(agent);
            continue;
        };
        for protocol in descriptor
            .forwarded_protocols
            .iter()
            .chain(&descriptor.direct_protocols)
        {
            let list = speaking.entry(protocol.clone()).or_default();
            if !list.iter().any(|each| each.id == agent.id) {
                list.push(agent.clone());
            }
        }
        let Some(native) = &descriptor.native_provider else {
            unavailable.push(api::ProviderSetupUnavailableSubscription {
                harness: HarnessKindName {
                    kind: row.harness_kind.clone(),
                    name: descriptor.harness.name.clone(),
                    names: descriptor.words_at("harness.name"),
                },
                agent: Some(agent),
                reason: api::ProviderSetupSubscriptionUnavailableReason::Unsupported,
            });
            continue;
        };
        let name = HarnessKindName {
            kind: native.harness_kind.clone(),
            name: native.title.clone(),
            names: descriptor.words_at("native_provider.title"),
        };
        if harnesses
            .resolved
            .value
            .as_ref()
            .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
            .is_some_and(|accepted| !accepted.contains(&native.harness_kind))
        {
            withheld.push(ProviderSetupPolicy {
                harness: name,
                scope: scope_of(&harnesses),
            });
            continue;
        }
        if sharing
            .resolved
            .value
            .as_ref()
            .and_then(|value| serde_json::from_value::<Vec<String>>(value.clone()).ok())
            .is_some_and(|shared| !shared.contains(&native.harness_kind))
        {
            unshared.push(ProviderSetupPolicy {
                harness: name.clone(),
                scope: scope_of(&sharing),
            });
        }
        let placed_as: Vec<ArrangementType> = descriptor
            .arrangements
            .iter()
            .filter_map(|entry| same(&entry.kind))
            .collect();
        let proposed: Vec<ArrangementType> = native.arrangements.iter().filter_map(same).collect();
        subscriptions.push(ProviderSetupSubscription {
            agent,
            harness: name,
            methods: native.login_methods.iter().filter_map(same).collect(),
            options_schema: native.options_schema.clone(),
            arrangements: consents(&proposed, Some(&placed_as)),
            template: template.clone(),
        });
    }
    // The public system catalogue names built-ins even before a person has a usable profile.
    // Never copy another person's private profile, or infer native support from a kind or a process.
    for named in orch_db::queries::resources::binding::system_agent_kinds(&mut conn).await? {
        for choice in unavailable.iter_mut().filter(|choice| {
            choice.harness.kind == named.harness_kind
                && choice.reason == api::ProviderSetupSubscriptionUnavailableReason::NotReported
        }) {
            choice.harness.name = named.name.clone();
        }
        if rows
            .iter()
            .any(|row| row.harness_kind == named.harness_kind)
            || unavailable
                .iter()
                .any(|row| row.harness.kind == named.harness_kind)
        {
            continue;
        }
        let reason = if descriptor_of(&named.descriptor, &Json::Null)
            .is_some_and(|descriptor| descriptor.native_provider.is_none())
        {
            api::ProviderSetupSubscriptionUnavailableReason::Unsupported
        } else {
            api::ProviderSetupSubscriptionUnavailableReason::ProfileNeeded
        };
        unavailable.push(api::ProviderSetupUnavailableSubscription {
            harness: HarnessKindName {
                names: crate::programs::shipped_name(&named.harness_kind, Some(&named.descriptor))
                    .names,
                kind: named.harness_kind,
                name: named.name,
            },
            agent: None,
            reason,
        });
    }
    let protocols = PROTOCOLS
        .iter()
        .map(|name| ProviderSetupProtocol {
            name: (*name).to_owned(),
            title: protocol_title(name).to_owned(),
            endpoint_options: standard_options(),
            agents: speaking.get(*name).cloned().unwrap_or_default(),
        })
        .collect();
    let presets = presets_of(
        setting::<Json>(&mut conn, Place::Node(org), keys::PROVIDERS_PRESETS)
            .await
            .map_err(ApiError::internal)?,
        org,
    );
    let physical = queries::physical_machines(&mut conn)
        .await?
        .into_iter()
        .filter(|row| row.is_active())
        .collect();
    let choices: Vec<api::ProviderSetupMachine> =
        super::machines::machine_views(&ctx, &mut conn, physical)
            .await?
            .into_iter()
            .filter(|machine| {
                machine.access.as_ref().is_some_and(|access| {
                    access
                        .permissions
                        .iter()
                        .any(|permission| matches!(permission.as_str(), "machine.use" | "host.use"))
                })
            })
            .map(|machine| api::ProviderSetupMachine {
                directory_runtime: machine
                    .facts
                    .get("provider_runtime_descriptor")
                    .or_else(|| machine.facts.get("environment_descriptor"))
                    .and_then(|descriptor| descriptor.get("lifecycle"))
                    .and_then(Json::as_str)
                    == Some("attach"),
                arrangements: orch_engine::binding::arrangements_of_policy(
                    orch_db::queries::environment_facts::policy_in(&machine.facts),
                ),
                upstreams: machine
                    .facts
                    .pointer("/policy/provider.upstreams")
                    .and_then(Json::as_array)
                    .map(|urls| {
                        urls.iter()
                            .filter_map(Json::as_str)
                            .map(str::to_owned)
                            .collect()
                    }),
                harnesses: machine
                    .facts
                    .get("adapters")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|adapter| adapter.get("kind").and_then(Json::as_str))
                    .filter(|kind| super::agents::machine_reports(&machine.facts, kind))
                    .map(str::to_owned)
                    .collect(),
                machine: machine.base,
                roles: machine.roles,
            })
            .collect();
    let mine: Vec<Id> = if let Some(provider) = &provider {
        provider.key_machine_id.into_iter().collect()
    } else {
        choices
            .iter()
            .filter(|choice| choice.roles.iter().any(|role| role == "provider"))
            .map(|choice| choice.machine.id)
            .collect()
    };
    let machine_refs = machine_refs(&ctx.app, &mut conn, &mine).await?;
    let key_machines: Vec<api::MachineRef> = mine
        .iter()
        .filter_map(|id| machine_refs.get(id).cloned())
        .collect();
    // The templates of the pool of providers' runtimes the caller may use.
    let templates = resources::visible(
        &mut conn,
        &principal,
        ResourceKind::Template,
        Page {
            after: None,
            limit: LIST_LIMIT,
        },
        Policy::default(),
    )
    .await
    .map_err(ApiError::internal)?;
    let usable_templates: Vec<Id> = if let Some(provider) = &provider {
        if (provider.key_machine_id.is_none() || provider.key_machine_id == provider.machine_id)
            && orch_db::queries::environment_facts::of_provider(&mut conn, provider.id)
                .await?
                .is_none_or(|machine| !machine.attaches())
        {
            let selected = setting::<Id>(
                &mut conn,
                Place::Provider(provider.id),
                keys::RUNTIME_TEMPLATE_ID,
            )
            .await
            .map_err(ApiError::internal)?;
            let selected = match selected {
                Some(selected) => Some(selected),
                None => orch_db::queries::resources::provider_runtime::live_runtime(
                    &mut conn,
                    provider.id,
                )
                .await?
                .map(|runtime| runtime.template_id),
            };
            selected.into_iter().collect()
        } else {
            Vec::new()
        }
    } else {
        templates
            .items
            .iter()
            .filter(|each| {
                each.held.iter().any(|held| {
                    matches!(
                        held.permission,
                        Permission::TemplateUse | Permission::TemplateManage
                    )
                })
            })
            .map(|each| each.resource.id)
            .collect()
    };
    let template_rows = queries::runtime_templates(&mut conn, &usable_templates).await?;
    let owners: Vec<Id> = template_rows
        .iter()
        .filter_map(|row| row.owner_id)
        .collect();
    let owners = refusals::user_refs(&mut conn, &owners).await?;
    let sources = super::provider_sources::offers(
        &mut conn,
        &key_machines,
        &template_rows,
        &ctx.app.builtin_sources,
    )
    .await?;
    Ok(ProviderSetup {
        machines: Some(choices),
        subscriptions,
        subscriptions_unreported: Some(unreported),
        subscriptions_unavailable: Some(unavailable),
        subscriptions_withheld: withheld,
        subscriptions_unshared: unshared,
        protocols,
        presets,
        api_arrangements: consents(
            &[ArrangementType::Separated, ArrangementType::KeySeparated],
            None,
        ),
        key_machines,
        http_allowed: policies.http_allowed,
        runtime_templates: template_rows
            .into_iter()
            .map(|row| ProviderSetupRuntimeTemplate {
                id: row.id,
                name: row.name,
                driver: row.driver,
                owner: row.owner_id.and_then(|owner| owners.get(&owner).cloned()),
            })
            .collect(),
        sources,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_profile_that_has_reported_no_descriptor_declares_nothing_yet() {
        assert!(descriptor_of(&json!({}), &json!({})).is_none());
        assert!(descriptor_of(&json!({ "kind": 3 }), &json!({})).is_none());
    }

    #[test]
    fn presets_are_the_vendors_endpoints_by_name_and_one_set_to_null_is_hidden() {
        let presets = presets_of(
            Some(json!({
                "OpenAI": { "upstreams": [{ "protocol": "openai-responses", "base_url": "https://api.openai.com/v1" }] },
                "Hidden": null,
                "Broken": { "upstreams": "nope" }
            })),
            Id::nil(),
        );
        assert_eq!(presets.len(), 1);
        assert_eq!(presets[0].name, "OpenAI");
        assert_eq!(presets[0].upstreams[0].options, json!({}));
    }

    #[test]
    fn a_key_only_offers_the_types_in_which_it_stays_home_by_default() {
        let types = consents(
            &[ArrangementType::Separated, ArrangementType::KeySeparated],
            None,
        );
        assert_eq!(types.iter().filter(|each| each.allowed).count(), 2);
        assert!(types.iter().all(|each| each.available));
    }

    #[test]
    fn a_native_reader_without_a_proposal_uses_only_the_supported_creation_default() {
        let choices = consents(
            &[],
            Some(&[ArrangementType::Separated, ArrangementType::KeySeparated]),
        );
        let proposed: Vec<_> = choices
            .iter()
            .filter(|choice| choice.allowed)
            .map(|choice| choice.kind)
            .collect();
        assert_eq!(proposed, [ArrangementType::Separated]);
        let unavailable = consents(&[], Some(&[ArrangementType::KeySeparated]));
        assert!(
            unavailable.iter().all(|choice| !choice.allowed),
            "no unsupported default is invented"
        );
    }
}
