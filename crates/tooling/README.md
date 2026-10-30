# Tooling libraries

Build support, verification, generation, packaging and future behavioral test support.

- [orch-testkit](testkit/README.md): Behavioral doubles, a clock that moves when told and a database of its own for each test, for the tests of the other packages.
- [orch-build-support](build-support/README.md): Build-script release validation and component counter allocation.
- [orch-xtask](xtask/README.md): Working Rust verification; contract generation and release modules are placeholders.
- [orch-stress](stress/README.md): Seeds a running system through its API and drives it at a team's scale, reporting each route's latency, errors, live-feed lag and long statements against the targets.
