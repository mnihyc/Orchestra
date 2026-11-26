# Deployment

This directory is what runs an Orch system on one Docker host: the Compose stack, the web edge's configuration, the settings, the updater and the installers. A release carries it as `orch-deploy-<tag>.tar.gz`. Unpack that where the system is to live (`/opt/orch`, say) and work in its `deploy/` directory. The step-by-step pages are in the [operator guide](../docs/operator/README.md), starting with [Install Orch](../docs/operator/deployment.md). This page says what the pieces are and how they fit together.

Run one API process: terminal, browser and display tickets and live tunnels belong to the process that issued them, and API replicas share no live-ticket routing. Keep `.env` and the secret files readable by their owner alone.

| File | What it is |
|------|------------|
| [compose.yml](compose.yml) | PostgreSQL, API, gateway, worker, node and the web edge, on stock images with the release mounted read-only |
| [nginx.conf](nginx.conf) | The web edge: the built application, the proxy to the API and the machines' link, the security headers |
| [.env.example](.env.example) | What the operator decides (database password, where the edge and the gateway are published); copy to `.env` |
| [config/orch.toml](config/orch.toml) | The deployment's non-secret settings, read by every service |
| `config/secrets/` | The secrets, made once by `update.sh first-start`; never committed |
| [update.sh](update.sh) | First start, verified updates (also from a directory) that also bring the files of this directory, backup, rollback, recovery of an interrupted transaction, and `doctor`: the readiness checks of the stack and its edge, once |
| [FILE_SETS](FILE_SETS) | What an update reads of these files: their generation, the oldest generation the release's programs work with, the set of files that must agree, and the SHA-256 of every file of every published release |
| [model-rates.json](model-rates.json) | The rate catalogue a release comes with: estimates by model family, each with the page it was read from and the day. Replaced by an upload, a fetch or a file given on the host |
| [harness-builds.json](harness-builds.json) | Pinned downloads of the harnesses; copied into the release's manifest |
| [install/](install/) | The connector installers for Linux, macOS and Windows, and the node's |
| [postgres/](postgres/README.md) | The database image: the pinned PostgreSQL image with pgvector for search by meaning, made by Compose on first start |
| `runtime/` | The default location of the database and server files. Also holds the node's state, the unpacked releases, the dumps and the updater's journal. Created by `update.sh`, ignored by git |

## The stack

```
        browser ──► HTTPS front you run ──► frontend (nginx) ──► api ──► postgres
                                                  │              ▲
        machines ──► gateway :9443 (pinned)       └─► gateway :9444 (behind the front)
        managed environments ─► the same pinned port    node ──► Docker socket
```

- **Networks.** `backend` holds the database and the services that use it (postgres, api, worker). `control` holds everything that talks to the API or the gateway (api, gateway, worker, node, frontend).
- **Ports.** Two are published: the web edge (`ORCH_FRONTEND_BIND`, on this host's loopback by default) and the gateway's pinned listener (`ORCH_GATEWAY_PUBLISH`). Only the node mounts the Docker socket.
- **Logs and health.** Logs rotate at 10 MiB, five files each. Every service that has something to say about its health has a health check; the API's is `orchctl doctor` run inside it.
- **The web edge** carries the browser's `/api/v1/` and the keys' `/openapi/v1/`, with their separate realtime sockets, and the machines' link to the services behind it. It refuses `/internal/` and answers `POST /edge-check` with 204 after reading the whole body, which is what `orchctl doctor --url` sends 8 MiB to, so that a front that cuts or refuses a large body is found.
- **Mounts.** The server payload is `runtime/server/current`, read-only at `/opt/orch`. The command line is `runtime/cli/current` at `/opt/orchctl`. The application is `runtime/frontend/current/dist` in the web edge. Server files are mounted at `/var/lib/orch` from `ORCH_DATA_DIR` (default `./runtime/data`), and `config/orch.toml` is one file at `/etc/orch/orch.toml`. Every secret is bound by itself under `/run/secrets`, so a service sees only the secrets it needs. Nothing else of the host is visible to a service, and every service but the database has a read-only root file system.
- **The files that restore a turn.** The default setup's persistent data includes the content store that turn-file recording and restore use; no outside blob service is needed. Keep the configured server directory and the environments' persistent homes or connector data when updating ([Configure Orch](../docs/operator/configuration.md#keep-the-files-that-restore-a-turn)).
- **Who the services run as.** The services write as the person who runs the stack (`ORCH_UID` and `ORCH_GID` in `.env`, written by `first-start`). The files, the database's included, are theirs to back up, move and remove without privilege. `initdb` wants the account it runs as to have a name, so `update.sh` writes `runtime/postgres/passwd` and Compose binds it over `/etc/passwd` of that one service. When `update.sh` runs as root it takes the owner of this directory, or the unprivileged id 10001 if root owns it, and gives `runtime/` to that id. The node needs the Docker socket and is given its group. This assumes the service's numeric UID is unchanged across bind mounts, as with a rootful Docker daemon without user-namespace remapping. Rootless Docker and remapping need a different arrangement.
- **The node's directory** is mounted at the same path inside the container as on the host, named by `ORCH_NODE_DATA_DIR`, because the Docker driver hands paths of its tool cache to the Docker daemon, which resolves them on the host. That is why `.env` has `ORCH_HOME`. For the same reason the node runs its program from `runtime/server/current` at the path it has on the host: a check of an agent program, a live mount and a volume copy give that program to a unit of their own.
- **Public roots.** The stock Debian image has no certificate authorities, and the services reach public servers (mail, bots, webhooks, push). The server payload carries `share/ca-certificates.crt`, and `SSL_CERT_FILE` names it. OpenSSL, which the push crate links, is compiled into the programs.

### The gateway's addresses

Three settings name the pinned listener, and they are not the same address.

- **What the system advertises** is `[[gateway.endpoints]]` in `config/orch.toml`: the entrances machines are told to dial. The shipped file holds one example, `wss://gateway.example.com:9443`, and Orch tells no machine an example address. Replace it before people add machines or make environments. [Run the HTTPS front](../docs/operator/edge.md#2-set-the-addresses) explains the list.
- **What a managed environment is told** is `[gateway] managed_endpoint`, by default `wss://gateway.orch.internal:9443`. That name is the one the gateway's certificate is good for and a network alias of the gateway on `control`. The node gives each environment it makes a host entry that sends it to the Docker host, where the pinned port is published.
- **What the bundled node dials** is `[node] gateway_url`, `wss://gateway:9443`: the gateway by its Compose service name, inside the stack. The node asks that address for the system's list the first time, and enrolls through an advertised entrance that answers, or at that address when none is advertised.

With the example address left in place nothing is advertised: `orchctl doctor --url` reports `the system advertises no endpoint, so no machine can connect`, **Add machine** gives no command, and an environment cannot connect. A real but wrong address is advertised as it is and fails loudly: the bundled node logs `waiting to enroll: the system cannot be reached yet`, tries again every ten seconds, warns again every ten minutes, and `orchctl doctor` fails its `node` line until the address is corrected and the services restarted (`docker compose restart`).

For gateways in several regions, see [Run regional gateways](../docs/operator/regional-gateways.md).

To put the server files or PostgreSQL on another disk, see [Move data to another disk](../docs/operator/deployment.md#move-data-to-another-disk).

## Agent program builds

[harness-builds.json](harness-builds.json) pins, for each of the three built-in agent programs and each Linux processor, the build a managed environment is given: its version, the address the vendor publishes it at, and the SHA-256 of what is there. A release copies the list into its manifest (`harness_builds`). `cargo xtask package` refuses a list whose entries are not well formed (an address that is not `https`, a hash that is not 64 hexadecimal digits, two defaults for one processor).

When an environment is made, the node of its host downloads the default build of every agent program the template lets run in it, checks the hash, and keeps it in its tool cache, `runtime/node/tools/<program>/<version>/`, whole or not at all. The environment sees it read-only under `/opt/orch/tools/<program>`. A version that is in the cache is never replaced: a newer pin is another version. The host of the node has to reach the vendors' addresses over HTTPS, once for each version, or a mirror ([Choose where agent program builds come from](../docs/operator/agent-programs.md)). Allow about 420 MB for Codex and Claude Code together, and 95 MB downloaded (400 MB unpacked) for DeepSeek Harness.

- **Codex** (Apache-2.0): the vendor's package archive of a GitHub release, `codex-package-<processor>-unknown-linux-musl.tar.gz`: a statically linked program with the helpers it works with beside it (a sandbox helper, ripgrep, the host of its code mode), which asks nothing of the image.
- **Claude Code** ([the vendor's terms](https://code.claude.com/docs/en/legal-and-compliance)): the program from the vendor's release bucket, `…/claude-code-releases/<version>/<platform>/claude`, linked dynamically against the C library, so any image of a distribution with glibc runs it. It is run as published. The vendor's terms for running it in a product or a hosted sandbox (the program must not be modified, and each person signs in with their own credentials) are the operator's to accept.
- **DeepSeek Harness** (MIT, with [the notices of what it bundles](https://pypi.org/project/deepseek-harness-runtime-bin/) in the wheel): the platform wheel of the vendor's runtime package on PyPI, `deepseek_harness_runtime_bin-<version>-py3-none-manylinux_2_28_<processor>.whl`. The node unpacks it into its tool cache and runs the one program inside it. It needs no Python and no Node.js on the host or in the image. The address is the file's permanent one on `files.pythonhosted.org`, and the SHA-256 in the list is the one PyPI publishes for that file.

To move a pin, read the vendor's own page for the release. Use Codex's published checksum list, or Claude Code's signed `manifest.json` after checking its signature against the vendor's published key fingerprint, and compare the download's SHA-256 with that value before you put the address and checksum in the list. DeepSeek Harness uses the SHA-256 of its PyPI file record, checked against the downloaded wheel. A system administrator can also add a build to a running system (`PUT /api/v1/admin/harnesses/<program>/<version>`): an added build replaces the release's for the same program, version and processor (that is how a host without access to the vendors is pointed at a mirror), and one marked default becomes the program's default.

## Secrets

`orchctl compose-secrets` makes them once, run by `first-start` inside a container as the services' user: the authority's key and certificate, the master key that seals what the database stores, the Web Push key, the first certificate of each service, and the token with which the bundled node enrolls. It refuses a directory that holds only part of the set. Losing `orch_master_key` makes stored API keys, authenticator secrets, mail passwords and bot tokens unreadable, and losing `orch_ca_key.pem` means enrolling every machine again. Back `config/` up off the host ([Back up and restore Orch](../docs/operator/backup-and-restore.md)). The API reads the installation's port assertion key from `config/secrets/`: keep it with the other secrets so a restart keeps the signing identity.

## Releases and the updater

A release is a directory of files and a `version.json` that lists them with the size and BLAKE3 hash of each. `cargo xtask package --out DIR` makes one from a source checkout for the platform of the machine's Docker. `update.sh` requires and checks the server, frontend and CLI payloads, and any other listed file that is present, before it installs anything. With `--from DIR` it fetches no release file (Docker may still need the network for images or the database build). Without `--from`, it fetches `<ORCH_RELEASE_URL>/download/<tag>/version.json` or `<ORCH_RELEASE_URL>/latest/download/version.json` and then the three payloads and the deployment bundle. `.env.example` sets `ORCH_RELEASE_URL` to this project's GitHub releases; an installation without outside network empties it and passes `--from`.

Three payloads are installed: `server`, `frontend` and `cli`. Each lives in
`runtime/<payload>/releases/<tag>-<hash>/`, and `current` and `previous` beside
it are links to a release. A link is replaced by one rename, so it is never half
made; unpacking happens in a staging directory that is renamed into place when
it is whole. `.env`, `config/orch.toml` and the secrets are never touched by an
update; a missing port assertion key is added to an older complete set. When a
release needs a configuration change in `config/orch.toml` its notes say so. The
other files of this directory (`compose.yml`, `compose.frontend-https.yml`,
`nginx.conf`, `frontend.sh`, `postgres/Dockerfile`, `hosts/runtimes/*`, `update.sh`
and the pages and catalogues) follow the release, as the next paragraphs say. The
key HTTP and WebSocket routes require the `/openapi/` and `/openapi/v1/ws` blocks
of the shipped `nginx.conf`, forwarded without stripping the prefix; keep them in
any outer HTTPS proxy or CDN in front of the edge too.
Installer downloads remain under `/api/v1/downloads/`. The
server archive is unpacked with `unzip` where the host has it, and
otherwise by a container of the web edge's image (`ORCH_UPDATE_NO_UNZIP=1` takes that
path on any host). The images that the manifest names (the default environment image)
are pulled when they are not on the host; a release made on the host has them.

**The deployment files** are the files of the release's `orch-deploy` bundle under
`deploy/`, but `config/`. An update takes the bundle with the payloads (it is
listed in `version.json`) and decides, before anything is stopped, what becomes of
each. The bundle's `FILE_SETS` lists the SHA-256 of every file of every published
release; a file that equals one of them is the product's and is replaced, and one
that equals none is the operator's edit and is kept. The files that must agree with
each other (`compose.yml`, `compose.frontend-https.yml`, `nginx.conf`, `frontend.sh`,
`postgres/Dockerfile`, `hosts/runtimes/*`) are one set: replaced whole, or, when any
member is edited, kept whole with each member that differs from the new release's
written beside as `<name>.new`. `./update.sh status` lists the kept files until the
`.new` files are gone; the update writes a `.new` again only when the shipped file
changed. Before it replaces the set it checks that the new `postgres/Dockerfile`
builds the data directory's PostgreSQL major, that the new `compose.yml` binds the
same directories of `runtime/` and `config/` and names the same project, and that
every image it names is on the host (pulled if not), and keeps the set, saying what
to do in one line, when one fails. The release states the oldest generation of the
set its programs work with (`deploy_files.minimum` in `version.json`, raised by hand
only when a release needs a change in these files, with its reason in the release
notes): an update whose kept or old set is older stops before it stops anything and
names the `.new` files to merge. `update.sh` itself is replaced at the end of a
successful update and is never rolled back. With `--from` and no bundle nothing of
this follows, and the update says so.

An update, a rollback and a first start are transactions. Before each step
`update.sh` writes it to `runtime/update/journal`:

```
begin ─ staged ─ stopping ─ dumping ─ filing ─ activating ─ (restoring_entered) ─ starting ─ verifying ─ done
```

`stopping` stops the application services (the database stays); `dumping` takes
a custom-format dump of the database, reads it back and keeps it under
`runtime/update-backups/`; `filing` writes the deployment files, keeping the
earlier ones under `runtime/update/files/`; `activating` moves the links; `starting` recreates
the services; `verifying` waits for `orchctl doctor --expect-version` to pass in
the API's container, so that every service is up and reports the new release.
On success the journal is closed and `runtime/update/state` records the installed
and the previous release and the dump of the database as the previous release left
it. On failure, or when readiness does not come within five minutes
(`ORCH_READY_TIMEOUT`), the services are stopped, the links are put back, the
database is made again from the dump and the old release is started and
verified. `update.sh recover` reads the journal after a power cut or a Ctrl-C:

| It stopped at | `recover` |
|---------------|-----------|
| `begin` … `activating` | Nothing ran the new release and the database is as it was: the files and the links go back and the old release starts |
| `restoring_entered` (a rollback), `restoring`, `restarting` | The database was being replaced: it is made again from the dump of what was left, and the old release starts |
| `starting`, `verifying` | The new release may be fine: it is started, and kept if it becomes ready and reports its tag, undone like a failed update if not |
| `done` | The record is closed |

A first start that was cut off is taken up again by repeating the original
`first-start` command, including `--from` and any `--restore-database` option.

`rollback` swaps `current` and `previous` with the same safeguards, puts back the
deployment files the last update replaced (each only where it is still what the
update wrote; `update.sh` stays the newer one), and puts the
database back as it was when the release being entered was left: compatible
payloads and database state together. What was written since is lost, and it says
so; a dump of the present database is kept first, so `rollback` again goes forward.
`backup` takes a dump without stopping anything.

What an update does to running work is printed before it starts: the number of
threads running (their turns are not interrupted, but exec tunnels that are not
back within the 60 s grace are reset) and how many environments and provider
runtimes a raised `connector_minimum` leaves unable to work until they are
updated. Prerelease connectors on the minimum's numeric version are reported for
an explicit compatibility check. If the work inventory cannot be read, the
updater stops before stopping services. `--yes` answers the question when nobody
can be asked; no question is needed when there is no affected work.

## Installers

The system serves its release's installers, connector and node at `/api/v1/downloads/`, and `connectors/version.json` lists each with its size, BLAKE3 and SHA-256. An installer checks what it fetched by size and SHA-256 (a shell and PowerShell have a SHA-256 tool everywhere and a BLAKE3 one almost nowhere; `b3sum` is used too where it is installed). The commands Orch shows when a machine or a host is added carry every value quoted for the shell they are pasted into.

| Installer | Runs as; program and data | Keeps running |
|-----------|--------------------------|---------------|
| `install.sh` (Linux, macOS) | **Your account**, without sudo; `~/.local/bin/orch-connector`. Linux data: `${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector`; macOS: `~/Library/Application Support/Orch/connector` | User service: `orch-connector.service` on Linux, `dev.orch.connector` on macOS. Linux stops at logout unless lingering is enabled |
| `install.ps1` (Windows) | **Your account**, without administrator rights; `%LOCALAPPDATA%\Orch\orch-connector.exe`, data in `%LOCALAPPDATA%\Orch\connector` | Logon task `Orch Connector`; runs while you are logged in |
| `install-node.sh` (Linux hosts) | **Root**; `/usr/local/bin/orch-node`, `/etc/orch/orch.toml`, data in `/var/lib/orch-node` | System service `orch-node.service`; one node per machine |

Read `sh install/install.sh --help`, `sh install/install-node.sh --help` or, in PowerShell, `./install/install.ps1 -Help` without a token or any installation. Help follows the shell's locale or PowerShell's UI language (English or Chinese). Pass `--gateway automatic` or `--gateway <id>` (`-Gateway` on Windows) to choose a gateway ([Run regional gateways](../docs/operator/regional-gateways.md)).

- **System addresses use HTTPS**, with one development exception: HTTP is accepted only for literal loopback hosts (`127.0.0.0/8` in four decimal octets, `[::1]`, or `localhost`, with an optional port). Run a loopback command on the computer that hosts that address. LAN or public HTTP, DNS lookalikes, userinfo, query strings and fragments are refused before a download. For a node this applies to `--from`; its pinned gateway `--url` stays `wss` or `https`. Curl follows HTTPS redirects only; wget and PowerShell need the canonical address without a redirect. The shell installers need curl or a wget that supports `--max-redirect` (BusyBox wget does not). For a node's usual direct gateway on port 9443, pass the people's system address as `--from`: the gateway port does not serve the installer downloads. The command in Orch supplies both addresses.
- **Run again.** Each installer may be run again: a matching program is kept, and a supplied token is checked even for an existing identity. Use a bound **Enroll again** token for that identity. The service is written and restarted unless service installation is disabled. After a failed run, a program or service that was newly installed is removed where possible; a replacement of an existing one is not restored. Named connector services have a suffix derived from the data directory, so their service commands must use the exact same path.
- **Stop, add and remove** a connector or a node: [Connect a machine](../docs/user/connect-a-machine.md) and [Clean up storage and uninstall](../docs/operator/storage-cleanup.md#uninstall-a-connector-or-a-node).

## Checks

`scripts/smoke/` holds the scenarios (`first-start`, `update`, `rollback`, `recover`, `restore`, and `managed-environment`, which uses the stack until an environment made from the Default template has run a turn on the bundled host). Each makes a stack of its own from packaged releases, runs what is described above and says which promise was kept. They are run by hand and by the release workflow's `scenarios` job, which gives them the release it packaged. Each needs Docker, and packages the releases it is not given. `ORCH_UPDATE_CRASH_AT=<step>` stops `update.sh` dead right after it wrote that step, as the recover scenario does; it is never set in a deployment.
