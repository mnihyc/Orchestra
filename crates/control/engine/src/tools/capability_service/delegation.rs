//! A native program helper is ordinary actor-owned work with an additional original tool condition.
use super::*;
use futures::future::BoxFuture;
use orch_api_contract::threads::ContentPart;
use orch_authz::CapturedCaller;
use serde_json::json;

const SLOT: &str = "delegated_capability";
mod run;
pub(super) use run::{execute, failure};

/// A conjunctive condition on the helper's real creator/input authority, never an acting credential.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParentLink {
    answer: Id,
    helper: Id,
    slot: String,
}

#[derive(Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Record {
    source: source::Source,
    call: CallContext,
    selected: selection::Selection,
    recovery: Option<recovery_binding::Binding>,
    attempt: Id,
    helper: Id,
    actor: Id,
    capability: String,
    agent: Id,
    provider: Id,
    environment: Id,
    topic: Id,
    directory: String,
    model: String,
    content: Vec<ContentPart>,
    input: Json,
    limits: Vec<CapturedCaller>,
    deadline: Time,
    profile: Hash,
    actor_name: String,
    actor_ref: Json,
    program_label: Json,
}

impl Record {
    fn helper_info(&self) -> Json {
        json!({"thread_id":self.helper,"actor_id":self.actor,"actor":self.actor_ref,"actor_name":self.actor_name,
            "program_label":self.program_label,"requester_thread_id":self.call.thread})
    }
}

impl ParentLink {
    async fn record(&self, conn: &mut Conn) -> Result<Record, EngineError> {
        let stored: Option<Json> =
            sqlx::query_scalar("SELECT plan->$2 FROM thread_tool_calls WHERE answer_id=$1")
                .bind(self.answer)
                .bind(&self.slot)
                .fetch_optional(conn)
                .await
                .map_err(orch_db::DbError::from)?
                .flatten();
        let record: Record = serde_json::from_value(stored.ok_or(EngineError::StaleToolSource)?)?;
        if record.helper != self.helper
            || record.source.answer() != self.answer
            || self.slot != format!("{SLOT}:{}", record.attempt)
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
                || thread.environment_id != Some(record.environment)
                || thread.topic_id != record.topic
                || thread.workspace_dir != record.directory
                || thread.deleted_at.is_some()
                || thread.archived_at.is_some()
                || actor != record.actor
                || serde_json::to_value(&record.content)? != *content
                || clock.time() >= record.deadline
            {
                return Ok(false);
            }
            let unfinished: bool = sqlx::query_scalar(
                "SELECT NOT(completed OR retired) FROM thread_tool_calls WHERE answer_id=$1",
            )
            .bind(self.answer)
            .fetch_optional(&mut *conn)
            .await
            .map_err(orch_db::DbError::from)?
            .unwrap_or(false);
            if !unfinished {
                return Ok(false);
            }
            for limit in &record.limits {
                if limit.restore_current(conn, clock).await?.is_none() {
                    return Ok(false);
                }
            }
            let checked = record
                .source
                .scope(source::check_call(conn, &record.call))
                .await;
            match checked {
                Ok(()) => Ok(clock.time() < record.deadline),
                Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => Ok(false),
                Err(error) => Err(error),
            }
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
                || !record.selected.is_current(conn, record.call.thread).await?
            {
                return Ok(None);
            }
            if let Some(binding) = &record.recovery
                && !binding
                    .current_call(
                        conn,
                        &record.call,
                        &record.capability,
                        Some(&record.selected),
                        record.attempt,
                    )
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
                || !native_available(
                    conn,
                    &BuiltinSources::default(),
                    record.provider,
                    &record.model,
                    &record.capability,
                )
                .await?
            {
                return Ok(None);
            }
            let origins = record
                .source
                .scope(source::original_callers_with_limits(
                    conn,
                    &record.call,
                    clock,
                    access,
                    &record.limits,
                ))
                .await;
            let origins = match origins {
                Ok(origins) => origins,
                Err(EngineError::StaleToolSource | EngineError::ToolCallIdentity) => {
                    return Ok(None);
                }
                Err(error) => return Err(error),
            };
            if crate::provider_policy::forbidden(conn, record.call.thread, record.provider)
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
            let parameters: orch_provider_contract::models::ModelParameters =
                serde_json::from_value(model.parameters)?;
            let use_request = model::ModelUse {
                place: record.call.thread,
                provider: record.provider,
                offering: &record.model,
                groups: &model.groups,
                effort: parameters.default_effort.as_deref(),
            };
            for caller in origins.callers() {
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
                    place: record.call.thread,
                    provider: record.provider,
                    model: &record.model,
                    tier: parameters.service_tier_start(),
                    purpose: &record.capability,
                    person: origins.attribution().user_id,
                },
                clock.now(),
            )
            .await?
            {
                return Ok(None);
            }
            Ok((clock.time() < record.deadline).then(|| {
                origins
                    .callers()
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
pub(crate) async fn native_available(
    conn: &mut Conn,
    sources: &BuiltinSources,
    provider: Id,
    model: &str,
    capability: &str,
) -> Result<bool, EngineError> {
    Ok(native_state(conn, sources, provider, model, capability)
        .await?
        .is_ok())
}

pub(crate) async fn native_state(
    conn: &mut Conn,
    sources: &BuiltinSources,
    provider: Id,
    model: &str,
    capability: &str,
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
        || !crate::providers::request_status_available(&account.status)
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
    Ok(Err(CapabilityFailure::unavailable(
        capability, reason, message,
    )))
}

impl Engine {
    /// Prepare one ordinary private helper under the actual actor's re-admitted creator credential.
    /// The private tool journal and input condition retain the separate original tool throughout.
    pub async fn start_delegated_capability_helper(
        &self,
        call: &CallContext,
        capability: &str,
        input: Json,
    ) -> Result<Id, EngineError> {
        use crate::binding::{Birth, BirthAgentAccess, NewThread};
        use orch_access_contract::pages::{PageChange, Whole};
        use orch_api_contract::nodes::FirstInput;
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .ok_or(EngineError::StaleToolSource)?;
        let selected = {
            let mut conn = self.db().acquire().await?;
            source::check_call(&mut conn, call).await?;
            backend(&mut conn, call.thread, &sources, capability).await?
        };
        if selected.executor != "program-session" {
            return Err(EngineError::StaleToolSource);
        }
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
            native_state(
                &mut conn,
                &sources,
                reference.provider,
                &reference.offering,
                capability,
            )
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
            if input.get("prompt").and_then(Json::as_str).is_none() {
                return Err(EngineError::ToolCallIdentity);
            }
            let count = input.get("n").and_then(Json::as_u64).unwrap_or(1);
            let mut content = vec![ContentPart::Text {
                text: format!(
                    "Use your native {capability} capability to produce {count} result(s) for this request. The prompt and admitted options follow as JSON. Do not read other conversations or change workspace files. Return the native result, without substituting a viewed or attached image.\n{}",
                    serde_json::to_string(&input)?
                ),
            }];
            if let Some(ContentPart::Text { text }) = content.first_mut() {
                text.push_str(&format!("\n\n[Requesting thread](/thread/{})", call.thread));
            }
            for image in accepted::images() {
                let orch_model_contract::generation::GenerationPart::ImageRef {
                    blob,
                    media_type,
                    name,
                    ..
                } = image
                else {
                    return Err(EngineError::ToolCallIdentity);
                };
                content.push(serde_json::from_value(
                    json!({"type":"image","blob":blob,"media_type":media_type,"name":name}),
                )?);
            }
            Record {
                source: source::Source::captured()?,
                call: call.clone(),
                selected: selected.clone(),
                recovery: recovery_binding::binding(),
                attempt,
                helper,
                actor: origins.actor(),
                capability: capability.into(),
                agent,
                provider: provider.id,
                environment: parent.environment_id.ok_or(EngineError::StaleToolSource)?,
                topic: parent.topic_id,
                directory: parent.workspace_dir,
                model: reference.offering.clone(),
                content,
                input: input.clone(),
                limits: origins
                    .callers()
                    .iter()
                    .map(CapturedCaller::capture)
                    .collect(),
                deadline: source::capability_deadline(&mut conn, call).await?,
                profile: source::hash_json(profile.profile)?,
                actor_name,
                actor_ref: person,
                program_label,
            }
        };
        let saved = source::value(self, &slot, serde_json::to_value(&record)?).await?;
        let record: Record = serde_json::from_value(saved)?;
        if record.source != source::Source::captured()?
            || record.helper != helper
            || record.selected != selected
            || record.capability != capability
            || record.attempt != attempt
        {
            return Err(EngineError::StaleToolSource);
        }
        let link = ParentLink {
            answer: record.source.answer(),
            helper,
            slot,
        };
        let mut tx = self.db().begin().await?;
        source::check_call(&mut tx, call).await?;
        if threads::thread(&mut tx, helper).await?.is_none() {
            let origins = source::original_callers_with_limits(
                &mut tx,
                call,
                self.inner.clock.as_ref(),
                self.inner.access.as_ref(),
                &record.limits,
            )
            .await?;
            let actor = origins
                .callers()
                .iter()
                .find(|caller| caller.user() == record.actor)
                .ok_or(EngineError::StaleToolSource)?;
            let actor = CapturedCaller::capture(actor)
                .restore_current(&mut tx, self.inner.clock.as_ref())
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let principal = Principal::load(&mut tx, actor, self.inner.clock.time()).await?;
            let new = NewThread {
                topic: record.topic,
                name: "Capability helper".into(),
                agent: record.agent,
                provider: record.provider,
                environment: record.environment,
                directory: record.directory.clone(),
                arrangement: None,
                model: Some(record.model.clone()),
                effort: None,
            };
            let birth = Birth {
                agent_access: BirthAgentAccess::nothing(),
                prepare_source: Some(false),
                page: Some(PageChange {
                    whole: Some(Whole::Private),
                    ..PageChange::default()
                }),
                first_input: Some(FirstInput {
                    content: record.content.clone(),
                    client_id: format!("capability-helper:{}", record.attempt),
                }),
                settings: vec![(keys::ORCH_TOOLS_ENABLED.into(), json!([]))],
                ..Birth::default()
            };
            let prefix = crate::operation_scope::creators::Prefix::prepare(
                &mut tx,
                &principal,
                "ThreadCreate",
                &new,
                None,
            )
            .await
            .map_err(EngineError::unreadable)?;
            Box::pin(self.create_thread_staged_in_transaction(
                &mut tx,
                prefix,
                &new,
                &birth,
                helper,
                self.inner.clock.now(),
            ))
            .await
            .map_err(EngineError::unreadable)?;
            let lane = threads::thread(&mut tx, helper)
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
            let input = threads::input_by_client(
                &mut tx,
                helper,
                Some(record.actor),
                &format!("capability-helper:{}", record.attempt),
            )
            .await?
            .ok_or(EngineError::StaleToolSource)?;
            let authority = crate::inputs::saved_authority(input.submission_authority.as_ref())?
                .ok_or(EngineError::StaleToolSource)?;
            let authority = authority.with_capability_parent(link.clone())?;
            let wrote=sqlx::query("UPDATE thread_inputs SET submission_authority=$2 WHERE id=$1 AND thread_id=$3 AND author_id=$4 AND state='pending' AND submission_authority=$5")
                .bind(input.id).bind(serde_json::to_value(authority)?).bind(helper).bind(record.actor).bind(input.submission_authority)
                .execute(&mut *tx).await.map_err(orch_db::DbError::from)?.rows_affected();
            if wrote != 1 {
                return Err(EngineError::StaleToolSource);
            }
        }
        tx.commit().await.map_err(orch_db::DbError::from)?;
        self.publish_delegated_helper(call, &record).await?;
        self.wake(helper);
        Ok(helper)
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
            || command.serial_key != SerialKey::Thread(self.parent.helper).wire().as_deref()
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
            ) != Some(self.machine)
        {
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
