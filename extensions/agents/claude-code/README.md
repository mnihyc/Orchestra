# orch-adapter-claude-code

Claude Code adapter; Claude Code-specific vocabulary stays here.

Native input-prefix preparation is separate from ordinal forks and is limited to Claude Code 2.1.296. The adapter inspects an actual main-conversation user UUID and verifies its parent chain and prefix fingerprint again before opening a fresh same-thread segment. The native fork loads through that parent with `--resume-session-at`; a first-input cut starts an empty conversation. An occupied target, a running source or live lane with a retiring generation, changed prefix bytes, or missing/ambiguous/cyclic identities refuses preparation. The retained native conversation/tool records and current workspace files remain available; native close may append its administrative prompt and cost checkpoints. Preparation sends no model input; execution selection and author/provider authority belong to the caller. Ordinary bounded ordinal forks retain their existing refusal.

Queued mid-turn input is delivered as native system guidance with a separate source UUID. Its source and attachment UUIDs are refused individually by this user-message checkpoint interface; the adapter does not round either identity to an earlier turn or claim a successful edited branch.

Ordinary Orch tools run without a second harness approval. Session instructions use a private file and are rebuilt on resume; pinned Docker cases verify changed UTF-8 instructions in the next model request and reads and changes in each approval mode.

The in-environment instruction loader reads both native file forms from the workspace root down to the working directory, with absolute exclusions for ordinary ancestors above that root. At each idle turn boundary a resumed process reloads the actual files, which a continuing process caches. Project settings files are refused before launch, since the native project loader couples those settings and directory instructions.

MCP changes take effect on the next turn by resuming the same conversation with the current server set. This adapter declares live MCP reload false and uses session resume to replace its MCP configuration. A running turn keeps its existing set; session reconfiguration submits no extra model input.

The adapter is implemented. It starts the vendor's command-line program, one process for each
conversation, through a launch description a launcher turns into a command; reads its JSON lines
and turns every message, tool use and permission prompt into neutral events, each tool call both as
Orch understands it and as the harness made it; and does start, resume, fork, steer, interrupt,
approvals, sign-in, discovery, export and resync. It is tested with the real program, from the image
`orch-dev-harness`, in a container, against the scripted model server of the test kit; those tests
say so and pass without running where Docker or the image is not there. It drives the versions its
descriptor names as supported, and says so for any other.

Provider reads ask Claude Code itself through its experimental `get_usage` control request without a model turn. The adapter makes no HTTP request with the sign-in. Session, weekly and model-family windows and extra spending keep their existing fact fields; turn-time `rate_limit_event` frames supply the binding window and admission decisions. Unanswered or unfamiliar reads leave usage unread or retain its previous observation time. Fixture-driven control tests cover those cases, and result frames make missing model contexts available to discovery.

**What it does when the harness fails.** A failure is taken in whole (`failure`): the typed fields of the harness's frames decide the code before any
sentence does (the class of the error and its status, the `terminal_reason` of the result, the `rate_limit_event` and its window, the tally of `api_retry`),
and of a sentence only what is stable in it; whatever no row knows is `harness_error` with the harness's words. The error carries those words (`said`, with how
often the harness tried again by itself), the moment a limit of the plan ends (from the event's epoch, else from the sentence, in the zone it names or the
runtime's) and whether an answer was cut short. A limit of the whole plan is `provider_exhausted`, one of a family of models or of spending the same but retryable,
a vendor that is busy or cannot be reached is `provider_busy` or `provider_unreachable`. A process that is killed, ends by itself, prints half a line or refuses to
start ends its turn with what was last seen, and the next turn starts a fresh process; one that goes silent past `silence_secs`, or goes on trying again past
`retry_patience_secs`, is stopped and its turn ends. Each kind of failure is a fixture recorded from Claude Code 2.1.296 (`tests/fixtures/failures`) and a test that
replays it without a container (`recorded_failures`); `tests/failures.rs` runs the same faults against the real program, and
`scripts/harness-canary/failures` runs that against another version of it and prints the rows that changed.

The harness is driven as a child process over JSON lines, one process for
each conversation. It has no protocol of its own that separates it from its
tools, so it declares three arrangement types. Beside its tools in the
environment, on a key that stays behind the forwarder (key-separated) or on
a token its provider shares (credential-shared). Or in its provider's
runtime, with its built-in tools switched off and Orch's executor in the
environment in their place (separated), which is what a provider that lets
nothing leave is used in. Which of these a thread gets follows from what
this adapter declares and what the provider's owner allows, not from its
name.

**The context and the speed.** A thread's context is the parameter `context`, an integer from 100 000 to the largest the model's managers
opened, which starts at all that is open unless the model's entry names another start. The harness is told the context and nothing else:
`CLAUDE_CODE_AUTO_COMPACT_WINDOW` in the process's environment, the suffix `[1m]` on the model's name above 200 000 (the harness takes it off
before the name goes upstream), and, for a model whose name it may not know, `CLAUDE_CODE_MAX_CONTEXT_TOKENS` with the model's window or the
context where that is larger; it keeps room for the answer by a margin of its own. Tried in the image against the scripted model server: the
variable has no effect on a name the harness knows; the suffix gives a million to a name it knows as 200 000 and to one it does not; the
conversation is shortened within the size given and not at the model's own point. The window it reports with its usage is that of the model the
process runs (the result lists every model the session ever used, a name with the suffix beside the same name without), so a context lowered again is
reported at the lower window. The speed is the harness's fast mode, offered by discovery
for the models that report it and enabled by the provider's managers, and is stated in the settings of every process (`fastMode`, off as
well as on: without the setting a headless run is told it needs one, `sdk_opt_in_required`); a change of either restarts the session's process.
The harness says in the result of a turn which state the mode is in (`on`, `off`, or `cooldown` once the vendor's limit for it was reached,
after which it goes on at the ordinary speed by itself), and a turn that ended in the last has a notice saying so; its use is recorded with
the speed it was asked for. Against a model server that is not the vendor's the real harness reports the state `off` even with the setting on.

| Module | Planned responsibility |
|--------|------------------------|
| [adapter](src/adapter.rs) | The adapter itself: start, stop, sessions, turns, sign-in, discovery. |
| [conversation](src/conversation.rs) | One conversation: its process, its turn and the events it yields. |
| [context_usage](src/context_usage.rs) | What fills the context, as the harness breaks it down on request (`get_context_usage`), in Orch's words. |
| [config](src/config.rs) | The names, limits and profile options of this adapter, each with the reason for its value. |
| [descriptor](src/descriptor.rs) | Version range, arrangement types with their executor and tool sets, credential forms, what a new subscription proposes, a memory it can keep in a directory, sessions held to the MCP servers they are given, and capabilities. |
| [process](src/process.rs) | One process for each conversation under one home: the quiet and pinned start, idle exit and resume. |
| [stream](src/stream.rs) | JSON lines in and out of the harness's non-interactive mode, parsed tolerantly. |
| [sessions](src/sessions.rs) | Per-session flags and settings: the MCP set and nothing else, `AGENTS.md` read beside the harness's own file, its memory kept in the thread's directory or switched off, resume, fork, and the restart a change of effort takes. |
| [events](src/events.rs) | Stream messages to neutral events: each tool use as a step of its kind, with the call as Claude Code made it kept whole beside it; sub-agent streams, usage, and unknown frames kept raw. |
| [performance](src/performance.rs) | Claude message boundaries and counts, with untimed sub-agents and explicit hidden thinking. |
| [failure](src/failure.rs) | What the harness says when a turn fails, taken in whole: its typed fields before its sentences, the moment a limit ends, and the vendor's words kept as they were said. |
| [approvals](src/approvals.rs) | The permission tool, approval modes and the shell sandbox mapping. |
| [review](src/review.rs) | Auto's availableModels policy, factual PermissionDenied hook and refusal of a Manual fallback. Auto is undeclared until the shared trial gate is verified. |
| [provider](src/provider.rs) | Sign-in on the provider (a pasted long-lived token, a browser code), login facts, shared tokens and the handed-over login. |
| [tools](src/tools.rs) | The harness's own tools; with its provider, those kept that act on no machine, and the rest replaced by a tool set of the environment's executor. |
| [discovery](src/discovery.rs) | Sessions found in a Claude Code home; the MCP servers of its own configuration, a user's and a directory's, by name, and one definition read out for an import. |
| [history::record](src/history/record.rs) | Adoption records reconcile item slots on disk and stream the latest complete turns. |
| [history::reader](src/history/reader.rs) | Captured transcripts yield one historical mapping batch at a time. |
| [history::spool](src/history/spool.rs) | Temporary indexed files keep historical item and tool correlation off the heap. |
| [history](src/history.rs) | Transcript export, import and fork between homes; arrival from another harness by summary. |
| [native_history](src/native_history.rs) | Exact transcript ownership, measurements and removal in managed Claude Code homes. |
| [instruction_sources](src/instruction_sources.rs) | Reported instruction loads and imports, observed without changing the harness's decisions or text. |
| [parameters](src/parameters.rs) | Map effort, auxiliary models and the adapter's own parameters. |

Goal mode is Orch's own and is the same for every program, so Claude Code's own `/goal` is unused: the adapter never sends it, and Orch's composer handles `/goal` itself (a message that begins with it and cannot be taken as the command is refused, not passed on). Orch keeps a goal going itself, by inputs the engine writes in the name of the person who set it.
