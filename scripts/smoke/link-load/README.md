# Released large transfers

`bash scripts/smoke/link-load/run.sh` starts an owned stack from `ORCH_SMOKE_RELEASE_A` and requires
`ORCH_SMOKE_MOCK_MODEL` to name an already-built `orch-mock-llm`. It never substitutes development
server/connector binaries. The fixture model and the byte producer are explicit test doubles;
the API, edge, gateway, node, connector and port carrier are the release's actual programs.

It needs Docker/Compose, Python 3, OpenSSL, jq, curl, zip, and the kernel's IFB/netem support. A tiny
test-only image provides iproute2. `NET_ADMIN` is granted only to a transient sidecar joining this
run's selected client or connector namespace: no host interface, qdisc, unrelated unit or process is changed. The default
stack/front/model ports are 41400–41403; concurrent copies must select their own `ORCH_SMOKE_PORT`.

`ORCH_LINK_SHAPE=client` (the default) measures the public client-to-edge path.
`ORCH_LINK_SHAPE=connector` shapes only the fixture connector's namespace, including its actual
pinned WSS connection to the gateway. Run these separately with distinct `ORCH_SMOKE_RUN` values;
two shaped150-ms legs would be600-ms RTT, not one300-ms profile. The connector fixture has one
explicit pinned endpoint. Retained evidence includes its actual established gateway socket,
network-namespace identity and bidirectional netem byte/packet/drop counters. Missing IFB/netem
or missing path attribution fails the relevant check rather than silently skipping it. This
does not shape the gateway-to-API backplane or prove that path's performance.

At 150 ms one-way, 750 kbit/s upload, 1600 kbit/s download and 1% independent packet loss, three
concurrent full-duplex streams each send **100 MiB in each direction**. Bytes differ by stream,
direction and offset; independent ends record SHA-256 and counts. One reader intentionally stalls
five seconds, a three-second 100% packet-loss interval interrupts all three, and every completed
stream must still have the exact bytes. A separate stream is abandoned after a MiB, its producer
must close, and a fresh admission transfers 100 MiB each way. The tenth-rate floor transfers a MiB
each way without a speed assertion. Two hours per transfer phase is a hang guard, not a latency
promise; the full profile can take over an hour. The peer's 300-second close guard detects leaked
producers. Finite 1024-packet queues apply backpressure; recorded write waits show the stalled path.

During each transfer phase the client also opens one ordinary terminal in the owned environment.
It sends numbered output markers whose command text cannot echo the marker itself, and sends one
public `MachineProbe` at a time. Independent observers sample at the existing20-second heartbeat
cadence; that frequency controls measurement density, not a performance promise. Idle and drained
observations are retained for comparison. A marker or command that remains pending prevents another
one from being queued. The terminal is closed by its exact created identity afterward; no existing
shell or owner credential is used. Marker numbers, elapsed times, overlap with actual bulk streams
and cleanup are checked. No numeric remote-terminal echo SLA is invented.
The owned fixture explicitly permits its terminal and current `[ports] allowed=["place"]` policy;
it creates no public listener or widened port opening. The older `environment.ports` fixture
setting is replaced by that existing policy shape. The full send-and-echo observation, terminal
upgrade and fixture socket shutdown have30-second hang guards derived from the existing probe/PTY
budgets; a blocked write cannot hide outside the echo guard. These guards are not latency targets.

**This remains a partial WSS qualification runner.** A public probe's successful completion within
1000ms conservatively proves its preceding command ACK met spec05's one-second requirement. A slower
completion leaves the exact ACK time unknown; it does not prove the ACK was late. The1500ms legacy
simulator threshold does not change that requirement. Each phase writes an explicit qualification
receipt with this distinction. At the tenth-rate floor there is no ACK or remote-PTY latency
promise: completion/echo times are still recorded, `ack_budget_ms` is null and qualification marks
the latency requirement as `not_required_at_floor`. Ordered bytes, progress, no hangs, bounds and
cleanup remain required, together with the existing protocol and harness hang guards. A floor
observation never relaxes the recommended profile's1000ms requirement. Kernel TCP queue lengths, connector RSS/high-water and container
working-set observations are supporting measurements; they do not expose every application queue
or prove a whole-session queue bound. `runner.json` therefore carries
`wss_release_gate_qualified:false`, and phase receipts carry `complete_wss_release_gate:false`, even
when the functional/hash checks pass. A zero runner exit means those bounded checks completed,
not that all existing WSS release criteria have passed, **unless release-proof mode is enabled**.
With the existing `ORCH_SMOKE_RELEASE_PROOFS=1` setting used by `scripts/smoke/run-all.sh`, the runner
saves `wss-release-qualification.json` and fails when any phase or placement qualification is
missing, false or incomplete. The current partial observations therefore cannot make the release
aggregator green. Direct developer runs may still finish their functional checks with an explicit
partial receipt. No success flag is manufactured from byte/hash success. Exact ACK observation (where the completion
bound is insufficient), application queue accounting/drain, and the separate backplane path remain
required follow-up proofs. Do not replace those missing observations with an RSS limit.

Evidence survives stack cleanup under `.tmp/deploy-smoke/evidence/<run>/linkload`: exact candidate
manifest and connector hash, image identities, qdisc configuration and counters, both ends' counts
and hashes, per-MiB progress checkpoints, mixed observations, namespace/counter attribution,
resource samples, network-cut times, logs and every phase's exit. `ORCH_SMOKE_KEEP=1` retains the fixture
for diagnosis. Credentials and TLS private keys stay outside the evidence directory.

The large-transfer, load and soak runners share dependency evidence: the policy and both lockfiles
with SHA-256 are retained. `ORCH_SMOKE_DEPENDENCY_PROOF` can name the whole proof runner's
`results.json`; only a green dependency check with matching clean commit and input hashes attests
to this candidate. Without it, the candidate dependency check is explicitly unperformed. The
reported round-copy result from cargo-deny 0.20.2 (bans, licences, sources all ok after MIT-0 and
the exact cipher/inout exceptions) is retained as a report, without implying an advisory scan or
a fresh candidate pass. Every integration round also records its own `deny=` status.

`python3 -m unittest discover -s scripts/smoke/link-load -p 'test_*.py'` checks evidence rejection,
not product transport. No execution is claimed by the presence of these runners; an integrator
must retain a successful released-stack run before calling the release transfer proof complete.
The default-off [dedicated observer controls](OBSERVERS.md) provide the exact real-TLS ACK and
selected queue hooks for a separately reviewed test invocation. Their loopback receipt always
remains partial; the hooks alone do not qualify the full mixed runtime path.
The new pure tests cover protocol/evidence rejection only and do not execute sockets, kernels,
containers or any released Orch program. Three100-MiB duplex streams share the shaped bandwidth:
the profile's payload-only upload minimum is about56minutes for that phase. The existing real
floor phase remains1MiB; it is not a claim of a100-MiB real floor pass, which alone would take
more than three hours at75kbit/s and cannot fit the current two-hour phase guard.
