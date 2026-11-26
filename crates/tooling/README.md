# Tooling libraries

The libraries that build, check, generate and package Orch, and support its tests.

- [orch-testkit](testkit/README.md): behavioral doubles, a clock that moves when told and a database of its own for each test, for the tests of the other packages.
- [orch-build-support](build-support/README.md): release validation inside the build scripts, and allocation of the build counters ([build/](../../build/README.md)).
- [orch-xtask](xtask/README.md): `cargo xtask`, the development commands shared by contributors and CI: `verify` (format, Clippy, build, docs and dependencies), `names`, `contracts` (writes the OpenAPI document and the browser's types), `package` (a release's files and manifest), `image` and `identity`.
- [orch-stress](stress/README.md): seeds a running system through its API and drives it at a team's scale, reporting each route's latency, errors, live-feed lag and long statements against the targets.

## Build and test

```sh
cargo xtask verify
cargo xtask contracts --check     # fails when the generated files differ from the committed ones
```
