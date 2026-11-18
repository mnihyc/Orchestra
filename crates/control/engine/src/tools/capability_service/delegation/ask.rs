//! The Ask model uses the same private program lifetime and physical result receipt as other consumers.
use super::*;
use crate::ask::{ModelReply, dialogue, program::Source};
use async_trait::async_trait;
use orch_model_contract::generation::GenerationPart;
use orch_provider_contract::capabilities::{CapabilityItem, CapabilityResult, ProgramPlacement};

pub(super) struct Dialogue;
#[async_trait]
impl codecs::Codec for Dialogue {
    fn support(&self) -> ExecutionSupport {
        ExecutionSupport {
            input: "ask.step".into(),
            result: "ask.reply".into(),
            confined_tools: true,
            read_tools: true,
            ..Default::default()
        }
    }
    fn prepare(
        &self,
        capability: &str,
        _: &Json,
        media: &[GenerationPart],
    ) -> Result<Vec<ContentPart>, EngineError> {
        if capability != "ask" || !media.is_empty() {
            return Err(EngineError::ToolCallIdentity);
        }
        // The canonical conversation already retains its governed sources. A helper's ordinary
        // input history needs only this receipt; its wire text comes from the exact Ask call.
        Ok(vec![ContentPart::Text {
            text: "Ask model request".into(),
        }])
    }
    async fn read(
        &self,
        conn: &mut Conn,
        frame: &codecs::Frame<'_>,
    ) -> Result<Option<codecs::Receipt>, EngineError> {
        if frame.capability != "ask" {
            return Err(EngineError::ToolCallIdentity);
        }
        Ok(
            text::completed(conn, frame.helper, frame.turn, frame.machine)
                .await?
                .map(|answer| codecs::Receipt {
                    command: answer.command,
                    result: CapabilityResult {
                        items: vec![CapabilityItem {
                            family: "ask".into(),
                            native_ref: Some(answer.native_ref),
                            payload: json!({"reply":answer.text}),
                        }],
                        usage: Json::Null,
                    },
                }),
        )
    }
}

impl Engine {
    pub async fn execute_program_ask(&self, source: Source) -> Result<ModelReply, EngineError> {
        let helper = crate::ids::derived("capability.helper", &[source.call.as_bytes()]);
        let retained = {
            let mut conn = self.db().acquire().await?;
            source.retained(&mut conn).await?
        };
        let record: Record = if let Some(record) = retained {
            serde_json::from_value(record)?
        } else {
            let record = {
                let mut tx = self.db().begin().await?;
                let callers = source
                    .callers(&mut tx, self.inner.clock.as_ref())
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                let backend = &source.request.backend;
                let offering = backend
                    .offering
                    .as_ref()
                    .ok_or(EngineError::StaleToolSource)?;
                let provider = resources::thread_engine::provider(&mut tx, offering.provider)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                let agent = provider.agent_id.ok_or(EngineError::StaleToolSource)?;
                let profile = resources::thread_engine::agent(&mut tx, agent)
                    .await?
                    .ok_or(EngineError::StaleToolSource)?;
                let descriptor = crate::harnesses::descriptor_of(&profile)?
                    .ok_or(EngineError::StaleToolSource)?;
                let required = codecs::Codec::support(&Dialogue);
                let codec = codecs::installed()
                    .map_err(EngineError::unreadable)?
                    .get(&required)?;
                backend_state(&mut tx, &BuiltinSources::default(), "ask", backend)
                    .await?
                    .map_err(EngineError::from)?;
                let placement = placement::Placement::configured(
                    &mut tx,
                    source.root,
                    None,
                    &ProgramPlacement::read(&backend.config).map_err(EngineError::unreadable)?,
                )
                .await?;
                let params = source.request.params.as_ref().unwrap_or(&Json::Null);
                let effort = helper_effort(
                    &mut tx,
                    provider.id,
                    &offering.offering,
                    placement.topic,
                    orch_provider_contract::calls::requested_effort(params)
                        .map_err(EngineError::unreadable)?,
                )
                .await?;
                let actor_ref = crate::views::user_ref_json(&mut tx, source.person).await?;
                let actor_name = actor_ref["display_name"]
                    .as_str()
                    .or_else(|| actor_ref["username"].as_str())
                    .unwrap_or_default()
                    .to_owned();
                if source.request.format.enforced != dialogue::structured_output(&descriptor) {
                    return Err(EngineError::StaleToolSource);
                }
                let input = json!({"max_output_tokens":source.request.input.max_output_tokens,"schema_enforced":source.request.format.enforced});
                let content = codec.prepare("ask", &input, &[])?;
                let record = Record {
                    origin: origin::Producer::capture_ask(
                        source.clone(),
                        callers.iter().map(CapturedCaller::capture).collect(),
                    ),
                    attempt: source.call,
                    helper,
                    actor: source.person,
                    person: Some(source.person),
                    capability: "ask".into(),
                    execution: required,
                    agent,
                    provider: provider.id,
                    placement,
                    model: offering.offering.clone(),
                    effort,
                    content,
                    input,
                    material: vec![],
                    deadline: source.deadline,
                    profile: source::hash_json(profile.profile)?,
                    actor_name,
                    actor_ref,
                    program_label: json!({"en":descriptor.harness.name,"zh-CN":descriptor.harness.name}),
                };
                tx.commit().await.map_err(orch_db::DbError::from)?;
                record
            };
            serde_json::from_value(source.retain(self, &serde_json::to_value(record)?).await?)?
        };
        if !matches!(&record.origin,origin::Producer::Ask{source:owner,..} if owner.as_ref()==&source)
            || record.helper != helper
            || record.attempt != source.call
            || record.capability != "ask"
        {
            return Err(EngineError::ToolCallIdentity);
        }
        let parent = self.start_program_record(&record).await?;
        let answer = run::await_result(self, &parent).await?;
        self.drive_turn_end_work(helper).await?;
        let item = answer
            .items
            .first()
            .filter(|item| item.family == "ask")
            .ok_or(EngineError::ToolCallIdentity)?;
        let reply = item.payload["reply"]
            .as_str()
            .ok_or(EngineError::ToolCallIdentity)?;
        let offering = source
            .request
            .backend
            .offering
            .as_ref()
            .ok_or(EngineError::ToolCallIdentity)?;
        let mut conn = self.db().acquire().await?;
        let room = self
            .room_for_ask(
                &mut conn,
                source.root,
                offering.provider,
                &offering.offering,
                None,
                source.person,
                Some(source.turn),
            )
            .await?;
        match dialogue::decode(&source.request.input.tools, reply) {
            Ok(output) => Ok(ModelReply::Answer {
                output: Box::new(output),
                room_for_next: room,
            }),
            Err(_) => Ok(ModelReply::InvalidFormat {
                repairable: room && record.input["schema_enforced"].as_bool() == Some(false),
            }),
        }
    }
    pub(crate) async fn stop_program_ask(&self, call: Id) -> Result<(), EngineError> {
        let parent = ParentLink {
            work: crate::ids::derived("capability.work", &[call.as_bytes()]),
            helper: crate::ids::derived("capability.helper", &[call.as_bytes()]),
        };
        let owned = {
            let mut conn = self.db().acquire().await?;
            capability_work::program(&mut conn, parent.work).await?
        };
        if owned.is_some_and(|work| {
            work.purpose == "ask"
                && work.request == call
                && work.attempt == call
                && work.helper == parent.helper
        }) {
            run::stop_owned(self, &parent, false).await?;
        }
        Ok(())
    }
}
