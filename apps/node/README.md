# orch-node

The same static runtime helper measures and streams whole owned home/private volumes, preserving Unix owners, modes, binary extended attributes, hard links, overlay whiteouts and opaque directory markers. It reads no backing mount data and restores into an unused volume before the target unit is created.

Managed runtime lifecycle through driver implementations.

Implemented. `orch-node enroll --url … --token …` joins a system with a host token, `orch-node run` (the default) loads the
drivers the configuration's `[node]` table names, enrolls the host first when it holds no identity and a token is given (`--token`, `ORCH_NODE_TOKEN` or
`--token-file`, as the bundled deployment mounts it), holds the session to the gateway and does what the system asks of the host, and `status` shows who
the host is. While the system does not accept the token yet (a new system accepts the bundled node's token once its first administrator is set up) or
cannot be reached, `run` says so once, tries again every ten seconds and warns every ten minutes, instead of ending for its service manager to start it
again; any other refusal ends it at once. `enroll` tries once. It reads `orch.toml` (`--config`, `ORCH_CONFIG`) and keeps its files in `[node] data_dir` (`--data-dir`, `ORCH_NODE_DATA_DIR`). The
`docker` and Linux `host` drivers are carried; configured external drivers are supervised through the plugin contract.
An unknown name or a driver that cannot start is reported unavailable with its reason. Standard error is its log.

The package is a library and the executable: `src/composition.rs` and `src/docker.rs` choose everything concrete (which drivers exist, how the Docker
driver is made from `[node.docker]`, how it reaches the progress of a pull, the digest behind a tag and the record of a unit's enrollment), and the tests
under `tests/` use the same two to run the node against a real Docker daemon, a real gateway and a real API: `cargo test -p orch-node -- --nocapture`
(they say so and return where there is no daemon or no static connector; `cargo build -p orch-connector --target x86_64-unknown-linux-musl` makes the
connector a unit runs inside any small image).


| Module | Responsibility |
|--------|----------------|
| [uninstall](src/uninstall.rs) | Read-only local inventory, one confirmation and dependency-ordered removal of owned runtime objects and installer files. |
| [conformance](src/conformance.rs) | Isolated native build runner, using the adapter tests' shared cases and the scripted model. |
| [conformance_unit](src/conformance_unit.rs) | Disposable networkless driver unit for one immutable cached build. |
| [conformance](tests/conformance.rs) | A scripted cached build runs only inside the node's disposable networkless conformance unit. |
| [runtime_helper](src/runtime_helper.rs) | One-shot mount namespace operations for an owned Docker unit, using the node's static binary. |
| [existing_directories](tests/existing_directories.rs) | Shared roots, escaped links, directory shape and actual-unit-user access are checked without changing borrowed metadata. |
| [volume_archive](src/volume_archive.rs) | Whole owned-volume archives with Unix metadata and overlay deletion markers, streamed by the static node helper. |
| [file_state_volume](src/file_state_volume.rs) | A fixed owned file-state subtree is measured and carried independently of other home contents. |
| [uploads](tests/uploads.rs) | A Files upload cancelled after real native writes waits for the connector's stopped result and preserves the original target. |

Live changes use the operator's `[node.docker] live_helper_image` and `live_helper_binary`.
The image is preloaded and trusted, and the binary is static; without them the change remains pending for recreate.
The helper enters only its owned unit's PID/mount namespace, has no Docker socket or host PID namespace,
uses the default seccomp profile, and has SYS_ADMIN/SYS_CHROOT with AppArmor unconfined.
Device creation additionally has its exact character-device mknod rule. The ordinary unit has no extra helper capabilities.

Native build checks use the runner carried in this same node binary and a preloaded environment base image.
`[node.docker] conformance_image` can name another trusted preloaded image. The check never pulls an image;
without one it reports that the build remains untested. Its disposable unit has no network, no shared home,
and no provider credentials, and each native case talks only to the scripted model inside it.
The conformance unit test accepts `ORCH_TEST_CONFORMANCE_NODE` for the prebuilt static node executable;
use it when the host's C library is newer than the test image's. The test never starts a compiler itself.
An explicit `--probe dynamic_tools` on the internal runner retains the unused Codex surface as an informational
observation. It is outside the background certification catalogue; Orch's tools use the required MCP cases.

The `isolation_enrollment` test opts in with `ORCH_TEST_ISOLATION_PROOF=1`. It requires an explicit prebuilt static
`ORCH_TEST_CONNECTOR_BINARY`, the daemon named by `ORCH_TEST_DOCKER_SOCKET`, a preloaded `busybox:latest`, and
registered `runc`, `runsc`, `runsc-keep` and `kata` runtimes. Run it where that daemon's bridge address is local;
for a daemon inside a container, a test runner can share that container's network namespace. Its temporary
directory must be visible to the daemon at the same path, because the node mounts its cached connector there.
The selected proof fails on a missing prerequisite. It enrolls each runtime through the real API and gateway,
executes a terminal command through its signed tunnel, and checks identity and workspace retention after restart.
