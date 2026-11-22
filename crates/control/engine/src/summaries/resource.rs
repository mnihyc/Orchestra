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

mod change;
pub mod command;
mod events;
mod failure;
pub use change::{SourceChange, source_change};
pub use events::node_event;
pub use failure::Reason;
mod route;
mod run;
#[cfg(test)]
mod tests;
pub use route::{preview_route, resolve_backend_choice, resolve_route};
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
    /// Frozen choice facts for the actual accepting producer; these contain no acting authority.
    pub fn backend_selection(&self) -> Result<Json, EngineError> {
        Ok(serde_json::to_value(ModelSelection::configured(
            &self.config,
        ))?)
    }

    /// Catch-up keeps the person's selected range. A title follows the whole current chat;
    /// its snapshot records the actual read position without turning it into a pinned range.
    #[must_use]
    pub fn pinned_range(&self) -> Option<SummaryChatRange> {
        if matches!(self.material.identity.target(), SummaryTarget::Chat { .. }) {
            self.material.range
        } else {
            None
        }
    }

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

/// The model task selected by the resource's actual native kind.
#[must_use]
pub fn field_for(target: SummaryTarget) -> orch_summarizer_contract::descriptor::SummaryField {
    if matches!(target, SummaryTarget::ChatTitle { .. }) {
        orch_summarizer_contract::descriptor::SummaryField::Title
    } else {
        orch_summarizer_contract::descriptor::SummaryField::Summary
    }
}

/// The same owned prompt operands for a current proposal and the actual admitted execution.
pub async fn request_for_selection(
    conn: &mut Conn,
    selected: &Selection,
) -> Result<orch_summarizer_contract::summarizer::SummaryRequest, EngineError> {
    let place = selected.material.identity.place();
    let target = selected.material.identity.target();
    let node = orch_db::queries::hierarchy::node(conn, place)
        .await?
        .ok_or_else(refused)?;
    let named = matches!(
        target,
        SummaryTarget::Thread { .. } | SummaryTarget::Topic { .. }
    );
    Ok(orch_summarizer_contract::summarizer::SummaryRequest {
        field: field_for(target),
        node: orch_plugin_contract::refs::NodeRef {
            id: place,
            kind: serde_json::from_value(serde_json::json!(node.kind))?,
            name: named.then(|| selected.material.title.clone()),
            redacted: !named,
        },
        previous: String::new(),
        activity: selected.material.activity.clone(),
        tag_choices: Vec::new(),
        limits: selected.policy.limits(),
        config: serde_json::to_value(&selected.config)?,
    })
}

/// Effect-free manual operands under the consuming read's actual source roles. No paid operation is captured.
pub async fn preview_selection(
    conn: &mut Conn,
    principal: &Principal,
    target: SummaryTarget,
    range: Option<SummaryChatRange>,
    scope: &sources::ReadScope<'_>,
    policy: options::Policy,
    producer: &options::Producer,
) -> Result<Selection, EngineError> {
    let mut config = Config::read(&producer.config).map_err(|_| refused())?;
    let identity = sources::identify(conn, target)
        .await
        .map_err(source_error)?;
    require_model(conn, principal, identity.place(), &config).await?;
    config.context_tokens = policy.context_tokens.get();
    config.context_tokens = super::budget::context_tokens(conn, &config, field_for(target)).await?;
    let budget = orch_summarizer_model::context::resource_budget(
        &config,
        field_for(target),
        &policy.limits(),
    )
    .map_err(|_| refused())?;
    let material =
        sources::materialize_with_plan(conn, principal, target, Some(scope), range, budget, None)
            .await
            .map_err(source_error)?;
    Ok(Selection {
        material,
        policy,
        config,
    })
}

/// A prompt estimate and the same request output reservation as the model producer, never base-rate arithmetic.
pub async fn proposal_budget(
    conn: &mut Conn,
    selected: &Selection,
) -> Result<(u32, i64), EngineError> {
    let request = request_for_selection(conn, selected).await?;
    let (system, input) = orch_summarizer_model::prompts::render(&request, &selected.config);
    let input = orch_summarizer_model::context::estimated_tokens(&system)
        .saturating_add(orch_summarizer_model::context::estimated_tokens(&input));
    let reasons = super::budget::reasons(conn, &selected.config).await?;
    Ok((
        input,
        selected.config.output_reserve(request.field, reasons),
    ))
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
    let selected = model.map(ModelSelection::chosen);
    select_with_backend_snapshot(
        conn,
        caller,
        target,
        range,
        selected.as_ref(),
        now,
        snapshot,
    )
    .await
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelSelection {
    backend: orch_provider_contract::capabilities::BackendRef,
    params: Json,
}

impl ModelSelection {
    fn configured(config: &Config) -> Self {
        Self {
            backend: config.backend.clone(),
            params: config.params.clone(),
        }
    }
    fn chosen(model: &SummaryModelChoice) -> Self {
        Self {
            backend: model.backend.clone(),
            params: model
                .params
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
        }
    }
    fn offering(&self) -> Result<&orch_provider_contract::capabilities::OfferingRef, EngineError> {
        self.backend.offering.as_ref().ok_or_else(refused)
    }
}

async fn select_with_backend_snapshot(
    conn: &mut Conn,
    caller: &Caller,
    target: SummaryTarget,
    range: Option<SummaryChatRange>,
    model: Option<&ModelSelection>,
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
        None if model.is_some() => {
            serde_json::from_value(serde_json::json!({"backend":model.map(|model|&model.backend)}))?
        }
        None => return Err(refused()),
    };
    if let Some(model) = model {
        config.backend = model.backend.clone();
        config.params = model.params.clone();
    }
    config.context_tokens = policy.context_tokens.get();
    let field = field_for(target);
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
        config.context_tokens = super::budget::context_tokens(conn, &config, field).await?;
    }
    Config::read(&serde_json::to_value(&config)?).map_err(|_| refused())?;
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

pub(crate) fn completion_protocol(protocols: &[String]) -> Result<Option<String>, EngineError> {
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

/// Recovery keeps the previous wire interface while the chosen provider still offers it.
pub(crate) fn recovery_protocol(
    protocols: &[String],
    previous: &str,
) -> Result<Option<String>, EngineError> {
    if !previous.is_empty() && protocols.iter().any(|protocol| protocol == previous) {
        Ok(Some(previous.to_owned()))
    } else {
        completion_protocol(protocols)
    }
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
    model_entry_at(conn, principal, Some(place), config).await
}

async fn model_entry_at(
    conn: &mut Conn,
    principal: &Principal,
    place: Option<Id>,
    config: &Config,
) -> Result<Option<Id>, EngineError> {
    judge_model_at(conn, principal, place, config)
        .await?
        .map_err(|_| refused())
}

/// The same backend decision supplies the actual source's safe refusal reason.
pub async fn judge_model(
    conn: &mut Conn,
    principal: &Principal,
    place: Id,
    config: &Config,
) -> Result<Result<Option<Id>, Reason>, EngineError> {
    judge_model_at(conn, principal, Some(place), config).await
}

async fn judge_model_at(
    conn: &mut Conn,
    principal: &Principal,
    place: Option<Id>,
    config: &Config,
) -> Result<Result<Option<Id>, Reason>, EngineError> {
    let offering = config.offering().map_err(EngineError::unreadable)?;
    let program = config.backend.route == "summary.delegation";
    let Some(provider) = resources::provider(conn, offering.provider).await? else {
        return Ok(Err(Reason::ProviderGone));
    };
    if provider.deleted_at.is_some() {
        return Ok(Err(Reason::ProviderGone));
    }
    if !program && !crate::providers::request_status_available(&provider.status) {
        return Ok(Err(Reason::ProviderNotReady));
    }
    if program {
        if crate::tools::capability_service::delegation::backend_state(
            conn,
            &orch_provider_contract::builtins::BuiltinSources::default(),
            "summary",
            &config.backend,
        )
        .await?
        .is_err()
        {
            return Ok(Err(Reason::NoInterface));
        }
    } else if provider.protocols.is_empty() || completion_protocol(&provider.protocols)?.is_none() {
        return Ok(Err(Reason::NoInterface));
    }
    let Some(model) = resources::model(conn, offering.provider, &offering.offering)
        .await?
        .filter(|model| model.enabled)
    else {
        return Ok(Err(Reason::ModelOff));
    };
    let (_, purpose) = orch_provider_contract::models::composed_model_with_purpose(
        &model.model_name,
        &model.parameters,
        &model.groups,
        &model.option_layers,
        None,
    )?;
    if !purpose.supports_purpose(orch_provider_contract::models::ModelPurpose::Generation) {
        return Ok(Err(Reason::NoInterface));
    }
    if let Some(place) = place {
        let effective = crate::resolved::load(conn, place).await?;
        if effective.models_allowed.as_ref().is_some_and(|patterns| {
            !crate::models::allowed_by(patterns, &offering.offering, &model.groups)
        }) {
            return Ok(Err(Reason::ModelNotAllowed));
        }
        if crate::provider_policy::forbidden(conn, place, provider.id)
            .await?
            .is_some()
        {
            return Ok(Err(Reason::ProviderForbidden));
        }
    }
    let policies = crate::access::resource_policies(conn, provider.id).await?;
    let usable =
        match orch_authz::usable::usable(conn, principal, provider.id, place, &policies).await? {
            orch_authz::usable::Usable::Yes(usable) => usable,
            orch_authz::usable::Usable::Unavailable(_) => return Ok(Err(Reason::ProviderNotReady)),
            orch_authz::usable::Usable::KeptToItsOwner | orch_authz::usable::Usable::No => {
                return Ok(Err(Reason::NotAllowed));
            }
        };
    let Ok(effort) = orch_provider_contract::calls::requested_effort(&config.params) else {
        return Ok(Err(Reason::EffortNotOffered));
    };
    let wanted = orch_authz::options::Wanted {
        purpose: orch_access_contract::permissions::UsagePurpose::Summary,
        model: Some((&offering.offering, &model.groups)),
        effort,
        ..orch_authz::options::Wanted::default()
    };
    if !usable.allows(&wanted) {
        let without_effort = orch_authz::options::Wanted {
            effort: None,
            ..wanted
        };
        return Ok(Err(if usable.allows(&without_effort) {
            Reason::EffortNotOffered
        } else {
            Reason::ModelNotAllowed
        }));
    }
    if !program {
        let Some(machine) = provider.key_machine_id else {
            return Ok(Err(Reason::NoKeeper));
        };
        if !orch_db::queries::resources::machine(conn, machine)
            .await?
            .is_some_and(|machine| machine.online)
        {
            return Ok(Err(Reason::KeeperOffline));
        }
    }
    Ok(Ok(usable.entry))
}

/// Immutable accepted native work. No lease, credential secret or successful admission is
/// serialized here. Every effect/publication restores and intersects the original operation.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Work {
    origin: CapturedOperation,
    http: Option<operation_scope::http_origin::HttpOrigin>,
    request: Id,
    call: Id,
    target: SummaryTarget,
    issuer: Option<Id>,
    payer: Id,
    selected: ModelSelection,
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
    pub(crate) fn prompt_governors(&self) -> (Id, Option<Id>, &[SummaryTarget]) {
        (self.request, self.issuer, &self.refs)
    }

    /// Capture a provisional source plan under the accepted operation's complete factual binding.
    /// Its continuing frame still restores raw identity and all current gates before any effect.
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
        selected
            .config
            .offering()
            .map_err(EngineError::unreadable)?;
        let frozen: ModelSelection = serde_json::from_value(
            request
                .accepted_source
                .get("selection")
                .cloned()
                .ok_or_else(refused)?,
        )?;
        if frozen != ModelSelection::configured(&selected.config) {
            return Err(refused());
        }
        Ok(Self {
            origin,
            http,
            request: request.id,
            call: request.call_id,
            target: selected.material.identity.target(),
            issuer: request.requested_by,
            payer: principal.caller().user(),
            selected: frozen,
            config: selected.config.clone(),
            policy: selected.policy.fingerprint,
            source: selected.material.snapshot.clone(),
            refs: selected.material.source_refs.clone(),
            range: selected.pinned_range(),
        })
    }
    pub(crate) fn issuer(&self) -> Option<Id> {
        self.issuer
    }
    pub(crate) fn request(&self) -> Id {
        self.request
    }
    pub(crate) fn call(&self) -> Id {
        self.call
    }
    pub(crate) fn context_tokens(&self) -> u32 {
        self.config.context_tokens
    }
    /// Fallback candidates retain this original caller/source and get a fresh provider binding.
    pub(crate) fn candidate(&self, request: &ProviderComplete) -> Self {
        let mut work = self.clone();
        work.selected.backend.offering = Some(orch_provider_contract::capabilities::OfferingRef {
            provider: request.provider_id,
            offering: request.model_name.clone(),
        });
        work.selected.params = request
            .params
            .clone()
            .unwrap_or_else(|| serde_json::json!({}));
        work
    }
    fn selected_candidate(&self, selected: ModelSelection) -> Self {
        let mut work = self.clone();
        work.selected = selected;
        work
    }
    pub(crate) fn program_candidate(
        &self,
        request: &orch_summarizer_model::CompletionRequest,
    ) -> Self {
        let mut work = self.clone();
        work.selected = ModelSelection {
            backend: request.backend.clone(),
            params: request
                .params
                .clone()
                .unwrap_or_else(|| serde_json::json!({})),
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
        self.read_frame(conn, clock).await
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
    let offering = selected
        .config
        .offering()
        .map_err(EngineError::unreadable)?;
    command::lock_sources(
        conn,
        selected.material.identity.target(),
        &selected.material.source_refs,
        offering.provider,
    )
    .await
}
