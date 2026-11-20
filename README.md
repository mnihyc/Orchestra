<h1 align="center">Orch</h1>

<p align="center">A self-hosted workspace where a team runs coding agents together.</p>

<!-- Screenshot: a thread in the Live view, an agent's commands and edits streaming in, two teammates on the thread and an approval waiting above the composer.
     Add the file docs/images/thread-live.png and remove these comment marks:
![A thread in the Live view: an agent's commands and edits stream in as they happen, two teammates are on the thread, and an approval waits above the composer.](docs/images/thread-live.png)
-->

Orch lets the people on a team start coding agents such as Codex and Claude Code in shared threads, watch the work as it happens, answer the agent's questions and steer it. The conversation, the documents and the decisions stay in one place, on your own servers.

**Status:** pre-release (0.x). Some things will change before 1.0. Read the known issues in the [changelog](CHANGELOG.md) before you install.

## What you get

- **Shared threads.** Everyone with access sees an agent's commands, edits and sub-agents step by step, and can send input, approve or deny what the agent asks, or stop it.
- **Your agents, your models.** Use Codex and Claude Code subscriptions, or any compatible API key.
- **Environments where the work is.** Managed Docker environments on your hosts, or a directory shared by a Linux connector. macOS and Windows connectors can run compatible agent programs and model connections. Files, terminals, browsers and ports are available in the browser.
- **Permissions on everything.** Organizations, projects, topics and threads each have a permission page, and every change of access is recorded.
- **Documents and chat beside the work,** written by people and agents together.
- **Self-hosted.** One Linux Docker host. Logs and metrics are kept locally. Model requests and notifications go to the destinations you configure.

## Requirements

- A Linux host, x86_64 or ARM64, with Docker Engine and the Compose plugin (version 2), `bash`, GNU coreutils, `flock` and `curl`. Docker must be rootful, without user-namespace remapping.
- Recommended estimates for the host (they are estimates, not limits; the stack sets no memory or CPU limits):
  - to try Orch alone or with a colleague: 2 CPU cores, 4 GB of memory, 30 GB of disk;
  - for a team of up to about 20 people: 4 CPU cores, 8 GB of memory, 100 GB of disk (SSD).
  - Every environment you run is a container on top of this; give it the memory your agents' work needs.
- For anything beyond a trial: an HTTPS address for the web application (a reverse proxy you run), and one TCP port (9443 by default) that your team's machines can reach.

The estimates rest on what the stack is made of, not on a measurement of a busy installation: six containers (PostgreSQL with pgvector, the API, the gateway, the worker, the node and the web edge); a Default environment image of about 1.6 GiB on disk, with a browser in it; a local semantic-search runtime that asks for 4 CPUs and 2 GiB by default; at most 50 MiB of logs per service. Check your own host with `docker stats` after a week of use, and tell us what you find.

## Quick start

This starts Orch on one host for a trial. For a team, read [Install Orch](docs/operator/deployment.md) first. Pick the way that fits the host, then do the common steps.

```sh
TAG=v0.1.2                  # the latest release: https://github.com/mnihyc/Orchestra/releases/latest
ARCH=$(uname -m)            # x86_64 or aarch64
```

**Files in a directory** (for a host that cannot reach GitHub, or to choose what you install). Download these five files from the release page into one directory, then unpack the deployment bundle:

```sh
mkdir -p ~/orch-release && cd ~/orch-release
for f in version.json orch-deploy-$TAG.tar.gz orch-frontend-dist.tar.gz orch-server-linux-$ARCH.zip orchctl-linux-$ARCH-musl; do
  curl -fLO "https://github.com/mnihyc/Orchestra/releases/download/$TAG/$f"
done
mkdir -p ~/orch && tar -xzf orch-deploy-$TAG.tar.gz -C ~/orch && cd ~/orch/deploy
```

**Fetched** (the host can reach GitHub). Download only the deployment bundle; `update.sh` fetches and checks the rest:

```sh
mkdir -p ~/orch && cd ~/orch
curl -fLO "https://github.com/mnihyc/Orchestra/releases/download/$TAG/orch-deploy-$TAG.tar.gz"
tar -xzf orch-deploy-$TAG.tar.gz && cd deploy
```

**Then, in the `deploy` directory, for both ways:**

```sh
install -m 600 .env.example .env             # the database password is made in the next line
sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$(openssl rand -base64 24 | tr '+/' '-_')/" .env
sed -i 's#^public_url = .*#public_url = "http://127.0.0.1:5180"#' config/orch.toml   # for a trial on this host only
```

Start it with the line for your way:

```sh
./update.sh first-start --from ~/orch-release        # files in a directory
./update.sh first-start $TAG                         # fetched
```

`update.sh` checks every file against its checksum, starts the stack and tells you when it is ready. Open <http://127.0.0.1:5180> and create the first administrator. Then follow [Getting started](docs/user/getting-started.md): add a provider, start a thread, invite a colleague.

To let other people and machines in, edit `config/orch.toml`: set `public_url` under `[system]` to the HTTPS address people will open, and the `url` under `[[gateway.endpoints]]` to an address your machines can reach on the gateway port. Both ship as `example.com` addresses (`public_url` is a local one if you used the trial line above). Then run `docker compose restart` in the `deploy` directory. [The HTTPS front](docs/operator/edge.md) and [Connect a machine](docs/user/connect-a-machine.md) have the details.

## Upgrade

```sh
cd ~/orch/deploy
./update.sh latest                          # fetched. With files in a directory: ./update.sh vX.Y.Z --from DIRECTORY
./update.sh status                          # the version that runs, and the state of the stack
```

Read the **Upgrade notes** of the release in the [changelog](CHANGELOG.md) first. `update.sh` saves a backup before it changes anything, and `./update.sh rollback` goes back. See the [upgrade guide](docs/operator/upgrades.md).

## Documentation

- [Getting started](docs/user/getting-started.md) and the [user guide](docs/user/README.md)
- [Operator guide](docs/operator/README.md): install, configure, upgrade, back up, monitor, troubleshoot
- [Extending Orch](docs/development/extensions/README.md): agent adapters, drivers, providers, plugins
- [API](docs/openapi.json): OpenAPI 3, with scoped API keys under `/openapi/v1`
- [Security](docs/security/README.md): [secure an installation](docs/operator/security.md), the [security model](docs/security/model.md) and how to report a vulnerability · [Changelog](CHANGELOG.md) · [Releases](https://github.com/mnihyc/Orchestra/releases)

## Get help

- A question or an idea: [open an issue](https://github.com/mnihyc/Orchestra/issues/new/choose) and choose **Question** or **Feature request**.
- A bug: open an issue and choose **Bug report**. Say which Orch version you run (the output of `./update.sh status`), your Docker version, and what you saw.
- A vulnerability: please do not open an issue; see [SECURITY.md](SECURITY.md).

## Contributing

Contributions are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) explains how to build Orch from source (Rust and TypeScript), run the checks and propose a change. Everyone taking part follows the [code of conduct](CODE_OF_CONDUCT.md).

## License

[Apache License 2.0](LICENSE). Third-party components keep their own terms: see [NOTICE](NOTICE).
