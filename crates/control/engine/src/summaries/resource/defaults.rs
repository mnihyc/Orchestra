//! A person's explicit summary may use an ordinary permitted API model without opting into automation.
use super::*;
use orch_api_contract::summaries::SummaryManualProblem;

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
    now: Time,
) -> Result<ManualDefault, EngineError> {
    let preferred = orch_settings::get::<Id>(
        conn,
        orch_settings::Place::Node(place),
        orch_settings_contract::registry::keys::AGENT_DEFAULT_PROVIDER_ID,
    )
    .await?;
    let mut capped = false;
    for (provider_id, model_name) in resources::default_api_models(conn, preferred).await? {
        let Some(provider) = resources::provider(conn, provider_id).await? else {
            continue;
        };
        let Some(protocol) = completion_protocol(&provider.protocols)? else {
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
        let endpoint = resources::provider_request_defaults(conn, provider_id, &protocol).await?;
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
                &serde_json::json!({"provider_id":provider_id,"model_name":model_name,"params":params}),
            ) else {
                continue;
            };
            let Ok(tokens) = orch_summarizer_model::context::fit_window(
                config.context_tokens,
                config.max_output_tokens,
                parameters
                    .context_open()
                    .or(parameters.context_window.filter(|window| *window > 0)),
            ) else {
                continue;
            };
            config.context_tokens = tokens;
            match require_model(conn, principal, place, &config).await {
                Ok(()) => {}
                Err(EngineError::BeforeEffect(_)) => continue,
                Err(error) => return Err(error),
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
