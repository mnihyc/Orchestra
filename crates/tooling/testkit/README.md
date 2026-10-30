# orch-testkit

Behavioral doubles, a clock that moves when told and a database of its own for each test, for the tests of the other packages.

What is built, and used by the tests of the other packages: `clock` (time
that moves when a test says so), `database` (a PostgreSQL database for each
test, copied from a template that is migrated once), `doubles/link` (a hub in
memory that both ends of the machine link connect to, with cuts, delays and
lost frames on demand), `doubles/machine` and `doubles/controller` (a scripted
machine for tests of the system's side, and a scripted system for tests of a
machine's side), `doubles/model` (a scripted model server that speaks both
built-in wire formats, also as the program `orch-mock-llm`),
`doubles/driver` (an environment driver that keeps its units in memory), and `oidc` (a signed
OpenID Connect provider on the loopback: discovery, rotating keys, PKCE, the token and user
information endpoints, and the faults a provider makes, for the tests of signing in).

Database-backed scripted harness worlds use `doubles/sessions` to open a
session on its planned machine before producing reports from its segment.

Database fixtures reserve a shared connection budget before opening their pools and
keep exact ownership and cleanup records through removal. The
[test job runner](../../../scripts/check-test-jobs/README.md) records process
exits independently of printed test summaries and checks only the databases
owned by that run; its regressions use PostgreSQL and competing processes.

Without configuration, admission reads PostgreSQL's connection limit, leaves
one fifth for other traffic, and allows at least 128 connections where the
server's usable capacity permits. Server-reserved connections are excluded.
Each fixture reserves 32 connections, including secondary pools: a server with
1,200 connections admits 30 concurrent fixtures, and one with 100 admits three.
`ORCH_TEST_DATABASE_CONNECTION_BUDGET` overrides the total budget;
`ORCH_TEST_DATABASE_CONNECTIONS_PER_FIXTURE` can increase the reservation per
fixture. Cooperating processes must agree on these values while fixtures are
active. Capacity discovery is serialized across processes and closes its one
administrative connection before waiting for admission. The capacity regressions
run disposable PostgreSQL 17 containers through Docker using the locally
available `postgres:17-alpine` image.

The scripted model server fails on purpose, in both wire formats: an `error` directive gives the answer a vendor gives
for each kind of fault (a limit of the plan as that vendor words it, with the headers its harness reads, and the time it
ends in a form the script chooses; 429 with and without a retry header; 529, 500 and 504; a refused key; a balance that
ran out; an unknown model; a prompt too long); `stall` holds the first byte back, `cut_after` ends a stream in the middle
of a sentence or of a tool call, `truncate` ends it without its closing events, `event` and `garbage` put an event nobody
knows and a line that is no JSON into the stream, `header` sets a header, and `body` answers with whatever text it is
given. The adapters' recordings of the real harnesses were made against it.

The other modules are documented placeholders: the HTTP and realtime doubles,
the notifier, summarizer and plugin doubles, the workflow harness, the
fixtures and the shaped link.

| Module | Planned responsibility |
|--------|------------------------|
| [clock](src/clock.rs) | Controlled time for product lifecycle behavior. |
| [database](src/database.rs) | Isolated PostgreSQL fixtures. |
| [programs](src/programs.rs) | A third agent program that is nothing but a manifest, for the tests of whatever lists, checks, offers and runs programs. |
| [prefix_history](src/prefix_history.rs) | Controlled-model history witnesses for native input-prefix proofs. |
| [native_configuration](src/native_configuration.rs) | Guarded native session-configuration proofs inside an already-owned Linux container. |
| [doubles/http](src/doubles/http.rs) | Contract-backed API responses and error scenarios. |
| [doubles/realtime](src/doubles/realtime.rs) | Feed player and WebSocket client. |
| [doubles/link](src/doubles/link.rs) | In-memory hub with link fault injection. |
| [doubles/machine](src/doubles/machine.rs) | Scripted role commands and events. |
| [doubles/sessions](src/doubles/sessions.rs) | Session openings for database-backed scripted harness worlds. |
| [doubles/controller](src/doubles/controller.rs) | Scripted controller driving connector roles. |
| [doubles/model](src/doubles/model.rs) | Scripted Responses upstream and client. |
| [doubles/notifier](src/doubles/notifier.rs) | A notifier of each kind of reach, for tests: one that takes an address, one that links people, one whose target is gone. |
| [doubles/driver](src/doubles/driver.rs) | In-memory environment driver. |
| [doubles/cluster](src/doubles/cluster.rs) | A cluster-shaped driver with opaque units, storage classes and independently failing machines. |
| [tests/driver_boundary](tests/driver_boundary.rs) | The shared contracts and production callers remain independent of driver implementation names. |
| [doubles/cluster/archive](src/doubles/cluster/archive.rs) | The existing owned-volume archive format for the cluster model's regular files. |
| [doubles/summarizer](src/doubles/summarizer.rs) | Rule-based summarizer. |
| [doubles/plugin](src/doubles/plugin.rs) | Plugin protocol and lifecycle fault injection. |
| [workflows](src/workflows.rs) | Harness linking visible workflows to committed outcomes and recovery. |
| [fixtures](src/fixtures.rs) | Deterministic neutral event, history, descriptor and content fixtures, with a step of every kind and the written forms they come to. |
| [slow_link](src/slow_link.rs) | The shaped link the slow-link scenarios run through: delay, rate and loss between browser and edge, at the profile and at its floor. |
| [oidc](src/oidc.rs) | A signed OpenID Connect provider on the loopback, for the tests of signing in: discovery, keys that rotate, authorization with PKCE, the code exchanged for signed tokens, user information, and the faults a provider makes. |
