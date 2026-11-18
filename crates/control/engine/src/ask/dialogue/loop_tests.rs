use super::*;
use crate::{
    Engine,
    ask::{Limits, ModelReply, Step, ToolAnswer, TurnHost},
    commands::Outgoing,
};
use orch_link_contract::commands::ProviderGenerate;
use orch_types::{Id, new_id};
use std::collections::VecDeque;
use std::sync::Mutex;

struct Model {
    replies: Mutex<VecDeque<(&'static str, bool)>>,
    turns: Mutex<Vec<bool>>,
    reads: Mutex<Vec<String>>,
    accepted: Mutex<Vec<GenerationOutput>>,
}

impl Model {
    fn new(replies: &[(&'static str, bool)]) -> Self {
        Self {
            replies: Mutex::new(replies.iter().copied().collect()),
            turns: Mutex::default(),
            reads: Mutex::default(),
            accepted: Mutex::default(),
        }
    }
}
#[async_trait::async_trait]
impl TurnHost for Model {
    async fn prepare(&self, input: &GenerationInput, _: bool) -> Result<Step, EngineError> {
        Ok(Step {
            summarizing: false,
            id: new_id(),
            segment: new_id(),
            turn: new_id(),
            root: new_id(),
            thread: new_id(),
            person: new_id(),
            machine: Some(new_id()),
            room: None,
            request: Request::Api(ProviderGenerate {
                native_capability: None,
                capture_native_result: false,
                provider_id: new_id(),
                protocol: "openai-responses".into(),
                model_name: "model".into(),
                params: None,
                model_parameters: Default::default(),
                input: input.clone(),
            }),
        })
    }
    async fn generate(
        &self,
        _: &Engine,
        step: &Step,
        repair: bool,
    ) -> Result<ModelReply, EngineError> {
        self.turns.lock().unwrap().push(repair);
        let (raw, repairable) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .ok_or_else(|| EngineError::unreadable("unexpected model turn"))?;
        Ok(match decode(&step.request.input().tools, raw) {
            Ok(output) => ModelReply::Answer {
                output: Box::new(output),
                room_for_next: true,
            },
            Err(_) => ModelReply::InvalidFormat { repairable },
        })
    }
    async fn check_active(&self) -> Result<(), EngineError> {
        Ok(())
    }
    async fn assistant(&self, _: &Step, output: &GenerationOutput) -> Result<(), EngineError> {
        self.accepted.lock().unwrap().push(output.clone());
        Ok(())
    }
    async fn tool(&self, id: &str, _: &str, _: &Json) -> Result<ToolAnswer, EngineError> {
        self.reads.lock().unwrap().push(id.into());
        Ok(ToolAnswer {
            content: "Current source".into(),
            is_error: false,
            link: None,
        })
    }
    async fn tool_answer(&self, _: &str, _: &ToolAnswer) -> Result<Option<Id>, EngineError> {
        Ok(Some(new_id()))
    }
    async fn discard(&self, _: &Step) -> Result<(), EngineError> {
        Ok(())
    }
    async fn issue(&self, _: &Engine, _: Outgoing) -> Result<(), EngineError> {
        Err(EngineError::unreadable("unexpected API issue"))
    }
    async fn window_exceeded(&self, _: &Step) -> Result<bool, EngineError> {
        Ok(false)
    }
    async fn summary(&self, _: &Step, _: &GenerationOutput) -> Result<(), EngineError> {
        Err(EngineError::unreadable("unexpected context summary"))
    }
}

#[tokio::test]
async fn a_program_repairs_once_and_only_valid_complete_batches_enter_asks_tool_loop() {
    let world = crate::test_common::World::new().await;
    let input = GenerationInput {
        system: String::new(),
        messages: vec![],
        max_output_tokens: 100,
        tools: vec![GenerationTool {
            name: "docs".into(),
            description: "Search help".into(),
            parameters: json!({"type":"object","required":["query"],"properties":{"query":{"type":"string"}},"additionalProperties":false}),
        }],
    };
    const READS: &str = r#"{"message":{"role":"assistant","content":[{"kind":"tool_call","id":"a","name":"docs","arguments":{"query":"one"}},{"kind":"tool_call","id":"b","name":"docs","arguments":{"query":"two"}}]},"stop":"tools"}"#;
    const DONE: &str = r#"{"message":{"role":"assistant","content":[{"kind":"text","text":"Answer"}]},"stop":"complete"}"#;
    let model = Model::new(&[("incomplete", true), (READS, true), (DONE, true)]);
    assert_eq!(
        world
            .engine()
            .run_ask(&model, input.clone(), Limits { steps: 3, tools: 2 })
            .await
            .unwrap(),
        GenerationStop::Complete
    );
    assert_eq!(*model.turns.lock().unwrap(), [false, true, false]);
    assert_eq!(*model.reads.lock().unwrap(), ["a", "b"]);
    assert_eq!(model.accepted.lock().unwrap().len(), 2);
    for (replies, steps) in [
        (vec![("bad", false)], 1),
        (vec![("bad", true), ("bad again", true)], 2),
        (vec![(READS, true)], 1),
    ] {
        let model = Model::new(&replies);
        assert!(
            world
                .engine()
                .run_ask(&model, input.clone(), Limits { steps: 3, tools: 1 })
                .await
                .is_err()
        );
        assert_eq!(model.turns.lock().unwrap().len(), steps);
        assert!(model.reads.lock().unwrap().is_empty());
        assert!(model.accepted.lock().unwrap().is_empty());
    }
}
