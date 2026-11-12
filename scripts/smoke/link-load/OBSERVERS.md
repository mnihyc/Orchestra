# Dedicated WSS observer controls

`test-observers` is an explicit, default-off QA feature on `orch-link`, `orch-api-core` and
`orch-gateway-core`. Normal/default compilation excludes the observer module, queue fields and
hooks. It adds no product endpoint, listener, protocol field, permission or metrics. A build with
`--all-features` intentionally includes it; such a build cannot establish its absence from a
release artifact. Keep the QA command and candidate packaging/build receipts separate.

The dedicated gateway test forwards the feature to link and its existing **dev-only** API
dependency. There is no added gateway-to-API normal dependency or API-to-gateway dependency.
The pure manifest checks are source guards; Cargo's resolved graph and compilation still need
to be retained for the exact candidate.

The finite hook set is:

| Observation | Actual boundary |
| --- | --- |
| API command handoff | The existing backplane writer selects `DownFrame::Cmd`, before encoding/socket send. |
| Gateway command handoff | The real upgraded TLS/WSS sink receives the watched `Frame::Cmd`, before its original send. |
| Gateway ACK | The original TLS/WSS source returns the watched `Frame::CmdAck`. |
| API ACK | The original API backplane reader decodes `UpFrame::CmdAck`, before state/DB mirroring. |
| Link outgoing queues | Existing control, events, data and datagram enqueue, selection and completed send. |
| Session queues | Existing inbound and session-event enqueue, selection and receiver lifetime. |
| API/gateway backplane queues | Existing enqueue, selection, completed socket send and receiver lifetime. |
| API terminal output | Existing TerminalHub/channel producers and the API's `terminal_out` consumer, including failed forwarding. |
| Controller output/notices | CommandIssuer and ControllerLink producers; actual API output/notice consumers, including delegated delivery lifetime. |
| Machine inputs | Existing supervisor/publication enqueue and the core's original `recv`/`try_recv` batch; selected unprocessed tails remain incomplete. |
| Tunnel storage | The existing optional channel `Gauge` in the owned machine fixture. |

The four ACK timestamps use the same process-local monotonic `Instant` clock. The carrier span
includes the real TLS transport; the API span conservatively includes both backplane directions.
The command result is held independently, so a quick ACK cannot be confused with quick completion
or DB status mirroring. Only explicitly registered command IDs are retained, without payloads.
The queue registry has an explicit ceiling; overflow makes the evidence incomplete.

Queue snapshots count messages and logical serialized payload bytes, including peaks. Selected
batch payload counts remain until handling or cancellation finishes. `processed` counts
completed local handling separately from `sent`, which means handoff to the existing next queue or
socket at that observer's boundary; neither implies native command completion. Queue capacity and
the original scheduling/cancellation branches are unchanged. The testkit's raw channel consumers
do not constitute observer evidence; the dedicated TLS fixture uses the instrumented API consumers.
These snapshots do not measure allocator use, kernel buffering, every product queue or a mathematical bound on an
unbounded queue. A selected send is separate from a completed send. Failed enqueue, selected work
without completion, receiver-close discard and indeterminate close races remain explicit. Zero
after closing a receiver is not proof of a drain. The positive fixture requires an open, observed
drain without those exceptional outcomes before taking its separate shutdown snapshot. Existing
queue capacities, priority, cancellation and failure policy remain unchanged.

The real-TLS control transfers the existing 100 MiB criterion in both directions with bytes that
depend on direction and offset. It pauses a reader after actual payload, observes a harmless
`MachineProbe` ACK/result separately, and echoes bytes over another signed PTY carrier. That echo
is not a native shell test. Its 120-second bulk guard only bounds this loopback test; it is not a
replacement for the released-stack transfer guards or a new performance threshold.

Future integrator commands, **not execution receipts**:

```sh
cargo tree --locked -p orch-gateway-core --features test-observers -e features
cargo test --locked -p orch-link --features test-observers --lib test_observers::tests:: -- --test-threads=1
cargo test --locked -p orch-gateway-core --features test-observers --test wss_observers -- --test-threads=1
cargo test --locked -p orch-link --no-default-features --doc
```

The default-feature doctest checks that importing the observer symbol fails; its compile-fail
example is omitted when the observer feature is selected, so legitimate all-features rustdoc
does not fail because the symbol exists. The default gateway control also checks its feature
selection. The pure `test_observer_features.py` checks current manifest forwarding, absence of a
new normal dependency cycle, cfg export and the current packaging argument vectors.

The reviewed packaging sources at the packet's base commit select:

| Builder | Cargo selection |
| --- | --- |
| `release/package.rs` server vector | `build --release --locked`, then existing `-p` programs. |
| `release/package.rs` static vector | `build --release --locked --target ...`, then existing `-p` programs. |
| `release/connectors.rs` native vectors | `build --release --locked`, existing target/package selections. |
| `ci.yml` native release job | `build --release --locked -p orchctl -p orch-connector -p orch-node --target ...`. |

None selects `test-observers` or `--all-features` in this source snapshot. Recheck the actual
build arguments, resolved feature graph and manifest for the final candidate; source inspection
alone cannot attest a binary. Do not reuse an instrumented all-features target as a normal
packaged release.

`ORCH_TEST_WSS_OBSERVATIONS` may name an owned output file for the dedicated test's snapshots.
It explicitly records `kernel_profile: unshaped_loopback` and
`complete_wss_release_gate: false`. The recommended profile still requires a measured ACK within
1000 ms; the tenth-rate floor has no ACK or PTY latency promise. Neither loopback success nor
source hooks qualify the shaped public-client/connector/backplane topology. Actual socket path,
loss/RTT placement, mixed native PTY traffic, progress, ordered bytes, bounds, cancellation and
cleanup must still be observed in the reviewed runtime topology before release qualification.

The QA process recorder adds explicit `test-observers` forwarding on the API, gateway and connector
apps. With that feature selected, an owned QA invocation may provide `ORCH_TEST_WSS_OBSERVER_DIR`,
`ORCH_TEST_WSS_RUN`, `ORCH_TEST_WSS_MACHINE`, `ORCH_TEST_WSS_SOURCE`, and an explicit positive
`ORCH_TEST_WSS_MAX_COMMANDS`. The output directory must already exist and be canonical; the machine
must be the enrolled fixture identity. The command budget bounds instrumentation only and must be
chosen from the reviewed run plan, including gateway candidate probes. These inputs are not product
CLI options, product configuration or a listener. Without the feature, the code and reads are absent.

Each process snapshot records its actual executable BLAKE3 and SHA-256, compiled build identity and fresh process
instance. `declared_source_commit` is an invocation claim requiring the independent build receipt;
it is not reported by the binary. Normal package and QA binary identities remain separate. Snapshot
replacement is atomic, normal completion explicitly joins the writer, and early scope exit is
`abandoned`. Every snapshot still says `complete_wss_release_gate:false`.

The public MachineProbe future supplies its existing request ID through a task-local observation
scope. Only its actual committed `machine.probe` command, before original adoption, is registered
at the API. The gateway registers bounded candidate probes for the selected machine only after its
existing session authentication; the API request/command ID join distinguishes measured public calls
from other probes. Existing `x-request-id` is retained by the mixed client without other headers.
ACK durations use pairs from one process clock domain; timestamps from different processes must
never be subtracted. Conflicting identity/session correlation or registry overflow stays incomplete.

The continuation wires explicit QA replacement into the owned stack after normal setup/enrollment,
with copied prelaunch SHA-256 checked against actual process SHA-256. `sha2` is optional under this
feature and already exists in the workspace lock; the integrator must regenerate its new optional direct edge
before locked builds and check the normal resolved graph. No observer work is selected by normal
packaging flags.

Checkpoint requests stay on the owned output mount. They name only this run, a fresh ID, the existing
four phases and start/drained boundaries, idle start or shutdown/pre_stop: ten acknowledgements at
most. Drained/pre_stop snapshots wait for the existing positive-open conservation predicate; they
cannot use receiver closure to create a zero. The same process instances must acknowledge before and
after each phase. The native connector receives an exact owned-process SIGTERM before removal; API
and gateway use their existing Compose stop lifecycle. Finished/abandoned outcomes are retained after
the separate pre-stop snapshot. Runtime qualification remains unperformed until the integrator executes the
normal/QA three-leg set and the final verifier accepts its actual evidence.
