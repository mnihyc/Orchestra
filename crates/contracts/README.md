# Contracts libraries

Neutral schemas, catalogues, descriptors and interfaces for every seam.

- [orch-api-contract](api/README.md): HTTP request and response contracts, with route metadata.
- [orch-realtime-contract](realtime/README.md): Browser WebSocket contracts, including terminal sockets.
- [orch-link-contract](link/README.md): Machine link and backplane schemas; commands and events shared by both ends.
- [orch-agent-contract](agent/README.md): Neutral harness adapter contract and declarations.
- [orch-agent-programs](agent-programs/README.md): Agent program manifests and the registry of the built-in programs.
- [orch-driver-contract](driver/README.md): EnvironmentDriver contract for provider and environment runtimes.
- [orch-executor-contract](executor/README.md): Executor contract for tool execution inside an environment.
- [orch-model-contract](model/README.md): Wire-protocol and translator contracts for provider access.
- [orch-plugin-contract](plugin/README.md): External extension JSONL contracts.
- [orch-provider-contract](provider/README.md): Provider source contracts: facts, models and credential presentation.
- [orch-settings-contract](settings/README.md): Settings catalogue and resolution data, without evaluation.
- [orch-access-contract](access/README.md): Permission catalogue, roles and permission-page contracts, without access decisions.
- [orch-summarizer-contract](summarizer/README.md): Summarizer contract for title, description and tag generation.
- [orch-notifier-contract](notifier/README.md): Notification channel contract hosted by the worker.
- [orch-blob-contract](blob/README.md): BlobStore trait seam; local storage is the v0.1 implementation.
- [orch-identity-contract](identity/README.md): IdentityProviderKind trait seam; OIDC is the v0.1 implementation.
