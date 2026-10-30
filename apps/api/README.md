# orch-api

HTTP, realtime and internal backplane composition; hosts the durable engine.

The program reads `orch.toml` and the secrets directory (`--secrets-dir`,
`ORCH_SECRETS_DIR`), brings the schema up to date, and serves the browser API
under the system's own authority and, on `[api] internal_bind`, the internal
listener that only a service certificate of that authority reaches: the
gateways' backplane, enrollment, renewal and content. `--plain-http` serves the
browser API without TLS for development, and without the secrets of a
deployment the API then has no machines. The thread engine and the realtime
socket are composed here, and the master key of the secrets directory is given
to every place that seals what a request stores: an authenticator, the secrets
of a node and the keys of providers. `tests/program.rs` runs the program as a
process from a configuration file and a secrets directory it makes, and makes the
first administrator, an org and a secret of each of those kinds over HTTP.
