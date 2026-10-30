# orch-adapter-fake

Scripted adapter for product behavior exercises and UI development.

Implemented. The adapter of kind `fake` drives no harness: it answers an input as a script says, with the same neutral
events, in the same order and at the same pace as a harness would, so that the product can be exercised end to end, and
an interface developed, without a model. It is exact and deterministic. Every directive of its table is built (a
message, reasoning, a plan, a command, an edit, an MCP call, a question to a person, a sub-agent, one directive for
each of Orch's tools, notes, usage, the failures a provider has, a lost session and an expired credential), beside a
few that make what reached it visible (the model, the mode, the parameters, the directory, the notes it loaded) and
that compact, title and crash. Sessions are files of its home, so they persist, resume, fork, export (native and
neutral), import and say themselves again. Its capabilities, arrangements, credential forms, sign-in methods and
failures are set by the `config` of the agent profile. The crate documentation is the manual of the directives, and
the tests run each of them through the adapter's own interface in paused time, so that the pauses cost nothing and the
moments the events come at are exact. A turn on the forwarded path makes no model call, and `run` and `mcp` are always
simulated, until the connector has the endpoints for them.

MCP server changes use the next-turn resume path with the same conversation identity and transcript. Live MCP reload is false, including in configurable profiles: the adapter does not implement a live reload operation.

**Model requests.** Script parsing completes before a model attempt or credential turn is recorded. An
invalid script emits the turn's parse error and terminal outcome, with no `model.request`. A valid input
opens one simulated main-stream model step: its reasoning and message pieces share one request id.
A local tool ends that step before execution; its result supplies the next step's asked timestamp. The
next script step opens that request only if work follows, so a trailing tool invents no empty call.
Approval questions close model output before the person waits; a command resumes only after its actual
tool result, and a direct question resumes when the answer is given back.
Asked/first-output/terminal observations reuse one id and carry the thread and segment on both native
and external transports. Counts are nullable and terminal-only; a sole request can use its declared
usage, while cumulative multi-request usage stays a turn total. Timing arithmetic remains shared outside
the fake adapter. The fake's recorded clock can stand still independently of its scripted Tokio delays.
`tests/requests.rs` asserts these boundaries, parse rejection, counts and routing alongside the exact
directive/session event sequences. The `<orch>` context prefix remains handled by `script::parse`.

**Managing documents.** `manage` and `confirm` are the two calls of the tool that deletes, moves and brings back documents (`manage <command> <path> [| <path>…] [=> <to>]`, then `confirm` with the word the first answer gave), kept apart in `script/manage` and tested in `tests/manage.rs`, so that a script can show a plan, confirm it, confirm it twice or confirm what changed meanwhile.

**What a model can be run with.** It publishes three parameters, one of each kind that reads the model's entry at its provider: `service_tier`,
`access_program` and `context` (from 1 000 to the largest the entry opened), each starting with what the entry says a thread starts with. It
offers a speed, an access program and a larger context for `fake-large` that nothing enables, and reports its usage against the context a
turn was started with, so that every path of the generic parameters is exercised without a harness.

**What a failure says.** A script fails a turn in each way a harness does, with the vendor's words and the moment a
limit ends where there are any: the whole plan used up until a moment it names (`exhaust`, whose facts keep that moment
and are whole again when it has passed), one part of the plan used up while the rest is not (`limit`, retryable, the
facts unchanged), a provider that is busy or out of reach (`busy`, `unreachable`, neither held against the plan), a
stream that breaks in the middle of an answer (`cut`, whose error says the answer may be cut short), and any code at all
(`fail`). That is how the engine's reaction and the screens that show a failure are exercised without a harness.

| Module | Responsibility |
|--------|----------------|
| [descriptor](src/descriptor.rs) | Configurable capabilities, arrangement types, executors and tool sets, credential forms, each kind of memory, and a sign-in method of each kind with every field type, one of which takes a file's text or its path. |
| [script](src/script.rs) | Every fake directive and interruptible execution, with one directive for each of Orch's tools and one for a step of each kind. |
| [script/manage](src/script/manage.rs) | The directives `manage` and `confirm`: the two calls of the tool that deletes, moves and brings back documents, the second made with the word that the first was given. |
| [sessions](src/sessions.rs) | Persist, resume, export, import, fork and resync scripted sessions; notes kept by the harness or left to the agent, as configured. |
| [credentials](src/credentials.rs) | Shared and transferred test credential lifecycles. |
| [events](src/events.rs) | Scripted neutral events and failure injection. |
| [script/browser](src/script/browser.rs) | Browser directives call the browser tool and say its result as an agent message. |
| [script/display](src/script/display.rs) | Display directives call the display tool and say its safe result as an agent message. |
