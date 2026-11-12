//! Native paragraph work. The original act, source selection and real completion journal stay
//! separate from a reader's disposable capability preview. No transaction spans model IO.
use super::{options, sources};
use crate::{Engine, EngineError, operation_scope};
use orch_api_contract::{
    routes::Endpoint,
    summaries::{SummaryChatRange, SummaryModelChoice, SummaryRefresh, SummaryTarget},
    summary_sources::{self, ResourceSource as Role},
};
use orch_authz::{Caller, CapturedOperation, Principal};
use orch_db::{
    Conn, DbError,
    queries::{resources::thread_engine as resources, summaries as rows},
};
use orch_link_contract::commands::ProviderComplete;
use orch_summarizer_model::descriptor::Config;
use orch_types::{Hash, Id, Json, Time};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub mod command;
mod events;
pub use events::node_event;
mod route;
mod run;
pub use route::{preview_route, resolve_route};
mod defaults;
pub use defaults::{ManualDefault, manual_default};

fn refused() -> EngineError {
    EngineError::BeforeEffect(Box::new(operation_scope::continuing::Error::Refused))
}
fn source_error(error: sources::SourceError) -> EngineError {
    match error {
        sources::SourceError::Storage(error) => error.into(),
        sources::SourceError::Authz(error) => error.into(),
        _ => refused(),
    }
}

/// Actual effect-free native operands, not an admission or a serializable permission.
pub struct Selection {
    /// Current text and all actual native source facts.
    pub material: sources::Material,
    /// Current target defaults and producer provenance.
    pub policy: options::Policy,
    /// Validated selected model configuration; never public metadata.
    pub config: Config,
}
impl Selection {
    /// Canonical typed facts, including every contributing resource.
    pub fn facts(&self) -> Vec<operation_scope::sources::Facts<Role>> {
        let mut groups = BTreeMap::new();
        for target in &self.material.inputs {
            groups
                .entry(target.object.kind)
                .or_insert_with(Vec::new)
                .push(target.clone());
        }
        vec![
            operation_scope::sources::Facts::one(
                Role::Target,
                self.material.identity.fact().object.kind,
                self.material.identity.fact().clone(),
            ),
            operation_scope::sources::Facts::collection(Role::Inputs, groups.into_iter().collect()),
        ]
    }
}

/// Select under the consuming operation's common Read roles before materializing words.
/// The caller is still admitted against the complete returned facts by the native owner.
pub async fn select(
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    range: Option<SummaryChatRange>,
    model: Option<&SummaryModelChoice>,
    now: Time,
) -> Result<Selection, EngineError> {
    select_with_snapshot(conn, caller, target, range, model, now, None).await
}

/// Re-read under the original act using only its factual cut positions as a planning seed.
/// Source Read, masking, policy, provider Use and complete publication guards remain current.
pub async fn select_with_snapshot(
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    range: Option<SummaryChatRange>,
    model: Option<&SummaryModelChoice>,
    now: Time,
    snapshot: Option<&Json>,
) -> Result<Selection, EngineError> {
    let plan = operation_scope::Prepared::new(
        SummaryRefresh::ROUTE,
        summary_sources::RESOURCE_REFRESH.operation,
    )
    .map_err(|_| refused())?;
    let principal = Principal::load(
        conn,
        plan.preparation_caller(caller).map_err(|_| refused())?,
        now,
    )
    .await?;
    let identity = sources::identify(conn, target)
        .await
        .map_err(source_error)?;
    let policy = options::resolve(conn, &identity).await?;
    let mut config = match policy
        .producer
        .as_ref()
        .filter(|producer| producer.kind == "model")
    {
        Some(producer) => Config::read(&producer.config).map_err(|_| refused())?,
        None if model.is_some() => serde_json::from_value(
            serde_json::json!({"provider_id":model.map(|m|m.provider_id),"model_name":model.map(|m|&m.model)}),
        )?,
        None => return Err(refused()),
    };
    if let Some(model) = model {
        config.provider_id = model.provider_id;
        config.model_name = model.model.clone();
        config.params = model
            .params
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
    }
    config.context_tokens = policy.context_tokens.get();
    let previous = snapshot
        .map(sources::context_plan)
        .transpose()
        .map_err(source_error)?
        .flatten();
    if let Some(plan) = &previous {
        if plan.budget.tokens > policy.context_tokens.get() {
            return Err(refused());
        }
        config.context_tokens = plan.budget.tokens;
    } else {
        config.context_tokens = super::budget::context_tokens(conn, &config).await?;
    }
    Config::read(&serde_json::to_value(&config)?).map_err(|_| refused())?;
    let field = if matches!(target, SummaryTarget::ChatTitle { .. }) {
        orch_summarizer_contract::descriptor::SummaryField::Title
    } else {
        orch_summarizer_contract::descriptor::SummaryField::Summary
    };
    let budget = orch_summarizer_model::context::resource_budget(&config, field, &policy.limits())
        .map_err(|_| refused())?;
    if previous.as_ref().is_some_and(|plan| plan.budget != budget) {
        return Err(refused());
    }
    let scope = sources::ReadScope::new(&plan, 0, 1).map_err(source_error)?;
    let material = sources::materialize_with_plan(
        conn,
        &principal,
        target,
        Some(&scope),
        range,
        budget,
        previous,
    )
    .await
    .map_err(source_error)?;
    Ok(Selection {
        material,
        policy,
        config,
    })
}

pub(super) fn completion_protocol(protocols: &[String]) -> Result<Option<String>, EngineError> {
    let interfaces = orch_provider_contract::capabilities::protocol_interfaces()?;
    Ok(protocols
        .iter()
        .find(|protocol| {
            orch_model_contract::protocol::SUPPORTED.contains(&protocol.as_str())
                && interfaces
                    .get(*protocol)
                    .is_some_and(|interface| interface.kind == "generation")
        })
        .cloned())
}

/// The native ordinary provider door, retaining this exact caller and its Use restrictions.
pub async fn require_model(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    config: &Config,
) -> Result<(), EngineError> {
    model_entry(conn, principal, place, config)
        .await
        .map(|_| ())
}

/// Private current Use entry for the original ledger, under this exact principal and model.
pub async fn model_entry(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    config: &Config,
) -> Result<Option<Id>, EngineError> {
    let provider = resources::provider(conn, config.provider_id)
        .await?
        .filter(|row| {
            row.deleted_at.is_none()
                && crate::providers::request_status_available(&row.status)
                && !row.protocols.is_empty()
        })
        .ok_or_else(refused)?;
    if completion_protocol(&provider.protocols)?.is_none() {
        return Err(refused());
    }
    let model = resources::model(conn, config.provider_id, &config.model_name)
        .await?
        .filter(|row| row.enabled)
        .ok_or_else(refused)?;
    let effective = crate::resolved::load(conn, place).await?;
    if effective.models_allowed.as_ref().is_some_and(|patterns| {
        !crate::models::allowed_by(patterns, &config.model_name, &model.groups)
    }) || crate::provider_policy::forbidden(conn, place, provider.id)
        .await?
        .is_some()
    {
        return Err(refused());
    }
    let policies = crate::access::resource_policies(conn, provider.id).await?;
    let usable =
        orch_authz::usable::usable(conn, principal, provider.id, Some(place), &policies).await?;
    let orch_authz::usable::Usable::Yes(usable) = usable else {
        return Err(refused());
    };
    let effort =
        orch_provider_contract::calls::requested_effort(&config.params).map_err(|_| refused())?;
    // One entry has to allow the model and the effort together.
    if !usable.allows(&orch_authz::options::Wanted {
        model: Some((&config.model_name, &model.groups)),
        effort,
        ..orch_authz::options::Wanted::default()
    }) {
        return Err(refused());
    }
    let machine = provider.key_machine_id.ok_or_else(refused)?;
    if !orch_db::queries::resources::machine(conn, machine)
        .await?
        .is_some_and(|machine| machine.online)
    {
        return Err(refused());
    }
    Ok(usable.entry)
}

/// Immutable accepted native work. No lease, credential secret or successful admission is
/// serialized here. Every effect/publication restores and intersects the original operation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    origin: CapturedOperation,
    http: Option<operation_scope::http_origin::HttpOrigin>,
    request: Id,
    target: SummaryTarget,
    issuer: Option<Id>,
    payer: Id,
    model: SummaryModelChoice,
    config: Config,
    policy: Hash,
    source: Json,
    refs: Vec<SummaryTarget>,
    range: Option<SummaryChatRange>,
}
impl std::fmt::Debug for Work {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SummaryWork([private])")
    }
}
impl Work {
    /// Capture only after complete native admission; the body cannot supply this origin.
    pub fn capture(
        request: &rows::Request,
        principal: &Principal,
        selected: &Selection,
    ) -> Result<Self, EngineError> {
        let origin = match &request.accepted_operation {
            Some(origin) => serde_json::from_value::<CapturedOperation>(origin.clone())?,
            None => {
                CapturedOperation::capture(principal.caller()).map_err(EngineError::unreadable)?
            }
        };
        if origin.operation() != summary_sources::RESOURCE_REFRESH.operation
            || request.paying_user != Some(principal.caller().user())
            || request
                .requested_by
                .is_some_and(|issuer| issuer != principal.caller().user())
        {
            return Err(refused());
        }
        let http = request
            .accepted_source
            .get("http")
            .filter(|value| !value.is_null())
            .cloned()
            .map(serde_json::from_value::<operation_scope::http_origin::HttpOrigin>)
            .transpose()?;
        if http.as_ref().is_some_and(|http| http.origin() != &origin) {
            return Err(refused());
        }
        Ok(Self {
            origin,
            http,
            request: request.id,
            target: selected.material.identity.target(),
            issuer: request.requested_by,
            payer: principal.caller().user(),
            model: SummaryModelChoice {
                provider_id: selected.config.provider_id,
                model: selected.config.model_name.clone(),
                params: Some(selected.config.params.clone()),
            },
            config: selected.config.clone(),
            policy: selected.policy.fingerprint,
            source: selected.material.snapshot.clone(),
            refs: selected.material.source_refs.clone(),
            range: selected.material.range,
        })
    }
    pub(crate) fn issuer(&self) -> Option<Id> {
        self.issuer
    }
    pub(crate) fn request(&self) -> Id {
        self.request
    }
    pub(crate) fn context_tokens(&self) -> u32 {
        self.config.context_tokens
    }
    /// Fallback candidates retain this original caller/source and get a fresh provider binding.
    pub(crate) fn candidate(&self, request: &ProviderComplete) -> Self {
        let mut work = self.clone();
        work.model = SummaryModelChoice {
            provider_id: request.provider_id,
            model: request.model_name.clone(),
            params: request.params.clone(),
        };
        work
    }
    pub(crate) async fn caller(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
    ) -> Result<Caller, EngineError> {
        if let Some(http) = &self.http {
            return http
                .current(conn, clock)
                .await
                .map(|current| current.caller().clone())
                .map_err(command::frame_error);
        }
        self.origin
            .restore_current(conn, clock)
            .await?
            .ok_or_else(refused)
    }
    pub(crate) async fn frame(
        &self,
        conn: &mut Conn,
        clock: &dyn orch_types::Clock,
    ) -> Result<operation_scope::continuing::Frame<command::Source>, EngineError> {
        if let Some(http) = &self.http {
            if http.origin() != &self.origin {
                return Err(refused());
            }
            return http
                .prepare::<command::Source>(conn, clock, self.clone())
                .await
                .map_err(command::frame_error);
        }
        operation_scope::continuing::prepare::<command::Source>(
            conn,
            clock,
            &self.origin,
            self.clone(),
        )
        .await
        .map_err(command::frame_error)
    }
    pub(crate) fn bind_command(
        &self,
        command: &crate::commands::Outgoing,
        native: crate::commands::CommandAuthority,
    ) -> Result<crate::commands::CommandAuthority, EngineError> {
        match &self.http {
            Some(http) => http.command(command, native),
            None => Ok(native),
        }
    }
    /// All effective caps, including the original person's weekly summary cap, use the shared matcher.
    pub async fn room(
        &self,
        engine: &Engine,
        conn: &mut Conn,
        request: &ProviderComplete,
    ) -> Result<bool, EngineError> {
        let tier = request.params.as_ref().and_then(|params| {
            orch_model_contract::parameters::requested_tier_for_caps(&request.protocol, params)
        });
        let identity = sources::identify(conn, self.target)
            .await
            .map_err(source_error)?;
        engine
            .room_for_own_use(
                conn,
                &crate::caps::OwnCapAsk {
                    place: identity.place(),
                    provider: request.provider_id,
                    model: &request.model_name,
                    tier: tier.as_deref(),
                    purpose: "summary",
                    person: self.issuer,
                },
            )
            .await
    }
}

/// Hold only the native source/settings/provider footprint on the caller's short transaction.
pub async fn lock_selection(conn: &mut Conn, selected: &Selection) -> Result<(), EngineError> {
    command::lock_sources(
        conn,
        selected.material.identity.target(),
        &selected.material.source_refs,
        selected.config.provider_id,
    )
    .await
}
