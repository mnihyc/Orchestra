# orch-driver-host

Host-process implementation of the runtime driver contract.

This driver starts managed connectors on Linux under a template's local account,
retains workspace, home and state directories, and reconciles processes by their
kernel start identity after a node restart. A proved systemd scope enforces CPU,
memory and task limits; otherwise limits are reported. Lifecycle tests cover the
account, launch paths, restart, kept files and refusal to signal an unrelated PID.

`[node.host] base` selects the driver's records and default volumes. When using
another account, choose a base and configured storage directories whose parents that account
can traverse, separate from the node's private identity directory. The node must
already run as root to switch accounts; it uses `runuser` and `prlimit`, never an
elevation command. In-place mounts rely on the harness sandbox and file permissions
for read-only use and offer no private overlay. Removing a unit retains every
volume; an explicit volume removal checks its owned record first. Templates contain
the account and commands; `env.storage` and `runtime.storage` select named machine
storages. Workspace, home and mount base paths belong in the machine's storage
configuration, so the same template can run on machines with different disks.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Host template and provider schemas and confinement facts. |
| [units](src/units.rs) | Systemd scopes or process groups and lifecycle. |
| [identity](src/identity.rs) | Configured run_as identity including permitted root execution. |
| [volumes](src/volumes.rs) | Persistent workspace, home and state directories, under a named storage when one is asked for. |
| [mounts](src/mounts.rs) | Mounts in place: paths as they are, volumes as directories under the mounts base. |
| [limits](src/limits.rs) | Enforced or reported resource constraints. |
| [stats](src/stats.rs) | Host unit process and resource facts. |
