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
one of the program's tools say the tool by its role (`command`, `ask_user`, `read_file`, `spawn_agent`); the manifest's
`checks.tool_names` maps each role the program's cases need to the program's own tool and the names of its arguments.

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
