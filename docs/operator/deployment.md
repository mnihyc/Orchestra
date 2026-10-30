# Deployment

How to install Orch on one Docker host, from the files of a release to a running system with its first
administrator. Everything here was run; the commands are the ones that were typed. What the pieces are and
how they hold together is in [deploy/README.md](../../deploy/README.md).

This release runs one API process. Browser and terminal socket tickets and their tunnels belong to the
process that issued the ticket; a deployment with multiple API processes must route each socket to that process.

For the node's lifecycle programs and per-unit Docker pool, read [Node hooks and unit networks](node-hooks.md).
Hooks are set with `orch-node policy set hooks`; the initial network pool accepts `[node.docker.networks]`.
Existing node configuration is imported once without interrupting an upgrade. The worked routing recipe keeps a
unit's outbound traffic closed while its host VPN is down.

## What the host needs

- Linux with Docker and its Compose plugin (version 2), `bash`, GNU coreutils and `flock`. A person who can use
  Docker runs everything below; nothing needs `sudo` unless Docker itself does. `curl` is needed to fetch a
  release, and `unzip` is used if it is there (otherwise a small container of the web edge's image unpacks the
  server archive).
- Three stock images, pulled on the first start: `postgres:17-alpine`, `debian:bookworm-slim` and `nginx:1.30-alpine`.
  The release's Compose file selects each by its image-index digest as well as its tag (the database through
  [its recipe](../../deploy/postgres/README.md), which adds the pgvector extension for search by meaning and is
  made by Compose on the first start, with Alpine's build tools and the extension's checked source), and the
  manifest records the same digests. The [release image script](../../scripts/release/README.md) prints current
  indexes for review and refreshes the pins only with `--write`.
  On a host without access to a registry, bring them with `docker load` first. A release also names the image
  its environments are made from (`orch-env-base`); `update.sh` pulls it if the host lacks it. That image is a
  working machine for an agent that also holds a browser, a VNC display and `sudo` without a password for the
  agent's account (the unit is the boundary, and a template that sets `no_new_privileges` takes `sudo` away): about
  650 MB to pull and 1.6 GiB on disk. [Its page](../../images/orch-env-base/README.md) lists what it holds, with
  the versions.
  A release also names `orch-env-dev`, the same image with Docker Engine, QEMU and build tools added (about 0.6 GiB more,
  2.2 GB on disk), which the Developer tools template is made from; `update.sh` pulls it too, and a pull that fails is
  said and holds back only the environments made from that template.
- An HTTPS front for the web edge, which is yours: Orch's own edge speaks plain HTTP on this host and expects
  something in front of it to speak HTTPS to people. The [edge guide](edge.md) says what the front must do and
  how to check it. For a trial, the web edge's own address on loopback is enough.
- Machines and managed runtimes reach the system through the gateway's pinned port, so that one port is the
  only other thing to open in a firewall (`ORCH_GATEWAY_PUBLISH`, 9443 unless you choose another).

Nothing connects out of an Orch system unless somebody configured the thing it connects to: the worker to a mail
server, to the Telegram Bot API, to webhooks and to the push services of people's browsers; the API to an
identity provider or to a rate-catalogue address an administrator set; the node to image registries and to
whatever a Dockerfile fetches; and `update.sh`, when you run it without a directory, to the release site.

## Get the release

A release is a directory of files and a `version.json` that lists them with the size and BLAKE3 hash of each.
Either download the files you need into a directory (a host that cannot reach the release site gets them on a
disk), or let `update.sh` fetch them. Unpack the deploy bundle where the system is to live:

```sh
mkdir -p /opt/orch && tar -xzf orch-deploy-v0.1.0.tar.gz -C /opt/orch
cd /opt/orch/deploy
```

(`cargo xtask package --out DIR` makes such a directory from a checkout; see
[the packaging command](../../crates/tooling/xtask/README.md).)

## Say what you decide

```sh
install -m 600 .env.example .env
```

Set `POSTGRES_PASSWORD` in `.env` to something of at least 24 characters made of letters, digits, `-` and `_`;
`openssl rand -base64 24 | tr '+/' '-_'` makes one. `update.sh` refuses a short one, one with other characters
(it is written into a URL) and a placeholder. The other lines are commented out with their defaults:
`ORCH_FRONTEND_BIND` (where the web edge is published; this host's loopback unless you change it),
`ORCH_GATEWAY_PUBLISH`, `ORCH_RELEASE_URL` (where `update.sh` fetches from) and `COMPOSE_PROJECT_NAME`.

Keep `.env` readable by its owner alone. The updater sets its mode to 0600 and prints a line when it
corrects it. If it cannot do that, it stops before Compose runs and asks the file's owner to run
`chmod 600 -- .env`. Its values stay as you wrote them.
`orchctl doctor --only secrets --env-file .env --secrets-dir config/secrets` names any password or secret
file that others can read.

In `config/orch.toml`, set `[system] public_url` to the people's HTTPS address and replace the
example in `[[gateway.endpoints]]` with the direct machine door. With a CDN, use two DNS records:
the people's name proxied through the CDN and the machine name DNS-only to the Orch host. Without
a CDN one name may serve both, on their respective ports. `/link` always accepts connections, but
is advertised only if you add it to that list. See [Ways in and Certificates](edge.md).
The bundled node's explicit internal address and Docker `gateway_host` stay local. Other nodes'
environments follow their node by default; that node's `[gateway] managed_endpoint` may override it.

The node fetches the connector and Orch-served builds through its authenticated connection address
and verifies their checksums. `public_url` need not be reachable from its containers. Vendor or
internal mirror builds keep the operator-selected download address. `ORCH_READY_URL`, when set,
selects the address the ordinary updater checks from the host.

## Start it

```sh
./update.sh first-start --from /path/to/the/release     # files in a directory
./update.sh first-start v0.1.0                          # or fetched from ORCH_RELEASE_URL
```

It checks every file against `version.json` and refuses the whole release if one differs (nothing is installed
from a damaged download), writes the facts of this machine to `.env` (the user and group the services run as, the
Docker group, this directory), unpacks the three payloads (server, frontend, `orchctl`) under `runtime/`, makes
the secrets with `orchctl compose-secrets` (it refuses a `config/secrets/` that holds only part of a set),
starts PostgreSQL, takes the first dump, starts the stack, and waits until `orchctl doctor` passes in the API's
container: the API answers and reports the release's version, the gateway is connected to it, the worker beats,
and the node has a session once there is an administrator to enroll it. The host's CLI also checks the page,
API release and gateway upgrade through `public_url` (or the explicitly selected `ORCH_READY_URL`). A missing
or broken edge fails readiness; an update restores the prior payloads and database. Then it prints, for example:

```
ok    secrets          11 password and secret files are accessible by their owner alone
ok    config           read /etc/orch/orch.toml: Orch at https://orch.example.com
ok    database         connected to postgres:5432/orch (PostgreSQL 17.11); 1 migrations applied, the newest 1 (the release ships up to 1)
ok    api              GET https://127.0.0.1:8443/health -> 200; /api/v1/meta -> release 0.1.0 (build 159)
ok    gateway          heartbeat 5 s ago, release 0.1.0: the backplane to the API is connected
ok    worker           heartbeat 5 s ago, release 0.1.0: alive
wait  node             no host yet, and no one has signed up: the bundled node enrolls itself as soon as the first administrator exists
all 7 checks passed
ok    edge-page        GET / -> 200 text/html, with the content policy and the four other security headers
ok    edge-api         GET /health -> 200; GET /api/v1/meta -> 200, release 0.1.0 (build 159), first administrator still to make: yes
ok    edge-link        upgraded /link/v1/session: the gateway answers with the subprotocol orch-link.v1
all 3 checks passed

Orch v0.1.0 is running.

  Authority fingerprint (SHA-256): 10d62a0f…
    A machine that enrolls shows this; compare it before trusting the system.
  Open https://orch.example.com (the stack's own web edge listens on http://127.0.0.1:5180),
  and make the first administrator at /setup.
```

If it fails before the end, it says why and leaves the journal; run the same command again, which takes up
where it stopped (`docker compose logs <service>` shows a service's side). **Back `config/` up off the host
now**: it holds the secrets, and losing `orch_master_key` or `orch_ca_key.pem` cannot be undone.

## The first administrator

Open the address. A system without people shows `/setup`, which makes the first system administrator and signs
them in. Who may register afterwards is by invitation, as shipped; the system's mail server and the rest are
entered in Management. The bundled node waits for this step: its token is registered by the API once the first
administrator exists, and the node container restarts until then (its log says the token is unknown). It
enrolls on a start after that, which takes up to a minute or two, and `orchctl doctor` shows its line go from
`wait` to `ok`:

```sh
docker compose exec api orchctl doctor
```

## What is on disk

| Path | What it holds |
|------|---------------|
| `.env`, `config/orch.toml` | Yours; an update keeps their values and protects `.env` with mode 600 |
| `config/secrets/` | The secrets, mode 600 in a directory of mode 700 |
| `runtime/postgres/data` | The database |
| `runtime/data` | Blobs and uploads (`blobs/` is immutable and content-addressed) |
| `runtime/node` | The bundled node's identity and its tool cache: the connector units run, and the builds of the three harnesses that [the release lists](../../deploy/README.md#harness-builds) (some 1.1 GB once unpacked) |
| `runtime/{server,frontend,cli}/` | The unpacked releases, and the links `current` and `previous` |
| `runtime/update-backups/` | The dumps: one before every update and rollback, and the ones `backup` takes |
| `runtime/update/` | The journal of a transaction in progress, and the record of what is installed |

All of it is owned by the user the services run as, which is the one who ran `first-start`, so none of it needs
privilege to copy, move or remove.

The release ships Codex 0.165.0 and Claude Code 2.1.312. Bypass lets the agent act without permission prompts inside
its environment; whoever chooses it answers for those actions. See [How approvals work](../user/how-approvals-work.md).
Codex keeps a thread's history in its own store, a page at a time. Orch never copies or rewrites it. In the home
where a thread was made, and in a home that moves with its environment (Mount as-is or Copy), the thread resumes
natively, and a moved environment opens a thread when it is next used. Native forks also require the turn check
described in [Carry a thread](../user/carry-a-thread.md); a history beyond its configured check size uses a bounded
carry instead. Into a different home
(another environment, another credential's home, a machine without the volume) Codex cannot give its history back
whole: Carry brings the newest turns that fit the place's carry share (80 % of the new model's window by default) and
the full conversation as a file. The model's input stays within its window. The full transcript is written once,
a page at a time, so disk work follows its length while held memory stays bounded.
See [Codex history and carries](codex-history.md) ([简体中文](zh-CN/codex-history.md)) for the home,
fork-check and transcript storage requirements.

## Is it healthy

`orchctl doctor` (run inside the API's container, where the configuration, the database address and the
secrets are) tries, in turn, the local credential files' permissions, the configuration, the database,
the API on its own listener, the gateway's
heartbeat, the worker's and the node, and prints a line for each saying what was tried and what came back; it
exits 1 when a line says `FAIL`. `--only` and `--skip` choose lines (`--skip node` for a stack without the
bundled node). Stop a service and the line of that service fails, naming it:

```
FAIL  worker           last heard 58 s ago (release 0.1.0), and a live one reports every few seconds: it reports through the database
```

The same program checks the edge in front of the stack, from wherever you are: see [the edge guide](edge.md).

Native Claude Code transfers use temporary storage for archive payloads and file lists. Provision that storage for the largest transfer; archive copying uses bounded memory and runs outside the async request executor.
