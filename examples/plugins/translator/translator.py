#!/usr/bin/env python3
"""A plain-text Anthropic Messages -> Responses translator on Orch's local plugin channel.

No HTTP client, credential handling or third-party module is needed. Requests go to the host's
Responses wire implementation; each response gets its own converter, freed by finish_response.
Only JSON transport replies go to stdout. See README.md for the deliberate conversion losses.
"""

import json
import math
import sys

TRANSPORT_VERSION = 1
CONTRACT_VERSION = 1
NAME = "messages-text"
VERSION = "0.1.0"
METHODS = ["describe", "translate_request", "response_translator", "translate_chunk", "finish_response"]
DESCRIPTOR = {
    "from": "anthropic-messages",
    "to": "openai-responses",
    "title": "Messages to Responses, text only",
    "summary": "An example converting plain text requests and streamed text answers with token usage.",
    "losses": {
        "preserved": ["System, user and assistant text in order.", "Text deltas and streaming.",
                      "Reported input, cached input and output token counts."],
        "approximated": ["Text block boundaries are collapsed into one string.",
                         "Stop reasons: completed becomes end_turn; the output limit becomes max_tokens; other incomplete reasons become end_turn."],
        "dropped": ["Tool definitions, choices, calls and results.", "Images and other non-text content.",
                    "Reasoning and thinking blocks and options.", "Cache control and cache-write details.",
                    "Other request options, including stop sequences, top_p and metadata."],
    },
    "endpoint_options": [],
    "credential_fields": [],
}

# A broken or unterminated stream must not accumulate an unlimited event. This small example
# retains at most this many characters of one SSE record or a non-streamed JSON answer.
MAX_BUFFER = 1024 * 1024


class Refusal(Exception):
    def __init__(self, code, message):
        super().__init__(message)
        self.code, self.message = code, message


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False)


def text_only(content):
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        raise Refusal("malformed", "message content must be text or a list of blocks")
    # Non-text blocks are deliberately dropped, as the descriptor tells the person choosing it.
    return "".join(block["text"] for block in content
                   if isinstance(block, dict) and block.get("type") == "text")


def translate_request(params):
    if params["path"] != "/messages":
        raise Refusal("unsupported", "this example translates only POST /messages")
    body = json.loads(params["body"])
    if not isinstance(body, dict) or not isinstance(body.get("model"), str) or not body["model"]:
        raise Refusal("malformed", "a Messages request needs a model")
    maximum = body.get("max_tokens")
    if type(maximum) is not int or maximum <= 0:
        raise Refusal("malformed", "max_tokens must be a positive integer")
    if not isinstance(body.get("messages"), list):
        raise Refusal("malformed", "messages must be a list")
    messages = []
    for message in body["messages"]:
        if message["role"] not in ("user", "assistant"):
            raise Refusal("unsupported", "this example accepts user and assistant messages only")
        text = text_only(message["content"])
        if text:
            messages.append({"role": message["role"], "content": text})
    if not messages:
        raise Refusal("unsupported", "no plain-text message remains after declared losses")
    stream = body.get("stream", False)
    if not isinstance(stream, bool):
        raise Refusal("malformed", "stream must be a boolean")
    converted = {"model": body["model"], "input": messages, "max_output_tokens": maximum,
                 "stream": stream, "store": False}
    if "system" in body:
        converted["instructions"] = text_only(body["system"])
    if "temperature" in body:
        temperature = body["temperature"]
        if type(temperature) not in (int, float) or not math.isfinite(temperature) or not 0 <= temperature <= 1:
            raise Refusal("malformed", "temperature must be between zero and one")
        converted["temperature"] = temperature
    # Responses is implemented by the host, so its wire implementation alone presents the credential.
    return {"method": "POST", "path": "/responses", "query": {},
            "headers": {"content-type": "application/json"}, "body": encoded(converted), "authorized": False}


def usage_of(response):
    usage = response["usage"]
    total, output = usage["input_tokens"], usage["output_tokens"]
    cached = (usage.get("input_tokens_details") or {}).get("cached_tokens", 0)
    if any(type(value) is not int or value < 0 for value in (total, cached, output)) or cached > total:
        raise Refusal("malformed", "the Responses usage counts are invalid")
    # Responses includes cached input in input_tokens; Messages lists the two disjoint counts.
    return {"input_tokens": total - cached, "cache_read_input_tokens": cached, "output_tokens": output}


def stop_of(response):
    status = response["status"]
    if response.get("error") or status not in ("completed", "incomplete"):
        raise Refusal("malformed", "the upstream response did not complete")
    reason = (response.get("incomplete_details") or {}).get("reason")
    return "max_tokens" if status == "incomplete" and reason == "max_output_tokens" else "end_turn"


def message_of(response, content, stop, usage):
    if not isinstance(response["id"], str) or not isinstance(response["model"], str):
        raise Refusal("malformed", "the response needs its id and model")
    return {"id": response["id"], "type": "message", "role": "assistant", "model": response["model"],
            "content": content, "stop_reason": stop, "stop_sequence": None, "usage": usage}


def event(kind, **members):
    return "event: " + kind + "\ndata: " + encoded({"type": kind, **members}) + "\n\n"


class Converter:
    def __init__(self):
        self.buffer, self.data, self.mode = "", [], None
        self.data_size, self.identity = 0, None
        self.started, self.block, self.terminal = False, False, False

    def start(self, response):
        identity = (response["id"], response["model"])
        if self.started:
            if self.identity != identity:
                raise Refusal("malformed", "the response identity changed inside one stream")
            return ""
        self.identity = identity
        self.started = True
        usage = usage_of(response) if response.get("usage") else {
            "input_tokens": 0, "cache_read_input_tokens": 0, "output_tokens": 0}
        return event("message_start", message=message_of(response, [], None, usage))

    def sse(self, data):
        if data == "[DONE]":
            if not self.terminal:
                raise Refusal("malformed", "the stream ended without a final response and usage")
            return ""
        part = json.loads(data)
        if not isinstance(part, dict):
            raise Refusal("malformed", "an SSE data record must be a Responses event")
        kind = part["type"]
        if kind == "response.created":
            return self.start(part["response"])
        if kind == "response.output_text.delta":
            if not self.started or self.terminal or not isinstance(part["delta"], str):
                raise Refusal("malformed", "a text delta needs a current response")
            out = ""
            if not self.block:
                self.block = True
                out = event("content_block_start", index=0, content_block={"type": "text", "text": ""})
            return out + event("content_block_delta", index=0, delta={"type": "text_delta", "text": part["delta"]})
        if kind in ("response.completed", "response.incomplete"):
            if self.terminal:
                raise Refusal("malformed", "the stream has more than one final response")
            response = part["response"]
            stop, usage = stop_of(response), usage_of(response)
            out = self.start(response)
            if self.block:
                out += event("content_block_stop", index=0)
            self.terminal = True
            return out + event("message_delta", delta={"stop_reason": stop, "stop_sequence": None}, usage=usage) + event("message_stop")
        if kind in ("error", "response.failed"):
            raise Refusal("malformed", "the upstream stream failed")
        # Lifecycle metadata and all non-text/tool/reasoning events are outside this example's output.
        return ""

    def feed(self, chunk):
        self.buffer += chunk
        if self.mode is None:
            first = self.buffer.lstrip()
            if first:
                self.mode = "json" if first.startswith("{") else "sse"
        out = ""
        if self.mode == "sse":
            while "\n" in self.buffer:
                line, self.buffer = self.buffer.split("\n", 1)
                if line.endswith("\r"):
                    line = line[:-1]
                if len(line) > MAX_BUFFER:
                    raise Refusal("unsupported", "the example's event buffer is full")
                if not line:
                    if self.data:
                        data, self.data = "\n".join(self.data), []
                        self.data_size = 0
                        out += self.sse(data)
                elif line.startswith("data:"):
                    self.data.append(line[5:].lstrip(" "))
                    self.data_size += len(self.data[-1]) + 1
                    if self.data_size > MAX_BUFFER:
                        raise Refusal("unsupported", "the example's event buffer is full")
        if len(self.buffer) + self.data_size > MAX_BUFFER:
            raise Refusal("unsupported", "the example's response buffer is full")
        return out

    def finish(self):
        if self.mode == "json":
            response = json.loads(self.buffer)
            if not isinstance(response, dict):
                raise Refusal("malformed", "the response must be a Responses object")
            text = "".join(block["text"] for item in response.get("output", []) if item.get("type") == "message"
                           for block in item.get("content", []) if block.get("type") == "output_text")
            return encoded(message_of(response, [{"type": "text", "text": text}], stop_of(response), usage_of(response)))
        if self.buffer.strip() or self.data or not self.terminal:
            raise Refusal("malformed", "the response ended before a complete final SSE record")
        return ""


converters = {}
next_converter = 0


def response_translator(_params):
    global next_converter
    next_converter += 1
    key = str(next_converter)
    converters[key] = Converter()
    return {"converter": key}


def convert(params, finish=False):
    key = params["converter"]
    current = converters.pop(key, None) if finish else converters.get(key)
    if current is None:
        raise Refusal("malformed", "unknown or finished response converter")
    try:
        return {"chunk": current.finish() if finish else current.feed(params["chunk"])}
    except (Refusal, ValueError, TypeError, KeyError, AttributeError):
        converters.pop(key, None)
        raise


def hello(params):
    transport = min(params["transport"][1], TRANSPORT_VERSION)
    contract = min(params["contract"][1], CONTRACT_VERSION)
    if transport < params["transport"][0] or contract < params["contract"][0]:
        raise Refusal("unsupported_version", "this program speaks transport 1 and contract 1")
    return {"kind": "translator", "transport": transport, "contract": contract, "name": NAME,
            "version": VERSION, "methods": METHODS, "descriptor": DESCRIPTOR}


def main():
    methods = {"hello": hello, "describe": lambda params: DESCRIPTOR, "translate_request": translate_request,
               "response_translator": response_translator, "translate_chunk": convert,
               "finish_response": lambda params: convert(params, finish=True), "ping": lambda params: {}}
    for raw in sys.stdin:
        try:
            request = json.loads(raw)
        except ValueError:
            continue
        if not isinstance(request, dict):
            continue
        name = request.get("method")
        if "id" not in request:
            if name == "shutdown":
                return
            continue  # Synchronous calls finish at once; cancel has no outstanding work to stop.
        try:
            if name not in methods:
                raise Refusal("unsupported", "unknown method")
            result = methods[name](request.get("params") or {})
            reply = {"id": request["id"], "result": result}
        except (ValueError, TypeError, KeyError, AttributeError):
            reply = {"id": request["id"], "error": {
                "code": "malformed", "message": "invalid translator parameters or JSON", "retryable": False}}
        except Refusal as refusal:
            reply = {"id": request["id"], "error": {
                "code": refusal.code, "message": refusal.message, "retryable": False}}
        sys.stdout.write(encoded(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
