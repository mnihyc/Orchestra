//! Incremental conversion preserves upstream termination; EOF alone is never success.

use super::*;
use orch_model_contract::usage::SseParser;

struct Block {
    value: Value,
    arguments: String,
    stopped: bool,
}

/// A converter holds a single upstream answer, never a credential.
pub(super) struct Converter {
    direction: Direction,
    answer: Answer,
    custom: BTreeSet<String>,
    parser: SseParser,
    undecided: Vec<u8>,
    json: Option<bool>,
    whole: Vec<u8>,
    blocks: BTreeMap<usize, Block>,
    // Responses messages can contain several content parts; each is one Messages block.
    parts: BTreeMap<(usize, usize), usize>,
    next_part: usize,
    id: String,
    created_at: u64,
    model: String,
    usage: Value,
    stop: String,
    sequence: u64,
    terminal: bool,
}

impl Converter {
    pub(super) fn new(direction: Direction, answer: Answer, custom: BTreeSet<String>) -> Self {
        Self {
            direction,
            answer,
            custom,
            parser: SseParser::new(),
            undecided: Vec::new(),
            json: None,
            whole: Vec::new(),
            blocks: BTreeMap::new(),
            parts: BTreeMap::new(),
            next_part: 0,
            id: String::new(),
            created_at: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|time| time.as_secs())
                .unwrap_or_default(),
            model: String::new(),
            usage: json!({}),
            stop: String::new(),
            sequence: 0,
            terminal: false,
        }
    }

    fn emit(&mut self, out: &mut Vec<u8>, kind: &str, mut value: Value) {
        value["type"] = json!(kind);
        if self.direction == Direction::Responses {
            value["sequence_number"] = json!(self.sequence);
            self.sequence += 1;
        }
        out.extend_from_slice(format!("event: {kind}\ndata: {value}\n\n").as_bytes());
    }

    fn response(&self, status: &str) -> Value {
        let output = if self.answer == Answer::CompactStream {
            if status == "completed" {
                vec![self.compaction_item()]
            } else {
                Vec::new()
            }
        } else {
            self.blocks
                .iter()
                .map(|(index, block)| self.response_item(*index, block))
                .collect::<Vec<_>>()
        };
        json!({"id":self.id,"object":"response","created_at":self.created_at,"status":status,"model":self.model,
            "output":output,"usage":response_usage(&self.usage),"error":null,
            "incomplete_details":if status=="incomplete"{json!({"reason":"max_output_tokens"})}else{Value::Null}})
    }

    fn summary(&self) -> String {
        self.blocks
            .values()
            .filter(|block| block.value["type"] == "text")
            .map(|block| text(&block.value, "text"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn compaction_item(&self) -> Value {
        json!({"id":format!("cmp_{}",self.id),"type":"compaction",
            "encrypted_content":opaque(requests::COMPACT_STATE,&json!(self.summary()))})
    }

    fn item_id(&self, index: usize) -> String {
        let prefix = self
            .blocks
            .get(&index)
            .map(|block| match block.value["type"].as_str() {
                Some("thinking" | "redacted_thinking") => "rs",
                Some("tool_use") if self.custom.contains(&text(&block.value, "name")) => "ctc",
                Some("tool_use") => "fc",
                _ => "msg",
            })
            .unwrap_or("msg");
        format!("{prefix}_{}_{index}", self.id)
    }

    fn response_item(&self, index: usize, block: &Block) -> Value {
        let id = self.item_id(index);
        match block.value["type"].as_str() {
            Some("text") => json!({"id":id,"type":"message","role":"assistant",
                "status":if block.stopped{"completed"}else{"in_progress"},
                "content":if block.stopped {json!([{"type":"output_text","text":block.value["text"],"annotations":[]}])} else {json!([])}}),
            Some("thinking" | "redacted_thinking") => {
                if block.stopped {
                    requests::reasoning(&block.value, &id)
                } else {
                    json!({"id":id,"type":"reasoning","summary":[]})
                }
            }
            Some("tool_use") => {
                let name = text(&block.value, "name");
                let arguments = if block.stopped && block.arguments.is_empty() {
                    block.value["input"].to_string()
                } else {
                    block.arguments.clone()
                };
                if self.custom.contains(&name) {
                    let input = serde_json::from_str::<Value>(&arguments)
                        .ok()
                        .and_then(|value| value.get("input").cloned())
                        .unwrap_or(json!(""));
                    json!({"id":id,"type":"custom_tool_call","call_id":block.value["id"],
                        "name":name,"input":input,"status":if block.stopped{"completed"}else{"in_progress"}})
                } else {
                    json!({"id":id,"type":"function_call","call_id":block.value["id"],
                        "name":name,"arguments":arguments,"status":if block.stopped{"completed"}else{"in_progress"}})
                }
            }
            _ => Value::Null,
        }
    }

    fn messages_event(&mut self, event: Value, out: &mut Vec<u8>) -> Result<(), TranslateError> {
        let index = event["index"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        match event["type"].as_str() {
            Some("message_start") => {
                self.id = text(&event["message"], "id");
                self.model = text(&event["message"], "model");
                self.usage = event["message"]["usage"].clone();
                self.emit(
                    out,
                    "response.created",
                    json!({"response":self.response("in_progress")}),
                );
                self.emit(
                    out,
                    "response.in_progress",
                    json!({"response":self.response("in_progress")}),
                );
            }
            Some("content_block_start") => {
                let value = event["content_block"].clone();
                if !matches!(
                    value["type"].as_str(),
                    Some("text" | "thinking" | "redacted_thinking" | "tool_use")
                ) {
                    return Err(unsupported(
                        "The upstream returned a server content block without a Responses equivalent",
                    ));
                }
                self.blocks.insert(
                    index,
                    Block {
                        value,
                        arguments: String::new(),
                        stopped: false,
                    },
                );
                if self.answer == Answer::CompactStream {
                    return Ok(());
                }
                let item = self
                    .blocks
                    .get(&index)
                    .map(|block| self.response_item(index, block))
                    .unwrap_or(Value::Null);
                self.emit(
                    out,
                    "response.output_item.added",
                    json!({"output_index":index,"item":item}),
                );
                if self
                    .blocks
                    .get(&index)
                    .is_some_and(|block| block.value["type"] == "thinking")
                {
                    self.emit(out,"response.reasoning_summary_part.added",json!({"output_index":index,
                        "summary_index":0,"item_id":self.item_id(index),"part":{"type":"summary_text","text":""}}));
                }
                let item_id = self.item_id(index);
                if self
                    .blocks
                    .get(&index)
                    .is_some_and(|block| block.value["type"] == "text")
                {
                    self.emit(
                        out,
                        "response.content_part.added",
                        json!({"output_index":index,"content_index":0,
                        "item_id":item_id,
                        "part":{"type":"output_text","text":"","annotations":[]}}),
                    );
                }
            }
            Some("content_block_delta") => {
                let delta = &event["delta"];
                let item_id = self.item_id(index);
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| malformed("a delta has no open content block"))?;
                let mut emitted = None;
                match delta["type"].as_str() {
                    Some("text_delta") => {
                        let added = text(delta, "text");
                        let mut accumulated = text(&block.value, "text");
                        accumulated.push_str(&added);
                        block.value["text"] = json!(accumulated);
                        emitted = Some((
                            "response.output_text.delta",
                            json!({"output_index":index,"content_index":0,
                            "item_id":item_id,"delta":added}),
                        ));
                    }
                    Some("input_json_delta") => {
                        let added = text(delta, "partial_json");
                        block.arguments.push_str(&added);
                        if !self.custom.contains(&text(&block.value, "name")) {
                            emitted = Some((
                                "response.function_call_arguments.delta",
                                json!({"output_index":index,
                                "item_id":item_id,"delta":added}),
                            ));
                        }
                        // Custom tool arguments must be valid JSON before its string can be unwrapped.
                        // They are emitted at block stop, without waiting for the rest of the response.
                    }
                    Some("thinking_delta") => {
                        let added = text(delta, "thinking");
                        let mut accumulated = text(&block.value, "thinking");
                        accumulated.push_str(&added);
                        block.value["thinking"] = json!(accumulated);
                        emitted = Some((
                            "response.reasoning_summary_text.delta",
                            json!({"output_index":index,"summary_index":0,
                            "item_id":item_id,"delta":added}),
                        ));
                    }
                    Some("signature_delta") => {
                        let mut signature = text(&block.value, "signature");
                        signature.push_str(&text(delta, "signature"));
                        block.value["signature"] = json!(signature);
                    }
                    Some("citations_delta") => {} // Citations are a declared server feature, not client tool state.
                    _ => {}
                }
                if self.answer == Answer::CompactStream {
                    out.extend_from_slice(b": compacting\n\n");
                    return Ok(());
                }
                if let Some((kind, value)) = emitted {
                    self.emit(out, kind, value);
                }
            }
            Some("content_block_stop") => {
                let block = self
                    .blocks
                    .get_mut(&index)
                    .ok_or_else(|| malformed("a stop has no open content block"))?;
                if block.value["type"] == "tool_use"
                    && self.custom.contains(&text(&block.value, "name"))
                {
                    let arguments = if block.arguments.is_empty() {
                        block.value["input"].to_string()
                    } else {
                        block.arguments.clone()
                    };
                    let input: Value = serde_json::from_str(&arguments).map_err(malformed)?;
                    if input.get("input").and_then(Value::as_str).is_none() {
                        return Err(malformed(
                            "a custom tool did not return its declared string argument",
                        ));
                    }
                }
                block.stopped = true;
                if self.answer == Answer::CompactStream {
                    return Ok(());
                }
                let item = self.response_item(
                    index,
                    self.blocks
                        .get(&index)
                        .ok_or_else(|| malformed("content block is absent"))?,
                );
                match item["type"].as_str() {
                    Some("custom_tool_call") => {
                        let input = item["input"].as_str().ok_or_else(|| malformed("custom tool input is not a string"))?;
                        self.emit(out,"response.custom_tool_call_input.delta",json!({"output_index":index,
                            "item_id":item["id"],"delta":input}));
                        self.emit(out,"response.custom_tool_call_input.done",json!({"output_index":index,
                            "item_id":item["id"],"input":input}));
                    }
                    Some("function_call") => self.emit(out,"response.function_call_arguments.done",
                        json!({"output_index":index,"item_id":item["id"],"arguments":item["arguments"]})),
                    Some("reasoning") => {
                        if let Some(summary) = item["summary"].as_array().and_then(|parts|parts.first()) {
                            self.emit(out,"response.reasoning_summary_text.done",json!({"output_index":index,
                                "summary_index":0,"item_id":item["id"],"text":summary["text"]}));
                            self.emit(out,"response.reasoning_summary_part.done",json!({"output_index":index,
                                "summary_index":0,"item_id":item["id"],"part":summary}));
                        }
                    }
                    Some("message") => {
                        self.emit(out,"response.output_text.done",json!({"output_index":index,"content_index":0,
                            "item_id":item["id"],"text":item["content"][0]["text"]}));
                        self.emit(out,"response.content_part.done",json!({"output_index":index,"content_index":0,
                            "item_id":item["id"],"part":item["content"][0]}));
                    }
                    _ => {}
                }
                self.emit(
                    out,
                    "response.output_item.done",
                    json!({"output_index":index,"item":item}),
                );
            }
            Some("message_delta") => {
                self.stop = text(&event["delta"], "stop_reason");
                merge_usage(&mut self.usage, &event["usage"]);
            }
            Some("message_stop") => {
                if self.blocks.values().any(|block| !block.stopped) || self.stop.is_empty() {
                    return Err(malformed(
                        "the upstream stopped before its content and stop reason were complete",
                    ));
                }
                if self.stop == "pause_turn" {
                    let mut response = self.response("failed");
                    response["error"] = json!({"code":"server_error","message":"The upstream paused its server-tool turn before completion."});
                    self.emit(out, "response.failed", json!({"response":response}));
                    self.terminal = true;
                    return Ok(());
                }
                if self.answer == Answer::CompactStream {
                    if self.stop != "end_turn" || self.summary().is_empty() {
                        return Err(malformed(
                            "the upstream did not complete a continuation summary",
                        ));
                    }
                    let item = self.compaction_item();
                    self.emit(
                        out,
                        "response.output_item.added",
                        json!({"output_index":0,"item":item}),
                    );
                    self.emit(
                        out,
                        "response.output_item.done",
                        json!({"output_index":0,"item":item}),
                    );
                    self.emit(
                        out,
                        "response.completed",
                        json!({"response":self.response("completed")}),
                    );
                    self.terminal = true;
                    return Ok(());
                }
                let kind =
                    if self.stop == "max_tokens" || self.stop == "model_context_window_exceeded" {
                        "response.incomplete"
                    } else {
                        "response.completed"
                    };
                let status = if kind == "response.incomplete" {
                    "incomplete"
                } else {
                    "completed"
                };
                self.emit(out, kind, json!({"response":self.response(status)}));
                self.terminal = true;
            }
            Some("error") => {
                self.emit(out, "error", response_error(&event));
                self.terminal = true;
            }
            Some("ping") => out.extend_from_slice(b": keepalive\n\n"),
            _ => {}
        }
        Ok(())
    }

    fn part(&mut self, output: usize, content: usize, block: Value, out: &mut Vec<u8>) -> usize {
        if let Some(index) = self.parts.get(&(output, content)) {
            return *index;
        }
        let index = self.next_part;
        self.next_part += 1;
        self.parts.insert((output, content), index);
        self.emit(
            out,
            "content_block_start",
            json!({"index":index,"content_block":block}),
        );
        index
    }

    fn stop_parts(&mut self, output: usize, out: &mut Vec<u8>) {
        let indices = self
            .parts
            .iter()
            .filter(|((item, _), _)| *item == output)
            .map(|(_, index)| *index)
            .collect::<Vec<_>>();
        for index in indices {
            self.emit(out, "content_block_stop", json!({"index":index}));
        }
    }

    fn responses_event(&mut self, event: Value, out: &mut Vec<u8>) -> Result<(), TranslateError> {
        let output = event["output_index"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        let content = event["content_index"]
            .as_u64()
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or(0);
        match event["type"].as_str() {
            Some("response.created") => {
                self.id = text(&event["response"], "id");
                self.model = text(&event["response"], "model");
                self.usage = event["response"]["usage"].clone();
                self.emit(
                    out,
                    "message_start",
                    json!({"message":{"id":self.id,"type":"message","role":"assistant",
                    "model":self.model,"content":[],"stop_reason":null,"stop_sequence":null,
                    "usage":messages_usage(&self.usage)}}),
                );
            }
            Some("response.output_item.added") => {
                let item = &event["item"];
                match item["type"].as_str() {
                    Some("function_call") => {
                        self.stop = "tool_use".into();
                        self.part(output,0,json!({"type":"tool_use","id":item["call_id"],"name":item["name"],"input":{}}),out);
                    }
                    Some("reasoning") => {
                        self.part(
                            output,
                            0,
                            json!({"type":"thinking","thinking":"","signature":""}),
                            out,
                        );
                    }
                    Some("message") => {}
                    _ => {
                        return Err(unsupported(
                            "The Responses upstream returned a server tool without a Messages equivalent",
                        ));
                    }
                }
            }
            Some("response.content_part.added") => {
                if event["part"]["type"] == "output_text" {
                    self.part(output, content, json!({"type":"text","text":""}), out);
                } else if event["part"]["type"] == "refusal" {
                    self.part(output, content, json!({"type":"text","text":""}), out);
                    self.stop = "refusal".into();
                }
            }
            Some("response.output_text.delta" | "response.refusal.delta") => {
                let index = self.part(output, content, json!({"type":"text","text":""}), out);
                self.emit(
                    out,
                    "content_block_delta",
                    json!({"index":index,"delta":{"type":"text_delta","text":event["delta"]}}),
                );
            }
            Some("response.function_call_arguments.delta") => {
                let index = *self
                    .parts
                    .get(&(output, 0))
                    .ok_or_else(|| malformed("tool arguments have no open item"))?;
                self.emit(out,"content_block_delta",json!({"index":index,"delta":{"type":"input_json_delta","partial_json":event["delta"]}}));
            }
            Some("response.reasoning_summary_text.delta") => {
                let index = self.part(
                    output,
                    0,
                    json!({"type":"thinking","thinking":"","signature":""}),
                    out,
                );
                self.emit(out,"content_block_delta",json!({"index":index,"delta":{"type":"thinking_delta","thinking":event["delta"]}}));
            }
            Some("response.output_item.done") => {
                if event["item"]["type"] == "reasoning" {
                    let index = self.part(
                        output,
                        0,
                        json!({"type":"thinking","thinking":"","signature":""}),
                        out,
                    );
                    self.emit(out,"content_block_delta",json!({"index":index,"delta":{
                        "type":"signature_delta","signature":opaque(requests::RESPONSES_STATE,&event["item"])}}));
                }
                self.stop_parts(output, out);
                self.blocks.insert(
                    output,
                    Block {
                        value: event["item"].clone(),
                        arguments: String::new(),
                        stopped: true,
                    },
                );
            }
            Some("response.completed" | "response.incomplete") => {
                if self.id.is_empty()
                    || self
                        .parts
                        .keys()
                        .any(|(item, _)| !self.blocks.contains_key(item))
                {
                    return Err(malformed(
                        "the upstream ended before its output items completed",
                    ));
                }
                let response = &event["response"];
                self.usage = response["usage"].clone();
                let reason = if event["type"] == "response.incomplete" {
                    "max_tokens"
                } else if self.stop.is_empty() {
                    "end_turn"
                } else {
                    &self.stop
                };
                self.emit(
                    out,
                    "message_delta",
                    json!({"delta":{"stop_reason":reason,"stop_sequence":null},
                    "usage":messages_usage(&self.usage)}),
                );
                self.emit(out, "message_stop", json!({}));
                self.terminal = true;
            }
            Some("response.failed") => {
                self.emit(out, "error", messages_error(&event["response"]));
                self.terminal = true;
            }
            Some("error") => {
                self.emit(out, "error", messages_error(&event));
                self.terminal = true;
            }
            _ => {}
        }
        Ok(())
    }

    fn json_answer(&self, value: Value) -> Result<Value, TranslateError> {
        if value.get("error").is_some_and(|value| !value.is_null()) || value["type"] == "error" {
            return Ok(match self.direction {
                Direction::Responses => response_error(&value),
                Direction::Messages => messages_error(&value),
            });
        }
        if self.answer == Answer::Count {
            let count = value
                .get("input_tokens")
                .and_then(Value::as_u64)
                .ok_or_else(|| malformed("the upstream did not return an input token count"))?;
            return Ok(json!({"input_tokens":count}));
        }
        match self.direction {
            Direction::Responses => {
                let content = value
                    .get("content")
                    .and_then(Value::as_array)
                    .ok_or_else(|| malformed("the upstream did not return a Message"))?;
                if self.answer == Answer::Compact {
                    if value["stop_reason"] != "end_turn" {
                        return Err(malformed(
                            "the upstream did not complete a continuation summary",
                        ));
                    }
                    let summary = content
                        .iter()
                        .filter(|block| block["type"] == "text")
                        .map(|block| text(block, "text"))
                        .collect::<Vec<_>>()
                        .join("\n");
                    return Ok(
                        json!({"id":value["id"],"object":"response.compaction","created_at":self.created_at,
                        "output":[{"id":format!("{}_compact",text(&value,"id")),"type":"compaction",
                            "encrypted_content":opaque(requests::COMPACT_STATE,&json!(summary))}],
                        "usage":response_usage(&value["usage"])}),
                    );
                }
                let mut converter =
                    Converter::new(self.direction, self.answer, self.custom.clone());
                converter.id = text(&value, "id");
                converter.created_at = self.created_at;
                converter.model = text(&value, "model");
                converter.usage = value["usage"].clone();
                converter.stop = text(&value, "stop_reason");
                for (index, block) in content.iter().enumerate() {
                    if !matches!(
                        block["type"].as_str(),
                        Some("text" | "thinking" | "redacted_thinking" | "tool_use")
                    ) {
                        return Err(unsupported(
                            "The upstream returned a server content block without a Responses equivalent",
                        ));
                    }
                    if block["type"] == "tool_use"
                        && self.custom.contains(&text(block, "name"))
                        && block
                            .pointer("/input/input")
                            .and_then(Value::as_str)
                            .is_none()
                    {
                        return Err(malformed(
                            "a custom tool did not return its declared string argument",
                        ));
                    }
                    converter.blocks.insert(
                        index,
                        Block {
                            value: block.clone(),
                            arguments: String::new(),
                            stopped: true,
                        },
                    );
                }
                if converter.stop == "pause_turn" {
                    let mut response = converter.response("failed");
                    response["error"] = json!({"code":"server_error","message":"The upstream paused its server-tool turn before completion."});
                    return Ok(response);
                }
                if self.answer == Answer::CompactStream
                    && (converter.stop != "end_turn" || converter.summary().is_empty())
                {
                    return Err(malformed(
                        "the upstream did not complete a continuation summary",
                    ));
                }
                let status = if matches!(
                    value["stop_reason"].as_str(),
                    Some("max_tokens" | "model_context_window_exceeded")
                ) {
                    "incomplete"
                } else {
                    "completed"
                };
                Ok(converter.response(status))
            }
            Direction::Messages => {
                if value["status"] == "failed" || value["status"] == "cancelled" {
                    return Ok(messages_error(&value));
                }
                let output = value
                    .get("output")
                    .and_then(Value::as_array)
                    .ok_or_else(|| malformed("the upstream did not return a Response"))?;
                let mut content = Vec::new();
                let mut tool = false;
                let mut refusal = false;
                for item in output {
                    match item["type"].as_str() {
                        Some("message") => {
                            for part in item
                                .get("content")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                            {
                                match part["type"].as_str() {
                                    Some("output_text") => {
                                        content.push(json!({"type":"text","text":part["text"]}))
                                    }
                                    Some("refusal") => {
                                        refusal = true;
                                        content.push(json!({"type":"text","text":part["refusal"]}));
                                    }
                                    _ => {
                                        return Err(unsupported(
                                            "This response part has no Messages equivalent",
                                        ));
                                    }
                                }
                            }
                        }
                        Some("function_call") => {
                            tool = true;
                            let input: Value =
                                serde_json::from_str(item["arguments"].as_str().unwrap_or("{}"))
                                    .map_err(malformed)?;
                            content.push(json!({"type":"tool_use","id":item["call_id"],"name":item["name"],"input":input}));
                        }
                        Some("reasoning") => {
                            let summary = item
                                .get("summary")
                                .and_then(Value::as_array)
                                .into_iter()
                                .flatten()
                                .map(|part| text(part, "text"))
                                .collect::<Vec<_>>()
                                .join("\n");
                            content.push(json!({"type":"thinking","thinking":summary,"signature":opaque(requests::RESPONSES_STATE,item)}));
                        }
                        _ => {
                            return Err(unsupported(
                                "This response item has no Messages equivalent",
                            ));
                        }
                    }
                }
                let stop = if value["status"] == "incomplete" {
                    "max_tokens"
                } else if tool {
                    "tool_use"
                } else if refusal {
                    "refusal"
                } else {
                    "end_turn"
                };
                Ok(
                    json!({"id":value["id"],"type":"message","role":"assistant","model":value["model"],
                    "content":content,"stop_reason":stop,"stop_sequence":null,"usage":messages_usage(&value["usage"])}),
                )
            }
        }
    }
}

impl ResponseTranslator for Converter {
    fn translate_chunk(&mut self, chunk: Bytes) -> Result<Bytes, TranslateError> {
        if self.terminal {
            if chunk.iter().any(|byte| !byte.is_ascii_whitespace()) {
                // A vendor may send [DONE] after the semantic terminal event.
                return Ok(Bytes::new());
            }
            return Ok(Bytes::new());
        }
        if self.json.is_none() {
            self.undecided.extend_from_slice(&chunk);
            let Some(first) = self
                .undecided
                .iter()
                .find(|byte| !byte.is_ascii_whitespace())
            else {
                return Ok(Bytes::new());
            };
            let is_json = *first == b'{';
            self.json = Some(is_json);
            if is_json {
                self.whole.append(&mut self.undecided);
                return Ok(Bytes::new());
            }
        } else if self.json == Some(true) {
            self.whole.extend_from_slice(&chunk);
            return Ok(Bytes::new());
        }
        let bytes = if self.undecided.is_empty() {
            chunk.to_vec()
        } else {
            std::mem::take(&mut self.undecided)
        };
        let mut out = Vec::new();
        for event in self.parser.push(&bytes) {
            if event.data == "[DONE]" {
                continue;
            }
            let mut value: Value = serde_json::from_str(&event.data).map_err(malformed)?;
            if value.get("type").is_none()
                && let Some(kind) = event.event
            {
                value["type"] = json!(kind);
            }
            match self.direction {
                Direction::Responses => self.messages_event(value, &mut out)?,
                Direction::Messages => self.responses_event(value, &mut out)?,
            }
        }
        Ok(Bytes::from(out))
    }

    fn finish(&mut self) -> Result<Bytes, TranslateError> {
        if self.json == Some(true) {
            let value: Value = serde_json::from_slice(&self.whole).map_err(malformed)?;
            self.whole.clear();
            let result = self.json_answer(value)?;
            self.terminal = true;
            return Ok(Bytes::from(result.to_string()));
        }
        if !self.terminal {
            return Err(malformed(
                "the upstream stream ended before its terminal event",
            ));
        }
        Ok(Bytes::new())
    }
}

fn merge_usage(usage: &mut Value, delta: &Value) {
    if !usage.is_object() {
        *usage = json!({});
    }
    if let Some(delta) = delta.as_object() {
        for (key, value) in delta {
            usage[key] = value.clone();
        }
    }
}

fn count(value: &Value, field: &str) -> u64 {
    value.get(field).and_then(Value::as_u64).unwrap_or(0)
}

fn response_usage(usage: &Value) -> Value {
    let read = count(usage, "cache_read_input_tokens");
    let write = count(usage, "cache_creation_input_tokens");
    let input = count(usage, "input_tokens")
        .saturating_add(read)
        .saturating_add(write);
    let output = count(usage, "output_tokens");
    json!({"input_tokens":input,"output_tokens":output,"total_tokens":input.saturating_add(output),
        "input_tokens_details":{"cached_tokens":read,"cache_creation_tokens":write},
        "output_tokens_details":{"reasoning_tokens":0}})
}

fn messages_usage(usage: &Value) -> Value {
    let read = usage
        .pointer("/input_tokens_details/cached_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    json!({"input_tokens":count(usage,"input_tokens").saturating_sub(read),"output_tokens":count(usage,"output_tokens"),
        "cache_read_input_tokens":read,"cache_creation_input_tokens":0})
}

fn response_error(value: &Value) -> Value {
    let error = value.get("error").unwrap_or(value);
    let kind = match error["type"].as_str() {
        Some("overloaded_error") => "server_error",
        Some("rate_limit_error") => "rate_limit_exceeded",
        Some("authentication_error") => "invalid_api_key",
        Some("permission_error") => "permission_denied",
        Some("invalid_request_error") => "invalid_request_error",
        _ => "server_error",
    };
    json!({"error":{"type":kind,"code":kind,"message":text(error,"message"),"param":null},
        "code":kind,"message":text(error,"message")})
}

fn messages_error(value: &Value) -> Value {
    let error = value.get("error").unwrap_or(value);
    let code = error
        .get("code")
        .or_else(|| error.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let kind = match code {
        "rate_limit_exceeded" | "rate_limit_error" => "rate_limit_error",
        "invalid_api_key" | "authentication_error" => "authentication_error",
        "permission_denied" | "permission_error" => "permission_error",
        "invalid_request_error" => "invalid_request_error",
        "overloaded" | "overloaded_error" => "overloaded_error",
        _ => "api_error",
    };
    json!({"type":"error","error":{"type":kind,"message":text(error,"message")}})
}
