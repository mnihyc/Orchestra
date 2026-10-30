# orch-plugin

External plugin transport, supervision and trait bridges.

Implemented. The package starts the programs that a component's configuration names, or attaches to ones that
already run on a local socket; negotiates versions and methods with each; matches overlapping requests with their
answers, progress and cancellation under deadlines; pings what is silent and starts again what is gone, after a pause
that doubles; keeps each program's log apart; stops them cleanly; and presents each as the Rust trait of its
contract, so that code which uses the trait cannot tell an external implementation from a built-in one. It is tested
with real programs: the two example plugins under `examples/plugins`, and a scripted one (`tests/support`) that hangs,
exits in the middle of a request, writes lines that are no message, answers out of order or speaks another version,
with time moved by hand so that thirty seconds of silence take no time.

Started programs and loopback TCP use Tokio's portable transport. Unix socket
attachment is available only on Unix systems; other systems receive an explicit
unsupported error. Synchronous translator calls and notifier link generation
require a multithread runtime, or a calling thread outside the runtime. A
current-thread runtime is refused immediately; asynchronous bridges work on it.

| Module | Planned responsibility |
|--------|------------------------|
| [registration](src/registration.rs) | Load locally configured implementations. |
| [transport](src/transport.rs) | JSONL over spawned stdio or local socket attachment. |
| [negotiation](src/negotiation.rs) | Version and method negotiation. |
| [schema](src/schema.rs) | Local JSON Schema validation with field paths and refusals that omit supplied values. |
| [requests](src/requests.rs) | Matching overlapping requests, deadlines, progress and cancellation. |
| [supervisor](src/supervisor.rs) | Ping, restart, reconnect, logs and shutdown. |
| [bridges/agent](src/bridges/agent.rs) | External AgentAdapter bridge. |
| [bridges/driver](src/bridges/driver.rs) | External EnvironmentDriver bridge. |
| [bridges/executor](src/bridges/executor.rs) | External Executor bridge. |
| [bridges/translator](src/bridges/translator.rs) | External ProtocolTranslator bridge. |
| [bridges/summarizer](src/bridges/summarizer.rs) | External Summarizer bridge. |
| [bridges/notifier](src/bridges/notifier.rs) | External Notifier bridge. |
| [bridges/provider](src/bridges/provider.rs) | External provider facts, models, resets and key presentation through one supervised program. |
