# Deployment

This directory is what runs an Orch system on one Docker host: the Compose
stack, the web edge's configuration, the settings, the updater and the
installers. A release carries it as `orch-deploy-<tag>.tar.gz`; unpack that where
the system is to live (`/opt/orch`, say) and work in its `deploy/` directory.
The step-by-step pages are in [the operator guides](../docs/operator/README.md);
this page says what the pieces are and how they hold together.

Run one API process in this release. Terminal, browser and display tickets and
live tunnels belong to the process that issued them; API replicas have no
shared live-ticket routing. Use [incident diagnosis](../docs/operator/incidents.md)
for local health checks, account recovery and cleanup scoped to this system's
runtime labels. Keep `.env` and deployment secret files owner-readable only.

| File | What it is |
|------|------------|
| [compose.yml](compose.yml) | PostgreSQL, API, gateway, worker, node and the web edge, on stock images with the release mounted read-only |
| [nginx.conf](nginx.conf) | The web edge: the built application, the proxy to the API and the machines' link, the security headers |
| [.env.example](.env.example) | What the operator decides (database password, where the edge and the gateway are published); copy to `.env` |
| [config/orch.toml](config/orch.toml) | The deployment's non-secret settings, read by every service |
| `config/secrets/` | The secrets, made once by `update.sh first-start`; never committed |
| [update.sh](update.sh) | First start, verified updates (also from a directory), backup, rollback and recovery of an interrupted transaction |
| [model-rates.json](model-rates.json) | The rate catalogue a release comes with: estimates by model family, each with the page it was read from and the day. Replaced by an upload, a fetch or a file given on the host |
| [harness-builds.json](harness-builds.json) | Pinned downloads of the harnesses; copied into the release's manifest |
| [install/](install/) | The connector installers for Linux, macOS and Windows, and the node's |
| [postgres/](postgres/README.md) | The database image: the pinned PostgreSQL image with pgvector for search by meaning, made by Compose on first start |
| `runtime/` | Everything the stack writes: the database, the blobs, the node's state, the unpacked releases, the dumps and the updater's journal. Created by `update.sh`, ignored by git |

## The stack

```
        browser ──► HTTPS edge you run ──► frontend (nginx) ──► api ──► postgres
                                                  │              ▲
        machines ──► gateway :9443 (pinned)       └─► gateway :9444 (behind the edge)
        managed runtimes ─► the same pinned port        node ──► Docker socket
```

Two networks. `backend` is the database and the services that use it
(postgres, api, worker); `control` is everything that talks to the API or the
gateway (api, gateway, worker, node, frontend). Two ports are published: the web
edge (`ORCH_FRONTEND_BIND`, on this host's loopback by default) and the gateway's
pinned listener (`ORCH_GATEWAY_PUBLISH`). Only the node mounts the Docker
socket. Logs rotate at 10 MiB, five files each. Every service that has
something to say about its health has a health check; the API's is
`orchctl doctor` run inside it.

**The web edge** answers more than the application: it carries the browser's `/api/v1/`
and keys' `/openapi/v1/`, including their separate realtime sockets, and the machines'
link to the services behind it, refuses `/internal/`, and answers
`POST /edge-check` with 204 after reading the whole body, which is what `orchctl doctor --url` sends 8 MiB
to, so that a front that cuts or refuses a large body is found out (the API would close the
connection on a body it will not take, and say nothing about the front).

**Mounts.** The server payload is `runtime/server/current`, read-only at
`/opt/orch`; the command line is `runtime/cli/current` at `/opt/orchctl`; the
application is `runtime/frontend/current/dist` in the web edge; data is
`runtime/data` at `/var/lib/orch`; `config/orch.toml` is one file at
`/etc/orch/orch.toml`; every secret is bound by itself under `/run/secrets` (a
service sees only the secrets it needs). Nothing else of the host is visible to a
service, and the root file system of every service but the database is
read-only.

The default setup's persistent data also includes the content store used by turn-file recording and
restore; no external blob service is required. Keep `runtime/data` and the environments' persistent
homes or connector data when updating. The [configuration guide](../docs/operator/configuration.md#recorded-files-and-restore)
explains the server manifests and machine-side immutable file objects.

**Who the services run as.** Everything writes under `runtime/`, so everything
runs as the person who runs the stack (`ORCH_UID` and `ORCH_GID` in `.env`,
written by `first-start`): the files are theirs to back up, move and remove
without privilege, including the database's, which the PostgreSQL image would
otherwise give to its own user. `initdb` wants the account it runs as to have a
name, so `update.sh` writes `runtime/postgres/passwd` and Compose binds it over
`/etc/passwd` of that one service. When `update.sh` is run as root it takes the
owner of this directory, or the unprivileged id 10001 if root owns it, and gives
`runtime/` to that id. The node needs the Docker socket and is given its group.
This assumes the service's numeric UID is unchanged across bind mounts, as with
a rootful Docker daemon without user-namespace remapping. Rootless Docker and
remapping need a different ownership and mount arrangement.

**The node's directory** is mounted at the same path inside the container as it
has on the host and named by `ORCH_NODE_DATA_DIR`, because the Docker driver
hands paths of its tool cache to the Docker daemon, which resolves them on the
host. That is why `.env` has `ORCH_HOME`.

**Public roots.** The stock Debian image has no certificate authorities, and
the services reach public servers (mail, bots, webhooks, push). The server
payload therefore carries `share/ca-certificates.crt`, and `SSL_CERT_FILE`
names it; OpenSSL, which the push crate links, is compiled into the programs.

**The gateway's addresses.** Three settings name the pinned listener, and they
are not the same address.

- *What the system advertises* is `[[gateway.endpoints]]` in `config/orch.toml`:
  the entrances that machines are told to dial. The shipped file holds one
  placeholder, `wss://gateway.example.com:9443`, and an example address is told
  to no machine. Replace it before people add machines or make environments,
  with an address that resolves and answers on the published port
  (`ORCH_GATEWAY_PUBLISH` in `.env`) from machines outside, from inside the
  stack, and from the host's own Docker containers: a machine outside enrolls
  and connects through the entrances it is told, and so do the units of
  environments. The gateway's certificate is checked against the system's own
  authority, so a name and an IP address both serve. "Add machine" makes no
  command while the list holds nothing but examples, or `gateway.orch.internal`.
  An address that is real is told as it is, whether or not it answers.
- *What a managed runtime is told* is `[gateway] managed_endpoint`, by default
  `wss://gateway.orch.internal:9443`. `gateway.orch.internal` is the name the
  gateway's certificate is good for and a network alias of the gateway on
  `control`; the node gives each runtime it makes a host entry that sends it to
  the Docker host, where the pinned port is published. It means something only
  here, so it is not what is advertised to people's machines.
- *What the bundled node dials* is `[node] gateway_url`, `wss://gateway:9443`:
  the gateway by its Compose service name, inside the stack. The node asks that
  address for the system's list the first time; when the system advertises an
  entrance, it enrolls through one that answers, and when it advertises none,
  at that address itself. Once enrolled, it keeps its session at that address.

So the shipped file starts: the bundled node comes online without any edit.
With the placeholder left in place nothing is advertised, `orchctl doctor
--url` reports `the system advertises no endpoint, so no machine can connect`,
"Add machine" gives no command and an environment's unit cannot connect. An
address that is real but wrong is advertised as it is, and fails loudly: the
bundled node logs, once, `waiting to enroll: the system cannot be reached yet
reason=the system could not be reached: could not connect: failed to lookup
address information` (for a name that does not resolve), tries again every ten
seconds, warns again every ten minutes, and `orchctl doctor` fails its `node`
line (`no host has enrolled, though the first administrator exists`) until the
address is corrected and the services restarted (`docker compose restart`).

**Regional gateways.** With no `gateway.instances`, the existing `gateway_id` and
`gateway.endpoints` form one entry. To add regions, list each gateway's stable
ID, operator label, region and pinned/public entrances in `config/orch.toml` as
shown there. Each deployed gateway needs its own matching `gateway_id`, service
certificate and existing outbound API backplane. Gateways have no peer links or
database connection. Admin catalogue edits use a persisted system override;
they change advertised metadata, without deploying a gateway or moving a healthy
connector.

Each connector keeps one primary for control and people's connections. A tool
pair uses the executor's primary gateway: the other connector attaches directly
there for tunnels. If that entrance cannot serve both ends, the frozen pair
plan tries a common gateway and reports a labelled refusal if none works.
Tunnel-only sessions close after `link.tunnel_session_idle_secs` of being unused
(60 seconds by default); live channels and thread references protect them.
Machine-to-machine tunnel bytes pass through one gateway and bypass the API.
If a gateway process is lost, affected tunnels reset and ordinary interrupted
work recovery applies. Deploy the API and gateways with this capability before
enabling regional connector choices.

## Harness builds

[harness-builds.json](harness-builds.json) pins, for each of the three built-in harnesses and each Linux processor, the
build a managed environment is given: its version, the address the vendor publishes it at, and the SHA-256 of what
is there. A release copies the list into its manifest (`harness_builds`); `cargo xtask package` refuses a list whose
entries are not well formed (an address that is not `https`, a hash that is not 64 hexadecimal digits, two defaults for
one processor), and nothing of a harness is kept in this repository.

When a unit is made, the node of its host downloads the default build of every harness the template lets run in it
(and the one whose exec server it offers), checks the hash, and keeps it in its tool cache, `runtime/node/tools/<harness>/<version>/`,
whole or not at all; the unit sees it read-only under `/opt/orch/tools/<harness>`. So the host of the node has to
reach the vendors' addresses over HTTPS, once for each version (about 420 MB for the first two, and 95 MB downloaded for the third, 400 MB once unpacked), and a unit made while a
vendor is out of reach is not made until the build is pointed at a mirror (below). A version that is in the cache is
never replaced: a newer pin is another version.

- **Codex** (Apache-2.0): the vendor's package archive of a release on GitHub, `codex-package-<processor>-unknown-linux-musl.tar.gz`: a
  statically linked program with the helpers it works with beside it (a sandbox helper, ripgrep, the host of its code
  mode), which asks nothing of the image.
- **Claude Code** ([the vendor's terms](https://code.claude.com/docs/en/legal-and-compliance)): the native program from the vendor's release bucket, the one its installer
  reads, `…/claude-code-releases/<version>/<platform>/claude`. It is dynamically linked against the C library and
  needs nothing else, so any image of a distribution with glibc runs it, the Default template's among them. It is
  run as published; the vendor's terms for running it in a product or a hosted sandbox (the program must not be
  modified, each person signs in with their own credentials) are the operator's to accept.
- **DeepSeek Harness** (`dsh`; MIT, with [the notices of what it bundles](https://pypi.org/project/deepseek-harness-runtime-bin/) in the wheel): the platform wheel of the vendor's runtime package on PyPI,
  `deepseek_harness_runtime_bin-<version>-py3-none-manylinux_2_28_<processor>.whl`. A wheel is a zip archive; the node
  unpacks it into its tool cache and runs the one native program inside it (`deepseek_harness_runtime/runtime/deepseek-harness-sdk-runtime-linux-<x64|arm64>`,
  with its `-rg` sibling beside it). It needs no Python and no Node.js on the host or in the image. The address is the
  file's permanent one on `files.pythonhosted.org`, so the SHA-256 in the list is the one PyPI publishes for that file
  (checked by downloading it).

To move a pin, read the vendor's own page for the release. Use Codex's published checksum list or Claude Code's signed
`manifest.json` after verifying its signature against the vendor's published key fingerprint. Compare the download's
SHA-256 with that vendor value before putting the address and checksum in the list. DeepSeek Harness uses the SHA-256 of its PyPI file record, checked against the downloaded wheel. A system admin can also add builds
to a running system (`PUT /api/v1/admin/harnesses/<harness>/<version>`); an added build replaces the release's of the
same harness, version and processor (that is how a host without access to the vendors is pointed at a mirror), and one
marked default becomes the harness's default.

## Secrets

`orchctl compose-secrets` makes them once, run by `first-start` inside a
container as the services' user: the authority's key and certificate, the master
key that seals what the database stores, the Web Push key, the first certificate
of each service, and the token with which the bundled node enrolls. It refuses a
directory that holds only part of the set. Losing `orch_master_key` makes stored
API keys, authenticator secrets, mail passwords and bot tokens unreadable;
losing `orch_ca_key.pem` means enrolling every machine again. Back
`config/` up off the host (see [backup and restore](../docs/operator/backup-and-restore.md)).

## Releases and the updater

A release is a directory of files and a `version.json` that lists them with the
size and BLAKE3 hash of each. `cargo xtask package --out DIR` makes one from a
source checkout for the platform of the machine's Docker. `update.sh` requires and
checks the server, frontend and CLI payloads, and checks any other listed file
present, before installing anything. With `--from DIR` it does not fetch release
files; Docker may still need network access for images or the database build.
`b3sum` is used if the host has
it; otherwise the release's own `orchctl checksum` (the same hash, in the format
`b3sum` prints). Without `b3sum`, the updater checks the CLI's size before running
it as the checksum tool, so the CLI itself is part of the release the operator
trusts. Install `b3sum` independently to check the CLI without executing it.
Without `--from`, `update.sh` fetches
`<ORCH_RELEASE_URL>/download/<tag>/version.json` or
`<ORCH_RELEASE_URL>/latest/download/version.json` and then the three payloads.
Set `ORCH_RELEASE_URL` before the first start if fetching: after installation,
the updater can also read the release source from the installed configuration.
The API reads the installation's port assertion key from `config/secrets/`;
keep it with the other secrets so a restart preserves the signing identity. If
an older complete set lacks that key, the updater adds it without replacing the
other secrets before it starts the services.

Three payloads are installed: `server`, `frontend` and `cli`. Each lives in
`runtime/<payload>/releases/<tag>-<hash>/`, and `current` and `previous` beside
it are links to a release. A link is replaced by one rename, so it is never half
made; unpacking happens in a staging directory that is renamed into place when
it is whole. `compose.yml`, `nginx.conf`, `.env`, `config/orch.toml` and existing
secrets are kept by an update; a missing port assertion key is added to an older
complete set. When a release needs a configuration change its notes say so. The
key HTTP and WebSocket routes require the `/openapi/` and `/openapi/v1/ws` blocks
from the shipped `nginx.conf`, forwarded without stripping the prefix. Review and
merge these into an older installation's retained edge configuration before using
keys, including any outer HTTPS proxy or CDN; the updater preserves that file.
Installer downloads remain under `/api/v1/downloads/`. The
server archive is unpacked with `unzip` where the host has it, and
otherwise by a container of the web edge's image (`ORCH_UPDATE_NO_UNZIP=1` takes that
path on any host). The images that the manifest names (the default environment image)
are pulled when they are not on the host; a release made on the host has them.

An update, a rollback and a first start are transactions. Before each step
`update.sh` writes it to `runtime/update/journal`:

```
begin ─ staged ─ stopping ─ dumping ─ activating ─ (restoring_entered) ─ starting ─ verifying ─ done
```

`stopping` stops the application services (the database stays); `dumping` takes
a custom-format dump of the database, reads it back and keeps it under
`runtime/update-backups/`; `activating` moves the links; `starting` recreates
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
| `begin` … `activating` | Nothing ran the new release and the database is as it was: the links go back and the old release starts |
| `restoring_entered` (a rollback), `restoring`, `restarting` | The database was being replaced: it is made again from the dump of what was left, and the old release starts |
| `starting`, `verifying` | The new release may be fine: it is started, and kept if it becomes ready and reports its tag, undone like a failed update if not |
| `done` | The record is closed |

A first start that was cut off is taken up again by repeating the original
`first-start` command, including `--from` and any `--restore-database` option.

`rollback` swaps `current` and `previous` with the same safeguards, and puts the
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

The system serves its release's installers, connector and node at `/api/v1/downloads/`, and
`connectors/version.json` lists each with its size, BLAKE3 and SHA-256: an installer checks what
it fetched by its size and SHA-256 (a shell and PowerShell have a SHA-256 tool everywhere, a
BLAKE3 one almost nowhere; `b3sum` is used as well where it is installed). The commands Orch shows
when a machine or a host is added carry every value quoted for the shell they are pasted into.

| Installer | Runs as; program and data | Keeps running |
|-----------|--------------------------|---------------|
| `install.sh` (Linux, macOS) | **Your account**, without sudo; `~/.local/bin/orch-connector`. Linux data: `${XDG_DATA_HOME:-$HOME/.local/share}/orch-connector`; macOS: `~/Library/Application Support/Orch/connector` | User service: `orch-connector.service` on Linux, `dev.orch.connector` on macOS. Linux stops at logout unless lingering is enabled |
| `install.ps1` (Windows) | **Your account**, without administrator rights; `%LOCALAPPDATA%\Orch\orch-connector.exe`, data in `%LOCALAPPDATA%\Orch\connector` | Logon task `Orch Connector`; runs while you are logged in |
| `install-node.sh` (Linux hosts) | **Root**; `/usr/local/bin/orch-node`, `/etc/orch/orch.toml`, data in `/var/lib/orch-node` | System service `orch-node.service`; one node per machine |

Read `sh install/install.sh --help`, `sh install/install-node.sh --help`, or
`./install/install.ps1 -Help` in PowerShell, without a token or any installation. Help follows
the shell's locale or PowerShell's UI language (English or Chinese). The enrollment dialog uses
the same **User service**, **System service** and **Foreground** choices.

Pass `--gateway automatic` or `--gateway <id>` to either POSIX installer,
or `-Gateway automatic` / `-Gateway <id>` to PowerShell. Direct connector/node
enrollment accepts the same `--gateway` flag. Without a flag, a terminal offers
Automatic and the operator's labelled regions; unattended fresh enrollment uses
Automatic, and re-enrollment keeps an existing choice. Automatic uses the median
of three verified round trips on the enrolling machine and measures again on
primary reconnect. A hand-picked primary is always honoured, including when it
is unreachable; another gateway is never substituted for it. The local console's
gateway selector is an explicit operator change. A tunnel's gateway may differ
from the connector's primary without changing that choice.

System addresses use HTTPS, with one development exception: HTTP is accepted only for literal
loopback hosts, `127.0.0.0/8` in four decimal octets, `[::1]` (including its expanded form), or
`localhost`, with an optional port. Run a loopback command on the computer hosting that address.
LAN/public HTTP, DNS lookalikes, userinfo, query strings and fragments are refused before a download.
For a node this rule applies to `--from`; its pinned gateway `--url` remains wss/https. Curl follows
HTTPS redirects only; wget and PowerShell need the canonical address without a redirect. The shell
installers need curl or a wget that supports `--max-redirect` (BusyBox wget does not).
For a node's usual direct gateway on port 9443, pass the people's system address as `--from`: the
gateway port does not serve the installer downloads. The command in Orch supplies both addresses.

**Connectors.** Stop or start with `orch-connector service stop` / `service start`; on Windows,
invoke `& "$env:LOCALAPPDATA\Orch\orch-connector.exe"` before those subcommands. For **Foreground**,
install with `--no-service` (`-NoService` on Windows), then run the command printed by the installer,
ending in `run`; Ctrl+C stops it. Stop an existing service first. To add another connector, use
`--data-dir <absolute path>` (`-DataDir` for the Windows installer); it has its own identity and
service. Use the same `--data-dir` before subsequent program subcommands, including service
commands. Omit it for an installation made without that option. Several connectors share the
program, so changing it updates the file used by every instance; each running instance must restart.
The connector works in registered folders, independently of the install command's working directory.

**Nodes.** Stop or start with `systemctl stop orch-node.service` / `systemctl start orch-node.service`.
For **Foreground**, install with `--no-service`, then run as root:
`/usr/local/bin/orch-node --config /etc/orch/orch.toml run`; Ctrl+C stops it. This mode requires no
systemd and does not disable an existing service. Share the host through **Access** to serve
other organizations; another node or data directory on the same machine is not supported.

**Remove cleanly.** Revoke a connector's machine in Orch, then run `orch-connector uninstall` with
the same instance selection. For a node, remove its environments and their volumes, then its host,
in Orch; run `orch-node uninstall` as root on that host. Each command prints a plan and asks for the
machine or host name. Without `--purge` it keeps data and identity; `--purge` also removes the data
and objects named in the plan. `--plan` previews and `--yes` confirms without typing the name.
The connector keeps existing work folders and removes its shared program only after the last instance.
Stopping or removing a service alone does not revoke enrollment or delete data.

Each installer may be run again: a matching program is kept, and a supplied token is checked even
for an existing identity. Use a bound **Enroll again** token for that identity. The service is
written and restarted unless service installation is disabled. After a failed run, a newly
installed program or service is removed where possible; a replacement of an existing one is not
restored. Named connector services have a suffix derived from the data directory, so their service
commands must use the exact same path.

## Checked here

`scripts/smoke/` holds the scenarios (`first-start`, `update`, `rollback`,
`recover`, `restore`, and `managed-environment`, which uses the stack until an
environment made from the Default template has run a turn on the bundled host);
each makes a stack of its own from packaged releases, runs what is described
above and says which promise was kept. They are run by hand and in the
`deploy-smoke` CI job; each needs Docker, and builds the
releases it is not given. `ORCH_UPDATE_CRASH_AT=<step>` stops `update.sh` dead
right after it wrote that step, as the recover scenario does; it is never set in
a deployment.
