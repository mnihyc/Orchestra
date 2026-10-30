# orch-link-contract

Machine link and backplane schemas; commands and events shared by both ends.

Implemented. Every message of the link is a type with its wire form: both
handshakes, frames, durable streams, the commands of each role with their
results, the events, tunnels, content transfer, the gateway's backplane,
enrollment, versions and limits, and a machine's local policy. The
arithmetic both ends must agree on is here as plain functions (sequence and
acknowledgement, a tunnel's window, negotiation) and is what the unit tests
state.

Foreign payloads re-export their concrete adapter, model, driver and API contracts.
The recipe a node receives carries a reference or a sealed credential; the driver
receives the same recipe after that credential is opened. Native background and
browser observations share the API contract below these contracts, avoiding a
cycle while preserving their existing import paths. Partial item changes and
protocol-specific extension options remain JSON because their declarations
explicitly describe member replacements and extensible options.

| Module | Planned responsibility |
|--------|------------------------|
| [session](src/session.rs) | Hello, authentication, readiness and machine facts. |
| [streams](src/streams.rs) | Durable streams, acknowledgement and reset. |
| [commands](src/commands.rs) | Role command catalogue, serial keys, results and errors. |
| [mcp_import](src/mcp_import.rs) | A local connector's request to file an imported tool server in the system as its machine's owner. |
| [local_ports](src/local_ports.rs) | Environment-scoped port reads and assertion refresh over the authenticated live machine link. |
| [enrollment_choices](src/enrollment_choices.rs) | Role and policy handover choices bound to an enrollment token's existing secret hash. |
| [background](src/background.rs) | Live native session commands and factual termination observations. |
| [source](src/source.rs) | Commands for repository preparation, immutable file states, restoration and publication. |
| [events](src/events.rs) | Session, provider, environment, harness and node events. |
| [ports](src/ports.rs) | Live port observation requests and replies, separate from retained command results. |
| [tunnels](src/tunnels.rs) | Tickets, channels, flow control and resume messages. |
| [content](src/content.rs) | Command-bound blob transfers. |
| [backplane](src/backplane.rs) | Gateway/API messages and terminal tunnel frames. |
| [enrollment](src/enrollment.rs) | Enrollment and renewal payloads. |
| [version](src/version.rs) | Protocol negotiation and limits. |
| [browser](src/browser.rs) | Discovery, profiles and lifecycle commands for browsers inside an environment. |
| [browser_window](src/browser_window.rs) | Opening the virtual display needed by a browser with a window. |
| [display](src/display.rs) | Adoption, closure and state reports for displays served inside an environment. |
