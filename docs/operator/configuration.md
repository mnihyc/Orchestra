# Configure Orch

Set the addresses, ports, images, logs and storage of an Orch installation. At the end the services start with your values, and you know which file holds which kind of setting.

The bundled node's permissions live in `runtime/node/config.toml`, initialized from
`config/node.toml` on first start. In that file, `[drivers.images] builds` permits
Dockerfile builds together with unrestricted image and registry rules. See
[driver policy](driver-policy.md#the-bundled-node) ([简体中文](zh-CN/driver-policy.md)).

## Before you start

You need an installed stack ([Install Orch](deployment.md)). You change files in its `deploy/` directory and restart the services they affect.

## Where each kind of setting lives

| What | Where | How you change it |
|---|---|---|
| The stack itself: ports, Docker socket, project name, release address | `.env` in the deployment directory | Edit a line, then run `docker compose up -d` so Compose recreates what it affects |
| The services' settings | `config/orch.toml` | Edit it and restart the services it affects. An environment variable `ORCH_<SECTION>_<KEY>` overrides a key, for example `ORCH_DATABASE_SLOW_STATEMENT_MS` |
| Secrets | A file each, under `config/secrets/` | Replace the file; keep it owner-only |
| What people change while they use Orch: who may register, limits, channels such as mail, Telegram and webhooks, sign-in providers, prices | The database, in **Management** and in each place's settings | Use the screens; none of it is in `orch.toml` |

A key left out of `orch.toml` takes the default that ships. An unknown key, or a value of the wrong type, stops the service that reads it at start. The database address is not in the file: the services are given it as `ORCH_DATABASE_URL`, so no file holds the password. Every default and its description is in the [shipped example file](../../deploy/config/orch.toml) and in the [server configuration source](../../crates/platform/config/src/server.rs).

## Set the addresses

1. In `[system]`, set `public_url` to the address people open, as the HTTPS front serves it. Orch uses it in links, in mail and for installer downloads. Change it before the first start.
2. List other browser origins in `alternate_urls`. IP addresses and ports must be explicit. Links from these addresses open on the address already in use; **Copy link** uses `public_url`.
3. Replace the example in `[[gateway.endpoints]]`. [Run the HTTPS front](edge.md#2-set-the-addresses) explains the list and which machines follow it. For several regional gateways, see [Run regional gateways](regional-gateways.md).

## Set the stack's `.env`

Only the database password and the release address are set when you start; every other line is commented out with its default.

| Setting | What it sets | Default |
|---|---|---|
| `POSTGRES_PASSWORD` | The database password: at least 24 characters of letters, digits, `-` and `_` | None. You set it |
| `ORCH_FRONTEND_BIND` | Where the web edge is published | `127.0.0.1:5180`, this host only |
| `ORCH_FRONTEND_HTTPS_BIND` | Where the web edge also speaks HTTPS, with a certificate of this installation's authority ([Run the HTTPS front](edge.md)) | Off |
| `ORCH_GATEWAY_PUBLISH` | Where the gateway's direct listener is published, as `[address:]port:9443`. Machines and managed environments connect to it | `9443:9443` |
| `ORCH_DATA_DIR` | Host directory for blobs, uploads, frontend TLS and update status | `./runtime/data` |
| `ORCH_POSTGRES_DATA_DIR` | Host directory for the PostgreSQL cluster | `./runtime/postgres/data` |
| `ORCH_RELEASE_URL` | Where `update.sh` fetches a release when you give it no directory, and where **Management** looks for newer releases. It overrides `[release] url` | This project's GitHub releases. Leave it empty on a host without outside network and give `update.sh` a directory with `--from` |
| `ORCH_READY_URL` | The address `update.sh` uses to check the web edge from this host, when the host reaches the system by another route than `public_url` | `public_url` |
| `DOCKER_SOCKET` | The host's Docker socket, if it is not `/var/run/docker.sock` | `/var/run/docker.sock` |
| `ORCH_DOCKER_GID` | The numeric group that owns the Docker socket, for the node | Written by the first start |
| `COMPOSE_PROJECT_NAME` | The name of the Compose project, and so of its containers and networks | `orch` |

- **The password.** `openssl rand -base64 24 | tr '+/' '-_'` makes a good one. `update.sh` refuses a shorter one, one with other characters (it is written into a URL) and a placeholder.
- **The file's mode.** Keep `.env` readable by its owner alone. `update.sh` sets mode 0600 and prints a line when it corrects it. If it cannot, it stops before Compose runs and asks the file's owner to run `chmod 600 -- .env`. After the first start, `./runtime/cli/current/orchctl doctor --only secrets --env-file .env --secrets-dir config/secrets` names any password or secret file that others can read.
- **A different Docker socket.** Set `DOCKER_SOCKET` so later Compose commands use the same socket, and make sure the Docker command line uses that daemon. The node sees it at `/var/run/docker.sock` inside its container. If the socket or its group changes later, set `ORCH_DOCKER_GID` to the number `stat -c %g /path/to/docker.sock` prints, then run `docker compose up -d node`.
- **A different gateway port.** If the host port of `ORCH_GATEWAY_PUBLISH` is not 9443, set the same port in `[gateway] managed_endpoint` of `config/orch.toml`. The bundled node's `[node] gateway_url` keeps the container's port 9443.
- **Reaching the node.** `public_url` need not be reachable from the stack's own containers: the node fetches the connector and the builds Orch serves through its own connection to the gateway and checks their checksums. Builds from a vendor or an internal mirror keep the address you chose.

## Set `orch.toml`

The sections an operator changes, with their shipped values:

| Section | Keys you may change | Notes |
|---|---|---|
| `[system]` | `name`, `public_url`, `alternate_urls`, `web_push_contact` | Push services receive `web_push_contact` (HTTPS or `mailto:`). When it is empty, Orch uses the public HTTPS origin or omits the contact on HTTP. |
| `[release]` | `url`, `request_timeout_secs` (10), `manifest_max_bytes` (1 MiB) | The updater and read-only checks share this address. Scheduled notices of new releases are off (the system setting `release.check_days` is 0); **Management** reads an hourly cache; no check installs anything |
| `[api]` | `db_pool` (32), `db_pool_warm` (8), `db_pool_lifetime_secs` (14400), `trusted_proxy_cidrs`, `metrics_bind`, `worker_stack_mib` | The API has no published port: only the web edge reaches it. [Run the HTTPS front](edge.md#5-name-the-proxies-that-may-describe-a-caller) explains the proxy list. See [Keep connections ready](#keep-connections-ready) |
| `[gateway]` | `managed_endpoint`, `[[gateway.endpoints]]`, `metrics_bind`, `worker_stack_mib` | `pinned_bind` is the published port; `edge_bind` serves machines that come through the HTTPS front |
| `[link]` | heartbeat (20 s), dead after (60 s), frame, tunnel and port limits | Change only for a measured reason |
| `[worker]` | `db_pool` (8), `metrics_bind`, `worker_stack_mib` | |
| `[database]` | `slow_statement_ms`, `slow_acquire_ms`, `prepared_statements` (512) | See [Watch the database](#watch-the-database) and [Keep statements prepared](#keep-statements-prepared) |
| `[storage]` | `root` (`/var/lib/orch`) | Where blobs, uploads and temporary files are kept; the shipped deployment mounts `runtime/data` there |
| `[auth]` | token and session lifetimes, `password_min_length` (10), sign-in failure limits, mailed-code limits, `known_device_days` (90) | |
| `[retention]` | `metrics_days` (0 keeps the hourly history for good), `metrics_fine_days` (30) | Each minute of the measurements is kept for `metrics_fine_days` and then only its hour |
| `[pki]` | `leaf_validity_days` (365), `expired_renew_grace_days` (90), `service_dns_names` | |
| `[node]`, `[node.docker]` | `gateway_url`, `image_check_hours` (0), `egress_proxy`, `enroll_timeout_secs` (120), `docker_inside_gb`, `resolvers`, `builds`, `[node.docker.runtimes.*]` | See [Recommended host setups](recommended-host-setups.md) for the runtimes. `image_check_hours = 0` keeps a disconnected host quiet |
| `[log]` | `format` | `text`, or `json` for a collector |

`ORCH_LOG` selects the logging filter (`info` by default). `[api] metrics_bind`, `[gateway] metrics_bind` and `[worker] metrics_bind` choose the private metrics listeners ([Monitor an installation](monitoring.md)).

### Set the worker stack

Each service reads `worker_stack_mib` before it builds its runtime: the stack reserved for each runtime thread, in MiB. The default is 64 for the API, gateway, worker and node. Any positive whole number works; zero, or a value whose byte count overflows the platform, is rejected when the configuration loads. `ORCH_API_WORKER_STACK_MIB=96` overrides the API's value. The node's `run` and `enroll` commands use its setting; its maintenance and conformance commands work without a deployment file.

### Watch the database

```toml
[database]
slow_statement_ms = 1000
slow_acquire_ms = 1000
```

The API and the worker warn when a statement reaches `slow_statement_ms`, or when a successful pooled connection acquisition takes longer than `slow_acquire_ms`. Both are in milliseconds, and zero watches every operation. The warning holds the statement text with its parameter placeholders, the elapsed time and the threshold, never bound values. Do not put secrets in SQL text or turn on logging of credentials.

The two [monitoring counters](monitoring.md) count these events even if `ORCH_LOG` suppresses their
printed warnings. Pool acquisition defaults to ten seconds; these warning thresholds do not
change the configured deadlines below, a pool's size, or statement execution limits. A failed/timed-out acquisition is an
error, not a successful slow acquisition in that counter. Investigate the statement/plan and connection
ownership before changing pool sizes. Never hold a connection while awaiting another machine.

### Keep statements prepared

```toml
[database]
prepared_statements = 512
```

Each connection of the API's and the worker's pools keeps this many statements prepared, and gives up the least
recently used first. A statement that is no longer kept is prepared again the next time it runs: one more round trip,
and PostgreSQL parses and plans it again, which for the permission reads costs more than running them. A turn's start
runs a few hundred distinct statements, and the five hundred most used make almost all that the services run. Each
kept statement holds some tens of kilobytes in its database session, for every connection of both pools
(`[api] db_pool`, `[worker] db_pool`). Zero keeps none. `ORCH_DATABASE_PREPARED_STATEMENTS=1024` overrides it; restart
the API and the worker after changing it.

## Keep connections ready

```toml
[api]
db_pool_warm = 8
db_pool_lifetime_secs = 14400
```

The API keeps `db_pool_warm` database connections open while nobody is asking, so that the first page opened after a
pause is answered by connections the database has already served: they keep their prepared statements and what those
statements use. A page's first reads run at once, on several connections. When the API opens a connection, it
prepares there, in the background, the statements its other connections ran most. A connection beyond the warm ones
that sits idle is closed, one a minute. Each connection serves for `db_pool_lifetime_secs` before it is replaced; zero
keeps it for good. With `db_pool_warm = 0` the API keeps none, and an idle connection closes after ten minutes. Restart
the API after changing either.

## Database deadlines

The API, worker and deployment commands read their deadlines before opening the database.
They are startup configuration, with compiled defaults and ordinary file/environment overrides.
Restart the affected process after changing them; they are not database-backed settings.

| Key in `[database]` | Default | Meaning |
|---|---|---|
| `acquire_timeout_ms` | 10000 | Pool acquisition, including opening and checking a connection; must be positive. |
| `request_statement_timeout_ms` | 15000 | API statement deadline; zero disables it. |
| `background_statement_timeout_ms` | 0 | Worker and maintenance statement deadline; zero disables it. |
| `idle_transaction_timeout_ms` | 10000 | Idle time inside a transaction; zero disables it. |

For example, `ORCH_DATABASE_ACQUIRE_TIMEOUT_MS=30000` selects a thirty-second acquisition deadline.
PostgreSQL's statement and idle-transaction deadlines use signed 32-bit milliseconds; this is a native
representation limit, not a product ceiling. See the [PostgreSQL timeout settings](https://www.postgresql.org/docs/17/runtime-config-client.html#RUNTIME-CONFIG-CLIENT-STATEMENT).
These deadlines do not repair slow queries or excessive lock ownership; those causes still need repair.

Management → System → Details shows the API process’s captured safe running values and their compiled, file or environment origins. Later edits do not relabel that running process. Unreported processes stay Not reported; the API does not infer another service’s environment.

## Build environment images

The **Default** and **Developer tools** templates carry editable Dockerfiles. A host that offers builds makes the image an environment needs the first time it is needed, then reuses it on that host. The job shows its progress and any failure.

- **Developer tools** builds on the Default recipe as its first stage, so the host needs neither a built Default image nor a checkout of Orch.
- **Time.** A first build takes minutes, longer for Developer tools, and varies with cache, processor and network. The Docker driver's `build_timeout_secs` is 1,800 seconds unless the host's operator changes it.
- **What a first build fetches.** The pinned Debian base from its registry, Debian packages, Node.js, npm packages and the browser download of Playwright. Developer tools also needs Docker's package repository. On a host that cannot reach them, use internal sources or an image archive ([Run Orch without internet access](offline.md)).
- **Contents.** The Default image includes a browser, a VNC display and passwordless `sudo` for its agent account. A template can turn that privilege off. [Its page](../../images/orch-env-base/README.md) lists the contents and the size. Developer tools adds Docker Engine, QEMU and build tools.
- **A full disk.** If a pull or build fails because the host's disk is full, free space on that host and try again. A pull that fails for lack of space does not fall back to an older cached tag.

## Keep the files that restore a turn

Setup already gives a local content store: `[storage] root` defaults to `/var/lib/orch`, mounted from `runtime/data`. It holds manifests and the plans transferred between the server and a connector. No outside blob service or extra setting is needed to restore a turn's files. The API opens this store before it starts thread work and refuses to start if it cannot.

The connector keeps the immutable file objects outside the project, below its own data directory (`file-states/`) or the environment's persistent home (`.orch/file-states/`). Preserve both the server data and these machine-side roots: copying only the database cannot restore file bytes. By default, included files are recorded for seven days, up to 1 GiB per state. The project's ignore rules and the environment's recording settings still apply. A new thread directory prepares its indexed coverage before the first recording answers; a live tree, or a capture that is genuinely incomplete, stays unavailable.

## Set the deletion and blob retention windows

In System settings, `retention.deleted_keep_days` sets how long deleted items can be restored. It defaults to 30 days. Zero makes an item eligible for the next purge.

`retention.blob_grace_hours` defaults to 24. Complete each dump's separate blob copy after that dump, and within this grace measured from the dump's start; keep the pair together. A larger grace leaves more time to make the copy and keeps more unpinned data. See [Back up and restore](backup-and-restore.md).

## If it does not work

- **A service stops at start with an unknown key.** A key in `orch.toml` is misspelled or belongs to another section. The message names it; check it against the [example file](../../deploy/config/orch.toml).
- **A change has no effect.** Restart the services it affects; for `.env`, run `docker compose up -d`.
- **`update.sh` refuses the password.** It must be at least 24 characters of letters, digits, `-` and `_`, and not a placeholder.
- **The node cannot find Docker.** Set `DOCKER_SOCKET` and `ORCH_DOCKER_GID` as above.
