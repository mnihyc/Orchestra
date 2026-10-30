# Deployment scenarios

Each scenario makes a stack of its own from packaged releases, in a directory of its own below
`runs/<run>`, as a Compose project named `orchsmoke<run><scenario>`, runs what the updater and the stack
promise, and prints `PASS` or `FAIL` for each promise. It takes the stack down and removes its directory at
the end, whatever happened, and exits non-zero when a promise was not kept. Runs share the releases they
package: one packaging at a time on a machine, put in place whole, so that two runs at once neither
replace nor remove each other's releases, images, directories or projects (each scenario's ports are its
own; two runs of the same scenario at once need `ORCH_SMOKE_PORT`).

| Scenario | What it shows |
|----------|---------------|
| [first-start](first-start/README.md) | A clean directory and a release become a running system, and nothing is installed from files that do not match the manifest |
| [update](update/README.md) | A newer release is installed in a transaction; one that does not become ready is taken back with the database |
| [rollback](rollback/README.md) | The previous release and the database as it was when it was left come back together, and back again |
| [recover](recover/README.md) | A transaction cut off by a power cut is finished or undone, never left ambiguous |
| [restore](restore/README.md) | A backup puts the same system back on a clean directory |
| [installers](installers/README.md) | The Linux command of "Add machine", through an HTTPS front: the connector fetched from the system and checked against its manifest, placed and enrolled; run again, nothing changes; a changed connector is refused and nothing is left |
| [release-distribution](release-distribution/README.md) | A served manifest is fetched and its absent image pulled; an unavailable image is named honestly |
| [installation-records](installation-records/README.md) | A read-only release check and actual manifest-led backup facts while the API runs and is stopped; metadata-only repair |
| [link-load](link-load/README.md) | Released programs carry concurrent 100-MiB transfers in both directions through netem, a packet cut, abandonment, fresh admission and the floor; opt in with `ORCH_SMOKE_RELEASE_PROOFS=1` |
| [load](load/README.md) | 100 overlapping fake-adapter turns and thirty actual browser processes, with retained latency, resource and once-only-work evidence; same opt in |
| [soak](soak/README.md) | Requested-duration restart/fault soak with independent final database reconciliation; a shortened run always leaves the 24-hour proof owed; same opt in |
| [managed-environment](managed-environment/README.md) | A release is started, used over HTTP until an agent has worked in an environment made from the Default template on the bundled host, and the environment is stopped, started and removed |
| [managed-harness](managed-harness/README.md) | The builds a release lists of Codex and Claude Code are downloaded and checked by the host's node, seen by the connector in a unit made from the Default template, and each harness runs a turn there that runs a command, changes a file and is decided over HTTP, with the key on another machine |
| [managed-provider](managed-provider/README.md) | A subscription of each harness gets a runtime of its own on the bundled host, is signed in by importing a credential that is not real, is stopped and started without losing the sign-in, signed out and deleted with its unit and its volume |
| [shipped-image](shipped-image/README.md) | The image every default environment is made from is run as a unit is run, with no server: root through `sudo` without a password and not under `no_new_privileges`; the browser and its command line, with its sandbox where the container allows one and without where it cannot, and fonts for Chinese, Japanese, Korean and emoji; a VNC display started as a display is started, with a viewer, a window manager and a browser with a window |

```sh
scripts/smoke/run-all.sh                     # every scenario, one after the other
scripts/smoke/first-start/run.sh
ORCH_SMOKE_RELEASE_A=/path/to/release-a ORCH_SMOKE_RELEASE_B=/path/to/release-b scripts/smoke/update/run.sh
```

They need Docker with Compose, curl and zip (and `jq` and `node` for the managed ones), and the Rust toolchain the repository pins when they have to
package a release (`cargo xtask package`; the first one compiles for a long while). `shipped-image` needs only Docker and the network: it builds the image
from its recipe, or takes one that is named (`ORCH_SMOKE_IMAGE`), and makes no stack. The variables are described at the head of
[lib.sh](lib.sh). No scenario touches a container, network or volume that it did not make: the stack is
named, and taken down with `docker compose down -v` for that project alone.

The scenarios use ports 41200 to 41217 (and 41250 to 41252, 41300 to 41302 and 41350 to 41351 for the three managed ones; `shipped-image` publishes none) of this host (the web edge on the loopback address, the gateway's
pinned listener on the Docker bridge's address), which `ORCH_SMOKE_PORT` moves.
