# Native Claude Code 2.1.319 recordings

These eight captures came from the production raw-frame observer of an isolated native binary on
2026-11-08, using a scripted model and disposable program home. Each fixture names its original
recording's SHA256. They are not SDK examples or reconstructed protocol events.

`default` and `disabled` identify whether the feature-flags-off override was requested by the test.
Both captured settings replies report `CLAUDE_CODE_DISABLE_FEATURE_FLAGS=1`; these captures therefore
do not establish behavior with native feature flags enabled. Managed-settings qualification has its
own conformance test; this replay suite does not replace it.

| Case, in each requested mode | Frames, both directions | Incoming frames | Observed behavior |
|---|---:|---:|---|
|startup|20|17|Init, requesting status, echoed input, streamed and complete assistant text|
|tools.result_form|36|31|Connected MCP at init; two tool calls, text and structured replies|
|parallel_calls|43|40|Two distinct Read calls, inputs and results|
|image_result|32|29|Read result with the actual inline PNG bytes|

Every frame and unknown member remains. Transient workspace/socket paths, UUIDs, timestamps and local
ports are anonymized consistently. Workspace aliases have the original length, so path fragments keep
their original input-JSON chunk boundaries. No native kind, ordering, model, mode, counter, text, result
form or image byte was invented or removed. Only incoming frames enter the mapper; outgoing requests
remain evidence of direction and are never replayed as harness output.

The observation window ends before the native terminal `result` frame is consumed by the recorder.
The replay must not manufacture turn completion. Older failure recordings retain their original
version and separate coverage.

These captures contain no compaction, retry, hook, rate-limit, background-task, native resume/import,
post-init MCP status/failure, elicitation-request, actual model-change or auto-review frame. A connected
server in init, an outgoing control response or a changed test-side expected model proves none of
those transitions. The candidate-only auto-classifier test remains ignored pending its real
subscription qualification and explicit manifest activation; this corpus cannot activate it.

## A background sub-agent (`default/child_usage.json`)

One more capture of the same kind, taken on 2026-11-11: a turn that starts a background sub-agent making two model
requests (inputs 70 and 90, outputs 10 and 10 as the scripted model states them). The turn ends between the two
requests, and the sub-agent ends five seconds later. It holds the frames that bear on usage: the stream of the main
turn, the sub-agent's printed messages (`assistant` with `parent_tool_use_id` and `agent_id`, each with the output
count it started with, 1, and no stop reason), the task frames, and the sub-agent's end. `child_usage.transcript.jsonl`
is the sub-agent's own transcript (`projects/<directory>/<session>/subagents/agent-<agent id>.jsonl`) from the same run,
without the lines that carry no message: it holds both messages with their stop reasons and the usage the model closed
them with.

The program wrote its transcript 48 to 144 ms after it printed each message in three runs, and the line of the last
message 1 to 47 ms after the frame that said the sub-agent ended (the capture of the timing is not kept: it came from
the daemon's timestamps of the stdout lines and the modification times of the file). Left out: control requests and
responses, command lifecycle frames, the lists of the first system frame (slash commands, skills, plugins,
capabilities), and everything of the second turn after the sub-agent ended. The workspace alias has the original length,
so the partial-json pieces of the sub-agent's call keep their boundaries; the user's id in the program's temporary
directory name is replaced by `uid`.

Both messages of this capture have one content block, so the capture shows nothing of a message of several blocks. The
program's source treats the entries of a message that are not the last as `stop_reason === null`, and the older
`adopt/session.jsonl` fixture has that shape (a text line without a stop reason, then the call's line with it); the
cases of `tests/child_usage.rs` for a message of several blocks are built on that shape, and are constructed, not
recorded. The shape of the pinned program is checked in the image by
`a_sub_agent_message_of_text_and_a_call_is_settled_from_the_last_line_of_its_transcript` of `tests/turns.rs`: the
sub-agent of that case says a few words and then makes a call in one message, and the case fails with the lines the
program wrote if they are not as assumed.
