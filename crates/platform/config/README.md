# orch-config

Configuration loading shared by service and machine entry points.

Implemented for the server's `orch.toml` and for a connector's file:
every key with its shipped default, unknown keys refused, each key
replaceable by `ORCH_<SECTION>_<KEY>`, and secret files read without their
values ever being printed. A connector's file holds its tunables and what
makes it a machine of a system: its data directory, the gateway endpoints and
how each is trusted, its name and roles, the owner's local policy (a table for
each dotted key: `environment`, `mcp`, `exec`, `agent`, `provider`, `tools`,
`update`, and `policy.delegated`), the plugins it runs, and the figures that
decide how a dead harness is started again. The `[limits]` table bounds, among other things, how many orgs (`org_depth`) and how many projects (`project_depth`) a path may hold one inside another, how many exports (`exports_per_user`) and imports (`imports_per_user`) of one person run at once and the largest file an export takes in (`export_file_max_bytes`).

The local `environment.browser` policy defaults to false, so a connected
environment opens browsers only after the machine owner allows them.

The API and the gateway edge share `[api] trusted_proxy_cidrs`, an empty list
unless the installation names its proxies. Socket peers outside those ranges
cannot choose their caller address with a header. Trusted connections use the
last untrusted hop, and unreadable chains use the connection address. Tests
cover forged chains, direct calls, repeated headers, both address families and
the standard `Forwarded` header.

| Module | Planned responsibility |
|--------|------------------------|
| [client_address](src/client_address.rs) | Caller addresses from socket peers and explicitly trusted proxy chains, and the one hourly unnamed-private-proxy diagnostic. |
| [server](src/server.rs) | Server operational configuration. |
| [connector](src/connector.rs) | Connector configuration and operational tunables. |
| [connector/driver_options](src/connector/driver_options.rs) | Lossless JSON connection options in the machine policy file. |
| [loader](src/loader.rs) | TOML loading, validation and environment overrides. |
| [secrets](src/secrets.rs) | Secret-file loading without logging values. |
