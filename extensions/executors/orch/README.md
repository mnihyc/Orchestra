# orch-executor-orch

Orch's own executor: plain exec tools served through MCP, one of the executors an environment offers.

It reports **Full access** only. File-tool path checks are advisory; shell commands are not confined
by this executor. Installing bubblewrap does not add Workspace or Read only modes here. The Codex
executor probes and supplies its own Linux sandbox; an environment's reported executor modes, its
machine policy and its template ceilings determine which tools a turn can use.

`OrchExec` is the executor an exec node starts for a harness that runs elsewhere and has no tools of its own, or
has them switched off. It listens on a loopback port the system gives, behind a token, and serves each session
the tools of its set: the eight `plain` tools, or tools with the names and inputs of Claude Code's own shell and
file tools. Commands carry the thread and tool-call ids, long output is cut in the middle and kept whole in a
file, and the file tools hold to the thread's directory and the attached mounts in the sandbox modes that ask
for it. The unit tests of each module are joined by an integration suite that starts the executor and drives it
with a real MCP client over the loopback address; it needs no Docker.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Executor identity, its three tool sets and attribution. |
| [server](src/server.rs) | Environment-local MCP execution service: the tools of the session's set. |
| [scope](src/scope.rs) | What a request says of the session it belongs to: the thread, the tool set, the directory, the sandbox mode and the mounts. |
| [tools](src/tools.rs) | The tool sets a harness works through when it has no tools of its own, or has them switched off: which one a session gets from what the environment offers, and what each lists; none for a session that is to act through the environment's own servers alone. |
| [plain](src/plain.rs) | The eight plain tools: shell with its output and stop, read, write, edit, list and search. |
| [claude_code](src/claude_code.rs) | The set with the names and inputs of Claude Code's own shell and file tools, carried out here. |
| [commands](src/commands.rs) | Command execution with thread and tool-call context. |
| [output](src/output.rs) | What a command's output is cut to in a result, and the file that keeps it. |
| [files](src/files.rs) | File tools, held to the thread's directory and the attached mounts by the agent's sandbox mode. |
| [sandbox](src/sandbox.rs) | Which paths a session may reach: the thread's directory and the attached mounts, held by the agent's sandbox mode. |
| [search](src/search.rs) | What a search looks through and how a match is found. |
