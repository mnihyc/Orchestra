# orch-model-contract

Wire-protocol and translator contracts for provider access.

The contract of provider access by wire protocol: the `WireProtocol` trait (allowed requests, model location, credential presentation, error shape, usage observer, model discovery, issuable credential forms), the options one endpoint may carry (how the key is presented, fixed and secret headers, query parameters) with the rule that presents them and the check that refuses a header the forwarder sets itself, a shared reader of server-sent events for usage observers, and the `ProtocolTranslator` seam with the rules for which protocols a provider serves, how a request is routed and who presents the credential. Each rule is tested as a rule, and the translator seam is exercised by a test translator between two test protocols; there are no protocol implementations here.

| Module | Planned responsibility |
|--------|------------------------|
| [protocol](src/protocol.rs) | WireProtocol request allowlists, authorization as an endpoint's options say, and model discovery; a plain relay per protocol. |
| [endpoint](src/endpoint.rs) | What one endpoint may configure beyond its address: how the key is presented, fixed and secret headers, query parameters. |
| [translator](src/translator.rs) | ProtocolTranslator seam, declared conversion losses, and the endpoint and credential fields its upstream needs; for external implementations only. |
| [usage](src/usage.rs) | Streaming usage observation. |
| [credentials](src/credentials.rs) | Forms issuable from an API credential. |
