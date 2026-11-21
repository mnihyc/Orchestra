---
id: agent-adapters
title: Build your own agent adapter
section: build-your-own
screens: /resources/agents, /resources/machines
keywords: custom agent, adapter, program, machine, sessions, inputs, neutral events
---

# Build your own agent adapter

An agent can be any coding program with an adapter. The adapter is the program
Orch talks to; it wraps the agent's command line or service and translates its
sessions, inputs and results into the agent contract. Codex and Claude Code are
examples of that interface, not a closed list of supported agents.

## An agent program is a manifest, an adapter and, where it helps, a plugin

Three things make an agent program, and shared code names none of them:

1. A **protocol adapter**: Orch's code for one wire protocol (the Codex app-server, Claude Code's
   stream-json, the scripted one), which implements `AgentAdapter`. A program that speaks a protocol Orch has an
   adapter for needs no code of its own.
2. A **manifest**: data, one for each program (`ProgramManifest` of
   [orch-agent-programs](../../../crates/contracts/agent-programs/README.md)). It holds what is true of the
   program however it is built and before it runs: its kind and names in English and Chinese, which protocol adapter
   drives it, how it is launched, where its builds are published and how each is verified (a GitHub release, an
   npm package, a PyPI project or a manifest its vendor publishes), what a build is checked for (the names of its
   interface, the native cases and the feature a failure switches off), the executors it brings, the files it
   reads and what it is kept from doing unasked. The manifests of the programs Orch ships are files of that crate.
   A program that Orch does not ship sends the same manifest in the `hello` of its adapter plugin (the `manifest`
   member of the descriptor) and is then listed, checked and offered like a shipped one; its kind is shown beside
   its name, and it can never take the kind of a shipped program.
3. A **companion plugin**, for a program that has a plugin system: code that runs inside the program, supplies
   what its protocol lacks and announces each extra, so that the descriptor turns the matching capability on. A
   build whose check shows that an extra no longer works loses that extra and keeps the rest.

The descriptor, by contrast, is what the running adapter declares of one build and what that build's check
proves. A fact is written in the manifest or in the descriptor, never in both: an adapter fills the descriptor's
`harness.name` from the manifest.

The capabilities that used to be branches on a program's name are declared: `session_reload` (`turn` when the
program keeps a directory's files while its session is open), `instruction_root_marker`, `skills` (`none`,
`directory` or `plugin`), `approval_reviewer`, `plan_mechanics`, `usage_context` (`breakdown`, `size_only` or
`none`), and `native_providers` for a program that signs in to several vendors. Auto is offered for a program only
when its manifest lists the conformance case that proves its reviewer and a build passed it; no override can supply
that proof.

The native conformance cases are shared. What differs between programs is said by the adapter in three hooks of
`AgentAdapter`: `native_check`, `mcp_tool_name` and `verify_quiet_start`. The scripts of the cases that have the model call
one of the program's tools say the tool by its role (`command`, `background_command`, `ask_user`, `read_file`,
`spawn_agent`); the manifest's `checks.tool_names` maps each role the program's cases need to the program's own tool, the
names of its arguments and, in `with`, the arguments it needs besides the script's (what makes a command outlive its turn).
A case's `feature` names what a failure switches off for the build: a capability that is a switch, an approval kind, an
approval mode, or `native_input_prefix`.

`cargo xtask names` fails when shared code names a program or a vendor of one.

Use **Add agent → A program on a machine** for a machine-bound profile. The
machine's **Programs** section registers and tests the adapter, reports what it
declares, and supplies the same registration control to the profile dialog.
The adapter's declared modes, formats and sign-ins decide what can be chosen;
installing a program does not claim a capability it has not reported.

Implement [`AgentAdapter`](../../../crates/contracts/agent/README.md) and keep
vendor names and protocol frames inside the implementation. Start, open a
session, accept an input and emit the neutral session catalogue through its
event sink. Each event retains its thread and segment; starts, deltas, completed
items and final turn outcomes must preserve their native correlation ids.
Keep a tool's normalized payload and its original name, arguments and result.
Unknown native items become an `other` call or a raw frame rather than vanish.

Retained image bytes and complete raw outputs use `output_directory`, typed image or
`OutputRef` hashes, and the connector's existing content upload. A native adoption
record uses the same path: its typed output dependencies are retained before the
record is published. Hashes inside opaque tool arguments or results are never file
references. The original temporary record is removed on success, failure or
cancellation; its retained output cache stays available. The thread's authorized
blob reader serves these results to the ordinary file viewer.

An image path reported by a tool in an external environment does not authorize a
read of the adapter's own filesystem. Retain the bytes the native tool actually
returned, with its exact session, turn and call identity. Codex uses the persisted
function result in its managed native history for this, including after a cold
resume, during replay and in adoption records. Its complete original result stays
available through the ordinary raw-output viewer. Missing or changed source data
produces an image-result notice and leaves the model's turn outcome intact.

The descriptor drives the available arrangements, approval and sandbox modes,
parameters, memory placement, sign-in methods, credential forms and model
facts. Declare only a native behavior that the loaded binary and configuration
have proved. A refused daemon attachment, unsupported import or unproved
question form remains unavailable or explicitly fixture-only.

A subscription belongs to its provider. A sign-in method declares the fields
the person supplies and the states the harness reports; the connector seals
secret inputs and carries them to that runtime. Credential issue, sharing and
handover follow the same contract as built-ins. Use a disposable synthetic
credential in deterministic tests; a real subscription run needs its manager's
own sign-in. Keep telemetry, error reporting and self-update disabled at launch.

The optional startup `provider_kind` is the account's reported API/subscription kind, without a
credential. A program that needs a process-wide provider identity can use it before any session
exists. Codex API accounts select one non-vendor identity per account at startup; each session
still overrides that identity's endpoint with its own admitted loopback URL. The startup-only URL
has no valid session key and cannot make an upstream call. Native sign-ins keep their own selection.

Session opens and turns may also carry optional `native_model`: the selected `model`, existing
manager-authored `parameters`, and `capabilities` containing the capability core's exact serialized
per-thread resolution. It is private control data, separate from chosen public `params`. Adapters
that do not use it leave it alone; future native-model members are retained, and the capability
result remains opaque unless an adapter interprets a declared member. It never contains a key,
native token, credential or a separately inferred route. Missing data does not prove support.
The engine captures it at the shared session preparation boundary from that turn's current
model row, composed endpoint/group/model options, inherited capability settings and ordinary
Use answer. Session and turn commands share that capture; a later model selection gets a fresh
one. Private endpoint metadata does not grant visibility of the account or replace admission.
The serialized resolution's `native_config` supplies the program's own setting instructions.
An adapter applies supported switches as supplied and ignores unknown switches; it never recovers
private routes or infers native support from an endpoint's address or a model's spelling.

The core's native instruction map is also part of the existing accepted session
configuration. A changed or removed instruction uses the normal session-opening
path before the next turn; inspector wording, prices and private route metadata
are excluded from this comparison. Codex applies the supplied standalone-search
boolean and remote-compaction protocol (`v2` or `unsupported`) only to the actual
forwarded provider entry. Native sign-ins keep their own provider configuration.
Unknown native controls and future protocol values are ignored.

The session gives standing instructions and its actual directory, model,
parameters, tools, MCP servers and memory location. Instruction files are
workspace files read by the harness. Configuration trust is a separate choice:
do not let an instruction file silently authorize project plugins or hooks.
Orch's own tools sit beside the executor; a new executor cannot replace them.

Run `orch_conformance::agent::run` with an `agent::Fixture` naming a disposable
home, two independent sessions and deterministic turns. The scripted controller
checks event round trips, routing and start/end ordering. Add native tests for
every capability and exact version you offer; command denial must prevent the
effect, questions must be answerable and nested streams must keep their parent.
`scripts/harness-canary/conformance` runs the pinned native cases against a
controlled model with the external network cut. Record any fixture-only cases
instead of certifying them from a captured frame alone.

## Native profile parameter aliases

An adapter can publish `x_profile_parameters` in its existing descriptor extensions. Version `1`
contains `bindings`: each has a JSON `pointer` into `AgentProfile.config`, an existing `ParameterDef`
under `parameter`, and optional exact string aliases under `values`. The declaration stays with the
program's syntax; shared code does not interpret program or vendor names. Native null values remain
omitted. Unknown configuration keys and unknown declaration members are preserved.

Preflight validates these explicit startup values with the same model choices, ranges and provider
`native` schema as ordinary parameters. Managed turn values retain precedence, while the raw startup
values must also satisfy their declared bounds. Descriptor overrides cannot replace or remove the
program's mapping. This mechanism validates configuration and never offers a capability, chooses a
service, changes a default or grants provider access.

## Model requests

Every adapter reports `model.request` through `AdapterEvent::ModelRequest`. Built-ins use the same
event sink and wire representation as an external adapter loaded from the connector configuration or
`ORCH_PLUGINS`. A future command-line harness implements this report without adding a provider-specific
path. The provider's sign-in, API key, program or address never changes these definitions.

The event is one ordinary plugin JSON line. Keep its request id stable within its turn. Send an asked
observation, a first-output observation when available, then one terminal observation with the counts
you have and its outcome. Repeating an identical observation is harmless. Times are UTC timestamps with
millisecond precision; missing times and counts are `null`, never zero standing for “not measured”. A
genuine zero count remains zero. There are no derived durations or speeds in the adapter event.

```json
{"event":"model.request","params":{"thread_id":"00000000-0000-4000-8000-000000000001","segment_id":"00000000-0000-4000-8000-000000000002","turn_id":"00000000-0000-4000-8000-000000000003","request":{"id":"request-1","model":"model-a","asked":"2026-10-15T10:00:00.000Z","first_output":"2026-10-15T10:00:01.200Z","end":"2026-10-15T10:00:03.200Z","tokens":{"input_tokens":100,"cached_input_tokens":0,"cache_write_tokens":null,"output_tokens":160,"reasoning_tokens":40,"total_tokens":260},"reasoning_hidden":true,"output_streamed":true,"outcome":{"status":"done"}}}}
```

For the asked and first-output observations, leave `outcome` and every count null; leave `end` null
until the terminal report. An error's outcome is an object such as:

```json
{"status":"error","code":"provider_busy","message":"The provider is rate limiting requests.","said":"Too many requests; retry shortly."}
```

Use the existing normalized turn-error codes; `said` retains the vendor's original words where supplied,
and is null when there are none. Cancellation uses `forced`; it is visible in the request log and is
excluded from the error numerator. Every request must end with an outcome, even when every time or count
is unmeasured. A failure before output has no first-output boundary. Do not invent a timestamp to make
a request look complete.

Asked means the input given to the harness, or a tool result given back. First output means the first
output event, including visible reasoning or a tool call. End excludes tool execution and human waits.
Set `reasoning_hidden` when thinking was not streamed; report a separate reasoning count only when the
harness gives it. `output_streamed` distinguishes token-level events from a buffered complete reply.
Set `subagent: true` for requests made inside a tool/sub-agent; omitted or false means the main stream.
Claude Code sub-agent messages, for example, have counts but no timed boundaries. Turn model time sums
only timed main-stream requests, and the remaining tool time includes sub-agent work and untimed intervals.

The shared arithmetic computes first-output and request time from those boundaries. Speed uses streamed
output tokens, subtracting hidden reasoning where its count is known. It requires at least 500 ms and
32 streamed tokens; unseparated hidden thinking, shorter bursts and unproven boundaries remain unmeasured.
For the example above the streamed count is 120 and the speed is 60 tok/s. The fake adapter is the reference
producer: it reports observations from its scripted stream, and never calculates a duration or speed.

The adapter conformance suite checks routing, ordered times, nonnegative nullable counts, stable boundaries
and a terminal outcome for every request. It rejects epoch timestamps, a first-output event with zero
output tokens, and streamed output without a first-output boundary; nullable counts remain valid. The pinned built-in adapters also replay sanitized real streams;
Codex native conformance compares its generated notification schema. Re-establish these observations when
upgrading a harness. Without proven boundaries, report unmeasured values rather than plausible numbers.


A quantity-only `TurnUsageReport` sets optional `counters_only: true`. Its token
containers and token allocations are unobserved; the receiver merges model
capability quantities while retaining prior token totals, context and breakdown.
Missing markers preserve the program's ordinary reporting declaration, including
genuine zero-token observations. This uses the same usage event and ledger;
historical replay must not generate another capability charge.

A program that reports a turn's use as what its running totals grew by (so that the use holds the requests its
sub-agents made, which it also reports one by one as request rows) says, with the use, how much of it the rows carry:
`TurnUsageReport.carried_by_rows`, the sum part by part of the figures of exactly the rows whose cost is in that use and
which the turn owns, as they are reported. The engine closes the turn as the use less that figure and counts the rows
whenever they come, so that nothing depends on the order in which the rows and the completion arrive; a row for a
request whose cost the use does not hold is simply added, and so is one that is larger than the use. The statement belongs
to the use it came with: a use said again without it is closed by the engine's other rule, the use less the rows it has
when it closes the turn. A program that states nothing is closed that way, and the use that is shown is the same either way.

A program may declare in its descriptor the profile options that hold back the end of a turn (`x_turn_end_delays`:
the pointer of each option in the profile's configuration, what the program waits besides it, and the value the option
has when the profile does not set it). The engine honours a value the profile sets or refuses the profile, naming the
option and `[engine] interrupt_force_secs`; it refuses only when the value itself does not fit under that time, and
never where what the program waits besides is already all of it. An option the profile does not set takes its default,
or less where the time leaves less. A declaration that cannot be read, or is of another version, declares nothing.

Known limits of the Claude Code adapter's accounting of requests. The program reports a turn's use as the growth of its running
totals, and the requests of its sub-agents are reported one by one; a turn states with its use how much of it its rows
carry (its own requests of the main conversation, whose figures are final when the result comes, and the sub-agents' requests
that the totals of its window name), and the engine closes it as the use less that figure. Two things are known and not
changed:
- Reasoning. The transcript of a sub-agent does not count the reasoning of a message apart, so the row of such a request
  carries none, while the use of the window that holds it does (it is a part of the output). A part cannot be taken out of
  a use by a row that does not carry it, so the reasoning stays in the turn's remainder, clamped to the output that is left;
  the total, input and output, is conserved whatever the parts.
- A request of the main conversation that did not end with its stop reason. Two causes were measured on the pinned
  program with a mock model server (`runs/program_interrupt_mid_stream.txt`, `runs/program_dropped_stream.txt`).
  A stop while a request is streamed: the program prints the message so far, a message_delta with the count it had
  started with (one token, no stop reason) and the end of the message, then a result that is an error
  (`aborted_streaming`) whose use and totals hold the requests that had ended and not the one that was cut off (a
  request of 1 200 input was in neither). A stream dropped by the network: the program writes the start of the attempt
  and the end of the message, retries, and its totals after three attempts of 1 200 input held 2 400. So whether the
  program's totals hold such a request depends on the cause, and the adapter does not guess it from the row. It reports
  the request as a row, with the input the program gave it and the output placeholder, ended in error, because the
  provider charges the input it was given and the output it made; the engine counts it, so the ledger then holds
  more than the program's own totals, which is the charge. The statement carries the main conversation's rows up to
  the use's own tally of the main conversation (the result's `usage`, class by class) and no more: the use never
  holds more of them than that, and a statement that said more would clamp the remainder to nothing and swallow
  what the use holds that no row carries. The output of such a row is the placeholder, not what the provider
  generated. A sub-agent's message cut off by a stop is not known: it is printed whole, with no stop reason as every
  printed message has, and settled from its transcript; if no window's totals name it, it is no row.
- Requests whose window cannot be decided (a side call, a shortening of the conversation, ambiguous sums, a message the
  program was still writing when it was lost) are not rows: the growth that holds them counted them once, in the remainder
  of its turn. What is lost is the line of the request, and, for a background sub-agent, the attribution to its origin.


## Shipped feature states

An agent descriptor declares optional features in `features`, a map from the program's feature IDs
to booleans, for example `{"example.extra_tools": true}`. A parameter's `feature` references one
of those IDs. The program's declaration supplies the shipped state; explicit `agent.features`
settings are laid over it, including `false`. No program name belongs in the shared settings default.
The same resolution governs native session options, parameter offers, recovery candidates and
place-default forms. The settings switches show the declared state and the source of an override.
