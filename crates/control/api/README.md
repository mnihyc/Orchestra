# orch-api-core

Related lists share weighted ranking, current reading access and generation-bound whole-item vectors.
`tests/related_items.rs` covers their HTTP behavior; `tests/related_scale.rs` measures the real route and agent
tool on an explicitly supplied isolated native corpus without model calls.

A document's Links section (`routes/links`) lists what refers to it and what it refers to from the store of typed references, only through what the reader may read, with sealed cursors; `tests/links.rs` proves over HTTP that a source the reader may not read is in no list, count, cursor or graph, and that superseded thread history is never a source.

API application hosting HTTP, realtime and the machine backplane.

Resource choices carry batched applicable provider caps, seven-day performance, anonymous environment
activity and observed host load. Thread parameters include provider details only with that provider's
own View permission. `tests/resource_facts.rs` verifies those cuts and observed zero counts; host and
provider list tests retain their constant statement-count checks.

Host managers can remove either kind of unit through the same host-runtime route. Provider removal
commits the exact unit and volume choice into its existing lifecycle job; it preserves the provider
and retires only the selected unit. `tests/hosts.rs` covers host authority, exact confirmation and
both volume choices independently of provider ownership. Machine calls follow the committed request.

`tests/realtime_switches.rs` subscribes once to a real thread socket, observes a switch job's
hold, progress and completion, then changes provider through the engine and requires the new
binding and next completed turn on that same connection. No reload or reconnect repairs the page.

`tests/realtime_daily_work.rs` subscribes before native approval/question creation and observes
both the pending card and its decision on two API processes. Separate socket cases require a
delivered input to leave its sending state, a rename to replace the tree name, and ThreadCreate
to insert the new tree row. Frames retain complete authorized thread views and continuous cursors.

Provider GET/list projections use stored reports and already observed tool catalogues; they never
refresh credentials, inspect the host or hash release files. Creation returns its committed snapshot
before source reads, with subsequent observations over the live feed. `tests/provider_read_latency.rs`
holds all native/catalogue answers and requires GET/list inside 100 ms; the test is authored, not a
measurement claim. `tests/threads_start_source.rs` covers the new-environment/source/first-message
flow and atomic rollback of a refused first input. `tests/notification_exceptions.rs` covers personal
manual exceptions, true ancestor defaults, current View/privacy, bounded pages and the bell's shared
subscription mutation returning the recomputed page.
Node Access also projects the strict ancestor notification level for Default; changing a personal
exception echoes its requested page boundary so the web cache applies the actual page without a read.

Saving a provider key commits before asking its facts/model sources, then returns the fresh Provider
with suggestions and source errors; it does not move either location or accept models. Setup also
names usable profiles whose sign-in declarations are still unread, so disabled examples have a reason.
Setup also retains public system program names with no usable profile and usable profiles that
explicitly lack subscription support. Neither installed processes nor kind names imply native support;
private profile names remain outside this catalogue. Both wizard pickers use this same availability.
Suggestion acceptance keeps reported display names and refuses collisions atomically instead of
silently replacing them with an empty name. API/component regression sources cover these acknowledgements.

The Resources landing counts every discoverable kind through the inventory's native access frame in one stored-state read,
without loading rich resource pages or contacting a runtime. Real-route tests cover all kinds and private counts.

Archive and deletion settle thread work through the lifecycle engine, with ordinary restore returning archived and starting no turn. Confirmed environment deletion settles current bindings, and home deletion queues owned cleanup; integration tests cover current authority, rollback, side closure and replay.

Serves authenticated HTTP requests, realtime feeds and the machine backplane for the product. Runtime changes are planned, checked again and confirmed as durable jobs, individually or in bounded batches; fleet recycling and host maintenance require their own management permissions. Integration tests exercise request behavior and permission refusals against PostgreSQL.

Permanent audit rows capture event-time names, paths and original turn responsibility. The existing deleted-content readers require restoration authority, and audit lists and CSV/JSON Lines exports share kind and person filters. Managed retained-file jobs and their request shapes are implemented; complete catalogue coverage, restricted-network restoration, connected registration restoration and the host's kept-file rows remain integration work. Fresh databases use the complete initial migration.

The finder searches reachable name candidates in bounded batches and reports partial answers. Names and Everything share literal words/quoted phrases, relevance before decaying personal visits and activity, and one canonical web-address builder (`routes/destinations`). Name rows carry their current state and activity; stars and visits cover every searchable kind. Content search resolves access before matching current conversations, chats and documents, supports scope, kind, author and date filters, and returns stable pages with Unicode excerpts; HTTP tests cover privacy and large datasets.

Content search consumes the same inventory access projection inside its SQL statement, inheriting ordinary thread decisions from topics and hydrating only the returned page. `sort=relevance` offers readable place names and document titles before bodies, with the common name tiers, personal visits and activity; newest-first remains the default. Cursors bind the query/order and freeze visit decay. `tests/search_projection.rs` adds real-route ordering, permission/lock/focus parity, cursor-clock and 20,001-thread regressions without changing the existing access matrix or search assertions.

Readers can start document discussions; their owners and the block's last writer receive one renewed notice, while a personal bell adds every discussion or keeps replies to those involved. Request tests exercise recipients and personal state, and a real webhook case checks duplicate delivery and loss of document reading.

Binding choices are available on a topic, project or folder before any thread exists. Startup harness discovery, initial-facts readiness, repeated local sign-in discovery and agreement between offered arrangements and binding placement are tested against PostgreSQL and real containerized harnesses.

Binding previews read stored reports and batch their defaults and holder names on one connection. Debug phase timings distinguish pool waits from projection work. `tests/binding_readiness.rs` covers missing reports without a probe, a single-connection preview, concurrent open pages and writes, and resource measurements that release page connections and action locks while waiting for the host.

Thread parameters also carry the native approval modes offered by the declared harness, intersected with place policy and actual driver confinement. The composer and Parameters use this same stored-facts projection and the guarded setting write; request tests cover a viewer's refusal and the confinement gate without starting a turn or probing a machine.

Harness questions travel with their conversation items, personalized for each reader at HTTP or live
delivery. Answering uses the ordinary guarded approval act; the shared feed keeps no reader's decision
authority. The Outline names turns, messages, milestones and requests, with titles read together. Initial
sync preserves both the main conversation and recent child activity, and child history flags count only
that stream. Regression sources cover these readers, manager/viewer decisions and busy child windows.

File references require the caller's retained upload or copy, or a currently readable saved source. Chat drafts retain their authority and attachment pins in their save transaction; private draft downloads require that authority and remain available through blob collection. Real database, HTTP and blob-store tests cover ownership, copies, duplicate bytes, legacy references and pin release.

Partly implemented. Tree summaries count following documents and include icon colours; node managers
change colours, new places resolve a colour setting, personal state includes the sidebar, and a machine
can enroll again with its identity and registrations retained. Real database and socket tests cover these changes.
Action transactions that load their caller recheck current credentials, scopes and declared permissions through
`middleware/transaction_guard`; handlers can also begin their transaction with that guard directly.
Every changing request has one row in the guard's ledger (`middleware/guard_ledger.json`): guarded, open with
why, or separate for a request without a signed-in caller; the registry refuses, in a build with debug
assertions, a guarded request that answers without checking its guard, so every test that serves it proves it.
Ordinary actions share the authority gate; the registry marks authority writers so their transaction locks
it exclusively before any other statement. Database tests cover authority lost during body decoding,
revocation races, concurrent actions and the guard's statement and latency costs.
Document writes, thread read positions, pins and drafts, channel and destination actions, recents and usage
writes use that guard in their action transaction. Real database tests cover delayed bodies and lock waits,
with successful controls and webhook samples delivered to a local HTTP receiver (`tests/kguard_content.rs`).
The binder with its test client, the middleware that finds out who asks and
puts every decision to the access engine, signing in, the routes of the hierarchy (nodes, access, settings,
users and invitations, the org directory, ownership and a person's own state), the routes of threads and
approvals (a thread made, read, driven and exported, its parameters, what a harness asks and its answer) and
the routes of resources (a person's machines with their local policy, agent profiles, providers on a key with
their models and the page that adds one, templates and hosts as data, the permission page of any resource, and
environments that are a directory on a person's machine) are in place, and so are those of documents (made beside a node from blocks or
from Markdown, listed, read whole or from a sequence, changed in batches that are applied whole or not at all, their versions and what
changed between two, discussions, moving, deleting, the permission page, and the export as Markdown, HTML, Typst and JSON), and so are the
files a document holds: put in from a body or from a finished upload under the limits of the document's place, kept as long as the document holds them,
read by range (pictures, films and sounds inline, everything else as a download under its own type) by whoever the document's page lets read it, and the
entries of an archive among them listed one directory at a time and read one at a time without unpacking the rest (`routes/documents/files`, `archives`). A thread is made out of another by a plan and a job
(`routes/forks`): the plan of a fork or a switch says how the history travels and what of it is kept, who will own the result, what
stands still, what is left behind and everything that stops it, with what the caller lacks named, and is good for fifteen minutes;
carrying it out makes a job and, for a switch, holds the thread in the same transaction (`Thread.lock`). Several threads are switched
together, each by a job of its own that succeeds or fails alone, and a batch is read by its creator or by whoever manages one of its
threads, who sees the jobs of the threads they may view and a count of the others, and is cancelled by its creator. A side thread is
started by one request and closed by deleting it; whatever it does not take is `thread.side`. They are tested over HTTP against a real
PostgreSQL with a scripted machine (`tests/forks.rs`). The routes of chats are in
place (a node's chat and the direct and group chats of a root org: messages with their replies, edits, deletes, reactions
and pins, the list of an org's chats, members and guests, a person's levels, positions and drafts, quotes of a thread's
items, forwarding to chats and people, and the audited reading of an admin), and so are the files of a chat: a file stored to be named by a
message, from a body or from a finished upload under the limit of the chat's place, and an image or a file served to whoever reads the chat
from the messages of that chat that carry it, forwarded copies among them and nothing else the store holds (`routes/chats/files`). The Inbox and the ways out of the
application are in place: a person's notifications newest first with their counters, what they read and what they chose
about being told, the browsers that take push, the kinds of notifier there are, the channels of the system, of a root org and
of a person (checked against their notifier and the far side, their secrets sealed and returned by nobody, tried on request),
each person's routes through them, off until switched on, with the link of a bot, and the destinations of a node. The notifiers
are given to the requests once, by the program that composes the application (`routes::notifications::install_notifiers`). The workspace of an environment is served too, and is described below. Every
bound request is tested over HTTP against a real PostgreSQL, in `tests/`; the thread tests stand a thread engine
behind the application that talks to a scripted machine through a hub, so that a thread goes the whole way from
its making to the agent's answer. A key, a secret header and an environment's secret are sealed before they are
stored and are returned by no request.

Folders are bound: a project is made inside a project (`routes/nodes/make`), moved into another project or into an org, and a topic is moved to another project or folder with the plan that comes first (`routes/nodes/moves`); there the depth of projects one inside another, the tree of the move, the threads a fork, a switch or a move holds and the rows of the ledger that follow the topic are settled. A project counts the projects directly in it. They are tested over HTTP in `tests/folders.rs` and `tests/folders_announced.rs`.

Org list/detail projections also carry optional `people_count`, separate from their domain-member
count: active humans working in the branch through membership or Viewer-or-higher roles, including
inherited/group holders, counted once by the shared native expansion. `tests/org_people_counts.rs`
covers inherited access, sibling exclusion, deduplication, actual collection/detail agreement and a
suspension cut. The source is authored for the integrator's round; old projections omit an unknown count.

What a client that lost its place on a feed reads is made from one list of what the feed's state consists of (`syncs`): a topic with its threads, the jobs about them and its announcements (`routes/topics/sync`), and an environment with the jobs that make or change it (`routes/environments/sync`). A part that gives a feed one more thing to carry adds a member to the list of that feed, which is the only place that names them. The whole is read in one transaction that sees the database as it was when the feed's number was read, and an answer says it is complete only when every member gave everything that changed, so that a client that is told it is not reads again from the start. What a harness reports of its own protocol is listed newest first, a page at a time, to whoever manages the thread (`routes/threads/raw`), and a session that a person began in a harness by hand is taken in as a thread of a topic: found by asking the harness, put in the environment of the caller's whose folder holds the directory it ran in, and checked as any thread is for what the caller may use (`routes/threads/adopt`). They are tested over HTTP in `tests/syncs.rs`, `tests/threads_raw.rs` and `tests/threads_adopt.rs`.

What a thread may be put on is answered by `routes/threads/binding`: every provider, profile and environment the caller may choose, each arrangement with the reason one is not available or not permitted and whose decision that is, the defaults and the access a thread made there would start with; a thread without history is put on something else, a thread with history is sent to a switch, and whoever binds is asked for their own use of what they choose then and never again. What a person's machine has is registered from it by its owner, in `routes/resources`: a profile for a harness it reported it drives, an API whose key is typed on it (or given to the server afterwards), and the login a harness already holds there, which is looked at with the harness before it is kept; the first owner is the machine's owner and stays so, and the provider is used on that machine until its owner allows more. The adapters of the system, the capabilities, the drivers, the settings of a provider's runtime and the threads bound to a provider are listed there too; a provider's credential is looked at on request, facts are written by hand over what it reports, a harness is brought up anew and the conversations it holds are listed. They are tested over HTTP with a scripted machine and, for a thread made on a profile whose harness never ran, a thread whose harness, sign-in and tools are on one machine, and a key typed on the machine, against the connector as a process (`tests/real_connector.rs`).

A provider that Orch looks after is bound too. A subscription or an API provider made through `POST /providers` is given the job that makes its runtime (a request that names a method to sign in by begins it as soon as the provider exists), the page of a provider shows its runtime (its unit's state, host, template and generation, what it is behind in, when it will be stopped for being idle or started to renew a sign-in, and the job at work on it), and a runtime is started and stopped by hand (a stop is refused while a turn runs on the provider unless the request says to interrupt it). Signing in is bound (`provider_login`): the method and its values are checked against the fields the profile's adapter declares, every fault named by its field; what a person gives for a secret field is sealed the moment the request has it, in a row of its own, and is returned by no request, no feed and no log; what the harness shows (an address, a code, a command) and asks for comes back as data; an answer is sent back; a sign-in is given up; a harness is signed out; and a sign-in by another method replaces the old one in place. A sign-in whose runtime is not up waits for it sealed and is forgotten with its secret if the unit cannot be made. A credential that was handed over to an environment is brought home, or given up, by `POST /providers/{id}/handover/return` (whoever manages the provider, or manages the environment that holds it; refused while a turn of that environment runs on the provider unless the request says to interrupt it). They are tested over HTTP with a scripted machine that answers as a harness does (`tests/provider_managed.rs`). The provider resource feed carries committed facts and runtime changes; ordinary page/list reads use those stored observations.

Environments made from a template on a host are bound: a request makes one and is given the job that builds its unit
(a named host that cannot take it is refused, with why), a job is read with how far it has come and is stopped by whoever
manages its environment, the places of an org, a project or a topic list the hosts the caller may use with the reasons
one cannot be chosen, an environment is started, stopped, kept awake or kept from a removal that has not begun, the
environments of a topic and the one that new threads join are read and set, the fleet of a place is listed, and a host's
manager sees what runs on it and takes a unit away. The connector inside a unit enrolls with a token that only its unit
can use, sealed in the secrets directory and handed to the node sealed to its key (`hosting` is what the engine asks of
this program to make a unit). Recreate, update, workspace move and recycle requests are bound too, including
fleet batches and host maintenance, with their plans and permission checks covered in `tests/runtime_updates.rs`. They
are tested over HTTP against a real PostgreSQL (`tests/hosted_environments.rs`, `tests/certificates.rs`), with a
connector program that serves a connected environment (`tests/real_connector.rs`), and against a real Docker daemon in the
node's tests (`apps/node/tests/environment.rs`), where a request makes a container whose connector comes up and a thread
bound to the environment runs a turn.

Management is bound. A system admin in a browser session lists the people of the system with the proofs that
strangers have locked at their names, makes a person with a link to set a password, suspends and lets in again (a
suspended person's sessions and tokens stop at once; the last system admin stays), makes and unmakes system admins and
gives a reset link that may switch the second step off; lists every org and sets the two limits of a root org; reads and
sets the settings of the system with where each value comes from, and reads, changes and puts back the defaults page of
the system; reads the audit trail of the whole system and writes a span of it out as a streamed file; reads how the
system stands (its parts with their heartbeats, its queues, the blob store, its certificates and the document of its
requests); lists hosts, both pools of runtimes, the measurements the system stored about itself and the plugins; keeps
the definitions of capabilities, the builds of harnesses and the agent profiles that belong to no org; and keeps the
single sign-in methods. The files of a release (a connector, the installers and the manifest) are served as they are to
anybody, from the directory of the release's payload. One request is not bound: testing an identity provider waits for
the single sign-on extension. The preview of a change to the system's defaults is bound in `routes/access/previews`. Each bound request is tested
over HTTP, in `tests/admin_*.rs` and `tests/downloads.rs`, together with the refusal of somebody who is no system admin,
of an org's admin, and of a token.

The requests that are the same for every kind of thing (`/{object}/{id}/…`) are bound in `routes/access`, for a node, a resource of any of the seven kinds and a document alike: the permission page, whose object says what kind of thing it is of; what was done to the thing (its history, newest first, for whoever may view it; for an environment also what was done in it, asked for by kinds); what one person holds on it and through what; the people behind each holder its page shows; the preview of a change of a page, of the defaults of a place and of the system (how many pages it reaches and who gains and loses what, named as far as the caller may know them, the change made inside a transaction that is never committed; the page of a place is one page and opens nothing inside it, the defaults of a place reach the pages inside it that follow); and the hand-over of a resource or a document to another person (`routes/transfer`). A refused person asks for what they lack with one request: it becomes an approval for each thing asked for (the thing that refused them, never the place in its stead), each sent to the nearest people who can give it, who are told, and decided by any of those who could have given it; the person who asked is told of each decision and once more when all they asked for is held, and a request nobody answers runs out. A page tells beforehand what its reader may give: `can_give` on each row, `gives` on the page (the choices, the roles, the levels for agents, and whether the place lets holders share), and on a place `covers`, the places, threads and documents inside it that follow it and those made private or cut loose, as far as the reader may see them; a role named on a page comes with how many people it stands for. Who may share is one rule for the page, the roles, a share with agents and the decision of a request: a holder shares what they hold where the place lets holders share (`sharing.allow_holders`), whoever manages the page does the rest, and an entry, a role and a share each say who gave it. A person with as many requests waiting as the policy allows is answered `409 access.too_many_requests` with how many may wait and how many do. A chat has no permission page, and every one of these requests answers a chat, and a word that names no kind of thing, as not there, never as a fault. They are tested over HTTP in `tests/access_*.rs`, `tests/giving.rs`, `tests/reaches.rs` and `tests/manage_is_the_thing.rs` (deleting, bringing back and moving each kind of thing is its Manage and no lower level).

The usage requests are bound in `routes/usage`: figures, series and breakdowns at the depth the reader's access gives (their own use in full wherever it was, a place's totals with the row `usage.summary`, every record with `usage.view`, what ran on a provider with `provider.view`), the records behind a figure, the rate table of a node and the shipped catalogue with its replacement by a system admin from a file, from the address they set or from the release, caps made, changed and removed by whoever manages the place they are defined on (naming models, a group of a provider's models, and the service tiers they cover), the limits at a node, a person's own share of the caps counted per person and the queue for room in a cap; the series and the records are answered as CSV when the reader asks for it. Credits are exact decimals marked as an estimate where the shipped catalogue gave the rate, and tokens without a rate are counted with none. They are tested over HTTP in `tests/usage_*.rs`.

Sharing with agents is bound in `routes/share`: a thing, a place with what is in it or a place by itself (`inside: false`: its own chat and the documents beside it, never a conversation) shared with the agents of threads or places as a live share or a copy, each share saying which it is, the shares of a node, a document and a chat and what names the agents of a place or a thread, a share changed and taken back, and the tree of an agent's reach as the agent lists it, with the names the person may not see left out, each entry saying whether the agent may manage it (delete, move, bring back) and whether it is a document that was deleted and that the agent may bring back (seen by whoever holds `doc.manage`, as the agent sees it). A share of documents can be at Manage as well as at Read and Edit, given by whoever holds `doc.manage` there and refused to anyone else with what they lack (`tests/shares_manage.rs`), and an agent on a real connector deletes a document it was shared to manage in two calls, the second with the word of the first (`tests/real_connector.rs`). A copy is refused until something is attached that keeps its files. A thread is exported in its written form (`routes/threads/written_export`), and the items of a conversation are forwarded into a chat as copies in that form (as live quotes into the chat of their own thread) and to an agent's thread as an input of the forwarder's, all in one transaction (`tests/shares.rs`, `tests/forwarding_threads.rs`).

Taking things away as files is bound in `routes/exports`, `routes/chats/export` and the single exports of a thread and a document: a thread (`routes/threads/export`, `written_export`), a document (`routes/documents/export`) and a chat (`GET /chats/{id}/export`) each ask for the Export row of their kind besides reading, and a chat takes its files along as a zip when it is asked to; a direct or a group chat is taken away by those who are in it and by nobody else, so that somebody who only inspects one as an admin reads it and does not take it away as a file (their `can.export` is false and the request is refused `chat.export`); the archive of a place (`POST /nodes/{id}/exports/preflight` and `/exports`, `GET /me/exports`, `GET` and `DELETE /exports/{id}`, `GET /exports/{id}/download`) is planned, asked for as a job that streams a zip into the store of content and pins it for the days `export.keep_hours` says, and is its person's alone to read, fetch and remove (the job too: for anybody else it is not there). A thing in an archive is the very bytes of its single export, because the forms that need the views of the API are written by one function that both call (`routes/exports::ApiForms`, `routes/documents/export::produce`); the Markdown of a document with files is a zip of the text and of them, and links each file by the path it lies at beside the text, `<title>.files/<name>`, as an archive does. A conversation exported by itself names its files as `conversation.files/`, as it does where it lies as `conversation.md`; it comes without them. A refusal says what it comes to: `export.nothing`, `export.too_many` and `export.too_large` with the limit and the figure, `429` past the archives one person may have in the making (it says how many are being written: two as shipped), `410` for an archive that was removed. An archive whose place was purged names no place (`node` is null) and stays its person's until its time; one whose person is gone is removed by the next sweep. They are tested over HTTP against a real PostgreSQL in `tests/exports.rs`, `tests/exports_limits.rs`, `tests/chats_export.rs` and `tests/export_rules.rs` (two people at a time: a reader without the row gets no file by any way and still reads, a choice carries the row as far as its giver holds it, an archive counts what may only be read).

The browser's socket is served at `/api/v1/ws`: a connection says who it is with the access token of its session and
renews it before it runs out; it follows the feeds of a thread, of a root organization's tree, of a topic, of a document, of its
person's own Inbox, which carries the two counters of its badge, and of a chat (with who is looking and who is typing), each decided by the access engine when it is asked for and again whenever the authorization epoch
moves; the rows of a durable feed are sent by sequence from the database, read for every announcement and by a sweep for
the ones that were lost, so that any number of API processes serve the same feeds; growing text arrives at once from the
engine and is merged for a reader that is behind; and a connection is closed for not keeping up only by the two buffer
keys. A document's feed carries its blocks, its discussions and who is writing where, and tells who has it open. The feeds of
environments, resources and watches wait for their data and are answered `not_found`. It is tested on a loopback socket with a real client, a real database, the thread engine and a scripted
machine, in `tests/realtime_*.rs`.

A person's shells are served by `routes/terminals` (open one in an environment, with the gates, the limits and the choice of which of their own to give up; list them; a ticket
to attach again; close; what a closed shell last printed; and the ticket of the agent's own terminal) and by two sockets of their own, apart from the one every tab holds,
in `realtime/terminals`: `/api/v1/ws/terminals/{id}` and `/api/v1/ws/agent-terminals/{id}`. A socket is entered with a ticket that is good for thirty seconds and once; it
carries bytes both ways, sends the shell's last output first, reads from the machine only while the browser is less than a mebibyte behind, gives a shell to one screen at a
time (the screen that had it is told it went elsewhere), and tells why it closed. A task of the realtime server ends the shells that nobody came back to, those of an
environment that stopped, and those whose opener lost what they were opened by on every move of the authorization epoch. They are tested over HTTP, on loopback sockets with a
machine that keeps shells (`tests/terminals_*.rs`), and with a real connector process and a real WebSocket client (`tests/terminals_connector.rs`).

**The workspace of an environment** is the environment's: its files and its shells are two of the four doors on the
environment's own page (`files.view`, `files.manage`, `terminal.view`, `terminal.manage`, and beside them
`browser.*` and `vnc.*`), and whoever manages the environment
holds the Manage of all four without being named in them. A thread a request names says beside which one the
caller works and gives nothing: the request's declaration holds the caller to the permission on the environment
before a handler runs (`workspace/door`), and a caller who holds everything on a thread and nothing on its
environment is refused with what is missing named on the environment. Who reaches into an environment, by which
door or through which agent they may talk to, is `GET /environments/{id}/reach` (`environments/reach`), and a
thread's page says what it reaches, how many people read its files whatever the page says, and what sharing it
offers beside it (`access/thread_environment`). An environment is read by whoever holds a permission on it, all of it,
and by whoever holds none and may view a thread that runs in it, as far as its name, owner, place, state and the job
under way (`environments/reading`: `decide` says who and how much, `read` is the request's whole). Every request
(files listed, described, brought to the store and read in ranges, written on the version the writer saw, made,
renamed, copied and deleted; the changes agents made; a search by name and one inside the text; what an archive
holds and one of its entries; the state and the differences of a repository; snapshots with their files and their
archives, renamed and deleted; the processes of the environment and their signals; and its measurements) is a
command to the machine after the caller's right was checked, and a refusal of the machine is answered as the problem
it means. The machine is asked only where the environment stands attached in the session it has, which the engine
sees to (it attaches an environment in each session of its machine, and says so in the environment's row); this part
attaches nothing, and asks a machine, once for each session, for the measurements of an environment it holds. The
jobs of the workspace are read and cancelled by the same two requests as every other job (bound with the
environments), which ask this part who may for a snapshot and for files on their way into a workspace. A machine
that is away, or an environment that does not stand attached, answers from the last list of its tree and says how
old that is, and from the files that were read before. The store of content is in the application state (`blobs`): the routes of the
internal listener that machines upload to and download from (`content`: pieces that go on where they stopped, a
hash that is checked, only for commands that are open for the machine), the uploads of a browser, whole or in pieces
(`uploads`), what a thread carries served by its hash, and the housekeeping that lets go of content nobody wants
after its grace, trims the cache of an environment from the content asked for least lately and removes uploads that
were given up (`cache`). The files of an input are written into the thread's directory before its turn begins by
the engine, and the last output of a person's shells is put away in the same store with the environment's secrets
taken out of it (`recordings`, which the program gives to the terminals through `recordings::install`). The long
output of a command that the timeline kept the end of is read from the store in pieces. It is tested over HTTP against a real PostgreSQL with a machine that a test plays, in `tests/content.rs`,
`tests/blob_housekeeping.rs`, `tests/uploads.rs` and `tests/workspace_*.rs`, and against a connector started as a
program in `tests/workspace_connector.rs`, and the doors themselves in `tests/doors.rs` (each of the eight rows alone,
a thread opening none of them, the tick and no further, the Manage of all four held by a manager, the shipped rows
by role in both placements, a restricted thread's files). Not served: the live watches of directories and processes
(they belong to the feeds, which are decided here by the environment's rows and carry nothing yet), and a machine's own
list of an archive while the machine is away.

The machine link is implemented in `backplane`, `dispatch`, `certificates`,
`tickets` and `server`: gateways connect over an internal listener that only a
service certificate reaches; a machine's events are applied through an
`EventApplier` in the transaction that moves its cursor; commands are rows that
are sent whenever the machine has a session and carry no secret; certificates
are issued, renewed and revoked; and tunnel tickets are signed. They are
tested against a real gateway in the gateway's own tests and against a database
here.

The thread engine (`orch-engine`) is built once at start by `thread_engine`: its commands go through the dispatcher of the link, it is registered as the applier of what machines say, and it is held in the application state for the routes of threads to call.

Announcements use one place-level path for orgs, projects and topics (`routes/topics/announcements`), delivered by one guarded engine operation. Each topic keeps its delivery settings, private branches are skipped, and lists apply the ordinary topic and thread visibility checks. The shared place settings, tags, member subjects and archive behavior are covered by `tests/uniform_places.rs` beside the announcement and hierarchy tests. Capability grants and their approvals are served by `routes/capabilities` and `routes/approvals` and exercised with a real connector in `tests/real_connector.rs`.

The ports server core is staged: stable targets before listening, reviewed
program configuration, guarded project/place and link admissions, own use,
safe projections and an administrator's outside list. A real PostgreSQL and
pinned-TLS connector walk proves two simultaneous people, stop/start, saved
admission and close. The port routes are served: the carrier's bounds come from
the deployment's `[link]` section and travel to machines with the session's
limits, and every request that changes who may do what closes the admissions
it ended, with their live channels, before it answers (changes made outside a
request are caught within the runtime's quarter-second tick). Agent decisions
and tools and HTTP, helper and host entry compose the admission and channel
interfaces here.

The production engine starts after its configured local blob store is open and attached. Restore
planning releases its guard before native dispatch and content reads, then checks the current authority,
environment, snapshot, settings and visible writers before saving the plan. The persistent-store
regression in `tests/workspace_connector.rs` covers native recording and the restore APIs with only one
pool slot free; native environment tests cover the first recording without retrying new identities.

A turn's stored unavailable fallback can be revalidated on its authorized snapshot read when its
original recording command succeeded. The original protected body, immutable manifest/hash, owning
directory, generation and expiry are checked again; no command is sent and no current files are
captured. Content reads finish before the short metadata/pin transaction, and the reader is checked
again before the answer. The real-connector default restore case exercises this recovery and one
recording command through Snapshot, preflight, confirm and undo.

Usage feed reads omit unbounded time predicates instead of binding Chrono's timestamp extrema;
history dates and cursors are validated before SQL. Harness decisions queue their user notices in
the same transaction as the changed approval, turn and thread, so every API process observes the
committed state. `tests/realtime_usage.rs`, `usage_figures.rs` and `realtime_threads.rs` cover empty
subscriptions, revisions, invalid dates, two open pages, answers and persisted human-wait timing.
`tests/search_content.rs` covers injected-context exclusion before and after legacy projection repair.

`GET /me/bootstrap` combines the shell's existing personal projections on one connection and
principal, without loading trees or chat contents. Administrator mode keeps its ordinary organization
rail. `tests/bootstrap.rs` covers parity with individual reads, isolation and mode.

Machine views subscribe on `/api/v1/ws`: `env:<id>:procs` carries `{as_of, processes}`, `:stats`
carries resource samples, `:browsers` carries browser/profile lists, `:terminals` carries the personal
open and recently closed pages, and `:ports` carries the ordinary port view. `:dir:<base64url path>`
starts with `reset:true`, then entry changes/removals; `truncated` reports the native bound.
`thread:<id>:git` resolves the current binding; `env:<id>:git:<base64url path>` names one explicitly.
Both carry `{environment_id,path,status}`. Subscribe and delivery use the matching GET permissions;
thread viewing never grants an environment door. Hidden views unsubscribe; the distributed coordinator
shares native watches and ends them after its ordinary departure grace. Git uses native file notices.
Server terminal/port commits and job/batch changes wake their own feeds through commit notifications,
without client refetches. `job:<id>` carries `{job}`, `fork:<id>` `{batch}`, with the same private cuts as
their HTTP reads. Socket/connector regression sources are in `tests/realtime_machine_views.rs`,
`tests/realtime_environment.rs` and the environment host's tests; these additions await the integrator.

| Module | Planned responsibility |
|--------|------------------------|
| [sandbox_readiness](src/sandbox_readiness.rs) | Common requested sandbox, actual executor modes, limit sources and authorized policy acts on machine pages and blocked threads. |
| [ports_runtime](src/ports_runtime.rs) | Live port measurements and single-use channel admissions over the existing backplane. |
| [port_tools](src/port_tools.rs) | The four port tools over the thread owner's current environment and authority. |
| [ports_browser](src/ports_browser.rs) | The browser route of a port: a request to its stable name, let in by a person's entry or a link and carried to the program with who is calling. |
| [host_ports](src/host_ports.rs) | A host's ports at the API: its node's check of a declaration under its own ceiling (`host.ports`), and the node's last proposal for the host's page. |
| [backplane::port_records](src/backplane/port_records.rs) | Live whole-row pages from the gateway and host nodes, combined without a connection-history table. |
| [routes/ports](src/routes/ports.rs) | Guarded stable port targets, reviewed wider admissions and safe outside listings. |
| [routes/port_entry](src/routes/port_entry.rs) | Entering a port without the browser route: a link's use, its landing, the raw socket and the public assertion keys. |
| [routes/port_requests](src/routes/port_requests.rs) | A thread's request to let more people reach a port, and a person's decision on its card. |
| [routes/resources/host_maintenance](src/routes/resources/host_maintenance.rs) | Host load history, safe removal of empty hosts and recycling by their managers. |
| [routes/resources/host_storage](src/routes/resources/host_storage.rs) | Host maintenance settings use the shared registry, resolver, locks and revision guards. |
| [routes/resources/provider_updates](src/routes/resources/provider_updates.rs) | Planned updates and recreations of provider runtimes, individually or in bounded batches. |
| [routes/environments/moves](src/routes/environments/moves.rs) | Workspace move plans and confirmations, for the same environment on another managed host. |
| [routes/environments/updates](src/routes/environments/updates.rs) | Update and recreate plans over HTTP, confirmed once and checked again before a runtime is held. |
| [server](src/server.rs) | TLS listeners, startup and shutdown. |
| [machine_requests](src/machine_requests.rs) | Local machine requests carried by its authenticated link and authorized as its current owner. |
| [local_ports](src/local_ports.rs) | Read-only inside port projections and live peer assertions, scoped to the authenticated current environment machine. |
| [state](src/state.rs) | Application composition and shared service handles. |
| [attachments](src/attachments.rs) | Files that people put into chats and documents: taken into the store from a body or from a finished upload, told apart by name and first bytes, and given back under the name and the type they were kept with. |
| [archives](src/archives.rs) | What an archive held in the store contains, listed and read one entry at a time without unpacking it. |
| [syncs](src/syncs.rs) | The state behind a durable feed, put together from one list of what that state consists of. |
| [registry](src/registry.rs) | HTTP registration with permission and token metadata. |
| [sign_in](src/sign_in.rs) | Signing in: the proofs a sign-in takes and what it still lacks; wrong tries counted by the name that was tried, each proof apart and a browser its person signed in from by itself; a code sent by mail; and the answers that tell nothing about a name. |
| [sign_in/accounts](src/sign_in/accounts.rs) | Making a person: the rules a username, a name, an address and a password meet, and the user with the org that is theirs. |
| [sign_in/codes](src/sign_in/codes.rs) | The codes of a second step: the code that is mailed, and the one an authenticator app shows, with the key that seals an authenticator's secret. |
| [sign_in/login](src/sign_in/login.rs) | The sign-in, and the request for a code by mail; the second answers the same to every name. |
| [sign_in/mail](src/sign_in/mail.rs) | The mail that concerns a person's own sign-in: the confirmation of an address, the reset of a password, a code to sign in with, and the notice to an address that it was replaced. |
| [sign_in/fresh](src/sign_in/fresh.rs) | A fresh proof for a sensitive act: the caller's password, or the one-use proof of their identity provider where they have none, with their second step where they have one, all in the request that does the act. |
| [sign_in/passwords](src/sign_in/passwords.rs) | Passwords: kept as Argon2id hashes, compared in constant time, and the rules a new one has to meet. |
| [sign_in/people](src/sign_in/people.rs) | A person as others see them and as they see themself: the avatar, what a mailed code may be for them, and the address they asked for and have not confirmed. |
| [sign_in/proofs](src/sign_in/proofs.rs) | Looking at a proof: a password, a code that was mailed, and a code of an authenticator app, each counted by itself; and what a set of right proofs lacks of a sign-in. |
| [sign_in/recovery_codes](src/sign_in/recovery_codes.rs) | Ten one-use, one-hundred-bit authenticator backups; only user-bound, domain-separated hashes are stored. |
| [sign_in/reset_links](src/sign_in/reset_links.rs) | A reset link made by hand, for a person who cannot sign in and whom no mail reaches: made by a system admin, or by the operator on the host. |
| [sign_in/second_step](src/sign_in/second_step.rs) | The second step an org can require: of whom it is required, where a request waits for it, and what the person and their org's managers are told of it. |
| [sign_in/throttle](src/sign_in/throttle.rs) | Counting wrong tries: by the name that was tried, each proof apart, a browser its person got in from by itself, and one address across all names. |
| [middleware/authentication](src/middleware/authentication.rs) | Resolve sessions and scoped tokens. |
| [middleware/admin_openings](src/middleware/admin_openings.rs) | One audit row for each private object opened through a browser session's administrative reach. |
| [middleware/authorization](src/middleware/authorization.rs) | Delegate decisions to orch-authz, and carry what is missing, and whether the caller may force it, into refusals. |
| [middleware/transaction_guard](src/middleware/transaction_guard.rs) | Transaction-local request authority using current identity, token scopes and the application clock. |
| [middleware/authorization/refusals](src/middleware/authorization/refusals.rs) | What the access engine says, as the problems a client acts on: a refusal with everything that is missing and who can give it, a suspension in an org, and a place that is archived. |
| [middleware/requests](src/middleware/requests.rs) | Request IDs, errors, body limits and rate limits. |
| [middleware/requests/words](src/middleware/requests/words.rs) | Public problem sentences from the same English catalogue as the interface, with diagnostic fields kept separate and both languages checked against every API code. |
| [middleware/idempotency](src/middleware/idempotency.rs) | Transactional replay and revisions. |
| [routes/meta](src/routes/meta.rs) | Health, setup and public metadata. |
| [routes/auth](src/routes/auth.rs) | Registration, signing in by a password, by a code sent by mail and through an identity provider, the second step, sessions, recovery and the change of an address. |
| [routes/auth/me](src/routes/auth/me.rs) | The signed-in person: what they set for themself, their password, their second steps, their sessions and their API tokens. |
| [routes/auth/bootstrap](src/routes/auth/bootstrap.rs) | One shell read of the person, personal state, organizations, mode, favorites, chat state and inbox summary. |
| [routes/auth/oidc](src/routes/auth/oidc.rs) | Signing in through an identity provider: the start and the callback of a sign-in, of a link to the signed-in person and of a fresh proof for one sensitive act, the start of a link and the unlinking of an identity, and the kinds that speak to providers. |
| [routes/auth/oidc/landing](src/routes/auth/oidc/landing.rs) | What an identity a provider vouched for leads to: a person signed in, a person made and signed in, the identity linked to the person who asked, or a one-use proof for one sensitive act. |
| [routes/auth/tokens](src/routes/auth/tokens.rs) | API tokens made narrow and with a fresh proof, within the longest life their places admit, asked again by their key without their secret; and an org's list of the tokens that reach it, each of which it can refuse there. |
| [routes/users](src/routes/users.rs) | Users, and invitations into an org at any level, to one place or the system's: a link to share or a personal link, within what the root org allows. |
| [routes/profiles](src/routes/profiles.rs) | Personal pages, visible pins, work by day and direct shares between people, with recorded inspection of otherwise closed pages. |
| [routes/chats/mentions](src/routes/chats/mentions.rs) | Current readable chat mention choices, filtered before pagination. |
| [routes/chats/asking](src/routes/chats/asking.rs) | Explicit agent asks reuse ordinary chat-source forwarding and the sender's guarded input producer in one transaction. |
| [routes/chats/agents](src/routes/chats/agents.rs) | Human management of a chat's per-origin posting stop, serialized with the agent's own sends. |
| [routes/chats/readers](src/routes/chats/readers.rs) | Current explicit inspection holders for a static reading-access description, with no actual-read event. |
| [routes/documents/creation](src/routes/documents/creation.rs) | Copies and imports use the ordinary guarded creation producer, with current source rights and truthful entry receipts. |
| [routes/documents/references](src/routes/documents/references.rs) | The card a reference's chip opens: what the reference names, and the part it names, as the caller may read them. |
| [routes/links](src/routes/links.rs) | A document's Links section: which tabs it shows, what refers to it, what it refers to, and the graph around it. |
| [routes/links/references](src/routes/links/references.rs) | What refers to a document, with the line around each mention, and what the document refers to. |
| [routes/links/graph](src/routes/links/graph.rs) | The graph around a document: only readable documents, chats and threads, joined by the references among them. |
| [routes/links/placed](src/routes/links/placed.rs) | What a reader is told of the things a list names: each one's name as they may read it, and the places it lies in. |
| [routes/documents/imports](src/routes/documents/imports.rs) | Bounded Markdown and HTML imports with per-entry guarded creation and durable document identities. |
| [routes/nodes](src/routes/nodes.rs) | Hierarchy with orgs inside orgs, a project and a department moved inside their tree, archive, what was deleted and its restore, tags, subscriptions, a place's Activity, members with their domain, moving one and suspending one, and the end of a membership. |
| [routes/nodes/view](src/routes/nodes/view.rs) | A node as the API shows it to one caller: what is stored of it, its way down from the root org with the names the caller may know, and what the caller holds on it, for any number of nodes in the same few statements. |
| [routes/nodes/make](src/routes/nodes/make.rs) | Making an org inside an org, a project and a topic, and a team's root org, each with the Owners row of its maker and the audit row of its making. |
| [routes/nodes/tree](src/routes/nodes/tree.rs) | The nodes of a whole tree for a view of it, what somebody let the caller into by name, and the topics of a project with the filters of a list. |
| [routes/nodes/tree_work](src/routes/nodes/tree_work.rs) | Compact waiting and blocked explanations for the same authorized tree over HTTP and the live feed. |
| [routes/nodes/moves](src/routes/nodes/moves.rs) | Moving a project, or an org that lies in another, under another org of its own tree, and the plan that says what the move would change before anything does. |
| [routes/nodes/moves/review](src/routes/nodes/moves/review.rs) | Stored move-consequence stamps and the ordinary access engine's rolled-back move comparison. |
| [routes/nodes/archive_plan](src/routes/nodes/archive_plan.rs) | Read-only archive consequences and caller-bound subtree fences, including exact inverse reviews. |
| [routes/nodes/removal](src/routes/nodes/removal.rs) | Deleting a node with everything in it, bringing it back until it is purged, and handing a node to another owner. |
| [routes/nodes/activity](src/routes/nodes/activity.rs) | A place's Activity and its audit trail. |
| [routes/storage](src/routes/storage.rs) | One authorized storage report from existing retention records, content metadata and node inventories. |
| [routes/settings](src/routes/settings.rs) | Settings of a node, an environment, a provider and the system; secrets metadata and parameters. |
| [routes/settings/reach](src/routes/settings/reach.rs) | Batched, visibility-filtered setting reach using the resolver's merge and lock provenance. |
| [routes/access](src/routes/access.rs) | The permission catalogue, roles held in a place, permission pages with their owners and defaults pages with previews, the people behind a holder a page at a time, the one change that names owners and holders at a choice, asks who may name somebody from outside and marks an override, a thing's history, and access requests for everything missing at once. |
| [routes/access/page](src/routes/access/page.rs) | A page as the engine reads it, turned into the permission page a client draws: every holder by name, every entry with the person who wrote it, and every place it comes from. |
| [routes/access/subject](src/routes/access/subject.rs) | What a request that is the same for every kind of thing names by `{object}` and `{id}` of its path, and the checks that are the same for all of them. |
| [routes/access/thread_environment](src/routes/access/thread_environment.rs) | What a thread's page shows of the environment it runs in: how many people read its files whatever the page says, and what sharing the thread offers beside it (looking in, and for whoever sets the thread up the Manage of its files and its shells). |
| [routes/access/readers](src/routes/access/readers.rs) | What one person holds on a thing and what gives it to them, the people behind a holder of its page, and what was done to it and by whom: the same three requests for a node, a resource and a document. |
| [routes/access/previews](src/routes/access/previews.rs) | What a change of a page, of the defaults of a place or of the defaults of the system would do, said before anything is changed: how many pages it reaches and who would gain and lose what on them. |
| [routes/access/share_preview](src/routes/access/share_preview.rs) | One ordinary share reviewed before applying it, with current holders, effects and the rules of its place. |
| [routes/access/requests](src/routes/access/requests.rs) | Asking for access to things, reading where the request stands, and deciding it: one approval for each thing asked for, each sent to the nearest people who can give it. |
| [routes/transfer](src/routes/transfer.rs) | Handing a resource or a document over to another person, who is its first owner from then on. |
| [routes/threads](src/routes/threads.rs) | Sync, timeline with the lines of a thread's history and that history as a view, inputs on the sender's own access, side threads, the limits in a turn's way and letting an input past them, draft, pins, binding, fork from any message and exports at three depths. |
| [routes/threads/view](src/routes/threads/view.rs) | A thread as the API shows it to one caller: what the engine knows of it, what it runs on and what that comes to, whether the caller may give it input, and what they may do with it. |
| [routes/threads/make](src/routes/threads/make.rs) | Making a thread in a topic: its binding to a provider with model and effort, an agent profile and an environment, checked for the caller by the access engine with everything missing in the refusal; the page it starts with; and what is said to it at once. |
| [routes/threads/read](src/routes/threads/read.rs) | Reading a thread: a sync that brings a client up to date, a stretch of the timeline around a place, one item, a command's output as plain text, the outline, the catch-up, and how far the caller has read. |
| [routes/threads/read_history](src/routes/threads/read_history.rs) | Explicit historical turn and segment pages, projected with only their required references. |
| [routes/threads/opening](src/routes/threads/opening.rs) | One initial screen read; the existing native handlers remain the source of every projection. |
| [routes/threads/approval_items](src/routes/threads/approval_items.rs) | Personalize harness request cards in a timeline batch, without a per-card HTTP read. |
| [tests/thread_approval_items](tests/thread_approval_items.rs) | A native question arrives with its item, personalized for its reader, and is answered in place. |
| [tests/thread_child_windows](tests/thread_child_windows.rs) | Busy child streams preserve the conversation's first window and page only their own history over HTTP. |
| [routes/threads/source](src/routes/threads/source.rs) | Explicit accepted thread links, with current authority and one history marker. |
| [routes/threads/drive](src/routes/threads/drive.rs) | Driving a thread: an input sent, with its delivery and the client's own value that makes sending it again safe, then edited, put in order or cancelled while it waits; a turn stopped; and a draft kept for the composer. |
| [routes/threads/parameters](src/routes/threads/parameters.rs) | What a thread's agent is set to, and what it could be set to: the model with the models its provider lists, the effort, and the parameters its harness declares, each with where its value comes from and whether the caller may change it. |
| [routes/threads/export](src/routes/threads/export.rs) | A thread exported whole as JSON: the thread, its segments, its turns and its items, which is what a person keeps when they want the conversation outside Orch. |
| [routes/threads/written_export](src/routes/threads/written_export.rs) | A thread exported in its written form: the conversation as Markdown at one of three depths, the same text that an agent which may read the thread is given, and the thread's own chat after it when it is asked for. |
| [routes/threads/binding](src/routes/threads/binding.rs) | What a thread may be put on and putting it there: the providers with their models and the profiles that run on them, in every arrangement, with the reason one is not available or not permitted; the environments the caller manages and what a thread would get there; the defaults; and the binding and the environment of a thread that has no history yet. |
| [routes/threads/binding/start](src/routes/threads/binding/start.rs) | Binding choices before a topic's first thread, judged in an existing environment or the resolved template. |
| [routes/threads/uploads](src/routes/threads/uploads.rs) | Images and files sent from a browser, whole or in pieces, and a file attached to a thread so that it reaches the agent when the input is delivered. |
| [routes/threads/raw](src/routes/threads/raw.rs) | The raw events of a segment of a thread, for whoever manages the thread: what went to the harness and what came back, newest first. |
| [routes/threads/instructions](src/routes/threads/instructions.rs) | What a thread's agent reads as its instructions, source by source, as the last session was opened with them and as the reader may see them (a file's path only with the files of its environment, a home's file by its label alone), and the standing instructions as they stand now. |
| [routes/threads/model_inputs](src/routes/threads/model_inputs.rs) | What Orch supplied to a model for a node, as it was sent: the records written before each sending, block by block and whole, the newest first, for whoever may read the node. |
| [routes/threads/adopt](src/routes/threads/adopt.rs) | A session that a person began in a harness by hand, taken in as a thread of a topic: found by asking the harness, put in the folder of theirs that holds the directory it ran in, and checked as any thread is for what the caller may use. |
| [routes/chats](src/routes/chats.rs) | Chats, messages, states, attachments, inspection and forwarding. |
| [routes/chats/files](src/routes/chats/files.rs) | The bytes of a chat's files: a file stored to be named by an `attachment` part, and an image or a file that a message carries, given back to whoever reads the chat. |
| [routes/chats/export](src/routes/chats/export.rs) | A chat taken away as a file: its written form or its messages as records, and with its files an archive of them. |
| [routes/exports](src/routes/exports.rs) | Exports over HTTP: an archive of what a person may read in a place, planned before anything is made, asked for as a job, listed, fetched and removed by its person alone. |
| [routes/documents](src/routes/documents.rs) | Documents: listing beside a node, blocks and their operations, comments, versions and what changed between two of them, files, export as Markdown, HTML and Typst with what it is asked to hold, import, moving and deleting. |
| [routes/document_tree](src/routes/document_tree.rs) | Typed document-folder metadata, child lists and atomic replay-safe organization. |
| [document_containers](tests/document_containers.rs) | Distinct document folders, native permissions, complete operations and replay over HTTP. |
| [routes/documents/build](src/routes/documents/build.rs) | A document and its parts as a reader is shown them: names put in the place of identifiers, and what the reader may not see left without a name. |
| [routes/documents/listing](src/routes/documents/listing.rs) | The private documents administered beside a place, paged by name without content. |
| [routes/documents/limits](src/routes/documents/limits.rs) | What a document may hold where it stands, as the settings of its node resolve it, and the refusals that say which limit was met and where it is set. |
| [routes/documents/notices](src/routes/documents/notices.rs) | Who is told that a document names them, and who is told of an answer in a discussion beside it. |
| [routes/documents/ops](src/routes/documents/ops.rs) | A batch of changes applied to a document, whole or not at all, and what a refused batch is answered with. |
| [routes/documents/mentions](src/routes/documents/mentions.rs) | Current document readers for mentions through the common holder graph and ordinary document checks. |
| [routes/documents/history](src/routes/documents/history.rs) | The history of a document: versions cut by the clock, listed newest first with their authors and what each changed; what differs between two states block by block; a version as it stood; and putting one back. |
| [routes/documents/comments](src/routes/documents/comments.rs) | Discussions on a block, or on words of it: opened, answered, resolved, and their comments edited, deleted and reacted to. |
| [routes/documents/subscription](src/routes/documents/subscription.rs) | A document's personal discussion bell, independent of its text and read position. |
| [routes/documents/export](src/routes/documents/export.rs) | A document written out as a file: Markdown, one HTML page, Typst source with what it names, or its blocks as JSON, with the files and the discussions beside it when they are asked for. |
| [routes/documents/files](src/routes/documents/files.rs) | The files a document holds: put in from a body or from a finished upload, given back to whoever reads the document, and what an archive among them contains. |
| [routes/documents/page](src/routes/documents/page.rs) | The permission page of a document, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice. |
| [routes/share](src/routes/share.rs) | Sharing a chat, a document, a thread or a whole place with agents, by area, as a copy or a live share to read or to write; listing, taking back, and an agent's reach as a tree. |
| [routes/forks](src/routes/forks.rs) | Fork and switch plans with their options, and execution, single and batch; side threads started and listed. |
| [routes/topics](src/routes/topics.rs) | Overview, the environments a topic's threads use, places and announcements. |
| [routes/topics/announcements](src/routes/topics/announcements.rs) | What a person tells the threads of a topic, or of every open topic of a project, and what has been told: the four requests that list and make announcements for people. |
| [routes/topics/sync](src/routes/topics/sync.rs) | The state behind a topic's feed: the topic, its threads, the jobs about them and its announcements, or what changed after a sequence. |
| [routes/capabilities](src/routes/capabilities.rs) | Taking a capability back from an environment: the key removed, the removal written down, and the unit of the environment asked to be made anew without it. |
| [routes/environments](src/routes/environments.rs) | Environments as resources: creation, lifecycle, configuration changed while one runs, their own secrets, what the exec node offers, the MCP servers found there, their import and one of an environment's own, who reaches into one, its history, update plans, removal, moves, and their jobs. |
| [routes/environments/hosted](src/routes/environments/hosted.rs) | Environments made from a template on a host over HTTP: making one, starting and stopping it, keeping it awake, taking back a removal that has not begun, and the fleet of a place. |
| [routes/environments/list_settings](src/routes/environments/list_settings.rs) | Resolve static environment display settings in one batch before projecting a resource list. |
| [routes/environments/jobs](src/routes/environments/jobs.rs) | The jobs of environments over HTTP: reading one with how far it has come, and asking that one be stopped. |
| [routes/environments/places](src/routes/environments/places.rs) | Where an environment could be made or put, over HTTP: the hosts the caller may use for one more, the most free memory first, then the environments they manage. |
| [routes/environments/network](src/routes/environments/network.rs) | An environment's or a runtime's network as people see it: what its host's node reported, the restriction retained on it, and what its template asks for where nothing was reported yet. |
| [routes/environments/reach](src/routes/environments/reach.rs) | Who reaches into an environment, and by what, over HTTP: its owners, the people its own page lets in by managing it or by one of its four doors, and those who may talk to an agent that runs there. |
| [routes/environments/agent_reach](src/routes/environments/agent_reach.rs) | What the agents of an environment can reach, over HTTP: one read-only overview cut by the caller's own rights. |
| [routes/environments/reading](src/routes/environments/reading.rs) | Who may read an environment and how much of it: whoever holds a permission on it reads all of it, and whoever holds none and may view a thread that runs there reads its name, owner, place, state and the job under way, and nothing of what is behind it. |
| [routes/environments/support](src/routes/environments/support.rs) | What an environment was found to have for browsers and displays, as the facts its machine reported hold it: the programs, the window manager, the command line an agent drives a browser with, and when the environment looked. |
| [routes/environments/sync](src/routes/environments/sync.rs) | The state behind an environment's feed: the environment as the caller sees it, and the jobs that make it or change it. |
| [routes/workspace](src/routes/workspace.rs) | Files with rename, copy and the search inside them, what an archive holds, cache, snapshots with their archives, Git and attached mounts of an environment's workspace, reached by the permissions of the environment's own page. |
| [routes/workspace/paths](src/routes/workspace/paths.rs) | A path of a workspace as a request writes it, and what it may be. |
| [routes/workspace/problems](src/routes/workspace/problems.rs) | What a machine's refusal is as a problem of the API: the code a machine answers with, and what was being done, tell which. |
| [routes/workspace/door](src/routes/workspace/door.rs) | How a request gets into an environment's workspace: by the permission of the environment's own page, whichever thread the caller works beside. |
| [routes/workspace/live](src/routes/workspace/live.rs) | Whether an environment can be asked now, asking it, and asking its machine for its measurements once for each session. |
| [routes/workspace/listing](src/routes/workspace/listing.rs) | Entries and file descriptions made from what the API kept: the list of a tree a machine uploaded, or the versions of files whose content it holds. |
| [routes/workspace/files](src/routes/workspace/files.rs) | The files of an environment's workspace: listed live or from the last list of the tree, described, brought to the store and read from it in ranges, written on the version the writer saw, made, renamed, copied and deleted. |
| [routes/workspace/looking](src/routes/workspace/looking.rs) | What can be found out about a workspace without opening its files one by one: the latest change of each path that agents made, a search by name or inside the text, what an archive holds, and the state and differences of a repository. |
| [routes/workspace/snapshots](src/routes/workspace/snapshots.rs) | A thread's directory kept as of a moment: taken as a job, listed, read file by file, downloaded as one stream, renamed and deleted; and who may read and cancel the jobs of the workspace. |
| [routes/workspace/source](src/routes/workspace/source.rs) | Guarded source preparation and reviewed Git publication through the existing file jobs. |
| [routes/workspace/restores](src/routes/workspace/restores.rs) | Current-authority restore plans bind immutable manifests, whole namespaces and visible writers. |
| [routes/processes](src/routes/processes.rs) | Process control and resource samples of an environment. |
| [routes/terminals](src/routes/terminals.rs) | Shells that are their opener's alone: sessions, the ticket that takes one to another screen, the choice of which to close at a person's limit, and closing by whoever manages the environment. |
| [routes/fleet](src/routes/fleet.rs) | Listings of environments and provider runtimes, recycle, and batch moves and updates. |
| [routes/approvals](src/routes/approvals.rs) | Approval listing and decisions. |
| [routes/resources](src/routes/resources.rs) | Providers, listed by what the caller owns, may use and manages, with their setup, sign-in by each declared method, allowed arrangement types and runtime (start, stop, update plans), models, profiles, hosts with their stored metrics, machines with their policy, templates of both kinds with their import, drivers, MCP presets and imported servers, and mounts; the owners of a resource. |
| [routes/resources/agents](src/routes/resources/agents.rs) | Agent profiles: the ones the caller may use or manage, reading one, making one in an org, changing, switching off and deleting one that no thread is bound to. |
| [routes/resources/hosts](src/routes/resources/hosts.rs) | Hosts: the machines that make environments from templates, listed by what the caller may use or manage, read, changed by their managers, and the token with which one enrolls. |
| [routes/resources/machines](src/routes/resources/machines.rs) | A person's machines: the token a machine enrolls with, listing and reading with whether each is online and what was registered from it, renaming, revoking, asking a machine for its facts again, and its local policy as it reported it with the keys its owner handed over. |
| [routes/resources/machines/enrollment](src/routes/resources/machines/enrollment.rs) | Issuer-scoped enrollment progress, reusing the native resource projection and its current rights. |
| [routes/resources/machines/programs](src/routes/resources/machines/programs.rs) | Stored registry reads, machine-management authority and a native mutation acknowledged with full facts; transactions end before every native wait. |
| [routes/resources/models](src/routes/resources/models.rs) | The models a provider lists, with the name that is sent and the name that is shown, adding, changing, enabling and removing them, and the models that discovery suggests, asked of the machine that holds the key. |
| [routes/resources/page](src/routes/resources/page.rs) | The permission page of a resource, read and changed through the requests that serve the page of anything: who is named in which row, what each row follows, and the one change that names owners and holders at a choice. |
| [routes/resources/provider_login](src/routes/resources/provider_login.rs) | Signing a managed provider in, again, or out, by any of its adapter's methods: the method and what it asks for, what the harness shows and asks back as data, the answer sent back, a sign-in given up, and the harness signed out. |
| [routes/resources/provider_plan](src/routes/resources/provider_plan.rs) | Visible provider plan measurements, peak-preserving charts and reset-aware forecasts. |
| [routes/resources/provider_resets](src/routes/resources/provider_resets.rs) | Browser-only confirmed provider resets, current authority, durable receipts and audit attribution. |
| [routes/resources/provider_runtime](src/routes/resources/provider_runtime.rs) | A managed provider's runtime over HTTP: started and stopped by hand, and what the provider's page shows of it (its unit, its host, its template, how many units it has had, when the idle stop or the renewal of its sign-in comes, and the job at work on it). |
| [routes/resources/providers](src/routes/resources/providers.rs) | Providers on a key: the ones the caller owns, may use and manages, reading one without its key, making one with its endpoints and its key sealed, changing, switching off and deleting it, and writing the key and the secret headers again. |
| [routes/resources/provider_sources](src/routes/resources/provider_sources.rs) | Source choices use the endpoint scheme policy, explicit key consent and only programs registered beside the credential. |
| [tests/provider_sources](tests/provider_sources.rs) | Provider managers choose registered detail sources with write-only secrets, current policy and revision, and see each member's state. |
| [routes/resources/sealing](src/routes/resources/sealing.rs) | The key that seals what a request brings, and sealing a provider's key and secret headers with it before they are stored. |
| [routes/resources/setup](src/routes/resources/setup.rs) | Everything the page that adds a provider needs, for the caller, in one answer: the profiles whose harness can hold a sign-in with the ways to sign in, the protocols and what an endpoint of each is asked for, the presets, the machines that may keep a key, and what the policies there say. |
| [routes/resources/templates](src/routes/resources/templates.rs) | Templates of both kinds: the ones the caller may use or manage, reading one, making one for an org or for the system, changing, switching off and deleting one that nothing was made from. |
| [routes/resources/templates/import](src/routes/resources/templates/import.rs) | A driver's import proposes a validated recipe and explains what it read, without saving a template. |
| [routes/resources/validation](src/routes/resources/validation.rs) | Whole-value schema refusals for resource forms, with field paths and no supplied values. |
| [routes/resources/mounts](src/routes/resources/mounts.rs) | Mount resources, locations on hosts and personal machines, and the places and environments that use them. |
| [routes/resources/mcp](src/routes/resources/mcp.rs) | MCP presets with atomic revisioned definition and sealed-secret writes, environment-owned servers and imports whose definitions stay on their machine; public reads and audit never return credential values. |
| [routes/resources/skills](src/routes/resources/skills.rs) | Skills: versions from an uploaded archive, a git repository or address fetched by a host, or an environment's directory; Update from the recorded origin; drafts edited file by file and published as a version. |
| [routes/environments/mcp](src/routes/environments/mcp.rs) | The MCP servers an environment enables, imports copied on their machine and servers written for this environment alone. |
| [routes/resources/mcp_readers](src/routes/resources/mcp_readers.rs) | MCP presets listed with private permission-page destinations and read only with MCP View. |
| [routes/access/dependencies](src/routes/access/dependencies.rs) | Permission-page dependencies resolved from the same defaults, model settings and giving rules as work. |
| [routes/access/candidates](src/routes/access/candidates.rs) | One visible holder search for people, groups, roles, members and work, with whole current places. |
| [tests/holder_model](tests/holder_model.rs) | Visible locator results, cross-place roles, nested groups, cycles, depth, renames, membership moves and Reaches chains over HTTP. |
| [tests/workspace_source](tests/workspace_source.rs) | Source publication and observed work are served through current environment and thread doors. |
| [tests/realtime_origins](tests/realtime_origins.rs) | A real socket inspection records the trusted-chain or direct-peer origin and browser, ignoring a forged first hop. |
| [routes/resources/upstreams](src/routes/resources/upstreams.rs) | The endpoints of an API provider: the protocols there are, the options each declares and what is refused. |
| [routes/resources/view](src/routes/resources/view.rs) | Machines, providers, profiles and environments as other things point at them, and what the caller holds on a resource, as the API shows them, for any number of resources in the same few statements. |
| [routes/notifications](src/routes/notifications.rs) | The Inbox and its preferences; channels of the system, a root org and a person with their check, test and texts; a person's routes and the link of one to a bot; push subscriptions; and the destinations of a node. |
| [routes/notifications/channels](src/routes/notifications/channels.rs) | Channels, the ways out of the application: listed for the caller with their own route, set up by the system, a root org or a person, changed, removed and tried. |
| [routes/notifications/chat_grants](src/routes/notifications/chat_grants.rs) | Gives one person access to a chat and optionally tells them in the same act. |
| [routes/notifications/exceptions](src/routes/notifications/exceptions.rs) | Personal exceptions use stored subscriptions, one bounded View projection and the bell's mutation. |
| [routes/notifications/destinations](src/routes/notifications/destinations.rs) | The destinations of a node: where what happens at an org, a project or a topic is sent, for nobody in particular, such as a team's room in a chat tool or a build system. |
| [routes/notifications/grouping](src/routes/notifications/grouping.rs) | The Inbox by place: the same notifications grouped under the nearest place the caller may read, what they can act on first, paged under a fixed cut; a group marked read up to its cut, and undone by a sealed ticket. |
| [routes/notifications/inbox](src/routes/notifications/inbox.rs) | The caller's Inbox: the list newest first, the badge's counters, what is marked read, what they chose about being notified, the browsers that take push, and the kinds of notifier this system has. |
| [routes/notifications/person](src/routes/notifications/person.rs) | Explicit person notices on readable documents, chats and places. |
| [tests/people_scale](tests/people_scale.rs) | People pages and discussion jobs keep bounded batches, exact replay and current reading at scale. |
| [tests/people_notify](tests/people_notify.rs) | Explicit notices keep recipient authority, anchors, replay and grants together. |
| [routes/notifications/routing](src/routes/notifications/routing.rs) | A person's routes through the channels that reach them: switched on or off, with the kinds they tell, the unread delay and the limit, the link of a route that has to be linked, and ended. |
| [routes/notifications/views](src/routes/notifications/views.rs) | The shapes the requests answer with: a channel together with the caller's own route through it, a route, a destination, a kind of notifier in the caller's language, and the texts a channel keeps. |
| [routes/admin](src/routes/admin.rs) | Management: users and their suspension, a reset link that may switch a second step off, what a new person's own org starts as, root orgs and their two limits, both pools of runtimes and their updates, stored metrics, audit and system state. |
| [routes/admin/users](src/routes/admin/users.rs) | People as a system admin keeps them: listed with the proofs strangers have locked, made with a link to set a password, suspended and let in again, made and unmade system admins, and given a link to set a password anew. |
| [routes/admin/orgs](src/routes/admin/orgs.rs) | Every org of the system for the people who run it, and the two limits of a root org: whether its tree takes further members and whether it holds orgs inside it. |
| [routes/admin/semantic](src/routes/admin/semantic.rs) | Search by meaning as a system admin sees it: whether the database can keep vectors, each root org's index with its progress, estimate, disk and reason, the choice to rebuild one in place, and the provider Local embeddings. |
| [routes/admin/settings](src/routes/admin/settings.rs) | The settings of the system: every key that is set at system scope with where its value comes from, written (and locked for every node below, or not), and taken back to what Orch ships. |
| [routes/admin/defaults](src/routes/admin/defaults.rs) | The defaults page of the system, which every org starts from: read, changed, and put back to what Orch ships. |
| [routes/admin/audit](src/routes/admin/audit.rs) | The audit trail of the whole system, read a page at a time with its filters and written out as a streamed file for a span of at most a year. |
| [routes/admin/system](src/routes/admin/system.rs) | How the system stands: its parts with their heartbeats, its queues, what its blob store holds, the state of its certificates, the document of its requests, and the certificate of its authority. |
| [routes/admin/fleet](src/routes/admin/fleet.rs) | What runs on the system's hosts and how it has fared: every host, the runtimes of both pools, the measurements the system stored about itself over a span, and the plugins loaded anywhere. |
| [routes/admin/catalogue](src/routes/admin/catalogue.rs) | The data a system admin keeps for the whole system: the definitions of capabilities, the builds of harnesses that hosts may fetch, and the agent profiles that have no home. |
| [harness_checks::channels](tests/harness_checks/channels.rs) | Release channels stay silent until effective policy or an explicit refresh asks for them. |
| [routes/admin/identity_providers](src/routes/admin/identity_providers.rs) | The single sign-in methods a system admin keeps: listed, added, changed by revision, tested against the provider itself, and removed unless somebody could then no longer sign in. |
| [routes/downloads](src/routes/downloads.rs) | Connector binaries and installers served by the system. |
| [routes/search](src/routes/search.rs) | Authorized transcript and node search, the search inside one conversation, and the finder's names. |
| [routes/everything](src/routes/everything.rs) | The complete authorized hierarchy for every signed-in person, with a whole-system scope for administrators. |
| [routes/everything/resources](src/routes/everything/resources.rs) | Resource-list filters and bounded rich rows, using the inventory's own frame and existing readers. |
| [routes/search/content](src/routes/search/content.rs) | Content search over readable conversations, chats and current document blocks, with stable pages and Unicode excerpts. |
| [routes/search/meaning](src/routes/search/meaning.rs) | Results by meaning joined with results by words in people's search ordered by best matches: one order by reciprocal-rank fusion, each result checked against what its reader may read now. |
| [routes/usage](src/routes/usage.rs) | The ledger read at three depths as figures, series and records, in tokens and credits; rates, with the catalogue of estimates and its upload, fetch and restore; caps on tokens, on credits and on turns running at once, with who is waiting and how often each was passed. |
| [routes/usage/scope](src/routes/usage/scope.rs) | What a reader of usage is shown, by the scope they ask about: their own use, a place and everything below it, or what ran on a provider; how deep their access goes there, and which dimensions and filters that depth gives. |
| [routes/usage/names](src/routes/usage/names.rs) | The names that figures and records are shown by: people, providers, models by what they are shown as, agents by kind, and places, each place a redacted reference where the reader may not see it. |
| [routes/usage/csv](src/routes/usage/csv.rs) | The other form of an answer: comma-separated lines, escaped as RFC 4180 says, for the series of figures and the records of the ledger; and the JSON answer of a request that can give either. |
| [routes/usage/figures](src/routes/usage/figures.rs) | Usage over a span: totals, a series cut into buckets, and breakdowns by dimension, each as deep as the reader's access to the scope goes; and the series as CSV. |
| [routes/usage/events](src/routes/usage/events.rs) | The ledger rows behind a figure, with the same scope and filters, each with its thread and its turn, what it ran on, the rate it was counted at and its credits; a page of them, or all of a page as CSV. |
| [routes/usage/rates](src/routes/usage/rates.rs) | What each model usable at a node would be counted at now and from where that rate comes, the shipped catalogue of estimates as it is loaded, and its replacement by a system admin from a file, from the address an admin set, or from the release. |
| [routes/usage/caps](src/routes/usage/caps.rs) | Caps made, changed and removed by whoever the place of the cap says; how each of them stands; everything that bounds work at a node; a person's own share of the caps counted per person; and who waits for room in a cap. |
| [routes/personal](src/routes/personal.rs) | Favorites, recents and read/subscription state; what the interface keeps for a person. |
| [routes/first_steps](src/routes/first_steps.rs) | Current readable setup observations and atomic portable first-steps choices. |
| [first_steps tests](tests/first_steps.rs) | First steps are served from readable stored facts and save only the caller's portable choices. |
| [routes/directory](src/routes/directory.rs) | Org groups, roles, tags and service identities, each kept by one org at any level. |
| [routes/directory/members](src/routes/directory/members.rs) | Ending a membership and what it touches, and changing the domain of a member, each with the plan of what it does before it does it. |
| [routes/directory/outsiders](src/routes/directory/outsiders.rs) | People from outside a root tree who reach something of one of its orgs: the list, the ways each reaches it, and taking one out by a plan read whole. |
| [realtime/server](src/realtime/server.rs) | Browser socket authentication and reauthentication. |
| [realtime/subscriptions](src/realtime/subscriptions.rs) | Authorization-aware feed subscriptions. |
| [realtime/sync](src/realtime/sync.rs) | Sequenced fan-out, gap repair and database notification sweep. |
| [realtime/deltas](src/realtime/deltas.rs) | Streaming delivery bounded in bytes and in time, with deltas merged for a reader that is behind and no connection closed for being slow. |
| [realtime/presence](src/realtime/presence.rs) | Visible reader aggregation. |
| [realtime/chats](src/realtime/chats.rs) | Chat feed fan-out, typing and unread signals. |
| [realtime/documents](src/realtime/documents.rs) | The document feed: blocks that changed by sequence, discussions, presence, and who is writing where. |
| [realtime/environments](src/realtime/environments.rs) | The environment feed: state, jobs and pending updates, for everyone who may see the environment. |
| [realtime/resource_changes](src/realtime/resource_changes.rs) | Resource invalidations and private provider progress for transient subscribers. |
| [realtime/resource_rows](src/realtime/resource_rows.rs) | A resource change carries its native GET view, projected for the actual session receiving it. |
| [realtime/resource_health](src/realtime/resource_health.rs) | Provider health projected through the same bounded, authorized reader as the status API. |
| [realtime/personal_changes](src/realtime/personal_changes.rs) | Personal-state event values projected after commit without enlarging database notifications. |
| [realtime/raw_traffic](src/realtime/raw_traffic.rs) | Newly committed harness frames forwarded with the same masking as the raw-frame list. |
| [realtime/resources](src/realtime/resources.rs) | The resource feed: a change to reload, and a provider's sign-in state for those who manage it. |
| [realtime/threads](src/realtime/threads.rs) | The thread feed: the rows of a thread by its sequence, the thread itself, and who is looking. |
| [realtime/history_read_tests](src/realtime/history_read_tests.rs) | Deterministic snapshot boundaries with the native HTTP, machine and thread projections. |
| [realtime/topics](src/realtime/topics.rs) | The topic feed: the topic as the caller sees it, by the topic's own sequence. |
| [realtime/tree](src/realtime/tree.rs) | The organization feed: the nodes of a root organization's tree that changed, cut down to the ones the subscriber may view. |
| [realtime/user](src/realtime/user.rs) | The user feed: the rows of a person's Inbox by its sequence, and the news that is no row. |
| [realtime/environment_watches](src/realtime/environment_watches.rs) | Environment measurements and directory watches shared by transient feed subscribers. |
| [realtime/machine_views](src/realtime/machine_views.rs) | Current machine-backed view projections with the same permissions and personal cuts as their HTTP reads. |
| [realtime/ephemeral](src/realtime/ephemeral.rs) | Transient feed delivery, shared watch ownership and current whole lists. |
| [realtime/watches](src/realtime/watches.rs) | Who may follow the ephemeral feeds of an environment, decided by the environment's own page: its processes and its load by `terminal.view`, a directory of its workspace by `files.view`, and none of them through a thread; and diagnostics. |
| [realtime/usage](src/realtime/usage.rs) | Live model requests projected under each subscriber's current usage scope and record rights. |
| [realtime/terminals](src/realtime/terminals.rs) | Terminal sockets: one screen at a time for a shell that runs on without one, and the replay it is taken up with. |
| [backplane](src/backplane.rs) | Gateway authentication, machine presence and command/event routing. |
| [backplane/report_sources](src/backplane/report_sources.rs) | Session sources checked before stream ordering, effects and cursor writes. |
| [backplane/report_refusals](src/backplane/report_refusals.rs) | Body-free machine refusal counts and hourly system audit publication. |
| [backplane/session_source_tests](src/backplane/session_source_tests.rs) | Session source refusals through the API's real link and database. |
| [backplane/machine_report_tests](src/backplane/machine_report_tests.rs) | Foreign environment, harness and host reports are dropped through the real link. |
| [backplane/binding_tests](src/backplane/binding_tests.rs) | A locally discovered subscription is registered once across repeated discovery and reconnects. |
| [dispatch](src/dispatch.rs) | Durable commands and redelivery. |
| [thread_engine](src/thread_engine.rs) | The thread engine inside the API: its commands go through the link's dispatcher, a machine's events are applied by it, and it is built once at start. |
| [hosting](src/hosting.rs) | What the engine asks of this program when it makes a unit for an environment: the token its connector enrolls with, kept sealed in the system's store, the address and the authority it trusts, and the builds of the programs it runs. |
| [thread_access](src/thread_access.rs) | What the thread engine asks about resources, answered by the access engine. |
| [thread_markers](src/thread_markers.rs) | The lines a person's change of a thread leaves in its history: a new model or effort, another setting of its own, its page, its first owner. |
| [tickets](src/tickets.rs) | Session, lease and terminal ticket issuance. |
| [blobs](src/blobs.rs) | Authorized blob access and pinning. |
| [uploads](src/uploads.rs) | Browser and machine uploads in chunks sized to the link, which go on where they stopped. |
| [content](src/content.rs) | What machines upload to and download from the store of content over the internal listener: pieces that go on where they stopped, checked against their hash, and only for commands that are open. |
| [media](src/media.rs) | What a file is, from its name and its first bytes: its media type, whether its content is text, and a name made safe to put in a path. |
| [previews](src/previews.rs) | Bounded static file previews cached by content and served only through their authorized owner. |
| [cache](src/cache.rs) | Offline manifests, per-environment eviction and blob garbage collection. |
| [documents_upkeep](src/documents_upkeep.rs) | The documents' scheduled jobs: a version of each document that was left alone, and the daily purge of what no document needs any more. |
| [recordings](src/recordings.rs) | The last output of a person's shells put away in the store of content with the values of the environment's secrets taken out, and read back. |
| [certificates](src/certificates.rs) | Enrollment, machine identity checks and renewal endpoints. |
| [machine_enrollment_home](tests/machine_enrollment_home.rs) | Actual enrollment and resource routes preserve a person's home, ownership, private access, environment registration and explicit administrator marks. |
| [openapi](src/openapi.rs) | Supply realtime schemas to the API-contract OpenAPI renderer and cache its served bytes. |
| [routes/browsers](src/routes/browsers.rs) | Browser requests and the whole current list shared with the unretained browser feed. |
| [routes/displays](src/routes/displays.rs) | Display requests share the same safe current rows with the unretained display feed. |
| [realtime/browsers](src/realtime/browsers.rs) | Browser sockets share one live tunnel and let each viewer draw only its newest picture. |
| [realtime/displays](src/realtime/displays.rs) | Display sockets terminate viewer security and filter every input under current authority. |
| [tests/displays_support](tests/displays_support/mod.rs) | Display clients use the real shipped-image peer and the same guarded API fixture as browsers. |
| [realtime/rfb_filter](src/realtime/rfb_filter.rs) | Bounded RFB client framing keeps watching sockets from sending actions. |
| [routes/nodes/color](src/routes/nodes/color.rs) | Node icon palette validation and the colour a new place inherits from its parent's settings. |
| [routes/nodes/tree_docs](src/routes/nodes/tree_docs.rs) | Reading the document rows of many tree nodes with a fixed number of access statements. |
| [routes/resources/machines/reenrollment](src/routes/resources/machines/reenrollment.rs) | Issuing a machine's replacement enrollment token to its owner or the managers of its host. |
| [certificates/reenrollment](src/certificates/reenrollment.rs) | Tokens that enroll an existing machine again and certificate handover without replacing its row or registrations. |
| [backplane/reenrollment_tests](src/backplane/reenrollment_tests.rs) | Replacement enrollment tests against a real database and the session authorization path. |

Transient feeds share watch ownership across API processes and coalesce whole browser and display lists. Their subscriptions use the same access checks and revocation path as sequenced feeds.

Browser profiles and lifecycle requests are bound, with one current-list builder for HTTP and the transient feed. Browser sockets use single-use tickets, share a tunnel, retain only each viewer's newest picture, and recheck the browser and file permissions during use. Real-browser tests run a private environment peer in the shipped image and prove view-only input does not reach the page and sitting history keeps hosts alone.

Environment feed subscribers share load, process and directory watches, which stop after the last departure grace or once an unreachable machine returns. A new subscriber checks availability, and a successful process signal immediately reports a new snapshot when process watching is active; connector integration tests exercise these paths.

Resource feeds announce row, access-page, model and runtime changes, with private sign-in steps and runtime jobs checked for each recipient. Newly committed diagnostic frames use the engine’s current masking before reaching transient subscribers; socket tests check both privacy rules.

| [retained](src/routes/environments/retained.rs) | Kept environment metadata and guarded restore or permanent removal requests. |

| [trace](src/routes/trace.rs) | Permanent audit rows projected with current content and restoration authority. |

| [inspection](src/middleware/inspection.rs) | Explicit retained-content reads reuse restoration guards and never mark anything read or notify anyone. |

| [trace_readers](tests/trace_readers.rs) | Real HTTP and database proofs of permanent document scope, deleted inspection and stale deletion selection. |
| [input_schedules](src/input_schedules/mod.rs) | Guarded future-input definitions, private drafts, safe occurrence views and the API's due sweep. |
| [input_schedules/timing](src/input_schedules/timing.rs) | Civil schedule occurrences with one earlier fold instant, explicit gap skips and no paused-date catch-up. |
| [input_schedules/runner](src/input_schedules/runner.rs) | Current-authority due claims atomically create ordinary work and report each civil occurrence once. |
| [input_schedules tests](tests/input_schedules.rs) | Real HTTP future inputs keep drafts private, use current authority and queue each occurrence once. |
| [threads/native](src/routes/threads/native.rs) | Current thread viewers read an existing native session's effective commands with a fresh return guard. |
| [native_actions tests](tests/native_actions.rs) | A cold native catalogue is truthful and reveals nothing without current thread View. |
| [trace_history](tests/trace_history.rs) | Permanent HTTP trail and surviving snapshot bytes after completed private references are collected. |

| [trace_walk](tests/trace_walk.rs) | A real scripted harness calls an Orch tool and a person follows its frozen responsibility into retained HTTP history. |
| [audit_emitters](tests/audit_emitters.rs) | Every public audit action is backed by a linked production writer. |
| [audit_tolerant](tests/audit_tolerant.rs) | Audit readers retain thread creation and unfamiliar stored actions without losing the rest of the trail. |
| [admin_mode](tests/admin_mode.rs) | One explicit session mode, bounded administrative reach and consistent ordinary/elevated provenance over HTTP. |
| [private_documents](tests/private_documents.rs) | A page manager without content access sees only a private document's identity, with independent name paging. |
| [threads/calls](src/routes/threads/calls.rs) | Authorized recovery reads and explicit retry-now or stop controls for a thread. |
| [call_recovery tests](tests/call_recovery.rs) | Recovery is readable with the thread, controllable only by its drivers, and governed by validated inherited settings. |

| `routes::search::stream` | One cancellable stream of search parts, source progress and authoritative reciprocal-rank orders. |

| `routes::search::words` | Bounded indexed word windows with exact current permission frames and server-side field and occurrence ranking. |

| [tests/search_meaning](tests/search_meaning.rs) | Best matches joined with results by meaning: a text in another language found by what it means, a result both orders rank first, nothing the reader may not read, and plain results by words where the org does not search by meaning. |

| [tests/semantic_admin](tests/semantic_admin.rs) | Search by meaning in Management: where each org's index stands, for system admins alone, and the provider Local embeddings made at home in an org from its template, with its one model offered and the value that names it. |

| [tests/semantic_embeddings](tests/semantic_embeddings.rs) | The embeddings an org names for search by meaning: a setting whose writer answers for its use, texts made into vectors in the provider's runtime at the named length, counted under the purpose `embeddings`, and refused with the reason when the writer may not use the provider. |

| [tests/semantic_index](tests/semantic_index.rs) | The index of meaning of an org: its text cut and made into vectors in the background, newest first; a pause with its reason before the disk share is passed; a change of model that builds a new generation while the old one answers, the same model served elsewhere and a shorter length that need none; and without the vector extension, everything else working and the reason said. |

| `routes/storage/cleanup.rs` | Shared cleanup previews, real fates and one confirmed durable operation |


| [routes::loss](src/routes/loss.rs) | Permanent-loss previews and typed confirmations under fresh resource authority. |
| [routes::environments::recovery](src/routes/environments/recovery.rs) | One reviewed batch independently moves, re-creates or switches the selected environments. |

| [routes/resources/host_delete](src/routes/resources/host_delete.rs) | Reviewed host deletion runs independent recovery or removal before detaching native resources and revoking its machine. |
| [routes/resources/host_delete/data](src/routes/resources/host_delete/data.rs) | Host deletion gives each existing data source its own fate before physical erasure. |

| [routes/resources/provider_recovery](src/routes/resources/provider_recovery.rs) | A provider's existing re-create control reviews lost runtime provisioning without borrowing its owner's destination authority. |

| [resource_resume tests](tests/resource_resume.rs) | Lifecycle Enable continues exact resource-stopped work as its original author before preserving queued work. |
| [routes/resources/library](src/routes/resources/library.rs) | Resource pages read the same batched library figures as inventory rows, after ordinary View checks. |

| [routes/everything/library](src/routes/everything/library.rs) | Library views, privacy projections and recorded-use ordering share the inventory's single read. |

| [certificates/frontend](src/certificates/frontend.rs) | The web edge's renewable certificate, issued by the existing authority and published as one complete pair. |

| [routes/admin/programs](src/routes/admin/programs.rs) | Paged agent program administration, independent release selections and localized declaration capabilities. |
