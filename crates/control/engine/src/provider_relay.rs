//! Public model calls retain their original provider, actor, request digest and accounting operands.

use crate::EngineError;
use orch_access_contract::permissions::{Arrangement, Permission};
use orch_access_contract::whereabouts::Whereabouts;
use orch_authz::{Policy, Principal};
use orch_db::{Conn, DbError, queries::resources::thread_engine as rows};
use orch_provider_contract::models::{
    ModelParameters, ProviderRelayOptions, composed_model_parameters,
};
use orch_types::{Id, Json};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("the caller does not hold Relay for these model options")]
    Denied,
    #[error("Provider Relay is off.")]
    Off,
    #[error("subscriptions use a normal thread rather than a public model relay")]
    Subscription,
    #[error("this model is not enabled and opened for Relay")]
    Model,
    #[error("the provider is unavailable")]
    Unavailable,
    #[error("{0}")]
    Options(&'static str),
    #[error(transparent)]
    Db(#[from] DbError),
    #[error(transparent)]
    Authz(#[from] orch_authz::Error),
    #[error(transparent)]
    Internal(#[from] EngineError),
}

/// Current effect operands, never an authority cache or a public provider projection.
pub struct Selection {
    pub provider: rows::ProviderRow,
    pub model: rows::ModelRow,
    pub parameters: ModelParameters,
    pub entry: Option<Id>,
}

/// The same model grant, including one complete entry rather than member-wise unions.
/// A standalone SDK call supplies no attested environment and cannot borrow a location-bound grant.
pub async fn select(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    model: &str,
    protocol: &str,
    body: &Json,
) -> Result<Selection, Error> {
    select_inner(
        conn,
        principal,
        provider,
        model,
        protocol,
        orch_provider_contract::calls::requested_effort(body).map_err(Error::Options)?,
        false,
    )
    .await
}

/// A continuing call keeps its original effective effort while current grants are restored.
pub async fn select_frozen(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    model: &str,
    protocol: &str,
    effort: Option<&str>,
) -> Result<Selection, Error> {
    select_inner(conn, principal, provider, model, protocol, effort, true).await
}

async fn select_inner(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    model: &str,
    protocol: &str,
    requested_effort: Option<&str>,
    frozen: bool,
) -> Result<Selection, Error> {
    let (row, configured, grant) = opened(conn, principal, provider, Some(protocol)).await?;
    if !configured.models.iter().any(|opened| opened == model) {
        return Err(Error::Model);
    }
    let model = rows::model(conn, provider, model)
        .await?
        .filter(|row| row.enabled)
        .ok_or(Error::Model)?;
    let native = if row.protocols.iter().any(|native| native == protocol) {
        protocol.to_owned()
    } else {
        let ends = row.protocols.iter().map(String::as_str).collect::<Vec<_>>();
        orch_model_contract::translator::builtin_translation(protocol, &ends)
            .ok_or(Error::Unavailable)?
            .to
    };
    let parameters = composed_model_parameters(
        &model.parameters,
        &model.groups,
        &model.option_layers,
        Some(&native),
    )
    .map_err(EngineError::unreadable)?;
    let effort = if frozen {
        requested_effort
    } else {
        requested_effort.or(parameters.default_effort.as_deref())
    };
    let unknown = Whereabouts::default();
    let wanted = orch_authz::options::Wanted {
        purpose: orch_access_contract::permissions::UsagePurpose::Relay,
        model: Some((&model.model_name, &model.groups)),
        effort,
        service: None,
        arrangement: Some(Arrangement::KeySeparated),
        at: Some(&unknown),
    };
    let allowing = grant.allowing(&wanted);
    if allowing.is_empty() {
        return Err(Error::Denied);
    }
    let entry = grant.entry_among(&allowing);
    Ok(Selection {
        provider: row,
        model,
        parameters,
        entry,
    })
}

/// Refreshing an offering is descriptive; current owner-authored request policy remains authoritative.
pub fn same_request_policy(frozen: &Json, current: &ModelParameters) -> Result<bool, EngineError> {
    let mut frozen: ModelParameters =
        serde_json::from_value(frozen.clone()).map_err(EngineError::unreadable)?;
    let mut current = current.clone();
    frozen.offered = None;
    current.offered = None;
    Ok(frozen == current)
}

async fn opened(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    protocol: Option<&str>,
) -> Result<
    (
        rows::ProviderRow,
        ProviderRelayOptions,
        orch_authz::usable::Use,
    ),
    Error,
> {
    let grant = orch_authz::resources::model_permission(
        conn,
        principal,
        provider,
        Permission::ProviderRelay,
        Policy::default(),
    )
    .await?
    .map_err(|_| Error::Denied)?;
    let row = rows::provider(conn, provider)
        .await?
        .ok_or(Error::Unavailable)?;
    if row.kind != "api" {
        return Err(Error::Subscription);
    }
    let configured: ProviderRelayOptions = serde_json::from_value(
        orch_db::queries::provider_relay::configuration(conn, provider)
            .await?
            .ok_or(Error::Unavailable)?,
    )
    .map_err(EngineError::unreadable)?;
    if !configured.enabled {
        return Err(Error::Off);
    }
    if row.deleted_at.is_some()
        || row.status == "disabled"
        || orch_db::queries::provider_connections::available(conn, provider).await? == Some(false)
    {
        return Err(Error::Unavailable);
    }
    if !row
        .arrangements
        .iter()
        .any(|arrangement| arrangement == "key_separated")
    {
        return Err(Error::Unavailable);
    }
    if let Some(protocol) = protocol
        && !row.protocols.iter().any(|native| native == protocol)
    {
        let translation = orch_settings::get::<bool>(
            conn,
            orch_settings::Place::Provider(provider),
            orch_settings_contract::registry::keys::PROVIDERS_TRANSLATION,
        )
        .await
        .map_err(EngineError::unreadable)?
        .unwrap_or(true);
        if !translation
            || !row.protocols.iter().any(|native| {
                orch_model_contract::translator::builtin_translation(protocol, &[native.as_str()])
                    .is_some()
            })
        {
            return Err(Error::Unavailable);
        }
    }
    Ok((row, configured, grant))
}

/// List the actual opened offerings under one current grant frame, without per-model database reads.
/// A nondefault enabled effort may make a model usable even when its default is outside the grant.
pub async fn models(
    conn: &mut Conn,
    principal: &Principal,
    provider: Id,
    protocol: Option<&str>,
) -> Result<Vec<orch_db::queries::resources::ProviderModelRow>, Error> {
    let (_, configured, grant) = opened(conn, principal, provider, protocol).await?;
    let all = orch_db::queries::resources::models_of_provider(conn, provider).await?;
    let opened: std::collections::BTreeSet<_> = configured.models.into_iter().collect();
    let unknown = Whereabouts::default();
    let mut shown = Vec::new();
    for model in all {
        if !model.enabled || !opened.contains(&model.model_name) {
            continue;
        }
        let parameters = composed_model_parameters(
            &model.parameters,
            &model.groups,
            &model.option_layers,
            protocol,
        )
        .map_err(EngineError::unreadable)?;
        let mut efforts = std::iter::once(parameters.default_effort.as_deref()).chain(
            parameters
                .efforts
                .as_deref()
                .unwrap_or_default()
                .iter()
                .map(|effort| Some(effort.as_str())),
        );
        if efforts.any(|effort| {
            !grant
                .allowing(&orch_authz::options::Wanted {
                    purpose: orch_access_contract::permissions::UsagePurpose::Relay,
                    model: Some((&model.model_name, &model.groups)),
                    effort,
                    service: None,
                    arrangement: Some(Arrangement::KeySeparated),
                    at: Some(&unknown),
                })
                .is_empty()
        }) {
            shown.push(model);
        }
    }
    Ok(shown)
}

impl crate::Engine {
    /// Actual paid work remains attributed to its accepted caller after a stop, move or lost reader.
    /// A known receipt consumes the report even when an invalid peer or model is refused.
    pub(crate) async fn retain_relay_usage(
        &self,
        unit: &mut crate::unit::Unit<'_>,
        machine: Id,
        provider: Id,
        usage: &orch_link_contract::events::LeaseUsage,
        at: chrono::DateTime<chrono::Utc>,
    ) -> Result<bool, EngineError> {
        let Some(call) =
            orch_db::queries::provider_relay::call(&mut *unit.conn, usage.lease_id).await?
        else {
            return Ok(false);
        };
        if call.provider_id != provider
            || call.machine_id != Some(machine)
            || call.model_name != usage.model
        {
            tracing::warn!(call=%call.id,"a public model report changed its original association");
            return Ok(true);
        }
        let Some(command) = orch_db::queries::link::command(&mut *unit.conn, call.id).await? else {
            return Ok(true);
        };
        if command.kind != "provider.relay"
            || command.machine_id != machine
            || command.body != call.command_body
        {
            return Ok(true);
        }
        let value = serde_json::to_value(usage.usage)?;
        let protocol = usage
            .timing
            .as_ref()
            .and_then(|timing| timing.wire_protocol.as_deref())
            .unwrap_or(&call.protocol);
        self.count_own_use(
            &mut *unit.conn,
            &crate::turns::OwnUse {
                purpose: crate::turns::OwnPurpose::Relay,
                timing: usage.timing.as_ref(),
                id: call.id,
                at,
                topic: call.org_id,
                thread: None,
                provider: call.provider_id,
                entry: call.entry_id,
                person: Some(call.user_id),
                model: &call.model_name,
                protocol: Some(protocol),
                tokens: crate::providers::tokens_of(&value),
                request_ref: usage.request_ref.as_deref(),
            },
        )
        .await?;
        orch_db::queries::provider_relay::native_done(&mut *unit.conn, call.id, at).await?;
        Ok(true)
    }
}

/// The original HTTP credential and immutable native call remain conjunctive through runtime admission.
pub struct RuntimeAdmission {
    source: crate::operation_scope::provider_relay::CallSource,
    raw: crate::operation_scope::http_origin::HttpOrigin,
}
impl RuntimeAdmission {
    pub fn new(
        source: crate::operation_scope::provider_relay::CallSource,
        raw: crate::operation_scope::http_origin::HttpOrigin,
    ) -> Self {
        Self { source, raw }
    }
    pub(crate) async fn require(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
        provider: Id,
    ) -> Result<(), EngineError> {
        self.raw.lock_identity(conn).await?;
        if self.source.provider() != provider {
            return Err(EngineError::unreadable(
                "the public call changed its provider",
            ));
        }
        self.source
            .check(conn, clock, &self.raw)
            .await
            .map_err(|error| EngineError::BeforeEffect(Box::new(error)))
    }
}
