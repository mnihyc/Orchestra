# orch-connector-core

`tests/provider_homes.rs` covers the shared made/existing harness-home choice, actual ownership,
retained borrowed metadata, and refusal of outside, linked-out or non-directory homes.

Managed and connected role host for agent, provider and environment services.

The environment role prepares `fs.record` under its environment key, including
durable state, the frozen reply, a verified open manifest and a retention lease.
Its dispatcher signals and joins local capture on cancellation or expiry of the
existing six-hour budget. Publication uses the remaining budget outside the key
and the same verified content transport with four slots; ordinary commands keep
their complete serial execution. The command server owns publication through
durable terminal persistence, independently of link reconnects. Connector stop
drains recordings before stopping roles. A live publication or finishing lease
refuses destructive detach/import/removal with retryable `busy`; callers retry
with new command identities within their original budget. `recording_keys` and
dispatcher unit sources exercise held uploads, subsequent ordered writes and
captures, immutable raw bytes, independent environments and joined shutdown.

Responses, Chat Completions and Messages use the same lease and credential policy. The
`chat_completions` connector regressions send complete adapter turns through a local upstream as
JSON and SSE, require unchanged tool/custom fields and replies, count native usage, and exercise
model-list isolation, model/path/closed-lease refusal and process-token authentication.

Shared-tree listings name their root once and show relative entries with recency, child counts and topic status. Archived entries have one counted line and listings continue by cursor. Search prints one entry summary with match counts and native positions, with excerpts only on request and partial counts stated as lower bounds. For either tool, a connector byte cut resumes after its last visible entry or explicitly retries the same page when no row cursor is available. Authenticated MCP-listener regressions exercise metadata, paging, byte bounds and excerpt selection; the connector's existing clock supplies relative ages.

Directory resource controls are probed under the actual running account. CPU, memory and process
ceilings are reported separately; unsupported controls and the absence of a disk quota have their
own reasons. Executors, shells, tool servers, private browsers/displays, after-start commands and
provider harnesses share the runtime's native aggregate. A process journal retains that boundary
across recovery; old processes must be detached before a new ceiling can be promised. The native
resource regressions inspect the kernel boundary and exercise a real memory refusal when supported.

Connected provider roles discover existing sign-ins through locally allowed harnesses on each authenticated connection. A real containerized Codex sign-in and transport reconnect test verifies the discovery announcement without a thread or model call.

The physical connector's `machine.roles` policy switch permits page-based role changes only when
handed to Orch (managed `true`, cautious `false`, explicit local values win). The native command
saves `[connector].roles` in the same locked configuration as policy/programs, fences new turn starts,
and refuses active turns. Its answer is journaled before the ordinary supervisor reconnects with the
same identity and rebuilt Environment/Agent/Provider handlers. Active-role facts change only after
that rebuild. Node remains the separate orch-node program. A kept switch shows the exact local
instance's `roles set …` command; the API never changes delegation or the dedicated-machine fact.

Provider runtime values are resolved separately from adapter options and override profile variables. Secret values arrive sealed to the receiving machine and remain encrypted in restart records. Sign-in changes in a person's own harness home require the live `provider.signin` policy; connector-owned homes use the adapter's ordinary methods. The variable and policy regression sources exercise these rules without a vendor account.

The agent role's transfers of a session's history (`session.export`, `session.open` with its archive, history file or skills,
and `agent.read_session`) end at the next piece, hash read or fetch when the system asks for the command not to be carried
out, and answer `conflict`; the file made for the command is removed whichever way the command ends (done, failed, given
up, or dropped by the dispatcher at the end of its allowance). The part of an upload that arrived stays with the system,
which a later upload of the same bytes goes on from. The `agent_role` tests `an_export_the_system_gave_up_...` and
`an_opening_the_system_gave_up_...` show it on a line made slow.

Partly implemented. The core and the agent role are built (a connector enrolls, holds its
session, applies its owner's limits before anything the system asks, hosts harnesses through adapters registered by
kind and reports what they do in order), and so is the provider role: it keeps a provider's key sealed to the machine,
lets it be used turn by turn under leases, and forwards the model calls of the harnesses beside it with the key added,
handing each answer back as it arrives and reading what it used; a harness whose key is on another machine reaches it
through an `llm` tunnel, and copies of a credential, or a whole login, are issued, handed over, returned and taken home
sealed from machine to machine. It is tested on the in-memory hub with the scripted controller and the scripted model
server (a turn of the scripted adapter forwarded, every refusal in the protocol's own shape, streams passed on while they
arrive, upstream errors and cut streams, a restart, two connectors with a tunnel between them, a key searched for on
disk, in the log and in every environment), with the real Codex and Claude Code in containers of the development image
(one turn each through the forwarder), and, only when asked for, with a few requests to a real model service. A sign-in is given to the harness with every value in the type of its field: a command carries each value as text, and the connector reads a boolean from `true` or `false` and a map from a JSON object, by the fields the method declares (and, for an answer, by what the sign-in last asked), so that the scripted adapter, which declares a field of every type, takes a method with all of them. Orch's own
requests leave from the provider role with the key held there: `provider.complete` for summaries and `provider.embed`
for the vectors of search by meaning (tested against the scripted model server and a stub of an embeddings server).
Not built yet: fetched builds. The hourly `update.auto` is written: it honors the owner's switch, stages a verified platform build, waits for all admitted turns to finish and fences new starts through installation/restart. Managed runtimes never take this path. Native service-manager execution remains part of the release proof.

The environment role runs on it too. Every command about an environment is carried to the environment host with
the owner's limits applied first, so that a path outside the workspace, a write the policy forbids and a shell the
policy forbids are refused whoever asks; what the host reports goes on the stream it belongs on, the status, the
facts and the manifest of an environment as events that are kept and the file changes, processes and samples as
measurements that are dropped when nobody listens; files, snapshots and moved workspaces travel through the link's
content transfers; a person's shell is taken up through a tunnel that ends at the API; and the executors and tool
servers of an environment are reached from the machine where the harness runs through tunnels of the tickets the
system signed, or directly when the environment is attached on the same machine. The tool servers found in the
harnesses' own configuration are named, one definition is copied into the connector's directory when it is
imported, run from the copy and forgotten. It is tested on the in-memory hub with the scripted controller over real
temporary directories: a directory listed and a file written through a content transfer and read back, a write, a
path and a shell the policy forbids refused with its code, a snapshot taken and materialized elsewhere, a shell
typed into, left and taken up again, a command run through Orch's executor over a tunnel with its output and its
attribution, a tool server called through the relay, killed three times and reported failed, an import found,
copied and forgotten, a harness that reaches its executor and its tool servers through its own listener, and the
link cut in the middle of a content transfer and of a shell, each going on where it stopped. A runtime the system
made looks in its tools directory when it starts: it reports the harnesses it finds there (the program, and the version
the node left beside it) among the harnesses it has, runs them from there, and lists them as the tools its environment
was made with, so that an executor that is a harness's own program (Codex's exec server) is available in it; that is
tested over temporary directories as well. Not built yet: a tool server reached by an `https` address.

The fetching of builds is for a later part; its module is as it was, and the core is shaped to take it. The
per-user service, the doctor, the update a person asks for, the console and the local socket it asks the running
connector over are built (`service`, `doctor`, `update`, `console`, `local_socket`, below):

* A **role** is a [`roles::RoleHandler`](src/roles/mod.rs): the command kinds it takes, which command it claims where
  two roles take one kind, a `handle` for each command (the dispatcher has already run the owner's limits over it),
  what it adds to the machine's facts, what it does after a restart, and how it stops. It is added with
  `ConnectorBuilder::role`, which gives it the connector's [`roles::Services`](src/roles/mod.rs) (the clock, the
  publisher of events, the policy, the listener, content moving, the keys, the tunnels, the harness processes, the
  configuration). The provider role is added with [`roles::provider::plug_in`](src/roles/provider.rs).
* An **endpoint** of the listener for harnesses is a [`loopback::Endpoint`](src/loopback.rs): what answers
  `/llm/<session key>/…`, `/exec/<session key>` or `/mcp/<session key>/<name>`, after the listener has checked the
  bearer token of the harness process and found the session. It is added with `ConnectorBuilder::endpoint`; the model
  endpoint is [`forwarder::LlmEndpoint`](src/forwarder.rs).
* A **tunnel server** is a [`tunnels::TunnelServer`](src/tunnels.rs): the link gives a machine its incoming tunnels as
  one queue, once, so the connector takes it and hands each tunnel to the server registered for its kind
  (`Services::tunnels`).
* An **adapter** is registered by kind in the [`harnesses::AdapterRegistry`](src/harnesses.rs) a program gives the
  builder; the scripted adapter is `orch-adapter-fake`, and the program registers those of Codex and Claude Code.

Orch's own tools are presented to hosted harnesses by the listener's `orch` MCP server (`/mcp/<session key>/orch`). It lists the session's declared tools and carries each call as an `orch.tool_call` from that exact admitted native segment and turn. Ordinary calls wait 50 seconds; `open_port` waits 70 seconds to include its human decision. A timeout of a changing call reports that its effect may have happened and asks the agent to inspect the target before repeating it.

The listener captures its actual source before reading request bytes and rechecks it before publication; a delayed body cannot become the next turn's call. A pending call's reference is retained on disk before publication. A retry in the same segment, turn and native conversation keeps that reference and any late answer within the existing ten-minute replay interval. Identical arguments from another turn or lane create a separate call. Older retained entries without a native source refuse matching work until their original bound expires; they are never assigned to the currently running turn. Listener-generated references route replies only to their own waiters, including late replies, without falling through to a replacement adapter. Ordinary completed calls may be intentionally repeated. The real HTTP listener cases in `tests/orch_server.rs` cover source separation, retained answers and revoked listener keys; the real-harness cases in `tests/orch_server_harnesses.rs` cover the native MCP presentation.

The environment role also discovers browser support and serves browser lifecycle commands and a live browser tunnel. It applies the machine's browser switch to commands and input, passes pictures under transport back-pressure, and acknowledges the browser only after a complete frame was handed on. Browser integration tests use the shipped browser and Playwright command line.

The environment role also carries admitted owned TCP and UDP ports over its
existing link. Current attachment, generation, socket ownership and the local
ports switch are checked before carrying and during use. Real native services
prove replacement, half-close, empty datagrams, capacity, control loss and both
slow profiles; a private restricted container proves admitted ingress without
independent outbound access. The port agent tools and local CLI are later parts.

| Module | Responsibility |
|--------|----------------|
| [after_up](src/after_up.rs) | Journalled argv commands after a managed unit starts, bounded output and whole-group cancellation. |
| [ports_carrier](src/ports_carrier.rs) | Admitted TCP and per-sender UDP sockets served through the existing machine tunnel. |
| [host_ports](src/host_ports.rs) | Public address and range review, constrained by the one machine ports policy. |
| [host_publish](src/host_publish.rs) | Physical public TCP/UDP listeners shared by directories and node units; withdrawal closes existing callers. |
| [host_publish/record](src/host_publish/record.rs) | Bounded numeric socket records without retaining traffic content. |
| [roles/ports](src/roles/ports.rs) | One physical publisher command handler and live policy watcher, offered through the machine's existing Environment or Node role. |
| [local_ports](src/local_ports.rs) | Environment-local read-only ports, exact active peer lookup and coalesced assertion refresh without a private signer. |
| [local_ports tests](tests/local_ports.rs) | An attached environment gets only its own read bearer, which dies on detach; its HTTP reader never writes. |
| [port_forward](src/port_forward.rs) | The helper of a port: a listener on the caller's own loopback whose each connection, or each UDP sender, is carried to a port of an environment. |
| [connector](src/connector.rs) | The connector as a whole: the link, the listener, the policy, the roles it plays and the commands they answer, started together and stopped together. |
| [programs](src/programs.rs) | One native program-registry interface and offered-role/local-policy gate, rechecked at receipt and persistence. |
| [uninstall](src/uninstall.rs) | Installer ownership receipts, shared-program instances, terminal plans and an instance lock that survives data removal. |
| [installation](src/installation.rs) | Observed executable/account/service and exact data-directory/configuration commands for connector and node. |
| [enrollment](src/enrollment.rs) | Initial local identity and endpoint setup, in a data directory of its own, so that several connectors run on one computer. |
| [session](src/session.rs) | Machine link connection and reconnect, and the round trip it measures. |
| [policy](src/policy.rs) | Local ceilings that hold for everyone, admins included, and come before every page: the arrangement types the machine allows and what its exec node may offer among them; the switches for file writes, delivered files, fetched builds and shells, and for listing and importing the tool servers found here; shown to the server, changed here, and from the server only for the keys the owner handed over, by whoever manages what each bounds. |
| [dispatch](src/dispatch.rs) | Role commands and durable event production. |
| [roles](src/roles/mod.rs) | Roles components. |
| [roles/agent](src/roles/agent.rs) | Adapter hosting and harness lifecycle. |
| [harnesses](src/harnesses.rs) | Provider/profile homes, the quiet and pinned start, a harness held to the MCP servers of its session, its own memory pointed at the thread's directory or switched off, supervision of one process or one for each conversation, and session recovery. |
| [reconciliation](src/reconciliation.rs) | Reopen spool and reconcile hosted sessions. |
| [reconciliation/environments](src/reconciliation/environments.rs) | Sealed environment attachment and tool intent retained before effects and rebuilt under current local policy. |
| [loopback](src/loopback.rs) | Session-keyed model, executor, MCP and Orch-tool endpoints; Orch's own tools, declared with their schemas, titles and hints, beside every executor unless the environment says otherwise. |
| [tunnels](src/tunnels.rs) | The tunnels other machines open to this one, handed to whatever serves their kind, and the opening of the ones this machine needs. |
| [events](src/events.rs) | What an adapter reports, as events of the link: which stream each goes on, and how the text that grows is bundled. |
| [facts](src/facts.rs) | What the machine reports of itself: the host, its roles' contributions, its local policy and its link. |
| [storages](src/storages.rs) | Named machine storages and pinned owned directories under the machine's allowed roots. |
| [mount_data](tests/mount_data.rs) | A directory machine uses volume commands and the common storage factory to create, attach, read and remove owned mount data safely. |
| [ledger](src/ledger.rs) | What the connector hosts, kept on disk: the harness processes it started and the sessions that are open in them. |
| [lifecycle](src/lifecycle.rs) | Lifecycle hooks shared by unit hosts and directory connectors. |
| [content](src/content.rs) | Content that moves beside the session, for the commands that carry it: an archive or an image fetched for a command, a session exported and put up. |
| [https](src/https.rs) | HTTPS enrollment/renewal/content requests; enrollment alone permits direct loopback HTTP, without a proxy or redirects. |
| [console](src/console.rs) | The local console: a terminal interface to enrollment, environments, what the exec node offers, the tool servers found and imported, limits, providers and activity. Six screens (ratatui on crossterm); limits and directories written to the configuration file, the rest asked of the running connector. |
| [console/enrollment](src/console/enrollment.rs) | Extract pasted enrollment data without executing the command; Start hides its token and uses the ordinary enrollment/service paths. |
| [automatic](src/automatic.rs) | Hourly verified recommended updates, with current local policy and an atomic idle fence; the console names a newer recommendation while automatic updates are off. |
| [local_socket](src/local_socket.rs) | The running connector's local socket in its data directory (a `0600` socket file on Unix; the loopback with a token file on Windows): one line of JSON a request and an answer, for the console and `orch-connector mcp`; the system's latest requests kept in memory for the console's Activity. |
| [roles/provider](src/roles/provider.rs) | Credential custody, sign-in on the provider by its adapter's methods, and model-serving role. |
| [roles/environment](src/roles/environment.rs) | Compose environment-host services. |
| [mcp_import](src/mcp_import.rs) | The MCP servers of the harnesses' own configuration here: names reported, one definition copied into the connector's own directory when it is imported, started from that copy, and forgotten. |
| [orch_tools](src/orch_tools.rs) | Orch's own tools as the MCP server `orch` on the listener: the tools a session was given, listed to its harness, and each call carried to the system as an event and answered with its result, or with the error that Orch is unreachable when nothing answers in time. |
| [shells](src/shells.rs) | A person's shell attached through a tunnel that ends at the API: what the shell printed last first, then what comes, typing and resizes the other way. |
| [tool_tunnels](src/tool_tunnels.rs) | Executors and tool servers reached through tunnels from the machine where the harness runs: an HTTP exchange forwarded to the executor's or the server's address, the messages of Codex's exec server carried both ways. |
| [tool_endpoints](src/tool_endpoints.rs) | What a harness on this machine reaches its tools at: its connection to its executor, `/exec/<session key>`, and its tool servers, `/mcp/<session key>/<name>`, each carried to where the tool runs. |

The local `mcp import` command and console Tools → Import ask the system through the running
connector's link. The system checks the current machine owner and environment, files the same
copy as the web import, and audits its command-line origin. The local import ceiling still applies.
Name collisions require an explicit harness or directory choice; a lost answer is unconfirmed.
| [builds](src/builds.rs) | Harness and executor builds fetched on the server's word where the owner allows it, verified by hash. |
| [forwarder](src/forwarder.rs) | Lease, model, allowance and path checks, endpoint choice by protocol, the key presented as the endpoint says with its headers and query, optional translation and upstream streaming. |
| [upstream](src/upstream.rs) | The HTTP exchange with an upstream model service: a connection made for the request, the request sent whole, and the answer handed on piece by piece as it arrives. |
| [leases](src/leases.rs) | The leases of the credential kept here: one turn's leave to use it, the models it allows, what it has used, and which one a session's model calls run under. |
| [credentials](src/credentials.rs) | Local keys, sealed exchange, shared renewal and handover. |
| [usage](src/usage.rs) | Provider facts and per-request usage reporting. |
| [key_facts](src/key_facts.rs) | Ordinary-key observations for documented vendors, in vendor units, with bounded read-only requests. |
| [sources](src/sources/mod.rs) | Independent provider facts, model lists and credential presentation beside the credential's keeper. |
| [sources::reports](src/sources/reports.rs) | Member-by-member provider reports, independent observation times and removal of supplied secret values. |
| [sources::endpoints](src/sources/endpoints.rs) | The endpoints' bounded model catalogues, preserving pagination, deduplication and refusal behavior. |
| [sources::adapter](src/sources/adapter.rs) | The credential's own harness as the facts, models and reset source, preserving native calls and failures. |
| [sources::address](src/sources/address.rs) | Bounded provider source requests to one authorized address, without redirects, cookies or credential-bearing errors. |
| [sources::dispatch](src/sources/dispatch.rs) | Source choices resolved beside the credential, with keeper metadata, per-member reports and safe link results. |
| [sources::presenter](src/sources/presenter.rs) | A locally registered program presents a key, with endpoint-specific renewal and no fallback after a failed exchange. |
| [doctor](src/doctor.rs) | Connectivity, identity, clock and harness diagnostics: the certificate's state, each gateway address answering over TLS as the machine trusts it, the clock against the gateway's `Date` (the link's 120 s skew), the harnesses named, and the warnings and errors of the last two days of its own log. |
| [update](src/update.rs) | Verified atomic update: the system's listing for this platform, a newer build downloaded and checked against its size and SHA-256, put in the program's place in one rename (`.old` on Windows, removed at the next start). |
| [service](src/service/mod.rs) | The per-user service that keeps a connector running, named as the installers name it: one for the platform's own data directory, one with a suffix of its path's SHA-256 for each named one. |
| [service/linux](src/service/linux.rs) | The systemd user unit: starts with the user manager after the network, again ten seconds after a failure. |
| [service/macos](src/service/macos.rs) | The launchd agent in the GUI session: at login, again ten seconds after a failure, standard error in `logs/launchd.log`. |
| [service/windows](src/service/windows.rs) | The scheduled task at logon, least privilege, no time limit, started again a minute after a failure. |
| [browser_tunnel](src/browser_tunnel.rs) | The shared browser tunnel, carrying live pictures and checked input without a spool. |
| [display_tunnel](src/display_tunnel.rs) | Each display viewer gets one locally authenticated shared RFB connection. |
| [background](src/background.rs) | Volatile bounded native snapshots whose active transfers renew their idle deadline. |

| [tests/provider_embed](tests/provider_embed.rs) | Embeddings made on the system's own behalf in the provider's runtime: a batch of texts goes to the endpoint of the Embeddings format with the key and the secret headers its options say, the vectors come back in the texts' order with what they used, and a refusal says what was used without the secrets. |
