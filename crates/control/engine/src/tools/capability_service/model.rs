//! The model backend uses the same original source, native journal and measured completion path.
use super::*;
use orch_link_contract::commands::{
    NativeCapabilityInput, ProviderComplete, ProviderCompleteResult, ProviderGenerate,
    ProviderGenerateResult,
};
use orch_model_contract::generation::{
    GenerationInput, GenerationMessage, GenerationPart, GenerationRole,
};
use orch_provider_contract::capabilities::Binding;

/// The selected interface, never a guessed replacement provider or model.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ModelPlan {
    pub(super) protocol: String,
    #[serde(default)]
    pub(super) complete: bool,
}

pub(super) struct RequestFacts {
    pub(super) provider_id: Id,
    pub(super) model_name: String,
    pub(super) protocol: String,
    pub(super) params: Option<Json>,
    pub(super) model_parameters: orch_provider_contract::models::ModelParameters,
    pub(super) capture_native_result: bool,
    pub(super) native_capability: Option<NativeCapabilityInput>,
}
impl RequestFacts {
    pub(super) fn generate(request: &ProviderGenerate) -> Self {
        Self {
            provider_id: request.provider_id,
            model_name: request.model_name.clone(),
            protocol: request.protocol.clone(),
            params: request.params.clone(),
            model_parameters: request.model_parameters.clone(),
            capture_native_result: request.capture_native_result,
            native_capability: request.native_capability.clone(),
        }
    }
    pub(super) fn read(complete: bool, body: &Json) -> Result<Self, EngineError> {
        if !complete {
            return Ok(Self::generate(&serde_json::from_value(body.clone())?));
        }
        let request: ProviderComplete = serde_json::from_value(body.clone())?;
        Ok(Self {
            provider_id: request.provider_id,
            model_name: request.model_name,
            protocol: request.protocol,
            params: request.params,
            model_parameters: request
                .model_parameters
                .ok_or(EngineError::StaleToolSource)?,
            capture_native_result: false,
            native_capability: None,
        })
    }
}

pub(super) fn parameters(
    row: &resources::thread_engine::ModelRow,
    protocol: &str,
) -> Result<orch_provider_contract::models::ModelParameters, EngineError> {
    Ok(orch_provider_contract::models::composed_model_parameters(
        &row.parameters,
        &row.groups,
        &row.option_layers,
        Some(protocol),
    )?)
}

pub(super) fn available(
    sources: &BuiltinSources,
    upstreams: &Json,
    row: &resources::thread_engine::ModelRow,
    capability: &str,
) -> Result<bool, EngineError> {
    Ok(projection(sources, upstreams, row, capability)?.is_some())
}

pub(super) fn projection(
    sources: &BuiltinSources,
    upstreams: &Json,
    row: &resources::thread_engine::ModelRow,
    capability: &str,
) -> Result<Option<ModelBackendFacts>, EngineError> {
    if !row.enabled {
        return Ok(None);
    }
    let upstreams: Vec<orch_api_contract::resources::Upstream> =
        serde_json::from_value(upstreams.clone())?;
    for upstream in upstreams {
        if !orch_model_contract::protocol::SUPPORTED.contains(&upstream.protocol.as_str()) {
            continue;
        }
        let parameters = parameters(row, &upstream.protocol)?;
        let mut ceiling = parameters.clone();
        let endpoint = orch_model_contract::services::generation_endpoint(
            &upstream.protocol,
            &upstream.base_url,
            &upstream.options,
        )
        .map_err(EngineError::unreadable)?;
        let Some(binding) = orch_provider_contract::services::native_binding(
            sources.capabilities().declarations(),
            &endpoint,
            &row.model_name,
            &ceiling,
            capability,
        ) else {
            continue;
        };
        let Some(auxiliary) = binding.extensions.get("auxiliary") else {
            continue;
        };
        let options = auxiliary
            .get("options")
            .cloned()
            .unwrap_or_else(|| serde_json::json!({}));
        if let Some(search) = options.get("web_search") {
            let Ok(mode) = orch_provider_contract::search::mode(search) else {
                continue;
            };
            if !ceiling
                .web_search_modes()
                .iter()
                .any(|allowed| allowed == mode)
            {
                continue;
            }
        }
        let address = url::Url::parse(&endpoint.address).map_err(EngineError::unreadable)?;
        let projected =
            sources.endpoint_capabilities(&endpoint.interface, &address, &row.model_name);
        ceiling
            .native_tools
            .entry(endpoint.interface.clone())
            .or_default()
            .extend(projected.native_tools);
        if orch_model_contract::parameters::constrain(&endpoint.interface, &ceiling, &options)
            .is_ok()
        {
            return Ok(Some(ModelBackendFacts {
                endpoint,
                parameters,
                binding,
                options,
                native_switches: projected.native_switches,
            }));
        }
    }
    Ok(None)
}

/// Current model Use operands shared by wire-backed and native program helpers.
pub(super) struct ModelUse<'a> {
    pub place: Id,
    pub provider: Id,
    pub offering: &'a str,
    pub groups: &'a [String],
    pub effort: Option<&'a str>,
}

pub(super) async fn offering_use(
    conn: &mut Conn,
    caller: &Caller,
    request: &ModelUse<'_>,
    clock: &dyn Clock,
) -> Result<Option<Option<Id>>, EngineError> {
    let principal = Principal::load(conn, caller.clone(), clock.time()).await?;
    let policies = crate::access::resource_policies(conn, request.provider).await?;
    let usable = orch_authz::usable::usable(
        conn,
        &principal,
        request.provider,
        Some(request.place),
        &policies,
    )
    .await?;
    let Some(granted) = usable.granted() else {
        return Ok(None);
    };
    let options = granted.provider();
    if !orch_authz::options::allows_model(&options, request.offering, request.groups)
        || options.efforts.is_some()
            && request
                .effort
                .is_none_or(|effort| !options.allows_effort(effort))
    {
        return Ok(None);
    }
    let def = super::super::definition_of(keys::MODELS_ALLOWED)?;
    let snapshot = orch_settings::Snapshot::load(
        conn,
        Place::Node(request.place),
        orch_settings::Wanted::Keys(&[def]),
    )
    .await?;
    for part in snapshot.resolve(def).parts {
        let patterns: Vec<String> = serde_json::from_value(part.value)?;
        if !orch_agent_contract::discovery::ModelPattern::list_names(
            &patterns,
            request.offering,
            request.groups,
        ) {
            return Ok(None);
        }
    }
    Ok(Some(granted.entry))
}

pub(super) async fn model_use(
    conn: &mut Conn,
    caller: &Caller,
    evidence: &ServiceCommand,
    request: &RequestFacts,
    clock: &dyn Clock,
) -> Result<Option<Option<Id>>, EngineError> {
    let params = request
        .params
        .as_ref()
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let effort = orch_provider_contract::calls::requested_effort(&params)
        .map_err(EngineError::unreadable)?
        .or(request.model_parameters.default_effort.as_deref());
    offering_use(
        conn,
        caller,
        &ModelUse {
            place: evidence.origin.place()?,
            provider: evidence.provider,
            offering: &evidence.offering,
            groups: &evidence.groups,
            effort,
        },
        clock,
    )
    .await
}

pub(super) async fn current(
    evidence: &ServiceCommand,
    conn: &mut Conn,
    command: &link::NewCommand<'_>,
    clock: &dyn Clock,
    origins: &origin::Origins,
) -> Result<Option<CurrentCallers>, EngineError> {
    let Some(plan) = &evidence.model else {
        return Ok(None);
    };
    if let Some(backend) = &evidence.backend
        && (backend.executor != "model-backed"
            || backend.offering().is_none_or(|selected| {
                selected.provider != evidence.provider || selected.offering != evidence.offering
            }))
    {
        return Ok(None);
    }
    let request = RequestFacts::read(plan.complete, command.body)?;
    if request.provider_id != evidence.provider
        || request.model_name != evidence.offering
        || request.protocol != plan.protocol
        || evidence.backend.is_some()
            && (!request.capture_native_result
                || request
                    .native_capability
                    .as_ref()
                    .is_none_or(|native| native.capability != evidence.capability))
        || request
            .native_capability
            .as_ref()
            .is_some_and(|native| native.capability != evidence.capability)
    {
        return Ok(None);
    }
    let Some(row) =
        resources::thread_engine::model(conn, evidence.provider, &evidence.offering).await?
    else {
        return Ok(None);
    };
    if !row.enabled
        || row.groups != evidence.groups
        || parameters(&row, &plan.protocol)? != request.model_parameters
    {
        return Ok(None);
    }
    for caller in origins.callers() {
        if model_use(conn, caller, evidence, &request, clock)
            .await?
            .is_none()
        {
            return Ok(None);
        }
    }
    let tier = request.params.as_ref().and_then(|params| {
        orch_model_contract::parameters::requested_service_tier(&request.protocol, params)
    });
    if origins.callers.is_empty()
        || !crate::caps::own_room(
            conn,
            &crate::caps::OwnCapAsk {
                place: evidence.origin.place()?,
                provider: evidence.provider,
                model: &evidence.offering,
                tier: tier.as_deref(),
                purpose: &evidence.capability,
                person: origins.person,
            },
            clock.now(),
        )
        .await?
    {
        return Ok(None);
    }
    if origins.callers.iter().any(|(caller, until)| {
        caller
            .as_ref()
            .is_some_and(|caller| !caller.key_deadline_allows(clock))
            || until.is_some_and(|until| clock.time() >= until)
    }) {
        return Ok(None);
    }
    Ok(Some(origins.callers.clone()))
}

fn binding(
    sources: &BuiltinSources,
    upstreams: &Json,
    request: &ProviderGenerate,
) -> Result<Binding, EngineError> {
    let upstreams: Vec<orch_api_contract::resources::Upstream> =
        serde_json::from_value(upstreams.clone())?;
    let endpoint = upstreams
        .into_iter()
        .find(|upstream| upstream.protocol == request.protocol)
        .ok_or(EngineError::StaleToolSource)?;
    let endpoint = orch_model_contract::services::generation_endpoint(
        &endpoint.protocol,
        &endpoint.base_url,
        &endpoint.options,
    )
    .map_err(EngineError::unreadable)?;
    let native = request
        .native_capability
        .as_ref()
        .ok_or(EngineError::ToolCallIdentity)?;
    let binding = orch_provider_contract::services::native_binding(
        sources.capabilities().declarations(),
        &endpoint,
        &request.model_name,
        &request.model_parameters,
        &native.capability,
    )
    .ok_or(EngineError::StaleToolSource)?;
    if orch_provider_contract::services::definition_hash(&endpoint, &binding)? != native.definition
    {
        return Err(EngineError::StaleToolSource);
    }
    Ok(binding)
}

impl Engine {
    pub(super) async fn execute_web_model_service(
        &self,
        sources: &BuiltinSources,
        call: &CallContext,
        capability: &str,
    ) -> Result<Json, EngineError> {
        let input = accepted::input(call, capability)?;
        let images = accepted::images();
        if call
            .arguments
            .get("images")
            .and_then(Json::as_array)
            .is_some_and(|images| !images.is_empty())
            && images.is_empty()
        {
            return Err(EngineError::unreadable(
                "this model capability needs its required input-image materializer before execution",
            ));
        }
        let id = recovery_binding::command(source::resource_id(
            "provider-service",
            crate::ids::tool_result(call.thread, &call.call_ref),
        ))?;
        let (command, evidence, binding) = {
            let mut tx = self.db().begin().await?;
            source::check_call(&mut tx, call).await?;
            if let Some(retained) = link::command_with_authority(&mut tx, id).await? {
                let Some(CommandAuthority::CapabilityService(evidence)) =
                    retained.authority.map(serde_json::from_value).transpose()?
                else {
                    return Err(EngineError::ToolCallIdentity);
                };
                let current_source = source::Source::captured()?;
                if evidence.origin.tool().is_none_or(|(saved, original)| {
                    saved != &current_source
                        || original.tool != call.tool
                        || original.arguments != call.arguments
                }) || evidence.model.is_none()
                    || evidence.capability != capability
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let request: ProviderGenerate = serde_json::from_value(retained.command.body)?;
                if request
                    .native_capability
                    .as_ref()
                    .is_none_or(|native| native.capability != capability || native.input != input)
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                if request
                    .input
                    .messages
                    .first()
                    .is_none_or(|message| message.content.get(1..) != Some(images.as_slice()))
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let codec = binding(sources, &evidence.upstreams, &request)?;
                let mut outgoing = Outgoing::new(
                    id,
                    retained.command.machine_id,
                    SerialKey::Provider(evidence.provider),
                    &request,
                )?;
                outgoing.authority = Some(CommandAuthority::CapabilityService(evidence.clone()));
                outgoing.deadline = Some(Time::new(retained.command.deadline_at));
                tx.commit().await.map_err(orch_db::DbError::from)?;
                (outgoing, *evidence, codec)
            } else {
                let origins = source::original_callers(
                    &mut tx,
                    call,
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?;
                let choice = backend(&mut tx, call.thread, sources, capability).await?;
                if choice.executor != "model-backed" {
                    return Err(EngineError::StaleToolSource);
                }
                let selected_reference = choice.offering().ok_or(EngineError::StaleToolSource)?;
                let provider_id = &selected_reference.provider;
                let model_name = &selected_reference.offering;
                let row = resources::routes::providers(&mut tx, &[*provider_id])
                    .await?
                    .into_iter()
                    .next()
                    .ok_or(EngineError::StaleToolSource)?;
                let machine = row.key_machine_id.ok_or(EngineError::StaleToolSource)?;
                let selected = resources::thread_engine::model(&mut tx, *provider_id, model_name)
                    .await?
                    .filter(|model| model.enabled)
                    .ok_or(EngineError::StaleToolSource)?;
                let upstreams: Vec<orch_api_contract::resources::Upstream> =
                    serde_json::from_value(row.upstreams.clone())?;
                let mut ready = None;
                for upstream in upstreams {
                    if !orch_model_contract::protocol::SUPPORTED
                        .contains(&upstream.protocol.as_str())
                    {
                        continue;
                    }
                    let parameters = parameters(&selected, &upstream.protocol)?;
                    let endpoint = orch_model_contract::services::generation_endpoint(
                        &upstream.protocol,
                        &upstream.base_url,
                        &upstream.options,
                    )
                    .map_err(EngineError::unreadable)?;
                    let Some(codec) = orch_provider_contract::services::native_binding(
                        sources.capabilities().declarations(),
                        &endpoint,
                        model_name,
                        &parameters,
                        capability,
                    ) else {
                        continue;
                    };
                    let auxiliary =
                        orch_provider_contract::capabilities::render_auxiliary(&codec, &input)
                            .map_err(EngineError::unreadable)?;
                    if let Some(search) = auxiliary.pointer("/options/web_search") {
                        let mode = orch_provider_contract::search::mode(search)
                            .map_err(EngineError::unreadable)?;
                        if !parameters
                            .web_search_modes()
                            .iter()
                            .any(|allowed| allowed == mode)
                        {
                            continue;
                        }
                    }
                    let prompt = auxiliary
                        .get("prompt")
                        .and_then(Json::as_str)
                        .ok_or(EngineError::StaleToolSource)?;
                    // The narrative is bounded; source links and documents come from captured native results.
                    let output_tokens = auxiliary
                        .get("max_output_tokens")
                        .and_then(Json::as_i64)
                        .unwrap_or(1024);
                    if output_tokens <= 0 {
                        return Err(EngineError::StaleToolSource);
                    }
                    if !images.is_empty() && parameters.input_images != Some(true) {
                        continue;
                    }
                    let mut content = vec![GenerationPart::Text {
                        text: prompt.into(),
                    }];
                    content.extend(images.clone());
                    let request = ProviderGenerate {
                        native_capability: Some(NativeCapabilityInput {
                            capability: capability.into(),
                            input: input.clone(),
                            definition: orch_provider_contract::services::definition_hash(
                                &endpoint, &codec,
                            )?,
                        }),
                        capture_native_result: true,
                        provider_id: *provider_id,
                        protocol: upstream.protocol,
                        model_name: model_name.clone(),
                        params: auxiliary.get("options").cloned(),
                        model_parameters: parameters,
                        input: GenerationInput {
                            system: String::new(),
                            messages: vec![GenerationMessage {
                                role: GenerationRole::User,
                                content,
                                continuation: vec![],
                            }],
                            tools: vec![],
                            max_output_tokens: output_tokens,
                        },
                    };
                    ready = Some((request, codec));
                    break;
                }
                let (request, codec) = ready.ok_or(EngineError::StaleToolSource)?;
                let declarations = sources.capabilities().declarations();
                let definition = declarations
                    .capabilities
                    .get(capability)
                    .ok_or(EngineError::StaleToolSource)?;
                if !declarations
                    .routes
                    .get(&choice.reference.route)
                    .is_some_and(|route| {
                        route.executor.as_deref() == Some("model-backed")
                            && route.orch_tool.as_deref() == Some(call.tool.name())
                    })
                {
                    return Err(EngineError::ToolCallIdentity);
                }
                let mut outgoing =
                    Outgoing::new(id, machine, SerialKey::Provider(*provider_id), &request)?;
                if managed_tool(call.tool) {
                    outgoing.deadline = Some(source::capability_deadline(&mut tx, call).await?);
                }
                let mut evidence = ServiceCommand {
                    recovery: recovery_binding::binding(),
                    model: Some(ModelPlan {
                        protocol: request.protocol.clone(),
                        complete: false,
                    }),
                    origin: Origin::Tool {
                        source: source::Source::captured()?,
                        call: Box::new(call.clone()),
                    },
                    command: id,
                    machine,
                    body: source::hash_json(outgoing.body.clone())?,
                    provider: *provider_id,
                    offering: model_name.clone(),
                    capability: capability.into(),
                    upstreams: row.upstreams,
                    config: ServiceConfig::default(),
                    backend: Some(choice.clone()),
                    groups: selected.groups,
                    unit: definition.ledger_unit.clone(),
                    counter: None,
                    quote: None,
                    explicit_price: false,
                    entry: None,
                };
                for caller in origins.callers() {
                    let entry = model_use(
                        &mut tx,
                        caller,
                        &evidence,
                        &RequestFacts::generate(&request),
                        self.inner.clock.as_ref(),
                    )
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                    if Some(caller.user()) == origins.attribution().user_id {
                        evidence.entry = entry;
                    }
                }
                outgoing.authority = Some(CommandAuthority::CapabilityService(Box::new(
                    evidence.clone(),
                )));
                tx.commit().await.map_err(orch_db::DbError::from)?;
                (outgoing, evidence, codec)
            }
        };
        let (command, outcome) = self.run_capability_command(command, &evidence).await?;
        let crate::commands::Outcome::Succeeded(raw) = outcome else {
            return Err(EngineError::unreadable(
                "the admitted model service did not complete; actual usage was retained",
            ));
        };
        let mut result: ProviderGenerateResult = serde_json::from_value(raw)?;
        if let Some(blob) = result.native_result_blob {
            result.output.native_result = self
                .service_result_blob(call, command.id, blob, ResultBlob::Native)
                .await?;
        }
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        tx.commit().await.map_err(orch_db::DbError::from)?;
        if accepted::is_prepared() {
            let raw = if binding
                .extensions
                .get("auxiliary")
                .and_then(|value| value.get("result_envelope"))
                .and_then(Json::as_bool)
                == Some(true)
            {
                &result.output.native_result
            } else {
                result
                    .output
                    .native_result
                    .get("response")
                    .ok_or(EngineError::ToolCallIdentity)?
            };
            if accepted::capture(call, capability, command.id, &input, &binding, raw)? {
                return Ok(Json::Null);
            }
        }
        let limit = call
            .arguments
            .get("count")
            .and_then(Json::as_u64)
            .and_then(|count| usize::try_from(count).ok())
            .unwrap_or(200);
        orch_model_contract::services::project_model_result(&binding, &input, &result.output, limit)
            .map_err(EngineError::unreadable)
    }
}

pub(super) async fn counted(
    engine: &Engine,
    conn: &mut Conn,
    evidence: &ServiceCommand,
    command: link::CommandRow,
) -> Result<(), EngineError> {
    struct Measured {
        usage: orch_link_contract::foreign::TokenUsage,
        timing: orch_api_contract::usage::RequestTiming,
        request_ref: Option<String>,
    }
    let measured = match command.state.as_str() {
        "succeeded" => match command.result {
            Some(result) if evidence.model.as_ref().is_some_and(|plan| plan.complete) => {
                let result: ProviderCompleteResult = serde_json::from_value(result)?;
                Some(Measured {
                    usage: result.usage,
                    timing: result.timing,
                    request_ref: result.request_ref,
                })
            }
            Some(result) => {
                let result: ProviderGenerateResult = serde_json::from_value(result)?;
                Some(Measured {
                    usage: result.usage,
                    timing: result.timing,
                    request_ref: result.request_ref,
                })
            }
            None => None,
        },
        "failed" => match command.error {
            Some(error) => {
                crate::ask::failed_answer(&serde_json::from_value(error)?)?.map(|result| Measured {
                    usage: result.usage,
                    timing: result.timing,
                    request_ref: result.request_ref,
                })
            }
            None => None,
        },
        _ => None,
    };
    let Some(measured) = measured else {
        return Ok(());
    };
    let attribution = evidence
        .origin
        .billing(conn, evidence.provider, evidence.entry)
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    let purpose = match evidence.capability.as_str() {
        "web_search" => crate::turns::OwnPurpose::WebSearch,
        "page_fetch" => crate::turns::OwnPurpose::PageFetch,
        "image_generation" => crate::turns::OwnPurpose::ImageGeneration,
        "summary" => crate::turns::OwnPurpose::Summary,
        "ask" => crate::turns::OwnPurpose::Ask,
        _ => return Err(EngineError::StaleToolSource),
    };
    let frozen = attribution.model_attribution(evidence.provider, evidence.entry);
    engine
        .count_own_use_frozen_context(
            conn,
            &crate::turns::OwnUse {
                purpose,
                timing: Some(&measured.timing),
                id: command.id,
                at: command.updated_at,
                topic: attribution.topic(),
                thread: attribution.thread(),
                provider: evidence.provider,
                entry: evidence.entry,
                person: attribution.person(),
                model: &evidence.offering,
                protocol: evidence.model.as_ref().map(|plan| plan.protocol.as_str()),
                tokens: crate::providers::tokens_of(&serde_json::to_value(measured.usage)?),
                request_ref: measured.request_ref.as_deref(),
            },
            attribution.conversation(),
            &frozen,
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod request_fact_tests {
    use super::*;
    #[test]
    fn completion_financial_projection_keeps_its_native_body_and_requires_frozen_model_constraints()
    -> Result<(), Box<dyn std::error::Error>> {
        let request = ProviderComplete {
            model_parameters: Some(orch_provider_contract::models::ModelParameters::default()),
            provider_id: orch_types::new_id(),
            purpose: orch_link_contract::commands::CompletionPurpose::Summary,
            protocol: "fixture-generation".into(),
            model_name: "actual-summary-model".into(),
            params: Some(serde_json::json!({"effort":"low","future_option":true})),
            system: "Retain the actual instruction".into(),
            input: "Retain the exact source".into(),
            max_output_tokens: 91,
        };
        let body = serde_json::to_value(&request)?;
        let before = body.clone();
        let facts = RequestFacts::read(true, &body)?;
        assert_eq!(facts.provider_id, request.provider_id);
        assert_eq!(facts.model_name, request.model_name);
        assert_eq!(facts.params, request.params);
        assert!(!facts.capture_native_result);
        assert!(facts.native_capability.is_none());
        assert_eq!(
            body, before,
            "the shared guard projects financial facts without changing the native request"
        );
        let mut missing = body;
        missing
            .as_object_mut()
            .ok_or("expected actual completion body")?
            .remove("model_parameters");
        assert!(
            RequestFacts::read(true, &missing).is_err(),
            "an unconstrained owned request cannot gain a paid admission"
        );
        Ok(())
    }
}
