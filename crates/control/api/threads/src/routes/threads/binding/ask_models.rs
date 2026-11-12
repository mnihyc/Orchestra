//! Configured Ask choices share native snapshot facts and ordinary Use decisions in batches.

use super::*;

use orch_api_contract::ask::AskModelIdentity;

use orch_api_contract::ask::AskModelOption;

use orch_types::Json;

use std::collections::BTreeSet;

use std::collections::HashSet;
struct Inputs {
    providers: HashMap<Id, ProviderRow>,
    grants: HashMap<Id, Vec<orch_access_contract::permissions::ProviderUse>>,
    references: HashMap<Id, ProviderRef>,
    models: HashMap<Id, HashMap<String, ModelOption>>,
    defaults: HashMap<(Id, String), Json>,
    forbidden: HashSet<Id>,
}

async fn inputs(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    ids: &[Id],
    settings: &Resolved,
) -> ApiResult<Inputs> {
    let providers: HashMap<_, _> = engine_rows::providers(conn, ids)
        .await?
        .into_iter()
        .map(|provider| (provider.id, provider))
        .collect();
    let valid: Vec<_> = ids
        .iter()
        .copied()
        .filter(|id| {
            providers
                .get(id)
                .is_some_and(|provider| provider.kind == "api" && provider.deleted_at.is_none())
        })
        .collect();
    let policies = orch_engine::access::resource_policies_among(conn, &valid)
        .await
        .map_err(ApiError::internal)?;
    // What each entry allows, one by one: a model is offered when any single entry allows it.
    let grants: HashMap<_, Vec<_>> =
        usable::usable_among(conn, principal, &valid, Some(root), &policies)
            .await
            .map_err(ApiError::internal)?
            .into_iter()
            .filter_map(|(id, decision)| {
                decision.granted().map(|granted| {
                    (
                        id,
                        granted
                            .each
                            .iter()
                            .map(|allowance| allowance.provider.clone())
                            .collect(),
                    )
                })
            })
            .collect();
    let granted: Vec<_> = valid
        .into_iter()
        .filter(|id| grants.contains_key(id))
        .collect();
    let references: HashMap<_, _> = provider_refs(conn, &granted, &[])
        .await?
        .into_iter()
        .map(|(_, provider)| (provider.id, provider))
        .collect();
    let rows = orch_db::queries::resources::models_of_providers(conn, &granted).await?;
    let models = rows
        .into_iter()
        .map(|(id, rows)| {
            Ok((
                id,
                model_options(
                    rows,
                    settings,
                    grants
                        .get(&id)
                        .map(|each| each.iter().collect::<Vec<_>>())
                        .as_deref(),
                )?
                .into_iter()
                .map(|model| (model.model_name.clone(), model))
                .collect(),
            ))
        })
        .collect::<ApiResult<HashMap<_, _>>>()?;
    let defaults = engine_rows::provider_request_defaults_among(conn, &granted).await?;
    let forbidden = forbidden(conn, root, &references).await?;
    Ok(Inputs {
        providers,
        grants,
        references,
        models,
        defaults,
        forbidden,
    })
}

async fn forbidden(
    conn: &mut Conn,
    root: Id,
    references: &HashMap<Id, ProviderRef>,
) -> ApiResult<HashSet<Id>> {
    if references.is_empty() {
        return Ok(HashSet::new());
    }
    let def = orch_settings_contract::registry::get(keys::PROVIDERS_ALLOWED)
        .ok_or_else(|| ApiError::internal("the provider policy is not registered"))?;
    let snapshot =
        orch_settings::Snapshot::load(conn, Place::Node(root), orch_settings::Wanted::Keys(&[def]))
            .await
            .map_err(ApiError::internal)?;
    let policy = snapshot.resolve(def);
    Ok(references
        .iter()
        .filter_map(|(id, provider)| {
            orch_engine::provider_policy::refusing_scope(&policy.parts, &provider.address)
                .map(|_| *id)
        })
        .collect())
}

fn choice(selection: AskModelIdentity, inputs: &Inputs) -> AskModelOption {
    let id = selection.provider_id;
    let mut choice = AskModelOption {
        selection,
        provider: None,
        model: None,
        request_option_schema: None,
        allowed: false,
        is_default: false,
        unavailable_reason: Some("provider_unavailable".into()),
    };
    let Some(provider) = inputs
        .providers
        .get(&id)
        .filter(|provider| provider.kind == "api" && provider.deleted_at.is_none())
    else {
        return choice;
    };
    let Some(grant) = inputs.grants.get(&id) else {
        choice.unavailable_reason = Some("no_use".into());
        return choice;
    };
    choice.provider = inputs.references.get(&id).cloned();
    choice.model = inputs
        .models
        .get(&id)
        .and_then(|models| models.get(&choice.selection.model))
        .cloned();
    let protocol = provider
        .protocols
        .iter()
        .find(|protocol| orch_model_contract::generation::supported(protocol));
    choice.request_option_schema = protocol.and_then(|protocol| {
        choice
            .model
            .as_ref()?
            .option_constraints
            .get(protocol)
            .cloned()
    });
    let reason = if provider.status == "disabled" {
        Some("disabled")
    } else if matches!(
        provider.status.as_str(),
        "offline" | "error" | "needs_setup"
    ) {
        Some("provider_unavailable")
    } else if protocol.is_none() {
        Some("generation_unsupported")
    } else if inputs.forbidden.contains(&id) {
        Some("excluded_by_policy")
    } else if choice.model.is_none() {
        Some("model_unavailable")
    } else if choice.model.as_ref().is_some_and(|model| !model.allowed) {
        Some(match choice.model.as_ref().and_then(|model| model.reason) {
            Some(ModelOptionReason::ExcludedByPolicy) => "excluded_by_policy",
            Some(ModelOptionReason::Disabled) => "disabled",
            _ => "no_use",
        })
    } else {
        None
    };
    choice.unavailable_reason = reason.map(str::to_owned);
    if choice.unavailable_reason.is_none()
        && let (Some(protocol), Some(model)) = (protocol, choice.model.as_ref())
    {
        let empty = serde_json::json!({});
        let defaults = inputs
            .defaults
            .get(&(id, protocol.clone()))
            .unwrap_or(&empty);
        let resolved =
            orch_model_contract::parameters::resolve(protocol, &model.parameters, defaults, &empty);
        let permitted = resolved.as_ref().is_ok_and(|params| {
            orch_provider_contract::calls::requested_effort(params).is_ok_and(|effort| {
                effort.is_none_or(|effort| {
                    // One entry has to allow the model and its effort together.
                    grant.iter().any(|entry| {
                        orch_authz::options::entry_allows(
                            entry,
                            &orch_authz::options::Wanted {
                                model: Some((&choice.selection.model, &model.groups)),
                                effort: Some(effort),
                                ..orch_authz::options::Wanted::default()
                            },
                        )
                    })
                })
            })
        });
        if !permitted {
            choice.unavailable_reason = Some("parameters_unavailable".into());
        }
    }
    if choice.provider.is_none() && choice.unavailable_reason.is_none() {
        choice.unavailable_reason = Some("provider_unavailable".into());
    }
    choice.allowed = choice.unavailable_reason.is_none();
    choice
}

pub(super) async fn read(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
) -> ApiResult<Vec<AskModelOption>> {
    let ordinary = principal.ordinary(conn).await.map_err(ApiError::internal)?;
    let configured =
        orch_settings::get::<Vec<AskModelIdentity>>(conn, Place::Node(root), "ask.models")
            .await
            .map_err(ApiError::internal)?
            .unwrap_or_default();
    if configured.is_empty() {
        return Ok(Vec::new());
    }
    let settings = resolved::load(conn, root)
        .await
        .map_err(ApiError::internal)?;
    let ids = configured
        .iter()
        .map(|model| model.provider_id)
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    let inputs = inputs(conn, &ordinary, root, &ids, &settings).await?;
    let mut result = Vec::with_capacity(configured.len());
    let mut marked = false;
    for selection in configured {
        let mut option = choice(selection, &inputs);
        if option.allowed && !marked {
            option.is_default = true;
            marked = true;
        }
        result.push(option);
    }
    Ok(result)
}
