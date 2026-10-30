# orch-conformance

The suite every implementation of a contract passes, for tests and for orchctl plugin check.

It reports each contract rule as passed, failed or skipped with a reason and
drives every extension family through its contract trait. Fixtures name
disposable units, environments, notification targets, issuers and model
responses; tests compare conforming behavior with deliberate faults, and
native harness tests provide the separate proof of declared vendor behavior.

It is a production library, not test support: the operator's
`orchctl plugin check` runs the same suites against an external
implementation that the tests run against the built-in ones.

The shared `native_import` case exports two controlled turns and imports the
archive into a fresh harness home. The next model request must retain both
turns. An unsupported archive fails only `import_native`; the controller
keeps that result with the build and Carry chooses the remaining method.

| Module | Planned responsibility |
|--------|------------------------|
| [report](src/report.rs) | One result for each case of a suite, in the words an operator reads. |
| [scripted](src/scripted.rs) | The scripted counterparts a suite drives an implementation against: a controller, an upstream, an environment. |
| [agent](src/agent.rs) | The suite every agent adapter passes: the command and event catalogue, sessions, sign-in methods and credential forms it declares. |
| [driver](src/driver.rs) | The suite every environment driver passes: create, stop, start, recreate with volumes intact, sources, imports and reconciliation. |
| [executor](src/executor.rs) | The suite every executor passes: start, the protocol its harness expects, and attribution of what it starts. |
| [protocol](src/protocol.rs) | The suite every wire protocol passes: allowed requests, the model's place, the key as an endpoint says, usage and errors. |
| [translator](src/translator.rs) | The suite every translator passes: requests and streamed responses converted, declared losses, and who presents the credential. |
| [provider](src/provider.rs) | The suite every provider source passes: declarations, member states, models, resets, presentation, quiet errors and bounds. |
| [provider::tests](src/provider/tests.rs) | Controlled broken counterparts prove that each provider rule reports the behavior it names. |
| [summarizer](src/summarizer.rs) | The suite every summarizer passes: each field within its limits, with and without a model. |
| [notifier](src/notifier.rs) | The suite every notifier passes: each kind of reach, the check of a channel when it is saved, a text for one notification and for several, a link made and followed, a retry, and a target that is gone. |
| [native](src/native.rs) | Native harness cases shared by adapter tests and the node's isolated build runner. |
| [native::elicitation](src/native/elicitation.rs) | A controlled MCP server asks the real harness for a form while its only other endpoint is the scripted model. |
| [blob](src/blob.rs) | The suite every blob store passes: put, ranged get, existence, delete and size by content address. |
| [identity](src/identity.rs) | The suite every identity provider kind passes: start, callback, claims and refusal codes. |
| [transport](src/transport.rs) | The plugin transport's own cases: handshake, unknown members and methods, overlap, cancel and ping. |
