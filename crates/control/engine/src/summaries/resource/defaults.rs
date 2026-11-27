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

/// What a place's default is chosen from: its candidates (every enabled model of every API provider, the place's
/// ordinary provider first, then by provider and model name), each provider's interface and endpoint defaults and
/// each model's row, read in four statements for all of them. In a read that decides once they are read once: a
/// chat's opening asks for the default of its catch-up, its title and its place's summary.
pub(crate) struct Candidates {
    list: Vec<(Id, String)>,
    interfaces: HashMap<Id, Option<(String, Json)>>,
    models: HashMap<(Id, String), resources::ModelRow>,
}

async fn candidates(conn: &mut Conn, place: Id) -> Result<std::sync::Arc<Candidates>, EngineError> {
    crate::summaries::once::candidates(place, async {
        let preferred = orch_settings::get::<Id>(
            conn,
            orch_settings::Place::Node(place),
            orch_settings_contract::registry::keys::AGENT_DEFAULT_PROVIDER_ID,
        )
        .await?;
        let list = resources::default_api_models(conn, preferred).await?;
        let mut ids: Vec<Id> = Vec::new();
        for (provider, _) in &list {
            if !ids.contains(provider) {
                ids.push(*provider);
            }
        }
        let rows = resources::providers(conn, &ids).await?;
        let endpoints = resources::provider_request_defaults_among(conn, &ids).await?;
        let mut interfaces: HashMap<Id, Option<(String, Json)>> =
            ids.iter().map(|id| (*id, None)).collect();
        for row in &rows {
            let interface = completion_protocol(&row.protocols)?.map(|protocol| {
                let endpoint = endpoints
                    .get(&(row.id, protocol.clone()))
                    .cloned()
                    .unwrap_or_else(|| serde_json::json!({}));
                (protocol, endpoint)
            });
            interfaces.insert(row.id, interface);
        }
        let models: HashMap<_, _> = resources::models_named(conn, &list)
            .await?
            .into_iter()
            .collect();
        // The judgments of these models read the same rows.
        crate::summaries::once::keep_rows(
            ids.iter()
                .map(|id| (*id, rows.iter().find(|row| row.id == *id).cloned())),
            list.iter()
                .map(|key| (key.clone(), models.get(key).cloned())),
        );
        Ok::<_, EngineError>(std::sync::Arc::new(Candidates {
            list,
            interfaces,
            models,
        }))
    })
    .await
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
    let mut capped = false;
    let candidates = candidates(conn, place).await?;
    // A provider that refuses this reader at this place (gone, not ready, forbidden there, not usable, no keeper)
    // refuses every model it offers: the rest of its models are not judged.
    let mut refusing: HashSet<Id> = HashSet::new();
    for (provider_id, model_name) in &candidates.list {
        let provider_id = *provider_id;
        if refusing.contains(&provider_id) {
            continue;
        }
        let Some((protocol, endpoint)) = candidates.interfaces.get(&provider_id).cloned().flatten()
        else {
            continue;
        };
        let Some(model) = candidates.models.get(&(provider_id, model_name.clone())) else {
            continue;
        };
        let model_name = model_name.clone();
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
