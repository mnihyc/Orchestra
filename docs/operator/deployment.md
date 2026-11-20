# Install Orch

[简体中文](zh-CN/deployment.md)

Install Orch on one Linux Docker host, from the files of a release to a running system and its first administrator. At the end you have a stack that answers at an address, an administrator who can sign in, and a copy of the secrets away from the host.

What the pieces are and how they hold together is in the [readme of the deployment directory](../../deploy/README.md).

For the node's lifecycle programs and per-unit Docker pool, read [Node hooks and unit networks](node-hooks.md).
Hooks are set with `orch-node policy set hooks`; the initial network pool accepts `[node.docker.networks]`.
The node's machine policy is configured directly; no previous node configuration is imported.
The worked routing recipe keeps a unit's outbound traffic closed while its host VPN is down.

## Before you start

You need:

- **A Linux host**, x86_64 or ARM64, with Docker Engine and its Compose plugin (version 2), `bash`, GNU coreutils, `flock` and `curl`. `openssl` makes the database password. `unzip` and `b3sum` are optional: without them the updater unpacks the server archive in a container, and `orchctl` checks the checksums.
- **Rootful Docker, without user-namespace remapping.** The services' numeric user id must be the same on the host's bind-mounted directories. Rootless Docker needs a different arrangement and does not work with the supplied defaults.
- **Room on the host.** Recommended estimates, which are not limits (the stack sets none): to try Orch alone or with a colleague, 2 CPU cores, 4 GB of memory and 30 GB of disk; for a team of up to about 20 people, 4 cores, 8 GB and 100 GB of SSD. Every environment you run is a container on top of that. The Default environment image is about 1.6 GiB, and its browser needs memory when it runs. The estimates come from what the stack is made of, not from a measurement of a busy installation.
- **A registry.** The first start pulls three stock images (`postgres:17-alpine`, `debian:bookworm-slim` and `nginx:1.30-alpine`) and builds the database image from the first (it adds pgvector and pg_bigm). On a host without registry access, follow [Running without internet access](offline.md).
- **An HTTPS address** for the web application, unless you only try Orch on this host. Orch's web edge speaks plain HTTP on the host (`127.0.0.1:5180`) and expects a front that speaks HTTPS to people: [The HTTPS front](edge.md) says what the front must do. For a trial on this host, use `http://127.0.0.1:5180`.
- **One open port**, 9443 unless you choose another, on which machines and managed environments reach the gateway.

Run one API process. Terminal, browser and display tickets, and their tunnels, belong to the process that issued them, so a second API process would need its own routing.

## 1. Get the release

A release is a directory of files and a `version.json` that lists them with the size and BLAKE3 checksum of each. Choose a way.

**Files in a directory.** Download these five files from the [release page](https://github.com/mnihyc/Orchestra/releases/latest) into one directory. `ARCH` is `x86_64` or `aarch64`, whichever the Docker host's processor is:

```sh
TAG=v0.1.3        # the tag you install, with the "v"
ARCH=$(uname -m)  # x86_64 or aarch64
mkdir -p ~/orch-release && cd ~/orch-release
for f in version.json orch-deploy-$TAG.tar.gz orch-frontend-dist.tar.gz orch-server-linux-$ARCH.zip orchctl-linux-$ARCH-musl; do
  curl -fLO "https://github.com/mnihyc/Orchestra/releases/download/$TAG/$f"
done
```

A host that cannot reach GitHub can receive these files on a disk.

**Fetched.** Download only `orch-deploy-$TAG.tar.gz`. `update.sh` fetches the rest from this project's GitHub releases, as `.env` says.

## 2. Unpack

Unpack the deployment bundle where the system is to live, and work in its `deploy` directory:

```sh
mkdir -p ~/orch
tar -xzf orch-deploy-$TAG.tar.gz -C ~/orch     # run it where the bundle is
cd ~/orch/deploy
```

## 3. Configure

1. Make `.env` and give it a database password:

   ```sh
   install -m 600 .env.example .env
   sed -i "s/^POSTGRES_PASSWORD=.*/POSTGRES_PASSWORD=$(openssl rand -base64 24 | tr '+/' '-_')/" .env
   ```

   The password has at least 24 characters of letters, digits, `-` and `_`. `update.sh` refuses a short one, one with other characters (it goes into a URL) and a placeholder. Keep `.env` readable by its owner alone.
2. In `config/orch.toml`, set `public_url` in `[system]` to the HTTPS address people open (for a trial, `"http://127.0.0.1:5180"`).
3. In the same file, replace the example address in `[[gateway.endpoints]]` with an address that machines can reach on the gateway port, and that also answers from inside the stack. While the example is there, **Add machine** gives no command and environments cannot connect. The bundled node comes online without it. See [The HTTPS front](edge.md) for the addresses, and [Configuration](configuration.md#set-the-stacks-env) for the other settings of `.env`.

## 4. Start

```sh
./update.sh first-start --from ~/orch-release   # files in a directory
./update.sh first-start $TAG                    # fetched
```

`update.sh` checks every file against `version.json` before it installs anything, writes the facts of this host to `.env` (the user and group the services run as, the Docker group, this directory), unpacks the server, the web application and `orchctl` under `runtime/`, makes the secrets, starts PostgreSQL, takes the first backup, starts the stack, and waits until `orchctl doctor` passes. It ends with the address to open:

```
ok    secrets          11 password and secret files are accessible by their owner alone
ok    config           read /etc/orch/orch.toml: Orch at https://orch.example.com
ok    database         connected to postgres:5432/orch (PostgreSQL 17); 1 migrations applied
ok    api              GET https://127.0.0.1:8443/health -> 200; /api/v1/meta -> release 0.1.3
ok    gateway          heartbeat 5 s ago, release 0.1.3: the backplane to the API is connected
ok    worker           heartbeat 5 s ago, release 0.1.3: alive
wait  node             no host yet, and no one has signed up: the bundled node enrolls itself as soon as the first administrator exists
all 7 checks passed
ok    edge-page        GET / -> 200 text/html, with the content policy and the four other security headers
ok    edge-api         GET /health -> 200; GET /api/v1/meta -> 200, first administrator still to make: yes
ok    edge-link        upgraded /link/v1/session: the gateway answers with the subprotocol orch-link.v1
all 3 checks passed

Orch v0.1.3 is running.
  Open https://orch.example.com and make the first administrator at /setup.
```

Your output differs in addresses and numbers. If a step fails, `update.sh` says why and leaves its journal. Run the same command again: it takes up where it stopped. `docker compose logs <service>` shows a service's side.

## 5. Check that it works

1. Open the address. A system without people shows `/setup`: make the first system administrator there. You are signed in when it is done. Registration is by invitation as shipped; the other settings are entered in Management.
2. The bundled node waits for that step and enrolls itself after it, which takes up to a minute or two. Check it:

   ```sh
   docker compose exec api /opt/orchctl/orchctl doctor
   ```

   The `node` line goes from `wait` to `ok`. [Monitoring](monitoring.md) explains each line.
3. If people should get sign-in codes, address confirmations and invitations by mail, set up email. Open **Management → Email and channels → New channel…**, choose **Email**, and fill in **Server** and **Sender**. The usual ports are 587 with **STARTTLS** and 465 with **TLS**. Enter the **User name** and **Password** of the account that sends the mail (the password is sealed under the master key as it is stored), and choose **Test** to send a sample before you rely on it. To do the same from a shell, write the fields as one JSON object in `runtime/data/mail.json`, which the api container sees as `/var/lib/orch/mail.json`:

   ```json
   {"host": "smtp.example.org", "port": "587", "security": "starttls", "username": "orch", "password": "app-password", "from": "Orch <orch@example.org>"}
   ```

   Every value is text. Only `host` and `from` are required; the other keys are `auth`, `reply_to`, `tls_ca` and `timeout_secs`. Put the channel, send a sample, and remove the file, which holds the password:

   ```sh
   docker compose exec api /opt/orchctl/orchctl channel put email --file /var/lib/orch/mail.json
   docker compose exec api /opt/orchctl/orchctl channel test email --to you@example.org
   rm runtime/data/mail.json
   ```

## 6. Keep the secrets

**Copy `config/` away from the host now.** It holds the secrets, and losing `orch_master_key` or `orch_ca_key.pem` cannot be undone. See [Back up and restore](backup-and-restore.md).

## What the install made

All of it belongs to the user who ran `first-start`, so none of it needs privilege to copy, move or remove.

| Path | What it holds |
|---|---|
| `.env`, `config/orch.toml` | Yours. An update keeps your values and keeps `.env` at mode 600. |
| `config/secrets/` | The secrets, mode 600 in a directory of mode 700. |
| `runtime/postgres/data` | The database. |
| `runtime/data` | Blobs and uploads. |
| `runtime/node` | The bundled node's identity and tool cache, with the agent programs it unpacks (about 1.1 GB). |
| `runtime/{server,frontend,cli}/` | The unpacked releases, and the links `current` and `previous`. |
| `runtime/update-backups/` | The backups: one before every update and rollback, and the ones `backup` takes. |
| `runtime/update/` | The journal of a transaction in progress, and the record of what is installed. |

## What to do next

- Give Orch its HTTPS address and a gateway address: [The HTTPS front](edge.md).
- Add a provider and start a first thread: [Getting started](../user/getting-started.md).
- Connect a machine or a host: [Connect a machine](../user/connect-a-machine.md).
- Plan your backups: [Back up and restore](backup-and-restore.md).
- Update later: [Update and roll back](upgrades.md).
- Something does not work: [Incidents](incidents.md).

The release ships Codex 0.167.2 and Claude Code 2.1.322. Bypass lets the agent act without permission prompts inside
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

`./update.sh doctor` runs both checks once, as an update's wait for readiness does, and needs nothing else:

```sh
./update.sh doctor
edge address: https://orch.example.com (from [system] public_url in config/orch.toml)
inside the stack: orchctl doctor in the API's container
ok    api              ...
from this host: edge-page, edge-api and edge-link against https://orch.example.com
ok    edge-page        ...
doctor: every check passed
```

It prints every line of both as they come, names the address the edge check used and where it came from
(`ORCH_READY_URL` in the environment or in `.env`, else `[system] public_url` in `config/orch.toml`), runs the
second check even when the first fails, and exits 1 if either fails. It does not wait or retry, expects no
particular release, and changes nothing.

Native Claude Code transfers use temporary storage for archive payloads and file lists. Provision that storage for the largest transfer; archive copying uses bounded memory and runs outside the async request executor.

## Move data to another disk

`ORCH_DATA_DIR` holds the server's files, including blobs, frontend TLS and update status. `ORCH_POSTGRES_DATA_DIR` holds the PostgreSQL cluster. For a fresh installation, set either path in `.env` before first start. The defaults are `./runtime/data` and `./runtime/postgres/data`.

To move an existing installation, mount the new disk persistently and choose two empty directories on it. Finish any open update and save a [backup](backup-and-restore.md). From the deployment directory, stop all writers and copy the stores with their ownership, permissions and links intact. These commands assume the current default paths and a disk mounted at `/mnt/orch`.

```sh
docker compose down
mkdir -p /mnt/orch/server /mnt/orch/postgres
cp -a ./runtime/data/. /mnt/orch/server/
cp -a ./runtime/postgres/data/. /mnt/orch/postgres/
```

After both copies succeed, set `ORCH_DATA_DIR=/mnt/orch/server` and `ORCH_POSTGRES_DATA_DIR=/mnt/orch/postgres` in `.env`. Run `docker compose up -d`, then `./update.sh doctor`. Open an existing thread and one of its files before retiring the old copies. To revert before accepting writes, bring the stack down, restore the previous paths and start it again. Subsequent backups must copy blobs from the selected server directory.
