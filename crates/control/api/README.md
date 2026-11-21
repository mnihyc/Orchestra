# orch-api-core

The HTTP and realtime API of Orch. It is the server behind the web application: the browser API (`/api/v1`), the API for account keys (`/openapi/v1`), the realtime socket, and the backplane that the gateways connect to. This package assembles the application. Its parts live in the packages beside it in this directory.

The API runs as one process in a deployment. It keeps no state of its own that the database does not hold, and it reaches machines only through the gateways. The thread engine ([orch-engine](../engine/README.md)) does the durable work of turns; the database layer ([orch-db](../db/README.md)) holds the queries.

## What is in this directory

| Package | What it holds |
|---|---|
| [base](base) (`orch-api-base`) | The shared application: startup and state, middleware, the route registry and the OpenAPI document, sign-in, uploads and content, realtime feeds, the backplane to the gateways, certificates, ports, hosting, and the routes that belong to no other area. Most of the tests are here |
| [threads](threads) (`orch-api-threads`) | Routes for threads and topics, forks and switches, approvals, sharing, and the links of a document |
| [documents](documents) (`orch-api-documents`) | Routes for documents and the document tree, imports, exports, summaries, downloads and storage |
| [resources](resources) (`orch-api-resources`) | Routes for providers, models, hosts, machines and the other resources, capabilities, trust, and duplicating |
| [environments](environments) (`orch-api-environments`) | Routes for environments and their workspace (files, snapshots, source), the fleet, loss and recovery, and processes |
| [interactive](interactive) (`orch-api-interactive`) | Routes for terminals, browsers, displays and ports: their tickets, entry and requests |
| [admin](admin) (`orch-api-admin`) | Routes for administration, people and their profiles, first steps, settings and personal pages, and administrator messages |
| [collaboration](collaboration) (`orch-api-collaboration`) | Routes for chats, notifications, search, access, nodes, transfer, the trace and the everything list |
| [src](src) | `orch-api-core` itself: the stable assembly that re-exports the parts above |

The API is documented as an [OpenAPI document](../../../docs/openapi.json) and described for people who use it with a key in the [API with a key](../../../docs/operator/api-with-a-key.md) guide.

## Build and test

```sh
cargo build -p orch-api-core
cargo test -p orch-api-base --test <name>      # one test file, named for what it checks
cargo xtask verify                              # format, Clippy, build, docs and dependency checks
```

The tests run against a real PostgreSQL and make a database of their own for each test. They use the server named by `ORCH_TEST_DATABASE_URL`, or a PostgreSQL 17 on `127.0.0.1:55432` with the user and password `orch`. The test files are in [base/tests](base/tests); each is named for the behavior it checks. See [CONTRIBUTING.md](../../../CONTRIBUTING.md) for the checks every change passes.

## Module map

What each module does, by the file that holds it.

### Source

| Module | What it does |
|--------|--------------|
| [sandbox_readiness](base/src/sandbox_readiness.rs) | Common requested sandbox, actual executor modes, limit sources and authorized policy acts on machine pages and blocked threads. |
| [ports_runtime](base/src/ports_runtime.rs) | Live port measurements and single-use channel admissions over the existing backplane. |
| [port_tools](base/src/port_tools.rs) | The four port tools over the thread owner's current environment and authority. |
| [ports_browser](base/src/ports_browser.rs) | The browser route of a port: a request to its stable name, let in by a person's entry or a link and carried to the program with who is calling. |
| [host_ports](base/src/host_ports.rs) | A host's ports at the API: its node's check of a declaration under its own ceiling (`host.ports`), and the node's last proposal for the host's page. |
| [backplane::port_records](base/src/backplane/port_records.rs) | Live whole-row pages from the gateway and host nodes, combined without a connection-history table. |
| [routes/ports](base/src/routes/ports.rs) | Guarded stable port targets, reviewed wider admissions and safe outside listings. |
| [routes/port_entry](base/src/routes/port_entry.rs) | Entering a port without the browser route: a link's use, its landing, the raw socket and the public assertion keys. |
| [routes/port_requests](base/src/routes/port_requests.rs) | A thread's request to let more people reach a port, and a person's decision on its card. |
| [routes/resources/host_maintenance](resources/src/routes/resources/host_maintenance.rs) | Host load history, safe removal of empty hosts and recycling by their managers. |
| [routes/resources/host_storage](resources/src/routes/resources/host_storage.rs) | Host maintenance settings use the shared registry, resolver, locks and revision guards. |
| [routes/resources/provider_updates](resources/src/routes/resources/provider_updates.rs) | Planned updates and recreations of provider runtimes, individually or in bounded batches. |
| [routes/environments/moves](base/src/routes/environments/moves.rs) | Workspace move plans and confirmations, for the same environment on another managed host. |
| [routes/environments/updates](base/src/routes/environments/updates.rs) | Update and recreate plans over HTTP, confirmed once and checked again before a runtime is held. |
| [server](base/src/server.rs) | TLS listeners, startup and shutdown. |
| [machine_requests](base/src/machine_requests.rs) | Local machine requests carried by its authenticated link and authorized as its current owner. |
| [local_ports](base/src/local_ports.rs) | Read-only inside port projections and live peer assertions, scoped to the authenticated current environment machine. |
| [state](base/src/state.rs) | Application composition and shared service handles. |
| [attachments](base/src/attachments.rs) | Files that people put into chats and documents: taken into the store from a body or from a finished upload, told apart by name and first bytes, and given back under the name and the type they were kept with. |
| [archives](base/src/archives.rs) | What an archive held in the store contains, listed and read one entry at a time without unpacking it. |
| [syncs](base/src/syncs.rs) | The state behind a durable feed, put together from one list of what that state consists of. |
| [registry](base/src/registry.rs) | HTTP registration with permission and token metadata. |
| [sign_in](base/src/sign_in.rs) | Signing in: the proofs a sign-in takes and what it still lacks; wrong tries counted by the name that was tried, each proof apart and a browser its person signed in from by itself; a code sent by mail; and the answers that tell nothing about a name. |
| [sign_in/accounts](base/src/sign_in/accounts.rs) | Making a person: the rules a username, a name, an address and a password meet, and the user with the org that is theirs. |
| [sign_in/codes](base/src/sign_in/codes.rs) | The codes of a second step: the code that is mailed, and the one an authenticator app shows, with the key that seals an authenticator's secret. |
| [sign_in/login](base/src/sign_in/login.rs) | The sign-in, and the request for a code by mail; the second answers the same to every name. |
| [sign_in/mail](base/src/sign_in/mail.rs) | The mail that concerns a person's own sign-in: the confirmation of an address, the reset of a password, a code to sign in with, and the notice to an address that it was replaced. |
| [sign_in/fresh](base/src/sign_in/fresh.rs) | A fresh proof for a sensitive act: the caller's password, or the one-use proof of their identity provider where they have none, with their second step where they have one, all in the request that does the act. |
| [sign_in/passwords](base/src/sign_in/passwords.rs) | Passwords: kept as Argon2id hashes, compared in constant time, and the rules a new one has to meet. |
| [sign_in/people](base/src/sign_in/people.rs) | A person as others see them and as they see themself: the avatar, what a mailed code may be for them, and the address they asked for and have not confirmed. |
| [sign_in/proofs](base/src/sign_in/proofs.rs) | Looking at a proof: a password, a code that was mailed, and a code of an authenticator app, each counted by itself; and what a set of right proofs lacks of a sign-in. |
| [sign_in/recovery_codes](base/src/sign_in/recovery_codes.rs) | Ten one-use, one-hundred-bit authenticator backups; only user-bound, domain-separated hashes are stored. |
| [sign_in/reset_links](base/src/sign_in/reset_links.rs) | A reset link made by hand, for a person who cannot sign in and whom no mail reaches: made by a system admin, or by the operator on the host. |
| [sign_in/second_step](base/src/sign_in/second_step.rs) | The second step an org can require: of whom it is required, where a request waits for it, and what the person and their org's managers are told of it. |
| [sign_in/throttle](base/src/sign_in/throttle.rs) | Counting wrong tries: by the name that was tried, each proof apart, a browser its person got in from by itself, and one address across all names. |
| [middleware/authentication](base/src/middleware/authentication.rs) | Resolve sessions and scoped tokens. |
| [middleware/admin_openings](base/src/middleware/admin_openings.rs) | One audit row for each private object opened through a browser session's administrative reach. |
| [middleware/authorization](base/src/middleware/authorization.rs) | Delegate decisions to orch-authz, and carry what is missing, and whether the caller may force it, into refusals. |
| [middleware/transaction_guard](base/src/middleware/transaction_guard.rs) | Transaction-local request authority using current identity, token scopes and the application clock. |
| [middleware/authorization/refusals](base/src/middleware/authorization/refusals.rs) | What the access engine says, as the problems a client acts on: a refusal with everything that is missing and who can give it, a suspension in an org, and a place that is archived. |
| [middleware/requests](base/src/middleware/requests.rs) | Request IDs, errors, body limits and rate limits. |
| [middleware/requests/words](base/src/middleware/requests/words.rs) | Public problem sentences from the same English catalogue as the interface, with diagnostic fields kept separate and both languages checked against every API code. |
| [middleware/idempotency](base/src/middleware/idempotency.rs) | Transactional replay and revisions. |
| [routes/meta](base/src/routes/meta.rs) | Health, setup and public metadata. |
| [routes/auth](base/src/routes/auth.rs) | Registration, signing in by a password, by a code sent by mail and through an identity provider, the second step, sessions, recovery and the change of an address. |
| [routes/auth/me](admin/src/routes/auth/me.rs) | The signed-in person: what they set for themself, their password, their second steps, their sessions and their API tokens. |
| [routes/auth/bootstrap](admin/src/routes/auth/bootstrap.rs) | One shell read of the person, personal state, organizations, mode, favorites, chat state and inbox summary. |
| [routes/auth/oidc](admin/src/routes/auth/oidc.rs) | Signing in through an identity provider: the start and the callback of a sign-in, of a link to the signed-in person and of a fresh proof for one sensitive act, the start of a link and the unlinking of an identity, and the kinds that speak to providers. |
| [routes/auth/oidc/landing](admin/src/routes/auth/oidc/landing.rs) | What an identity a provider vouched for leads to: a person signed in, a person made and signed in, the identity linked to the person who asked, or a one-use proof for one sensitive act. |
| [routes/auth/tokens](base/src/routes/auth/tokens.rs) | API tokens made narrow and with a fresh proof, within the longest life their places admit, asked again by their key without their secret; and an org's list of the tokens that reach it, each of which it can refuse there. |
| [routes/users](base/src/routes/users.rs) | Users, and invitations into an org at any level, to one place or the system's: a link to share or a personal link, within what the root org allows. |
| [routes/profiles](base/src/routes/profiles.rs) | Personal pages, visible pins, work by day and direct shares between people, with recorded inspection of otherwise closed pages. |
| [routes/chats/mentions](collaboration/src/routes/chats/mentions.rs) | Current readable chat mention choices, filtered before pagination. |
| [routes/chats/asking](collaboration/src/routes/chats/asking.rs) | Explicit agent asks reuse ordinary chat-source forwarding and the sender's guarded input producer in one transaction. |
| [routes/chats/agents](collaboration/src/routes/chats/agents.rs) | Human management of a chat's per-origin posting stop, serialized with the agent's own sends. |
| [routes/chats/readers](collaboration/src/routes/chats/readers.rs) | Current explicit inspection holders for a static reading-access description, with no actual-read event. |
| [routes/documents/creation](documents/src/routes/documents/creation.rs) | Copies and imports use the ordinary guarded creation producer, with current source rights and truthful entry receipts. |
| [routes/documents/references](base/src/routes/documents/references.rs) | The card a reference's chip opens: what the reference names, and the part it names, as the caller may read them. |
| [routes/links](base/src/routes/links.rs) | A document's Links section: which tabs it shows, what refers to it, what it refers to, and the graph around it. |
| [routes/links/references](threads/src/routes/links/references.rs) | What refers to a document, with the line around each mention, and what the document refers to. |
| [routes/links/graph](threads/src/routes/links/graph.rs) | The graph around a document: only readable documents, chats and threads, joined by the references among them. |
| [routes/links/placed](threads/src/routes/links/placed.rs) | What a reader is told of the things a list names: each one's name as they may read it, and the places it lies in. |
| [routes/documents/imports](documents/src/routes/documents/imports.rs) | Bounded Markdown and HTML imports with per-entry guarded creation and durable document identities. |
| [routes/nodes](base/src/routes/nodes.rs) | Hierarchy with orgs inside orgs, a project and a department moved inside their tree, archive, what was deleted and its restore, tags, subscriptions, a place's Activity, members with their domain, moving one and suspending one, and the end of a membership. |
| [routes/nodes/view](base/src/routes/nodes/view.rs) | A node as the API shows it to one caller: what is stored of it, its way down from the root org with the names the caller may know, and what the caller holds on it, for any number of nodes in the same few statements. |
| [routes/nodes/make](base/src/routes/nodes/make.rs) | Making an org inside an org, a project and a topic, and a team's root org, each with the Owners row of its maker and the audit row of its making. |
| [routes/nodes/tree](base/src/routes/nodes/tree.rs) | The nodes of a whole tree for a view of it, what somebody let the caller into by name, and the topics of a project with the filters of a list. |
| [routes/nodes/tree_work](base/src/routes/nodes/tree_work.rs) | Compact waiting and blocked explanations for the same authorized tree over HTTP and the live feed. |
| [routes/nodes/moves](base/src/routes/nodes/moves.rs) | Moving a project, or an org that lies in another, under another org of its own tree, and the plan that says what the move would change before anything does. |
| [routes/nodes/moves/review](collaboration/src/routes/nodes/moves/review.rs) | Stored move-consequence stamps and the ordinary access engine's rolled-back move comparison. |
| [routes/nodes/archive_plan](collaboration/src/routes/nodes/archive_plan.rs) | Read-only archive consequences and caller-bound subtree fences, including exact inverse reviews. |
| [routes/nodes/removal](collaboration/src/routes/nodes/removal.rs) | Deleting a node with everything in it, bringing it back until it is purged, and handing a node to another owner. |
| [routes/nodes/activity](collaboration/src/routes/nodes/activity.rs) | A place's Activity and its audit trail. |
| [routes/storage](base/src/routes/storage.rs) | One authorized storage report from existing retention records, content metadata and node inventories. |
| [routes/settings](base/src/routes/settings.rs) | Settings of a node, an environment, a provider and the system; secrets metadata and parameters. |
| [routes/settings/reach](admin/src/routes/settings/reach.rs) | Batched, visibility-filtered setting reach using the resolver's merge and lock provenance. |
| [routes/access](base/src/routes/access.rs) | The permission catalogue, roles held in a place, permission pages with their owners and defaults pages with previews, the people behind a holder a page at a time, the one change that names owners and holders at a choice, asks who may name somebody from outside and marks an override, a thing's history, and access requests for everything missing at once. |
| [routes/access/page](base/src/routes/access/page.rs) | A page as the engine reads it, turned into the permission page a client draws: every holder by name, every entry with the person who wrote it, and every place it comes from. |
| [routes/access/subject](base/src/routes/access/subject.rs) | What a request that is the same for every kind of thing names by `{object}` and `{id}` of its path, and the checks that are the same for all of them. |
| [routes/access/thread_environment](collaboration/src/routes/access/thread_environment.rs) | What a thread's page shows of the environment it runs in: how many people read its files whatever the page says, and what sharing the thread offers beside it (looking in, and for whoever sets the thread up the Manage of its files and its shells). |
| [routes/access/readers](collaboration/src/routes/access/readers.rs) | What one person holds on a thing and what gives it to them, the people behind a holder of its page, and what was done to it and by whom: the same three requests for a node, a resource and a document. |
| [routes/access/previews](base/src/routes/access/previews.rs) | What a change of a page, of the defaults of a place or of the defaults of the system would do, said before anything is changed: how many pages it reaches and who would gain and lose what on them. |
| [routes/access/share_preview](collaboration/src/routes/access/share_preview.rs) | One ordinary share reviewed before applying it, with current holders, effects and the rules of its place. |
| [routes/access/requests](base/src/routes/access/requests.rs) | Asking for access to things, reading where the request stands, and deciding it: one approval for each thing asked for, each sent to the nearest people who can give it. |
| [routes/transfer](base/src/routes/transfer.rs) | Handing a resource or a document over to another person, who is its first owner from then on. |
| [routes/threads](base/src/routes/threads.rs) | Sync, timeline with the lines of a thread's history and that history as a view, inputs on the sender's own access, side threads, the limits in a turn's way and letting an input past them, draft, pins, binding, fork from any message and exports at three depths. |
| [routes/threads/view](base/src/routes/threads/view.rs) | A thread as the API shows it to one caller: what the engine knows of it, what it runs on and what that comes to, whether the caller may give it input, and what they may do with it. |
| [routes/threads/make](base/src/routes/threads/make.rs) | Making a thread in a topic: its binding to a provider with model and effort, an agent profile and an environment, checked for the caller by the access engine with everything missing in the refusal; the page it starts with; and what is said to it at once. |
| [routes/threads/read](threads/src/routes/threads/read.rs) | Reading a thread: a sync that brings a client up to date, a stretch of the timeline around a place, one item, a command's output as plain text, the outline, the catch-up, and how far the caller has read. |
| [routes/threads/read_history](threads/src/routes/threads/read_history.rs) | Explicit historical turn and segment pages, projected with only their required references. |
| [routes/threads/opening](threads/src/routes/threads/opening.rs) | One initial screen read; the existing native handlers remain the source of every projection. |
| [routes/threads/approval_items](base/src/routes/threads/approval_items.rs) | Personalize harness request cards in a timeline batch, without a per-card HTTP read. |
| [routes/threads/source](threads/src/routes/threads/source.rs) | Explicit accepted thread links, with current authority and one history marker. |
| [routes/threads/drive](threads/src/routes/threads/drive.rs) | Driving a thread: an input sent, with its delivery and the client's own value that makes sending it again safe, then edited, put in order or cancelled while it waits; a turn stopped; and a draft kept for the composer. |
| [routes/threads/parameters](threads/src/routes/threads/parameters.rs) | What a thread's agent is set to, and what it could be set to: the model with the models its provider lists, the effort, and the parameters its harness declares, each with where its value comes from and whether the caller may change it. |
| [routes/threads/export](threads/src/routes/threads/export.rs) | A thread exported whole as JSON: the thread, its segments, its turns and its items, which is what a person keeps when they want the conversation outside Orch. |
| [routes/threads/written_export](threads/src/routes/threads/written_export.rs) | A thread exported in its written form: the conversation as Markdown at one of three depths, the same text that an agent which may read the thread is given, and the thread's own chat after it when it is asked for. |
| [routes/threads/binding](base/src/routes/threads/binding.rs) | What a thread may be put on and putting it there: the providers with their models and the profiles that run on them, in every arrangement, with the reason one is not available or not permitted; the environments the caller manages and what a thread would get there; the defaults; and the binding and the environment of a thread that has no history yet. |
| [routes/threads/binding/start](threads/src/routes/threads/binding/start.rs) | Binding choices before a topic's first thread, judged in an existing environment or the resolved template. |
| [routes/threads/uploads](base/src/routes/threads/uploads.rs) | Images and files sent from a browser, whole or in pieces, and a file attached to a thread so that it reaches the agent when the input is delivered. |
| [routes/threads/raw](threads/src/routes/threads/raw.rs) | The raw events of a segment of a thread, for whoever manages the thread: what went to the harness and what came back, newest first. |
| [routes/threads/instructions](threads/src/routes/threads/instructions.rs) | What a thread's agent reads as its instructions, source by source, as the last session was opened with them and as the reader may see them (a file's path only with the files of its environment, a home's file by its label alone), and the standing instructions as they stand now. |
| [routes/threads/model_inputs](threads/src/routes/threads/model_inputs.rs) | What Orch supplied to a model for a node, as it was sent: the records written before each sending, block by block and whole, the newest first, for whoever may read the node. |
| [routes/threads/adopt](base/src/routes/threads/adopt.rs) | A session that a person began in a harness by hand, taken in as a thread of a topic: found by asking the harness, put in the folder of theirs that holds the directory it ran in, and checked as any thread is for what the caller may use. |
| [routes/chats](base/src/routes/chats.rs) | Chats, messages, states, attachments, inspection and forwarding. |
| [routes/chats/files](collaboration/src/routes/chats/files.rs) | The bytes of a chat's files: a file stored to be named by an `attachment` part, and an image or a file that a message carries, given back to whoever reads the chat. |
| [routes/chats/export](collaboration/src/routes/chats/export.rs) | A chat taken away as a file: its written form or its messages as records, and with its files an archive of them. |
| [routes/exports](base/src/routes/exports.rs) | Exports over HTTP: an archive of what a person may read in a place, planned before anything is made, asked for as a job, listed, fetched and removed by its person alone. |
| [routes/documents](base/src/routes/documents.rs) | Documents: listing beside a node, blocks and their operations, comments, versions and what changed between two of them, files, export as Markdown, HTML and Typst with what it is asked to hold, import, moving and deleting. |
| [routes/document_tree](base/src/routes/document_tree.rs) | Typed document-folder metadata, child lists and atomic replay-safe organization. |
| [routes/documents/build](base/src/routes/documents/build.rs) | A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name. |
| [routes/documents/listing](documents/src/routes/documents/listing.rs) | The private documents administered beside a place, paged by name without content. |
| [routes/documents/limits](base/src/routes/documents/limits.rs) | What a document may hold where it stands, as the settings of its node resolve it, and the refusals that say which limit was met and where it is set. |
| [routes/documents/notices](documents/src/routes/documents/notices.rs) | Who is told that a document names them, and who is told of an answer in a discussion beside it. |
| [routes/documents/ops](documents/src/routes/documents/ops.rs) | A batch of changes applied to a document, whole or not at all, and what a refused batch is answered with. |
| [routes/documents/mentions](documents/src/routes/documents/mentions.rs) | Current document readers for mentions through the common holder graph and ordinary document checks. |
| [routes/documents/history](documents/src/routes/documents/history.rs) | The history of a document: versions cut by the clock, listed newest first with their authors and what each changed; what differs between two states block by block; a version as it stood; and putting one back. |
| [routes/documents/comments](documents/src/routes/documents/comments.rs) | Discussions on a block, or on words of it: opened, answered, resolved, and their comments edited, deleted and reacted to. |
| [routes/documents/subscription](documents/src/routes/documents/subscription.rs) | A document's personal discussion bell, independent of its text and read position. |
| [routes/documents/export](base/src/routes/documents/export.rs) | A document written out as a file: Markdown, one HTML page, Typst source with what it names, or its blocks as JSON, with the files and the discussions beside it when they are asked for. |
| [routes/documents/files](documents/src/routes/documents/files.rs) | The files a document holds: put in from a body or from a finished upload, given back to whoever reads the document, and what an archive among them contains. |
| [routes/documents/page](base/src/routes/documents/page.rs) | The permission page of a document, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice. |
| [routes/share](base/src/routes/share.rs) | Sharing a chat, a document, a thread or a whole place with agents, by area, as a copy or a live share to read or to write; listing, taking back, and an agent's reach as a tree. |
| [routes/forks](base/src/routes/forks.rs) | Fork and switch plans with their options, and execution, single and batch; side threads started and listed. |
| [routes/topics](base/src/routes/topics.rs) | Overview, the environments a topic's threads use, places and announcements. |
| [routes/topics/announcements](base/src/routes/topics/announcements.rs) | What a person tells the threads of a topic, or of every open topic of a project, and what has been told: the four requests that list and make announcements for people. |
| [routes/topics/sync](base/src/routes/topics/sync.rs) | The state behind a topic's feed: the topic, its threads, the jobs about them and its announcements, or what changed after a sequence. |
| [routes/capabilities](base/src/routes/capabilities.rs) | Taking a capability back from an environment: the key removed, the removal written down, and the unit of the environment asked to be made anew without it. |
| [routes/environments](base/src/routes/environments.rs) | Environments as resources: creation, lifecycle, configuration changed while one runs, their own secrets, what the exec node offers, the MCP servers found there, their import and one of an environment's own, who reaches into one, its history, update plans, removal, moves, and their jobs. |
| [routes/environments/hosted](base/src/routes/environments/hosted.rs) | Environments made from a template on a host over HTTP: making one, starting and stopping it, keeping it awake, taking back a removal that has not begun, and the fleet of a place. |
| [routes/environments/list_settings](base/src/routes/environments/list_settings.rs) | Resolve static environment display settings in one batch before projecting a resource list. |
| [routes/environments/jobs](base/src/routes/environments/jobs.rs) | The jobs of environments over HTTP: reading one with how far it has come, and asking that one be stopped. |
| [routes/environments/places](base/src/routes/environments/places.rs) | Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage. |
| [routes/environments/network](base/src/routes/environments/network.rs) | An environment's or a runtime's network as people see it: what its host's node reported, the restriction retained on it, and what its template asks for where nothing was reported yet. |
| [routes/environments/reach](environments/src/routes/environments/reach.rs) | Who reaches into an environment, and by what, over HTTP: its owners, the people its own page lets in by managing it or by one of its four doors, and those who may talk to an agent that runs there. |
| [routes/environments/agent_reach](environments/src/routes/environments/agent_reach.rs) | What the agents of an environment can reach, over HTTP: one read-only overview cut by the caller's own rights. |
| [routes/environments/reading](base/src/routes/environments/reading.rs) | Who may read an environment and how much of it: whoever holds a permission on it reads all of it, and whoever holds none and may view a thread that runs there reads its name, owner, place, state and the job under way, and nothing of what is behind it. |
| [routes/environments/support](environments/src/routes/environments/support.rs) | What an environment was found to have for browsers and displays, as the facts its machine reported hold it: the programs, the window manager, the command line an agent drives a browser with, and when the environment looked. |
| [routes/environments/sync](base/src/routes/environments/sync.rs) | The state behind an environment's feed: the environment as the caller sees it, and the jobs that make it or change it. |
| [routes/workspace](base/src/routes/workspace.rs) | Files with rename, copy and the search inside them, what an archive holds, cache, snapshots with their archives, Git and attached mounts of an environment's workspace, reached by the permissions of the environment's own page. |
| [routes/workspace/paths](base/src/routes/workspace/paths.rs) | A path of a workspace as a request writes it, and what it may be. |
| [routes/workspace/problems](base/src/routes/workspace/problems.rs) | What a machine's refusal is as a problem of the API: the code a machine answers with, and what was being done, tell which. |
| [routes/workspace/door](base/src/routes/workspace/door.rs) | How a request gets into an environment's workspace: by the permission of the environment's own page, whichever thread the caller works beside. |
| [routes/workspace/live](base/src/routes/workspace/live.rs) | Whether an environment can be asked now, asking it, and asking its machine for its measurements once for each session. |
| [routes/workspace/listing](environments/src/routes/workspace/listing.rs) | Entries and file descriptions made from what the API kept: the list of a tree a machine uploaded, or the versions of files whose content it holds. |
| [routes/workspace/files](base/src/routes/workspace/files.rs) | The files of an environment's workspace: listed live or from the last list of the tree, described, brought to the store and read from it in ranges, written on the version the writer saw, made, renamed, copied and deleted. |
| [routes/workspace/looking](environments/src/routes/workspace/looking.rs) | What can be found out about a workspace without opening its files one by one: the latest change of each path that agents made, a search by name or inside the text, what an archive holds, and the state and differences of a repository. |
| [routes/workspace/snapshots](environments/src/routes/workspace/snapshots.rs) | A thread's directory kept as of a moment: taken as a job, listed, read file by file, downloaded as one stream, renamed and deleted; and who may read and cancel the jobs of the workspace. |
| [routes/workspace/source](environments/src/routes/workspace/source.rs) | Guarded source preparation and reviewed Git publication through the existing file jobs. |
| [routes/workspace/restores](environments/src/routes/workspace/restores.rs) | Current-authority restore plans bind immutable manifests, whole namespaces and visible writers. |
| [routes/processes](base/src/routes/processes.rs) | Process control and resource samples of an environment. |
| [routes/terminals](base/src/routes/terminals.rs) | Shells that are their opener's alone: sessions, the ticket that takes one to another screen, the choice of which to close at a person's limit, and closing by whoever manages the environment. |
| [routes/fleet](base/src/routes/fleet.rs) | Listings of environments and provider runtimes, recycle, and batch moves and updates. |
| [routes/approvals](base/src/routes/approvals.rs) | Approval listing and decisions. |
| [routes/resources](base/src/routes/resources.rs) | Providers, listed by what the caller owns, may use and manages, with their setup, sign-in by each declared method, allowed arrangement types and runtime (start, stop, update plans), models, profiles, hosts with their stored metrics, machines with their policy, templates of both kinds with their import, drivers, MCP presets and imported servers, and mounts; the owners of a resource. |
| [routes/resources/agents](base/src/routes/resources/agents.rs) | Agent profiles: the ones the caller may use or manage, reading one, making one in an org, changing, switching off and deleting one that no thread is bound to. |
| [routes/resources/hosts](base/src/routes/resources/hosts.rs) | Hosts: the machines that make environments from templates, listed by what the caller may use or manage, read, changed by their managers, and the token with which one enrolls. |
| [routes/resources/machines](base/src/routes/resources/machines.rs) | A person's machines: the token a machine enrolls with, listing and reading with whether each is online and what was registered from it, renaming, revoking, asking a machine for its facts again, and its local policy as it reported it with the keys its owner handed over. |
| [routes/resources/machines/enrollment](base/src/routes/resources/machines/enrollment.rs) | Issuer-scoped enrollment progress, reusing the native resource projection and its current rights. |
| [routes/resources/machines/programs](resources/src/routes/resources/machines/programs.rs) | Stored registry reads, machine-management authority and a native mutation acknowledged with full facts; transactions end before every native wait. |
| [routes/resources/models](resources/src/routes/resources/models.rs) | The models a provider lists, with the name that is sent and the name that is shown, adding, changing, enabling and removing them, and the models that discovery suggests, asked of the machine that holds the key. |
| [routes/resources/page](base/src/routes/resources/page.rs) | The permission page of a resource, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice. |
| [routes/resources/provider_login](resources/src/routes/resources/provider_login.rs) | Signing a managed provider in, again, or out, by any of its adapter's methods: the method and what it asks for, what the harness shows and asks back as data, the answer sent back, a sign-in given up, and the harness signed out. |
| [routes/resources/provider_plan](resources/src/routes/resources/provider_plan.rs) | Visible provider plan measurements, peak-preserving charts and reset-aware forecasts. |
| [routes/resources/provider_resets](resources/src/routes/resources/provider_resets.rs) | Browser-only confirmed provider resets, current authority, durable receipts and audit attribution. |
| [routes/resources/provider_runtime](resources/src/routes/resources/provider_runtime.rs) | A managed provider's runtime over HTTP: started and stopped by hand, and what the provider's page shows of it (its unit, its host, its template, how many units it has had, when the idle stop or the renewal of its sign-in comes, and the job at work on it). |
| [routes/resources/providers](base/src/routes/resources/providers.rs) | Providers on a key: the ones the caller owns, may use and manages, reading one without its key, making one with its endpoints and its key sealed, changing, switching off and deleting it, and writing the key and the secret headers again. |
| [routes/resources/provider_sources](resources/src/routes/resources/provider_sources.rs) | Source choices use the endpoint scheme policy, explicit key consent and only programs registered beside the credential. |
| [routes/resources/sealing](base/src/routes/resources/sealing.rs) | The key that seals what a request brings, and sealing a provider's key and secret headers with it before they are stored. |
| [routes/resources/setup](resources/src/routes/resources/setup.rs) | Everything the page that adds a provider needs, for the caller, in one answer: the profiles whose harness can hold a sign-in with the ways to sign in, the protocols and what an endpoint of each is asked for, the presets, the machines that may keep a key, and what the policies there say. |
| [routes/resources/templates](base/src/routes/resources/templates.rs) | Templates of both kinds: the ones the caller may use or manage, reading one, making one for an org or for the system, changing, switching off and deleting one that nothing was made from. |
| [routes/resources/templates/import](resources/src/routes/resources/templates/import.rs) | A driver's import proposes a validated recipe and explains what it read, without saving a template. |
| [routes/resources/validation](base/src/routes/resources/validation.rs) | Whole-value schema refusals for resource forms, with field paths and no supplied values. |
| [routes/resources/mounts](base/src/routes/resources/mounts.rs) | Mount resources, locations on hosts and personal machines, and the places and environments that use them. |
| [routes/resources/mcp](base/src/routes/resources/mcp.rs) | MCP presets with atomic revisioned definition and sealed-secret writes, environment-owned servers and imports whose definitions stay on their machine; public reads and audit never return credential values. |
| [routes/resources/skills](base/src/routes/resources/skills.rs) | Skills: versions from an uploaded archive, a git repository or address fetched by a host, or an environment's directory; Update from the recorded origin; drafts edited file by file and published as a version. |
| [routes/environments/mcp](base/src/routes/environments/mcp.rs) | The MCP servers an environment enables, imports copied on their machine and servers written for this environment alone. |
| [routes/resources/mcp_readers](base/src/routes/resources/mcp_readers.rs) | MCP presets listed with private permission-page destinations and read only with MCP View. |
| [routes/access/dependencies](collaboration/src/routes/access/dependencies.rs) | Permission-page dependencies resolved from the same defaults, model settings and giving rules as work. |
| [routes/access/candidates](collaboration/src/routes/access/candidates.rs) | One visible holder search for people, groups, roles, members and work, with whole current places. |
| [routes/resources/upstreams](resources/src/routes/resources/upstreams.rs) | The endpoints of an API provider: the protocols there are, the options each declares and what is refused. |
| [routes/resources/view](base/src/routes/resources/view.rs) | Machines, providers, profiles and environments as other things point at them, and what the caller holds on a resource, as the API shows them, for any number of resources in the same few statements. |
| [routes/notifications](base/src/routes/notifications.rs) | The Inbox and its preferences; channels of the system, a root org and a person with their check, test and texts; a person's routes and the link of one to a bot; push subscriptions; and the destinations of a node. |
| [routes/notifications/channels](collaboration/src/routes/notifications/channels.rs) | Channels, the ways out of the application: listed for the caller with their own route, set up by the system, a root org or a person, changed, removed and tried. |
| [routes/notifications/chat_grants](collaboration/src/routes/notifications/chat_grants.rs) | Gives one person access to a chat and optionally tells them in the same act. |
| [routes/notifications/exceptions](collaboration/src/routes/notifications/exceptions.rs) | Personal exceptions use stored subscriptions, one bounded View projection and the bell's mutation. |
| [routes/notifications/destinations](collaboration/src/routes/notifications/destinations.rs) | The destinations of a node: where what happens at an org, a project or a topic is sent, for nobody in particular, such as a team's room in a chat tool or a build system. |
| [routes/notifications/grouping](base/src/routes/notifications/grouping.rs) | The Inbox by place: the same notifications grouped under the nearest place the caller may read, what they can act on first, paged under a fixed cut; a group marked read up to its cut, and undone by a sealed ticket. |
| [routes/notifications/inbox](base/src/routes/notifications/inbox.rs) | The caller's Inbox: the list newest first, the badge's counters, what is marked read, what they chose about being notified, the browsers that take push, and the kinds of notifier this system has. |
| [routes/notifications/person](base/src/routes/notifications/person.rs) | Explicit person notices on readable documents, chats and places. |
| [routes/notifications/routing](collaboration/src/routes/notifications/routing.rs) | A person's routes through the channels that reach them: switched on or off, with the kinds they tell, the unread delay and the limit, the link of a route that has to be linked, and ended. |
| [routes/notifications/views](collaboration/src/routes/notifications/views.rs) | The shapes the requests answer with: a channel together with the caller's own route through it, a route, a destination, a kind of notifier in the caller's language, and the texts a channel keeps. |
| [routes/admin](base/src/routes/admin.rs) | Management: users and their suspension, a reset link that may switch a second step off, what a new person's own org starts as, root orgs and their two limits, both pools of runtimes and their updates, stored metrics, audit and system state. |
| [routes/admin/users](admin/src/routes/admin/users.rs) | People as a system admin keeps them: listed with the proofs strangers have locked, made with a link to set a password, suspended and let in again, made and unmade system admins, and given a link to set a password anew. |
| [routes/admin/orgs](admin/src/routes/admin/orgs.rs) | Every org of the system for the people who run it, and the two limits of a root org: whether its tree takes further members and whether it holds orgs inside it. |
| [routes/admin/semantic](admin/src/routes/admin/semantic.rs) | Search by meaning as a system admin sees it: whether the database can keep vectors, each root org's index with its progress, estimate, disk and reason, the choice to rebuild one in place, and the provider Local embeddings. |
| [routes/admin/settings](admin/src/routes/admin/settings.rs) | The settings of the system: every key that is set at system scope with where its value comes from, written (and locked for every node below, or not), and taken back to what Orch ships. |
| [routes/admin/defaults](base/src/routes/admin/defaults.rs) | The defaults page of the system, which every org starts from: read, changed, and put back to what Orch ships. |
| [routes/admin/audit](base/src/routes/admin/audit.rs) | The audit trail of the whole system, read a page at a time with its filters and written out as a streamed file for a span of at most a year. |
| [routes/admin/system](admin/src/routes/admin/system.rs) | How the system stands: its parts with their heartbeats, its queues, what its blob store holds, the state of its certificates, the document of its requests, and the certificate of its authority. |
| [routes/admin/fleet](admin/src/routes/admin/fleet.rs) | What runs on the system's hosts and how it has fared: every host, the runtimes of both pools, the measurements the system stored about itself over a span, and the plugins loaded anywhere. |
| [routes/admin/catalogue](base/src/routes/admin/catalogue.rs) | The data a system admin keeps for the whole system: the definitions of capabilities, the builds of harnesses that hosts may fetch, and the agent profiles that have no home. |
| [routes/admin/identity_providers](admin/src/routes/admin/identity_providers.rs) | The single sign-in methods a system admin keeps: listed, added, changed by revision, tested against the provider itself, and removed unless somebody could then no longer sign in. |
| [routes/downloads](base/src/routes/downloads.rs) | Connector binaries and installers served by the system. |
| [routes/search](base/src/routes/search.rs) | Authorized transcript and node search, the search inside one conversation, and the finder's names. |
| [routes/everything](base/src/routes/everything.rs) | The complete authorized hierarchy for every signed-in person, with a whole-system scope for administrators. |
| [routes/everything/resources](collaboration/src/routes/everything/resources.rs) | Resource-list filters and bounded rich rows, using the inventory's own frame and existing readers. |
| [routes/search/content](collaboration/src/routes/search/content.rs) | Content search over readable conversations, chats and current document blocks, with stable pages and Unicode excerpts. |
| [routes/search/meaning](collaboration/src/routes/search/meaning.rs) | Results by meaning joined with results by words in people's search ordered by best matches: one order by reciprocal-rank fusion, each result checked against what its reader may read now. |
| [routes/usage](base/src/routes/usage.rs) | The ledger read at three depths as figures, series and records, in tokens and credits; rates, with the catalogue of estimates and its upload, fetch and restore; caps on tokens, on credits and on turns running at once, with who is waiting and how often each was passed. |
| [routes/usage/scope](base/src/routes/usage/scope.rs) | What a reader of usage is shown, by the scope they ask about: their own use, a place and everything below it, or what ran on a provider; how deep their access goes there, and which dimensions and filters that depth gives. |
| [routes/usage/names](base/src/routes/usage/names.rs) | The names that figures and records are shown by: people, providers, models by what they are shown as, agents by kind, and places, each place a redacted reference where the reader may not see it. |
| [routes/usage/csv](interactive/src/routes/usage/csv.rs) | The other form of an answer: comma-separated lines, escaped as RFC 4180 says, for the series of figures and the records of the ledger; and the JSON answer of a request that can give either. |
| [routes/usage/figures](base/src/routes/usage/figures.rs) | Usage over a span: totals, a series cut into buckets, and breakdowns by dimension, each as deep as the reader's access to the scope goes; and the series as CSV. |
| [routes/usage/events](base/src/routes/usage/events.rs) | The ledger rows behind a figure, with the same scope and filters, each with its thread and its turn, what it ran on, the rate it was counted at and its credits; a page of them, or all of a page as CSV. |
| [routes/usage/rates](interactive/src/routes/usage/rates.rs) | What each model usable at a node would be counted at now and from where that rate comes, the shipped catalogue of estimates as it is loaded, and its replacement by a system admin from a file, from the address an admin set, or from the release. |
| [routes/usage/caps](base/src/routes/usage/caps.rs) | Caps made, changed and removed by whoever the place of the cap says; how each of them stands; everything that bounds work at a node; a person's own share of the caps counted per person; and who waits for room in a cap. |
| [routes/personal](base/src/routes/personal.rs) | Favorites, recents and read/subscription state; what the interface keeps for a person. |
| [routes/first_steps](base/src/routes/first_steps.rs) | Current readable setup observations and atomic portable first-steps choices. |
| [routes/directory](base/src/routes/directory.rs) | Org groups, roles, tags and service identities, each kept by one org at any level. |
| [routes/directory/members](admin/src/routes/directory/members.rs) | Ending a membership and what it touches, and changing the domain of a member, each with the plan of what it does before it does it. |
| [routes/directory/outsiders](admin/src/routes/directory/outsiders.rs) | People from outside a root tree who reach something of one of its orgs: the list, the ways each reaches it, and taking one out by a plan read whole. |
| [realtime/server](base/src/realtime/server.rs) | Browser socket authentication and reauthentication. |
| [realtime/subscriptions](base/src/realtime/subscriptions.rs) | Authorization-aware feed subscriptions. |
| [realtime/sync](base/src/realtime/sync.rs) | Sequenced fan-out, gap repair and database notification sweep. |
| [realtime/deltas](base/src/realtime/deltas.rs) | Streaming delivery bounded in bytes and in time, with deltas merged for a reader that is behind and no connection closed for being slow. |
| [realtime/presence](base/src/realtime/presence.rs) | Visible reader aggregation. |
| [realtime/chats](base/src/realtime/chats.rs) | Chat feed fan-out, typing and unread signals. |
| [realtime/documents](base/src/realtime/documents.rs) | The document feed: blocks that changed by sequence, discussions, presence, and who is writing where. |
| [realtime/environments](base/src/realtime/environments.rs) | The environment feed: state, jobs and pending updates, for everyone who may see the environment. |
| [realtime/resource_changes](base/src/realtime/resource_changes.rs) | Resource invalidations and private provider progress for transient subscribers. |
| [realtime/resource_rows](base/src/realtime/resource_rows.rs) | A resource change carries its native GET view, projected for the actual session receiving it. |
| [realtime/resource_health](base/src/realtime/resource_health.rs) | Provider health projected through the same bounded, authorized reader as the status API. |
| [realtime/personal_changes](base/src/realtime/personal_changes.rs) | Personal-state event values projected after commit without enlarging database notifications. |
| [realtime/raw_traffic](base/src/realtime/raw_traffic.rs) | Newly committed harness frames forwarded with the same masking as the raw-frame list. |
| [realtime/resources](base/src/realtime/resources.rs) | The resource feed: a change to reload, and a provider's sign-in state for those who manage it. |
| [realtime/threads](base/src/realtime/threads.rs) | The thread feed: the rows of a thread by its sequence, the thread itself, and who is looking. |
| [realtime/history_read_tests](base/src/realtime/history_read_tests.rs) | Deterministic snapshot boundaries with the native HTTP, machine and thread projections. |
| [realtime/topics](base/src/realtime/topics.rs) | The topic feed: the topic as the caller sees it, by the topic's own sequence. |
| [realtime/tree](base/src/realtime/tree.rs) | The organization feed: the nodes of a root organization's tree that changed, cut down to the ones the subscriber may view. |
| [realtime/user](base/src/realtime/user.rs) | The user feed: the rows of a person's Inbox by its sequence, and the news that is no row. |
| [realtime/environment_watches](base/src/realtime/environment_watches.rs) | Environment measurements and directory watches shared by transient feed subscribers. |
| [realtime/machine_views](base/src/realtime/machine_views.rs) | Current machine-backed view projections with the same permissions and personal cuts as their HTTP reads. |
| [realtime/ephemeral](base/src/realtime/ephemeral.rs) | Transient feed delivery, shared watch ownership and current whole lists. |
| [realtime/watches](base/src/realtime/watches.rs) | Who may follow the ephemeral feeds of an environment, decided by the environment's own page: its processes and its load by `terminal.view`, a directory of its workspace by `files.view`, and none of them through a thread; and diagnostics. |
| [realtime/usage](base/src/realtime/usage.rs) | Live model requests projected under each subscriber's current usage scope and record rights. |
| [realtime/terminals](base/src/realtime/terminals.rs) | Terminal sockets: one screen at a time for a shell that runs on without one, and the replay it is taken up with. |
| [backplane](base/src/backplane.rs) | Gateway authentication, machine presence and command/event routing. |
| [backplane/report_sources](base/src/backplane/report_sources.rs) | Session sources checked before stream ordering, effects and cursor writes. |
| [backplane/report_refusals](base/src/backplane/report_refusals.rs) | Body-free machine refusal counts and hourly system audit publication. |
| [backplane/session_source_tests](base/src/backplane/session_source_tests.rs) | Session source refusals through the API's real link and database. |
| [backplane/machine_report_tests](base/src/backplane/machine_report_tests.rs) | Foreign environment, harness and host reports are dropped through the real link. |
| [backplane/binding_tests](base/src/backplane/binding_tests.rs) | A locally discovered subscription is registered once across repeated discovery and reconnects. |
| [dispatch](base/src/dispatch.rs) | Durable commands and redelivery. |
| [thread_engine](base/src/thread_engine.rs) | The thread engine inside the API: its commands go through the link's dispatcher, a machine's events are applied by it, and it is built once at start. |
| [hosting](base/src/hosting.rs) | What the engine asks of this program when it makes a unit for an environment: the token its connector enrolls with, kept sealed in the system's store, the address and the authority it trusts, and the builds of the programs it runs. |
| [thread_access](base/src/thread_access.rs) | What the thread engine asks about resources, answered by the access engine. |
| [thread_markers](base/src/thread_markers.rs) | The lines a person's change of a thread leaves in its history: a new model or effort, another setting of its own, its page, its first owner. |
| [tickets](base/src/tickets.rs) | Session, lease and terminal ticket issuance. |
| [blobs](base/src/blobs.rs) | Authorized blob access and pinning. |
| [uploads](base/src/uploads.rs) | Browser and machine uploads in chunks sized to the link, which go on where they stopped. |
| [content](base/src/content.rs) | What machines upload to and download from the store of content over the internal listener: pieces that go on where they stopped, checked against their hash, and only for commands that are open. |
| [media](base/src/media.rs) | What a file is, from its name and its first bytes: its media type, whether its content is text, and a name made safe to put in a path. |
| [previews](base/src/previews.rs) | Bounded static file previews cached by content and served only through their authorized owner. |
| [cache](base/src/cache.rs) | Offline manifests, per-environment eviction and blob garbage collection. |
| [documents_upkeep](base/src/documents_upkeep.rs) | The documents' scheduled jobs: a version of each document that was left alone, and the daily purge of what no document needs any more. |
| [recordings](base/src/recordings.rs) | The last output of a person's shells put away in the store of content with the values of the environment's secrets taken out, and read back. |
| [certificates](base/src/certificates.rs) | Enrollment, machine identity checks and renewal endpoints. |
| [openapi](base/src/openapi.rs) | Supply realtime schemas to the API-contract OpenAPI renderer and cache its served bytes. |
| [routes/browsers](base/src/routes/browsers.rs) | Browser requests and the whole current list shared with the unretained browser feed. |
| [routes/displays](base/src/routes/displays.rs) | Display requests share the same safe current rows with the unretained display feed. |
| [realtime/browsers](base/src/realtime/browsers.rs) | Browser sockets share one live tunnel and let each viewer draw only its newest picture. |
| [realtime/displays](base/src/realtime/displays.rs) | Display sockets terminate viewer security and filter every input under current authority. |
| [realtime/rfb_filter](base/src/realtime/rfb_filter.rs) | Bounded RFB client framing keeps watching sockets from sending actions. |
| [routes/nodes/color](collaboration/src/routes/nodes/color.rs) | Node icon palette validation and the colour a new place inherits from its parent's settings. |
| [routes/nodes/tree_docs](base/src/routes/nodes/tree_docs.rs) | Reading the document rows of many tree nodes with a fixed number of access statements. |
| [routes/resources/machines/reenrollment](resources/src/routes/resources/machines/reenrollment.rs) | Issuing a machine's replacement enrollment token to its owner or the managers of its host. |
| [certificates/reenrollment](base/src/certificates/reenrollment.rs) | Tokens that enroll an existing machine again and certificate handover without replacing its row or registrations. |
| [backplane/reenrollment_tests](base/src/backplane/reenrollment_tests.rs) | Replacement enrollment tests against a real database and the session authorization path. |
| [retained](base/src/routes/environments/retained.rs) | Kept environment metadata and guarded restore or permanent removal requests. |
| [trace](base/src/routes/trace.rs) | Permanent audit rows projected with current content and restoration authority. |
| [inspection](base/src/middleware/inspection.rs) | Explicit retained-content reads reuse restoration guards and never mark anything read or notify anyone. |
| [input_schedules](base/src/input_schedules/mod.rs) | Guarded future-input definitions, private drafts, safe occurrence views and the API's due sweep. |
| [input_schedules/timing](base/src/input_schedules/timing.rs) | Civil schedule occurrences with one earlier fold instant, explicit gap skips and no paused-date catch-up. |
| [input_schedules/runner](base/src/input_schedules/runner.rs) | Current-authority due claims atomically create ordinary work and report each civil occurrence once. |
| [threads/native](base/src/routes/threads/native.rs) | Current thread viewers read an existing native session's effective commands with a fresh return guard. |
| [threads/calls](threads/src/routes/threads/calls.rs) | Authorized recovery reads and explicit retry-now or stop controls for a thread. |
| [routes::loss](base/src/routes/loss.rs) | Permanent-loss previews and typed confirmations under fresh resource authority. |
| [routes::environments::recovery](environments/src/routes/environments/recovery.rs) | One reviewed batch independently moves, re-creates or switches the selected environments. |
| [routes/resources/host_delete](base/src/routes/resources/host_delete.rs) | Reviewed host deletion runs independent recovery or removal before detaching native resources and revoking its machine. |
| [routes/resources/host_delete/data](resources/src/routes/resources/host_delete/data.rs) | Host deletion gives each existing data source its own fate before physical erasure. |
| [routes/resources/provider_recovery](base/src/routes/resources/provider_recovery.rs) | A provider's existing re-create control reviews lost runtime provisioning without borrowing its owner's destination authority. |
| [routes/resources/library](base/src/routes/resources/library.rs) | Resource pages read the same batched library figures as inventory rows, after ordinary View checks. |
| [routes/everything/library](base/src/routes/everything/library.rs) | Library views, privacy projections and recorded-use ordering share the inventory's single read. |
| [certificates/frontend](base/src/certificates/frontend.rs) | The web edge's renewable certificate, issued by the existing authority and published as one complete pair. |
| [routes/admin/programs](admin/src/routes/admin/programs.rs) | Paged agent program administration, independent release selections and localized declaration capabilities. |

### Tests

Some of the test files, and what they check.

| Test file | What it checks |
|-----------|----------------|
| [tests/thread_approval_items](base/tests/thread_approval_items.rs) | A native question arrives with its item, personalized for its reader, and is answered in place. |
| [tests/thread_child_windows](base/tests/thread_child_windows.rs) | Busy child streams preserve the conversation's first window and page only their own history over HTTP. |
| [document_containers](base/tests/document_containers.rs) | Distinct document folders, native permissions, complete operations and replay over HTTP. |
| [tests/provider_sources](base/tests/provider_sources.rs) | Provider managers choose registered detail sources with write-only secrets, current policy and revision, and see each member's state. |
| [tests/holder_model](base/tests/holder_model.rs) | Visible locator results, cross-place roles, nested groups, cycles, depth, renames, membership moves and Reaches chains over HTTP. |
| [tests/workspace_source](base/tests/workspace_source.rs) | Source publication and observed work are served through current environment and thread doors. |
| [tests/realtime_origins](base/tests/realtime_origins.rs) | A real socket inspection records the trusted-chain or direct-peer origin and browser, ignoring a forged first hop. |
| [tests/people_scale](base/tests/people_scale.rs) | People pages and discussion jobs keep bounded batches, exact replay and current reading at scale. |
| [tests/people_notify](base/tests/people_notify.rs) | Explicit notices keep recipient authority, anchors, replay and grants together. |
| [harness_checks::channels](base/tests/harness_checks/channels.rs) | Release channels stay silent until effective policy or an explicit refresh asks for them. |
| [first_steps tests](base/tests/first_steps.rs) | First steps are served from readable stored facts and save only the caller's portable choices. |
| [machine_enrollment_home](base/tests/machine_enrollment_home.rs) | Actual enrollment and resource routes preserve a person's home, ownership, private access, environment registration and explicit administrator marks. |
| [tests/displays_support](base/tests/displays_support/mod.rs) | Display clients use the real shipped-image peer and the same guarded API fixture as browsers. |
| [trace_readers](base/tests/trace_readers.rs) | Real HTTP and database tests of permanent document scope, deleted inspection and stale deletion selection. |
| [input_schedules tests](base/tests/input_schedules.rs) | Real HTTP future inputs keep drafts private, use current authority and queue each occurrence once. |
| [native_actions tests](base/tests/native_actions.rs) | A cold native catalogue is truthful and reveals nothing without current thread View. |
| [trace_history](base/tests/trace_history.rs) | Permanent HTTP trail and surviving snapshot bytes after completed private references are collected. |
| [trace_walk](base/tests/trace_walk.rs) | A real scripted harness calls an Orch tool and a person follows its frozen responsibility into retained HTTP history. |
| [audit_emitters](base/tests/audit_emitters.rs) | Every public audit action is backed by a linked production writer. |
| [audit_tolerant](base/tests/audit_tolerant.rs) | Audit readers retain thread creation and unfamiliar stored actions without losing the rest of the trail. |
| [admin_mode](base/tests/admin_mode.rs) | One explicit session mode, bounded administrative reach and consistent ordinary/elevated provenance over HTTP. |
| [private_documents](base/tests/private_documents.rs) | A page manager without content access sees only a private document's identity, with independent name paging. |
| [call_recovery tests](base/tests/call_recovery.rs) | Recovery is readable with the thread, controllable only by its drivers, and governed by validated inherited settings. |
| [tests/search_meaning](base/tests/search_meaning.rs) | Best matches joined with results by meaning: a text in another language found by what it means, a result both orders rank first, nothing the reader may not read, and plain results by words where the org does not search by meaning. |
| [tests/semantic_admin](base/tests/semantic_admin.rs) | Search by meaning in Management: where each org's index stands, for system admins alone, and the provider Local embeddings made at home in an org from its template, with its one model offered and the value that names it. |
| [tests/semantic_embeddings](base/tests/semantic_embeddings.rs) | The embeddings an org names for search by meaning: a setting whose writer answers for its use, texts made into vectors in the provider's runtime at the named length, counted under the purpose `embeddings`, and refused with the reason when the writer may not use the provider. |
| [tests/semantic_index](base/tests/semantic_index.rs) | The index of meaning of an org: its text cut and made into vectors in the background, newest first; a pause with its reason before the disk share is passed; a change of model that builds a new generation while the old one answers, the same model served elsewhere and a shorter length that need none; and without the vector extension, everything else working and the reason said. |
| [resource_resume tests](base/tests/resource_resume.rs) | Lifecycle Enable continues exact resource-stopped work as its original author before preserving queued work. |

### Named in code

| Name | What it does |
|------|--------------|
| `routes::search::stream` | One cancellable stream of search parts, source progress and authoritative reciprocal-rank orders. |
| `routes::search::words` | Bounded indexed word windows with exact current permission frames and server-side field and occurrence ranking. |
| `routes/storage/cleanup.rs` | Shared cleanup previews, real fates and one confirmed durable operation |
