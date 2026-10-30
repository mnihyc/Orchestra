# orch-driver-contract

EnvironmentDriver contract for provider and environment runtimes.

The contract of environment drivers: the `EnvironmentDriver` trait that `orch-node` calls to make, start, stop, inspect, list and remove units and their volumes, the `DriverDescriptor` that each driver publishes (pools it makes units for, confinement, capabilities, mounts, private views, storages, suspend, sources, imports, limits, runtime classes), the driver-neutral `RuntimeSpec` with what every driver must deliver to the connector, and the rules that need no driver: the names and labels of units and volumes, which storage a volume lands on, which mounts can be attached together and what a mode comes to where a private view cannot be made, how capability patches merge, how a build is identified, and how a unit found is reconciled with what is recorded. Each rule and each descriptor and request has a test on a literal example; a small in-memory driver in the tests stands for an implementation that claims the least.

| Module | Planned responsibility |
|--------|------------------------|
| [driver](src/driver.rs) | Driver operations and errors. |
| [host](src/host.rs) | Scheduling scope, runtime delivery and aggregate observations supplied by a driver. |
| [descriptor](src/descriptor.rs) | A schema for each pool it makes units for, confinement, capabilities, mount, private-view, storage and suspend support, and runtime classes. |
| [resources](src/resources.rs) | Driver-neutral inventory and exact removal of installation-owned objects. |
| [runtime](src/runtime.rs) | Driver-neutral RuntimeSpec and required connector launch inputs. |
| [sources](src/sources.rs) | Where a unit's file system comes from: a reference, a build, an archive; and what a host offers of them. |
| [imports](src/imports.rs) | What a spec can be proposed from, the fields each import asks for, and the notes it returns. |
| [units](src/units.rs) | Unit identity, lifecycle facts and succession. |
| [volumes](src/volumes.rs) | Persistent storage operations on a host's named storages. |
| [mounts](src/mounts.rs) | Mounts a driver attaches: placed, in place or not supported; read-only, shared or a private view. |
| [capabilities](src/capabilities.rs) | Per-driver capability patches. |
| [isolation](src/isolation.rs) | Runtimes as declared classes: what holds a unit in, and what each class can and cannot do. |
| [kept](src/kept.rs) | Owned images that keep one environment's installed files across unavoidable same-image replacements. |
| [network](src/network.rs) | Egress modes, the network policy frozen into a runtime's request, and what a driver reports of a unit's network. |
| [process_limits](src/process_limits.rs) | Native process control facts and resource properties shared by scopes and directory groups. |
| [resources](src/resources.rs) | Driver-neutral inventory and exact removal of installation-owned objects. |
