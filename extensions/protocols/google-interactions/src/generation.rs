//! Stateless request history and terminally validated native steps.
use orch_model_contract::credentials::WireError;
use orch_model_contract::generation::{
    GenerationDelta, GenerationInput, GenerationMessage, GenerationOutput, GenerationPart,
    GenerationRole, GenerationStop, GenerationStream,
};
use orch_model_contract::usage::SseParser;
use orch_types::Json;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};

fn bad(message: impl Into<String>) -> WireError {
    WireError::Internal(message.into())
}

pub fn request(model: &str, input: &GenerationInput, stream: bool) -> Option<Json> {
    let mut steps = Vec::new();
    for message in &input.messages {
        if message.role == GenerationRole::Assistant && !message.continuation.is_empty() {
            steps.extend(message.continuation.clone());
            continue;
        }
        let mut content = Vec::new();
        let flush = |content: &mut Vec<Json>, steps: &mut Vec<Json>| {
            if !content.is_empty() {
                steps.push(json!({"type":if message.role == GenerationRole::Assistant {"model_output"} else {"user_input"},"content":std::mem::take(content)}));
            }
        };
        for part in &message.content {
            match part {
                GenerationPart::Text { text } => content.push(json!({"type":"text","text":text})),
                GenerationPart::Image {
                    media_type, data, ..
                } => content.push(json!({"type":"image","mime_type":media_type,"data":data})),
                GenerationPart::ImageRef { .. } => return None,
                GenerationPart::ToolCall {
                    id,
                    name,
                    arguments,
                } => {
                    flush(&mut content, &mut steps);
                    steps.push(
                        json!({"type":"function_call","id":id,"name":name,"arguments":arguments}),
                    );
                }
                GenerationPart::ToolResult {
                    call_id,
                    content: result,
                    is_error,
                    ..
                } => {
                    flush(&mut content, &mut steps);
                    steps.push(json!({"type":"function_result","call_id":call_id,"result":result,"is_error":is_error}));
                }
            }
        }
        flush(&mut content, &mut steps);
    }
    Some(
        json!({"model":model,"system_instruction":input.system,"input":steps,"stream":stream,"store":false,
        "generation_config":{"max_output_tokens":input.max_output_tokens},
        "tools":input.tools.iter().map(|tool| json!({"type":"function","name":tool.name,"description":tool.description,"parameters":tool.parameters})).collect::<Vec<_>>()}),
    )
}

pub fn output(reply: Json) -> Result<GenerationOutput, WireError> {
    if reply.get("error").is_some_and(|error| !error.is_null()) {
        return Err(bad(reply["error"].to_string()));
    }
    let status = reply
        .get("status")
        .and_then(Json::as_str)
        .ok_or_else(|| bad("Gemini did not report a terminal status"))?;
    let mut content = Vec::new();
    let steps = reply
        .get("steps")
        .and_then(Json::as_array)
        .cloned()
        .unwrap_or_default();
    for step in &steps {
        match step.get("type").and_then(Json::as_str) {
            Some("model_output") => {
                for part in step
                    .get("content")
                    .and_then(Json::as_array)
                    .into_iter()
                    .flatten()
                {
                    if part["type"] == "text"
                        && let Some(text) = part.get("text").and_then(Json::as_str)
                    {
                        content.push(GenerationPart::Text { text: text.into() });
                    }
                }
            }
            Some("function_call") => {
                let text = |key| {
                    step.get(key)
                        .and_then(Json::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned)
                        .ok_or_else(|| bad("Gemini returned an unfinished function call"))
                };
                let arguments = match step.get("arguments") {
                    Some(Json::String(text)) => serde_json::from_str(text)
                        .map_err(|_| bad("Gemini returned incomplete function arguments"))?,
                    Some(value) => value.clone(),
                    None => return Err(bad("Gemini omitted function arguments")),
                };
                content.push(GenerationPart::ToolCall {
                    id: text("id")?,
                    name: text("name")?,
                    arguments,
                });
            }
            _ => {}
        }
    }
    let has_calls = content
        .iter()
        .any(|part| matches!(part, GenerationPart::ToolCall { .. }));
    let stop = match status {
        "requires_action" if has_calls => GenerationStop::Tools,
        "completed" if has_calls => GenerationStop::Tools,
        "completed" => GenerationStop::Complete,
        "incomplete" | "requires_continuation" if !has_calls => GenerationStop::Limit,
        _ => return Err(bad(format!("Gemini did not complete this step ({status})"))),
    };
    let answer = GenerationOutput {
        native_result: json!({"response":reply}),
        message: GenerationMessage {
            role: GenerationRole::Assistant,
            content,
            continuation: steps,
        },
        stop,
    };
    answer.validate()?;
    Ok(answer)
}

#[derive(Default)]
pub struct GoogleStream {
    parser: SseParser,
    steps: BTreeMap<u64, Json>,
    stopped: BTreeSet<u64>,
    terminal: Option<Json>,
    frames: Vec<Json>,
}

impl GenerationStream for GoogleStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<GenerationDelta>, WireError> {
        let mut deltas = Vec::new();
        for event in self.parser.push(bytes) {
            let value: Json = serde_json::from_str(&event.data)
                .map_err(|_| bad("Gemini sent an unreadable stream event"))?;
            let kind = value
                .get("event_type")
                .and_then(Json::as_str)
                .or(event.event.as_deref());
            match kind {
                Some("step.start") => {
                    let index = value
                        .get("index")
                        .and_then(Json::as_u64)
                        .ok_or_else(|| bad("Gemini omitted the step index"))?;
                    self.steps.insert(index, value["step"].clone());
                }
                Some("step.delta") => {
                    let index = value
                        .get("index")
                        .and_then(Json::as_u64)
                        .ok_or_else(|| bad("Gemini omitted the step index"))?;
                    let step = self
                        .steps
                        .get_mut(&index)
                        .ok_or_else(|| bad("Gemini changed a step before starting it"))?;
                    let delta = &value["delta"];
                    if delta["type"] == "text"
                        && let Some(text) = delta.get("text").and_then(Json::as_str)
                    {
                        if step.get("content").is_none() {
                            step["content"] = json!([]);
                        }
                        let parts = step
                            .get_mut("content")
                            .and_then(Json::as_array_mut)
                            .ok_or_else(|| bad("Gemini returned malformed text"))?;
                        if let Some(last) = parts.last_mut().filter(|part| part["type"] == "text") {
                            let old = last.get("text").and_then(Json::as_str).unwrap_or_default();
                            last["text"] = json!(format!("{old}{text}"));
                        } else {
                            parts.push(json!({"type":"text","text":text}));
                        }
                        deltas.push(GenerationDelta::Text { text: text.into() });
                    } else if let (Some(fields), Some(target)) =
                        (delta.as_object(), step.as_object_mut())
                    {
                        for (key, part) in fields {
                            if let Some(old) = target.get_mut(key) {
                                if key == "arguments" {
                                    match (old, part) {
                                        (Json::Object(old), Json::Object(extra)) => {
                                            old.extend(extra.clone());
                                            continue;
                                        }
                                        (Json::String(old), Json::String(extra)) => {
                                            old.push_str(extra);
                                            continue;
                                        }
                                        _ => {}
                                    }
                                } else if key == "result"
                                    && let (Json::Array(old), Json::Array(extra)) = (old, part)
                                {
                                    old.extend(extra.clone());
                                    continue;
                                }
                            }
                            target.insert(key.clone(), part.clone());
                        }
                    }
                }
                Some("step.stop") => {
                    if let Some(index) = value.get("index").and_then(Json::as_u64) {
                        self.stopped.insert(index);
                    }
                }
                Some("interaction.completed") => self.terminal = Some(value["interaction"].clone()),
                Some("error") => return Err(bad(value.to_string())),
                _ => {}
            }
            self.frames.push(value);
        }
        Ok(deltas)
    }
    fn finish(self: Box<Self>) -> Result<GenerationOutput, WireError> {
        let mut reply = self
            .terminal
            .ok_or_else(|| bad("Gemini's stream ended before its terminal event"))?;
        if reply.get("steps").is_none() {
            if self.steps.iter().any(|(index, step)| {
                step["type"] == "function_call" && !self.stopped.contains(index)
            }) {
                return Err(bad("Gemini's stream left a function call unfinished"));
            }
            reply["steps"] = Json::Array(self.steps.into_values().collect());
        }
        let mut answer = output(reply)?;
        answer.native_result["frames"] = Json::Array(self.frames);
        Ok(answer)
    }
}
