//! The client's own methods, carried out in the environment: files and terminals of an agent that works through Orch's executor.
//!
//! An agent that asks for `fs/read_text_file`, `fs/write_text_file` and `terminal/*` leaves the work to its client.
//! Where the arrangement is `separated`, the agent runs in its provider's runtime and the environment is somewhere
//! else, so the client carries each call out there: through Orch's executor, as the MCP server named `exec` of the
//! session, with the tools of its `plain` set (`read`, `write`, `shell`, `shell_output`, `shell_stop`). An agent
//! that uses these methods needs no plugin to be separated.
//!
//! The adapter speaks MCP's Streamable HTTP as a small client of its own: one POST for each call, answered either
//! as one JSON document or as an event stream with the answer in its first `data:` line. The executor keeps
//! nothing between calls but what is kept by thread, so no session of the transport is needed.
//!
//! A terminal is a background process of the executor: `terminal/create` starts it and its `process_id` is the
//! terminal's id, `terminal/output` reads what it printed, `terminal/wait_for_exit` asks for output in long waits
//! until the process has ended, and `terminal/kill` and `terminal/release` stop it.

use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use orch_types::Json;
use serde_json::json;

use crate::wire;

/// How long one call of a tool waits for the executor: a little over the longest wait a tool of the plain set
/// makes by itself.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(70);

/// How long one wait for the end of a terminal's command is, in milliseconds, which the executor allows at most.
const WAIT_STEP_MS: i64 = 25_000;

/// How much of a terminal's output is read, when the agent says nothing.
const DEFAULT_OUTPUT_LIMIT: usize = 1 << 20;

/// An error answered to the agent's request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientError {
    /// The JSON-RPC code.
    pub code: i64,
    /// What went wrong.
    pub message: String,
}

impl ClientError {
    fn failed(message: impl Into<String>) -> Self {
        Self {
            code: crate::rpc::INTERNAL_ERROR,
            message: message.into(),
        }
    }

    fn invalid(message: impl Into<String>) -> Self {
        Self {
            code: crate::rpc::INVALID_PARAMS,
            message: message.into(),
        }
    }
}

/// Whether a method is one of the client's own that [`Methods`] carries out.
#[must_use]
pub fn is_client_method(method: &str) -> bool {
    matches!(
        method,
        wire::client::FS_READ
            | wire::client::FS_WRITE
            | wire::client::TERMINAL_CREATE
            | wire::client::TERMINAL_OUTPUT
            | wire::client::TERMINAL_WAIT
            | wire::client::TERMINAL_KILL
            | wire::client::TERMINAL_RELEASE
    )
}

/// The tools of the executor, called as an MCP client.
pub struct Executor {
    url: String,
    token: String,
    http: reqwest::Client,
}

impl Executor {
    /// A client of the server at `url`, presenting `token`.
    #[must_use]
    pub fn new(url: String, token: String) -> Self {
        Self {
            url,
            token,
            http: reqwest::Client::new(),
        }
    }

    async fn post(&self, body: &Json) -> Result<Option<Json>, ClientError> {
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .header("accept", "application/json, text/event-stream")
            .timeout(CALL_TIMEOUT)
            .json(body)
            .send()
            .await
            .map_err(|error| {
                ClientError::failed(format!("the executor could not be reached: {error}"))
            })?;
        let status = response.status();
        if !status.is_success() {
            let said = response.text().await.unwrap_or_default();
            return Err(ClientError::failed(format!(
                "the executor answered {status}: {}",
                said.chars().take(300).collect::<String>()
            )));
        }
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok(None);
        }
        let streamed = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .is_some_and(|kind| kind.starts_with("text/event-stream"));
        let text = response.text().await.map_err(|error| {
            ClientError::failed(format!("the executor's answer was cut: {error}"))
        })?;
        let wanted = body.get("id");
        let candidates: Vec<&str> = if streamed {
            text.lines()
                .filter_map(|line| line.strip_prefix("data:"))
                .map(str::trim)
                .collect()
        } else {
            vec![text.trim()]
        };
        for candidate in candidates {
            if let Ok(message) = serde_json::from_str::<Json>(candidate)
                && message.get("id") == wanted
            {
                return Ok(Some(message));
            }
        }
        Err(ClientError::failed("the executor's answer holds no result"))
    }

    /// Calls one tool; the answer is its structured result, else the JSON its text holds, else the text.
    pub async fn call(&self, tool: &str, arguments: Json) -> Result<Json, ClientError> {
        let answer = self
            .post(&json!({
                "jsonrpc": "2.0", "id": 1, "method": "tools/call",
                "params": { "name": tool, "arguments": arguments }
            }))
            .await?
            .ok_or_else(|| ClientError::failed("the executor answered nothing"))?;
        if let Some(error) = answer.get("error") {
            return Err(ClientError::failed(
                error
                    .get("message")
                    .and_then(Json::as_str)
                    .unwrap_or("the executor refused the call"),
            ));
        }
        let result = answer.get("result").cloned().unwrap_or(Json::Null);
        let text = result
            .get("content")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|block| block.get("text").and_then(Json::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if result.get("isError").and_then(Json::as_bool) == Some(true) {
            return Err(ClientError::failed(if text.is_empty() {
                format!("{tool} failed")
            } else {
                text
            }));
        }
        Ok(result
            .get("structuredContent")
            .cloned()
            .filter(|structured| !structured.is_null())
            .or_else(|| serde_json::from_str(&text).ok())
            .unwrap_or(Json::String(text)))
    }
}

struct Terminal {
    process_id: String,
    output: String,
    exit: Option<Exit>,
    limit: usize,
    truncated: bool,
}

#[derive(Clone, Copy)]
struct Exit {
    code: Option<i64>,
}

/// The client methods of one session.
pub struct Methods {
    executor: Executor,
    cwd: String,
    terminals: Mutex<HashMap<String, Terminal>>,
}

fn text_of<'a>(params: &'a Json, member: &str) -> Option<&'a str> {
    params.get(member).and_then(Json::as_str)
}

/// A word as the executor's shell reads it as one word.
#[must_use]
pub fn quote(text: &str) -> String {
    if !text.is_empty()
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./=:,@%+".contains(c))
    {
        text.to_owned()
    } else {
        format!("'{}'", text.replace('\'', "'\\''"))
    }
}

/// A command and its arguments as one line for the executor's shell, each argument quoted.
#[must_use]
pub fn command_line(command: &str, args: &[String]) -> String {
    std::iter::once(command)
        .chain(args.iter().map(String::as_str))
        .enumerate()
        .map(|(position, word)| {
            // The command itself may be a whole line the agent wrote (`ls -la | head`), which is left as it is.
            if position == 0 && args.is_empty() {
                word.to_owned()
            } else {
                quote(word)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

impl Methods {
    /// The methods of a session whose executor is `executor` and which starts in `cwd`.
    #[must_use]
    pub fn new(executor: Executor, cwd: String) -> Self {
        Self {
            executor,
            cwd,
            terminals: Mutex::new(HashMap::new()),
        }
    }

    /// Carries out one request of the agent.
    pub async fn handle(&self, method: &str, params: &Json) -> Result<Json, ClientError> {
        match method {
            wire::client::FS_READ => self.read(params).await,
            wire::client::FS_WRITE => self.write(params).await,
            wire::client::TERMINAL_CREATE => self.create(params).await,
            wire::client::TERMINAL_OUTPUT => self.output(params).await,
            wire::client::TERMINAL_WAIT => self.wait(params).await,
            wire::client::TERMINAL_KILL => self.kill(params).await,
            wire::client::TERMINAL_RELEASE => self.release(params).await,
            other => Err(ClientError {
                code: crate::rpc::METHOD_NOT_FOUND,
                message: format!("{other} is not a method this client has"),
            }),
        }
    }

    async fn read(&self, params: &Json) -> Result<Json, ClientError> {
        let path =
            text_of(params, "path").ok_or_else(|| ClientError::invalid("a read names its path"))?;
        let mut arguments = json!({ "path": path });
        if let Some(line) = params.get("line").and_then(Json::as_i64) {
            arguments["offset"] = json!(line);
        }
        if let Some(limit) = params.get("limit").and_then(Json::as_i64) {
            arguments["limit"] = json!(limit);
        }
        let answer = self.executor.call("read", arguments).await?;
        let content = answer
            .get("content")
            .or_else(|| answer.get("text"))
            .and_then(Json::as_str)
            .map(str::to_owned)
            .or_else(|| answer.as_str().map(str::to_owned))
            .ok_or_else(|| ClientError::failed("the executor's answer to a read holds no text"))?;
        Ok(json!({ "content": content }))
    }

    async fn write(&self, params: &Json) -> Result<Json, ClientError> {
        let path = text_of(params, "path")
            .ok_or_else(|| ClientError::invalid("a write names its path"))?;
        let content = text_of(params, "content")
            .ok_or_else(|| ClientError::invalid("a write has its content"))?;
        self.executor
            .call("write", json!({ "path": path, "content": content }))
            .await?;
        Ok(json!({}))
    }

    async fn create(&self, params: &Json) -> Result<Json, ClientError> {
        let command = text_of(params, "command")
            .ok_or_else(|| ClientError::invalid("a terminal has its command"))?;
        let args: Vec<String> = params
            .get("args")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(Json::as_str)
            .map(str::to_owned)
            .collect();
        let mut line = command_line(command, &args);
        let env: Vec<String> = params
            .get("env")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
            .filter_map(|variable| {
                let name = variable.get("name").and_then(Json::as_str)?;
                let value = variable.get("value").and_then(Json::as_str)?;
                name.chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_')
                    .then(|| format!("export {name}={}", quote(value)))
            })
            .collect();
        if !env.is_empty() {
            line = format!("{}; {line}", env.join("; "));
        }
        let cwd = text_of(params, "cwd").unwrap_or(&self.cwd);
        let answer = self
            .executor
            .call(
                "shell",
                json!({ "command": line, "cwd": cwd, "background": true }),
            )
            .await?;
        let process_id = answer
            .get("process_id")
            .and_then(Json::as_str)
            .ok_or_else(|| ClientError::failed("the executor started no process"))?
            .to_owned();
        let limit = params
            .get("outputByteLimit")
            .and_then(Json::as_u64)
            .and_then(|limit| usize::try_from(limit).ok())
            .unwrap_or(DEFAULT_OUTPUT_LIMIT);
        self.terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(
                process_id.clone(),
                Terminal {
                    process_id: process_id.clone(),
                    output: String::new(),
                    exit: None,
                    limit,
                    truncated: false,
                },
            );
        Ok(json!({ "terminalId": process_id }))
    }

    fn known(&self, params: &Json) -> Result<String, ClientError> {
        let id = text_of(params, "terminalId")
            .ok_or_else(|| ClientError::invalid("a terminal is named"))?;
        if self
            .terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .contains_key(id)
        {
            Ok(id.to_owned())
        } else {
            Err(ClientError::invalid(format!("there is no terminal {id}")))
        }
    }

    /// Reads what the process printed since the last read, waiting up to `wait_ms` for more.
    async fn pull(&self, id: &str, wait_ms: i64) -> Result<(), ClientError> {
        let process_id = {
            let terminals = self
                .terminals
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            terminals
                .get(id)
                .map(|terminal| terminal.process_id.clone())
                .ok_or_else(|| ClientError::invalid(format!("there is no terminal {id}")))?
        };
        let answer = self
            .executor
            .call(
                "shell_output",
                json!({ "process_id": process_id, "wait_ms": wait_ms }),
            )
            .await?;
        let mut terminals = self
            .terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(terminal) = terminals.get_mut(id) else {
            return Ok(());
        };
        if let Some(text) = answer.get("output").and_then(Json::as_str) {
            terminal.output.push_str(text);
            if terminal.output.len() > terminal.limit {
                let mut cut = terminal.output.len() - terminal.limit;
                while !terminal.output.is_char_boundary(cut) {
                    cut += 1;
                }
                terminal.output.drain(..cut);
                terminal.truncated = true;
            }
        }
        if answer.get("running").and_then(Json::as_bool) == Some(false) {
            terminal.exit = Some(Exit {
                code: answer.get("exit_code").and_then(Json::as_i64),
            });
        }
        Ok(())
    }

    fn view(&self, id: &str) -> Json {
        let terminals = self
            .terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let Some(terminal) = terminals.get(id) else {
            return Json::Null;
        };
        let mut view = json!({ "output": terminal.output, "truncated": terminal.truncated });
        if let Some(exit) = terminal.exit {
            view["exitStatus"] = json!({ "exitCode": exit.code, "signal": null });
        }
        view
    }

    async fn output(&self, params: &Json) -> Result<Json, ClientError> {
        let id = self.known(params)?;
        self.pull(&id, 0).await?;
        Ok(self.view(&id))
    }

    async fn wait(&self, params: &Json) -> Result<Json, ClientError> {
        let id = self.known(params)?;
        loop {
            self.pull(&id, WAIT_STEP_MS).await?;
            let exit = self
                .terminals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get(&id)
                .and_then(|terminal| terminal.exit);
            if let Some(exit) = exit {
                return Ok(json!({ "exitCode": exit.code, "signal": null }));
            }
        }
    }

    async fn kill(&self, params: &Json) -> Result<Json, ClientError> {
        let id = self.known(params)?;
        let process_id = self
            .terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .map(|terminal| terminal.process_id.clone());
        if let Some(process_id) = process_id {
            let answer = self
                .executor
                .call("shell_stop", json!({ "process_id": process_id }))
                .await?;
            if let Some(terminal) = self
                .terminals
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .get_mut(&id)
            {
                terminal.exit = Some(Exit {
                    code: answer.get("exit_code").and_then(Json::as_i64),
                });
            }
        }
        Ok(json!({}))
    }

    async fn release(&self, params: &Json) -> Result<Json, ClientError> {
        let id = self.known(params)?;
        let running = self
            .terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&id)
            .is_some_and(|terminal| terminal.exit.is_none());
        if running {
            self.kill(params).await?;
        }
        self.terminals
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&id);
        Ok(json!({}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_with_arguments_is_quoted_word_by_word_and_a_whole_line_is_left_as_it_is() {
        assert_eq!(command_line("ls -la | head", &[]), "ls -la | head");
        assert_eq!(
            command_line("git", &["commit".into(), "-m".into(), "it's done".into()]),
            "git commit -m 'it'\\''s done'"
        );
        assert_eq!(command_line("echo", &["".into()]), "echo ''");
    }

    #[test]
    fn only_the_clients_own_methods_are_carried_out() {
        assert!(is_client_method("fs/read_text_file"));
        assert!(is_client_method("terminal/create"));
        assert!(!is_client_method("session/request_permission"));
        assert!(!is_client_method("_orch/steer"));
    }
}
