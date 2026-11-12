//! Messages dialogue and tools, including signed thinking continuation.

use bytes::Bytes;
use http::Method;
use orch_model_contract::credentials::WireError;
use orch_model_contract::generation::{
    GenerationDelta, GenerationInput, GenerationMessage, GenerationOutput, GenerationPart,
    GenerationRole, GenerationStop, GenerationStream,
};
use orch_model_contract::usage::SseParser;
use serde_json::{Value, json};
use std::collections::BTreeMap;

fn bad() -> WireError {
    WireError::Internal("the message contains no completed dialogue step".to_owned())
}

/// Tools and answers remain structured content, separate from the system instruction.
pub fn request(
    model: &str,
    input: &GenerationInput,
    stream: bool,
) -> (Method, &'static str, Bytes) {
    let mut messages = Vec::<Value>::new();
    for message in &input.messages {
        let mut content = message.continuation.clone();
        content.extend(message.content.iter().map(|part| match part {
            GenerationPart::Image {media_type,data,..}=>json!({"type":"image","source":{"type":"base64","media_type":media_type,"data":data}}),
            GenerationPart::ImageRef {name,url,..}=>json!({"type":"text","text":format!("[{name}]({url})")}),
            GenerationPart::Text {text}=>json!({"type":"text","text":text}),
            GenerationPart::ToolCall {id,name,arguments}=>json!({"type":"tool_use","id":id,"name":name,"input":arguments}),
            GenerationPart::ToolResult {call_id,content,is_error,..}=>json!({"type":"tool_result","tool_use_id":call_id,"content":content,"is_error":is_error}),
        }));
        let role = if message.role == GenerationRole::Assistant {
            "assistant"
        } else {
            "user"
        };
        if let Some(last) = messages.last_mut().filter(|last| last["role"] == role) {
            if let Some(parts) = last.get_mut("content").and_then(Value::as_array_mut) {
                parts.extend(content);
            }
        } else {
            messages.push(json!({"role":role,"content":content}));
        }
    }
    let tools:Vec<_>=input.tools.iter().map(|tool|json!({"name":tool.name,"description":tool.description,"input_schema":tool.parameters})).collect();
    (Method::POST,"/v1/messages",Bytes::from(json!({"model":model,"system":input.system,"messages":messages,"tools":tools,"max_tokens":input.max_output_tokens,"stream":stream}).to_string()))
}

/// Completed tool inputs are validated before the host can act on them.
pub fn output(body: &[u8]) -> Result<GenerationOutput, WireError> {
    let reply: Value = serde_json::from_slice(body).map_err(|_| bad())?;
    if reply["type"] == "error" || reply.get("error").is_some_and(|error| !error.is_null()) {
        return Err(bad());
    }
    let stop = match reply.get("stop_reason").and_then(Value::as_str) {
        Some("end_turn" | "stop_sequence") => GenerationStop::Complete,
        Some("tool_use") => GenerationStop::Tools,
        Some("max_tokens" | "model_context_window_exceeded") => GenerationStop::Limit,
        Some("refusal") => GenerationStop::Refused,
        _ => return Err(bad()),
    };
    let mut content = Vec::new();
    let mut continuation = Vec::new();
    for block in reply
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
    {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => content.push(GenerationPart::Text {
                text: block
                    .get("text")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
            }),
            Some("tool_use") => content.push(GenerationPart::ToolCall {
                id: block
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                name: block
                    .get("name")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                arguments: block
                    .get("input")
                    .filter(|value| value.is_object())
                    .ok_or_else(bad)?
                    .clone(),
            }),
            Some("thinking" | "redacted_thinking") => continuation.push(block.clone()),
            _ => {}
        }
    }
    let answer = GenerationOutput {
        native_result: json!({"response":reply,"frames":[]}),
        message: GenerationMessage {
            role: GenerationRole::Assistant,
            content,
            continuation,
        },
        stop,
    };
    answer.validate()?;
    Ok(answer)
}

#[derive(Default)]
struct Block {
    value: Value,
    arguments: String,
    ended: bool,
}

/// A stream is complete only after message_stop and every content block's end.
#[derive(Default)]
pub struct MessagesStream {
    parser: SseParser,
    blocks: BTreeMap<u64, Block>,
    reason: Option<String>,
    started: bool,
    done: bool,
    native_frames: Vec<Value>,
}

impl GenerationStream for MessagesStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<GenerationDelta>, WireError> {
        let mut deltas = Vec::new();
        for event in self.parser.push(bytes) {
            let value: Value = serde_json::from_str(&event.data).map_err(|_| bad())?;
            self.native_frames.push(value.clone());
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .or(event.event.as_deref());
            if self.done && kind != Some("ping") {
                return Err(bad());
            }
            match kind {
                Some("message_start") => {
                    if self.started {
                        return Err(bad());
                    }
                    self.started = true;
                }
                Some("content_block_start") if self.started => {
                    let index = value.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
                    if self.blocks.contains_key(&index) {
                        return Err(bad());
                    }
                    self.blocks.insert(
                        index,
                        Block {
                            value: value.get("content_block").ok_or_else(bad)?.clone(),
                            ..Default::default()
                        },
                    );
                }
                Some("content_block_delta") => {
                    let index = value.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
                    let block = self
                        .blocks
                        .get_mut(&index)
                        .filter(|block| !block.ended)
                        .ok_or_else(bad)?;
                    let delta = value.get("delta").ok_or_else(bad)?;
                    match delta.get("type").and_then(Value::as_str) {
                        Some("text_delta") => {
                            let text = delta.get("text").and_then(Value::as_str).ok_or_else(bad)?;
                            let current = block.value.get_mut("text").ok_or_else(bad)?;
                            *current = Value::String(format!(
                                "{}{text}",
                                current.as_str().ok_or_else(bad)?
                            ));
                            deltas.push(GenerationDelta::Text {
                                text: text.to_owned(),
                            });
                        }
                        Some("input_json_delta") => block.arguments.push_str(
                            delta
                                .get("partial_json")
                                .and_then(Value::as_str)
                                .ok_or_else(bad)?,
                        ),
                        Some("thinking_delta" | "signature_delta") => {
                            let field = if delta["type"] == "thinking_delta" {
                                "thinking"
                            } else {
                                "signature"
                            };
                            let fragment =
                                delta.get(field).and_then(Value::as_str).ok_or_else(bad)?;
                            let current =
                                block.value.get(field).and_then(Value::as_str).unwrap_or("");
                            block.value[field] = json!(format!("{current}{fragment}"));
                        }
                        _ => {}
                    }
                }
                Some("content_block_stop") => {
                    let index = value.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
                    let block = self
                        .blocks
                        .get_mut(&index)
                        .filter(|block| !block.ended)
                        .ok_or_else(bad)?;
                    if matches!(
                        block.value["type"].as_str(),
                        Some("tool_use" | "server_tool_use")
                    ) && !block.arguments.is_empty()
                    {
                        block.value["input"] =
                            serde_json::from_str(&block.arguments).map_err(|_| bad())?;
                    }
                    block.ended = true;
                }
                Some("message_delta") => {
                    self.reason = value
                        .pointer("/delta/stop_reason")
                        .and_then(Value::as_str)
                        .map(str::to_owned)
                }
                Some("message_stop") if self.started => self.done = true,
                Some("error") => return Err(bad()),
                _ => {}
            }
        }
        Ok(deltas)
    }
    fn finish(self: Box<Self>) -> Result<GenerationOutput, WireError> {
        if !self.done || self.blocks.values().any(|block| !block.ended) {
            return Err(bad());
        }
        let content: Vec<_> = self
            .blocks
            .values()
            .map(|block| block.value.clone())
            .collect();
        let mut answer = output(
            &serde_json::to_vec(
                &json!({"type":"message","content":content,"stop_reason":self.reason}),
            )
            .map_err(|_| bad())?,
        )?;
        answer.native_result["frames"] = json!(self.native_frames);
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_server_results_and_partial_query_input_survive_the_generation_reader() {
        let blocks = json!([
            {"type":"server_tool_use","id":"native-search","name":"web_search","input":{"query":"fixture query"}},
            {"type":"web_search_tool_result","tool_use_id":"native-search","content":[{"type":"web_search_result","title":"Actual source","url":"https://source.example.invalid/"}]},
            {"type":"web_fetch_tool_result","tool_use_id":"native-fetch","content":{"type":"web_fetch_result","url":"https://source.example.invalid/","content":{"type":"document","source":{"type":"text","data":"Reported page"}}}},
            {"type":"text","text":"Reported answer"}
        ]);
        let whole = output(
            &serde_json::to_vec(
                &json!({"type":"message","stop_reason":"end_turn","content":blocks}),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(whole.native_result["response"]["content"], blocks);
        assert_eq!(whole.stop, GenerationStop::Complete);
        assert_eq!(whole.message.content.len(), 1);
        assert!(whole.message.continuation.is_empty());
        let mut frames = vec![json!({"type":"message_start","message":{"model":"fixture"}})];
        for (index, block) in blocks.as_array().unwrap().iter().enumerate() {
            let mut opened = block.clone();
            if index == 0 {
                opened["input"] = json!({});
            }
            frames.push(json!({"type":"content_block_start","index":index,"content_block":opened}));
            if index == 0 {
                frames.push(json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{\"query\":\"fixture query\"}"}}));
            }
            frames.push(json!({"type":"content_block_stop","index":index}));
        }
        frames.push(json!({"type":"message_delta","delta":{"stop_reason":"end_turn"}}));
        frames.push(json!({"type":"message_stop"}));
        let mut reader = MessagesStream::default();
        for frame in &frames {
            reader
                .push(format!("data: {frame}\n\n").as_bytes())
                .unwrap();
        }
        let streamed = Box::new(reader).finish().unwrap();
        assert_eq!(streamed.message, whole.message);
        assert_eq!(streamed.native_result["response"]["content"], blocks);
        assert_eq!(streamed.native_result["frames"], json!(frames));
    }

    #[test]
    fn messages_carries_an_image_as_image_content() {
        let input = GenerationInput {
            system: "Read the image".into(),
            messages: vec![GenerationMessage {
                role: GenerationRole::User,
                content: vec![GenerationPart::Image {
                    media_type: "image/png".into(),
                    data: "c3ludGhldGlj".into(),
                    name: "example.png".into(),
                }],
                continuation: vec![],
            }],
            tools: vec![],
            max_output_tokens: 64,
        };
        let (_, _, body) = request("model", &input, true);
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["messages"][0]["content"][0]["type"], "image");
        assert_eq!(
            body["messages"][0]["content"][0]["source"]["data"],
            "c3ludGhldGlj"
        );
    }

    #[test]
    fn messages_tools_keep_signed_thinking_and_untrusted_results_separate() {
        let answer=output(br#"{"content":[{"type":"thinking","thinking":"reason","signature":"signature"},{"type":"tool_use","id":"call","name":"read","input":{"operation":"ThreadGet"}}],"stop_reason":"tool_use"}"#).unwrap();
        let input = GenerationInput {
            system: "Read only".into(),
            messages: vec![
                answer.message,
                GenerationMessage {
                    role: GenerationRole::User,
                    content: vec![GenerationPart::ToolResult {
                        call_id: "call".into(),
                        content: "untrusted".into(),
                        is_error: false,
                        witness: Some(orch_types::Id::from_u128(42)),
                    }],
                    continuation: vec![],
                },
            ],
            tools: vec![],
            max_output_tokens: 64,
        };
        let (_, _, wire) = request("model", &input, true);
        let wire: Value = serde_json::from_slice(&wire).unwrap();
        assert!(!wire.to_string().contains("witness"));
        assert!(
            !wire
                .to_string()
                .contains(&orch_types::Id::from_u128(42).to_string())
        );
        assert_eq!(wire["messages"][0]["content"][0]["signature"], "signature");
        assert_eq!(wire["messages"][1]["content"][0]["type"], "tool_result");
        assert_eq!(wire["system"], "Read only");
    }

    #[test]
    fn messages_stream_refuses_truncated_tools() {
        let mut reader = MessagesStream::default();
        reader.push(b"event: message_start\ndata: {\"type\":\"message_start\"}\n\nevent: content_block_start\ndata: {\"type\":\"content_block_start\",\"index\":0,\"content_block\":{\"type\":\"tool_use\",\"id\":\"call\",\"name\":\"read\",\"input\":{}}}\n\n").unwrap();
        assert!(Box::new(reader).finish().is_err());
        let answer=output(br#"{"content":[{"type":"tool_use","id":"call","name":"read","input":{}}],"stop_reason":"max_tokens"}"#);
        assert!(answer.is_err());
    }
}
