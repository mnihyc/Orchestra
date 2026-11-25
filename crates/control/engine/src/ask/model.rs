//! The same current model, payer and Use selection for admission and native delivery.
use crate::EngineError;
use orch_access_contract::permissions::ProviderUsePurpose;
use orch_api_contract::ask::{AskModel, AskModelIdentity};
use orch_authz::Principal;
use orch_db::{Conn, DbError, queries::resources::thread_engine as resources};
use orch_provider_contract::{capabilities::OfferingRef, models::ModelParameters};
use orch_settings::Place;
use orch_types::{Id, Json, Time};
use serde_json::json;

/// A model choice refused by current policy, or a failed factual read.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Preserve the ordinary direct database outcome, including transaction retry.
    #[error(transparent)]
    Db(#[from] DbError),
    /// The same ordinary refusal shown at interactive admission.
    #[error("{0}")]
    Denied(Refusal),
    /// The native read failed rather than granting work.
    #[error(transparent)]
    Native(#[from] EngineError),
}
/// A refused choice: why, as a stable name a client acts on, and as the sentence a person is given.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The stable name of the reason.
    pub reason: String,
    /// The sentence for the person.
    pub message: String,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error {
    fn denied(reason: impl Into<String>, message: impl std::fmt::Display) -> Self {
        Self::Denied(Refusal {
            reason: reason.into(),
            message: message.to_string(),
        })
    }
    fn native(error: impl Into<EngineError>) -> Self {
        Self::Native(error.into())
    }
}

/// Current explicit choice, before its existing cap and online-keeper gates.
pub struct Selection {
    /// Original explicit options, before declared request defaults are filled.
    pub selection: AskModel,
    /// Validated account and model taken from the selected backend.
    pub offering: OfferingRef,
    /// Exact model and resolved request options.
    pub model: AskModel,
    /// Actual declared dialogue interface.
    pub protocol: Option<String>,
    pub structured_output: Option<bool>,
    /// The model's actual request constraints.
    pub parameters: orch_provider_contract::models::ModelParameters,
    /// Current payer selected by the root's policy.
    pub payer: Id,
    keeper: Option<Id>,
    offline: bool,
}
/// An available choice after the existing cap and online-keeper gates.
pub struct Chosen {
    /// Original explicit options, retained for preference rather than execution.
    pub selection: AskModel,
    /// Validated account and model taken from the selected backend.
    pub offering: OfferingRef,
    /// Exact model and resolved request options.
    pub model: AskModel,
    /// Actual declared dialogue interface.
    pub protocol: Option<String>,
    pub structured_output: Option<bool>,
    /// The model's actual request constraints.
    pub parameters: orch_provider_contract::models::ModelParameters,
    /// Current payer selected by the root's policy.
    pub payer: Id,
    /// Actual online credential keeper.
    pub machine: Option<Id>,
}
impl Selection {
    /// Called after the cap gate, preserving the interactive refusal order.
    pub async fn online(self, conn: &mut Conn) -> Result<Chosen, Error> {
        let machine = if self.protocol.is_some() {
            let keeper=self.keeper.ok_or_else(|| Error::denied("no_keeper","This provider has no machine that keeps its key. Ask whoever manages it to set one."))?;
            if !orch_db::queries::resources::machine(conn, keeper)
                .await?
                .is_some_and(|machine| machine.online)
            {
                return Err(Error::denied(
                    "keeper_offline",
                    "The machine that keeps this provider's key is offline. Try again shortly or choose another model.",
                ));
            }
            Some(keeper)
        } else {
            None
        };
        if self.protocol.is_some() && self.offline {
            return Err(Error::denied(
                "provider_unavailable",
                "This model's provider is not available. Choose another model or ask whoever manages the provider.",
            ));
        }
        Ok(Chosen {
            selection: self.selection,
            offering: self.offering,
            model: self.model,
            protocol: self.protocol,
            structured_output: self.structured_output,
            parameters: self.parameters,
            payer: self.payer,
            machine,
        })
    }
}
/// Ask declares its own read tools, and the machine that keeps the key refuses a request whose options add
/// another tool. So its request never selects hosted web search, whatever the model or the provider offers or
/// starts with, or an earlier choice kept: the option is set to disabled wherever the model has one.
fn without_web_search(protocol: &str, parameters: &ModelParameters, chosen: Option<Json>) -> Json {
    let mut options = chosen
        .as_ref()
        .and_then(Json::as_object)
        .cloned()
        .unwrap_or_default();
    options.remove("web_search");
    if orch_model_contract::web_search::for_model(protocol, parameters).is_some()
        && parameters.option_rule(protocol, "web_search") != Some(&Json::Bool(false))
    {
        options.insert("web_search".to_owned(), json!({"mode":"disabled"}));
    }
    Json::Object(options)
}

/// Select the exact explicit model through the ordinary Ask policy and Use checks.
pub async fn select(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    model: AskModel,
    now: Time,
) -> Result<Selection, Error> {
    let configured =
        orch_settings::get::<Vec<AskModelIdentity>>(conn, Place::Node(root), "ask.models")
            .await
            .map_err(Error::native)?
            .unwrap_or_default();
    if !configured
        .iter()
        .any(|identity| identity.backend == model.backend)
    {
        return Err(Error::denied(
            "not_in_list",
            "This model is not on this organization's Ask list. Choose one of the listed models.",
        ));
    }
    select_offering(conn, principal, root, model, now).await
}

/// Report the availability of an entry already read from the resolved policy.
/// This preview yields no admitted selection or authority to run work.
pub async fn preview_configured(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    entry: &AskModelIdentity,
    now: Time,
) -> Result<(), Error> {
    select_offering(conn, principal, root, entry.clone().into(), now)
        .await
        .map(|_| ())
}

async fn select_offering(
    conn: &mut Conn,
    principal: &Principal,
    root: Id,
    mut model: AskModel,
    now: Time,
) -> Result<Selection, Error> {
    let selection = model.clone();
    model
        .backend
        .validate()
        .map_err(|reason| Error::denied("invalid_backend", reason))?;
    let offering = model
        .backend
        .offering
        .clone()
        .ok_or_else(|| Error::denied("model_unavailable", "Choose an Ask model offering."))?;
    let program = match model.backend.route.as_str() {
        orch_api_contract::ask::API_ROUTE => false,
        orch_api_contract::ask::PROGRAM_ROUTE => true,
        _ => {
            return Err(Error::denied(
                "generation_unsupported",
                "This route has no installed Ask executor.",
            ));
        }
    };
    let found = resources::model(conn, offering.provider, &offering.offering)
        .await?
        .filter(|found| found.enabled)
        .ok_or_else(|| {
            Error::denied(
                "model_disabled",
                "This model is switched off. Choose another model.",
            )
        })?;
    let provider = resources::provider(conn, offering.provider)
        .await?
        .filter(|provider| {
            (if program {provider.kind == "subscription"} else {provider.kind=="api"})
                && provider.deleted_at.is_none()
                && (program || !matches!(provider.status.as_str(),"disabled" | "error" | "needs_setup"))
        })
        .ok_or_else(|| {
            Error::denied(
                "provider_unavailable",
                "This model's provider is not available. Choose another model or ask whoever manages the provider.",
            )
        })?;
    let protocol = if program {
        None
    } else {
        Some(
            provider
                .protocols
                .iter()
                .find(|protocol| orch_model_contract::generation::supported(protocol))
                .cloned()
                .ok_or_else(|| {
                    Error::denied(
                        "no_interface",
                        "This model's provider offers no way to hold a conversation. Choose another model.",
                    )
                })?,
        )
    };
    let (parameters, purpose) = orch_provider_contract::models::composed_model_with_purpose(
        &found.model_name,
        &found.parameters,
        &found.groups,
        &found.option_layers,
        None,
    )
    .map_err(EngineError::unreadable)?;
    if !purpose.supports_purpose(orch_provider_contract::models::ModelPurpose::Generation) {
        return Err(Error::denied(
            "generation_unsupported",
            crate::models::NotAllowed::GenerationUnsupported.describe(&offering.offering),
        ));
    }
    let (params, default_effort) = if let Some(protocol) = &protocol {
        let defaults = resources::provider_request_defaults(conn, provider.id, protocol).await?;
        let resolve = |params: &Json| {
            orch_model_contract::parameters::resolve(protocol, &parameters, &defaults, params)
                .map_err(|message| Error::denied("options_refused", message))
        };
        let chosen = resolve(&without_web_search(
            protocol,
            &parameters,
            model.params.clone(),
        ))?;
        let defaults = resolve(&json!({}))?;
        let effort = orch_provider_contract::calls::requested_effort(&defaults)
            .map_err(|message| Error::denied("options_refused", message))?
            .map(str::to_owned);
        (chosen, effort)
    } else {
        let mut params = model.params.clone().unwrap_or_else(|| json!({}));
        if !params.is_object() {
            return Err(Error::denied(
                "options_refused",
                "Program model parameters must be an object.",
            ));
        }
        let effort = orch_provider_contract::calls::requested_effort(&params)
            .map_err(|message| Error::denied("options_refused", message))?;
        if parameters.effort_is_gone(effort) {
            return Err(Error::denied(
                "options_refused",
                "The selected model does not offer this effort.",
            ));
        }
        if let Some(effort) = parameters.effort_for(effort) {
            params["effort"] = json!(effort);
        }
        (params, parameters.default_effort.clone())
    };
    let effective = crate::resolved::load(conn, root)
        .await
        .map_err(Error::native)?;
    if effective.models_allowed.as_ref().is_some_and(|patterns| {
        !crate::models::allowed_by(patterns, &offering.offering, &found.groups)
    }) {
        return Err(Error::denied(
            "model_not_allowed",
            "This organization's policy does not allow this model. Choose another model.",
        ));
    }
    if crate::provider_policy::forbidden(conn, root, provider.id)
        .await
        .map_err(Error::native)?
        .is_some()
    {
        return Err(Error::denied(
            "provider_not_allowed",
            "This organization's policy does not allow this model's provider. Choose another model.",
        ));
    }
    let paid = orch_settings::resolve(conn, Place::Node(root), "ask.paid_by")
        .await
        .map_err(Error::native)?;
    let payer = if paid.value().and_then(Json::as_str) == Some("setter") {
        orch_db::queries::settings::row(conn, paid.resolved.source.node, "ask.paid_by")
            .await?
            .and_then(|row| row.updated_by)
            .ok_or_else(|| {
                Error::denied(
                    "payer_unset",
                    "Ask is set to be paid for by whoever set it, and that person is no longer known. Ask an administrator to set who pays.",
                )
            })?
    } else {
        principal.caller().user()
    };
    let payer_principal = if payer == principal.caller().user() {
        principal.clone()
    } else {
        Principal::load(
            conn,
            orch_authz::Caller::session(payer).with_admin_mode(false),
            now,
        )
        .await
        .map_err(Error::native)?
    };
    let policies = crate::access::resource_policies(conn, provider.id)
        .await
        .map_err(Error::native)?;
    let person_use =
        orch_authz::usable::usable(conn, principal, provider.id, Some(root), &policies)
            .await
            .map_err(Error::native)?;
    let orch_authz::usable::Usable::Yes(person_use) = person_use else {
        return Err(Error::denied(
            "no_use",
            "You may not use this model's provider in this organization. Ask whoever manages it to let you.",
        ));
    };
    if !person_use.allows_for(
        &orch_authz::options::Wanted::default(),
        ProviderUsePurpose::Ask,
    ) {
        return Err(Error::denied(
            "no_use",
            "You may not use this model's provider for Ask in this organization. Ask whoever manages it to let you.",
        ));
    }
    if default_effort.as_deref().is_some_and(|effort| {
        !person_use.allows_for(
            &orch_authz::options::Wanted {
                effort: Some(effort),
                ..Default::default()
            },
            ProviderUsePurpose::Ask,
        )
    }) {
        return Err(Error::denied(
            "identity_not_usable",
            "Your access to this provider does not cover this model's default options. Choose another model.",
        ));
    }
    let effort = orch_provider_contract::calls::requested_effort(&params)
        .map_err(|message| Error::denied("options_refused", message))?;
    let wanted = orch_authz::options::Wanted {
        purpose: orch_access_contract::permissions::UsagePurpose::Ask,
        model: Some((&offering.offering, &found.groups)),
        effort,
        ..orch_authz::options::Wanted::default()
    };
    if !person_use.allows_for(&wanted, ProviderUsePurpose::Ask) {
        return Err(Error::denied(
            "use_excludes",
            "Your access to this provider does not allow Ask with this model and effort. Choose another or request access.",
        ));
    }
    let grant =
        orch_authz::usable::usable(conn, &payer_principal, provider.id, Some(root), &policies)
            .await
            .map_err(Error::native)?;
    let orch_authz::usable::Usable::Yes(granted) = grant else {
        return Err(Error::denied(
            "payer_no_use",
            "The person who pays for Ask here may not use this provider in this organization. Ask an administrator.",
        ));
    };
    if !granted.allows_for(
        &orch_authz::options::Wanted::default(),
        ProviderUsePurpose::Ask,
    ) {
        return Err(Error::denied(
            "payer_no_use",
            "The person who pays for Ask here may not use this provider for Ask. Ask an administrator.",
        ));
    }
    if !granted.allows_for(&wanted, ProviderUsePurpose::Ask) {
        return Err(Error::denied(
            "payer_excludes",
            "The person who pays for Ask here does not have Ask access with this model and effort. Choose another.",
        ));
    }
    let structured_output = if program {
        program_availability(conn, root, &model.backend).await?;
        let agent = resources::agent(
            conn,
            provider
                .agent_id
                .ok_or_else(|| Error::denied("program_missing", "This sign-in has no program."))?,
        )
        .await?
        .ok_or_else(|| Error::denied("program_unavailable", "This program is unavailable."))?;
        let descriptor = crate::harnesses::descriptor_of(&agent)?.ok_or_else(|| {
            Error::denied(
                "program_unreported",
                "The program has not reported its capabilities.",
            )
        })?;
        Some(super::dialogue::structured_output(&descriptor))
    } else {
        None
    };
    model.params = Some(params);
    Ok(Selection {
        selection,
        model,
        offering,
        protocol,
        parameters,
        payer,
        structured_output,
        keeper: provider.key_machine_id.or(provider.machine_id),
        offline: provider.status == "offline",
    })
}

/// The same installed program declaration, source codec, custody and explicit placement
/// are checked for model selection and paid execution. Listing a choice starts no work.
pub async fn program_availability(
    conn: &mut Conn,
    root: Id,
    backend: &orch_provider_contract::capabilities::BackendRef,
) -> Result<(), Error> {
    use crate::tools::capability_service::delegation;
    if let Err(failure) = delegation::backend_state(
        conn,
        &orch_provider_contract::builtins::BuiltinSources::default(),
        "ask",
        backend,
    )
    .await?
    {
        return Err(Error::denied(
            failure
                .reason
                .unwrap_or_else(|| "program_execution_unavailable".into()),
            failure.message,
        ));
    }
    let placement = orch_provider_contract::capabilities::ProgramPlacement::read(&backend.config)
        .map_err(|message| Error::denied("placement_unavailable", message))?;
    delegation::placement::Placement::configured(conn, root, None, &placement)
        .await
        .map_err(|error| match error {
            EngineError::Unreadable(message) => Error::denied("placement_unavailable", message),
            other => Error::Native(other),
        })?;
    Ok(())
}

/// Policy editing can describe an offering before its root selects it or a helper
/// placement is filled in. The accepted turn still has to pass both people's current Use.
pub async fn inspect_backend(
    conn: &mut Conn,
    principal: &Principal,
    place: Option<Id>,
    backend: &orch_provider_contract::capabilities::BackendRef,
) -> Result<orch_provider_contract::capabilities::Availability, EngineError> {
    use orch_provider_contract::capabilities::Availability;
    let denied = |reason: &str| Availability {
        allowed: false,
        reason: Some(reason.into()),
        ..Default::default()
    };
    let Some(offering) = &backend.offering else {
        return Ok(denied("offering_not_selected"));
    };
    let Some(provider) = resources::provider(conn, offering.provider)
        .await?
        .filter(|p| p.deleted_at.is_none())
    else {
        return Ok(denied("provider_missing"));
    };
    let policies = crate::access::resource_policies(conn, provider.id).await?;
    let orch_authz::usable::Usable::Yes(grant) =
        orch_authz::usable::usable(conn, principal, provider.id, place, &policies).await?
    else {
        return Ok(denied("current_authority_refused"));
    };
    let Some(model) = resources::model(conn, provider.id, &offering.offering)
        .await?
        .filter(|row| row.enabled)
    else {
        return Ok(denied("model_unavailable"));
    };
    let (_, purpose) = orch_provider_contract::models::composed_model_with_purpose(
        &model.model_name,
        &model.parameters,
        &model.groups,
        &model.option_layers,
        None,
    )?;
    if !purpose.supports_purpose(orch_provider_contract::models::ModelPurpose::Generation) {
        return Ok(denied("generation_unsupported"));
    }
    if !grant.allows_for(
        &orch_authz::options::Wanted {
            model: Some((&offering.offering, &model.groups)),
            ..Default::default()
        },
        ProviderUsePurpose::Ask,
    ) {
        return Ok(denied("current_authority_refused"));
    }
    if let Some(place) = place {
        let settings = crate::resolved::load(conn, place).await?;
        if settings.models_allowed.as_ref().is_some_and(|rules| {
            !crate::models::allowed_by(rules, &offering.offering, &model.groups)
        }) || crate::provider_policy::forbidden(conn, place, provider.id)
            .await?
            .is_some()
        {
            return Ok(denied("excluded_by_policy"));
        }
    }
    match backend.route.as_str() {
        orch_api_contract::ask::API_ROUTE
            if provider.kind == "api"
                && crate::providers::request_status_available(&provider.status)
                && provider
                    .protocols
                    .iter()
                    .any(|protocol| orch_model_contract::generation::supported(protocol)) => {}
        orch_api_contract::ask::PROGRAM_ROUTE => {
            if let Err(failure) = crate::tools::capability_service::delegation::backend_state(
                conn,
                &orch_provider_contract::builtins::BuiltinSources::default(),
                "ask",
                backend,
            )
            .await?
            {
                return Ok(denied(
                    failure
                        .reason
                        .as_deref()
                        .unwrap_or("program_execution_unavailable"),
                ));
            }
        }
        _ => return Ok(denied("generation_unsupported")),
    }
    Ok(Availability {
        allowed: true,
        ..Default::default()
    })
}
