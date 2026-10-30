# Component inventory

This map links the implemented application, contract, control and runtime
packages and the extension seams that remain reserved. Package READMEs describe
their behavior and test coverage. Release support also depends on the
validation of the packaged programs on their supported platforms.

## Executables

| Home | Responsibility |
|------|----------------|
| [api](../../apps/api/README.md) | HTTP, realtime and internal backplane composition; hosts the durable engine. |
| [gateway](../../apps/gateway/README.md) | Authenticated machine sessions, routing, tunnels and stateless backplane relay. |
| [worker](../../apps/worker/README.md) | Database and outside-world delivery/upkeep; no machine orchestration. |
| [connector](../../apps/connector/README.md) | Agent, provider and environment roles on managed or connected machines. |
| [node](../../apps/node/README.md) | Managed runtime lifecycle through driver implementations. |
| [ctl](../../apps/ctl/README.md) | Administrator, bootstrap, diagnostics, consumption and plugin commands. |

The service entry points compose the libraries below. `orchctl` provides local
administration, bootstrap, diagnostics, provider import and plugin checks.
`orch-mock-llm` in testkit is a controlled model server used by native harness
and end-to-end tests; its scripted replies are separate from real model proof.

## Libraries and implementations

The workspace manifest lists the applications, libraries and tooling below.
Contracts carry the wire types and trait seams; control packages own durable
product decisions and runtime packages enforce the machine side. Built-in OIDC
sign-in and the model summarizer have implementations; each package's README
states its behavior, tests and any remaining limitations.

| Package | Responsibility |
|---------|----------------|
| [orch-types](../../crates/foundation/types/README.md) | Shared primitive types without service or transport behavior. |
| [orch-api-contract](../../crates/contracts/api/README.md) | HTTP request and response contracts, with route metadata. |
| [orch-realtime-contract](../../crates/contracts/realtime/README.md) | Browser WebSocket contracts, including terminal sockets. |
| [orch-link-contract](../../crates/contracts/link/README.md) | Machine link and backplane schemas; commands and events shared by both ends. |
| [orch-agent-contract](../../crates/contracts/agent/README.md) | Neutral harness adapter contract and declarations. |
| [orch-agent-programs](../../crates/contracts/agent-programs/README.md) | Agent program manifests and the registry of the built-in programs. |
| [orch-driver-contract](../../crates/contracts/driver/README.md) | EnvironmentDriver contract for provider and environment runtimes. |
| [orch-executor-contract](../../crates/contracts/executor/README.md) | Executor contract for tool execution inside an environment. |
| [orch-model-contract](../../crates/contracts/model/README.md) | Wire-protocol and translator contracts for provider access. |
| [orch-plugin-contract](../../crates/contracts/plugin/README.md) | External extension JSONL contracts. |
| [orch-provider-contract](../../crates/contracts/provider/README.md) | Provider source contracts: facts, models and credential presentation. |
| [orch-settings-contract](../../crates/contracts/settings/README.md) | Settings catalogue and resolution data, without evaluation. |
| [orch-access-contract](../../crates/contracts/access/README.md) | Permission catalogue, roles and permission-page contracts, without access decisions. |
| [orch-summarizer-contract](../../crates/contracts/summarizer/README.md) | Summarizer contract for title, description and tag generation. |
| [orch-notifier-contract](../../crates/contracts/notifier/README.md) | Notification channel contract hosted by the worker. |
| [orch-blob-contract](../../crates/contracts/blob/README.md) | BlobStore trait seam; local storage is the v0.1 implementation. |
| [orch-identity-contract](../../crates/contracts/identity/README.md) | IdentityProviderKind trait seam; OIDC is the v0.1 implementation. |
| [orch-config](../../crates/platform/config/README.md) | Configuration loading shared by service and machine entry points. |
| [orch-telemetry](../../crates/platform/telemetry/README.md) | Structured logging and metrics shared by components; what a system records stays inside it. |
| [orch-pki](../../crates/platform/pki/README.md) | Certificate authority and machine identity mechanisms. |
| [orch-link](../../crates/platform/link/README.md) | Shared reliable link mechanisms; persistence of product state belongs to the API. |
| [orch-plugin](../../crates/platform/plugin/README.md) | External plugin transport, supervision and trait bridges. |
| [orch-conformance](../../crates/platform/conformance/README.md) | The suite every implementation of a contract passes, for tests and for orchctl plugin check. |
| [orch-db](../../crates/control/db/README.md) | PostgreSQL persistence, transactions and projections. |
| [orch-authz](../../crates/control/authz/README.md) | The single access-decision implementation. |
| [orch-settings](../../crates/control/settings/README.md) | Settings evaluation over the shared catalogue. |
| [orch-chat](../../crates/control/chat/README.md) | People's chats: messages, following, inspection and forwarding. |
| [orch-documents](../../crates/control/documents/README.md) | Documents: pages of blocks written in place, with their operations, comments, history, files and exports. |
| [orch-engine](../../crates/control/engine/README.md) | Durable product orchestration hosted inside the API. |
| [orch-api-core](../../crates/control/api/README.md) | API application hosting HTTP, realtime and the machine backplane. |
| [orch-gateway-core](../../crates/control/gateway/README.md) | Stateless machine gateway and tunnel relay. |
| [orch-worker-core](../../crates/control/worker/README.md) | Database and outside-world delivery and upkeep; no machine orchestration. |
| [orch-connector-core](../../crates/runtime/connector/README.md) | Managed and connected role host for agent, provider and environment services. |
| [orch-envhost](../../crates/runtime/envhost/README.md) | Environment-side files, tools, processes and interactive services. |
| [orch-node-core](../../crates/runtime/node/README.md) | Host driver composition and managed runtime lifecycle. |
| [orch-adapter-codex](../../extensions/agents/codex/README.md) | Codex app-server adapter; Codex-specific vocabulary stays here. |
| [orch-adapter-claude-code](../../extensions/agents/claude-code/README.md) | Claude Code adapter; Claude Code-specific vocabulary stays here. |
| [orch-adapter-fake](../../extensions/agents/fake/README.md) | Scripted adapter for product behavior exercises and UI development. |
| [orch-driver-docker](../../extensions/drivers/docker/README.md) | Docker Engine implementation of the runtime driver contract. |
| [orch-driver-host](../../extensions/drivers/host/README.md) | Host-process implementation of the runtime driver contract. |
| [orch-executor-codex](../../extensions/executors/codex/README.md) | Codex exec-server hosted inside a thread's environment. |
| [orch-executor-orch](../../extensions/executors/orch/README.md) | Orch's own executor: plain exec tools served through MCP, one of the executors an environment offers. |
| [orch-protocol-openai-responses](../../extensions/protocols/openai-responses/README.md) | OpenAI Responses wire-protocol implementation. |
| [orch-protocol-anthropic-messages](../../extensions/protocols/anthropic-messages/README.md) | Anthropic Messages wire-protocol implementation. |
| [orch-summarizer-model](../../extensions/summarizers/model/README.md) | Model-backed title, description and tag generation selected by configuration. |
| [orch-notifier-email](../../extensions/notifiers/email/README.md) | SMTP notification delivery across configured security and authentication modes. |
| [orch-notifier-push](../../extensions/notifiers/push/README.md) | Web Push notification delivery. |
| [orch-notifier-telegram](../../extensions/notifiers/telegram/README.md) | Telegram bot notification delivery to the people who linked a chat with the bot. |
| [orch-notifier-webhook](../../extensions/notifiers/webhook/README.md) | Signed webhook notification delivery. |
| [orch-blob-local](../../extensions/blobs/local/README.md) | Local directory implementation of content-addressed storage. |
| [orch-identity-oidc](../../extensions/identity/oidc/README.md) | OpenID Connect identity-provider implementation. |
| [orch-testkit](../../crates/tooling/testkit/README.md) | Behavioral doubles, a clock that moves when told and a database of its own for each test, for the tests of the other packages. |
| [orch-build-info](../../crates/foundation/build-info/README.md) | Compiled version and component identity shared by executables. |
| [orch-build-support](../../crates/tooling/build-support/README.md) | Build-script release validation and component counter allocation. |
| [orch-xtask](../../crates/tooling/xtask/README.md) | Rust verification, generated contracts and release packaging with manifest validation. |

[The extension host map](../../extensions/README.md) includes the translator
seam for external implementations and one reserved protocol. [External examples](../../examples/plugins/README.md)
cover driver and summarizer plugin homes; neither is executable.

## Frontend

The shared foundation covers the workbench that is the shell, typed API and realtime clients,
live stores, routing, generated contracts, UI primitives, context menus,
the help index, themes, localization and tooling.
The feature areas below contain the screens and their controls. Their READMEs
describe the implemented behavior and any remaining work.

| Feature home | Responsibility |
|--------------|----------------|
| [workbench](../../frontend/src/features/workbench/README.md) | The workbench: areas, views and arrangements |
| [auth](../../frontend/src/features/auth/README.md) | Setup, registration, signing in by each proof, and invitations |
| [onboarding](../../frontend/src/features/onboarding/README.md) | First-run guidance |
| [navigation](../../frontend/src/features/navigation/README.md) | Organization and work navigation, and the finder |
| [help](../../frontend/src/features/help/README.md) | Hints and help |
| [search](../../frontend/src/features/search/README.md) | Authorized global search |
| [org](../../frontend/src/features/org/README.md) | Organization home, the same at every level |
| [project](../../frontend/src/features/project/README.md) | Project topics and environments |
| [fleet](../../frontend/src/features/fleet/README.md) | Fleet: environments, provider runtimes and recycling |
| [topic](../../frontend/src/features/topic/README.md) | Topic: threads, environments and collaboration |
| [announcements](../../frontend/src/features/announcements/README.md) | Configurable human broadcasts |
| [thread](../../frontend/src/features/thread/README.md) | Shared conversation screen |
| [timeline](../../frontend/src/features/timeline/README.md) | The conversation: the agent's work step by step, side threads, selection and history |
| [composer](../../frontend/src/features/composer/README.md) | Inputs and delivery queue |
| [inspector](../../frontend/src/features/inspector/README.md) | The views of a thread: workspace, runtime and people |
| [files](../../frontend/src/features/files/README.md) | Files and code |
| [access](../../frontend/src/features/access/README.md) | Access: the permission page |
| [resources](../../frontend/src/features/resources/README.md) | Providers, profiles, environments, hosts, connected machines and templates |
| [settings](../../frontend/src/features/settings/README.md) | Personal and hierarchical configuration |
| [admin](../../frontend/src/features/admin/README.md) | System management |
| [start](../../frontend/src/features/start/README.md) | Type-and-go and explicit binding |
| [fork](../../frontend/src/features/fork/README.md) | Fork, switch, move and update |
| [inbox](../../frontend/src/features/inbox/README.md) | Notification inbox |
| [approvals](../../frontend/src/features/approvals/README.md) | All approval kinds |
| [usage](../../frontend/src/features/usage/README.md) | Usage at three depths, caps, rates and passing a limit |
| [chat](../../frontend/src/features/chat/README.md) | Chats among people |
| [forward](../../frontend/src/features/forward/README.md) | Selecting and forwarding |
| [documents](../../frontend/src/features/documents/README.md) | Documents written in blocks |

See [Frontend](../../frontend/README.md) for shared infrastructure, routes,
assets and validation commands. The TypeScript and OpenAPI files are generated
from the contract crates and checked against their committed forms.

## Operations and documentation

| Home | Responsibility |
|------|----------------|
| [Migrations](../../migrations/README.md) | Canonical schema; no empty SQL migration |
| [Deploy](../../deploy/README.md) | Compose, proxy, config, updater, installers, the harness build catalogue, and the rate catalogue a release comes with |
| [Images](../../images/README.md) | Default managed environment image recipe |
| [Build counters](../../build/README.md) | One build counter for each shipped component |
| [Deployment scenarios](../../scripts/smoke/README.md) | Isolated first-start, update, rollback, recovery and managed-runtime checks |
| [CI](../../.github/workflows/ci.yml) | Rust, web, dependency and deployment checks |
| [Release](../../.github/workflows/release.yml) | Validated native inputs, package assembly and reviewed release drafts |
| [Harness canary](../../.github/workflows/harness-canary.yml) | Daily compatibility checks of vendor harness builds with recorded evidence |
| [Automation helpers](../../.github/scripts/README.md) and [release notes](../../.github/release-notes/README.md) | Release validation and user-facing release information |
| [Public docs](../README.md) | User, operator, security and extension guides; the user guide also supplies application help; generated OpenAPI |

This inventory describes where code lives. Release support depends on the
behavioral tests, deployment scenarios and native platform checks for the
exact packaged candidate; a source listing alone does not establish it.
