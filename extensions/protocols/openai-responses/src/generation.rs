//! Responses dialogue and function calls, including stateless reasoning continuation.

use bytes::Bytes;
use http::Method;
use orch_model_contract::credentials::WireError;
use orch_model_contract::generation::{
    GenerationDelta, GenerationInput, GenerationMessage, GenerationOutput, GenerationPart,
    GenerationRole, GenerationStop, GenerationStream,
};
use orch_model_contract::usage::SseParser;
use serde_json::{Value, json};

fn bad() -> WireError {
    WireError::Internal("the response contains no completed dialogue step".to_owned())
}

/// Builds an owned Responses request. No server-side conversation is retained.
pub fn request(
    model: &str,
    input: &GenerationInput,
    stream: bool,
) -> (Method, &'static str, Bytes) {
    let mut messages = Vec::new();
    for message in &input.messages {
        messages.extend(message.continuation.iter().cloned());
        for part in &message.content {
            messages.push(match part {
                GenerationPart::Image {media_type,data,..}=>json!({"role":"user","content":[{"type":"input_image","image_url":format!("data:{media_type};base64,{data}")}]}),
                GenerationPart::ImageRef {name,url,..}=>json!({"role":"user","content":[{"type":"input_text","text":format!("[{name}]({url})")}]}),
                GenerationPart::Text { text } => json!({
                    "role": if message.role == GenerationRole::User { "user" } else { "assistant" },
                    "content": [{"type": if message.role == GenerationRole::User { "input_text" } else { "output_text" }, "text": text}]
                }),
                GenerationPart::ToolCall { id, name, arguments } => json!({
                    "type": "function_call", "call_id": id, "name": name, "arguments": arguments.to_string()
                }),
                GenerationPart::ToolResult { call_id, content, .. } => json!({
                    "type": "function_call_output", "call_id": call_id, "output": content
                }),
            });
        }
    }
    let tools: Vec<_> = input
        .tools
        .iter()
        .map(|tool| {
            json!({
                "type": "function", "name": tool.name, "description": tool.description,
                "parameters": tool.parameters
            })
        })
        .collect();
    (
        Method::POST,
        "/responses",
        Bytes::from(
            json!({
                "model": model, "instructions": input.system, "input": messages, "tools": tools,
                "max_output_tokens": input.max_output_tokens, "stream": stream, "store": false,
                "include": ["reasoning.encrypted_content"]
            })
            .to_string(),
        ),
    )
}

/// Reads only completed function arguments. Truncated calls never reach the tool host.
pub fn output(body: &[u8]) -> Result<GenerationOutput, WireError> {
    let reply: Value = serde_json::from_slice(body).map_err(|_| bad())?;
    if reply.get("error").is_some_and(|error| !error.is_null()) {
        return Err(bad());
    }
    let status = reply
        .get("status")
        .and_then(Value::as_str)
        .ok_or_else(bad)?;
    if !matches!(status, "completed" | "incomplete") {
        return Err(bad());
    }
    let mut content = Vec::new();
    let mut continuation = Vec::new();
    let mut refused = false;
    for item in reply
        .get("output")
        .and_then(Value::as_array)
        .ok_or_else(bad)?
    {
        match item.get("type").and_then(Value::as_str) {
            Some("message") => {
                for part in item
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(bad)?
                {
                    match part.get("type").and_then(Value::as_str) {
                        Some("output_text") => content.push(GenerationPart::Text {
                            text: part
                                .get("text")
                                .and_then(Value::as_str)
                                .ok_or_else(bad)?
                                .to_owned(),
                        }),
                        Some("refusal") => refused = true,
                        _ => {}
                    }
                }
            }
            Some("function_call") if status == "completed" => {
                content.push(GenerationPart::ToolCall {
                    id: item
                        .get("call_id")
                        .and_then(Value::as_str)
                        .ok_or_else(bad)?
                        .to_owned(),
                    name: item
                        .get("name")
                        .and_then(Value::as_str)
                        .ok_or_else(bad)?
                        .to_owned(),
                    arguments: serde_json::from_str(
                        item.get("arguments")
                            .and_then(Value::as_str)
                            .ok_or_else(bad)?,
                    )
                    .map_err(|_| bad())?,
                });
            }
            Some("function_call") => return Err(bad()),
            Some("reasoning") => continuation.push(item.clone()),
            _ => {}
        }
    }
    let tools = content
        .iter()
        .any(|part| matches!(part, GenerationPart::ToolCall { .. }));
    let stop = if refused {
        GenerationStop::Refused
    } else if status == "incomplete" {
        GenerationStop::Limit
    } else if tools {
        GenerationStop::Tools
    } else {
        GenerationStop::Complete
    };
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

/// Incremental Responses reader; the completed event is the authoritative final output.
#[derive(Default)]
pub struct ResponsesStream {
    parser: SseParser,
    answer: Option<GenerationOutput>,
    native_frames: Vec<Value>,
}

impl GenerationStream for ResponsesStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<GenerationDelta>, WireError> {
        let mut deltas = Vec::new();
        for event in self.parser.push(bytes) {
            if event.data == "[DONE]" {
                continue;
            }
            let value: Value = serde_json::from_str(&event.data).map_err(|_| bad())?;
            self.native_frames.push(value.clone());
            let kind = value
                .get("type")
                .and_then(Value::as_str)
                .or(event.event.as_deref());
            match kind {
                Some("response.output_text.delta") if self.answer.is_none() => {
                    deltas.push(GenerationDelta::Text {
                        text: value
                            .get("delta")
                            .and_then(Value::as_str)
                            .ok_or_else(bad)?
                            .to_owned(),
                    });
                }
                Some("response.completed" | "response.incomplete") if self.answer.is_none() => {
                    self.answer = Some(output(
                        &serde_json::to_vec(value.get("response").ok_or_else(bad)?)
                            .map_err(|_| bad())?,
                    )?);
                }
                Some("error" | "response.failed") => return Err(bad()),
                _ => {}
            }
        }
        Ok(deltas)
    }

    fn finish(self: Box<Self>) -> Result<GenerationOutput, WireError> {
        let mut answer = self.answer.ok_or_else(bad)?;
        answer.native_result["frames"] = json!(self.native_frames);
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_search_sources_and_citations_survive_both_complete_and_streamed_generation() {
        let body = json!({"status":"completed","output":[
            {"type":"web_search_call","id":"native-search","status":"completed","action":{"type":"search","queries":["fixture query"],"sources":[{"url":"https://source.example.invalid/","title":"Actual source"}]}},
            {"type":"message","content":[{"type":"output_text","text":"Reported answer","annotations":[{"type":"url_citation","url":"https://source.example.invalid/","title":"Actual source"}]}]}
        ],"future_native_member":{"kept":true}});
        let whole = output(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(whole.native_result["response"], body);
        assert_eq!(whole.stop, GenerationStop::Complete);
        assert!(whole.message.continuation.is_empty());
        assert_eq!(whole.message.content.len(), 1);
        let event = json!({"type":"response.completed","response":body});
        let mut reader = ResponsesStream::default();
        reader
            .push(format!("data: {event}\n\n").as_bytes())
            .unwrap();
        let streamed = Box::new(reader).finish().unwrap();
        assert_eq!(streamed.message, whole.message);
        assert_eq!(
            streamed.native_result["response"],
            whole.native_result["response"]
        );
        assert_eq!(streamed.native_result["frames"], json!([event]));
    }

    #[test]
    fn tool_journal_witnesses_do_not_enter_either_openai_http_body() {
        let witness = "00000000-0000-0000-0000-00000000002a";
        let input = GenerationInput {
            system: "Read only".into(),
            messages: vec![
                GenerationMessage {
                    role: GenerationRole::Assistant,
                    content: vec![GenerationPart::ToolCall {
                        id: "provider-call".into(),
                        name: "read".into(),
                        arguments: json!({}),
                    }],
                    continuation: vec![],
                },
                GenerationMessage {
                    role: GenerationRole::User,
                    content: vec![GenerationPart::ToolResult {
                        call_id: "provider-call".into(),
                        content: "Checked source".into(),
                        is_error: false,
                        witness: Some(witness.parse().unwrap()),
                    }],
                    continuation: vec![],
                },
            ],
            tools: vec![],
            max_output_tokens: 64,
        };
        for wire in [
            request("model", &input, true).2,
            crate::chat_generation::request("model", &input, true).2,
        ] {
            let wire: Value = serde_json::from_slice(&wire).unwrap();
            let body = wire.to_string();
            assert!(body.contains("provider-call"));
            assert!(body.contains("Checked source"));
            assert!(!body.contains("witness"));
            assert!(!body.contains(witness));
        }
    }

    #[test]
    fn responses_carries_an_image_as_image_content() {
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
        assert_eq!(body["input"][0]["content"][0]["type"], "input_image");
        assert_eq!(
            body["input"][0]["content"][0]["image_url"],
            "data:image/png;base64,c3ludGhldGlj"
        );
    }

    #[test]
    fn responses_tools_and_reasoning_continue_without_stored_upstream_state() {
        let answer = output(br#"{"status":"completed","output":[{"type":"reasoning","id":"reasoning","encrypted_content":"opaque"},{"type":"function_call","call_id":"call","name":"read","arguments":"{\"operation\":\"ThreadGet\"}"}]}"#).unwrap();
        let input = GenerationInput {
            system: "Read only".into(),
            messages: vec![answer.message],
            tools: vec![],
            max_output_tokens: 256,
        };
        let (_, _, wire) = request("model", &input, true);
        let wire: Value = serde_json::from_slice(&wire).unwrap();
        assert_eq!(wire["store"], false);
        assert_eq!(wire["stream"], true);
        assert_eq!(wire["input"][0]["encrypted_content"], "opaque");
        assert_eq!(wire["input"][1]["call_id"], "call");
    }

    #[test]
    fn responses_stream_requires_terminal_output_and_preserves_split_unicode() {
        let stream = "data: {\"type\":\"response.output_text.delta\",\"delta\":\"你好\"}\n\ndata: {\"type\":\"response.completed\",\"response\":{\"status\":\"completed\",\"output\":[{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"你好\"}]}]}}\n\n";
        let mut reader = ResponsesStream::default();
        let mut deltas = Vec::new();
        for byte in stream.as_bytes() {
            deltas.extend(reader.push(&[*byte]).unwrap());
        }
        assert_eq!(
            deltas,
            vec![GenerationDelta::Text {
                text: "你好".into()
            }]
        );
        assert_eq!(
            Box::new(reader).finish().unwrap().stop,
            GenerationStop::Complete
        );
        assert!(Box::new(ResponsesStream::default()).finish().is_err());
    }
}
