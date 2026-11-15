//! Request conversion and exact restoration of the other interface's signed state.

use super::*;

const MESSAGES_STATE: &str = "orch:messages:v1:";
pub(super) const RESPONSES_STATE: &str = "orch:responses:v1:";
pub(super) const COMPACT_STATE: &str = "orch:compact:v1:";

pub(super) fn append_message(messages: &mut Vec<Value>, role: &str, mut blocks: Vec<Value>) {
    if blocks.is_empty() {
        return;
    }
    if let Some(last) = messages.last_mut()
        && last["role"] == role
        && let Some(content) = last["content"].as_array_mut()
    {
        content.append(&mut blocks);
        return;
    }
    messages.push(json!({"role":role,"content":blocks}));
}

fn array_or_text(value: &Value) -> Vec<Value> {
    match value {
        Value::String(text) => vec![json!({"type":"text","text":text})],
        Value::Array(blocks) => blocks.clone(),
        _ => Vec::new(),
    }
}

fn message_image(url: &str) -> Result<Value, TranslateError> {
    if let Some(data) = url.strip_prefix("data:") {
        let (kind, data) = data
            .split_once(";base64,")
            .ok_or_else(|| unsupported("An inline image must use a base64 data URL"))?;
        Ok(json!({"type":"image","source":{"type":"base64","media_type":kind,"data":data}}))
    } else {
        Ok(json!({"type":"image","source":{"type":"url","url":url}}))
    }
}

fn response_image(block: &Value) -> Result<Value, TranslateError> {
    let source = &block["source"];
    let url = match source["type"].as_str() {
        Some("base64") => format!(
            "data:{};base64,{}",
            text(source, "media_type"),
            text(source, "data")
        ),
        Some("url") => text(source, "url"),
        _ => {
            return Err(unsupported(
                "Only URL and base64 image sources cross interfaces",
            ));
        }
    };
    Ok(json!({"type":"input_image","image_url":url}))
}

fn output_limit(request: &Value, upstream: &UpstreamSide) -> Result<Value, TranslateError> {
    if let Some(limit) = request
        .get("max_output_tokens")
        .filter(|value| value.as_u64().is_some_and(|v| v > 0))
    {
        return Ok(limit.clone());
    }
    let parameters = upstream.model.as_ref();
    let limit = parameters
        .and_then(|model| model.offered.as_ref())
        .and_then(|offered| offered.report.as_ref())
        .and_then(|report| report.output_max)
        .or_else(|| {
            parameters
                .and_then(|model| model.option_schema(ANTHROPIC_MESSAGES))
                .and_then(|schema| schema.pointer("/properties/max_tokens/default"))
                .and_then(Value::as_i64)
        });
    limit
        .or(Some(upstream.output_tokens))
        .filter(|value| *value > 0)
        .map(|value| json!(value))
        .ok_or_else(|| malformed("the captured translation output allowance is not positive"))
}

fn enabled_effort(
    model: &orch_model_contract::translator::UpstreamSide,
    protocol: &str,
    path: &[&str],
    effort: &str,
) -> bool {
    model.model.as_ref().is_some_and(|model| {
        model
            .efforts
            .as_ref()
            .is_some_and(|values| values.iter().any(|value| value == effort))
            || model
                .option_rule_at(protocol, path)
                .and_then(|rule| rule.get("enum"))
                .and_then(Value::as_array)
                .is_some_and(|values| values.iter().any(|value| value.as_str() == Some(effort)))
    })
}

fn thinking(request: &Value, upstream: &UpstreamSide, body: &mut Value) {
    let Some(effort) = request.pointer("/reasoning/effort").and_then(Value::as_str) else {
        return;
    };
    if !enabled_effort(
        upstream,
        ANTHROPIC_MESSAGES,
        &["output_config", "effort"],
        effort,
    ) {
        return;
    }
    body["output_config"]["effort"] = json!(effort);
    if let Some(model) = &upstream.model {
        if model
            .reasoning_modes
            .as_ref()
            .is_some_and(|modes| modes.iter().any(|mode| mode == "adaptive"))
        {
            body["thinking"] = json!({"type":"adaptive"});
        } else if let Some(default) = model
            .option_rule(ANTHROPIC_MESSAGES, "thinking")
            .and_then(|rule| rule.get("default"))
        {
            body["thinking"] = default.clone();
        }
    }
}

fn sampling(request: &Value, upstream: &UpstreamSide, protocol: &str, body: &mut Value) {
    for key in ["temperature", "top_p"] {
        if let Some(value) = request.get(key)
            && upstream
                .model
                .as_ref()
                .and_then(|model| model.option_rule(protocol, key))
                .is_some_and(|rule| rule != &Value::Bool(false))
        {
            body[key] = value.clone();
        }
    }
}

pub(super) fn to_messages(
    request: &Value,
    upstream: &UpstreamSide,
) -> Result<Value, TranslateError> {
    let model = request
        .get("model")
        .filter(|value| value.is_string())
        .ok_or_else(|| malformed("model is required"))?;
    let mut body = json!({"model":model,"messages":[],"max_tokens":output_limit(request,upstream)?,
        "stream":request.get("stream").cloned().unwrap_or(json!(false))});
    let mut system = array_or_text(&request["instructions"]);
    let mut messages = Vec::new();
    let items = match &request["input"] {
        Value::String(input) => vec![json!({"role":"user","content":input})],
        Value::Array(items) => items.clone(),
        Value::Null => Vec::new(),
        _ => return Err(malformed("input is text or a list of conversation items")),
    };
    for item in items {
        match item["type"].as_str().unwrap_or("message") {
            "message" => {
                let role = item["role"].as_str().unwrap_or("user");
                let mut blocks = Vec::new();
                for block in array_or_text(&item["content"]) {
                    match block["type"].as_str() {
                        Some("text" | "input_text" | "output_text") => {
                            blocks.push(json!({"type":"text","text":block["text"]}))
                        }
                        Some("input_image") => blocks.push(message_image(
                            block["image_url"]
                                .as_str()
                                .ok_or_else(|| malformed("image_url is required"))?,
                        )?),
                        Some("refusal") => {
                            blocks.push(json!({"type":"text","text":block["refusal"]}))
                        }
                        _ => {
                            return Err(unsupported(
                                "This conversation content has no Messages equivalent",
                            ));
                        }
                    }
                }
                if matches!(role, "system" | "developer") {
                    if messages.is_empty() {
                        system.extend(blocks);
                    } else {
                        append_message(&mut messages, "user", blocks);
                    }
                } else {
                    append_message(
                        &mut messages,
                        if role == "assistant" {
                            "assistant"
                        } else {
                            "user"
                        },
                        blocks,
                    );
                }
            }
            "function_call" | "custom_tool_call" => {
                let input = if item["type"] == "custom_tool_call" {
                    json!({"input":item["input"]})
                } else {
                    serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
                        .map_err(malformed)?
                };
                append_message(
                    &mut messages,
                    "assistant",
                    vec![json!({
                        "type":"tool_use","id":item["call_id"],"name":item["name"],"input":input
                    })],
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                let mut result = Vec::new();
                for block in array_or_text(&item["output"]) {
                    match block["type"].as_str() {
                        Some("text" | "input_text" | "output_text") => {
                            result.push(json!({"type":"text","text":block["text"]}))
                        }
                        Some("input_image") => {
                            result.push(message_image(&text(&block, "image_url"))?)
                        }
                        _ => {
                            return Err(unsupported("This tool output has no Messages equivalent"));
                        }
                    }
                }
                append_message(
                    &mut messages,
                    "user",
                    vec![json!({
                        "type":"tool_result","tool_use_id":item["call_id"],"content":result
                    })],
                );
            }
            "reasoning" => {
                if let Some(block) = restore(MESSAGES_STATE, &item["encrypted_content"])? {
                    if !matches!(
                        block["type"].as_str(),
                        Some("thinking" | "redacted_thinking")
                    ) {
                        return Err(malformed("the carried Messages reasoning block is invalid"));
                    }
                    append_message(&mut messages, "assistant", vec![block]);
                } else {
                    let summary = item
                        .get("summary")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .map(|part| text(part, "text"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !summary.is_empty() {
                        append_message(
                            &mut messages,
                            "assistant",
                            vec![json!({"type":"text","text":summary})],
                        );
                    }
                }
            }
            "compaction_trigger" => {
                if !super::compaction_trigger(request) {
                    return Err(malformed(
                        "a compaction trigger must be the final conversation item",
                    ));
                }
            }
            "compaction" => {
                let summary = restore(COMPACT_STATE, &item["encrypted_content"])?
                    .and_then(|value| value.as_str().map(str::to_owned))
                    .ok_or_else(|| {
                        unsupported("This compaction item belongs to another upstream")
                    })?;
                append_message(
                    &mut messages,
                    "user",
                    vec![json!({"type":"text","text":summary})],
                );
            }
            _ => {
                return Err(unsupported(
                    "This conversation item has no Messages equivalent",
                ));
            }
        }
    }
    // Messages requires a user opening; a carried assistant item may follow system-only input.
    if messages
        .first()
        .is_some_and(|message| message["role"] == "assistant")
    {
        messages.insert(
            0,
            json!({"role":"user","content":[{"type":"text","text":"Continue the conversation."}]}),
        );
    }
    if let Some(block) = system.last_mut() {
        block["cache_control"] = json!({"type":"ephemeral"});
    }
    if !system.is_empty() {
        body["system"] = json!(system);
    }
    if let Some(last) = messages
        .last_mut()
        .and_then(|message| message["content"].as_array_mut())
        .and_then(|blocks| blocks.last_mut())
    {
        if !matches!(
            last["type"].as_str(),
            Some("thinking" | "redacted_thinking")
        ) {
            last["cache_control"] = json!({"type":"ephemeral"});
        }
    }
    body["messages"] = json!(messages);
    let tools = request.get("tools").and_then(Value::as_array).into_iter().flatten().filter_map(|tool| {
        match tool["type"].as_str() {
            Some("function") => Some(json!({"name":tool["name"],"description":tool.get("description").cloned().unwrap_or(json!("")),
                "input_schema":tool.get("parameters").cloned().unwrap_or(json!({"type":"object","properties":{}}))})),
            Some("custom") => Some(json!({"name":tool["name"],
                "description":format!("{}\nInput format: {}",text(tool,"description"),tool.get("format").unwrap_or(&Value::Null)),
                "input_schema":{"type":"object","properties":{"input":{"type":"string"}},"required":["input"],"additionalProperties":false}})),
            _ => None,
        }
    }).collect::<Vec<_>>();
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        let choice = &request["tool_choice"];
        body["tool_choice"] = match choice.as_str() {
            Some("required") => json!({"type":"any"}),
            Some("none") => json!({"type":"none"}),
            Some(_) | None if choice.is_string() || choice.is_null() => json!({"type":"auto"}),
            _ => json!({"type":"tool","name":choice["name"]}),
        };
        if request["parallel_tool_calls"] == false {
            body["tool_choice"]["disable_parallel_tool_use"] = json!(true);
        }
    }
    if let Some(format) = request
        .pointer("/text/format")
        .filter(|format| format["type"] == "json_schema")
    {
        body["output_config"] = json!({"format":{"type":"json_schema","schema":format["schema"]}});
    }
    thinking(request, upstream, &mut body);
    sampling(request, upstream, ANTHROPIC_MESSAGES, &mut body);
    Ok(body)
}

fn tool_result(block: &Value) -> Result<(String, Vec<Value>), TranslateError> {
    let mut result = Vec::new();
    let mut images = Vec::new();
    for part in array_or_text(&block["content"]) {
        match part["type"].as_str() {
            Some("text") => result.push(text(&part, "text")),
            Some("image") => images.push(response_image(&part)?),
            _ => {
                return Err(unsupported(
                    "This tool result content has no Responses equivalent",
                ));
            }
        }
    }
    if block["is_error"] == true {
        result.insert(0, "The tool failed:".into());
    }
    Ok((result.join("\n"), images))
}

pub(super) fn to_responses(
    request: &Value,
    upstream: &UpstreamSide,
) -> Result<Value, TranslateError> {
    let mut body = json!({"model":request["model"],"stream":request.get("stream").cloned().unwrap_or(json!(false)),
        "max_output_tokens":request["max_tokens"],"input":[]});
    let system = array_or_text(&request["system"])
        .iter()
        .map(|block| text(block, "text"))
        .collect::<Vec<_>>()
        .join("\n");
    if !system.is_empty() {
        body["instructions"] = json!(system);
    }
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("messages are required"))?;
    let mut items = Vec::new();
    for message in messages {
        let role = message["role"].as_str().unwrap_or("user");
        let mut content = Vec::new();
        let flush = |content: &mut Vec<Value>, items: &mut Vec<Value>| {
            if !content.is_empty() {
                items.push(json!({"role":role,"content":std::mem::take(content)}));
            }
        };
        for block in array_or_text(&message["content"]) {
            match block["type"].as_str() {
                Some("text") => content.push(json!({"type":if role=="assistant"{"output_text"}else{"input_text"},"text":block["text"]})),
                Some("image") => content.push(response_image(&block)?),
                Some("tool_use") => {
                    flush(&mut content,&mut items);
                    items.push(json!({"type":"function_call","call_id":block["id"],"name":block["name"],"arguments":block["input"].to_string()}));
                }
                Some("tool_result") => {
                    flush(&mut content,&mut items);
                    let (output,images) = tool_result(&block)?;
                    items.push(json!({"type":"function_call_output","call_id":block["tool_use_id"],"output":output}));
                    if !images.is_empty() { items.push(json!({"role":"user","content":images})); }
                }
                Some("thinking") => {
                    if let Some(item) = restore(RESPONSES_STATE,&block["signature"])? {
                        flush(&mut content,&mut items);
                        if item["type"] != "reasoning" { return Err(malformed("the carried Responses reasoning item is invalid")); }
                        items.push(item);
                    } else if !text(&block,"thinking").is_empty() {
                        content.push(json!({"type":"output_text","text":block["thinking"]}));
                    }
                }
                Some("redacted_thinking") => {}
                _ => return Err(unsupported("This Messages content has no Responses equivalent")),
            }
        }
        flush(&mut content, &mut items);
    }
    body["input"] = json!(items);
    let tools = request.get("tools").and_then(Value::as_array).into_iter().flatten()
        .filter(|tool| tool.get("type").is_none() || tool["type"] == "custom")
        .map(|tool| json!({"type":"function","name":tool["name"],
            "description":tool.get("description").cloned().unwrap_or(json!("")),
            "parameters":tool["input_schema"],"strict":tool.get("strict").cloned().unwrap_or(json!(false))}))
        .collect::<Vec<_>>();
    if !tools.is_empty() {
        body["tools"] = json!(tools);
        body["tool_choice"] = match request.pointer("/tool_choice/type").and_then(Value::as_str) {
            Some("any") => json!("required"),
            Some("none") => json!("none"),
            Some("tool") => json!({"type":"function","name":request["tool_choice"]["name"]}),
            _ => json!("auto"),
        };
        if request.pointer("/tool_choice/disable_parallel_tool_use") == Some(&json!(true)) {
            body["parallel_tool_calls"] = json!(false);
        }
    }
    if let Some(format) = request
        .pointer("/output_config/format")
        .filter(|format| format["type"] == "json_schema")
    {
        body["text"] = json!({"format":{"type":"json_schema","name":"output","schema":format["schema"],"strict":true}});
    }
    if let Some(session) = &upstream.session {
        body["prompt_cache_key"] = json!(session);
    }
    let effort = request
        .pointer("/output_config/effort")
        .and_then(Value::as_str)
        .or_else(|| {
            (request.pointer("/thinking/type").and_then(Value::as_str) == Some("enabled"))
                .then(|| {
                    upstream
                        .model
                        .as_ref()
                        .and_then(|model| model.default_effort.as_deref())
                })
                .flatten()
        });
    if let Some(effort) = effort.filter(|effort| {
        enabled_effort(upstream, OPENAI_RESPONSES, &["reasoning", "effort"], effort)
    }) {
        body["reasoning"] = json!({"effort":effort,"summary":"auto"});
    }
    sampling(request, upstream, OPENAI_RESPONSES, &mut body);
    Ok(body)
}

pub(super) fn reasoning(block: &Value, id: &str) -> Value {
    json!({"id":id,"type":"reasoning","summary":if block["type"] == "thinking" {json!([{"type":"summary_text","text":text(block,"thinking")}])}else{json!([])},
        "encrypted_content":opaque(MESSAGES_STATE,block)})
}
