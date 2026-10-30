# orch-plugin-contract

External extension JSONL contracts.

The contract of the plugin transport that lets an implementation of any extension point live outside Orch's own code: the `[[plugin]]` registration of a program the host starts or one it attaches to (loopback only), the JSON Lines messages in both directions, the handshake with its version ranges and the catalogue of each kind's required and optional methods, the deadlines, pings and back-off that keep a plugin honest, the preserved `x_` members of a descriptor, and the two small shapes every extension contract shares (a secret that is never printed, a reference to a person or a place). Every rule is code with tests, from the doubling pause to the check that a plugin of another kind, of a version the host did not offer, or without a required method is stopped; the messages and the handshake are round-tripped on literal examples.

| Module | Planned responsibility |
|--------|------------------------|
| [registration](src/registration.rs) | Start or attach configuration and supported extension kinds. |
| [messages](src/messages.rs) | Requests, replies, progress, events and cancellation. |
| [negotiation](src/negotiation.rs) | Transport and contract version ranges, required and optional methods. |
| [descriptor](src/descriptor.rs) | Extension identity and preserved x_ fields. |
| [lifecycle](src/lifecycle.rs) | How long a plugin may take, whether it is alive, and which requests are still waiting. |
| [secret](src/secret.rs) | A value that is given to an implementation and never printed. |
| [refs](src/refs.rs) | People and places as an extension is told of them: the references that notifications and summaries carry. |
