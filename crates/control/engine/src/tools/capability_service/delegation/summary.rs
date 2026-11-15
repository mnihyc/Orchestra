//! The real summary producer enters the same private helper, native receipt and lifetime service.
use super::*;
use orch_summarizer_model::CompletionAnswer;

impl Engine {
    pub(crate) async fn execute_program_summary(
        &self,
        owner: crate::summaries::ProgramSource,
    ) -> Result<CompletionAnswer, EngineError> {
        let sources = self
            .inner
            .capability_services
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
            .unwrap_or_default();
        let required = sources
            .capabilities()
            .declarations()
            .routes
            .get(&owner.backend().route)
            .filter(|route| {
                route.executor.as_deref() == Some("program-session")
                    && route
                        .capabilities
                        .iter()
                        .any(|capability| capability == "summary")
            })
            .and_then(|route| route.execution.clone())
            .ok_or(EngineError::StaleToolSource)?;
        let codecs = codecs::installed().map_err(EngineError::unreadable)?;
        let codec = codecs.get(&required)?;
        let attempt = owner.call_id();
        let helper = crate::ids::derived("capability.helper", &[attempt.as_bytes()]);
        let record = {
            let mut conn = self.db().acquire().await?;
            let callers = owner
                .current_callers(&mut conn, self.inner.clock.as_ref())
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            backend_state(&mut conn, &sources, "summary", owner.backend())
                .await?
                .map_err(EngineError::from)?;
            let offering = owner
                .backend()
                .offering
                .as_ref()
                .ok_or(EngineError::StaleToolSource)?;
            let provider = resources::thread_engine::provider(&mut conn, offering.provider)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let agent = provider.agent_id.ok_or(EngineError::StaleToolSource)?;
            let profile = resources::thread_engine::agent(&mut conn, agent)
                .await?
                .ok_or(EngineError::StaleToolSource)?;
            let descriptor =
                crate::harnesses::descriptor_of(&profile)?.ok_or(EngineError::StaleToolSource)?;
            if !descriptor
                .side_thread_modes
                .contains(&orch_agent_contract::descriptor::SideReading::NoTools)
            {
                return Err(EngineError::StaleToolSource);
            }
            let actor = owner.actor()?;
            let actor_ref = crate::views::user_ref_json(&mut conn, actor).await?;
            let actor_name = actor_ref
                .get("display_name")
                .and_then(Json::as_str)
                .filter(|name| !name.is_empty())
                .or_else(|| actor_ref.get("username").and_then(Json::as_str))
                .unwrap_or("")
                .to_owned();
            let input = owner.input();
            Record {
                origin: origin::Producer::capture_summary(
                    owner.clone(),
                    callers.iter().map(CapturedCaller::capture).collect(),
                ),
                attempt,
                helper,
                actor,
                person: owner.person(),
                capability: "summary".into(),
                execution: required.clone(),
                agent,
                provider: provider.id,
                placement: owner.placement(&mut conn).await?,
                model: offering.offering.clone(),
                content: codec.prepare("summary", &input, &[])?,
                input,
                deadline: owner.deadline(),
                profile: source::hash_json(profile.profile)?,
                actor_name,
                actor_ref,
                program_label: json!({"en":descriptor.harness.name,"zh-CN":descriptor.harness.name}),
            }
        };
        let saved = owner.retain(self, &serde_json::to_value(&record)?).await?;
        let record: Record = serde_json::from_value(saved)?;
        if record.origin.summary() != Some(&owner)
            || record.helper != helper
            || record.attempt != attempt
            || record.capability != "summary"
            || record.execution != required
        {
            return Err(EngineError::StaleToolSource);
        }
        let parent = self.start_program_record(&record).await?;
        let result = run::await_result(self, &parent).await?;
        let answer = result
            .items
            .into_iter()
            .next()
            .filter(|item| item.family == "summary")
            .ok_or(EngineError::StaleToolSource)?;
        let text = answer
            .payload
            .get("text")
            .and_then(Json::as_str)
            .ok_or(EngineError::StaleToolSource)?
            .to_owned();
        Ok(CompletionAnswer {
            text,
            usage: None,
            request_ref: answer.native_ref,
        })
    }
}
