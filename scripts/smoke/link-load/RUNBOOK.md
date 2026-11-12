# Executing the three-leg WSS gate

These are integrator commands, not execution receipts. Source and pure controls do not qualify WSS.
P2P, QUIC, Kubernetes and additional platform/driver matrices are outside this gate.

1. Integrate the reviewed source series. Regenerate the lockfile through Cargo for the new optional
   `orch-link → sha2 0.11` edge; the package is already locked elsewhere. Inspect that generated delta,
   run the feature-on/default-off checks, and freeze the candidate before packaging. Use the existing
   normal packaging path and retain its successful unchanged-source build receipt and package check.
   Do not hand-edit the lockfile or replace normal archives with observer builds.
2. Build the three explicit QA apps from that same frozen source, using the same platform/toolchain
   and service-compatible builder as their normal counterparts. Inside that approved builder the
   selection is `cargo build --release --locked -p orch-api -p orch-gateway --features
   orch-api/test-observers,orch-gateway/test-observers`; the native connector uses its declared target
   with `-p orch-connector --features orch-connector/test-observers`. Keep the normal release version
   and tag, separate output files, actual command vectors and build receipts. No `--all-features`.
3. Supply an already-built mock-model fixture, Docker/Compose, the normal package's exact images,
   the test netem image prerequisites (`iproute2` and Python), and IFB support. Check the owned ports
   41400–41403 and control subnet. Never shape the host or use sudo/prune. The harness refuses stale
   run/evidence directories, mismatched running package bytes, and unsupported shaping attribution.

Capture QA builds on the host under the integrator's existing resource supervisor. The optional
capture entrypoint is QA-only; normal packaging remains unchanged. Use the exact immutable builder
image ID observed for the paired normal package, its existing cargo-home and counters, and the
approved QA target directory. The capture process runs host Git/JSON/hash operations and invokes
Cargo in that builder; Python is not required inside the image. It retains the actual Docker command,
compiler version, Cargo manifest/target/features/profile, original executable path, mapped host
output, and the SHA captured only after Cargo succeeds. It accepts no input executable to capture.

The path mapping comes directly from `container::Builder`: host repository → `/work:ro`, owned
host target directory → `/target`, cargo home → `/cargo-home`, counters → `/counters`. Server Cargo
emits `/target/release/orch-{api,gateway}`; the native Linux connector emits
`/target/<arch>-unknown-linux-musl/release/orch-connector`. Those paths map to the existing host
`Builder::target_dir()` (`<target-base>/container/target`) or the explicitly selected separate QA
target directory. `package::build` already reads those host paths and copies files into the normal
release. Keep the QA outputs separate from normal archives. The source mount does not need to
resolve a linked worktree's `.git`, because capture checks frozen source on the host.

```sh
# Host invocation. BUILDER_IMAGE_ID is the actual paired normal builder sha256:... ID.
# QA_TARGET, CARGO_HOME_DIR and COUNTERS are existing approved directories.
# VERSION/TAG are the normal package identity; SOURCE is its frozen commit.
python3 scripts/smoke/link-load/build_capture.py \
  --repo "$PWD" --source "$SOURCE" --output "$QA_SERVER_BUILD_RECORD" \
  --builder-image "$BUILDER_IMAGE_ID" --target-dir "$QA_TARGET" \
  --cargo-home "$CARGO_HOME_DIR" --counters "$COUNTERS" \
  --release-version "$VERSION" --release-tag "$TAG" -- \
  cargo build --release --locked --message-format=json -p orch-api -p orch-gateway \
  --features orch-api/test-observers,orch-gateway/test-observers

python3 scripts/smoke/link-load/build_capture.py \
  --repo "$PWD" --source "$SOURCE" --output "$QA_CONNECTOR_BUILD_RECORD" \
  --builder-image "$BUILDER_IMAGE_ID" --target-dir "$QA_TARGET" \
  --cargo-home "$CARGO_HOME_DIR" --counters "$COUNTERS" \
  --release-version "$VERSION" --release-tag "$TAG" -- \
  cargo build --release --locked --message-format=json --target "$TARGET" \
  -p orch-connector --features orch-connector/test-observers
```

The capture mirrors normal server static-OpenSSL and release-version/tag variables. Both normal
Linux paths select `--release`; the workspace release profile changes only `strip = "symbols"`.
Capture requires actual compiler events with opt-level3, no debug assertions/overflow checks and
`test:false`. Server builds use the observed GNU compiler host; the connector must explicitly name
the same architecture's musl target. Seal additionally matches the current native fixture platform.
These are existing package settings, not new performance criteria. Retain the normal build's
observed image ID alongside these captures; an invocation claim alone does not prove that a normal
package used that image.

Use each receipt's mapped host output, or an explicit copy of it, as the three seal inputs below.
A copy is accepted only when its SHA equals the SHA already captured from the successful build's
emitted artifact. The sealed record retains both original compiler output and selected copy paths.
There is no post-hoc receipt-upgrade command. Command-only receipts, changed copied bytes, missing
features, debug builds or incompatible targets fail. Normal package receipts keep the existing
successful command and clean frozen-source checks. Compiled BuildInfo, invocation source metadata
and actual executable hashes remain distinct; the launched copy must match the captured bytes.

Seal the captured QA inputs only after those builds pass.
The command registry budget is explicit instrumentation capacity: derive it from four phase guards,
the existing20-second single-probe cadence, initial/final probes, and gateway candidate probes.
Overflow invalidates evidence; increasing this budget never changes a product or latency limit.

```sh
python3 scripts/smoke/link-load/qa.py seal \
  --release "$CANDIDATE" --normal-record "$NORMAL_BUILD_RECORD" \
  --api "$QA_BIN/orch-api" --api-record "$QA_SERVER_BUILD_RECORD" \
  --gateway "$QA_BIN/orch-gateway" --gateway-record "$QA_SERVER_BUILD_RECORD" \
  --connector "$QA_BIN/orch-connector" --connector-record "$QA_CONNECTOR_BUILD_RECORD" \
  --max-commands "$REVIEWED_COMMAND_BUDGET" --output "$QA_INPUT_MANIFEST"
```

Run the `connector` normal case, then its QA companion **serially first**. These are the primary
100-MiB common/floor cases. Then run short normal/QA placement cases for `client` and `backplane`.
Use distinct fresh lower-case alphanumeric run IDs. A case failure stops the sequence for inspection;
do not retry automatically. Each run is intentionally partial until the complete index exists.

```sh
# Repeat this same fixed workload once per named leg and artifact mode, sequentially.
env -u ORCH_SMOKE_QA_MANIFEST \
  ORCH_SMOKE_RELEASE_A="$CANDIDATE" ORCH_SMOKE_MOCK_MODEL="$MOCK_MODEL" \
  ORCH_SMOKE_NORMAL_BUILD_RECORD="$NORMAL_BUILD_RECORD" ORCH_SMOKE_DIR="$PWD/.tmp/deploy-smoke" \
  ORCH_SMOKE_RUN=wss1connectornormal ORCH_SMOKE_PORT=41400 ORCH_LINK_SHAPE=connector \
  ORCH_SMOKE_RELEASE_PROOFS=0 bash scripts/smoke/link-load/run.sh

ORCH_SMOKE_RELEASE_A="$CANDIDATE" ORCH_SMOKE_MOCK_MODEL="$MOCK_MODEL" \
  ORCH_SMOKE_QA_MANIFEST="$QA_INPUT_MANIFEST" ORCH_SMOKE_DIR="$PWD/.tmp/deploy-smoke" \
  ORCH_SMOKE_RUN=wss1connectorqa ORCH_SMOKE_PORT=41400 ORCH_LINK_SHAPE=connector \
  ORCH_SMOKE_RELEASE_PROOFS=0 bash scripts/smoke/link-load/run.sh
```

For the other legs replace `connector` in the run ID and `ORCH_LINK_SHAPE` with `client` or
`backplane`. Keep the same frozen normal package and QA binaries. The QA mode first records the
normal running executable bytes, then stops the owned API/gateway and fixture connector before
starting separate QA copies. It does not turn this bootstrap into a normal functional run.

The reviewed allocation is centralized in `workload.py`. Each phase has one duplex bulk stream
alongside the owned terminal and MachineProbe. On the connector leg, common and floor each carry
**100 MiB each way**. On client/backplane, those are1-MiB placement probes. The abandoned stream still
advertises100 MiB but stops after receiving1 MiB; its actually received prefixes are checked and it
is never called a complete100-MiB transfer. Fresh admission uses1 MiB, avoiding another full repeat.
One reader stalls five seconds. Only the primary connector common phase has the existing three-second
100%-loss blackout; short probes retain1% random loss and actual steady loaded observations.

Profiles remain150ms each direction/1% independent loss,750/1600kbit common and75/160kbit floor.
Steady recommended API/carrier own-clock ACK bounds remain1000ms each. Floor has no ACK/PTY latency
promise. Every positive phase still needs actual request/command/machine/session joins, completion,
ordered bytes, observed queue drain and complete cleanup. A packet blackout is not by itself proof
that a WSS session disconnected/reconnected.

One100-MiB floor upload alone needs11184.81s (3h6m25s) at75kbit, before overhead. One common+floor pair
needs at least3h25m3s; the SERIAL normal/QA pair needs at least6h50m7s. Short placement, abort/fresh,
setup, loss/retransmissions and cleanup add time: the current allocation's useful-payload floor is
about7hours before measured setup and overhead.

The old7200s whole-phase guard was insufficient for the floor. The primary floor now has an explicit
21600s (six-hour) **harness work budget**, a conservative execution allowance using the existing
six-hour I/O work horizon; it is not a product timeout or floor performance promise. Other phases
retain the7200s harness guard. Budget expiry cancels/joins owned observation work and saves partial
payload progress with `failure_domain:harness_budget` and `assessment:incomplete_no_product_verdict`.
The run is incomplete and cannot qualify; continued progress at that stop must never be called a
product defect. An inner observation timeout retains its own classification. Phase result/mixed JSON
is copied into evidence before fixture teardown, including incomplete runs.

For bounded execution planning, the serial primary pair has at most24hours of client-phase budgets;
the four short cases retain32hours of existing phase watchdog allowances (56hours total if every phase
uses its entire budget). This is a guard ceiling, not an expected duration or a whole-stack bound:
setup/image commands have no one global deadline. Plan from the approximately7-hour payload lower
bound plus measured setup/overhead, and let the integrator choose its work window; a window stop is incomplete.
No further numeric performance requirement is introduced.

**The combined-concurrency gate remains OPEN.** The current stream case does not exercise64live and
16waiting native TCP channels under actual impaired WSS. Same-candidate complementary controls include
`ports_carrier::rule_2_sixty_four_real_tcp_connections_and_sixteen_waiters_share_one_environment_and_one_deadline`,
`tunnels::a_hundred_mebibytes_each_way_under_a_one_mebibyte_window_arrive_identical_and_memory_stays_bounded`,
and `tunnels::a_tunnel_whose_session_is_lost_and_found_within_the_grace_never_shows_a_close_to_its_program`.
Reuse their real fixtures/protocols for that remaining combined phase; do not infer waiting counts
from pending HTTP or unused tickets. The existing slow native port tests use an accelerated transport
clock and cannot replace the real-time kernel/WSS observation. Even six successful attributed cases
currently produce `wss_release_gate_qualified:false` with the combined-concurrency gap named.

Put an index beside the retained evidence, with paths relative to that directory:

```json
{
  "source": "<the exact frozen commit>",
  "placements": {
    "client": {"normal": "wss1clientnormal/linkload", "qa": "wss1clientqa/linkload"},
    "connector": {"normal": "wss1connectornormal/linkload", "qa": "wss1connectorqa/linkload"},
    "backplane": {"normal": "wss1backplanenormal/linkload", "qa": "wss1backplaneqa/linkload"}
  }
}
```

```sh
ORCH_SMOKE_WSS_INDEX="$PWD/.tmp/deploy-smoke/evidence/wss-index.json" \
  ORCH_SMOKE_RELEASE_PROOFS=1 bash scripts/smoke/link-load/run.sh
```

This final command starts no stack. It revalidates raw case evidence against the current checkout's
commit, all six distinct run identities, matching normal/QA binaries, actual profiles and cleanup.
It writes false before verification. Current successful attributed cases still leave the named
combined-concurrency gap open; the index returns nonzero and cannot declare full qualification. Its nonzero exit always rejects qualification. Supplying the same index to the
existing release-proof aggregator reuses this exact frozen-candidate check.

Retain phase exits, both ends' counts/hashes (including the received cut prefixes), progress,
ordered native PTY markers, request/command/machine/session joins, per-process clock domains,
positive open checkpoints, separate finished-process snapshots, qdisc/filter/socket attribution,
build identities, exact owned-image retirement and final cleanup. Keep==1, a missing normal build
receipt, a replaced process, unknown/inconsistent queue accounting or any incomplete case fails.
No logical peak is described as an RSS measurement or a mathematical capacity bound.

Queue lifecycle evidence keeps every historical gauge. A receiver that closes with zero queued and
in-flight work, no unknown size/loss/failure and exact enqueue/selected/completed conservation records
immutable close-time counters. It can qualify as `retired_drained` only while those counters remain
unchanged and an active same-family replacement exists. Closing with pending work, later completion,
discard, cancellation, overflow or a racing sender never becomes clean retirement. Required active
families are checked again at phase boundaries; final shutdown is recorded separately from the last
positive open phase. Public PortUse/PTY closure does not itself close the carrier queues.

Connector placement joins the observed socket inode to the exact recorded connector PID, then joins
that process to the owned run and enrolled machine. An unrelated same-namespace socket, ambiguous
connector process set, missing inode or changed process cannot qualify any of the three legs.
