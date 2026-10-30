//! Plugin protocol and lifecycle fault injection.
//!
//! A tiny deterministic summarizer program for checks of the real JSON-lines transport and CLI status.
//! Its one deliberate fault accepts an unknown method, which must fail transport.unknown_method.

use std::path::Path;

/// Writes a Python plugin counterpart at `path`, accepting unknown methods only when `careless` is true.
pub fn write_summarizer(path: &Path, careless: bool) -> std::io::Result<()> {
    let script = SCRIPT.replace(
        "CARELESS = False",
        if careless {
            "CARELESS = True"
        } else {
            "CARELESS = False"
        },
    );
    std::fs::write(path, script)
}

/// Writes a controlled Responses-to-Messages translator that converts JSON and streamed text/tools/usage.
pub fn write_translator(path: &Path) -> std::io::Result<()> {
    std::fs::write(path, TRANSLATOR)
}

const TRANSLATOR: &str = r#"import json, sys
descriptor = {"from":"openai-responses", "to":"anthropic-messages", "title":"Controlled translator",
              "summary":"A translator for protocol-path tests.", "losses":{"preserved":["text", "tools", "usage", "errors"]},
              "endpoint_options":[], "credential_fields":[]}
converters = {}
def usage(raw):
    cached = raw.get("cache_read_input_tokens", 0)
    written = raw.get("cache_creation_input_tokens", 0)
    return {"input_tokens":raw.get("input_tokens", 0) + cached + written,
            "output_tokens":raw.get("output_tokens", 0),
            "input_tokens_details":{"cached_tokens":cached, "cache_write_tokens":written}}
def response(message):
    if message.get("type") == "error": return {"error":message.get("error", {})}
    output = []
    for block in message.get("content", []):
        if block.get("type") == "text":
            output.append({"type":"message", "role":"assistant", "content":[{"type":"output_text", "text":block["text"]}]})
        if block.get("type") == "tool_use":
            output.append({"type":"function_call", "call_id":block["id"], "name":block["name"], "arguments":json.dumps(block.get("input", {}))})
    return {"id":message.get("id", "controlled"), "model":message.get("model"), "status":"completed",
            "output":output, "usage":usage(message.get("usage", {}))}
def event(kind, **parts):
    return "event: " + kind + "\ndata: " + json.dumps(dict(type=kind, **parts)) + "\n\n"
def streamed(state, item):
    kind = item.get("type")
    if kind == "message_start": state["message"] = item["message"]
    if kind == "content_block_start":
        block = item["content_block"]
        if block.get("type") == "tool_use":
            return event("response.output_item.added", item={"type":"function_call", "call_id":block["id"], "name":block["name"], "arguments":""})
    if kind == "content_block_delta":
        delta = item["delta"]
        if delta.get("type") == "text_delta": return event("response.output_text.delta", delta=delta["text"])
        if delta.get("type") == "input_json_delta": return event("response.function_call_arguments.delta", delta=delta["partial_json"])
    if kind == "message_delta": state["message"].setdefault("usage", {}).update(item.get("usage", {}))
    if kind == "message_stop":
        state["finished"] = True
        return event("response.completed", response=response(state["message"]))
    if kind == "error": return event("error", error=item.get("error", {}))
    return ""
for line in sys.stdin:
    request = json.loads(line)
    name = request.get("method")
    if "id" not in request:
        if name == "shutdown": break
        continue
    p = request.get("params") or {}
    error = None
    try:
        if name == "hello":
            result = {"kind":"translator", "transport":1, "contract":1, "name":"controlled-translator", "version":"1",
                      "methods":["describe", "translate_request", "response_translator", "translate_chunk", "finish_response"], "descriptor":descriptor}
        elif name == "describe": result = descriptor
        elif name == "ping": result = {}
        elif name == "translate_request":
            body = json.loads(p["body"])
            content = body.get("input", "")
            messages = [{"role":"user", "content":content}] if isinstance(content, str) else content
            sent = {"model":body["model"], "messages":messages, "system":body.get("instructions", ""),
                    "max_tokens":body.get("max_output_tokens", 120), "stream":body.get("stream", False)}
            if body.get("tools"):
                sent["tools"] = [{"name":t["name"], "description":t.get("description", ""), "input_schema":t.get("parameters", {})} for t in body["tools"]]
            result = {"method":"POST", "path":"/v1/messages", "query":{}, "headers":{"content-type":"application/json"},
                      "body":json.dumps(sent), "authorized":False}
        elif name == "response_translator":
            key = str(len(converters))
            converters[key] = {"buffer":"", "message":{}, "finished":False}
            result = {"converter":key}
        elif name == "translate_chunk":
            state = converters[p["converter"]]
            state["buffer"] += p["chunk"]
            output = ""
            while "\n\n" in state["buffer"]:
                record, state["buffer"] = state["buffer"].split("\n\n", 1)
                data = "\n".join(line[5:].strip() for line in record.splitlines() if line.startswith("data:"))
                if data and data != "[DONE]": output += streamed(state, json.loads(data))
            result = {"chunk":output}
        elif name == "finish_response":
            state = converters.pop(p["converter"])
            result = {"chunk":json.dumps(response(json.loads(state["buffer"]))) if state["buffer"].strip().startswith("{") else ""}
        else: error = {"code":"unsupported", "message":"unknown method", "retryable":False}
    except (ValueError, KeyError, TypeError): error = {"code":"malformed", "message":"malformed test request", "retryable":False}
    print(json.dumps({"id":request["id"], "error":error} if error else {"id":request["id"], "result":result}), flush=True)
"#;

const SCRIPT: &str = r#"import json, sys
CARELESS = False
descriptor = {"name":"counterpart", "title":"Controlled summarizer", "config_schema":{}, "fields":["description"]}
for line in sys.stdin:
    request = json.loads(line)
    name = request.get("method")
    if "id" not in request:
        if name == "shutdown": break
        continue
    params = request.get("params") or {}
    error = None
    if name == "hello":
        result = {"kind":"summarizer", "transport":1, "contract":1, "name":"counterpart", "version":"1",
                  "methods":["describe","summarize"], "descriptor":descriptor}
    elif name == "describe": result = descriptor
    elif name == "ping": result = {}
    elif name == "summarize":
        if params.get("field") != "description": error = {"code":"unsupported", "message":"field not declared", "retryable":False}
        else: result = {"text":"Controlled summary", "tags":None, "usage":None}
    elif CARELESS: result = {}
    else: error = {"code":"unsupported", "message":"unknown method", "retryable":False}
    reply = {"id":request["id"], "error":error} if error else {"id":request["id"], "result":result}
    print(json.dumps(reply), flush=True)
"#;
