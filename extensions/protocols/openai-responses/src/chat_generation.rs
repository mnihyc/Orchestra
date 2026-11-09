//! Chat dialogue with tool calls and bounded incremental argument assembly.

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
    WireError::Internal("the chat response contains no completed dialogue step".to_owned())
}

/// Builds one Chat request; results are separate tool messages, never system text.
pub fn request(
    model: &str,
    input: &GenerationInput,
    stream: bool,
) -> (Method, &'static str, Bytes) {
    request_at(&Default::default(), model, input, stream)
}

/// Uses the same endpoint output-limit declaration as owned text completions.
pub fn request_at(
    options: &orch_model_contract::endpoint::EndpointOptions,
    model: &str,
    input: &GenerationInput,
    stream: bool,
) -> (Method, &'static str, Bytes) {
    let mut messages = vec![json!({"role":"system","content":input.system})];
    for message in &input.messages {
        let mut text = String::new();
        let mut calls = Vec::new();
        let mut results = Vec::new();
        let mut images = Vec::new();
        for part in &message.content {
            match part {
                GenerationPart::Image {media_type,data,..}=>images.push(json!({"type":"image_url","image_url":{"url":format!("data:{media_type};base64,{data}")}})),
                GenerationPart::ImageRef {name,url,..}=>text.push_str(&format!("[{name}]({url})")),
                GenerationPart::Text { text: value } => text.push_str(value),
                GenerationPart::ToolCall { id, name, arguments } => calls.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":arguments.to_string()}})),
                GenerationPart::ToolResult { call_id, content, .. } => results.push(json!({"role":"tool","tool_call_id":call_id,"content":content})),
            }
        }
        if !text.is_empty() || !calls.is_empty() || !images.is_empty() {
            let mut row = json!({"role":if message.role == GenerationRole::Assistant {"assistant"} else {"user"},"content":text});
            if !calls.is_empty() {
                row["tool_calls"] = json!(calls);
            }
            if !images.is_empty() {
                images.insert(0, json!({"type":"text","text":text}));
                row["content"] = json!(images);
            }
            for retained in &message.continuation {
                if let Some(details) = retained.get("reasoning_details") {
                    row["reasoning_details"] = details.clone();
                }
            }
            messages.push(row);
        }
        messages.extend(results);
    }
    let tools: Vec<_> = input.tools.iter().map(|tool| json!({"type":"function","function":{"name":tool.name,"description":tool.description,"parameters":tool.parameters}})).collect();
    let field = options.output_limit_field.unwrap_or_default().as_str();
    let mut body = json!({"model":model,"messages":messages,"tools":tools,(field):input.max_output_tokens,"stream":stream});
    if stream {
        body["stream_options"] = json!({"include_usage":true});
    }
    (
        Method::POST,
        "/chat/completions",
        Bytes::from(body.to_string()),
    )
}

/// Parse one complete Chat answer, permitting tools only with the tools stop reason.
pub fn output(body: &[u8]) -> Result<GenerationOutput, WireError> {
    let reply: Value = serde_json::from_slice(body).map_err(|_| bad())?;
    if reply.get("error").is_some_and(|error| !error.is_null()) {
        return Err(bad());
    }
    let choice = reply.pointer("/choices/0").ok_or_else(bad)?;
    let message = choice.get("message").ok_or_else(bad)?;
    let stop = match choice.get("finish_reason").and_then(Value::as_str) {
        Some("stop") => GenerationStop::Complete,
        Some("tool_calls") => GenerationStop::Tools,
        Some("length") => GenerationStop::Limit,
        Some("content_filter") => GenerationStop::Refused,
        _ => return Err(bad()),
    };
    let mut content = Vec::new();
    if let Some(text) = message.get("content").and_then(Value::as_str) {
        content.push(GenerationPart::Text {
            text: text.to_owned(),
        });
    }
    if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
        for call in calls {
            if call.get("type").and_then(Value::as_str) != Some("function") {
                return Err(bad());
            }
            content.push(GenerationPart::ToolCall {
                id: call
                    .get("id")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                name: call
                    .pointer("/function/name")
                    .and_then(Value::as_str)
                    .ok_or_else(bad)?
                    .to_owned(),
                arguments: serde_json::from_str(
                    call.pointer("/function/arguments")
                        .and_then(Value::as_str)
                        .ok_or_else(bad)?,
                )
                .map_err(|_| bad())?,
            });
        }
    }
    let continuation = message
        .get("reasoning_details")
        .map(|details| vec![json!({"reasoning_details":details})])
        .unwrap_or_default();
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
struct Tool {
    id: String,
    name: String,
    arguments: String,
}

/// Chat stream finalizes only after a finish reason and the terminating DONE frame.
#[derive(Default)]
pub struct ChatStream {
    parser: SseParser,
    text: String,
    tools: BTreeMap<u64, Tool>,
    reason: Option<String>,
    done: bool,
    native_frames: Vec<Value>,
}

impl GenerationStream for ChatStream {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<GenerationDelta>, WireError> {
        let mut deltas = Vec::new();
        for event in self.parser.push(bytes) {
            if event.data == "[DONE]" {
                self.done = true;
                continue;
            }
            if self.done {
                return Err(bad());
            }
            let value: Value = serde_json::from_str(&event.data).map_err(|_| bad())?;
            self.native_frames.push(value.clone());
            if value.get("error").is_some_and(|error| !error.is_null()) {
                return Err(bad());
            }
            let Some(choice) = value.pointer("/choices/0") else {
                continue;
            };
            if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
                self.reason = Some(reason.to_owned());
            }
            if let Some(text) = choice.pointer("/delta/content").and_then(Value::as_str) {
                self.text.push_str(text);
                deltas.push(GenerationDelta::Text {
                    text: text.to_owned(),
                });
            }
            for call in choice
                .pointer("/delta/tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
            {
                let index = call.get("index").and_then(Value::as_u64).ok_or_else(bad)?;
                let tool = self.tools.entry(index).or_default();
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    tool.id.push_str(id);
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    tool.name.push_str(name);
                }
                if let Some(arguments) = call.pointer("/function/arguments").and_then(Value::as_str)
                {
                    tool.arguments.push_str(arguments);
                }
            }
        }
        Ok(deltas)
    }
    fn finish(self: Box<Self>) -> Result<GenerationOutput, WireError> {
        if !self.done {
            return Err(bad());
        }
        let calls:Vec<_>=self.tools.values().map(|call|json!({"id":call.id,"type":"function","function":{"name":call.name,"arguments":call.arguments}})).collect();
        let annotations: Vec<_> = self
            .native_frames
            .iter()
            .flat_map(|frame| {
                frame
                    .pointer("/choices/0/delta/annotations")
                    .or_else(|| frame.pointer("/choices/0/message/annotations"))
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .cloned()
            })
            .collect();
        let mut answer = output(&serde_json::to_vec(&json!({"choices":[{"finish_reason":self.reason,"message":{"content":self.text,"tool_calls":calls,"annotations":annotations}}]})).map_err(|_| bad())?)?;
        answer.native_result["frames"] = json!(self.native_frames);
        Ok(answer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reported_chat_annotations_and_unknown_native_members_remain_passive_result_data() {
        let citation = json!({"type":"url_citation","url_citation":{"url":"https://source.example.invalid/","title":"Actual source","content":"Reported snippet"}});
        let body = json!({"choices":[{"finish_reason":"stop","message":{"content":"Reported answer","annotations":[citation]}}],"future_native_member":{"kept":true}});
        let whole = output(&serde_json::to_vec(&body).unwrap()).unwrap();
        assert_eq!(whole.native_result["response"], body);
        let frame = json!({"choices":[{"finish_reason":"stop","delta":{"content":"Reported answer","annotations":[citation]}}],"future_stream_member":{"kept":true}});
        let mut reader = ChatStream::default();
        reader
            .push(format!("data: {frame}\n\ndata: [DONE]\n\n").as_bytes())
            .unwrap();
        let streamed = Box::new(reader).finish().unwrap();
        assert_eq!(streamed.message, whole.message);
        assert_eq!(
            streamed.native_result["response"]["choices"][0]["message"]["annotations"],
            json!([citation])
        );
        assert_eq!(streamed.native_result["frames"], json!([frame]));
    }

    #[test]
    fn chat_carries_an_image_as_image_content() {
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
        assert_eq!(body["messages"][1]["content"][1]["type"], "image_url");
        assert_eq!(
            body["messages"][1]["content"][1]["image_url"]["url"],
            "data:image/png;base64,c3ludGhldGlj"
        );
    }

    #[test]
    fn chat_stream_assembles_tools_and_requires_a_terminal_frame() {
        let stream=b"data: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"id\":\"call\",\"function\":{\"name\":\"read\",\"arguments\":\"{\\\"operation\\\":\"}}]}}]}\n\ndata: {\"choices\":[{\"delta\":{\"tool_calls\":[{\"index\":0,\"function\":{\"arguments\":\"\\\"ThreadGet\\\"}\"}}]},\"finish_reason\":\"tool_calls\"}]}\n\ndata: [DONE]\n\n";
        let mut reader = ChatStream::default();
        for chunk in stream.chunks(3) {
            reader.push(chunk).unwrap();
        }
        let answer = Box::new(reader).finish().unwrap();
        assert_eq!(answer.stop, GenerationStop::Tools);
        assert!(
            matches!(&answer.message.content[1],GenerationPart::ToolCall {arguments,..} if arguments["operation"]=="ThreadGet")
        );
        assert!(Box::new(ChatStream::default()).finish().is_err());
    }
}
