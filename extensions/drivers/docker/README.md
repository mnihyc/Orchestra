# orch-driver-docker

Whole-volume carry uses the configured trusted static helper and streams its archive through the node's blob transfers. It preserves owners, modes, binary extended attributes and private-layer deletion markers. Import is refused once any container uses the target volume, so a new private overlay first mounts after its upper layer has been restored.

Docker Engine implementation of the runtime driver contract.

Implemented: the Docker driver for both pools of runtimes. A unit is a container labelled with its system, runtime and pool, made
idempotently by name, with its file system from a registry reference, a Dockerfile built on the host, or a loaded image archive; its
workspace, home and state in named volumes on the host's storages; mounts as bind mounts and volumes, read-only, shared or as private
views (overlays); the connector from the tool cache started with the launch inputs; a network of its own for every unit (and a way
out of its own for a restricted unit's proxy), cut from one pool of the operator's (`[node.docker.networks]`, 10.199.0.0/16 in /28s as
shipped) so that the daemon's own address pools stay untouched, and units made before on a common bridge moved off it at their next
start in the same container; enforced limits; capability patches; and the descriptor of what this daemon can do. It is tested offline for every rule that needs no daemon and against a real
Docker daemon for lifecycle, volumes, mounts, networks, images, builds and imports (tests skip with a message when none is reachable).

Units start with a zero soft core-file limit and an unlimited hard limit. A daemon test proves that a crashed process leaves no core
file and that a shell can raise the limit for itself.

Live mounts and device nodes require Linux 5.12 or later, a preloaded trusted `[node.docker] live_helper_image`, and a static `orch-node` binary (`live_helper_binary`, defaulting to the running executable). Docker Desktop and rootless daemons leave the change pending with a recreate reason. The short-lived helper shares only the owned unit's PID namespace, binds the chosen source itself, uses SYS_ADMIN/SYS_CHROOT with AppArmor unconfined and default seccomp, and runs no code from the unit's image. Device give retains MKNOD and an exact helper mknod rule. Read-only applies recursively; private writes stay in the environment's own layer. Mounts are replayed after every start until recreation incorporates them. [Live mount tests](tests/live_mounts.rs) use `ORCH_TEST_STATIC_NODE` to exercise the released static binary on real Docker.

Restricted and gateway-only units require an operator-controlled, compatible sidecar image already present on the daemon:
`[node.docker] restricted_proxy_image` (the driver's `egress.proxy_image`), preferably an immutable image ID or digest. The runtime's
requested image is never executed as its trusted proxy or host address observer. Missing configuration, rootless Docker and failed
isolation or host-address observation refuse restriction. The observer reads local interfaces and Docker bridge ranges in the daemon's
host network; it has no listener, host filesystem mount or capabilities. Its inventory and the authenticated node heartbeat use the
same twenty-second watchdog. An unavailable inventory closes all outward flows, including Orch's transport; loopback stays local.

What the node gives it: the directory with the `orch-connector` binary and the tool cache (`tool_cache`), the host the units reach the
gateway at (`gateway_host`), and, in each runtime request, the gateway address, the authority certificate and the single-use enrollment
token. Limits of this version: images are built by the daemon's classic builder, so Dockerfile features that need BuildKit
(`RUN --mount`, here-documents, a `# syntax` line) are not available; a disk limit is enforced only where the daemon's storage driver has
quotas and is otherwise measured and reported; the files of a volume on a declared storage are removed by the node itself, which must be
allowed to remove what the unit's user wrote there.

The two template schemas carry field titles, descriptions, defaults, bounds and
examples. A null default annotation means leave the member absent and use the
image, host or resolved setting. Each schema also carries `x-orch-examples`, an
array of `{id, name, description, spec}` entries containing complete starting
configurations. An earlier `title` is accepted when `name` is absent. The template editor reads the same annotation from plugin
drivers. Descriptor tests check every example against its schema and Docker's
configuration reader, without starting a container.

| Module | Planned responsibility |
|--------|------------------------|
| [descriptor](src/descriptor.rs) | Docker template and provider schemas and confinement facts. |
| [client](src/client.rs) | Docker socket API access. |
| [units](src/units.rs) | Container lifecycle and the system's labels. |
| [volumes](src/volumes.rs) | Named persistent volumes on the host's storages. |
| [mounts](src/mounts.rs) | Path locations as bind mounts and volume locations as named volumes, at their targets, for an environment. |
| [live_mounts](src/live_mounts.rs) | Live mounts and device nodes through the node's one-shot static helper in an owned unit's namespace. |
| [live_capabilities](src/live_capabilities.rs) | Prepared device permissions, live capability grants and replay before the managed connector enrolls. |
| [live_resources](src/live_resources.rs) | Resource limits changed on the owned container without replacing it or stopping its processes. |
| [overlay](src/overlay.rs) | Private views: a mount's location laid under a layer that belongs to one environment. |
| [images](src/images.rs) | Images by reference with digest pinning, and images loaded from an archive. |
| [kept_images](src/kept_images.rs) | Owned same-image commits, flattening, archive transfer and removal by exact environment/generation names. |
| [build](src/build.rs) | Images built from a Dockerfile: the content hash, the context, the host's limits, progress and log. |
| [imports](src/imports.rs) | Proposing a spec from a Dockerfile, an image reference or an archive, with a note for what cannot be carried over. |
| [networks](src/networks.rs) | Gateway-only managed networks. |
| [egress](src/egress/mod.rs) | The host's proxy for restricted units: what it admits, the files the node and the proxy share, the HTTP proxy, the filtered resolver and the program that runs them. |
| [egress/admission](src/egress/admission.rs) | Canonical host names, what a restricted unit's proxy admits, and the addresses no other name may reach. |
| [egress/files](src/egress/files.rs) | The files a node and a unit's proxy share: the orders the node writes, the state the proxy answers, and the node's heartbeat. |
| [egress/proxy](src/egress/proxy.rs) | The HTTP proxy of a restricted unit: `CONNECT` tunnels and plain requests, each decided before any lookup or socket, and the flows a changed policy closes. |
| [egress/resolver](src/egress/resolver.rs) | The filtered resolver of a restricted unit: it answers the addresses of the names the unit may reach, looked up by a query of its own, and refuses every other question. |
| [restricted](src/restricted.rs) | A restricted unit's own network and its proxy: the network made isolated for the unit alone, the proxy's container beside it, the orders the node gives the proxy, and its heartbeat. |
| [egress/program](src/egress/program.rs) | The proxy program of one restricted unit: its listeners on the unit's private network, its orders, its heartbeat and its state. |
| [capabilities](src/capabilities.rs) | Docker capability patches. |
| [state_moves](src/state_moves.rs) | A stopped unit's files below one directory of its state, taken out as an archive and put into another unit. |
| [volume_moves](src/volume_moves.rs) | Whole owned home and private-layer volumes, measured and streamed through the trusted static helper before first use. |
| [existing_directories](tests/existing_directories.rs) | Real Docker binds retain borrowed files, owner and mode through restart, recreate and owned-volume purge. |
| [stats](src/stats.rs) | Container resource facts. |
| [driver](src/driver.rs) | The Docker driver itself: its construction and the operations of the driver contract. |
| [config](src/config.rs) | The driver's own settings: where the daemon is, what the host offers, where its storages and tools are. |
| [spec](src/spec.rs) | The template spec as the driver reads it: the members of the Docker spec, their defaults, and what makes one wrong. |
| [archive](src/archive.rs) | Tar archives in and out of the driver, and the blobs they come from. |
| [progress](src/progress.rs) | What a long operation tells while it runs, and the end of its log kept for a failure. |
| [resources](src/resources.rs) | Exact ownership-checked inventory and removal without daemon-wide pruning. |
