# Harness compatibility corpora

One corpus format and one replay helper serve every harness program. A build lives
at `<kind>/<version>/`, with `expected.json` and one `<scenario>.jsonl` for each
supported scenario. The authoritative Rust types and required scenario names are
`orch_testkit::harness_corpus::{Manifest, Scenario, Frame, SCENARIOS}`. A fourth
program uses those same types and its own adapter; it adds no private replay engine.

[Scripted scenarios](FAKE-SCENARIOS.md) maps every required case to the fake
adapter's native directive or lifecycle method. [Version controls](VERSION-CONTROLS.md)
lists the shared selector, cache verification, native check, reviewed update and
rollback controls; the corpus runner does not implement another version policy.

Tests are offline. They never start a vendor binary or contact a model. A corpus
feeds the program-specific adapter's existing public mapper or launcher pipes and
control actor, then compares its real neutral events, facts and errors. Native
control requests, approvals and settings are not ordinary mapper notifications.
Run them through the existing conversation/control actor or fake byte-stream
launcher. A mapper-only test cannot claim that it exercised those actors.

## Expected results and provenance

`expected.json` is `Manifest`, format `1`. Its members are `program`, `version`,
`evidence`, `scenarios` and optional `managed_pins` (default `[]`). Evidence is one of:

```json
{"kind":"recorded","references":["recording identity and checksum"]}
```

```json
{"kind":"published_protocol","references":["immutable schema source URL"],"package":{"name":"vendor-sdk","version":"exact package version","sha256":"64 hexadecimal digits of the source package"}}
```

```json
{"kind":"synthetic","reason":"A shared helper test, not a vendor recording"}
```

Recorded frames remain anchored to their actual recording and program build.
Written fixtures derived from published SDK/protocol schemas say so, including
the source package's exact version and checksum. A recording from an older build
is never relabelled as a newer native capture. Sources may include an immutable
schema bundle and a recording reference when both informed the evidence.

Every name in `SCENARIOS` has one explicit support decision. A supported entry is:

```json
{"status":"supported","events":[{"name":"item.completed","body":{"payload":{"code":"harness_diverged"}},"subset":true}],"quiet":false,"ignore_events":[]}
```

An unsupported entry is `{"status":"unsupported","reason":"The actual capability the program lacks"}`.
An absent scenario, empty excuse, unlisted JSONL file or fixture attached to an
unsupported scenario fails loading. The six tolerance scenarios cannot be
unsupported or quiet. A supported operation that intentionally emits no event,
such as some Close implementations, must explicitly say `quiet:true`; its actual
return can still be observed as `adapter.result`.

Events compare exact order and count. `body` is exact JSON unless `subset:true`
explicitly selects stable object members; arrays remain exact in order and count.
Missing and null differ. `ignore_events` names individually irrelevant kinds,
never a wildcard or an expected marker/error. Raw logging being disabled is not
permission to ignore compatibility observations.

Each managed pin is `{"name":"exact config key or variable","value":false,"divergence_scenario":"a supported scenario"}`.
The referenced scenario must assert an actual `harness_diverged` completed error,
not its name in prose or a raw frame. The program's own pins test checks each key
against that build's actual schema or launch interface. Schema comparison, build
selection and cache admission use the existing program mechanisms, not a second
version policy inside this loader.

## Frames in both directions

Each JSONL row is `Frame`, from the adapter's viewpoint:

```jsonl
{"direction":"to_harness","frame":{"jsonrpc":"2.0","id":{"$replay_capture":"init"},"method":"initialize"},"subset":true}
{"direction":"from_harness","frame":{"jsonrpc":"2.0","id":{"$replay_ref":"init"},"result":{"protocolVersion":1}}}
{"direction":"tick","milliseconds":17}
{"direction":"stderr","line":"synthetic diagnostic"}
{"direction":"raw","line":"{broken","newline":false}
{"direction":"exit","code":7}
```

`to_harness` reads and checks a real adapter request. `from_harness` writes the
complete fixture response/notification. Raw output is for malformed protocol text
or a version command; newline defaults to true, while false can leave a partial
line before process loss. Exit shuts the output stream and returns the actual
scripted code for the launcher's process-control wrapper. No step follows Exit.

`{"$replay_capture":"name"}` captures one actual native scalar identifier;
`{"$replay_ref":"name"}` reuses it. Binding is consistent, not a per-comparison
wildcard. Only the helper's known native identity, clock and launcher-location
fields may use these symbols. Messages, commands, model names, settings and tool
results are not normalized or rewritten. In particular a real JSON Schema `$ref`
remains literal: it is not a replay symbol. A test may seed known request identities
or its own temporary launcher locations into `Bindings` before replay.
The native `codexHome` field is a launcher location and may reference that owned
temporary home; the helper never reads a person's installed home directory.

Recorded timing is removed. The adapter receives the same `ManualClock` as replay;
Tick advances logical observation time. Native completion/acknowledgement barriers
decide when a case has finished. A caller-supplied timeout is only a deadlock guard,
never a saved recording delay or a product latency allowance. Launcher streams
remain owned by the fixture so finishing a script does not accidentally turn a
successful turn into a process crash; only Exit represents scripted process loss.

## Shared entry points

- `Corpus::load(directory)` validates and loads the complete build corpus.
- `fixture_root()` finds this directory independent of the current directory.
- `replay_io(frames, reader, writer, stderr, clock, bindings, deadline)` drives a
  launcher's native byte streams, checks requests and returns `Transcript` with
  actual protocol frames and optional process exit. Use a bounded duplex pipe;
  its capacity supplies backpressure and does not limit a frame's size.
- `Observation::from_session(&SessionEvent)` captures a public mapper's native output.
- `Observation::from_event(&AdapterEvent)` captures the actual adapter event stream,
  preserving all body members including scope identities.
- `Observation::from_result(method, &Result<T, AdapterError>)` captures actual call
  results, startup facts and canonical error code/detail/retryability.
- `collect_through(stream, terminal_kind, deadline)` collects the real stream through
  an actual native barrier. Use the actor's own request/reply barrier where several
  events share that name; do not replace it with an arbitrary quiet sleep.
- `Scenario::assert_observed(rows, bindings)` checks the final complete sequence
  after native completion, including extra and missing events.
- `assert_unrecognized(rows, program, event, count)` refuses silent drops, even for
  idle sessions and with raw logging off.

`orch_agent_contract::observations::unrecognized_event` reports the shared passive,
counted `HarnessChanged` observation (`change="unrecognized_event"`). The existing
timeline says “Unrecognized <program> event <name>”. No unknown body is copied into
history and no action follows. `generic_approval` preserves real native answer
options in the existing Other card. When no option is known it offers only
`CANCEL_UNRECOGNIZED_APPROVAL`; the actor must map that to native cancellation/error,
never invent an allow response or leave the request waiting forever.

## Required scenario families

The exact spellings are the `SCENARIOS` constant. Every corpus accounts for:

- Start, resume and fork; close and unload.
- Command, file change, read, search, web search/fetch, MCP, subagent and plan tools.
- Command, file, permissions, plan, MCP and unknown approvals, checkpoints,
  prompt-user rules, network approvals and user-input requests.
- Interrupt and steer; automatic/manual/marker-only/idle compaction.
- Usage/rate limits, overloaded, expired/revoked sign-in, context overflow,
  broken stream and process exit.
- Background End/wake, subagents, MCP startup/failure/elicitation, hooks,
  model reroute and settings changed by the program/server.
- Every pinned automatic behavior arriving anyway: `harness_diverged`, no act.
- Unknown fields, unknown event/method/enum, reordered/missing optional fields,
  and malformed frame text. No crash, no silent event drop or approval wait.

Version/update controls additionally cover Shipped, Newest checked, Newest and
Exact, bad checksums, untested warnings, reviewed update/downgrade/rollback,
mixed machine versions and the check when a build enters the existing tool cache.
Use the existing fake harness directives and real lifecycle/control methods to
exercise the engine's handling; do not synthesize a new session by injecting an
unrelated event into an active turn.
