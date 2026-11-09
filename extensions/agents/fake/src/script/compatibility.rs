//! Upgrade scenarios stay inside the scripted adapter's interruptible, persisted event path.
use super::*;
use orch_agent_contract::events::{MarkerPayload, SessionEvent, UsageMeasurement};
use serde::Deserialize;
use serde_json::json;

/// Typed passive native observation. Identity, turn and observation time come from the real runner.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    change: String,
    #[serde(default)]
    subject: Option<String>,
    value: Json,
    #[serde(default)]
    previous: UsageMeasurement<Json>,
    #[serde(default)]
    expected: UsageMeasurement<Json>,
    #[serde(default)]
    next_turn: bool,
}

impl Observation {
    fn marker(&self, turn: Option<orch_types::Id>, at: Time) -> MarkerPayload {
        if self.change == "compaction" {
            return MarkerPayload::Compacted {
                duration_ms: self.value.get("duration_ms").and_then(Json::as_i64),
            };
        }
        MarkerPayload::HarnessChanged(Box::new(orch_api_contract::threads::MarkerHarnessChanged {
            program: "Fake".into(),
            change: self.change.clone(),
            subject: self.subject.clone(),
            previous: self.previous.clone(),
            value: self.value.clone(),
            expected: self.expected.clone(),
            changed: self
                .previous
                .value()
                .is_some_and(|value| value != &self.value),
            diverged: self
                .expected
                .value()
                .is_some_and(|value| value != &self.value),
            next_turn: self.next_turn,
            turn_id: turn,
            reported_at: at,
        }))
    }
}

pub(crate) async fn idle(
    shared: &Arc<Shared>,
    thread: orch_types::Id,
    value: Json,
) -> Result<(), orch_agent_contract::adapter::AdapterError> {
    use orch_agent_contract::adapter::AdapterError;
    use orch_agent_contract::events::{ItemCompleted, ItemKind};
    let live = shared.session(thread)?;
    let observation: Observation =
        serde_json::from_value(value).map_err(|error| AdapterError::Other {
            code: "invalid_script".into(),
            message: error.to_string(),
            retryable: false,
        })?;
    if observation.change.trim().is_empty() {
        return Err(AdapterError::Other {
            code: "invalid_script".into(),
            message: "an idle observation needs a change".into(),
            retryable: false,
        });
    }
    let (reference, payload) = {
        // Match start_turn's turn-then-file lock order; no new turn can race the idle record.
        let active = lock(&live.turn);
        let snapshot = live.snapshot();
        if active.as_ref().is_some_and(|turn| {
            snapshot
                .turns
                .last()
                .is_none_or(|last| last.turn_id != turn.turn_id)
        }) {
            return Err(AdapterError::Other {
                code: "turn_running".into(),
                message: "an idle observation cannot replace an active turn".into(),
                retryable: false,
            });
        }
        let now = shared.clock.time();
        let payload = serde_json::to_value(observation.marker(None, now))
            .map_err(|error| AdapterError::Internal(error.to_string()))?;
        let reference = format!("idle_item_{}", live.next_counter(Counter::Item));
        let item = crate::sessions::ItemRecord {
            native_ref: reference.clone(),
            turn_ordinal: live.turns_so_far(),
            stream: MAIN_STREAM.into(),
            parent_native_ref: None,
            kind: ItemKind::Marker,
            status: ItemStatus::Completed,
            payload: payload.clone(),
            output_blob: None,
        };
        live.update(|file| {
            file.items.push(item);
            file.updated_at = now;
        });
        (reference, payload)
    };
    shared
        .sink
        .session(
            live.thread_id,
            live.segment_id,
            SessionEvent::ItemCompleted(ItemCompleted {
                native_ref: reference,
                status: ItemStatus::Completed,
                payload,
                output_blob: None,
            }),
        )
        .await
        .map_err(|error| AdapterError::Internal(error.to_string()))
}

/// Native family controls supplement the existing run/edit/MCP/subagent/plan/lifecycle methods.
#[derive(Debug, Clone, PartialEq)]
pub enum Directive {
    Tool(Box<ToolPayload>),
    Request(Box<HarnessRequest>),
    Observe(Box<Observation>),
    Unknown(String),
    Diverged(String),
    Reroute(String),
    Fault(Box<TurnError>),
}

pub(super) fn parse(line: usize, keyword: &str, rest: &str) -> Result<Directive, ScriptError> {
    let json = || {
        serde_json::from_str::<Json>(rest)
            .map_err(|error| fault(line, format!("{keyword}: {error}: {rest}")))
    };
    let text = || {
        if rest.trim().is_empty() {
            Err(fault(line, format!("{keyword} needs a name")))
        } else {
            Ok(text_of(rest))
        }
    };
    Ok(match keyword {
        "tool" => Directive::Tool(Box::new(
            serde_json::from_value(json()?).map_err(|error| fault(line, error.to_string()))?,
        )),
        "request" => {
            let value = json()?;
            let kind = value
                .get("request_kind")
                .and_then(Json::as_str)
                .ok_or_else(|| fault(line, "request needs a native request_kind"))?;
            let request = if serde_json::from_value::<HarnessRequestKind>(Json::from(kind)).is_err()
            {
                let options = serde_json::from_value(
                    value.get("options").cloned().unwrap_or_else(|| json!([])),
                )
                .map_err(|error| fault(line, format!("request options: {error}")))?;
                orch_agent_contract::observations::generic_approval(
                    "Unrecognized fake approval".into(),
                    value,
                    options,
                )
            } else {
                serde_json::from_value(value)
                    .map_err(|error| fault(line, format!("request: {error}")))?
            };
            Directive::Request(Box::new(request))
        }
        "observe" => {
            let observation: Observation = serde_json::from_value(json()?)
                .map_err(|error| fault(line, format!("observe: {error}")))?;
            if observation.change.trim().is_empty() {
                return Err(fault(line, "observe needs a change"));
            }
            Directive::Observe(Box::new(observation))
        }
        "unknown" => Directive::Unknown(text()?),
        "diverged" => Directive::Diverged(text()?),
        "reroute" => Directive::Reroute(text()?),
        "fault" => {
            let error: TurnError = serde_json::from_value(json()?)
                .map_err(|error| fault(line, format!("fault: {error}")))?;
            if error.code == "harness_diverged" || error.code == "model_rerouted" {
                return Err(fault(
                    line,
                    "use diverged or reroute for a nonterminal observation",
                ));
            }
            Directive::Fault(Box::new(error))
        }
        _ => return Err(fault(line, "unknown compatibility directive")),
    })
}

impl Runner {
    pub(super) async fn compatibility(
        &mut self,
        directive: &Directive,
        stream: &str,
        parent: Option<&str>,
    ) -> Result<(), Flow> {
        match directive {
            Directive::Tool(tool) => {
                let value = serde_json::to_value(tool).map_err(|error| {
                    Flow::failed(turn_error("harness_error", &error.to_string(), false))
                })?;
                let name = value["kind"].as_str().unwrap_or("other");
                let reference = self.emitter.next_item_ref();
                self.tool_item(
                    &reference,
                    stream,
                    parent,
                    tool.as_ref().clone(),
                    name,
                    name,
                    &value,
                )
                .await;
                self.end_tool(
                    &reference,
                    ItemStatus::Completed,
                    tool.as_ref().clone(),
                    None,
                    false,
                    None,
                )
                .await;
                Ok(())
            }
            Directive::Request(request) => {
                let decision = self
                    .ask("Scripted native request", request.as_ref().clone())
                    .await?;
                if matches!(decision,ApprovalDecision::Option {ref option_id,..} if option_id == orch_agent_contract::observations::CANCEL_UNRECOGNIZED_APPROVAL)
                {
                    Err(Flow::Interrupted)
                } else {
                    Ok(())
                }
            }
            Directive::Unknown(name) => {
                let sequence = self.live.next_counter(Counter::Item);
                let events = orch_agent_contract::observations::unrecognized_event(
                    "Fake",
                    name,
                    sequence,
                    Some(self.turn.turn_id),
                    self.shared.clock.time(),
                )
                .map_err(|error| {
                    Flow::failed(turn_error("harness_error", &error.to_string(), false))
                })?;
                for event in events {
                    match event {
                        SessionEvent::ItemStarted(item) => {
                            self.emitter
                                .item_started(&item.native_ref, stream, parent, item.content)
                                .await
                        }
                        SessionEvent::ItemCompleted(item) => {
                            self.emitter
                                .item_completed(
                                    &item.native_ref,
                                    item.status,
                                    item.payload,
                                    item.output_blob,
                                )
                                .await
                        }
                        _ => {
                            return Err(Flow::failed(turn_error(
                                "harness_error",
                                "unrecognized-event helper returned a different native family",
                                false,
                            )));
                        }
                    }
                }
                Ok(())
            }
            Directive::Diverged(name) => {
                self.emitter
                    .error_item(
                        "harness_diverged",
                        &format!("Fake ignored the managed {name} pin"),
                        false,
                    )
                    .await;
                Ok(())
            }
            Directive::Observe(observation) => {
                let diverged = observation
                    .expected
                    .value()
                    .is_some_and(|value| value != &observation.value);
                let marker = observation.marker(Some(self.turn.turn_id), self.shared.clock.time());
                let reference = self.emitter.next_item_ref();
                self.emitter
                    .item_started(
                        &reference,
                        stream,
                        parent,
                        ItemContent::Marker(marker.clone()),
                    )
                    .await;
                self.emitter
                    .item_completed(
                        &reference,
                        ItemStatus::Completed,
                        serde_json::to_value(marker).map_err(|error| {
                            Flow::failed(turn_error("harness_error", &error.to_string(), false))
                        })?,
                        None,
                    )
                    .await;
                if diverged {
                    self.emitter
                        .error_item(
                            "harness_diverged",
                            &format!("Fake reported a different {}", observation.change),
                            false,
                        )
                        .await;
                }
                if observation.change == "mcp_startup" && observation.value["status"] == "failed" {
                    self.emitter
                        .error_item(
                            "mcp_server_failed",
                            "Fake reported an MCP startup failure",
                            true,
                        )
                        .await;
                }
                Ok(())
            }
            Directive::Reroute(model) => {
                self.emitter.reroute_model(model).await;
                self.turn.model.clone_from(model);
                Ok(())
            }
            Directive::Fault(error) => Err(Flow::Failed(error.clone())),
        }
    }
}
