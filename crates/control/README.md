# Control libraries

Durable control-plane policy, orchestration and database-backed services.

- [orch-db](db/README.md): PostgreSQL persistence, transactions and projections.
- [orch-authz](authz/README.md): The single access-decision implementation.
- [orch-settings](settings/README.md): Settings evaluation over the shared catalogue.
- [orch-chat](chat/README.md): People's chats: messages, following, inspection and forwarding.
- [orch-documents](documents/README.md): Documents: pages of blocks written in place, with their operations, comments, history, files and exports.
- [orch-engine](engine/README.md): Durable product orchestration hosted inside the API.
- [orch-api-core](api/README.md): API application hosting HTTP, realtime and the machine backplane.
- [orch-gateway-core](gateway/README.md): Stateless machine gateway and tunnel relay.
- [orch-worker-core](worker/README.md): Database and outside-world delivery and upkeep; no machine orchestration.
