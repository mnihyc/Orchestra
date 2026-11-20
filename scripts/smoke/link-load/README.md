# Large transfers on an impaired link

`bash scripts/smoke/link-load/run.sh` starts a stack of its own from a packaged release (`ORCH_SMOKE_RELEASE_A`) and carries 100 MiB each way through the released API, gateway and connector while the network is cut, slowed and made to lose packets. It checks that the bytes arrive complete and in order, that an abandoned stream is cleaned up, and that new connections are still admitted. It needs `ORCH_SMOKE_MOCK_MODEL` to name an already built `orch-mock-llm`, which acts as the model server.

Default mode measures the normal packaged programs. `ORCH_SMOKE_QA_MANIFEST` selects a separately identified QA companion: copies of the API, gateway and connector built with observers, which are used after normal setup and enrollment and must match their successful, unchanged build records. The mock model and the byte producer are test doubles, and a QA observation never passes for a normal package.

## Run it

Run one leg and one mode at a time, each with a fresh run name, and keep the evidence directory when it ends. A run takes hours: the 100-MiB transfer at the floor rate alone needs more than three hours of upload, and one common-plus-floor pair needs at least three and a half hours. Plan about seven hours for the normal and QA runs of the connector leg before setup and overhead.

**You need**

- Docker with Compose, Python 3, OpenSSL, `jq`, `curl`, `zip`, and a kernel with IFB and netem support. The runner shapes only its own containers' namespaces and needs no `sudo`.
- A release directory, for `ORCH_SMOKE_RELEASE_A`, that has been packaged the normal way.
- A built `orch-mock-llm`, for `ORCH_SMOKE_MOCK_MODEL`.
- For the QA companion: the QA builds of the API, gateway and connector, made from the same source (below).
- Free ports 41400 to 41403, or your own `ORCH_SMOKE_PORT` for each copy you run at the same time. A run name that has not been used before: the runner refuses an existing scenario or evidence directory.

**Steps**

1. Run the connector leg in normal mode first. It is the longest, so start it before the short legs. `ORCH_SMOKE_RUN` is a lower-case alphanumeric name, and `ORCH_LINK_SHAPE` is the leg: `connector`, `client` or `backplane`.

   ```sh
   env -u ORCH_SMOKE_QA_MANIFEST \
     ORCH_SMOKE_RELEASE_A="$RELEASE" ORCH_SMOKE_MOCK_MODEL="$MOCK_MODEL" \
     ORCH_SMOKE_NORMAL_BUILD_RECORD="$NORMAL_BUILD_RECORD" ORCH_SMOKE_DIR="$PWD/.tmp/deploy-smoke" \
     ORCH_SMOKE_RUN=wss1connectornormal ORCH_SMOKE_PORT=41400 ORCH_LINK_SHAPE=connector \
     ORCH_SMOKE_RELEASE_PROOFS=0 bash scripts/smoke/link-load/run.sh
   ```

Individual runs use `ORCH_SMOKE_RELEASE_PROOFS=0` to collect bounded case evidence. Their runner and
phase full-gate flags remain false. A direct single-run release-proof invocation still refuses to
claim the whole gate. Once distinct normal and QA cases for client, connector and backplane exist,
`ORCH_SMOKE_WSS_INDEX` selects the existing verifier without launching another stack. It rechecks raw
bytes/prefix hashes, counts/progress, mixed observations, actual shaping, identities, QA checkpoints,
normal and QA binary consistency, final process cleanup and exact owned-image retirement for the
same clean frozen candidate. The combined64-live/16-waiting impaired-native case is still missing: the index explicitly keeps
`wss_release_gate_qualified:false` even after all attributed stream cases pass. Existing native
capacity/window/resume controls are complementary evidence, not a substitute for that combined case.
Source presence and pure controls are not runtime qualification.

2. Make the QA builds of the same source with the features that expose the observers. Capture each build so that its command, compiler, features and checksum are recorded, then seal them into one manifest:

   ```sh
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

   python3 scripts/smoke/link-load/qa.py seal \
     --release "$RELEASE" --normal-record "$NORMAL_BUILD_RECORD" \
     --api "$QA_BIN/orch-api" --api-record "$QA_SERVER_BUILD_RECORD" \
     --gateway "$QA_BIN/orch-gateway" --gateway-record "$QA_SERVER_BUILD_RECORD" \
     --connector "$QA_BIN/orch-connector" --connector-record "$QA_CONNECTOR_BUILD_RECORD" \
     --max-commands "$COMMAND_BUDGET" --output "$QA_INPUT_MANIFEST"
   ```

   `BUILDER_IMAGE_ID` is the image ID of the builder that made the normal package. The capture accepts no ready-made executable: the checksum is taken only after Cargo succeeds. A debug build, a missing feature or a copy with other bytes is refused. The command budget is the capacity of the instrumentation, derived from the phases and probes of the run; running over it makes the evidence invalid and changes no product limit.

3. Run the QA companion of the same leg, then the normal and QA runs of `client` and `backplane`, each with its own run name and one after another. The QA mode records the normal bytes first, then stops the owned stack and starts the QA copies.

   ```sh
   ORCH_SMOKE_RELEASE_A="$RELEASE" ORCH_SMOKE_MOCK_MODEL="$MOCK_MODEL" \
     ORCH_SMOKE_QA_MANIFEST="$QA_INPUT_MANIFEST" ORCH_SMOKE_DIR="$PWD/.tmp/deploy-smoke" \
     ORCH_SMOKE_RUN=wss1connectorqa ORCH_SMOKE_PORT=41400 ORCH_LINK_SHAPE=connector \
     ORCH_SMOKE_RELEASE_PROOFS=0 bash scripts/smoke/link-load/run.sh
   ```

   Stop at the first failing run and read it. Do not retry automatically.

4. Write an index beside the evidence, with paths relative to its directory, naming the six runs and the exact source commit, then check it. This starts no stack:

   ```json
   {
     "source": "<the exact commit>",
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

**What you will see.** Each run prints its phases and ends with an exit status of 0 when every check of that case passed. It leaves its evidence under `.tmp/deploy-smoke/evidence/<run>/linkload`: the candidate manifest, image identities, shaping settings and counters, both ends' counts and hashes, progress, logs and every phase's exit. The final command with `ORCH_SMOKE_WSS_INDEX` writes `wss-release-qualification.json` and exits nonzero unless every case passed and the whole set holds together. The 64-live-and-16-waiting connections case under an impaired link is not part of these runs, so the index still reports `wss_release_gate_qualified:false` even when all six runs pass.

**If it fails**

- A budget that ends marks the run incomplete, with the failure in the harness and no verdict on the product. The partial progress is kept.
- Setup that fails before the runner owns anything leaves earlier evidence untouched. After that point the runner reports what it kept.
- Set `ORCH_SMOKE_KEEP=1` to keep the fixture for a look; such a run cannot count toward the index.
- Cleanup removes only what the run owns, and it writes `cleanup-files.sh` into the evidence directory first. Read that file before you remove anything yourself.

## What a run does

**Where the network is impaired.** `ORCH_LINK_SHAPE` picks the one path to shape. Shaping two would add two 300-ms round trips.

- `client` (the default) shapes the path from a client to the web edge.
- `connector` shapes only the namespace of the fixture connector, including its actual pinned WSS connection to the gateway. The connector has one explicit pinned endpoint.
- `backplane` shapes only the gateway-to-API address and TCP port 8444, with its inverse ingress redirected to the owned IFB. Other gateway traffic stays in the unshaped band. An address family it cannot shape, or a changed tuple, fails closed.

Each placement keeps its evidence: the established socket, the namespace, and bidirectional netem byte, packet and drop counters. A missing IFB or netem, or a path that cannot be attributed, fails the check rather than being skipped. A short configuration check runs before the long transfer. Run the three placements separately.

**The profile.** 150 ms each way, 1% independent loss, 750 kbit/s up and 1,600 kbit/s down in the common phase, and 75 and 160 kbit/s in the floor phase. On the connector leg the normal and QA runs each carry one 100-MiB stream both ways in the common and the floor phase. The client and backplane legs use the same mixed traffic with 1-MiB probes. The common phase on the connector leg also holds a five-second reader stall and a three-second blackout. An abandoned stream is checked by the bytes received; a fresh admission uses 1 MiB instead of another 100 MiB. Floor phases have no acknowledgement or latency promise.

**Alongside the transfer.** The client opens an ordinary terminal in the owned environment, sends numbered output markers whose command text cannot echo the marker, and sends one public `MachineProbe` at a time. Independent observers sample at the 20-second heartbeat. Idle and drained observations are kept for comparison. The terminal is closed by its own created identity afterwards. No existing shell or credential is used, and the fixture opens no public listener.

**What counts as success.** Public completion within 1,000 ms proves the acknowledgement that came before it; a slower one leaves the timing unknown. The QA companion joins the request id to the committed probe and the authenticated machine and session, with separate API and gateway duration pairs, and both must be within 1,000 ms on measured steady loaded probes. No timestamp is subtracted across process clocks. A call that spans the three-second blackout must still complete with correlation, progress and ordered bytes, but it counts as neither a steady-state success nor a failure. Every phase needs a steady loaded probe. Queue checks need zero queued and in-flight bytes at the drain points and exact conservation of what was selected, sent and processed.

**Time and budgets.** The 100-MiB upload at the floor rate needs at least 3 h 6 m 25 s, so the floor phase has a six-hour budget, and other phases keep their two-hour guards. These bound the test's work, not the product. When a budget runs out, the run keeps its progress and records an incomplete result of the harness, never a product defect. The peer's 300-second close guard still catches leaked producers.

## The evidence

Evidence survives the cleanup of the stack in `.tmp/deploy-smoke/evidence/<run>/linkload`: the release's manifest and connector hash, image identities, the shaping configuration and counters, both ends' counts and hashes, per-MiB progress, the mixed observations, namespace and counter attribution, resource samples, the times of the network cuts, logs and every phase's exit. Credentials and TLS private keys stay outside it. `ORCH_SMOKE_KEEP=1` keeps the fixture for diagnosis.

Individual runs use `ORCH_SMOKE_RELEASE_PROOFS=0`. Only the index of all six runs (`ORCH_SMOKE_WSS_INDEX`) rechecks raw bytes and prefix hashes, counts, mixed observations, shaping, identities, the QA checkpoints, the consistency of normal and QA binaries, process cleanup and the retirement of the owned image, for the same clean commit. The combined case of 64 live and 16 waiting connections under an impaired link is not a run here, so the index keeps `wss_release_gate_qualified:false` even when all six pass. The tests `ports_carrier::rule_2_sixty_four_real_tcp_connections_and_sixteen_waiters_share_one_environment_and_one_deadline` and `tunnels::a_hundred_mebibytes_each_way_under_a_one_mebibyte_window_arrive_identical_and_memory_stays_bounded` cover those limits separately, on a faster clock.

## Cleanup

Choose a fresh run name: startup refuses an existing scenario or evidence directory. Before it unpacks the stack, the shared helper writes `cleanup-files.sh` and `cleanup-files.json` in the evidence directory. The shell file holds one literal absolute `rm -r --` argument, without force, and work and evidence paths that pass through a symlink are refused. Its path, its content and the owned directory's identity are checked again right before removal. No variable or command substitution reaches `rm` on this runner's stack path.

Cleanup stops the run's own model, monitor and outstanding probe, checks labels before it removes extra containers, then needs a successful Compose teardown and the absence of the exact project's containers, networks and volumes. A failure keeps the fixture files, returns nonzero and marks the run as not passing. `cleanup.json` and `runner.json` are written after cleanup, and `final_exit` includes its failures. A failure before the runner owns anything never overwrites earlier evidence. The run's netem image tag is labelled and its image ID recorded; after all owned containers are gone, cleanup checks that ID and label and refuses a tag that was replaced or is still in use. It removes only that tag, with a non-forced `docker image rm --no-prune`. The release's own images, shared parent layers and cached releases stay, the empty run parent directory stays, and nothing is pruned globally.

The large-transfer, load and soak runners share their dependency evidence: the policy and both lockfiles, with their SHA-256. `ORCH_SMOKE_DEPENDENCY_PROOF` can name the `results.json` of the whole check run. Only a green dependency check with a matching clean commit and input hashes counts for this release. Without it the dependency check is reported as not done.

## Test the runner

```sh
python3 -m unittest discover -s scripts/smoke/link-load -p 'test_*.py'
```

These tests cover the rejection of bad evidence. They open no sockets and start no containers or released programs. They do not replace a run of the released stack. The default-off observer controls give the exact real-TLS acknowledgement and queue hooks for a separately reviewed test run, and their loopback receipt is partial: the hooks alone do not cover the full mixed path.

`python3 -m unittest discover -s scripts/smoke/link-load -p 'test_*.py'` checks evidence rejection,
not product transport. No execution is claimed by the presence of these runners; an integrator
must retain a successful released-stack run before calling the release transfer proof complete.
The default-off dedicated observer controls provide the exact real-TLS ACK and
selected queue hooks for a separately reviewed test invocation. Their loopback receipt always
remains partial; the hooks alone do not qualify the full mixed runtime path.
The pure tests cover protocol/evidence rejection only and do not execute sockets, kernels,
containers or any released Orch program. The primary connector common and floor phases each
retain one100-MiB duplex stream; the other legs use1-MiB placement probes. The floor's six-hour
harness budget preserves incomplete progress without creating a product latency requirement.
