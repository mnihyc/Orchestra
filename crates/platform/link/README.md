# orch-link

Shared reliable link mechanisms; persistence of product state belongs to the API.

Implemented. The package keeps a machine's events, commands and tunnels whole
across a session that breaks and a system that restarts: durable event streams
over a spool on disk, a command server with a journal that answers a repeated
command with its first result, tunnels with credit, replay and resumption,
content moved in resumable pieces, and the session, liveness and reconnect
mechanics under all of them. Two handles put it together, one for the machine
and one for the system's end of a machine's link. It is tested on an in-memory
hub that drops, repeats, reorders, cuts and shapes frames while the code under
test runs unchanged, on lines as thin as a tenth of the recommended one, and by
handshakes over real TLS on a loopback socket for each trust path.

Diagnostic queue transfers survive a restart at any point in the copy: their durable consumption
record prevents acknowledged frames being queued again. Tests reopen real spool files after partial
transfers and check that every unacknowledged frame retains its original number.
Raw capture numbers are allocated before append and retained in separate metadata outside the disposable spool directory.
Tests distinguish them from delivery numbers and cover allocator floors, expiration and overflow.
After loss of delivery counters, the server's cursor also advances the durable spool before queued
events resume. Real-file tests preserve captured numbers and times across that recovery and restart.

Browser and terminal tunnels end when their session is lost and open a fresh
attachment when it returns. Their streams do not enter the link's replay buffer.

Commands normally retain their serial key until their result is durable. Only
`fs.record` may return a captured continuation: its immutable state, upload file
and retention lease are acquired under the environment key, then the server owns
publication outside that key. The same command stays in flight through upload
and terminal persistence. Duplicates acknowledge it without starting another
owner. A failed terminal write retries the first staged outcome; neither replay
nor sending can expose Done before the successful flush, and the lease stays
held through those retries. A persistent disk failure therefore leaves a pending
command and prevents graceful recording drain from finishing.

Capture cancellation signals and joins local work before releasing the key.
Publication cancellation retains its lease through terminal persistence, and a
link reconnect retains the same owner. Graceful physical shutdown cancels and
drains recordings before tearing down their roles. Restarted Started commands
retain the interrupted-command refusal; Done commands replay their stored answer.
The existing six-hour recording budget and four content-transfer slots remain.
`orch_machine_command_phase_seconds` records key wait, capture, publication and
finalization with bounded kind/phase labels; `orch_machine_record_publications`
counts publications including terminal disk writes. The recording and durable
journal unit sources cover controlled upload/flush stalls, duplicates, failure,
ordering, cancellation, panic and result-kind refusal.

A local MCP import uses a bounded request/reply exchange on the authenticated session. The
system files it as the current machine owner and sends the ordinary copy command back on that
link; neither request nor reply contains the definition or its secrets. A lost reply is reported
as unconfirmed. Pending requests have a time limit and a fixed maximum, and cancellation releases
their slot. The gateway integration test exercises both directions over real TLS and checks
filing, replay and refusals with an explicitly scripted local copier.

Ports share that carrier: ordered TCP bytes and half-close have bounded credit,
UDP has whole bounded datagrams with transient drop counters, and live records
keep the signed outside end and the actual inside end. Current control loss
closes port channels; saved admissions belong to the server. Native connector
cases cover the slow profiles, saturation and socket limits.

| Module | Planned responsibility |
|--------|------------------------|
| [ports](src/ports.rs) | Bounded port datagram storage and transient records of both connection ends. |
| [session](src/session.rs) | Authenticated session handshake and liveness. |
| [framing](src/framing.rs) | Control and binary framing. |
| [websocket](src/websocket.rs) | The session over a real socket: WebSocket frames on TLS, on either end. |
| [outward](src/outward.rs) | Connections out of a machine, directly or through the HTTP proxy its environment names: a tunnel asked for with `CONNECT`, and the TLS above it the caller's own. |
| [streams](src/streams.rs) | Sequenced event delivery, replay and acknowledgement. |
| [spool](src/spool.rs) | Durable local event segments and overflow handling. |
| [commands](src/commands.rs) | Serial command execution and deadlines. |
| [journal](src/journal.rs) | Durable command deduplication and result replay. |
| [machine](src/machine.rs) | The machine's end of a link: one handle that lives as long as the machine. |
| [controller](src/controller.rs) | The system's end of one machine's link. |
| [tunnels/channel](src/tunnels/channel.rs) | Ordered channel lifecycle and local sockets. |
| [tunnels/flow](src/tunnels/flow.rs) | Credit and bounded backpressure. |
| [tunnels/replay](src/tunnels/replay.rs) | Acknowledged byte offsets and resumption. |
| [tunnels/relay](src/tunnels/relay.rs) | What a gateway does with the channels of the sessions it holds. |
| [tunnels/terminal](src/tunnels/terminal.rs) | API-terminated terminal tunnels. |
| [content](src/content.rs) | Resumable authenticated HTTPS content transfer. |
| [reconnect](src/reconnect.rs) | Endpoint selection, retry and backoff. |
