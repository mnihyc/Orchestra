# Build counters

`build-numbers/` reserves one counter for each shipped component: server,
connector, frontend and CLI. Only the CLI is currently wired to the working
Rust build-identity helper. Local builds advance that counter; CI reads the
committed value. See [Contributing](../CONTRIBUTING.md) for isolated
development counters.

Nothing else lives here. Packaging, manifest hashes, release validation and
the build identity of components Cargo does not build belong to
`crates/tooling/xtask/src/release/` and `.github/workflows/release.yml`, and
are placeholders; no artifacts are published by this scaffold.
