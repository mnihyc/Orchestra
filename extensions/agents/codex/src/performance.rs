//! Codex request boundaries and FIFO token reports, without provider-specific timing arithmetic.

use orch_agent_contract::events::UsageRequest;
use orch_agent_contract::requests::RequestTokens;
use orch_types::{Json, Time};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Debug, Clone)]
struct Open {
    request: UsageRequest,
    last_output: Option<Time>,
    reasoning_streamed: bool,
}

/// Observes the stable app-server surface; missing boundaries remain absent.
#[derive(Debug, Default)]
pub struct Requests {
    main_thread: Option<String>,
    current: HashMap<String, Open>,
    ended: HashMap<String, VecDeque<Open>>,
    awaiting_end: HashMap<String, Open>,
    serial: u64,
    tools: HashSet<(String, String)>,
}

fn text<'a>(value: &'a Json, key: &str) -> &'a str {
    value.get(key).and_then(Json::as_str).unwrap_or_default()
}
// Provider-side web search stays inside one response and its one token report.
fn tool(item: &Json) -> bool {
    matches!(
        text(item, "type"),
        "commandExecution"
            | "fileChange"
            | "mcpToolCall"
            | "dynamicToolCall"
            | "collabAgentToolCall"
    )
}

impl Requests {
    /// Input or a completed tool result is given to the harness; no duration is computed here.
    pub fn asked(&mut self, thread: &str, at: Time) {
        let subagent = self
            .main_thread
            .get_or_insert_with(|| thread.to_owned())
            .as_str()
            != thread;
        self.serial += 1;
        self.current.insert(
            thread.to_owned(),
            Open {
                request: UsageRequest {
                    id: format!("{thread}:{}", self.serial),
                    subagent,
                    asked: Some(at),
                    ..UsageRequest::default()
                },
                last_output: None,
                reasoning_streamed: false,
            },
        );
    }

    /// Current asked/first-output observations, before counts arrive.
    pub fn running(&self, thread: &str) -> Option<UsageRequest> {
        self.current.get(thread).map(|open| open.request.clone())
    }

    /// A harness failure ends the current attempt without inventing its last-output timestamp.
    pub fn abort(&mut self, thread: &str, code: &str, message: Option<&str>) -> Vec<UsageRequest> {
        let mut reports = Vec::new();
        for mut open in self.ended.remove(thread).unwrap_or_default() {
            open.request.finish("done", None, None);
            reports.push(open.request);
        }
        if let Some(mut open) = self.awaiting_end.remove(thread) {
            open.request.finish(code, message, None);
            reports.push(open.request);
        }
        if let Some(mut open) = self.current.remove(thread) {
            open.request.end = open.last_output;
            open.request.finish(code, message, None);
            reports.push(open.request);
        }
        self.tools.retain(|(owner, _)| owner != thread);
        reports
    }

    /// The containing turn/session also owns all child-stream observations.
    pub fn abort_all(&mut self, code: &str, message: Option<&str>) -> Vec<UsageRequest> {
        let mut threads: Vec<_> = self
            .current
            .keys()
            .chain(self.ended.keys())
            .chain(self.awaiting_end.keys())
            .cloned()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        threads.sort();
        threads
            .into_iter()
            .flat_map(|thread| self.abort(&thread, code, message))
            .collect()
    }

    /// Events and later last-token counts are paired in request order, independently of provider kind.
    pub fn observe(
        &mut self,
        method: &str,
        params: &Json,
        at: Time,
        outcome: Option<(&str, Option<&str>)>,
    ) -> Vec<UsageRequest> {
        let thread = text(params, "threadId");
        let item = params.get("item").unwrap_or(&Json::Null);
        if method == "error" {
            let error = crate::failure::classify(
                params.get("error").unwrap_or(params),
                &crate::failure::Notes::default(),
                None,
                at.get(),
                &chrono::Utc,
            );
            let mut reports = self.abort(thread, &error.code, Some(&error.message));
            for request in &mut reports {
                if let Some(orch_agent_contract::requests::RequestOutcome::Error { said, .. }) =
                    &mut request.outcome
                {
                    *said = error.said.clone();
                }
            }
            if params.get("willRetry").and_then(Json::as_bool) == Some(true) {
                self.asked(thread, at);
                reports.extend(self.running(thread));
            }
            return reports;
        }
        if method == "turn/started" {
            return self.running(thread).into_iter().collect();
        }
        if matches!(
            method,
            "item/agentMessage/delta"
                | "item/reasoning/textDelta"
                | "item/reasoning/summaryTextDelta"
        ) && !text(params, "delta").is_empty()
        {
            if let Some(open) = self.current.get_mut(thread) {
                let first = open.request.first_output.is_none();
                open.request.first_output.get_or_insert(at);
                open.request.output_streamed = Some(true);
                open.last_output = Some(at);
                if method.starts_with("item/reasoning/") {
                    open.reasoning_streamed = true;
                }
                if first {
                    return vec![open.request.clone()];
                }
            }
        } else if method == "item/started" && tool(item) {
            self.tools
                .insert((thread.to_owned(), text(item, "id").to_owned()));
            if let Some(mut open) = self.awaiting_end.remove(thread) {
                open.request.end = Some(at);
                open.request.finish("done", None, None);
                return vec![open.request];
            }
            if let Some(mut open) = self.current.remove(thread) {
                // A model's tool call is complete when the tool begins; tool execution follows this boundary.
                open.last_output = Some(at);
                open.request.end = open.last_output;
                self.ended
                    .entry(thread.to_owned())
                    .or_default()
                    .push_back(open);
            }
        } else if method == "item/completed" && tool(item) {
            self.tools
                .remove(&(thread.to_owned(), text(item, "id").to_owned()));
            if !self.tools.iter().any(|(owner, _)| owner == thread)
                && !self.current.contains_key(thread)
            {
                self.asked(thread, at);
                return self.running(thread).into_iter().collect();
            }
        } else if method == "thread/tokenUsage/updated" {
            let pending = self.ended.entry(thread.to_owned()).or_default().pop_front();
            let needs_outcome = pending.is_none() && self.current.contains_key(thread);
            let mut open = pending
                .or_else(|| self.current.remove(thread))
                .unwrap_or_else(|| {
                    self.serial += 1;
                    Open {
                        request: UsageRequest {
                            id: format!("{thread}:{}", self.serial),
                            subagent: self
                                .main_thread
                                .as_deref()
                                .is_some_and(|main| main != thread),
                            ..UsageRequest::default()
                        },
                        last_output: None,
                        reasoning_streamed: false,
                    }
                });
            if open.request.end.is_none() {
                open.request.end = open.last_output;
            }
            let counts = params.pointer("/tokenUsage/last").unwrap_or(&Json::Null);
            let n = |name: &str| counts.get(name).and_then(Json::as_i64);
            open.request.tokens = RequestTokens {
                input_tokens: n("inputTokens"),
                cached_input_tokens: n("cachedInputTokens"),
                cache_write_tokens: n("cacheWriteInputTokens"),
                output_tokens: n("outputTokens"),
                reasoning_tokens: n("reasoningOutputTokens"),
                total_tokens: n("totalTokens"),
            };
            open.request.reasoning_hidden =
                open.request.tokens.reasoning_tokens.is_some_and(|n| n > 0)
                    && !open.reasoning_streamed;
            // The final request's counts can precede turn/completed, which supplies
            // its outcome. A tool boundary already proves the preceding request ended.
            if needs_outcome {
                self.awaiting_end.insert(thread.to_owned(), open);
                return Vec::new();
            }
            open.request.finish("done", None, None);
            return vec![open.request];
        } else if method == "turn/completed" {
            let mut completed = Vec::new();
            for mut open in self.ended.remove(thread).unwrap_or_default() {
                // A version that omitted its later counts still ends explicitly,
                // with nullable counts and no fabricated speed.
                open.request.finish("done", None, None);
                completed.push(open.request);
            }
            if let Some(mut open) = self.awaiting_end.remove(thread) {
                let (code, message) = outcome.unwrap_or(("done", None));
                open.request.finish(code, message, None);
                completed.push(open.request);
            }
            if let Some(mut open) = self.current.remove(thread) {
                open.request.end = open.last_output;
                let (code, message) = outcome.unwrap_or(("done", None));
                open.request.finish(code, message, None);
                completed.push(open.request);
            }
            return completed;
        }
        Vec::new()
    }
}
