# orch-adapter-codex

Native input checkpoints use Codex 0.162.1's `beforeTurnId` boundary for the actual first user input of a native turn. Native item and turn membership is checked again before opening; an input added inside a turn is explicitly unsupported and is never rounded to that turn's start. The default paginated mode and native inherited lineage stay intact. The fingerprint streams every frozen ancestor range, including compressed ranges, and detects missing, changed, cyclic or mismatched ancestry. No raw-prefix auxiliary file is allocated and ordinary interruption settings stay unchanged. An empty conversation opens a new session; every nonempty checkpoint uses one native fork with goal continuation deferred. A running source, occupied target, external or unknowable goal, compaction, rollback or replaced physical rollout refuses preparation. No text search, native ordinal or local path substitutes for the selected input. Original source histories and current workspace files stay intact. Preparation submits no model input; execution selection and durable activation/replay authority belong to the caller.

Codex app-server adapter; Codex-specific vocabulary stays here.

Ordinary Orch tools are required at startup and run without a second harness approval, with their destructive annotations preserved. Pinned Docker cases exercise reads and changes in each approval mode against a scripted model server.

Native import is a build-checked feature. The shared `native_import` case
exports two turns, imports into a fresh home, and verifies the retained model
context. The pinned paginated archive is rejected before a destination session
is kept. The engine applies the build's `import_native` limit and Carry uses
neutral import where available; a profile override cannot restore the disabled import.

Remote sessions resolve the executor's shell through native `environment/info` after registration and before every turn. Registration alone does not establish that connection. An unavailable executor refuses the input with a retryable error before a model turn starts; the session's remote working directory and roots are retained.

An in-environment session uses a verified empty workspace marker to load the instruction files down to its working directory, including through empty or absent local files. A workspace containing a `.codex` source is refused before native session configuration is resolved; the adapter waits for actual native unloading when a resume changes the instruction root. The engine resumes local Codex sessions before each turn because an open native session caches directory files. Codex 0.162.1 retains original standing instructions on resume, and a readable thread item explains that a standing-text edit holds from the next new session.

Implements `AgentAdapter` for the Codex command line, which it drives through the app-server protocol (JSON-RPC over the
process's standard streams): it starts the harness quiet and pinned, opens, resumes and forks its threads as sessions, runs
turns with the model, effort, approval mode and sandbox stated every time, reports each item as a neutral step with the tool
call both by its kind and as the harness made it, carries approvals both ways, signs a provider in (by a code, by an existing
`auth.json` or by a workspace or ChatGPT access token), reads its plan, limits and models, finds the sessions and MCP servers the harness
already has, exports native rollouts and imports neutral histories. Standalone native import stays undeclared until the
exact build proves it can restore two completed turns into a fresh home without its original store. A paginated or
reference-backed archive remains refused before any session is written; Carry uses its labelled neutral or full-file fallback.
It is tested without Docker against
literal frames of the harness and a harness played in the test, and with the real Codex in Docker, from the image
`orch-dev-harness`, against the scripted model server of the testkit: the tests that need Docker say so and return at once
where the image is absent. One test (`tests/real_model.rs`) reaches a real model through a relay that holds the key and runs only when
it is asked for in the environment.

Access tokens retain their own issuance and expiry times and are never renewed here. Provider reads include available resets; a confirmed reset uses the stable app-server request with the same key on retries. Scripted account replies cover all outcomes, and the pinned harness accepts a made-up ChatGPT access token without a refresh token.

**What it does when the harness fails.** A failure is taken in whole (`failure`): the typed kind and status of the app-server's error and the rate limits it reports
decide the code before any sentence does, and of a sentence only what is stable in it; whatever no row knows is `harness_error` with the harness's words. The error
carries those words (`said`, with how often the harness tried again by itself), the moment a limit of the plan ends (from the window the failure belongs to, else from the
sentence, in the zone it names or the runtime's) and whether an answer was cut short. A limit of the whole plan is `provider_exhausted`, one of a named limit the same but
retryable, a vendor that is busy or cannot be reached is `provider_busy` or `provider_unreachable`. A process that is killed, ends by itself, prints half a line or refuses
to start ends its turn with what was last seen, and the next turn starts a fresh process that takes the session up; one that goes silent, or goes on reconnecting, past
its patience (`with_patience`) is asked to interrupt and its turn ends. Each kind of failure is a fixture recorded from Codex 0.162.1 (`tests/fixtures/failures`) and a
test that replays it without a container (`recorded_failures`); `tests/failures.rs` runs the same faults against the real program, and
`scripts/harness-canary/failures` runs that against another version of it and prints the rows that changed.

**The context, the speed and the access program.** A thread's context is the parameter `context`, an integer from 32 000 to the largest the
model's managers opened, which starts at all that is open unless the model's entry names another start; the speed (`service_tier`) and
the access program start where the entry says. The context goes into the session's configuration as the two members this harness
holds apart, `model_context_window` and `model_auto_compact_token_limit`: the whole context, and the point where the conversation is
shortened, which follows from it by one rule (nine tenths of it, in integers: 900 000 for a million, which is the harness's own rule, written out
so that a smaller threshold of a catalogue entry does not hold a raised window back; no room is taken for the model's answer, since the windows
of its catalogue already leave it) and is set by nobody; when the context is the model's own window the harness is told that window and keeps its
own threshold. What the real harness does with them was tried in the image, against the scripted model server: a resumed session takes a new
window; a model the catalogue knows is held to the largest its catalogue allows (872 000 for the gpt-6 family), and one it does not know to
272 000, and a smaller window is taken in either case; the window it reports with its usage is 95 % of the one it works with (828 400 for a
million asked of a model held to 872 000), and a threshold it is given is held to nine tenths of that window (with 900 000 written out for a
million, a conversation of 700 000 tokens is let be and one of 800 000 is shortened, as when no threshold is given; for a model it does not know,
between 200 000 and 250 000). Its model
list carries no window, but the catalogue the harness renders on request (`codex debug models`) does: discovery reads from it the window each model starts
with and, where it is more, the largest an override may set, which is put among what the model offers and opens nothing; a list that does carry them is
not corrected, and a harness that cannot render its catalogue leaves a manager to write the windows.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Version support, arrangement types with their executor, a memory that is one store for a whole home and is therefore kept off, sessions held to the MCP servers they are given, and capabilities. |
| [process](src/process.rs) | Spawn or attach the harness, started quiet and pinned. |
| [runtime](src/runtime.rs) | The running harness: the sessions it serves, the task that reads what it says and turns it into events, the answers to its requests, and what is reported when it ends. |
| [protocol](src/protocol.rs) | Initialize and exchange app-server requests. |
| [sessions](src/sessions.rs) | Per-session configuration, remote environment binding, the project root that puts `AGENTS.md` of the thread's directory on the harness's path, the servers of the home's own configuration switched off, its memories kept off, and a fork for a side thread. |
| [events](src/events.rs) | Codex notifications to neutral events: each item as a step of its kind, with the call as Codex made it kept whole beside it. |
| [performance](src/performance.rs) | Codex request boundaries and FIFO token reports, without provider-specific timing arithmetic. |
| [failure](src/failure.rs) | What the app-server says when a turn fails, taken in whole: its typed kind and status before its sentences, the moment a limit ends from the rate limits it reports, and the vendor's words kept as they were said. |
| [approvals](src/approvals.rs) | Map harness approval and user-input requests. |
| [review](src/review.rs) | Native Auto review completions, model declarations and provider policy refusal. Auto is undeclared until the shared trial gate is verified. |
| [provider](src/provider.rs) | Sign-in on the provider (a code, an existing sign-in file given as text or by its path inside the runtime, a workspace token), the workspace restriction, usage facts and model suggestions. |
| [discovery](src/discovery.rs) | Sessions found in a Codex home; the MCP servers of its own configuration, the home's and a directory's, by name, and one definition read out for an import. |
| [history](src/history.rs) | Neutral conversion of a carried conversation into the Responses items a new thread is told. |
| [native_history::spool](src/native_history/spool.rs) | Temporary native-history indexes keep home metadata and family walks off the heap. |
| [native_history](src/native_history.rs) | Exact native history ownership, measurements and removal in managed Codex homes. |
| [parameters](src/parameters.rs) | Map effort and eligible neutral parameters to Codex fields; the speeds and access programs Codex offers for a model, which a provider enables before a thread shows them. |

Goal mode is Orch's own and is the same for every program, so the harness's native goal feature stays off: the base configuration of every home Orch makes and the configuration of each thread's session carry `features.goals = false`, and the checkpoint preparation treats a thread whose goals feature is disabled as having no native goal to continue. Orch keeps a goal going itself, by inputs the engine writes in the name of the person who set it.
