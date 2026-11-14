//! The same current model, payer and Use selection for admission and native delivery.
use crate::EngineError;
use orch_api_contract::ask::{AskModel, AskModelIdentity};
use orch_authz::Principal;
use orch_db::{Conn, DbError, queries::resources::thread_engine as resources};
use orch_provider_contract::models::ModelParameters;
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
    pub reason: &'static str,
    /// The sentence for the person.
    pub message: String,
}
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl Error {
    fn denied(reason: &'static str, message: impl std::fmt::Display) -> Self {
        Self::Denied(Refusal {
            reason,
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
    /// Exact model and resolved request options.
    pub model: AskModel,
    /// Actual declared dialogue interface.
    pub protocol: String,
    /// The model's actual request constraints.
    pub parameters: orch_provider_contract::models::ModelParameters,
    /// Current payer selected by the root's policy.
    pub payer: Id,
    keeper: Option<Id>,
}
/// An available choice after the existing cap and online-keeper gates.
pub struct Chosen {
    /// Original explicit options, retained for preference rather than execution.
    pub selection: AskModel,
    /// Exact model and resolved request options.
    pub model: AskModel,
    /// Actual declared dialogue interface.
    pub protocol: String,
    /// The model's actual request constraints.
    pub parameters: orch_provider_contract::models::ModelParameters,
    /// Current payer selected by the root's policy.
    pub payer: Id,
    /// Actual online credential keeper.
    pub machine: Id,
}
impl Selection {
    /// Called after the cap gate, preserving the interactive refusal order.
    pub async fn online(self, conn: &mut Conn) -> Result<Chosen, Error> {
        let machine = self
            .keeper
            .ok_or_else(|| {
                Error::denied(
                    "no_keeper",
                    "This provider has no machine that keeps its key. Ask whoever manages it to set one.",
                )
            })?;
        if !orch_db::queries::resources::machine(conn, machine)
            .await?
            .is_some_and(|machine| machine.online)
        {
            return Err(Error::denied(
                "keeper_offline",
                "The machine that keeps this provider's key is offline. Try again shortly or choose another model.",
            ));
        }
        Ok(Chosen {
            selection: self.selection,
            model: self.model,
            protocol: self.protocol,
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
    mut model: AskModel,
    now: Time,
) -> Result<Selection, Error> {
    let configured =
        orch_settings::get::<Vec<AskModelIdentity>>(conn, Place::Node(root), "ask.models")
            .await
            .map_err(Error::native)?
            .unwrap_or_default();
    if !configured
        .iter()
        .any(|identity| identity.provider_id == model.provider_id && identity.model == model.model)
    {
        return Err(Error::denied(
            "not_in_list",
            "This model is not on this organization's Ask list. Choose one of the listed models.",
        ));
    }
    let selection = model.clone();
    let found = resources::model(conn, model.provider_id, &model.model)
        .await?
        .filter(|found| found.enabled)
        .ok_or_else(|| {
            Error::denied(
                "model_disabled",
                "This model is switched off. Choose another model.",
            )
        })?;
    let provider = resources::provider(conn, model.provider_id)
        .await?
        .filter(|provider| {
            provider.kind == "api"
                && provider.deleted_at.is_none()
                && !matches!(
                    provider.status.as_str(),
                    "disabled" | "error" | "needs_setup" | "offline"
                )
        })
        .ok_or_else(|| {
            Error::denied(
                "provider_unavailable",
                "This model's provider is not available. Choose another model or ask whoever manages the provider.",
            )
        })?;
    let protocol = provider
        .protocols
        .iter()
        .find(|protocol| orch_model_contract::generation::supported(protocol))
        .cloned()
        .ok_or_else(|| {
            Error::denied(
                "no_interface",
                "This model's provider offers no way to hold a conversation. Choose another model.",
            )
        })?;
    let parameters = orch_provider_contract::models::composed_model_parameters(
        &found.parameters,
        &found.groups,
        &found.option_layers,
        None,
    )
    .map_err(EngineError::unreadable)?;
    let defaults = resources::provider_request_defaults(conn, provider.id, &protocol).await?;
    let params = orch_model_contract::parameters::resolve(
        &protocol,
        &parameters,
        &defaults,
        &without_web_search(&protocol, &parameters, model.params.clone()),
    )
    .map_err(|message| Error::denied("options_refused", message))?;
    let effective = crate::resolved::load(conn, root)
        .await
        .map_err(Error::native)?;
    if effective
        .models_allowed
        .as_ref()
        .is_some_and(|patterns| !crate::models::allowed_by(patterns, &model.model, &found.groups))
    {
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
    if !person_use.allows(&orch_authz::options::Wanted {
        purpose: orch_access_contract::permissions::ProviderPurpose::Ask,
        ..orch_authz::options::Wanted::default()
    }) {
        return Err(Error::denied(
            "use_excludes",
            "Your access to this provider does not include Ask. Request Ask access or choose another model.",
        ));
    }
    let default_params =
        orch_model_contract::parameters::resolve(&protocol, &parameters, &defaults, &json!({}));
    if !default_params.as_ref().is_ok_and(|params| {
        orch_provider_contract::calls::requested_effort(params).is_ok_and(|effort| {
            effort.is_none_or(|effort| {
                person_use.allows(&orch_authz::options::Wanted {
                    purpose: orch_access_contract::permissions::ProviderPurpose::Ask,
                    effort: Some(effort),
                    ..orch_authz::options::Wanted::default()
                })
            })
        })
    }) {
        return Err(Error::denied(
            "identity_not_usable",
            "Your access to this provider does not cover this model's default options. Choose another model.",
        ));
    }
    let effort = orch_provider_contract::calls::requested_effort(&params)
        .map_err(|message| Error::denied("options_refused", message))?;
    let wanted = orch_authz::options::Wanted {
        purpose: orch_access_contract::permissions::ProviderPurpose::Ask,
        model: Some((&model.model, &found.groups)),
        effort,
        ..orch_authz::options::Wanted::default()
    };
    if !person_use.allows(&wanted) {
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
    if !granted.allows(&wanted) {
        return Err(Error::denied(
            "payer_excludes",
            "The person who pays for Ask here does not have Ask access with this model and effort. Choose another.",
        ));
    }
    model.params = Some(params);
    Ok(Selection {
        selection,
        model,
        protocol,
        parameters,
        payer,
        keeper: provider.key_machine_id.or(provider.machine_id),
    })
}
