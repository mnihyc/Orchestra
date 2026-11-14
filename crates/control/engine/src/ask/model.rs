//! The same current model, payer and Use selection for admission and native delivery.
use crate::EngineError;
use orch_api_contract::ask::{AskModel, AskModelIdentity};
use orch_authz::Principal;
use orch_db::{Conn, DbError, queries::resources::thread_engine as resources};
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
    Denied(String),
    /// The native read failed rather than granting work.
    #[error(transparent)]
    Native(#[from] EngineError),
}
impl Error {
    fn denied(message: impl std::fmt::Display) -> Self {
        Self::Denied(message.to_string())
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
            .ok_or_else(|| Error::denied("The provider has no credential keeper"))?;
        if !orch_db::queries::resources::machine(conn, machine)
            .await?
            .is_some_and(|machine| machine.online)
        {
            return Err(Error::denied("The provider's credential keeper is offline"));
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
            "Choose an entry from this root organization's ask.models list",
        ));
    }
    let selection = model.clone();
    let found = resources::model(conn, model.provider_id, &model.model)
        .await?
        .filter(|found| found.enabled)
        .ok_or_else(|| Error::denied("This model is not enabled"))?;
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
        .ok_or_else(|| Error::denied("Ask needs an available API provider"))?;
    let protocol = provider
        .protocols
        .iter()
        .find(|protocol| orch_model_contract::generation::supported(protocol))
        .cloned()
        .ok_or_else(|| Error::denied("This provider offers no declared dialogue interface"))?;
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
        &model.params.clone().unwrap_or_else(|| json!({})),
    )
    .map_err(Error::denied)?;
    let effective = crate::resolved::load(conn, root)
        .await
        .map_err(Error::native)?;
    if effective
        .models_allowed
        .as_ref()
        .is_some_and(|patterns| !crate::models::allowed_by(patterns, &model.model, &found.groups))
    {
        return Err(Error::denied(
            "This model is not allowed in the conversation's organization",
        ));
    }
    if crate::provider_policy::forbidden(conn, root, provider.id)
        .await
        .map_err(Error::native)?
        .is_some()
    {
        return Err(Error::denied(
            "This provider is not allowed in the conversation's organization",
        ));
    }
    let paid = orch_settings::resolve(conn, Place::Node(root), "ask.paid_by")
        .await
        .map_err(Error::native)?;
    let payer = if paid.value().and_then(Json::as_str) == Some("setter") {
        orch_db::queries::settings::row(conn, paid.resolved.source.node, "ask.paid_by")
            .await?
            .and_then(|row| row.updated_by)
            .ok_or_else(|| Error::denied("The Ask payer setting has no current writer"))?
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
            "You cannot use this provider in the conversation's organization",
        ));
    };
    let default_params =
        orch_model_contract::parameters::resolve(&protocol, &parameters, &defaults, &json!({}));
    if !default_params.as_ref().is_ok_and(|params| {
        orch_provider_contract::calls::requested_effort(params).is_ok_and(|effort| {
            effort.is_none_or(|effort| {
                person_use.allows(&orch_authz::options::Wanted {
                    purpose: Some(orch_access_contract::permissions::ProviderPurpose::Ask),
                    effort: Some(effort),
                    ..orch_authz::options::Wanted::default()
                })
            })
        })
    }) {
        return Err(Error::denied("This identity is not usable in ask.models"));
    }
    let effort = orch_provider_contract::calls::requested_effort(&params).map_err(Error::denied)?;
    let wanted = orch_authz::options::Wanted {
        purpose: Some(orch_access_contract::permissions::ProviderPurpose::Ask),
        model: Some((&model.model, &found.groups)),
        effort,
        ..orch_authz::options::Wanted::default()
    };
    if !person_use.allows(&wanted) {
        return Err(Error::denied(
            "Your provider Use entry excludes this model or effort",
        ));
    }
    let grant =
        orch_authz::usable::usable(conn, &payer_principal, provider.id, Some(root), &policies)
            .await
            .map_err(Error::native)?;
    let orch_authz::usable::Usable::Yes(granted) = grant else {
        return Err(Error::denied(
            "The current payer cannot use this provider here",
        ));
    };
    if !granted.allows(&wanted) {
        return Err(Error::denied(
            "The current payer cannot use this model or effort",
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
