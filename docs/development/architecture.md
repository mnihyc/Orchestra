# Repository architecture

Orch separates durable control-plane state, provider credentials and
conversations, and the environments in which threads run their tools. Contracts define the
interfaces between them. Applications compose replaceable implementations around
those interfaces. The [component inventory](components.md) links every package
and its module responsibilities.

## Source layout

| Location | Responsibility |
|----------|----------------|
| [apps/](../../apps/README.md) | Executable configuration, composition and startup |
| [crates/foundation/](../../crates/foundation/README.md) | Shared primitives and compiled build identity |
| [crates/contracts/](../../crates/contracts/README.md) | Neutral schemas, commands, events, descriptors, catalogues and interfaces |
| [crates/platform/](../../crates/platform/README.md) | Configuration, telemetry, PKI, link transport, plugin supervision and the contract suites |
| [crates/control/](../../crates/control/README.md) | Persistence, access, settings, engine, API, gateway and worker |
| [crates/runtime/](../../crates/runtime/README.md) | Connector, environment host and node |
| [crates/tooling/](../../crates/tooling/README.md) | Build support, verification, generators, release tooling and behavioral test support |
| [extensions/](../../extensions/README.md) | Concrete implementations behind contracts |
| [frontend/](../../frontend/README.md) | Static web application, routes, features and shared controls |
| [deploy/](../../deploy/README.md) | Operator configuration, installation, Compose, proxy and updater |
| [images/](../../images/README.md) | Default managed environment image |
| [migrations/](../../migrations/README.md) | Canonical database migrations |
| [examples/plugins/](../../examples/plugins/README.md) | External driver, notifier, provider and summarizer examples |
| [build/](../../build/README.md) | Component build counters |
| [scripts/smoke/](../../scripts/smoke/README.md) | Deployment behavior scenarios |
| [docs/](../README.md) | Developer, operator, security and user documentation |
| `.github/` | CI and release automation |

There is one Cargo workspace and one dependency lockfile. Contract libraries
define the wire types and extension traits; the service composition roots,
control libraries, runtime libraries and React screens implement the product
around them. Generated API artifacts come from the contracts.

## Dependency direction

| Source layer | May depend on workspace libraries in |
|--------------|--------------------------------------|
| foundation | foundation |
| contracts | foundation, contracts |
| platform | foundation, contracts, platform |
| control | foundation, contracts, platform, control |
| runtime | foundation, contracts, platform, runtime |
| extensions | foundation, contracts, platform telemetry |
| apps | All production library layers |
| tooling | All library layers |

Contracts are the meeting point between control-plane and machine-side
implementations. Neither imports the other. Composition roots in `apps/`
select extensions and pass them through contracts. Development dependencies
may cross layers to exercise real behavior; build-support is a build-only
helper. Cargo rejects cycles. These are review guidelines; there is no custom
architecture checker or test of implementation shape.

The suite that every implementation of a contract passes is a platform
library, `orch-conformance`, and not test support: the operator's
`orchctl plugin check` runs it against an external implementation. Test
doubles stay in tooling. An extension has no use for the link, the PKI or the
plugin transport, so it depends on telemetry alone among the platform
libraries. Packaging and every component's build identity belong to xtask.

One concept has one home. Shared types belong to the boundary they describe;
shared mechanisms belong to platform libraries. Settings and permission
catalogues are contracts; their decision engines belong in control libraries.
There is no catch-all common crate.

## How the product is divided

Organizations contain projects, topics and threads, and a thread runs in an environment on a machine or a host. [How the product is put together](product-model.md) describes organizations, places, threads, environments, providers and access, and who owns each.

## Checks

`cargo xtask verify` checks formatting, compilation, Clippy, rustdoc and
dependencies. CI additionally compiles native platform and musl targets.
Compilation is followed by contract and package behavior tests, PostgreSQL
integration tests, frontend checks and browser/deployment scenarios. Agent program cases need their pinned binaries and a real runner for each target. A compile or a test double does not show that a platform, a sign-in or a release stack works.
