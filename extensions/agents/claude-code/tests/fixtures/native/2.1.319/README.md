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
