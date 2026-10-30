# Released large transfers

`bash scripts/smoke/link-load/run.sh` starts an owned stack from `ORCH_SMOKE_RELEASE_A` and requires
`ORCH_SMOKE_MOCK_MODEL` to name an already-built `orch-mock-llm`. It never substitutes development
server/connector binaries. The fixture model and the byte producer are explicit test doubles;
the API, edge, gateway, node, connector and port carrier are the release's actual programs.

It needs Docker/Compose, Python 3, OpenSSL, jq, curl, zip, and the kernel's IFB/netem support. A tiny
test-only image provides iproute2. `NET_ADMIN` is granted only to a transient sidecar joining this
run's client namespace: no host interface, qdisc, unrelated unit or process is changed. The default
stack/front/model ports are 41400–41403; concurrent copies must select their own `ORCH_SMOKE_PORT`.

At 150 ms one-way, 750 kbit/s upload, 1600 kbit/s download and 1% independent packet loss, three
concurrent full-duplex streams each send **100 MiB in each direction**. Bytes differ by stream,
direction and offset; independent ends record SHA-256 and counts. One reader intentionally stalls
five seconds, a three-second 100% packet-loss interval interrupts all three, and every completed
stream must still have the exact bytes. A separate stream is abandoned after a MiB, its producer
must close, and a fresh admission transfers 100 MiB each way. The tenth-rate floor transfers a MiB
each way without a speed assertion. Two hours per transfer phase is a hang guard, not a latency
promise; the full profile can take over an hour. The peer's 300-second close guard detects leaked
producers. Finite 1024-packet queues apply backpressure; recorded write waits show the stalled path.

Evidence survives stack cleanup under `.tmp/deploy-smoke/evidence/<run>/linkload`: exact candidate
manifest and connector hash, image identities, qdisc configuration and counters, both ends' counts
and hashes, network-cut times, logs and every phase's exit. `ORCH_SMOKE_KEEP=1` retains the fixture
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
