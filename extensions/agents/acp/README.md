# orch-adapter-acp

Generic Agent Client Protocol adapter: any ACP v1 agent over stdio, driven by a program's manifest.

Implemented. The adapter drives an agent program that speaks version 1 of the Agent Client Protocol over its standard
streams. Nothing in it names a program: how a program is started (its arguments, its variables, the files written
first, the directory it starts in), which approval modes it has and what each comes to in its own words, how its tools
are named, and which companion plugin of Orch's may run inside it, are the program's launch data in its manifest.

A session is a process: the program is started with its own home, with the address its model calls go to and the
token for them in its variables, and ended with the session. It covers `initialize`, `session/new`, `session/resume`
and `session/load`, `session/close` and `session/cancel`, prompts with text and images, `session/set_config_option`
for the model and the effort and `session/set_mode` for plan mode, MCP servers by address (Orch's tools and the
environment's), tool calls and their updates as Orch's items, `session/request_permission` as approvals,
`elicitation/create` as questions, `usage_update` as context use, compaction updates as phases, `session_info_update`
as the title, and the client's own file and terminal methods carried out through Orch's executor, which is what lets an
agent that uses them be separated from its tools with no plugin.

What the protocol lacks (live text, steering, questions, plan mode with a plan to review, asking before changes, the
tokens of each model call, compaction phases, carrying a history in, the tools of a separated session) a companion
plugin gives through Orch's own `_orch/*` messages. For DeepSeek Harness the plugin is [dsh-bridge](dsh-bridge/README.md):
its built file is embedded in this crate (`programs::plugin_bundle`) and written into each session's home before the
program starts. The program announces which extras work in one hello, and each turns one capability on; nothing is
claimed that the hello did not say.

It is tested without a program against a scripted agent that speaks the protocol from a pipe (`tests/turns.rs`,
`tests/generic_suite.rs`), against a real operating-system process that does (`tests/process.rs`), and, where
`ORCH_DSH_BIN` names the program's native executable, against DeepSeek Harness itself with the scripted model of the
testkit as its only provider (`tests/dsh_real.rs`).

| Module | Responsibility |
|--------|----------------|
| [program](src/program.rs) | The manifest's launch data, and the templates it is written in. |
| [programs](src/programs.rs) | The launch data of the programs this build ships (DeepSeek Harness), as files read into the crate, and the plugin each is given. |
| [process](src/process.rs) | Starting a program: the launcher, its streams, its end, files written for its home. |
| [rpc](src/rpc.rs) | JSON-RPC over the streams, with calls whose answers arrive in order with what the agent said before. |
| [wire](src/wire.rs) | The protocol's method names, the shapes the adapter reads, small readers of the rest. |
| [launch](src/launch.rs) | Starting the program as the manifest says, its version, `initialize` and the plugin's hello. |
| [descriptor](src/descriptor.rs) | What the adapter claims of a program, from the protocol, the plugin and the manifest. |
| [session](src/session.rs) | One conversation: turns, requests of the agent, interrupts, the end of the process. |
| [mapper](src/mapper.rs) | The updates of a turn as neutral events. |
| [tools](src/tools.rs) | What a tool call is understood as, and the payload it makes. |
| [approvals](src/approvals.rs) | Requests to be allowed something and questions, as approvals, and their answers. |
| [client](src/client.rs) | The client's file and terminal methods, carried out through Orch's executor. |
| [extensions](src/extensions.rs) | Orch's own `_orch/*` messages and the companion plugin's hello. |
