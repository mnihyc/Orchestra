# Plain-text translator example

[translator.py](translator.py) is a plain Python 3 plugin, using only the standard
library. It converts Anthropic Messages requests to OpenAI Responses and converts
Responses answers back to Messages. It is a small example, not an installed or
offered translator. Nothing registers it automatically.

Run `python3 translator.py` and send one JSON object per line on standard input.
Every answer on standard output is one JSON object with the same `id`. For example:

```json
{"id":0,"method":"hello","params":{"kind":"translator","transport":[1,1],"contract":[1,1],"host_version":"0.1.0"}}
{"id":1,"method":"describe","params":{}}
```

The descriptor declares `from: anthropic-messages`, `to: openai-responses`, a
title, summary and these fidelity limits:

| Preserved | Approximated | Dropped |
|-----------|--------------|---------|
| System, user and assistant text, ordered text deltas, streaming, reported input/cached-input/output usage | Text block boundaries collapse; completed → `end_turn`, output limit → `max_tokens`, other incomplete reasons → `end_turn` | Tools and their calls/results, images and other non-text blocks, reasoning/thinking, cache control/cache-write details, other request options such as stop sequences, `top_p`, metadata |

`translate_request` receives `path: /messages`, `body` as JSON **text**, and
`upstream`. It produces `POST /responses` below the upstream base (for example
`https://upstream.example/v1`), with ordered text messages, `instructions` from
system text, `max_output_tokens`, `temperature`, `stream` and always `store: false`.
The host's Responses implementation presents the credential: `authorized` is
false and the example never reads or copies `upstream.credential`.

`response_translator` allocates a converter ID. `translate_chunk` accepts that ID
and a text chunk, emitting complete Messages SSE events as Responses text events
arrive. Each converter buffers incomplete lines/events independently; a chunk may
split any SSE delimiter, and the host bridge handles split UTF-8 characters. Both
LF and CRLF work. Metadata/tool/thinking events are ignored. A non-streamed text
JSON answer is converted at finish, so `stream: false` is also useful.

Final usage uses `input_tokens - cached_tokens` for Messages `input_tokens`,
`cached_tokens` for `cache_read_input_tokens`, and the reported `output_tokens`.
The final `message_delta` carries all three figures; no text-length estimate is
made. A truncated, failed or malformed stream is refused rather than completed
successfully. This example retains at most 1,048,576 characters of one event or
non-streamed answer. `finish_response` releases the converter even on a refusal;
another response always has a different converter. `ping`, `cancel`, unknown
methods and `shutdown` follow the common plugin transport.

The local calls are documented in `crates/contracts/model/src/translator.rs`;
`crates/platform/plugin/src/bridges/translator.rs` is the host bridge. The example
test in `crates/platform/conformance/tests/plugins.rs` starts this actual program
and supplies both formats to the existing translator suite, including a UTF-8
stream one byte at a time. No network or provider credential is used by that test.
