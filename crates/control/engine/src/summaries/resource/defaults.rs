//! A person's explicit summary may use an ordinary permitted API model without opting into automation.
use super::*;
use orch_api_contract::summaries::SummaryManualProblem;
use std::collections::{HashMap, HashSet};

/// Whether a model judgment's refusal is about the provider for this reader at this place rather than about the
/// model or its effort, so that it holds for every model of that provider.
fn refuses_every_model(reason: Reason) -> bool {
    matches!(
        reason,
        Reason::ProviderGone
            | Reason::ProviderNotReady
            | Reason::ProviderForbidden
            | Reason::NotAllowed
            | Reason::NoKeeper
            | Reason::KeeperOffline
    )
}

/// Effect-free default proposal. It carries no admission and is never written as automation configuration.
pub struct ManualDefault {
    /// One model/effort passing the existing native checks, or no usable default.
    pub config: Option<Config>,
    /// A safe self-usage/access cause, without another provider's identity or model catalogue.
    pub problem: Option<SummaryManualProblem>,
}

/// Prefer the place's ordinary provider, then stable provider/model names. Every proposed
/// option uses the existing parameter resolver, model Use/allowlists and summary cap matcher.
pub async fn manual_default(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    field: orch_summarizer_contract::descriptor::SummaryField,
    now: Time,
) -> Result<ManualDefault, EngineError> {
    let preferred = orch_settings::thread_provider(conn, orch_settings::Place::Node(place)).await?;
    let mut capped = false;
    // Each provider's interface and endpoint defaults, read once for all of its models.
    let mut interfaces: HashMap<Id, Option<(String, Json)>> = HashMap::new();
    // A provider that refuses this reader at this place (gone, not ready, forbidden there, not usable, no keeper)
    // refuses every model it offers: the rest of its models are not judged.
    let mut refusing: HashSet<Id> = HashSet::new();
    for (provider_id, model_name) in resources::default_api_models(conn, preferred).await? {
        if refusing.contains(&provider_id) {
            continue;
        }
        if let std::collections::hash_map::Entry::Vacant(unread) = interfaces.entry(provider_id) {
            let interface = match resources::provider(conn, provider_id).await? {
                Some(provider) => match completion_protocol(&provider.protocols)? {
                    Some(protocol) => {
                        let endpoint =
                            resources::provider_request_defaults(conn, provider_id, &protocol)
                                .await?;
                        Some((protocol, endpoint))
                    }
                    None => None,
                },
                None => None,
            };
            unread.insert(interface);
        }
        let Some((protocol, endpoint)) = interfaces.get(&provider_id).cloned().flatten() else {
            continue;
        };
        let Some(model) = resources::model(conn, provider_id, &model_name).await? else {
            continue;
        };
        let parameters = orch_provider_contract::models::composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            Some(&protocol),
        )?;
        let proposals = std::iter::once(serde_json::json!({})).chain(
            parameters
                .efforts
                .iter()
                .flatten()
                .map(|effort| serde_json::json!({"effort":effort})),
        );
        for proposal in proposals {
            let Ok(params) = orch_model_contract::parameters::resolve(
                &protocol,
                &parameters,
                &endpoint,
                &proposal,
            ) else {
                continue;
            };
            let Ok(mut config) = Config::read(
                &serde_json::json!({"backend":{"route":orch_summarizer_model::capabilities::ROUTE,"offering":{"provider":provider_id,"offering":model_name}},"params":params}),
            ) else {
                continue;
            };
            let Ok(tokens) = orch_summarizer_model::context::fit_window(
                config.context_tokens,
                config.output_reserve(field, parameters.needs_reasoning_room()),
                parameters
                    .context_open()
                    .or(parameters.context_window.filter(|window| *window > 0)),
            ) else {
                continue;
            };
            config.context_tokens = tokens;
            match judge_model(conn, principal, place, &config).await? {
                Ok(_) => {}
                Err(reason) if refuses_every_model(reason) => {
                    refusing.insert(provider_id);
                    break;
                }
                Err(_) => continue,
            }
            let tier =
                orch_model_contract::parameters::requested_tier_for_caps(&protocol, &config.params);
            if !crate::caps::own_room(
                conn,
                &crate::caps::OwnCapAsk {
                    place,
                    provider: provider_id,
                    model: &model_name,
                    tier: tier.as_deref(),
                    purpose: "summary",
                    person: Some(principal.caller().user()),
                },
                now.get(),
            )
            .await?
            {
                capped = true;
                continue;
            }
            return Ok(ManualDefault {
                config: Some(config),
                problem: None,
            });
        }
    }
    Ok(ManualDefault {
        config: None,
        problem: Some(if capped {
            SummaryManualProblem::CapReached
        } else {
            SummaryManualProblem::NoModel
        }),
    })
}
