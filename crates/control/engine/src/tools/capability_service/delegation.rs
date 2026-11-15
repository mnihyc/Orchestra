//! A native program helper is ordinary actor-owned work with an additional original tool condition.
use super::*;
use futures::future::BoxFuture;
use orch_api_contract::threads::ContentPart;
use orch_authz::CapturedCaller;
use orch_db::queries::capability_work;
use orch_provider_contract::capabilities::ExecutionSupport;
use serde_json::json;

const SLOT: &str = "delegated_capability";
mod codecs;
pub(crate) mod executor;
mod image;
mod origin;
pub mod placement;
mod run;
mod sessions;
mod summary;
pub(crate) mod text;
mod thread_tool;
mod unit;
pub(super) use run::failure;

/// These codecs are implemented by the installed native result reader, not by a route declaration.
pub(crate) fn execution_support() -> Vec<ExecutionSupport> {
    codecs::installed()
        .map(|codecs| codecs.support())
        .unwrap_or_default()
}

/// A conjunctive condition on the helper's real creator/input authority, never an acting credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentLink {
    work: Id,
    helper: Id,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    origin: origin::Producer,
    attempt: Id,
    helper: Id,
    actor: Id,
    person: Option<Id>,
    capability: String,
    execution: ExecutionSupport,
    agent: Id,
    provider: Id,
    placement: placement::Placement,
    model: String,
    effort: Option<String>,
    content: Vec<ContentPart>,
    input: Json,
    material: Vec<orch_model_contract::generation::GenerationPart>,
    deadline: Time,
    profile: Hash,
    actor_name: String,
    actor_ref: Json,
    program_label: Json,
}

/// Freeze the same effective effort ordinary helper preflight will select, before any helper exists.
async fn helper_effort(
    conn: &mut Conn,
    provider: Id,
    model: &str,
    topic: Id,
    requested: Option<&str>,
) -> Result<Option<String>, EngineError> {
    let row = resources::thread_engine::model(conn, provider, model)
        .await?
        .filter(|row| row.enabled)
        .ok_or(EngineError::StaleToolSource)?;
    let settings = crate::resolved::load(conn, topic).await?;
    let parameters = crate::binding::model_parameters(&row)?;
    let wanted = requested.or(settings.effort.as_deref());
    if parameters.effort_is_gone(wanted) {
        return Err(EngineError::StaleToolSource);
    }
    Ok(parameters.effort_for(wanted))
}

impl Record {
    fn helper_info(&self) -> Json {
        json!({"thread_id":self.helper,"actor_id":self.actor,"actor":self.actor_ref,"actor_name":self.actor_name,"provider_id":self.provider,
            "program_label":self.program_label,"requester_thread_id":self.origin.thread()})
    }
}

impl ParentLink {
    /// Frozen request choices apply only to the producer's actual initial input.
    /// Ordinary model admission still resolves their eligibility and current limits.
    pub(crate) async fn apply_input_options(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
        settings: &mut crate::resolved::Resolved,
    ) -> Result<(), EngineError> {
        if input.thread_id != self.helper || !self.owns_input(conn, input.id).await? {
            return Err(EngineError::StaleToolSource);
        }
        let record = self.record(conn).await?;
        if input.author_id != Some(record.actor)
            || input.content != serde_json::to_value(&record.content)?
        {
            return Err(EngineError::StaleToolSource);
        }
        if let Some(source) = record.origin.summary()
            && let Some(params) = source.params()
        {
            let chosen = params.as_object().ok_or(EngineError::StaleToolSource)?;
            let actual = settings
                .params
                .as_object_mut()
                .ok_or(EngineError::StaleToolSource)?;
            actual.extend(chosen.clone());
        }
        Ok(())
    }

    /// A withdrawn or unimplemented explicit option cannot silently become a different request.
    pub(crate) async fn input_options_match(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
        params: &Json,
        effort: Option<&str>,
    ) -> Result<bool, EngineError> {
        if input.thread_id != self.helper || !self.owns_input(conn, input.id).await? {
            return Ok(false);
        }
        let record = self.record(conn).await?;
        let Some(chosen) = record.origin.summary().and_then(|source| source.params()) else {
            return Ok(true);
        };
        let chosen = chosen.as_object().ok_or(EngineError::StaleToolSource)?;
        Ok(chosen.iter().all(|(key, value)| {
            if key == "effort" {
                value.is_null() || value.as_str() == effort
            } else {
                params.get(key) == Some(value) || (value.is_null() && params.get(key).is_none())
            }
        }))
    }

    /// A copied condition cannot make a later independent input part of this attempt.
    pub(crate) async fn owns_input(&self, conn: &mut Conn, input: Id) -> Result<bool, EngineError> {
        Ok(capability_work::program(conn, self.work)
            .await?
            .is_some_and(|work| work.helper == self.helper && work.input == Some(input)))
    }

    /// The native prompt keeps the actual producer's immutable Read governors.
    pub(crate) async fn retain_prompt_governors(
        &self,
        conn: &mut Conn,
        input: Id,
        record_id: Id,
    ) -> Result<(), EngineError> {
        if !self.owns_input(conn, input).await? {
            return Err(EngineError::StaleToolSource);
        }
        let record = self.record(conn).await?;
        if let Some(source) = record.origin.summary() {
            let (request, viewer, refs) = source.prompt_governors();
            crate::model_inputs::retain_summary_governors(conn, record_id, request, viewer, &refs)
                .await?;
        } else if let Some(thread) = record.origin.thread() {
            crate::model_inputs::retain_program_governors(
                conn,
                record_id,
                self.work,
                &[orch_api_contract::summaries::SummaryTarget::Thread { id: thread }],
            )
            .await?;
        } else {
            return Err(EngineError::StaleToolSource);
        }
        Ok(())
    }

    /// Only the actual owned input can mark its source at a successful native wire cut.
    pub(crate) async fn mark_model_admitted(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
        at: Time,
    ) -> Result<(), EngineError> {
        if input.thread_id != self.helper || !self.owns_input(conn, input.id).await? {
            return Err(EngineError::StaleToolSource);
        }
        let record = self.record(conn).await?;
        if let Some(source) = record.origin.summary() {
            source.mark_model_admitted(conn, at).await?;
        }
        Ok(())
    }

    /// The owned initial input asks for the program's declared native mode without changing inherited settings.
    pub(crate) async fn native_settings(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
        thread: Id,
        descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
        declarations: &orch_provider_contract::capabilities::Declarations,
    ) -> Result<BTreeMap<String, Json>, EngineError> {
        if thread != self.helper {
            return Ok(BTreeMap::new());
        }
        if input.thread_id != self.helper || !self.owns_input(conn, input.id).await? {
            return Err(EngineError::StaleToolSource);
        }
        let record = self.record(conn).await?;
        if input.author_id != Some(record.actor)
            || input.content != serde_json::to_value(&record.content)?
        {
            return Err(EngineError::StaleToolSource);
        }
        let program = descriptor.capability_program();
        let mode = program
            .access
            .get("native")
            .and_then(|access| access.native.get(&record.capability))
            .and_then(|support| support.delegated_mode.as_ref())
            .ok_or(EngineError::StaleToolSource)?;
        let definition = declarations
            .capabilities
            .get(&record.capability)
            .ok_or(EngineError::StaleToolSource)?;
        if !definition.modes.contains_key(mode) {
            return Err(EngineError::StaleToolSource);
        }
        Ok(definition
            .mode_setting
            .as_ref()
            .map(|key| BTreeMap::from([(key.clone(), json!(mode))]))
            .unwrap_or_default())
    }

    /// Confinement and output facts belong only to the actual owned initial model turn.
    pub(crate) async fn program_output(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
        turn: &threads::TurnRow,
        descriptor: &orch_agent_contract::descriptor::AgentDescriptor,
    ) -> Result<Option<orch_agent_contract::session::ProgramOutput>, EngineError> {
        if turn.thread_id != self.helper {
            return Ok(None);
        }
        let record = self.record(conn).await?;
        if !record.execution.confined_tools {
            return Ok(None);
        }
        if input.thread_id != self.helper
            || turn.input_id != input.id
            || !self.owns_input(conn, input.id).await?
        {
            return Err(EngineError::StaleToolSource);
        }
        if turn.model != record.model
            || turn.effort != record.effort
            || input.author_id != Some(record.actor)
            || input.content != serde_json::to_value(&record.content)?
            || !descriptor
                .side_thread_modes
                .contains(&orch_agent_contract::descriptor::SideReading::NoTools)
        {
            return Err(EngineError::StaleToolSource);
        }
        let output = orch_agent_contract::session::ProgramOutput {
            max_output_tokens: record
                .input
                .get("max_output_tokens")
                .and_then(Json::as_i64)
                .ok_or(EngineError::StaleToolSource)?,
            schema: None,
        };
        output.validate().map_err(EngineError::unreadable)?;
        Ok(Some(output))
    }

    /// The actual admitted producer creates only its fixed private execution artifact.
    pub(crate) async fn birth(
        &self,
        conn: &mut Conn,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<(Principal, crate::binding::NewThread, crate::binding::Birth), EngineError> {
        use crate::binding::{Birth, BirthAgentAccess, NewThread};
        use orch_access_contract::pages::{PageChange, Whole};
        if !self.current_use(conn, clock, access).await? {
            return Err(EngineError::StaleToolSource);
        }
        let record = self.record(conn).await?;
        if self.helper != record.helper
            || threads::thread(conn, self.helper).await?.is_some()
            || capability_work::program(conn, self.work)
                .await?
                .is_none_or(|work| work.input.is_some())
            || !record.origin.current_act(conn, clock).await?
        {
            return Err(EngineError::StaleToolSource);
        }
        let origins = record
            .origin
            .callers(conn, clock, access)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let actor = origins
            .callers
            .into_iter()
            .find(|caller| caller.user() == record.actor)
            .ok_or(EngineError::StaleToolSource)?;
        let principal = Principal::load(conn, actor, clock.time()).await?;
        let new = NewThread {
            topic: record.placement.topic,
            name: "Capability helper".into(),
            agent: record.agent,
            provider: record.provider,
            environment: record.placement.environment,
            directory: record.placement.directory,
            arrangement: None,
            model: Some(record.model),
            effort: record.effort,
        };
        let birth = Birth {
            agent_access: BirthAgentAccess::nothing(),
            prepare_source: Some(false),
            page: Some(PageChange {
                whole: Some(Whole::Private),
                ..PageChange::default()
            }),
            ..Birth::default()
        };
        Ok((principal, new, birth))
    }

    pub(crate) fn helper(&self) -> Id {
        self.helper
    }

    /// Re-enter the accepted producer, retaining its real operations for this exact helper input.
    pub(crate) fn input_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        thread: &'a threads::ThreadRow,
        content: &'a Json,
        actor: Id,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<Option<Vec<Caller>>, EngineError>> {
        Box::pin(async move {
            if !self.current(conn, thread, content, actor, clock).await? {
                return Ok(None);
            }
            let record = self.record(conn).await?;
            let origins = record.origin.input_callers(conn, clock).await?;
            if origins.actor != record.actor || origins.person != record.person {
                return Ok(None);
            }
            let callers: Vec<_> = origins
                .callers
                .into_iter()
                .filter(|caller| caller.user() == actor)
                .collect();
            Ok((!callers.is_empty()).then_some(callers))
        })
    }

    /// The shared input writer records ownership before its ordinary preflight may start work.
    pub(crate) async fn bind_input(
        &self,
        conn: &mut Conn,
        input: &threads::InputRow,
    ) -> Result<(), EngineError> {
        let record = self.record(conn).await?;
        if input.thread_id != self.helper
            || input.author_id != Some(record.actor)
            || input.content != serde_json::to_value(&record.content)?
            || !capability_work::bind_program_input(conn, self.work, input.id).await?
        {
            return Err(EngineError::StaleToolSource);
        }
        Ok(())
    }

    /// Reading an already paid result checks its source binding, without reserving another allowance.
    pub(crate) async fn current_placement(&self, conn: &mut Conn) -> Result<bool, EngineError> {
        self.record(conn).await?.placement.current(conn).await
    }

    async fn record(&self, conn: &mut Conn) -> Result<Record, EngineError> {
        let work = capability_work::program(conn, self.work)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
        let slot = format!("{SLOT}:{}", work.attempt);
        let stored: Option<Json> = match (work.source_answer, work.source_summary_call) {
            (Some(answer), None) => {
                sqlx::query_scalar("SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1")
                    .bind(answer)
                    .bind(&slot)
                    .fetch_optional(&mut *conn)
                    .await
                    .map_err(orch_db::DbError::from)?
                    .flatten()
            }
            (None, Some(call)) => sqlx::query_scalar(
                "SELECT delegated_program FROM summary_calls WHERE id=$1 AND purpose='summary'",
            )
            .bind(call)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
            .flatten(),
            _ => return Err(EngineError::StaleToolSource),
        };
        let record: Record = serde_json::from_value(stored.ok_or(EngineError::StaleToolSource)?)?;
        if record.helper != self.helper
            || !record.origin.matches_journal(&work)
            || record.origin.request() != work.request
            || work.id != crate::ids::derived("capability.work", &[record.attempt.as_bytes()])
            || work.helper != self.helper
            || work.actor != record.actor
            || work.person != record.person
            || work.node != record.origin.node()
            || work.purpose != record.capability
            || work.attempt != record.attempt
            || work.backend != serde_json::to_value(record.origin.backend())?
            || work.record != record.origin.locator(&slot)
            || work.deadline != record.deadline.get()
        {
            return Err(EngineError::StaleToolSource);
        }
        Ok(record)
    }

    /// Check only the retained source, original credential ceilings and exact input/binding.
    /// Ordinary helper admission and current parent resource Use are checked separately before IO.
    pub(crate) fn current<'a>(
        &'a self,
        conn: &'a mut Conn,
        thread: &'a threads::ThreadRow,
        content: &'a Json,
        actor: Id,
        clock: &'a dyn Clock,
    ) -> BoxFuture<'a, Result<bool, EngineError>> {
        Box::pin(async move {
            let record = match self.record(conn).await {
                Ok(record) => record,
                Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => {
                    return Ok(false);
                }
                Err(error) => return Err(error),
            };
            if thread.node_id != self.helper
                || thread.agent_id != Some(record.agent)
                || thread.provider_id != Some(record.provider)
                || thread.environment_id != Some(record.placement.environment)
                || thread.topic_id != record.placement.topic
                || thread.workspace_dir != record.placement.directory
                || !record.placement.current(conn).await?
                || thread.deleted_at.is_some()
                || thread.archived_at.is_some()
                || actor != record.actor
                || serde_json::to_value(&record.content)? != *content
                || clock.time() >= record.deadline
            {
                return Ok(false);
            }
            Ok(record.origin.current_act(conn, clock).await? && clock.time() < record.deadline)
        })
    }

    /// The paid boundary restores every actual original author/actor and the existing resource checker.
    pub(crate) fn current_use<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<bool, EngineError>> {
        Box::pin(async move { Ok(self.current_callers(conn, clock, access).await?.is_some()) })
    }
    pub(crate) fn current_callers<'a>(
        &'a self,
        conn: &'a mut Conn,
        clock: &'a dyn Clock,
        access: &'a dyn crate::access::ResourceAccess,
    ) -> BoxFuture<'a, Result<Option<CurrentCallers>, EngineError>> {
        Box::pin(async move {
            let record = self.record(conn).await?;
            if clock.time() >= record.deadline
                || !record.placement.current(conn).await?
                || !record
                    .origin
                    .selection_current(conn, &record.capability, record.attempt)
                    .await?
            {
                return Ok(None);
            }
            let Some(account) = resources::thread_engine::provider(conn, record.provider).await?
            else {
                return Ok(None);
            };
            let Some(agent) = resources::thread_engine::agent(conn, record.agent).await? else {
                return Ok(None);
            };
            if account.agent_id != Some(record.agent)
                || agent.disabled
                || agent.deleted_at.is_some()
                || source::hash_json(agent.profile)? != record.profile
                || !backend_state(
                    conn,
                    &BuiltinSources::default(),
                    &record.capability,
                    record.origin.backend(),
                )
                .await?
                .is_ok()
            {
                return Ok(None);
            }
            let Some(origins) = record.origin.callers(conn, clock, access).await? else {
                return Ok(None);
            };
            if origins.actor != record.actor || origins.person != record.person {
                return Ok(None);
            }
            if crate::provider_policy::forbidden(conn, record.origin.node(), record.provider)
                .await?
                .is_some()
            {
                return Ok(None);
            }
            let Some(model) = resources::thread_engine::model(conn, record.provider, &record.model)
                .await?
                .filter(|model| model.enabled)
            else {
                return Ok(None);
            };
            let parameters = crate::binding::model_parameters(&model)?;
            if parameters.effort_is_gone(record.effort.as_deref()) {
                return Ok(None);
            }
            let work = capability_work::program(conn, self.work)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let turn = if let Some(input) = work.input {
                let input = threads::input(conn, input)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                if input.thread_id != self.helper {
                    return Ok(None);
                }
                if let Some(turn) = input.turn_id {
                    let turn = threads::turn(conn, turn)
                        .await?
                        .ok_or(EngineError::StaleToolSource)?;
                    if turn.thread_id != self.helper
                        || turn.input_id != input.id
                        || turn.model != record.model
                        || turn.effort != record.effort
                    {
                        return Ok(None);
                    }
                    Some(turn)
                } else {
                    None
                }
            } else {
                None
            };
            let descriptor =
                crate::harnesses::descriptor_of(&agent)?.ok_or(EngineError::StaleToolSource)?;
            let tier = turn.as_ref().and_then(|turn| {
                crate::providers::service_tier(&descriptor.parameters, &turn.params)
            });
            let use_request = model::ModelUse {
                place: record.origin.node(),
                provider: record.provider,
                offering: &record.model,
                groups: &model.groups,
                effort: record.effort.as_deref(),
            };
            for caller in &origins.callers {
                if model::offering_use(conn, caller, &use_request, clock)
                    .await?
                    .is_none()
                {
                    return Ok(None);
                }
            }
            if !crate::caps::own_room(
                conn,
                &crate::caps::OwnCapAsk {
                    place: record.origin.node(),
                    provider: record.provider,
                    model: &record.model,
                    tier: tier.as_deref(),
                    purpose: &record.capability,
                    person: origins.person,
                },
                clock.now(),
            )
            .await?
            {
                return Ok(None);
            }
            Ok((clock.time() < record.deadline).then(|| {
                origins
                    .callers
                    .iter()
                    .cloned()
                    .map(|caller| (Some(caller), Some(record.deadline)))
                    .collect()
            }))
        })
    }
}

/// The actual sign-in profile and its same-resolver native capability support.
/// This is metadata for a prospective helper, not current Use or an installed paid executor.
pub(crate) async fn backend_state(
    conn: &mut Conn,
    sources: &BuiltinSources,
    capability: &str,
    backend: &orch_provider_contract::capabilities::BackendRef,
) -> Result<Result<(), CapabilityFailure>, EngineError> {
    if let Err(message) =
        orch_provider_contract::capabilities::ProgramPlacement::read(&backend.config)
    {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_placement_invalid",
            message,
        )));
    }
    let Some(reference) = backend.offering.as_ref() else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "offering_not_selected",
            "Choose a model for this delegated program.",
        )));
    };
    let Some(required) = sources
        .capabilities()
        .declarations()
        .routes
        .get(&backend.route)
        .filter(|route| {
            route.executor.as_deref() == Some("program-session")
                && route.capabilities.iter().any(|id| id == capability)
        })
        .and_then(|route| route.execution.as_ref())
    else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "execution_codec_unavailable",
            "The selected route has no implemented consumer codec.",
        )));
    };
    native_state_for(
        conn,
        sources,
        reference.provider,
        &reference.offering,
        capability,
        required,
    )
    .await
}

#[cfg(test)]
pub(crate) async fn native_state(
    conn: &mut Conn,
    sources: &BuiltinSources,
    provider: Id,
    model: &str,
    capability: &str,
) -> Result<Result<(), CapabilityFailure>, EngineError> {
    let route = sources
        .capabilities()
        .declarations()
        .routes
        .iter()
        .find(|(_, route)| {
            route.executor.as_deref() == Some("program-session")
                && route.capabilities.iter().any(|id| id == capability)
        })
        .map(|(id, _)| id.clone())
        .ok_or(EngineError::StaleToolSource)?;
    backend_state(
        conn,
        sources,
        capability,
        &orch_provider_contract::capabilities::BackendRef {
            route,
            offering: Some(orch_provider_contract::capabilities::OfferingRef {
                provider,
                offering: model.into(),
            }),
            ..Default::default()
        },
    )
    .await
}

async fn native_state_for(
    conn: &mut Conn,
    sources: &BuiltinSources,
    provider: Id,
    model: &str,
    capability: &str,
    required: &ExecutionSupport,
) -> Result<Result<(), CapabilityFailure>, EngineError> {
    use orch_provider_contract::capabilities as cap;
    let Some(account) = resources::thread_engine::provider(conn, provider).await? else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "provider_missing",
            "The selected native provider is no longer available.",
        )));
    };
    if account.kind != "subscription"
        || account.deleted_at.is_some()
        || !(crate::providers::request_status_available(&account.status)
            || account.status == "offline"
                && crate::provider_runtime::waits_for_runtime(conn, &account).await?)
    {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "sign_in_unavailable",
            "The selected provider is not ready to use its sign-in. Sign in on its provider page.",
        )));
    }
    let Some(agent) = account.agent_id else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_missing",
            "The selected native provider has no assigned agent program.",
        )));
    };
    let Some(agent) = resources::thread_engine::agent(conn, agent)
        .await?
        .filter(|agent| !agent.disabled && agent.deleted_at.is_none())
    else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_unavailable",
            "The selected native program is disabled or no longer available.",
        )));
    };
    let Some(descriptor) = crate::harnesses::descriptor_of(&agent)? else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_unreported",
            "The selected native program has not reported its supported capabilities.",
        )));
    };
    if !orch_agent_contract::arrangement::native_matches(
        &descriptor,
        &crate::binding::serves(&account),
    ) {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "native_program_mismatch",
            "The selected sign-in does not belong to this agent program.",
        )));
    }
    let Some(model_row) = resources::thread_engine::model(conn, provider, model)
        .await?
        .filter(|row| row.enabled)
    else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "model_unavailable",
            "The selected native model is disabled or no longer offered.",
        )));
    };
    let mut registry = cap::Registry::default();
    registry
        .register("common", cap::catalogue().map_err(EngineError::unreadable)?)
        .map_err(EngineError::unreadable)?;
    registry
        .register(
            "installed-providers",
            sources.capabilities().declarations().clone(),
        )
        .map_err(EngineError::unreadable)?;
    let mut declarations = cap::Declarations::default();
    declarations
        .programs
        .insert(descriptor.kind.clone(), descriptor.capability_program());
    declarations.providers.insert(
        provider,
        cap::Provider {
            offerings: BTreeMap::from([(
                model.into(),
                cap::Offering {
                    enabled: true,
                    kind: "model".into(),
                    parameters: model_row.parameters,
                    ..cap::Offering::default()
                },
            )]),
            ..cap::Provider::default()
        },
    );
    registry
        .register("native-helper", declarations)
        .map_err(EngineError::unreadable)?;
    let Some(mode) = registry
        .declarations()
        .programs
        .get(&descriptor.kind)
        .and_then(|program| program.access.get("native"))
        .and_then(|access| access.native.get(capability))
        .and_then(|support| support.delegated_mode.clone())
    else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "capability_not_offered",
            "The selected program has not declared a delegated mode for this capability.",
        )));
    };
    let failure_facts: Vec<String> = registry
        .declarations()
        .programs
        .get(&descriptor.kind)
        .and_then(|program| program.access.get("native"))
        .and_then(|access| access.native.get(capability))
        .and_then(|support| support.extensions.get("delegation_failure_facts"))
        .and_then(Json::as_array)
        .into_iter()
        .flatten()
        .filter_map(Json::as_str)
        .map(str::to_owned)
        .collect();
    let program_support = registry
        .declarations()
        .programs
        .get(&descriptor.kind)
        .and_then(|program| program.access.get("native"))
        .and_then(|access| access.native.get(capability));
    let installed = execution_support();
    let implemented = {
        installed.iter().any(|support| support.satisfies(required))
            && program_support.is_some_and(|program| {
                program
                    .execution
                    .iter()
                    .any(|support| support.satisfies(required))
            })
    };
    if required.confined_tools
        && !descriptor
            .side_thread_modes
            .contains(&orch_agent_contract::descriptor::SideReading::NoTools)
    {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_execution_unavailable",
            "The selected program cannot provide this model turn's tool-free execution context.",
        )));
    }
    if !implemented {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "program_execution_unavailable",
            "The selected program has no verified input and result codec for this capability.",
        )));
    }
    let context = cap::ThreadContext {
        choices: BTreeMap::from([(capability.into(), Some(mode))]),
        program: descriptor.kind,
        access: "native".into(),
        current: Some(cap::OfferingRef {
            provider,
            offering: model.into(),
        }),
        availability: BTreeMap::from([(
            provider,
            BTreeMap::from([(
                model.into(),
                cap::Availability {
                    allowed: true,
                    ..cap::Availability::default()
                },
            )]),
        )]),
        facts: json!({"plan":account.facts.get("plan"),"credential":account.facts.get("credential"),"credential_verified":account.facts.get("credential_verified")}),
        ..cap::ThreadContext::default()
    };
    let Some(row) = registry
        .resolve(&context)
        .into_iter()
        .find(|row| row.capability == capability)
    else {
        return Ok(Err(CapabilityFailure::unavailable(
            capability,
            "capability_not_declared",
            "The selected capability is not installed.",
        )));
    };
    if row
        .route
        .as_ref()
        .is_some_and(|route| route.kind == "native-program")
    {
        return Ok(Ok(()));
    }
    let fact = row
        .facts
        .iter()
        .find(|fact| failure_facts.contains(&fact.id));
    let reason = fact
        .map(|fact| fact.id.as_str())
        .or(row.reason.as_deref())
        .unwrap_or("native_capability_unavailable");
    let message = fact.and_then(|fact| fact.text.get("en")).map_or(
        "The selected program sign-in does not offer this capability on its reported plan.",
        String::as_str,
    );
    let mut failure = CapabilityFailure::unavailable(capability, reason, message);
    if let Some(fact) = fact {
        failure.text.clone_from(&fact.text);
    }
    Ok(Err(failure))
}

impl Engine {
    /// Reconcile already-generated native evidence before asking for another paid effect.
    pub(crate) async fn recover_program_capability_result(
        &self,
        call: &CallContext,
        capability: &str,
    ) -> Result<Option<accepted::AcceptedCapabilityResult>, EngineError> {
        run::reconcile(self, call, capability).await
    }

    /// Prepare the exact ordinary private helper under its actual producer's admission.
    /// Its journal and input retain every original tool credential throughout.
    pub(super) async fn start_delegated_capability_helper(
        &self,
        call: &CallContext,
        capability: &str,
        input: Json,
        selected: selection::Selection,
        required: &ExecutionSupport,
    ) -> Result<Id, EngineError> {
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(EngineError::StaleToolSource)?;
        if selected.executor != "program-session" {
            return Err(EngineError::StaleToolSource);
        }
        if sources
            .capabilities()
            .declarations()
            .routes
            .get(&selected.reference.route)
            .and_then(|route| route.execution.as_ref())
            != Some(required)
        {
            return Err(EngineError::StaleToolSource);
        }
        let codec = codecs::installed()
            .map_err(EngineError::unreadable)?
            .get(required)?;
        if sources
            .capabilities()
            .declarations()
            .routes
            .get(&selected.reference.route)
            .and_then(|route| route.orch_tool.as_deref())
            != Some(call.tool.name())
        {
            return Err(EngineError::StaleToolSource);
        }
        let reference = selected.offering().ok_or(EngineError::StaleToolSource)?;
        let root = source::resource_id(
            "provider-service",
            crate::ids::tool_result(call.thread, &call.call_ref),
        );
        let attempt = recovery_binding::command(root)?;
        let slot = format!("{SLOT}:{attempt}");
        let helper = crate::ids::derived("capability.helper", &[attempt.as_bytes()]);
        let record = {
            let mut conn = self.db().acquire().await?;
            let origins = source::original_callers(
                &mut conn,
                call,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
            )
            .await?;
            backend_state(&mut conn, &sources, capability, &selected.reference)
                .await?
                .map_err(EngineError::from)?;
            let parent = threads::thread(&mut conn, call.thread)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let provider = resources::thread_engine::provider(&mut conn, reference.provider)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let agent = provider.agent_id.ok_or(EngineError::StaleToolSource)?;
            let profile = resources::thread_engine::agent(&mut conn, agent)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let descriptor =
                crate::harnesses::descriptor_of(&profile)?.ok_or(EngineError::StaleToolSource)?;
            let program = descriptor.capability_program();
            let program_label = program
                .access
                .get("native")
                .and_then(|access| access.native.get(capability))
                .and_then(|support| support.extensions.get("delegation_label"))
                .cloned()
                .unwrap_or_else(
                    || json!({"en":descriptor.harness.name,"zh-CN":descriptor.harness.name}),
                );
            let person = crate::views::user_ref_json(&mut conn, origins.actor()).await?;
            let actor_name = person
                .get("display_name")
                .and_then(Json::as_str)
                .filter(|name| !name.is_empty())
                .or_else(|| person.get("username").and_then(Json::as_str))
                .unwrap_or("")
                .to_owned();
            let material = accepted::images();
            let mut content = codec.prepare(capability, &input, &material)?;
            if let Some(ContentPart::Text { text }) = content.first_mut() {
                text.push_str(&format!("\n\n[Requesting thread](/thread/{})", call.thread));
            }
            let placement = placement::Placement::for_thread(&mut conn, &parent).await?;
            let effort = helper_effort(
                &mut conn,
                provider.id,
                &reference.offering,
                placement.topic,
                None,
            )
            .await?;
            Record {
                origin: origin::Producer::capture_tool(
                    call,
                    &selected,
                    origins
                        .callers()
                        .iter()
                        .map(CapturedCaller::capture)
                        .collect(),
                )?,
                attempt,
                helper,
                actor: origins.actor(),
                person: origins.attribution().user_id,
                capability: capability.into(),
                execution: required.clone(),
                agent,
                provider: provider.id,
                placement,
                model: reference.offering.clone(),
                effort,
                content,
                input: input.clone(),
                material,
                deadline: source::capability_deadline(&mut conn, call).await?,
                profile: source::hash_json(profile.profile)?,
                actor_name,
                actor_ref: person,
                program_label,
            }
        };
        let saved = source::value(self, &slot, serde_json::to_value(&record)?).await?;
        let record: Record = serde_json::from_value(saved)?;
        if !record.origin.matches_tool(&selected)?
            || record.helper != helper
            || record.capability != capability
            || record.attempt != attempt
            || record.execution != *required
        {
            return Err(EngineError::StaleToolSource);
        }
        #[cfg(test)]
        source::tests::pause(call, source::tests::Point::ProgramCaptured).await;
        let link = self.start_program_record(&record).await?;
        self.publish_delegated_helper(call, &record).await?;
        Ok(link.helper)
    }

    async fn start_program_record(&self, record: &Record) -> Result<ParentLink, EngineError> {
        let slot = format!("{SLOT}:{}", record.attempt);
        let link = ParentLink {
            work: crate::ids::derived("capability.work", &[record.attempt.as_bytes()]),
            helper: record.helper,
        };
        let mut tx = self.db().begin().await?;
        record
            .origin
            .lock_source(&mut tx, self.inner.clock.as_ref())
            .await?;
        if !record
            .origin
            .current_act(&mut tx, self.inner.clock.as_ref())
            .await?
        {
            return Err(EngineError::StaleToolSource);
        }
        let backend = serde_json::to_value(record.origin.backend())?;
        let locator = record.origin.locator(&slot);
        let journal = capability_work::retain_program(
            &mut tx,
            &capability_work::NewProgramWork {
                id: link.work,
                request: record.origin.request(),
                source: record.origin.program_origin(),
                attempt: record.attempt,
                helper: record.helper,
                actor: record.actor,
                person: record.person,
                node: record.origin.node(),
                purpose: &record.capability,
                backend: &backend,
                record: &locator,
                at: self.inner.clock.now(),
                deadline: record.deadline.get(),
            },
        )
        .await?
        .ok_or(EngineError::StaleToolSource)?;
        // Birth and the later native command share the same original callers and current Use.
        // A metadata-eligible sign-in is not permission to create work on that provider.
        if !link
            .current_use(
                &mut tx,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
            )
            .await?
        {
            return Err(EngineError::StaleToolSource);
        }
        if threads::thread(&mut tx, record.helper).await?.is_none() {
            if journal.input.is_some() {
                return Err(EngineError::StaleToolSource);
            }
            let origins = record
                .origin
                .callers(
                    &mut tx,
                    self.inner.clock.as_ref(),
                    self.inner.access.as_ref(),
                )
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            Box::pin(self.create_program_thread_in_transaction(&mut tx, &link))
                .await
                .map_err(EngineError::unreadable)?;
            let lane = threads::thread(&mut tx, record.helper)
                .await?
                .and_then(|row| row.current_segment_id)
                .ok_or(EngineError::StaleToolSource)?;
            if threads::segment(&mut tx, lane)
                .await?
                .is_none_or(|lane| lane.credential_path != "native")
            {
                return Err(EngineError::unreadable(
                    "The selected sign-in has no native program path in this environment.",
                ));
            }
            let authority = crate::inputs::InputAuthority::for_program(
                link.clone(),
                record.actor,
                &origins.callers,
            )?;
            let thread = threads::thread(&mut tx, record.helper)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let request = crate::inputs::InputRequest {
                plan: false,
                content: record.content.clone(),
                delivery: orch_api_contract::threads::Delivery::Queue,
                client_id: format!("capability-helper:{}", record.attempt),
                bypass: None,
            };
            let mut unit = crate::unit::Unit::new(&mut tx, self.inner.clock.now());
            let callers = crate::inputs::input_authority_for_request(
                &mut unit,
                &thread,
                &request,
                None,
                record.actor,
                &authority,
                self.inner.clock.as_ref(),
            )
            .await?
            .ok_or(EngineError::StaleToolSource)?;
            Box::pin(self.submit_with_authority_in(
                &mut unit,
                &callers,
                record.helper,
                &request,
                None,
                crate::inputs::Made::PROGRAM,
            ))
            .await
            .map_err(EngineError::unreadable)?;
            unit.finish().await?;
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.wake(record.helper);
        Ok(link)
    }
    async fn publish_delegated_helper(
        &self,
        call: &CallContext,
        record: &Record,
    ) -> Result<(), EngineError> {
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        let item:Option<(Id,Json)>=sqlx::query_as("SELECT id,payload FROM items WHERE thread_id=$1 AND turn_id=$2 AND native_ref=$3 AND kind='tool_call' AND status='in_progress' AND payload#>>'{tool,kind}'='orch' AND payload#>>'{tool,tool}'=$4")
            .bind(call.thread).bind(call.turn).bind(&call.call_ref).bind(call.tool.name()).fetch_optional(&mut *tx).await.map_err(orch_db::DbError::from)?;
        if let Some((id, mut payload)) = item {
            payload["tool"]["result"] = json!({"state":"running","helper":record.helper_info()});
            let now = self.inner.clock.now();
            let mut unit = crate::unit::Unit::new(&mut tx, now);
            let seq = unit.stamp(call.thread).await?;
            threads::change_item(
                unit.conn,
                id,
                &threads::ItemChange {
                    status: None,
                    payload: &payload,
                    search_text: None,
                    at: now,
                },
                seq,
            )
            .await?;
            let finished = unit.finish().await?;
            self.commit(tx, finished).await?;
        } else {
            tx.commit().await.map_err(orch_db::DbError::from)?;
        }
        Ok(())
    }
}

/// One exact helper session proposal, conjunctive with its ordinary input and original requesting tool.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionAuthority {
    parent: ParentLink,
    input: Id,
    turn: Id,
    segment: Id,
    command: Id,
    machine: Id,
    peer: Id,
    unit: Option<unit::Plan>,
    body: Hash,
    kind: String,
    deadline: Time,
}
impl std::fmt::Debug for SessionAuthority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CapabilitySession([private])")
    }
}
impl SessionAuthority {
    pub(crate) fn command_kind(&self) -> &'static str {
        match self.kind.as_str() {
            "session.open" => "session.open",
            "session.open.fenced" => "session.open.fenced",
            "node.program.admit" => "node.program.admit",
            _ => "",
        }
    }
    pub(crate) async fn lock_source(&self, conn: &mut Conn) -> Result<(), EngineError> {
        threads::lock(conn, self.parent.helper).await?;
        Ok(())
    }
    pub(crate) async fn current_issue_callers(
        &self,
        conn: &mut Conn,
        command: &link::NewCommand<'_>,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        if command.id != self.command
            || command.machine_id != self.machine
            || command.kind != self.kind
            || command.serial_key
                != if self.kind == "node.program.admit" {
                    SerialKey::Runtime(self.parent.work).wire()
                } else {
                    SerialKey::Thread(self.parent.helper).wire()
                }
                .as_deref()
            || source::hash_json(command.body.clone())? != self.body
            || Time::new(command.deadline_at) != self.deadline
            || clock.time() >= self.deadline
        {
            return Ok(None);
        }
        let Some(thread) = threads::lock(conn, self.parent.helper).await? else {
            return Ok(None);
        };
        let Some(turn) = threads::turn(conn, self.turn).await? else {
            return Ok(None);
        };
        let Some(input) = threads::input(conn, self.input).await? else {
            return Ok(None);
        };
        let Some(segment) = threads::segment(conn, self.segment).await? else {
            return Ok(None);
        };
        if thread.current_turn_id != Some(turn.id)
            || thread.current_segment_id != Some(segment.id)
            || turn.input_id != input.id
            || turn.thread_id != thread.node_id
            || turn.segment_id != segment.id
            || segment.thread_id != thread.node_id
            || segment.credential_path != "native"
            || !self
                .parent
                .current(
                    conn,
                    &thread,
                    &input.content,
                    input.answers().unwrap_or_else(Id::nil),
                    clock,
                )
                .await?
            || !self.parent.current_use(conn, clock, access).await?
        {
            return Ok(None);
        }
        let Some(provider) = resources::thread_engine::provider(conn, segment.provider_id).await?
        else {
            return Ok(None);
        };
        let Some(environment) = orch_db::queries::environments::thread_engine::environment(
            conn,
            segment.environment_id,
        )
        .await?
        else {
            return Ok(None);
        };
        if environment.generation != segment.env_generation
            || crate::harnesses::machine_of(
                Some(&segment.arrangement),
                &provider,
                environment.machine_id,
            ) != Some(self.peer)
        {
            return Ok(None);
        }
        if self.kind == "node.program.admit" {
            let Some(plan) = &self.unit else {
                return Ok(None);
            };
            let body: orch_link_contract::commands::NodeProgramAdmit =
                serde_json::from_value(command.body.clone())?;
            if plan.node != self.machine
                || plan.admission.peer != self.peer
                || body.runtime_id != self.parent.work
                || body.work != plan.admission
                || !plan.current(conn, &self.parent, clock).await?
            {
                return Ok(None);
            }
        } else if self.unit.is_some() || self.machine != self.peer {
            return Ok(None);
        }
        let mut unit = crate::unit::Unit::new(conn, clock.now());
        let Some(actor) = crate::inputs::input_caller(&mut unit, &thread, &input, clock).await?
        else {
            return Ok(None);
        };
        for caller in actor.callers() {
            if let Err(error) = crate::inputs::check_sender_for_turn(
                &mut unit,
                caller,
                &thread,
                true,
                Some(&turn),
                access,
            )
            .await
            {
                return match error {
                    crate::inputs::InputError::Internal(error) => Err(error),
                    _ => Ok(None),
                };
            }
        }
        if clock.time() >= self.deadline {
            return Ok(None);
        }
        let mut callers: CurrentCallers = actor
            .callers()
            .iter()
            .cloned()
            .map(|caller| (Some(caller), Some(self.deadline)))
            .collect();
        let Some(parent) = self
            .parent
            .current_callers(unit.conn, clock, access)
            .await?
        else {
            return Ok(None);
        };
        callers.extend(parent);
        if callers.iter().any(|(caller, deadline)| {
            caller
                .as_ref()
                .is_some_and(|caller| !caller.key_deadline_allows(clock))
                || deadline.is_some_and(|deadline| clock.time() >= deadline)
        }) {
            return Ok(None);
        }
        Ok(Some(callers))
    }
    pub(crate) async fn current_callers(
        &self,
        conn: &mut Conn,
        command: &link::CommandRow,
        clock: &dyn Clock,
        access: &dyn crate::access::ResourceAccess,
    ) -> Result<Option<CurrentCallers>, EngineError> {
        self.current_issue_callers(
            conn,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine_id,
                kind: &command.kind,
                body: &command.body,
                serial_key: command.serial_key(),
                deadline_at: command.deadline_at,
            },
            clock,
            access,
        )
        .await
    }
}

pub(crate) async fn seal_session(
    engine: &Engine,
    snapshot: &crate::sessions::StartSnapshot,
    mut command: Outgoing,
) -> Result<Outgoing, EngineError> {
    let Some(parent) =
        crate::inputs::saved_authority(snapshot.input.submission_authority.as_ref())?
            .and_then(|authority| authority.capability_parent().cloned())
    else {
        return Ok(command);
    };
    if command.authority.is_some()
        || !matches!(
            command.kind,
            orch_link_contract::commands::CommandKind::SessionOpen
                | orch_link_contract::commands::CommandKind::SessionOpenFenced
        )
    {
        return Err(EngineError::ToolCallIdentity);
    }
    if snapshot
        .descriptor
        .extensions
        .get("x_program_work")
        .is_some()
    {
        engine
            .prepare_program_unit(snapshot)
            .await?
            .ok_or(EngineError::StaleToolSource)?;
    }
    let mut conn = engine.db().acquire().await?;
    let record = parent.record(&mut conn).await?;
    let deadline = command
        .deadline
        .unwrap_or(record.deadline)
        .min(record.deadline);
    command.deadline = Some(deadline);
    let authority = SessionAuthority {
        parent,
        input: snapshot.input.id,
        turn: snapshot.turn.id,
        segment: snapshot.segment.id,
        command: command.id,
        machine: command.machine,
        peer: snapshot.machine.id,
        unit: None,
        body: source::hash_json(command.body.clone())?,
        kind: command.kind.as_str().into(),
        deadline,
    };
    authority
        .current_issue_callers(
            &mut conn,
            &link::NewCommand {
                id: command.id,
                machine_id: command.machine,
                kind: command.kind.as_str(),
                body: &command.body,
                serial_key: command.key.wire().as_deref(),
                deadline_at: deadline.get(),
            },
            engine.inner.clock.as_ref(),
            engine.inner.access.as_ref(),
        )
        .await?
        .ok_or(EngineError::StaleToolSource)?;
    command.authority = Some(CommandAuthority::CapabilitySession(Box::new(authority)));
    Ok(command)
}
