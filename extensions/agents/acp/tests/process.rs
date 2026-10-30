//! The generic adapter against a real operating-system process: a small agent written in Python that speaks the
//! protocol on its standard streams, started by the adapter's own launcher, with the files, variables and home
//! the manifest says, and ended when the adapter is stopped.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::time::Duration;

use common::{agent_start, spec_in, turn_for};
use futures::StreamExt as _;
use orch_adapter_acp::AcpAdapter;
use orch_adapter_acp::program::AcpProgram;
use orch_agent_contract::adapter::{AgentAdapter, EventSink};
use orch_agent_contract::session::SessionMode;
use orch_types::{Json, new_id};
use serde_json::json;

/// The agent. Braces are doubled where the manifest writes it as a template.
const AGENT: &str = r#"
import json, os, sys

def send(frame):
    sys.stdout.write(json.dumps(frame) + "\n")
    sys.stdout.flush()

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    ident = message.get("id")
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": ident, "result": {
            "protocolVersion": 1,
            "agentInfo": {"name": "py-agent", "version": "1.0.0"},
            "agentCapabilities": {"mcpCapabilities": {"http": True}, "promptCapabilities": {"image": False},
                                   "sessionCapabilities": {"close": {}, "resume": {}}}}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": ident, "result": {"sessionId": "py-1", "configOptions": []}})
    elif method == "session/prompt":
        seen = {
            "token": os.environ.get("AGENT_TOKEN"),
            "home": os.environ.get("AGENT_HOME"),
            "cwd": os.getcwd(),
            "leak": os.environ.get("HOME"),
            "said": message["params"]["prompt"][0]["text"],
        }
        send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": "py-1", "update": {
            "sessionUpdate": "agent_message_chunk", "messageId": "m1",
            "content": {"type": "text", "text": json.dumps(seen)}}}})
        send({"jsonrpc": "2.0", "id": ident, "result": {"stopReason": "end_turn"}})
    elif method == "session/close":
        send({"jsonrpc": "2.0", "id": ident, "result": {}})
sys.stderr.write("the agent's input ended\n")
"#;

fn program() -> AcpProgram {
    let template = AGENT.replace('{', "{{").replace('}', "}}");
    AcpProgram::from_json(&json!({
        "kind": "pyagent", "title": "Python agent", "supported_range": "*",
        "programs": ["python3"],
        "args": ["-I", "{file:agent.py}"],
        "env": { "AGENT_TOKEN": "{llm_token}", "AGENT_HOME": "{home}" },
        "files": [ { "path": "agent.py", "template": template } ],
        "approvals": { "ask": { "permission_mode": "ask" } },
        "protocols": { "api": { "openai-chat": "openai-completions" }, "base_suffix": { "openai-chat": "/v1" } },
        "cancel_grace_ms": 500
    }))
    .unwrap()
}

async fn until(
    events: &mut orch_agent_contract::adapter::EventStream,
    name: &str,
) -> Vec<(String, Json)> {
    let mut seen = Vec::new();
    loop {
        let event = tokio::time::timeout(Duration::from_secs(60), events.next())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "nothing came; so far {:?}",
                    seen.iter()
                        .map(|(name, _): &(String, Json)| name)
                        .collect::<Vec<_>>()
                )
            })
            .expect("the adapter's stream closed");
        let (kind, body) = event.to_wire().unwrap();
        let done = kind == name;
        seen.push((kind.to_owned(), body));
        if done {
            return seen;
        }
    }
}

#[tokio::test]
async fn a_real_process_is_started_in_its_own_home_with_its_files_and_only_the_variables_the_manifest_names()
 {
    let home = tempfile::tempdir().unwrap();
    let work = tempfile::tempdir().unwrap();
    let (provider, profile) = (new_id(), new_id());
    let mut start = agent_start(provider, profile, home.path());
    start.binary = Some("python3".to_owned());
    let mut adapter = AcpAdapter::new(program());
    let (sink, mut events) = EventSink::channel();
    let started = adapter.start(start.clone(), sink).await.unwrap();
    assert!(
        !started.harness_version.is_empty(),
        "the version python prints is read"
    );
    assert_eq!(started.descriptor.check(), Ok(()));
    let spec = spec_in(work.path(), profile, provider, SessionMode::New);
    let opened = adapter.open_session(spec.clone()).await.unwrap();
    assert_eq!(opened.native_session_ref, "py-1");
    adapter
        .start_turn(turn_for(&spec, "hello from the test"))
        .await
        .unwrap();
    let seen = until(&mut events, "turn.completed").await;
    let message = seen
        .iter()
        .find(|(kind, body)| kind == "item.completed" && body["payload"]["phase"] == "final")
        .expect("a finished message");
    let said: Json = serde_json::from_str(message.1["payload"]["text"].as_str().unwrap()).unwrap();
    assert_eq!(
        said["token"],
        "t".repeat(64),
        "the token is the connector's, given by name"
    );
    assert_eq!(said["said"], "hello from the test");
    assert_eq!(
        said["leak"],
        Json::Null,
        "the connector's own HOME is not passed on: the manifest does not name it"
    );
    let own_home = said["home"].as_str().unwrap();
    assert!(
        std::path::Path::new(own_home).starts_with(home.path()),
        "the program's home is below the profile's: {own_home}"
    );
    assert!(
        std::path::Path::new(own_home).join("agent.py").is_file(),
        "the file the manifest writes is in it"
    );
    assert_eq!(
        std::fs::canonicalize(said["cwd"].as_str().unwrap()).unwrap(),
        std::fs::canonicalize(own_home).unwrap().join("neutral"),
        "the program starts in a neutral directory, not in the thread's"
    );
    adapter.stop().await.unwrap();
}
