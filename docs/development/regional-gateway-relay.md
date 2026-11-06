# Regional gateways with tunnel-side attachment

Status: approved first-release implementation contract; implementation is underway, with compilation and qualification performed centrally.
Source reviewed: `14fe63d0f6be49d73109d8c2cbdd5fb88826c9c7`.

Keep the harness beside its tools whenever the permitted arrangement supports it.
Otherwise let both ends attach directly to one designated tunnel gateway. Each
connector has one primary gateway for control and people's attachments, selected
explicitly or by measured round trip, and can have tunnel-only attachments to
other gateways. Gateways do not connect to one another. The API remains the
authority and control plane for machine-to-machine tunnels.

## 1. Today's path

`orch-connector` and `orch-node` each maintain one active machine WebSocket.
The connector supplies TLS/proxy configuration in
`crates/runtime/connector/src/session.rs`; the shared supervisor in
`crates/platform/link/src/machine.rs` holds and reconnects it. Endpoint choice
in `crates/platform/link/src/reconnect.rs` is sequential: the last endpoint
whose machine handshake succeeded first, then the enrollment/configuration
order. There is no latency ranking and no simultaneous active attachment.

An explicit connector endpoint list takes precedence over the authenticated
system list. Otherwise enrollment supplies the initial list and `ready.endpoints`
can refresh it. A node can separately advertise addresses for its hosted units.
The existing endpoint contract has only `url` and `trust`; neither a gateway
identifier nor a region is present. Last-success preference is supervisor memory;
the separately saved verified connection address also serves update/diagnostic
work and does not implement latency selection.

The gateway's current backplane is one outgoing mutual-TLS WebSocket to
`wss://api:8444/internal/backplane`. It is a gateway-to-API connection, with JSON
control and binary frames for tunnels ending at the API. It is neither a message
broker nor a gateway mesh. See `crates/control/gateway/src/backplane.rs` and
`crates/contracts/link/src/backplane.rs`.

| Traffic | Current path | API carries payload? |
| --- | --- | --- |
| Commands, acknowledgements, durable events, ephemeral facts, enrollment decisions | Machine ↔ gateway ↔ API; durable events commit in the API/database before acknowledgement | Yes |
| Remote executor and environment MCP tools | Harness loopback ↔ source connector ↔ gateway ↔ destination connector ↔ executor/MCP | No |
| Tools attached to the harness's own connector | Harness loopback ↔ local executor/MCP; normal session/environment checks still apply | No |
| Orch's own MCP tools, including shared documents and messaging | Harness loopback → `orch.tool_call` session event → gateway → API; result returns as an `orch.tool_result` command | Yes |
| Interactive terminal, browser and display | Browser ↔ API ↔ gateway ↔ connector ↔ PTY/browser/RFB endpoint | Yes |
| Through-Orch HTTP/raw port use and helper | Caller/helper ↔ API ↔ gateway ↔ environment connector ↔ owned service socket | Yes |
| Host-published port | Caller ↔ publishing node ↔ gateway ↔ environment connector ↔ owned service socket | No for its byte stream; admission/control still uses the API |
| Forwarded model call with the key on another machine | Harness loopback ↔ source connector ↔ `llm` tunnel via gateway ↔ provider connector ↔ upstream | No |
| Forwarded model call beside its key, or native/shared/handed-over login | Harness → local forwarder → upstream, or harness → upstream | No; usage/status reports still travel to the API |
| Blobs, installation downloads, enrollment and renewal HTTPS | Machine ↔ gateway HTTPS proxy ↔ API's internal HTTP handlers/storage | Yes; this is beside the WebSocket, not tunnel data |

The tool split is implemented by `tool_endpoints.rs` and `tool_tunnels.rs` in
`crates/runtime/connector/src/`. The same-machine shortcut already exists.
`forwarder.rs` implements the local and remote model paths. The data plane sees
actual connector processes, including a managed unit's connector; a physical
host's `orch-node` is a distinct enrolled machine.

### Current routing and recovery limits

`crates/platform/link/src/tunnels/relay.rs` owns a local `sessions` map and
translates source/destination channel numbers. Its `start` looks up the
destination in that map and returns `peer_offline` when absent. Adding a second
gateway and advertising its address alone therefore does not enable tools across
the two gateways.

The API already has a central in-memory machine route: `SessionRoute` in
`crates/control/api/src/backplane.rs` holds the gateway connection, session ID,
fingerprint and IP. It routes commands and API-owned tunnels. Machine presence
and session identity are projected into the database, but there is no durable
gateway-routing table to query as a ready-made distributed directory. Each
gateway's `Sessions` map enforces replacement locally; the API's route changes
do not constitute a fenced peer-routing protocol.

Machine-to-machine resume currently depends on the old gateway retaining its
`by_cid` route. An endpoint keeps unacknowledged bytes and local sockets for the
60-second grace, but a new gateway cannot resolve that channel identifier from
the ticket alone. A regional design must handle this explicitly. In particular,
a destination changing gateways must wake a still-connected opener to resume;
its own machine WebSocket did not necessarily drop. These are future-design
prerequisites, not a claim that a newly executed defect test failed.

The API signs tickets; gateways verify their signature, expiry and presenting
machine. The ticket fixes destination, service, thread and tool set, with
environment identity/generation and executor build for tool tunnels. Destination
tool handlers recheck the admitted environment and executor. The relay also
checks the machine's current session before accepting its frames and binds resume
to the original ticket and channel. Regional routing must carry these facts
unchanged. It must also retain the separate HTTP/WebSocket caller-origin checks
for API-owned interactive attachments.

Existing local sockets and replay storage survive an ordinary machine-link
interruption within the grace. PTY/browser/RFB attachments are opened anew;
their underlying shell/browser lifecycle is separate. Port authority loss closes
port uses and host admission immediately. UDP is never acknowledged or replayed.

When its API backplane drops, a gateway refuses new machine admission, leaves
durable event acknowledgement to reconnect, and continues existing ordinary
machine tunnels. Ports lose current control and close. Blobs and API-owned
attachments remain dependent on the API. Regional relay must preserve these
different failure rules.

## 2. Tunnel-side attachment

### Two choices with different jobs

The installer chooses the connector's **primary gateway**. A hand-picked gateway
is always honoured: its addresses may fail over between that gateway's pinned
and public entrances, but the connector never substitutes another gateway.
If it is unavailable, primary control stays offline until it returns or the
operator changes the choice. Automatic measures the eligible gateways at
enrollment and each primary reconnect and chooses the lowest measured round trip.
It does not move a healthy primary session in the background.

The API chooses a **tunnel gateway for each pair** when preparing its tool ticket.
The default is the executor connector's primary gateway. The other connector
opens a tunnel-only attachment there. This applies even when both primaries were
hand-picked and differ. Selection of an auxiliary attachment never edits either
primary choice. People's terminal/browser/display and through-Orch port traffic
to a machine continues through that machine's primary gateway and the API.

### System advertisement and installer

Add an operator-maintained gateway catalogue to the existing configuration. One
logical gateway has a stable ID, display label, region, supported attachment
capabilities and one or more existing `Endpoint { url, trust }` entrances.
Several URLs for the same gateway are aliases, not separate choices. A region
is descriptive text, never an IP-geolocation rule or an authority boundary.
Validate unique IDs and prevent one entrance being assigned to two gateways. The
new catalogue reserves the lowercase ID `automatic` for the measured selection
flag; legacy gateway IDs are retained unchanged.

First-release configuration (source implemented; central qualification is still required):

```toml
[[gateway.instances]]
id = "eu"
label = "Europe"
region = "Europe"
endpoints = [{ url = "wss://eu.orch.example:9443", trust = "pinned" }]

[[gateway.instances]]
id = "asia"
label = "Asia tools"
region = "Asia"
endpoints = [{ url = "wss://asia.orch.example:9443", trust = "pinned" }]

[link]
tunnel_session_idle_secs = 60
```

The API owns this catalogue; each gateway still has its existing local
`gateway_id`, listener addresses and `backplane_url`. Keep the flattened
`gateway.endpoints` projection for older clients. A legacy single-gateway
configuration synthesizes one catalogue entry. Update authenticated enrollment
and `ready` replies to carry the same catalogue, alongside the old
endpoint list. Store advertised metadata separately from local overrides, as
`EnrollmentRecord.server_endpoints` already does for addresses.

Authenticated `GET /api/v1/gateways` returns the catalogue and an opaque content
revision. `PUT /api/v1/gateways` is a full ordered replacement under ordinary
system administration and the existing scoped Manage operation; its response
contains only the new revision. A stale expected revision returns a content-free
409. Persist the operator override in the existing system settings table, load it
before enrollment/backplanes start, and publish catalogue/legacy endpoint metadata
together to primary sessions without requesting a reconnect. Configuration remains
the initial source. Anonymous bootstrap discovery has no admin write capability
or revision requirement. The API freezes its own catalogue revision beside each
tool ticket. No migration, service deployment or new routing directory follows
from a catalogue edit.

The screens read the connector's last reported local choice, dated entrance
samples and authoritative primary presence; they cannot remotely mutate its
selection. The thread's tool carrier is an actual source-bound report for the
current source/destination/environment generation, with an issued plan shown as
pending and a missing report shown as unknown. Model forwarding does not overwrite
that tool display. Refusals use server catalogue labels and structured failed
sides; they never echo raw ticket or transport credentials. The shared HTTP declarations provide the wire JSON and generated client names.

Offer a read-only `/link/v1/gateways` discovery response through the normal
verified enrollment entrance, before consuming a token. It contains only the
operator-advertised installation metadata and public trust material, no machine
presence, account data or secrets. Use `HttpsEnroll`'s existing pinned/public
verification and enrollment-token CA-pin check. Repeat the catalogue in the
verified enrollment response; do not enroll independently at each candidate.

The POSIX and PowerShell installers pass the choice to the downloaded connector
using `--gateway automatic|<gateway-id>` and `-Gateway` flags. Keep
probing, validation and persistence in Rust, shared with direct enrollment and
the local console. The scripts keep their existing verified download and
per-user service work. An interactive selector shows, for example:

```text
Gateway for this machine
  Automatic        Lowest measured round trip; measured again on reconnect
  Europe           Europe       94 ms
  Asia tools       Asia        188 ms
```

The measurements must come from the machine being enrolled, not the browser
displaying its installation command. The web Add machine/Host form can choose an
ID or Automatic and put the flag in the command; the downloaded connector shows
and measures the actual target's choices. POSIX piped installation prompts through
the controlling terminal, not the script's stdin. Without a terminal an explicit
flag wins; otherwise a fresh install visibly selects Automatic. Re-enrollment
without a flag retains the existing choice. Starting a service never prompts.
The console offers the same selector and preserves a pasted command's choice.

Persist the choice in the existing local `config.toml`, for example:

```toml
[connector.gateway]
selection = "automatic"
# For a hand-picked gateway instead:
# selection = "selected"
# gateway_id = "asia"
```

Keep the selected ID when labels, region text or endpoint aliases change. A
removed hand-picked gateway is visibly unavailable; never silently reset it to
Automatic. Reject an unknown explicit ID before spending the enrollment token.

Existing local `[[endpoints]]` remains an explicit primary candidate override and
is never overwritten by discovery. Map its verified origins to catalogue entries;
Automatic measures only that primary set and a selected ID must resolve within
it when it is present. Legacy unlabelled overrides remain usable, with their
address shown and their original trust verification. The primary override is
not a newly invented machine-wide egress policy: tunnel-only targets come from
the trusted system catalogue. Actual local/network restrictions still apply to
every attachment and narrow the common reachable set. Enrollment defaults, the
authenticated system list and a node's hosted-unit list retain their distinct
ownership. Restricted runtimes receive the exact trusted gateway/probe authorities
through their existing network policy, not through a new general egress exception.

For Automatic, probe all eligible entrances without starting machine sessions.
Add a small gateway-local HTTPS probe using the same trust path; it answers a
nonce and gateway ID without contacting the API. Public-edge probes also prove
the gateway's Orch certificate/key against the token pin or installed CA, so a
successful unrelated public HTTPS endpoint is not a candidate. Complete TLS,
then measure three request/reply intervals and use their median, which tolerates
one scheduling outlier. A gateway's score is its best verified entrance's score;
choose the lowest score, with exact ties settled by last success then declared
order. No improvement threshold overrides the minimum. Retain the existing
10-second connect/probe bound and use at most four concurrent probes to bound
sockets while covering a small regional list quickly. These are the first-release defaults, not measured acceptance thresholds. Probe failure is not a machine
admission failure; the chosen entrance must still complete ordinary authentication.

### Pairing, reachability and the ticket

The API already knows each primary's gateway through `SessionRoute`. It needs no
gateway-to-gateway routing table. Keep a small reachability/RTT vector per connector
from enrollment/reconnect probes, limited to the advertised set. Reports are
placement hints, not authority and not proof that a future dial will succeed.

For source S and tool destination D:

1. Keep the same-machine tool shortcut first; it needs no tunnel.
2. Prefer D's primary gateway when both can reach and use it. For an `llm`
   tunnel D is the credential keeper, so its gateway is the equivalent default.
3. Lowest-combined-RTT pairing is deferred. The first release uses the executor's
   primary gateway and the common reachable fallback below. Moving a healthy
   primary is also deferred; Automatic remeasures only when reconnecting.
4. If attachment fails, try a gateway both can reach: S's primary, then the
   remaining common candidates ordered by combined measured RTT. Recheck actual
   attachment readiness. If no common gateway works, return a specific safe
   `tunnel.gateway_unreachable` reason naming the attempted gateway labels and
   which end could not attach. Preserve the work/input for the normal recovery
   action; do not silently change either primary or the credential arrangement.

API preparation supplies the same frozen plan to both ends over their primaries:
`{ plan_id, src, dst, preferred_gateway, common_fallbacks, catalogue_revision }`.
This is one idempotent `tunnel.prepare` control operation, associated with the
existing thread/segment or lease, before returning a usable route to the source.
It does not start a second harness/executor. Existing native authority, source,
generation and build admission run at their current boundaries. No database
transaction spans probes, preparation or remote attachment waits.

Put the route **beside** the signed ticket: extend `TicketSlot` with a negotiated
routed form `{ ticket, route: { plan_id, gateway_id, endpoints, fallbacks } }`.
`TicketSlot::Signed(String)` remains the legacy representation. The dispatcher
resolves a stored ticket request and its route together, and the connector keeps
both in `loopback::Tickets`. The endpoint list comes only from trusted system
configuration within actual local/network restrictions, never from a model or a
tool's request body.
At open, the connector checks that the authenticated gateway matches the hint.

Keep the signed ticket format and all its claims unchanged. The route hint
chooses a carrier; it grants no right. A gateway still requires the ticket's
authenticated presenter and a local destination, then performs the existing
ticket/expiry/channel checks. A different address or gateway cannot turn a ticket
into access to another machine, service, environment generation or executor.
An already open channel is never migrated to a different gateway because of
a later RTT sample or primary choice. If a primary reconnect changes gateways,
or closes an auxiliary before attaching as primary there, the affected channels
reset under ordinary interrupted-work recovery; no channel moves between pool
entries in this release.

Each fallback is designated in that same plan. Both ends retain the order while
the work uses it, so they can rendezvous after the chosen gateway fails without
turning a hand-picked primary into Automatic. An API-directed updated plan may
replace unused candidates while control is available. A cached plan does not
override fresh machine authentication, ticket validity, local restrictions or
the original 60-second resume deadline.

### Connector sessions and control separation

Refactor the shared transport/session supervisor, rather than starting several
complete `MachineLink` instances on one data directory. One `MachineLink` retains
one spool, journal, command dispatcher, primary session and content token. It
owns a pool keyed by gateway ID, sharing an existing primary transport whenever
the tunnel is designated there. Each other gateway gets at most one tunnel-only
transport, reused across all relevant threads and leases. Several channels never
imply several sessions to the same gateway.

The auxiliary handshake must explicitly negotiate `purpose = tunnel`, using a
new recognized link capability/subprotocol before authentication. An old gateway
must refuse it rather than accidentally accepting a primary machine session.
The client checks that capability before sending an auxiliary authentication;
unknown optional JSON members alone are insufficient negotiation.
Both purposes keep the same certificate/signed machine-proof verification and
API `auth_query` active-machine/fingerprint check. A purpose-specific signed proof
binds the negotiated purpose as well as the existing nonce, machine, host and
time; the primary proof remains compatible with the existing protocol.
Extract the shared identity validation, but keep auxiliary admission out of the
primary's pending-fingerprint, roles/facts, cursor and presence update paths.

An auxiliary `ready` gives tunnel limits and a session ID, but no command cursors
or content bearer token. It carries only tunnel open/resume/data/credit/ack/close
and liveness frames. It cannot submit durable events, commands/results, policy,
enrollment or blob requests. The gateway authenticates this session but does not
send ordinary `machine_up`/`machine_down` for it. It therefore never replaces the
API's primary `SessionRoute`, presence, source binding or command target. Auxiliary
reports must be separately tagged and cannot make an offline primary look online.
Do not feed auxiliary endpoint refreshes, connectivity watches or `port_control`
messages into primary-only accessors. Primary loss keeps its existing global
port/network consequences. If a host-port tunnel uses an auxiliary, that carrier
also enforces its own gateway's current control and closes its affected use on
loss, without withdrawing unrelated openings or changing the primary's state.

Keep one gateway-local relay attachment per machine per gateway. The gateway's
session entry records its purpose. At primary reconnect, close any auxiliary on
the chosen gateway before attaching the primary there. The first release does
not implement an in-place purpose transition or move a healthy primary. A
selected primary changes only by operator choice; Automatic selects at reconnect.
There is still exactly one spool reader and command dispatcher.

Incoming tunnels from all carriers feed the same registered `TunnelServer`
handlers. Channel numbers are scoped to their gateway session; a shared channel
budget and byte gauges remain connector-wide. Keep the current 256-channel
default as the aggregate connector budget as well as each gateway's local cap,
so five attachments do not silently multiply a connector's admitted buffers.
Machine revocation/re-enrollment/role withdrawal reaches auxiliary sessions too:
broadcast the existing kick to all live gateways, at most five, and recheck the
normal destination/lease/lifecycle gates. No auxiliary-session routing directory
is needed for this broadcast.

A live thread/session, lease, host opening or channel holds a pool reference.
After the last reference is gone, close an unused auxiliary attachment after
`tunnel_session_idle_secs` (default 60; zero adds no idle delay).
Using the existing minute-scale recovery interval avoids another arbitrary
operational time scale. This idle clock never closes an active stream or a
thread still using its tools. Boot/reconnect restores needed attachments from
the existing session/lease state; it does not start or replay work to populate
the pool.

### Three paths and recovery

```text
Both primaries on G:
  S --primary--> G <--primary-- D       tool bytes: S ↔ G ↔ D

Different primaries, destination primary is G-Asia:
  S --primary--> G-US --> API <-- G-Asia <--primary-- D   control
  S --tunnel-only----------> G-Asia <--primary-- D       tool bytes

For the next open after G-Asia fails, common fallback G-Europe:
  S --tunnel-only--> G-Europe <--tunnel-only-- D         tool bytes
  primary choices stay as configured; no gateway forwards to another
```

An ordinary socket interruption reconnects to the designated gateway using the
existing backoff/trust machinery. If its relay process and channel map survive,
the current opener-led `resume` and byte offsets work unchanged within 60 seconds.
Loss of an auxiliary does not disconnect the primary or another auxiliary.

A gateway process loss destroys its `by_cid` map. In the first release the
affected tunnel resets as it does today, and the existing interrupted-work
recovery applies. The next open uses the prepared common fallback order while
leaving both primary choices intact. Never replay a non-idempotent tool to hide
that reset. Cross-gateway resume, retained claims offered to a replacement
gateway, `tunnel_suspended` and moving channels between pool entries are deferred.
Gateway authentication still needs a working API backplane; cached plans do not
admit a new attachment offline. Same-gateway link recovery retains the existing
60-second grace and byte-window checks.

PTY/browser/RFB attachments retain their existing fresh-attachment behavior;
PortUse loss/currentness and UDP no-replay rules also stay in force. There is no
promise of transparent new admission during an API outage, nor permission to
move a hand-picked primary because its gateway failed.

### Why the other paths are unnecessary

Pairwise gateway relay would add a second payload-forwarding gateway, peer TLS,
distributed session discovery, peer channel mapping and another failure seam to
a job served by one extra client attachment; five gateways could need ten peer
links. Direct attachment reaches the same chosen gateway without that extra
forwarding hop. Relaying machine-to-machine bytes through the API would put the
singleton on the data path, consume its bandwidth/queues and route US–Asia work
back through Europe. Neither alternative improves the chosen one-gateway path.

## 3. Cost, API availability and implementation

### Up to five gateways

For N gateways a connector has one primary and at most N−1 auxiliary sessions:
at N = 5, **five steady live sessions total**, not five per thread. Primary reuse
avoids a duplicate attachment to its gateway. Serialize replacement so at most
one ready session per machine/gateway exists; a bounded handshake/closing
transition is not another permanent pool slot. No gateway-to-gateway links exist.
The system has at most five existing-style gateway-to-API backplanes.

For M connectors, the worst case is 5M machine attachments across the gateways,
versus M today; most executors need only their primary and many harnesses need
one additional regional attachment. Gateway session/channel maps remain local.
There is **no new database or replicated session-routing table**. The API retains
its existing O(M) primary `SessionRoute` registry plus at most five placement
samples per connector, O(5M), and prepared plans in the existing session/lease
lifecycle. It does not distribute connector locations to gateways. With 10,000
connectors that is 10,000 primary routes and at most 50,000 RTT samples, not a
50,000-by-50,000 mesh. Exact HashMap/TLS RSS depends on implementation and must
be measured; a fabricated per-entry byte count would not be source evidence.

A tunnel still traverses **one** gateway. Per payload byte that gateway performs
one ingress TLS decrypt and one egress TLS encrypt, parses the existing binary
header and translates the channel number, just as today. Ticket signature
verification is at open/resume, not on every data frame. No additional bulk
TLS pass, peer header, copying gateway or per-byte API operation is introduced.
Additional cost is TLS/WebSocket state, authentication and heartbeats for unused
auxiliary connections, plus probing at primary reconnect. Four idle auxiliaries
produce roughly 0.4–0.8 small ping/pong frames per second at the existing
20-second heartbeat, depending on whether both sides initiate idle pings.
This is a frame-count estimate, not a measured CPU/RSS percentage.

Source-backed memory/flow constants are in
`crates/contracts/link/src/tunnels.rs`, `version.rs` and the channel state machine:

| Resource | Existing bound and consequence |
| --- | --- |
| Negotiated data/control frame | 262,144 bytes by default; tunnel data header is 13 bytes. The transport's pre-negotiation WebSocket message guard is separately 4 MiB. Neither is a per-idle-session allocation promise. |
| Ordinary receive credit | 1 MiB per direction. End-to-end in-flight payload across the gateway stays within credit; queues do not each earn an extra independent window. |
| Endpoint sent-byte replay | Up to 8 MiB per direction; stop reading the local producer at the bound. An empty auxiliary allocates no channel replay payload. |
| Endpoint receive storage | Up to one window normally and two windows during resume. A duplex pair's conservative payload maxima are 16 MiB of sent replay plus 2 MiB of receive windows normally, or 20 MiB including doubled resume windows. These are separate retained copies, not 20 MiB necessarily allocated for every channel. |
| Gateway payload queue | At most the applicable end-to-end outstanding windows, normally up to 2 MiB total for both directions of one ordinary tunnel, with resume overlap accounted separately. There is no durable gateway replay store. |
| Channels | Current default 256; preserve a shared connector admission budget across the pool. Memory grows with admitted channels and retained bytes, not N copies of every channel. At 256 fully populated duplex ordinary channels, the pair's conservative steady payload bound is 4.5 GiB; this is an extreme bound, not an idle cost. |
| TCP ports | 256 KiB per direction rather than ordinary 8 MiB replay; 64 live and 16 waiting clients per environment. The existing two-direction 64-live payload calculation is 32 MiB for one carrier's per-direction stores. UDP has one 256 KiB total waiting store, up to 128 associations and 4,000-byte datagrams. |

Some current control queues use `mpsc::unbounded_channel`; existing credit bounds
payload, not every possible control-message allocation or aggregate RSS. Do not
claim otherwise. The pool must share the existing prioritization and admission
budgets, retain no duplicate replay copy during migration, and instrument all
queues during qualification. Control goes before the next adaptive data frame:
payload sizing targets a quarter second of the observed link, with a 4 KiB floor.
At 750 kbit/s the source arithmetic yields about 23.4 KiB per data frame.
The 100 MiB fixture therefore involves about 4,474 such payload frames per
direction, with unchanged framing/TLS work at its single gateway. CPU time and
maximum throughput need a benchmark on the deployment's CPUs and routes.

For a one-way payload B, the two machine-to-gateway legs carry approximately
**2B** of link payload in total, excluding protocol overhead, under both today's
single-gateway path and tunnel-side attachment. The gateway receives B and sends
B. A 100 MiB transfer still means about 100 MiB in and 100 MiB out at the one
chosen gateway; a 100 MiB transfer in each direction doubles those figures.
Five gateways do not replicate that stream five times. Actual egress charges
depend on the operator's hosting and peering contracts.

With the gateway near the Asia executor, the US harness sends B over the
US–Asia leg **once**, then B over a short Asia-local leg. Reply bytes take the
reverse route once. With only a European gateway, B instead crosses US–Europe
and Europe–Asia. A pairwise two-gateway relay would carry about 3B over three
legs and process the stream at two gateways, adding work that direct attachment
avoids. Being near the executor changes which long path is paid; it does not
compress traffic or remove the remaining intercontinental distance.

### What the API keeps and what crosses Europe

The code confirms that **machine-to-machine** exec/MCP/model/host-port tunnel
bytes bypass the API today: gateway `routing.rs` hands them to the local relay,
whose destination is another local session. The auxiliary sessions
feed that same relay, so this remains true. There is no API fallback byte relay.
The API's `TerminalHub` and `ApiBridge` are a deliberate separate path: people's
terminal, browser, display and through-Orch port tunnels do carry bytes through
the API today and continue doing so, through each machine's primary gateway.

The API keeps the existing primary session-to-gateway registry, machine validity,
durable cursors/commands/business state and ticket signer. It gains only the
catalogue, small reachability samples and prepared pairing hints. It issues
tickets as now; gateways get only the verification key. Stored command bodies
contain ticket requests resolved at delivery, not a new credential store.
`DEFAULT_TICKET_SECS` is 43,200 seconds; interactive tickets last 60 seconds.
These are issuance defaults, not a guarantee that every ticket lives that long.

Control still crosses to Europe: machine authentication for every new primary or
auxiliary, commands/results, durable events/acks, usage, facts, pairing preparation
or revision, revocation and gateway statistics every 15 seconds. Orch's own tool
calls and blob HTTPS also keep their API path. A warm executor/MCP/model channel
neither looks up a PostgreSQL route nor asks Europe to forward each data frame.
An additional auxiliary authentication is a cold-attachment cost, amortized over
threads/channels by the pool; the existing `auth_query` has a five-second limit.

| Failure | First-release behavior |
| --- | --- |
| API or its backplane down, gateways/attachments alive | Existing ordinary machine-to-machine tunnel bytes continue. Events remain unacknowledged in the primary spool; control-dependent work may wait. A still-valid previously issued ticket may open another ordinary channel between already admitted local attachments, as today's relay permits. No new ticket or pairing decision can be issued by the unavailable API. |
| API down and a new primary/auxiliary is needed | Gateway authentication cannot confirm machine validity, so it returns `busy`. A cached plan/ticket is not a substitute for admission. Reconnect waits under the existing backoff; manual primary remains fixed. Transparent recovery is possible only if admission returns within the existing grace. |
| API restarts | Gateways reannounce primary sessions and reload control. The current ticket signer is generated at API startup; old ticket verification can therefore change after reconnect. Already open channels are not reauthorized per byte. Resume/new open must still pass the current signature/expiry checks; obtain a current ticket through normal guarded preparation or reset if unavailable. No new signing-key persistence is assumed. |
| One gateway's process dies | Its local attachments and route map disappear; other gateways and their tunnels continue. Affected tunnels reset; interrupted-work recovery applies and the next open uses the prepared common fallback. Manual primaries on the failed gateway remain offline; Automatic re-measures on primary reconnect. |
| Auxiliary alone drops | Its channels reconnect/resume within grace. Primary control, people's attachments on the primary and other auxiliaries remain independent. |
| Primary alone drops | Events spool, commands wait, and people's API-owned attachments need a fresh attach. Existing ordinary channels through surviving auxiliaries can continue; an auxiliary never takes over the primary's command/presence role. |
| Port authority or restricted-network control lost | Preserve existing fail-closed port admission/channels and host-proxy egress rules, even though an ordinary byte carrier may remain open. UDP is dropped, not replayed. A terminal's underlying shell and a durable port opening are not deleted. |

The minute-long grace and 8 MiB replay bound are recovery limits, not a promise
that a lengthy API/gateway outage can be hidden. Keeping an already open carrier
does not promise that an entire agent turn can finish without Orch tools, fresh
leases, model access, network policy or other API-controlled business work.

### Files, shared callers and protocol changes

Implement through the shared transport and native admission paths. No schema
migration is required for this design; configuration and existing command/session
JSON carry the added metadata.

| Area | Files/crates and all principal shared callers |
| --- | --- |
| Catalogue, selection and idle settings | `crates/platform/config/src/{server,connector}.rs`; server/gateway/API configuration, standalone connector, node and hosted runtime bootstrap all consume these settings. |
| Enrollment/handshake contracts | `crates/contracts/link/src/{enrollment,session,backplane,frames,commands}.rs`; connector, node, gateway, API, dispatcher, engine and testkit must understand catalogue, purpose and routed tickets. Preserve the existing ticket claims in `tunnels.rs`. |
| Pool and resume mechanism | `crates/platform/link/src/{reconnect,machine,session}.rs`, `tunnels/{channel,relay}.rs`; connector and node clients, gateway relay, API-owned tunnel path and fault-injecting testkit are the complete mechanism-level callers. Extract the common transport supervisor rather than duplicate business links. |
| Connector adapters | `crates/runtime/connector/src/{session,tunnels,loopback,tool_endpoints,tool_tunnels,forwarder,enrollment,console}.rs` and `console/enrollment.rs`; Exec/MCP/LLM, host publishing, all incoming TunnelServer handlers, content/update consumers and local console share the pool or the primary-only accessors. |
| Gateway boundary | `crates/control/gateway/src/{authentication,sessions,routing,listeners,content,backplane}.rs`; classify purpose before install, preserve local relay semantics, send primary presence only, add discovery/probe, and gate content tokens. |
| API/ticket preparation | `crates/control/api/src/{backplane,tickets,certificates,dispatch}.rs`; all primary admission, machine revocation, ticket expansion and command redelivery use the same producer. `crates/control/engine/src/{sessions,mcp}.rs` and lease/model-ticket producers freeze pairing beside native source/generation facts. Interactive ticket/origin paths remain primary-only and need regression coverage. |
| Installer/composition | `apps/connector/src/{arguments,enrollment_options,composition}.rs`; node composition/hosted endpoint flow; `deploy/install/{install.sh,install.ps1,install-node.sh}`, `deploy/{compose.yml,config/orch.toml,nginx.conf,.env.example}` and operator installation/topology documentation. |
| Installation UI/contracts | The existing Machine/Host installation form and command builder, `frontend/tests/component/machine-installation.test.tsx`, installation/enrollment contracts and generated clients. Show label/region/Automatic, keep actual target measurement in the connector and preserve role/handover consent. |
| Qualification | `crates/tooling/testkit`, link/gateway/connector integration tests, node/runtime conformance, installer smoke fixtures, genuine contract generation and relevant frontend/native suites. No handwritten generated contract edits. |

New or extended protocol units are: gateway catalogue/discovery and local probe;
explicit primary/tunnel session purpose and capability negotiation; purpose-tagged
auth replies/ready without auxiliary cursors or content token; small RTT reports;
idempotent `tunnel.prepare` and release/revision; and routed `TicketSlot`. Existing data, credit,
acknowledgement, EOF and UDP formats stay unchanged. A mixed-version deployment
admits only candidates both ends can use: an older connector can use only its
primary, while an upgraded peer can attach there. Two legacy connectors on
different gateways remain incompatible and receive an explicit reason. Do not
advertise the new mode before the gateway/API understand purpose separation.
Recovery through a different gateway is deferred for every version in this
release; same-gateway reconnect success does not certify process-loss recovery.

### Deployment and effort

Add one ordinary `orch-gateway` service/VM in the chosen region, with a unique
`gateway_id`, its existing pinned/public listeners, its own service leaf/key and
the same CA public certificate. Keep the CA private key on the API. Configure
its outbound `backplane_url` to the API's internal mTLS endpoint and its existing
content/renewal access; the gateway needs no PostgreSQL connection. Publish its
entrances, operator label and region in the system catalogue. An optional public
front must route that gateway's WebSocket and companion HTTPS to the same
instance, since session tokens and channel maps are local.

Allow connectors to reach eligible regional gateways under their actual
proxy/egress policy. Update the trusted endpoint set for managed restricted units
and node-hosted bootstrap lists through the existing acknowledged network-policy
mechanism. This adds neither inbound listeners on connectors nor gateway peer
ports. Deploy updated gateway/API contracts first, then connectors/installers,
then enable regional choices. Existing same-gateway work remains usable while
the opt-in capability rolls out. No resources are deployed by this memo.

Implementation budget: **three days**, with ordered commits for catalogue,
connector selection, installers/console, tunnel attachment, then screens/help.
The integrator performs batch compilation, first after connector selection, and
all runtime qualification centrally. The bound excludes deferred cross-gateway
resume, in-place purpose transition, lowest-combined-RTT pairing and moving a
healthy primary. Assumptions remain up to five gateways, one API authority,
existing PKI/WSS and fixtures, no new transport/NAT system or migration.

The first placement-only improvement needs configuration and existing supported
arrangements, not those implementation days. The estimate covers tunnel-side
attachment. The 100 MiB common/floor gate is still lengthy. With concurrent
directions, the 750/75 kbit/s direction alone takes roughly 18.6 minutes plus
186.4 minutes per build. Repeating that pair for normal and instrumented QA
artifacts has an ideal lower bound of about **6 hours 50 minutes**, before loss,
cut/retry, other topology legs, capacity cases or builds. The formula is
`2 * (100 MiB * 8 / 750,000 + 100 MiB * 8 / 75,000)` seconds. Serializing the
directions takes longer. This is a scheduling bound, not a test pass or a waiver
of any required workload.

### Acceptance and commands for the integrator

No build or test was run for this design. Existing baseline commands are listed
below; run them centrally from the chosen source tree with the ordinary fixture
setup. They do not qualify this unimplemented design. The WSS observer command
alone is loopback observation, not the required kernel-impaired acceptance.

```sh
cargo test -p orch-link --test resume_authority a_different_tool_ticket_cannot_take_over_an_existing_tunnel -- --exact
cargo test -p orch-gateway-core --test tunnels a_tunnel_between_two_machines_carries_bytes_both_ways_and_survives_a_reconnect -- --exact
cargo test -p orch-gateway-core --test presence while_the_system_is_away_new_sessions_are_told_to_wait_and_tunnels_and_sessions_go_on -- --exact
cargo test -p orch-connector-core --test harness_tools a_harness_whose_environment_is_on_its_own_machine_reaches_its_tools_without_a_tunnel -- --exact
cargo test -p orch-gateway-core --features test-observers --test wss_observers observed::exact_ack_and_queue_observation_on_real_tls_wss -- --exact
```

The future implementation needs these deciding tests,
with names proposed for a new gateway `tunnel_attachments` target:

| Proposed case | Required result |
| --- | --- |
| `auxiliary_does_not_replace_primary` | Two different hand-picked primaries exchange tool bytes through the executor's gateway; command/event/presence/content identity and people's attachments retain their original primaries. Auxiliary control-frame injection is refused. |
| `automatic_and_selected_gateway_choices_survive_reconnect` | Lowest median RTT wins at enrollment and each Automatic reconnect; a selected primary never changes, even when another is faster or its own gateway is unavailable; label/URL updates and process restart preserve the choice. |
| `a_common_third_gateway_is_used_or_failure_is_explicit` | Failed designated attachment selects a reachable common third gateway; no common gateway preserves held work with a safe reason, never a new primary or credential placement. |
| `five_gateways_share_sessions_and_channel_budgets` | At most five ready transports per connector, one dispatcher/spool/journal, no duplicate per-thread attachment, shared byte/channel budget and idle cleanup without active-stream closure. |
| `gateway_loss_resets_and_the_next_open_uses_fallback` | Same-gateway socket recovery preserves ordered bytes inside grace. Gateway process loss resets the original tunnel and the next open uses the planned fallback; no hidden replay of the interrupted tool occurs. |
| `api_outage_keeps_existing_tunnels_and_refuses_new_attachments` | Existing exec/MCP/model bytes continue without API bytes; new ticket/admission waits; primary controls and PortUse/restricted egress keep their existing loss behavior. Exercise ticket-key rotation after API restart. |
| `managed_runtime_pool_and_network_boundaries_hold` | Actual managed and directory connectors in both runtime pools preserve state, source/generation/build and restricted/none network boundaries while primaries and auxiliary sessions reconnect. |
| `bulk_and_primary_control_stay_bounded_on_impaired_wss` | Real packaged TLS/WSS carries ordered 100 MiB both ways while primary interactive traffic/commands remain live; exact queue, ACK and cleanup observations, with no API byte relay or mock substitution. |

Installer coverage must test labelled choices, Automatic/default/preserved choice,
explicit flags and unknown IDs, piped TTY and unattended service installation,
local endpoint overrides, CA/public/proxy verification, one-use token consumption,
EN/Chinese and POSIX/PowerShell parity. Runtime conformance must preserve both
provider/environment pools, persistent state, managed and directory connectors,
restricted/none egress, private loopback, no route to API/database/other units,
source/generation/build checks and actual executor/native behavior. Run all
ordinary link, gateway, connector, port, interactive-origin and arrangement
regressions through the shared changes.

The existing WSS gate remains 300 ms RTT and 1% packet loss, common rates
1.6 Mbit/s down / 750 kbit/s up, and the floor at one tenth, with simultaneous
bulk and interactive/control traffic, bounded queues, integrity, progress and
64 live/16 waiting TCP clients. Acknowledgement within one second is the common
profile requirement; do not invent a floor latency bound. Cover single-gateway,
different-primary/auxiliary, fallback and auxiliary-close-before-primary paths. Shape the
connector's shared access path so its primary cannot accidentally bypass the
impairment while the auxiliary alone is shaped. Attribute each leg's actual
delay/rate; do not silently turn a 300 ms path into an unlabelled 600 ms path.

## 4. The improvement available without this feature

Arrangement admission already combines adapter capabilities, provider consent,
Use-entry narrowing, `binding.allowed`/`binding.prefer`, machine policy and the
environment's ability to run the harness. Selecting another primary gateway
does not change any of them. The code's same-machine shortcut proves that tool
calls need no gateway when the harness and attached tools are there together.

| Arrangement | API-key provider | Subscription kept on a machine | Operator configuration |
| --- | --- | --- | --- |
| `separated` | Harness stays with the key keeper; tools are remote unless the places are already one machine. To co-locate, choose an admitted `key_separated` arrangement instead. | Harness stays with its subscription credential; this is the regional-tunnel case when the credential cannot leave. | Keep the provider/machine/Use-entry arrangement consent and compatible environment `exec.offers`; choose a nearby tool gateway, or move/place the environment with the existing credential machine where appropriate. |
| `key_separated` | Already runs the harness in its environment beside its own tools; key remains with the provider's forwarder. Both built-in adapters declare this arrangement for compatible API protocols. | Not available for an ordinary native subscription login; it is not an API-key forwarder. | Permit and prefer `key_separated`, keep a common forwarded protocol, and provide the actual harness build via `colocated_harnesses` or the connected machine's `agent.harnesses`. Tool WAN hops disappear; model requests still reach the key keeper. |
| `credential_shared` | Available only when the harness accepts an issued API-key form and direct protocol. The generic contract permits it, but the current built-in Codex/Claude descriptors do not advertise direct API-key credential forms. It is not their current shortcut. | Can run beside tools when the issuing/accepting form, actual adapter support and owner/machine consent permit a copy. Claude declares shared token forms; Codex does not. Verify the exact build's qualified behavior. | Explicit provider/Use/machine consent, form/lifetime disclosure, compatible harness in the environment and existing sealed issue/renew path. Never copy profiles or refresh material to force eligibility. |
| `handed_over` | No shipped shortcut for an ordinary API key: the protocol offers a shared key, not a transferred login. An extension would need truthful forms on both sides. | Contractually runs beside tools on a whole transferred login, with exclusive environment custody and return obligations. Claude's declaration alone is not current qualification; subscription handover remains deferred work, so it is not recommended as an available fix. Codex declares none. | Only use a separately supported/approved transferred form and ordinary custody/return controls; otherwise retain the supported arrangement. |
| `together` | Harness, credential and tools on the same enrolled machine already stay local; model requests use the local forwarder. | The machine already has the login and work environment; tools stay local without exporting a login. | Register provider and environment on that same machine and permit its harness/tools. This state is derived, not a selectable sixth placement switch. |

Being on one physical host in two different managed containers is not
`together`: they have separate enrolled connectors and runtime boundaries. A
managed provider runtime has no workspace/tool mount, so do not invent one to
simulate co-location. Put the harness in the environment through the arrangement
mechanism, preserving provider-runtime state, ownership, credential and isolation.
After permission/policy changes, use the existing reviewed bind/reopen procedure;
do not silently move a running harness or widen its sandbox.

### The stated Europe/US/Asia example

Use the supplied planning round trips: US–Europe 90 ms, Europe–Asia 180 ms,
US–Asia 190 ms, local-region legs below 10 ms. These are illustrative, not
measurements of a particular ISP or cloud. For one warm sequential tool
request/reply through G, the network term is
`RTT(harness,G) + RTT(G,executor)`. Do not double that sum again; each input
number is already a round trip. Actual tool execution, serialization, cold TLS
and queueing add their own time. Regional measurements commonly fall in these
broad orders of magnitude; see the [Azure regional latency methodology and
tables](https://learn.microsoft.com/en-us/azure/networking/azure-network-latency).

| Case | Primaries and chosen tool gateway | Tool network RTT | Hops and effect |
| --- | --- | --- | --- |
| One European gateway for all, today | US provider primary = Europe; Asia executor primary = Europe; tunnel = Europe | About 90 + 180 = **270 ms** | One forwarding gateway, but two long geographical legs. |
| Gateway near the executor | Executor primary = Asia; provider may keep Europe; tunnel = Asia, reached directly by provider auxiliary | About 190 + <10 = **190–200 ms** | One gateway, one US–Asia long leg. Europe's gateway is only on the provider's control path. No extra data hop. |
| Gateway near the provider | Both ends choose/use US for their primary/common tool gateway; tunnel = US | About <10 + 190 = **190–200 ms** | One gateway, one US–Asia long leg, now on the executor's access leg. If only the provider changes to US while the executor stays on Europe, the default remains Europe and the result is still about 270 ms. |
| Automatic, primaries differ | Provider primary = US, executor primary = Asia; default tunnel = Asia | About 190 + <10 = **190–200 ms** | Provider uses its Asia auxiliary directly. The US primary gateway carries no tool bytes, so there is no second gateway hop. Two hand-picked primaries in these same regions yield the same result. |
| Common third gateway is Europe | Primary choices remain US and Asia; both use Europe auxiliaries when the preferred/common regional entrances fail | About 90 + 180 = **270 ms** | Still one gateway; the Europe detour returns, about 70–80 ms more than the regional case. There is no gateway-to-gateway forwarding hop. |
| Harness beside tools in Asia | Admitted `key_separated`, supported credential sharing, or actual `together` | **0 ms of WAN tool delay**, plus local work | No tool gateway hop. Key-separated model calls still travel to the US key keeper; shared/native credentials retain their own upstream path. |

Twenty sequential warm remote tool round trips at these assumptions cost about
5.4 seconds of network wait via Europe versus 3.8–4.0 seconds through a regional
gateway, saving roughly 1.4–1.6 seconds before model/tool time. Co-location removes
that WAN tool term altogether. None of these cases removes Europe from the
approved API control/Orch-tool path or makes human interactive API tunnels local.
