# Executable entry points

Applications compose libraries and built-in extensions. Reusable behavior belongs
in the library layers. Only the CLI version command is implemented.

- [orch-api](api/README.md): HTTP, realtime and internal backplane composition; hosts the durable engine.
- [orch-gateway](gateway/README.md): Authenticated machine sessions, routing, tunnels and stateless backplane relay.
- [orch-worker](worker/README.md): Database and outside-world delivery/upkeep; no machine orchestration.
- [orch-connector](connector/README.md): Agent, provider and environment roles on managed or connected machines.
- [orch-node](node/README.md): Managed runtime lifecycle through driver implementations.
- [orchctl](ctl/README.md): Administrator, bootstrap, diagnostics, consumption and plugin commands.
