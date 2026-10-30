//! A tool server of the tests: a program that speaks MCP over its standard input and output, which is this very test binary asked to run one test as a server.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking; each test uses some of what is here"
)]

use std::io::{BufRead, Write};

use serde_json::{Value, json};

/// The variable that makes the test below a server.
pub const FLAG: &str = "ORCH_TEST_STAND_IN_SERVER";

/// The name of the test that is the server.
pub const TEST_NAME: &str = "stand_in_for_a_tool_server";

/// The command line of the server, as a definition gives it.
pub fn command() -> Vec<String> {
    vec![
        std::env::current_exe().unwrap().display().to_string(),
        "--exact".to_owned(),
        TEST_NAME.to_owned(),
        "--nocapture".to_owned(),
        "--test-threads=1".to_owned(),
    ]
}

/// Answers MCP on the standard input and output until the input ends. Its tools: `echo` says what it is told,
/// `secret` tells the variable `STAND_IN_SECRET`, `whoami` tells its own process number, `slow` waits a
/// second and reports progress, and `crash` ends the server.
pub fn run() {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    // The test harness has begun a line that the first message would end up on.
    let _ = writeln!(out);
    let _ = out.flush();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { return };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(method), id) = (
            message.get("method").and_then(Value::as_str),
            message.get("id").cloned(),
        ) else {
            continue;
        };
        let result = match method {
            "initialize" => json!({
                "protocolVersion": "2025-06-18", "capabilities": { "tools": {} },
                "serverInfo": { "name": "stand-in", "version": "1" }
            }),
            "tools/list" => json!({ "tools": [
                { "name": "echo", "description": "says what it is told", "inputSchema": { "type": "object" } },
                { "name": "secret", "description": "tells a variable", "inputSchema": { "type": "object" } },
                { "name": "whoami", "description": "tells its process number", "inputSchema": { "type": "object" } },
                { "name": "slow", "description": "takes a second", "inputSchema": { "type": "object" } },
                { "name": "crash", "description": "ends the server", "inputSchema": { "type": "object" } }
            ]}),
            "tools/call" => {
                let tool = message
                    .pointer("/params/name")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                let text = |text: String| json!({ "content": [{ "type": "text", "text": text }] });
                match tool {
                    "crash" => std::process::exit(7),
                    "secret" => text(std::env::var("STAND_IN_SECRET").unwrap_or_default()),
                    "whoami" => text(std::process::id().to_string()),
                    "slow" => {
                        if let Some(token) = message.pointer("/params/_meta/progressToken") {
                            let progress = json!({ "jsonrpc": "2.0", "method": "notifications/progress",
                                "params": { "progressToken": token, "progress": 1, "total": 2 } });
                            let _ = writeln!(out, "{progress}");
                            let _ = out.flush();
                        }
                        std::thread::sleep(std::time::Duration::from_millis(1200));
                        text("done slowly".to_owned())
                    }
                    _ => text(
                        message
                            .pointer("/params/arguments/text")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_owned(),
                    ),
                }
            }
            _ => continue,
        };
        if let Some(id) = id {
            let reply = json!({ "jsonrpc": "2.0", "id": id, "result": result });
            let _ = writeln!(out, "{reply}");
            let _ = out.flush();
        }
    }
}
