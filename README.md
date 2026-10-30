# Orch

Orch is a self-hosted workspace where a team runs coding agents together. People start agents such as Codex and
Claude Code in shared threads, watch their work live, answer their questions and steer them, and keep the
conversation, documents and decisions in one place.

## Features

- **Shared threads.** Each thread runs one agent in an environment. Everyone with access sees its work step by step:
  commands with their output, edits as differences, sub-agents and their results. Anyone allowed can send input,
  steer a running turn, approve or deny what the agent asks, or stop it.
- **Organizations, projects and topics.** Work is organized in a tree; roles given at a place apply to everything
  inside it, and every organization, project, topic, thread, document and resource has its own permission page.
  People ask for missing access in one request, and grants can end on a date.
- **Providers and agent programs.** Use subscriptions (Codex, Claude Code) or API keys from any compatible endpoint.
  Agent program versions are pinned per release and can be updated to newer checked builds.
- **Environments anywhere.** Run agents in managed Docker environments on your hosts, or in a directory on a
  teammate's machine through the connector for Linux, macOS and Windows. Files, terminals, browsers, displays and
  ports of an environment are available in the browser.
- **Documents and chat.** Pages of blocks written by people and agents together, with comments and history; chats
  beside every place; agents read and write them through a small set of tools.
- **Accountable by design.** Every change of access and every act on the system is recorded in the audit trail.
  Usage is counted per person and provider, bounded by caps. Nothing is sent to any third party.

## Install

Orch runs on one Docker host with PostgreSQL. Download the deployment bundle and the server package for your
architecture from the [latest release](../../releases/latest), then:

```sh
tar -xzf orch-deploy-<version>.tar.gz
cd deploy
cp .env.example .env              # set the database password and the published addresses
./update.sh first-start --from <directory with the release files>
```

Put an HTTPS front in front of the web edge, open the address and create the first administrator. Machines join by
installing the connector from the server's own downloads. The [deployment guide](deploy/README.md) and the
[operator guides](docs/operator/README.md) cover configuration, updates, backups and recovery.

## Documentation

- [Getting started](docs/user/getting-started.md) and the [user guides](docs/user/README.md)
- [Who can see and do what](docs/user/who-can-see-and-do-what.md)
- [Connect a machine](docs/user/connect-a-machine.md)
- [Operator guides](docs/operator/README.md)
- [Architecture](docs/development/architecture.md) and [contributing](CONTRIBUTING.md)

## Building from source

Orch is written in Rust (services, connector and CLI) and TypeScript (the web application). Install Rust with
[rustup](https://rustup.rs/); the toolchain is pinned in `rust-toolchain.toml`.

```sh
cargo check --workspace --all-targets --locked
cargo xtask package --out dist     # the release files for this machine's architecture
```

See [CONTRIBUTING.md](CONTRIBUTING.md) for the full checks and the [frontend guide](frontend/README.md) for the web
application.

## License

Licensed under the [Apache License 2.0](LICENSE).
