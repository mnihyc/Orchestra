# orch-agent-contract

Neutral harness adapter contract and declarations.

Usage, raw calls and their long-output references, model entries/rates, and sandbox levels are re-exported from
the API contract. Their arithmetic, UTF-8 tail and enabled-default helpers live with that owner;
agent/model consumers therefore use the same type that the API serializes. The API dependency is
acyclic (API depends on access/types; link and agent already depended transitively on it).

The contract between Orch and any agent harness: the `AgentAdapter` trait with the declarations an adapter publishes (`AgentDescriptor` with its arrangements, capabilities, sign-in methods and parameters), the neutral events a session emits, and the facts a machine and an environment report. The rules that need only these declarations are code here and are tested as rules: which arrangement a thread gets, whether a parameter exists for a thread, where a harness version stands against its range, the sandbox a turn can run in, who keeps an agent's notes, which method carries a history elsewhere, how model names are matched by pattern (a name, a family with `*`, or a group of the provider's own models written `@group`), and what a model can be run with: the range a parameter takes from the largest context a model's managers opened, the speed, the access program and the context a thread starts with, and which of a thread's choices are still open and which are flagged and replaced by that start (the model's own window travels to an adapter in `params`, under one reserved key). Every descriptor, event and command body has a round-trip test on a literal example; there are no implementations here.

| Module | Planned responsibility |
|--------|------------------------|
| [adapter](src/adapter.rs) | AgentAdapter interface and error vocabulary. |
| [background](src/background.rs) | Live native session commands and factual termination observations. |
| [native](src/native.rs) | Session-specific native action identities, arguments and safe provenance without skill contents. |
| [descriptor](src/descriptor.rs) | Capabilities, the arrangement types a harness can run in with their executors and tool sets, effort and quiet-start support, what the harness keeps as memory and where, whether a session is held to the MCP servers it is given and whether the harness's own can be listed for an import, the template its provider runtime is made from, and support facts. |
| [parameters](src/parameters.rs) | Parameter definitions, eligibility and model-driven options; what a harness offers for a model, apart from what a provider has enabled. |
| [session](src/session.rs) | Session (new, resumed, forked or imported), turn (with model and effort), approval and tool operations; the instructions given with a session, and where the agent's notes are kept and by whom. |
| [discovery](src/discovery.rs) | Sessions a harness already has; the models an endpoint lists, each with the name that is sent, the name that is shown and its rate where the list carries one; and MCP definitions with exact argv, declared secret variables and optional HTTP bearer authentication by secret name. Imported definitions stay on their machine. |
| [events](src/events.rs) | Neutral harness events: what was said, and each tool call both as Orch understood it, by kind, and as the harness made it. |
| [requests](src/requests.rs) | Neutral per-request observations shared by built-in, external and future command-line adapters. |
| [review](src/review.rs) | Native permission reviews, kept as factual thread items rather than permission requests. |
| [tool_answers](src/tool_answers.rs) | The model-facing projection of structured Orch tool results. |
| [credentials](src/credentials.rs) | Credential forms with their custody and lifetime, what an adapter issues and accepts, and exchange. |
| [login](src/login.rs) | Sign-in methods as declarations (fields, kind, what each yields; an import takes a credential as text or as the path of a file inside the runtime), the state a running sign-in reports, and a provider's own options. |
| [history](src/history.rs) | Native and neutral archives, import fidelity and exact native input-prefix/source identities. |
| [arrangement](src/arrangement.rs) | Which arrangement a thread gets: what is available from the declarations and the facts, in what order, and what is permitted. |
| [facts](src/facts.rs) | What a profile is configured with, and what a machine and an environment report of themselves. |
| [flow](src/flow.rs) | Byte-aware bounded handoff queues that propagate consumer backpressure to native producers. |
| [loopback](src/loopback.rs) | The addresses the connector serves a harness on, beside it. |

An adapter declaring `native_input_prefix: "turn_start"` implements three distinct operations:
`session.prefix.inspect` resolves the actual stored first input of a native turn;
`session.prefix.open` revalidates that fingerprint and creates a distinct native child;
`session.continuation.resume` observes or restores an exact retained idle source without model
input or automatic goal continuation. The last operation accepts `spec`, `source` and an optional
`replace_segment_id` for the provisional segment owned by the operation. Its default refuses;
ordinary Resume is not a substitute. Other in-turn inputs have their own unsupported result,
never a rounded turn cut. External programs use the same three RPC methods as built-in adapters.
Native identity, quiescence and fingerprint checks remain separate from server permission and
execution selection. Preparing or browsing conversation history does not restore workspace files.

