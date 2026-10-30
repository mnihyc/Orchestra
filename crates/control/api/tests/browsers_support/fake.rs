//! A fake browser directive reaches the real browser engine and says the answer back.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

use super::{Floor, WAIT};
#[path = "../rig_tool_source/mod.rs"]
mod source;
use futures::StreamExt;
use orch_adapter_fake::FakeAdapter;
use orch_agent_contract::adapter::{AgentAdapter, EventSink, EventStream};
use orch_agent_contract::session::{SessionSpec, TurnStart};
use orch_link_contract::commands::{Command, CommandKind, OrchToolResult};
use orch_types::{Id, Json};
use serde_json::json;

/// Calls with the tool's complete arguments, including its optional note and first address.
pub async fn call(floor: &Floor, thread: Id, arguments: Json) -> OrchToolResult {
    let turn = source::active(&floor.scene, thread)
        .await
        .expect("an actual admitted tool source");
    call_on(floor, thread, turn, arguments).await
}

/// A caller that already owns an admitted turn names that exact source explicitly.
pub async fn call_on(floor: &Floor, thread: Id, turn: Id, arguments: Json) -> OrchToolResult {
    let call_ref = orch_types::new_id().to_string();
    floor
        .scene
        .rig
        .engine
        .answer_tool_call(&orch_engine::tools::CallContext {
            machine: floor.scene.rig.machine_id,
            thread,
            turn,
            call_ref: call_ref.clone(),
            attempt: None,
            tool: orch_agent_contract::events::OrchTool::UseBrowser,
            arguments,
        })
        .await
        .unwrap();
    tokio::time::timeout(WAIT, async {
        loop {
            if let Some(reply) = floor
                .scene
                .rig
                .seen
                .of_kind(CommandKind::OrchToolResult)
                .into_iter()
                .find_map(|command| match command {
                    Command::OrchToolResult(result)
                        if result.thread_id == thread && result.call_ref == call_ref =>
                    {
                        Some(result)
                    }
                    _ => None,
                })
            {
                return reply;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap()
}

/// One fake session whose browser commands are answered by the real engine.
pub struct Agent {
    /// The calling thread.
    pub thread: Id,
    adapter: FakeAdapter,
    events: EventStream,
    _home: tempfile::TempDir,
}

impl Agent {
    /// Opens the fake session beside a thread in the fixture's environment.
    pub async fn start(floor: &Floor, thread: Id) -> Self {
        let _turn = source::active(&floor.scene, thread)
            .await
            .expect("an actual admitted tool source");
        let segment = floor.scene.rig.segment_of(thread).await;
        let home = tempfile::tempdir().unwrap();
        let mut adapter = FakeAdapter::new(floor.app.clock.clone());
        let (sink, events) = EventSink::channel();
        let start = serde_json::from_value(json!({
            "provider_id":floor.scene.on.provider,"agent_id":floor.scene.on.agent,"harness_kind":"fake",
            "profile":{"harness_version":null,"config":{},"env":{}},"home":home.path().display().to_string(),
            "provider_config":null,"binary":null,"loopback":{"base_url":"http://127.0.0.1:41822","token":"t".repeat(64)}
        })).unwrap();
        adapter.start(start, sink).await.unwrap();
        let spec: SessionSpec = serde_json::from_value(json!({
            "agent_id":floor.scene.on.agent,"thread_id":thread,"segment_id":segment,
                "mode":"new","native_session_ref":null,"upto_turn_ordinal":null,"directory":home.path().display().to_string(),"cwd":home.path().display().to_string(),
                "mounts":[],"model":"fake-large","effort":null,"params":{},"approval_mode":"on_request","sandbox_mode":"workspace","instructions":"Browser test.",
                "memory":null,"arrangement":"together","orch_tools":[],"raw_events":false,
            "session_key":"s".repeat(32),"archive":null,"history":null,"provider":{"path":"native","provider_id":floor.scene.on.provider},
            "exec":{"executor":null,"tool_set":null,"environment_id":floor.environment,"generation":1},"mcp":[]
        })).unwrap();
        adapter.open_session(spec).await.unwrap();
        Self {
            thread,
            adapter,
            events,
            _home: home,
        }
    }

    /// Runs a directive, returns its tool reply and verifies the fake also says it.
    pub async fn run(&mut self, floor: &Floor, line: &str) -> OrchToolResult {
        let admitted = source::active(&floor.scene, self.thread)
            .await
            .expect("the admitted directive source");
        let turn: TurnStart = serde_json::from_value(json!({
            "thread_id":self.thread,"turn_id":admitted,"text":format!("#!fake\n{line}"),"images":[],"model":"fake-large","effort":null,"params":{},"approval_mode":"on_request","sandbox_mode":"workspace"
        })).unwrap();
        tokio::time::timeout(WAIT, async {
            loop {
                match self.adapter.start_turn(turn.clone()).await {
                    Ok(()) => break,
                    Err(orch_agent_contract::adapter::AdapterError::Other { code, .. })
                        if code == "conflict" =>
                    {
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await
                    }
                    Err(error) => panic!("the fake could not start its next turn: {error}"),
                }
            }
        })
        .await
        .unwrap();
        let mut answer = None;
        let mut said = Vec::new();
        tokio::time::timeout(WAIT, async {
            while let Some(event) = self.events.next().await {
                let (kind, body) = event.to_wire().unwrap();
                if kind == "orch.tool_call" {
                    let call_ref = body["call_ref"].as_str().unwrap();
                    floor
                        .scene
                        .rig
                        .engine
                        .answer_tool_call(&orch_engine::tools::CallContext {
                            machine: floor.scene.rig.machine_id,
                            thread: self.thread,
                            turn: body["turn_id"].as_str().unwrap().parse().unwrap(),
                            call_ref: call_ref.to_owned(),
                            attempt: None,
                            tool: serde_json::from_value(body["tool"].clone()).unwrap(),
                            arguments: body["arguments"].clone(),
                        })
                        .await
                        .unwrap();
                    let reply = loop {
                        let reply = floor
                            .scene
                            .rig
                            .seen
                            .of_kind(CommandKind::OrchToolResult)
                            .into_iter()
                            .find_map(|command| match command {
                                Command::OrchToolResult(result)
                                    if result.thread_id == self.thread
                                        && result.call_ref == call_ref =>
                                {
                                    Some(result)
                                }
                                _ => None,
                            });
                        if let Some(reply) = reply {
                            break reply;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    };
                    let result = if let Some(error) = &reply.error {
                        Err(orch_agent_contract::session::ToolError {
                            code: error.code.clone(),
                            message: error.message.clone(),
                        })
                    } else {
                        Ok(reply.result.clone().unwrap())
                    };
                    self.adapter
                        .tool_result(self.thread, call_ref, result)
                        .await
                        .unwrap();
                    answer = Some(reply);
                }
                if kind == "item.completed"
                    && let Some(text) = body["payload"]["text"].as_str()
                {
                    said.push(text.to_owned());
                }
                if kind == "turn.completed" {
                    break;
                }
            }
        })
        .await
        .unwrap();
        let answer = answer.unwrap();
        assert!(
            !said.is_empty(),
            "the browser directive says its result or refusal"
        );
        if let Some(result) = &answer.result {
            assert!(
                said.iter()
                    .any(|text| serde_json::from_str::<Json>(text).ok().as_ref() == Some(result))
            );
        }
        answer
    }
}
