# orch-gateway

Authenticated machine sessions, routing, tunnels and stateless backplane relay.

The program reads `orch.toml` and the gateway's own certificate from the
secrets directory, listens on the pinned and the edge port, dials the system's
backplane under mutual TLS, and serves until it is told to stop; it keeps
nothing on disk. `--config` and `--secrets-dir` (or `ORCH_CONFIG` and
`ORCH_SECRETS_DIR`) say where its files are.
