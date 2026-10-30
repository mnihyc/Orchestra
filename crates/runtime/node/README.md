# orch-node-core

Host driver composition and managed runtime lifecycle.

Implemented. The node is a connector that plays the role `node`: this crate is its role. It carries out the ten commands of the
role through the drivers its configuration names (make an image available, make a volume on a named storage, put a verified tool in the cache, make,
start, stop and remove a unit, propose a template from what a person has, answer a probe), tells the system what the host has,
what it uses and what became of every unit, and compares its units with the driver's on an interval without removing anything. It is tested on the
in-memory hub with the scripted controller and the in-memory driver for every command and for reconciliation, and, in `apps/node`, against a real Docker
daemon with the real driver and a real static connector, enrolling through a real gateway and API on loopback.

The operator's lifecycle hooks (`[hooks]` in the machine policy: `pre_up`, `post_up`, `pre_down`, `post_down`) run around every start, stop
and removal with a JSON document on standard input (what the unit serves, its networks and addresses, its host openings);
the up hooks fail closed and the down hooks never hold a stop, each run is reported as `node.hook`, a starting node runs
`post_up` again for the units it finds running, and a unit that came up and ended by itself is given its `post_down`. The
hooks are tested on the in-memory hub and, with a real Docker daemon, for the subnet and address a `post_up` is told.

Previous-release node configuration remains accepted. The first policy import keeps its hooks, public ports,
storages and driver declarations locally, prints the replacement `orch-node policy` commands, and then retires
the old source. Import version two also repairs hooks omitted by an earlier import without overwriting newer
local values. An already enrolled node without policy provenance retains its previous Orch-controlled defaults.

Unit removal keeps volumes by default. A later purge must present frozen owned references, pass non-creating ownership checks and exclude every live successor. A real Docker test restores identical workspace, home and state bytes stopped after a node restart, confirms purge twice, and refuses foreign or missing volumes. Private-view layouts and legacy layers without ownership metadata remain unavailable for verified restoration or purge.

Approved live mount sets are retained before driver work. A node restart keeps an unchanged namespace; a new unit start replays its desired set against the ordinary mounts. A driver refusal answers pending with its recreate reason. The node-command regressions in [live_mounts](tests/live_mounts.rs) cover restart replay, pending starts and unsupported or failed helpers.

Built-in and supervised external implementations use the same driver contract. Connection options come from the machine policy's open configuration map and are validated before activation; new work refuses a stale connection. A driver may report a multi-machine pool's capacity and storage classes, with missing observations kept unknown. The cluster model exercises ordinary provisioning, moves, retained restore and exact removal without shared node paths or optional native features.

Not carried yet: the progress of a tool download
(`node.tool.ensure` carries no job to tell it to), and a real-daemon test of the blob path (a build context or an image archive is fetched through the
connector's content path and is tested on the hub only).

| Module | Planned responsibility |
|--------|------------------------|
| [enrollment](src/enrollment.rs) | Host identity and outbound machine link. |
| [machine_policy](src/machine_policy.rs) | One canonical machine policy, legacy local import, storage paths and public listener ceilings. |
| [drivers](src/drivers.rs) | Configured built-in and external driver host. |
| [runtimes](src/runtimes.rs) | Driver-neutral runtime commands for both pools, serialized lifecycle, successors and retired units. |
| [volumes](src/volumes.rs) | Persistent volume lifecycle on a named storage, for runtimes, mounts and private views. |
| [storages](src/storages.rs) | Named storages of the host: configuration, capacity, free space and private-view support. |
| [tools](src/tools.rs) | Verified tool downloads and read-only cache publication. |
| [skill_fetch](src/skill_fetch.rs) | A skill's files fetched onto the host for the server, which fetches no address itself (`node.skill.fetch`). |
| [images](src/images.rs) | Making a spec's source available on the host (pull, build, load) and the periodic look for a newer image behind a tag. |
| [managed](src/managed.rs) | Connector launch inputs and single-use enrollment. |
| [facts](src/facts.rs) | Capacity, load, what the units there declare they need, storages, driver descriptors, units and tool facts. |
| [hooks](src/hooks.rs) | The operator's lifecycle hooks: executables the node runs at four moments of every unit's life, told what the unit is. |
| [host_ports](src/host_ports.rs) | The host's ports as the node sees them: the operator's `[ports]` ceiling, a declaration checked against it (`host.ports`), and the proposal a host page is filled with. |
| [host_publish](src/host_publish.rs) | A host opening on its node (`port.publish`, `port.close`): the reviewed number bound for exactly the transports asked, each TCP caller and UDP sender carried to the environment on a channel of its own, with a PROXY v2 header where the program asks. |
| [host_publish::record](../connector/src/host_publish/record.rs) | Shared physical publisher's numeric lifetime and traffic capture, without retaining traffic content. |
| [reconciliation](src/reconciliation.rs) | Reconcile this system's units without removing volumes. |
| [role](src/role.rs) | The node's role: every `node.*` command carried out through the drivers, and the events the node publishes. |
| [ledger](src/ledger.rs) | What the node records of the runtimes it made, kept on disk. |
| [settings](src/settings.rs) | What the node is configured with: its data directory, the drivers it loads, its storages and its intervals. |
| [work](src/work.rs) | Bounded latest-value filesystem progress on the node feed, with the command's cooperative cancellation. |

| [retained](src/runtimes/retained.rs) | Frozen owned-volume verification and purge independent of the removed unit. |
| [runtimes/mounts](src/runtimes/mounts.rs) | Approved live mount sets retained across node restarts and reapplied when a unit gets a new namespace. |
| [runtimes/mount_copies](src/runtimes/mount_copies.rs) | A mount's data copied between hosts: measured and archived where it is, unpacked into a volume made for that mount. |
| [runtimes/volume_moves](src/runtimes/volume_moves.rs) | Whole owned-volume measurements and copies, with source pause recovery and import before any target unit exists. |
| [runtimes/live](src/runtimes/live.rs) | Live limits and capability changes stay within the prepared unit and persist their replay state. |
| [runtimes/kept_images](src/runtimes/kept_images.rs) | Kept-image commands, streamed movement and epoch-checked sweeps that protect live units and archived ownership. |
| [resources](src/resources.rs) | Installation-owned inventory joined across drivers and the verified tool cache. |

| [trace_retained](tests/trace_retained.rs) | Real Docker and node-command proof that kept bytes return stopped and frozen purge refuses foreign or successor files. |
