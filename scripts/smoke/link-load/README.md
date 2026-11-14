# Released large transfers

`bash scripts/smoke/link-load/run.sh` starts an owned stack from `ORCH_SMOKE_RELEASE_A` and requires
`ORCH_SMOKE_MOCK_MODEL` to name an already-built `orch-mock-llm`. Default mode measures the normal packaged programs. `ORCH_SMOKE_QA_MANIFEST` selects a separately
identified QA companion after normal setup and enrollment; its copied API/gateway/connector binaries
must match their successful, unchanged, explicitly feature-selected build receipts. The fixture model
and byte producer remain explicit test doubles. QA observations never relabel their binaries as normal packages.

It needs Docker/Compose, Python 3, OpenSSL, jq, curl, zip, and the kernel's IFB/netem support. A tiny
test-only image provides iproute2 and Python for owned namespace/process observation. `NET_ADMIN` is granted only to a transient sidecar joining this
run's selected client, connector or gateway namespace: no host interface, qdisc, unrelated unit or process is changed. The default
stack/front/model ports are 41400–41403; concurrent copies must select their own `ORCH_SMOKE_PORT`.

`ORCH_LINK_SHAPE=client` (the default) measures the public client-to-edge path.
`ORCH_LINK_SHAPE=connector` shapes only the fixture connector's namespace, including its actual
pinned WSS connection to the gateway. Run these separately with distinct `ORCH_SMOKE_RUN` values;
shaping both the client and connector would add two300-ms round trips. The connector fixture has one
explicit pinned endpoint. Retained evidence includes its actual established gateway socket,
network-namespace identity and bidirectional netem byte/packet/drop counters. Missing IFB/netem
or missing path attribution fails the relevant check rather than silently skipping it.
`ORCH_LINK_SHAPE=backplane` selects only the actual gateway→API IPv4 address and TCP8444 tuple,
with its inverse ingress redirected to the owned IFB. Other gateway traffic stays in the unshaped
band. Gateway PID/socket ownership, exact filters, namespace, profile text and final packet/drop
counters are retained. Unsupported address families or changed tuples fail closed. The short
configuration preflight runs before the long payload. All three placements are required separately.

At150ms each way,1% independent loss and750/1600kbit common rates (75/160kbit floor), the connector
normal/QA cases each carry one100-MiB duplex stream in BOTH common and floor phases. Run those two
cases serially first. Client/backplane cases use the same mixed controls with1-MiB placement probes.
The primary common case retains a5s reader stall and3s intentional blackout. An abandoned stream is
checked by its received prefixes; fresh admission uses1MiB rather than repeating another100MiB.

The100-MiB floor has a3h6m25s useful-upload lower bound. Its whole-phase harness budget is6hours;
other existing phase guards stay2hours. These bound QA work, not floor latency. An exhausted budget
retains progress and records incomplete harness evidence, never a product defect merely because a
transfer is still progressing. The peer's300-second close guard still detects leaked producers.

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

**Each run is one partial release case.** Public completion within1000ms conservatively proves
its preceding ACK; slower completion alone leaves exact timing unknown. The QA companion joins the
existing request ID to the actual committed MachineProbe ID and authenticated machine/session, using
separate API handoff→API ACK and gateway handoff→gateway ACK duration pairs. Both must satisfy1000ms
on measured steady loaded probes. No timestamp is subtracted across process clock domains.

The existing intentional three-second100%-loss fault has bounded generation markers before and after
each probe. Fault-spanning calls remain required to complete with actual correlation, progress and
ordered bytes, but do not count as steady1%-loss latency successes or failures. Every positive phase
still needs a steady loaded probe. Floor75/160kbit has no ACK or PTY latency promise. No numeric remote
PTY SLA, RSS limit or new queue capacity is introduced.

The QA-only output mount carries ten finite checkpoint requests: idle start, four phase start/drain
pairs, and a pre-stop open drain. Immutable acknowledgements must come from the same actual process
instances and executable hashes. Positive drain requires zero queued/inflight bytes and unknown
sizes, exact selected=sent+processed conservation, and no overflow, inconsistency, failed enqueue,
incomplete selection, receiver-close discard or closed receiver. Logical peaks remain measurements,
not allocator totals or a mathematical bound on an unbounded queue. Shutdown records remain separate.

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

Evidence survives stack cleanup under `.tmp/deploy-smoke/evidence/<run>/linkload`: exact candidate
manifest and connector hash, image identities, qdisc configuration and counters, both ends' counts
and hashes, per-MiB progress checkpoints, mixed observations, namespace/counter attribution,
resource samples, network-cut times, logs and every phase's exit. `ORCH_SMOKE_KEEP=1` retains the fixture
for diagnosis. Credentials and TLS private keys stay outside the evidence directory.

Choose a fresh run identity: startup refuses existing scenario files or evidence. Before unpacking
the stack, the shared smoke helper writes `cleanup-files.sh` and `cleanup-files.json` in the evidence
directory. The shell file contains one literal absolute `rm -r --` argument, without force. Both
work and evidence paths reject symlink ancestors before use. Its path, content, and owned
directory identity are checked again immediately before removal. No variable or command substitution
is passed to `rm` on this runner's stack path. The other release-building scenarios are not invoked
by this packaged-candidate runner.

Cleanup reaps the invocation's model, monitor and outstanding probe, verifies labels before removing
extra containers, then requires successful Compose teardown and absence of the exact project's
containers, networks and volumes. A failure retains the fixture files for explicit recovery and
returns nonzero. `cleanup.json` and `runner.json` are written after cleanup; `final_exit` includes its
failures. `ORCH_SMOKE_KEEP=1` records retained resources and cannot qualify release-proof mode.
Any nonzero final exit clears qualification. Failed setup before an ownership receipt never
overwrites previous evidence; failure after preparation reports retained/incomplete cleanup.
The run-owned netem tag is labelled and its built image ID is recorded. After all owned containers
are absent, cleanup rechecks that ID and label and refuses a tag that was replaced or is still in
use. It removes only that tag with nonforced `docker image rm --no-prune`; any error retains fixture
files and fails cleanup. Normal candidate images, shared parent layers and cached releases remain
protected. The empty run parent remains; no global prune occurs.

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
The default-off dedicated observer controls provide the exact real-TLS ACK and
selected queue hooks for a separately reviewed test invocation. Their loopback receipt always
remains partial; the hooks alone do not qualify the full mixed runtime path.
The pure tests cover protocol/evidence rejection only and do not execute sockets, kernels,
containers or any released Orch program. The primary connector common and floor phases each
retain one100-MiB duplex stream; the other legs use1-MiB placement probes. The floor's six-hour
harness budget preserves incomplete progress without creating a product latency requirement.
