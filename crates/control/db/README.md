# orch-db

PostgreSQL persistence, transactions and projections.

| Module | Responsibility |
|---|---|
| [tests/usage_commit_order](tests/usage_commit_order.rs) | Usage cursors number each changed row at commit while retries, savepoints and cancellation preserve atomicity. |
| [tests/usage_commit_scale](tests/usage_commit_scale.rs) | A million ledger rows commit as one bounded-memory cursor range without changing their token totals. |
| [queries/thread_history_cuts/scale_tests](src/queries/thread_history_cuts/scale_tests.rs) | Million-row indexed uncertainty checks and batched turn totals over 1,000 threads. |
| [tests/participation_summaries](tests/participation_summaries.rs) | Participation keeps descendant assignments, exact deadlines and transactional moves without reading the owned inventory. |
| [tests/document_reference_storage/chunks](tests/document_reference_storage/chunks.rs) | Multi-chunk edits retain current block state and exact counters while refusing and rolling back later conflicts. |
| [tests/document_reference_storage](tests/document_reference_storage.rs) | Stored reference projections preserve an existing database and keep native edits, anchors and paging exact. |

`queries/hierarchy::org_facts` keeps domain `member_count` separate from active-human `people_count`.
The latter unions domain members and Viewer-or-higher holders, including inherited/group roles,
through the existing batched holder expansion at the caller's clock. It deduplicates people and
keeps native suspension/expiry/deletion rules; no per-card query or parallel access evaluator.

`queries/resource_counts.sql` aggregates discoverable resources from the inventory's native grants, keeping physical
machines separate from runtime instances and excluding deleted homes without loading resource contents.

`queries/archive_plans.rs` reads the exact archive subtree/work stamp and its safe consequence counts; it changes no work or content.

`queries/installation.rs` stores the two installation records and inventories the exported read-only database snapshot used by the existing backup command. External file locations remain explicitly excluded; nothing reads a machine's credentials or volumes.

Persists product state, audit history, projections and durable work in PostgreSQL. Shared job holds serialize thread and runtime changes, successor rows keep replacement candidates separate from current units, and host measurements are read as scoped time series. Database and application integration tests cover transaction boundaries and observable behavior.

Search reads reachable name candidates in bounded batches and reports partial answers. Names, inventories and content share a literal word/quoted-phrase grammar; exact names lead personal visit scores and activity. Stars and age-decaying visit counts use kind/id identities for every addressed kind. Content applies Everything's exact projection in SQL before matching and paging, using the shared `node_visibility.sql` for ordinary thread decisions inherited from topics. It hydrates only returned rows; optional relevance puts name/title tiers before bodies and pins the visit clock across pages. Projection repairs compare source fields before updating text, and real API/engine tests exercise the queries.

`queries/transaction_guard` locks ordinary requests against the shared `authz_gate` barrier in the same statement as identity.
Authority writers take that barrier exclusively first. Personal credentials instead lock only their person; `orch_identity`
announces their changes at commit. New objects share the barrier and invalidate the separate cache epoch. Holder audiences
expand from indexed role, group and domain sources through `access_holder_people`, with no per-user audience scan.
The native proofs cover concurrent refreshes/drafts, revocation, expiry and every audience query with 1,000 people under 100 ms.

Partly implemented. Tree queries aggregate following documents, document changes announce the node beside them,
and revision-checked node edits change text and icon colour together. Replacement enrollment preserves machine
registrations and retires certificates under a row lock; real database tests cover these behaviors.
Connections, migrations, seeds and the audit writes are in place, with the queries of identity,
hierarchy and access that the sign-in and the routes of the hierarchy read and write; each is tested against a real
PostgreSQL. A chat has a sequence of its own: `queries/chats` takes its next number in the transaction that changes a row of it, holds the rows of
chats, their people, messages, per-person states and quoted items, and announces the numbers (`orch_chat`) and who is typing
(`orch_chat_typing`). `queries/shares` holds what sharing with agents reads and writes: the entries that name agents, the places of the tree by name, the documents and chats beside them, a conversation, a chat and a document read by the numbers of their parts, what a search looks through, and the copies given to threads with their taking back, the documents that were deleted beside a place, and what goes with a document (its files and its versions) counted for any number of them in one statement; a document an agent deletes is softly deleted with nobody named as having done it (the audit line says which agent). The history of a resource, an environment or a document is read by what its lines are about, the pages inside a place are found by identifier, and the people who share a tree with a person are asked for in a block of `queries/access`; the approvals that a request for access raises, the batches a person has waiting and the requests nobody decided in time are a block of `queries/approvals`. A module of the table below that no request reads yet still holds its planned responsibility only.

Projects lie inside projects as orgs lie inside orgs, and a topic is moved: `queries/hierarchy` counts the projects a project holds, how many projects lie one inside another down a path and within a project (what `[limits] project_depth` bounds), which threads of a topic a fork, a switch or a move holds, and gives the rows of the ledger and of both rollups that name a topic the project it lies in now, writing that column alone. A project's figures (`usage/rollup`) sum the folders inside it, and the topics a whole-project announcement goes to are those of the folders too. They are tested on a real database in `tests/folders.rs`.

The feeds of the realtime socket are numbered by the counters of their own rows and announced in the transaction that moved
them (`sequences`): a writer takes the next number of a thread's sequence and announces the last; the stamps of a tree, a topic
and an Inbox are announced by the queries that make them (a change of a thread's state, or of what is worth a reader's
attention, stamps the tree of its org, which shows each thread's state); news that is no row, a move of the authorization
epoch and what each process says of who reads what have announcements of their own; and `sequences::listen` is the one
connection per process that hears them. What the socket reads from a number is in `queries/feeds` and the thread queries. They are tested on a real
database, and through the socket in the API's tests.

What the state behind the feed of a topic and the feed of an environment reads is a transaction that sees the database as it was when it began (`queries/feeds`) and a block at the end of `queries/jobs`: the jobs under way, or those stamped on a feed after a number. The frames of a harness's own protocol are the rows of `queries/raw_events`: numbered in their segment in the order they were reported, read newest first, a month to a partition that is made when a frame is the first of its month, and no more of them for a segment than the number given with each frame. `queries/adoption` holds what taking in a session that exists already reads and writes: the lock that makes two requests for one session come one after the other, the environments that are directories of a machine, and whether the session of a segment waits to be opened by Orch. They are tested on a real database.

Raw frames retain the number assigned at capture. Their segment cursor is committed with the row
and survives removal of every frame's bytes, so replay across months or after pruning cannot add a
copy. Concurrent deliveries serialize on the segment. The newest captured frames displace older
ones. Real database tests cover these rules and preserve equal-payload frames with different
original numbers.

The rows of documents are written and read here: a batch of operations is applied to a document's blocks under the lock on the
document's row, which numbers everything it changes from the document's own sequence and announces it when the transaction commits;
versions are cut before a batch when the clock says so, kept within a number and a size, and put back; discussions take the next
number of the same sequence. Several people writing at once, a batch that conflicts and leaves nothing of itself behind, and a batch sent
twice are tested on a real database.

The runtime of a managed provider and its sign-in are rows too, read and written by the blocks `provider_runtime` of `queries/resources` and `provider_jobs` of `queries/jobs`: the live runtime of a provider and the one a deleted provider leaves to be taken away, the provider's state as the upkeep of its unit and its sign-in read it, the sign-in that waits for the unit (kept sealed, and its sealed secrets in rows of their own, all deleted together), what holds a unit up (the turns, leases and sessions on it), the runtimes that are idle, stopped for long, or retired, the tokens whose end is known, the managers to tell; the compare-and-set that takes a credential for an environment, brings it home or gives it up, the leases under which a copy or the login left, and the phases of the jobs that make, start, renew and remove a unit, queued one at a time for a provider. They are tested through the engine, the worker and the API on a real database.

A person's shells are rows of `queries/terminals`: a shell is a row from the moment it is asked for (`opening`, which holds its place against
the limits while the machine starts it), `open` while it runs, attached to a screen or not, and `closed` for good, each move a compare-and-set
so that two processes that both find a shell idle close it once. The statements count what the limits count, list a person's shells newest
first with a cursor, say which shells have been left alone for a given time, which belong to an environment that stopped or was made again, and
which are still being started by a process that went away, and keep the last output of a closed shell as a blob that is held while the shell's
line points at it. They are tested on a real database here and through the requests and sockets of the API.

The rows of the store of content and of the workspace are written in `queries/storage` (blobs with their pins and the grace
of one that nothing points at, machine uploads and a browser's in pieces with the bytes that have arrived, the versions of
files whose content is cached with what trims them, and snapshots with the states they move through), in a block of
`queries/environments` (an environment as the workspace reads it, the place of a thread in it, the list of its tree and the
latest change to each path that agents made). The jobs of this part are rows of `queries/jobs` like the others: they
are about a thread as well as an environment, and every change of one is stamped on the feed of the thread's topic as well
as on the environment's. They are tested on a real database through the API's tests.

`queries/uploads` retains each uploader's reference independently for identical bytes and lists saved sources for current permission checks. Its draft and blob locks keep draft replacement and collection consistent, tested through the HTTP API against PostgreSQL.

The machine link's rows are written: `queries/link` (the cursors of
streams, moved by compare-and-set, and the commands with states that only move
forward) and, in `queries/resources`, machines, the tokens that enroll them and
the hosts that are machines of one kind.

Personal notification exceptions are selected in `queries/notifications/exceptions`: bounded
manual rows differing from their nearest ancestor subscription, using the existing user/node
index. The API applies the ordinary current View decision and redacts ancestor names; subscription
writes still use `queries/hierarchy` and its existing table, without another preference store.

The rows of notifications are written in `queries/notifications/*`: channels and
destinations with the secrets sealed to them, routes with their codes and how far
each has told, the outbox, whose rows are claimed by the lock a transaction holds
while the delivery is made, and what the worker's schedule removes or trims. They
are tested on a real database: the claims of the outbox here, and the rest through
the worker's own tests. What the pages and the engine read and write beside them is
there too: a person's Inbox (the list newest first, the counters, what is marked read
and given the person's next number so that every device learns, an approval that nobody
needs to decide any more, and what the person chose about being told), the nearest
subscription of each person on the way down to a node, the automatic row of whoever
owns or writes in a thread, the destinations set at a node or above it, the delivery to
one of them written once, and the lists of channels and routes of a person.

What a thread may be put on and what a person's machine has are read in blocks at the end of `queries/threads`, `queries/resources` and `queries/environments`: whether a thread has history, where an environment is used, the threads bound to a provider and the conversations a harness holds that a thread adopted, the harness processes of a profile, the adapters, drivers and capabilities the dialogs list, and the login registered from a machine. They are tested on a real database through the API's tests.

The usage ledger is written by `usage/ledger`: one row for each use a machine reports (a request the forwarder relayed, or the
totals a harness reported for a turn that no forwarder counted) and one for each model call Orch made itself, whose identifier the
writer chooses, so that a use reported twice is one row; it says what the use ran on and whom it is put down to as it was when it
happened, and keeps the rate it was counted at and the credits exactly (`usage/micros`: amounts in millionths, never floats). The
rollups by day and by month are made by the one statement that `usage/rollup::rebuild` runs for the whole ledger, run for the key of
the row that arrived in the transaction that wrote it, so that what is kept as rows arrive is what a rebuild makes (tested over
random ledgers, whose rows run at different speeds); the service tier a row ran with is part of the key of both rollups, so that a cap
can cover one tier alone and a figure by tier is read from the days; a figure is read from the ledger by the hour and from the days
otherwise. A provider's entry for a model keeps the groups its managers put it in (`queries/resources`: the groups of one model, and of
many at once for the caps that name a group). Usage caps are made once for each
identity (which includes the tiers a cap names), changed against the revision that was read and removed in `usage/caps`, which also
finds the caps that could cover a turn at the tier it asks for,
what the people a cap covers used in its period (by provider and model, so that a group of a provider's models can be asked, and only
at the tiers the cap names), the covered turns that run now (counted under a lock for each cap, in ascending
order of identifiers, so that two pre-flights never both see the last free place), what waits for room, the notices sent once for
each cap, person, period and level, and each turn that was let past a cap. The hours of the metric minutes are folded by a query of
`queries/notifications/upkeep`. What the engine reads and writes to keep a provider on a key running
(its start row, the names of its secrets without their values, its standing and facts, which providers a machine's absence
moves, which were switched off, and which a limit of the plan closed until a moment that has passed) is the block `keeper` at
the end of `queries/resources`. Both are tested on a real database.

What the requests about resources read and write is in a block of its own at the end of `queries/resources` and of
`queries/environments` (the module `routes` of each): machines and their local policy, profiles, providers with their
endpoints, keys and models, hosts with what runs on them, templates, and environments registered from a machine, each
changed against the revision the caller saw. They are tested through the requests that use them.

The jobs that make, start and remove environments are rows with a lease, a request to cancel and a commit point
(`queries/jobs`): whoever runs a job holds a lease on it that lapses, so that another process takes over a job from one
that died and two never run it, and every change of a job is stamped on the feed of its environment. What the engine reads
and writes to keep environments on hosts is a block of its own at the end of `queries/environments` and of
`queries/resources` (the module `hosting` of each): the units and volumes of a host, the token with which a unit's
connector enrolls and the machine that comes of it, the environments on a machine that are out of step with the session
the machine is in, what a removal has to wait for, and the environment that new threads of a topic join. They are tested
through the engine's and the API's tests on a real database.

What Management reads and writes is in blocks of its own at the end of `queries/identity` (people with their filters, the
wrong tries held at their names, identity providers with the check that removing one shuts nobody out),
`queries/hierarchy` (orgs by name, root orgs that were deleted and not purged, the two limits, and the audit trail of the
whole system, newest or oldest first), `queries/resources` (the block `management`: the hosts and runtimes of the whole
system, machines by what they report, capability definitions written against a revision, the builds of harnesses an admin
added, and agent profiles that belong to no org), `queries/metrics` (series over a span from the minutes or from the hours,
what the blob store holds, and the sizes of the queues) and `queries/link` and `queries/jobs` (what waits). They are
tested through the requests that use them, in the tests of the API.

What giving access reads is in blocks beside the tables they use: `queries/access` counts the people behind every role that a page names in one statement (the role given directly, through a group or to all the members of an org, the owners of a place, who count as its admins, and the admins of the system), `queries/hierarchy` lists the side threads that lie inside a place at any depth, which are no thing of their own in a count, and `queries/documents` lists the documents that were not deleted beside a place or beside anything inside it. Each is one statement however big the place is. They are tested through the requests that use them, in the tests of the access engine, the engine and the API.

Thread, document and chat writes call `queries/hierarchy::touch_activity` on their existing connection.
It carries the event's timestamp up the node path, never backward, and stamps changed ancestors for
tree repair. An ancestor is written at most once per five seconds to bound streaming work while
keeping activity visible. The tree's changed order, Everything and name-search ties read that same
clock; a node without an event falls back to its creation and own metadata. The real-route regression
in [the API's org_contents suite](../api/tests/org_contents.rs) covers the three writers, throttling, rollback and the separate creation and
activity filters.

The queries of the tools of the `orch` server are blocks beside the tables they use: `queries/threads` has the announcements people and agents send and the messages one thread hands another (made once under an identifier derived from the call, counted from the lines they leave), the threads of a topic as they are addressed, and the rate of what a thread sends; `queries/approvals` the requests for a capability and those nobody decided in time; and `queries/environments` the catalogue of capabilities, what an environment has been given and was made from, and the job that makes its unit anew.

The queries of the tools of the `orch` server are blocks beside the tables they use: `queries/threads` has the announcements people and agents send and the messages one thread hands another (made once under an identifier derived from the call, counted from the lines they leave), the threads of a topic as they are addressed, and the rate of what a thread sends; `queries/approvals` the requests for a capability and those nobody decided in time; and `queries/environments` the catalogue of capabilities, what an environment has been given and was made from, and the job that makes its unit anew.

Browser rows have atomic profile reservations, lifecycle transitions and current-environment lookups. The queries preserve named profiles outside workspace data and retain lifecycle metadata; live page content and input stay out of these tables. Browser scenarios in the API tests exercise these queries on real PostgreSQL.

Port program options, wider admissions, full-body/effect request fences and
once-daily link audit use are durable queries. Their real database cases use
the initial migration on fresh test databases. Connection records are live
memory and have no durable traffic table.

| Module | Planned responsibility |
|--------|------------------------|
| [queries::ports](src/queries/ports.rs) | Durable port program options, wider admissions and their request fences. |
| [pool](src/pool.rs) | Database connections and transaction entry points. |
| [programs](src/programs.rs) | The agent programs the system knows: those Orch ships, and those the connectors of its machines have reported. |
| [migrations](src/migrations.rs) | Apply canonical SQL migrations. |
| [migrations/progress](src/migrations/progress.rs) | Migration step observations without changing the migrator's locking, checksums or transaction boundaries. |
| [seeds](src/seeds.rs) | Editable built-in defaults: the system's defaults page, capabilities, profiles, the two seeded templates and the rate catalogue a release comes with; no rate card and no caps. |
| [queries/identity](src/queries/identity.rs) | Users and their state, each with an org of their own, what the interface keeps for them, sessions, tokens, identity providers, invitations, the browsers a person signed in from, codes sent by mail, and the counts of wrong tries by name. |
| [queries/profiles](src/queries/profiles.rs) | Personal pages and ordered pins, with serialized changes and daily inspection deduplication. |
| [queries/first_steps](src/queries/first_steps.rs) | Stored candidates for permission-filtered setup observations and atomic personal choices. |
| [queries/chats/people](src/queries/chats/people.rs) | Frozen mention targets and visible asked-agent input references stored with a chat message. |
| [queries/chats/following](src/queries/chats/following.rs) | Current thread-owner and whole-follow reasons for a chat, and revision-fenced read positions. |
| [queries/chats/posting](src/queries/chats/posting.rs) | Per-chat human posting stops and the durable rolling-hour count of successful agent posts. |
| [queries/documents/copy](src/queries/documents/copy.rs) | Current document material copied through ordinary fresh blocks and independently pinned files. |
| [queries/documents/containers](src/queries/documents/containers.rs) | Document-only container rows, exact membership, structural locks and committed operation receipts. |
| [document_containers](tests/document_containers.rs) | Native typed parent/namespace constraints and complete candidate continuation reads. |
| [queries/reconciliation](src/queries/reconciliation.rs) | Startup work already held by live threads, read in exact bounded pages without creating another turn, job or lease. |
| [queries/transaction_guard](src/queries/transaction_guard.rs) | Current request identities locked with authorization changes until their action transaction ends. |
| [queries/lifecycle](src/queries/lifecycle.rs) | Thread settlement, committed close commands and retained environment removal references. |
| [queries/goals](src/queries/goals.rs) | A thread's goal and the reports its agent made on it: the rows the goal loop reads and the person's acts write. |
| [queries/hierarchy](src/queries/hierarchy.rs) | Nodes with orgs inside orgs and the two limits of a root org, topics, tags, favorites and recents; what was deleted and when it is purged. |
| [queries/everything](src/queries/everything.rs) | Indexed hierarchy pages whose visibility is supplied by the shared access engine, with stable keyset cursors. |
| [queries/access](src/queries/access.rs) | Members with their domain, roles held in places, permission rows and entries with the Owners row, suspensions in an org, groups, access requests and the authorization epoch. |
| [queries/access/participation](src/queries/access/participation.rs) | Personal assignment and follow facts for a permission frame, never a grant of access. Its SQL aggregates descendant assignments into requested ancestors without loading ordinary thread pages. |
| [queries/activity](src/queries/activity.rs) | A place's activity at two depths, with repeated meaningful acts grouped before paging. |
| [queries/settings](src/queries/settings.rs) | Settings and secret metadata. |
| [queries/recovery_codes](src/queries/recovery_codes.rs) | Hashed one-use authenticator recovery codes, replaced and consumed in the caller's transaction. |
| [queries/threads](src/queries/threads.rs) | Threads with their side threads, segments, inputs and turns with what each was let past, items, reads and drafts. |
| [queries/threads/reading](src/queries/threads/reading.rs) | Bounded conversation windows and only the historical metadata their references require. |
| [queries/thread_continuations](src/queries/thread_continuations.rs) | Immutable prefix spans, actual suffix ownership, version groups, durable operation/native-creation receipts and independent queue/read state. |
| [queries/thread_continuations/reading](src/queries/thread_continuations/reading.rs) | Indexed scoped row identities and bounded frozen-body decoding, with shared pin/quote metadata. |
| [queries/thread_history](src/queries/thread_history.rs) | Selected history aggregates and export pages share immutable-prefix membership and frozen item bodies. |
| [queries/source](src/queries/source.rs) | Frozen source choices, explicit work observations and recorded file-state associations. |
| [queries/approvals](src/queries/approvals.rs) | Approvals and decisions; announcements with their category and whom they went to. |
| [queries/archive_plans](src/queries/archive_plans.rs) | The exact subtree and settled-work identity reviewed before an archive or its inverse. |
| [queries/binding_start](src/queries/binding_start.rs) | Startup harness discovery candidates and locally found subscriptions, without creating a thread. |
| [queries/chats](src/queries/chats.rs) | Chats, members, messages, quoted items and per-person states. |
| [queries/documents](src/queries/documents.rs) | Documents, their blocks, accepted batches of changes, versions with their cause and what each changed, discussions and comments, files and per-person states. |
| [queries/documents/batch](src/queries/documents/batch.rs) | A batch of changes applied to a document's rows, and a version put back: whole, under the lock on the document's row, or not at all. |
| [queries/documents/history](src/queries/documents/history.rs) | The versions of a document: cut, listed, read, kept within a number and a size, and cut for a document that was left alone. |
| [queries/documents/discussions](src/queries/documents/discussions.rs) | Discussions beside a document and the comments in them: opened on a block, answered, reacted to, resolved, edited and deleted. |
| [queries/documents/people](src/queries/documents/people.rs) | Personal discussion choices and current discussion-start writers for documents. |
| [queries/exports](src/queries/exports.rs) | Archives of what a person may read in a place: whose each is, what was asked, where it stands and for how long it is kept, and the statements that count what an archive would hold. |
| [queries/imports](src/queries/imports.rs) | Imports into a place: whose each is, what was asked, where it stands, the report with a row for each object, and for each thing an import made or took over the thing of its source that it corresponds to, so that a later import finds it and an import can be taken back. |
| [tests/imports_storage](tests/imports_storage.rs) | Import page indexes bound examined rows, including cached generic plans and live node-order maintenance. |
| [queries/imports_chats](src/queries/imports_chats.rs) | What bringing the messages of a chat needs beyond the ordinary queries of chats: which messages of a source are here already, the messages an import adds in one statement with the original time and author, and the taking back of them. |
| [queries/imports_threads](src/queries/imports_threads.rs) | What bringing a thread as history needs beyond the ordinary queries of threads: the rows of its one imported segment, its items and turns under identifiers an import derived, and the taking back of what an import added. |
| [queries/package](src/queries/package.rs) | What an Orch package says of the things in it besides their records: who made each place, folder and document and when, what a document is about, where it lies, and who the people are that a package names, with the address of those whose address its maker may read. |
| [queries/shares](src/queries/shares.rs) | What agents were given: copies with their delivery and revocation, and live shares by area. |
| [queries/resources](src/queries/resources.rs) | Machines, hosts with their owners, load and storages, providers with their endpoints, profiles, harnesses, and models with their two names and what each has enabled. |
| [queries/provider_facts](src/queries/provider_facts.rs) | Durable provider observation timing, discovered model contexts and confirmed reset receipts. |
| [queries/provider_plan](src/queries/provider_plan.rs) | Plan-window measurements and early start-overs, kept with provider observations and read with their audit markers. |
| [queries/search](src/queries/search.rs) | Search candidates, readable content scopes, stable pages and repair of stored text projections. |
| [queries/semantic](src/queries/semantic.rs) | Search by meaning: the sources an org's index cuts, their pieces, the generations of vectors and where indexing stands. |
| [queries/related](src/queries/related.rs) | Bounded related-item candidates from live places, stored typed references and generation-owned item vectors. |
| [queries/links](src/queries/links.rs) | What refers to a document, what it refers to and the neighbourhood around it, read from the edge store of typed references. |
| [queries/environments](src/queries/environments.rs) | Templates of both kinds, runtimes of both pools with their successors, environments with their owners, homes, what each exec node offers, when each was last used and its scheduled removal, the threads in them, mounts with their locations, and MCP servers: presets, and those imported on a machine. |
| [queries/environment_facts](src/queries/environment_facts.rs) | Environment services and the policy of their hosting machine, resolved without placement assumptions. |
| [queries/storage](src/queries/storage.rs) | Blobs, uploads, file cache, snapshots, and shells with whether somebody is attached and the last of what each printed. |
| [queries/uploads](src/queries/uploads.rs) | Completed upload ownership, saved sources of file bytes and serialized draft references. |
| [queries/terminals](src/queries/terminals.rs) | A person's shells in an environment: the sessions with whether somebody is attached, how each ended and the last of what it printed, the counts the limits are checked against, and what the sweeps look for. |
| [queries/jobs](src/queries/jobs.rs) | Jobs with their phases and locks, and confirmed preflight plans. |
| [queries/job_leases](src/queries/job_leases.rs) | Leases on jobs: one process at a time runs a job, or holds a role such as the realtime coordinator, by name, with no transaction held open. |
| [queries/notifications](src/queries/notifications.rs) | Subscriptions, inbox, preferences, channels with their texts, routes with what each has sent, destinations, outbox and delivery records. |
| [queries/notifications/channels](src/queries/notifications/channels.rs) | Channels and destinations: where they are set up, the texts they carry, the secrets that belong to them and what the last delivery through each came to. |
| [queries/notifications/outbox](src/queries/notifications/outbox.rs) | The outbox: rows claimed under a lock so that two workers never take one, delivered, retried and finished, and what the engine and the worker write into it. |
| [queries/notifications/people](src/queries/notifications/people.rs) | The people a message is for and the places it is about: their language, time zone, confirmed address and quiet hours; their browsers that take push; the places notifications happened at; and the Inbox row a notification starts as. |
| [queries/notifications/routes](src/queries/notifications/routes.rs) | A person's routes through channels: switched on and off, linked by a code, what still waits to be told through them, and how far each has told. |
| [queries/notifications/upkeep](src/queries/notifications/upkeep.rs) | What the worker's schedule removes or trims: spent and expired sign-in material, soft-deleted rows after their days, old raw events and metric minutes, recents beyond a person's share, and the machine certificates that are about to end. |
| [queries/notifications/events](src/queries/notifications/events.rs) | What the engine reads to tell an event, and the one row it writes beside the Inbox: the first line of what an agent last said, the job that ended, and the delivery to a node's destination, written once. |
| [queries/notifications/inbox](src/queries/notifications/inbox.rs) | A person's Inbox as the pages read it: the list newest first, the counters of the badge, what is marked read, an approval that needs nobody any more, and what the person chose about being notified. |
| [queries/notifications/pages](src/queries/notifications/pages.rs) | What the pages of channels, routes and destinations read and change: the ways out that reach a person, their routes through them, the destinations of a node, and the browsers that take push. |
| [queries/notifications/subscribers](src/queries/notifications/subscribers.rs) | Who follows a place and which destinations are told of it: the nearest subscription of each person on the way down to a node, the automatic row of whoever owns or writes in a thread, and the destinations set at a node or above it. |
| [queries/link](src/queries/link.rs) | Committed stream cursors and commands. |
| [queries/machine_streams](src/queries/machine_streams.rs) | Immutable harness machines for segment streams. |
| [queries/model_inputs](src/queries/model_inputs.rs) | What Orch supplied to a model, as it was sent (`model_inputs`): written before the sending, settled after it. |
| [queries/machine_reports](src/queries/machine_reports.rs) | Machine sources of environment, harness, provider and host reports before any effects. |
| [queries/machine_refusals](src/queries/machine_refusals.rs) | Hourly system audit rows for refused machine reports, without report content. |
| [queries/network](src/queries/network.rs) | What is stored of runtimes' networks: the state their host's node last reported, the restriction retained on an old unit, and the network their template asks for. |
| [queries/metrics](src/queries/metrics.rs) | Stored metric samples: written by each service, folded into hours that are kept, read as series over any span, and the minutes dropped a day at a time. |
| [queries/feeds](src/queries/feeds.rs) | What the realtime socket reads of the database beside the rows of a thread: where each feed stands, the Inbox rows of a person after a number, what is gone, and how long the token of a socket is good. |
| [queries/raw_events](src/queries/raw_events.rs) | The raw events of a segment: the frames that crossed between Orch and a harness, kept for diagnosis and dropped with their month. |
| [queries/adoption](src/queries/adoption.rs) | What taking in a session that already exists reads and writes: the connected environments of a machine, and whether the session of a segment is one that Orch has yet to open. |
| [queries/agent_history](src/queries/agent_history.rs) | Stored native history observations and thread-locked removal reservations. |
| [sequences](src/sequences.rs) | Transactional feed sequences and database notifications. |
| [secrets](src/secrets.rs) | At-rest sealing for stored secrets. |
| [audit](src/audit.rs) | Append-only audit writes, kept for good, with the rows that are lines of a thing's history and the mark of an override. |
| [audit/emitters](src/audit/emitters.rs) | Linked production audit emission sites, independent of catalogue declarations and tests. |
| [idempotency](src/idempotency.rs) | Request replay records. |
| [usage/ledger](src/usage/ledger.rs) | Append-only usage events, each with what it ran on (model, provider, agent, kind of environment, place, person) and the rate it was counted at, kept for good. |
| [usage/rollup](src/usage/rollup.rs) | Counted consumption in tokens and credits, tokens without a rate, the rollups by day and by month for every dimension, and reproducible rebuilds. |
| [usage/performance](src/usage/performance.rs) | Mergeable request performance histograms and their percentile figures. |
| [usage/caps](src/usage/caps.rs) | Usage caps, period sums, turns running at once counted under a lock, sent notices, and each time a turn was let past one. |
| [usage/micros](src/usage/micros.rs) | Exact amounts counted to a millionth: read from and written as the decimal numbers the ledger and the caps keep, added and compared without a floating-point number. |
| [reconciliation](src/reconciliation.rs) | Startup recovery queries. |
| [heartbeat](src/heartbeat.rs) | Database-backed component health. |
| [queries/browsers](src/queries/browsers.rs) | Browser session reservations, their lifecycle and the environment facts that bound them. |
| [queries/displays](src/queries/displays.rs) | Display reservations and lifecycle keep credentials separate from public rows. |
| [queries/hierarchy/presentation](src/queries/hierarchy/presentation.rs) | Atomic node text and icon colour changes, initial colours and child lists narrowed by kind. |
| [queries/documents/tree](src/queries/documents/tree.rs) | Tree stamps for the node a document stands beside when its placement or visibility changes. |
| [queries/resources/reenrollment](src/queries/resources/reenrollment.rs) | Existing machines locked and reactivated with replacement certificates while retaining their registrations. |
| [queries/resources/mounts](src/queries/resources/mounts.rs) | Mount resources, their machine locations and the settings that enable them. |
| [queries/resources/mcp](src/queries/resources/mcp.rs) | MCP preset and environment-owned resources and their enablement. |
| [queries/resources/skills](src/queries/resources/skills.rs) | Skills, their versions with archive and manifest, drafts with their files, and the versions a turn was given. |
| [queries/resources/template_imports](src/queries/resources/template_imports.rs) | Online hosts whose reported drivers can propose a template without allocating a unit. |

| [retained](src/queries/environments/retained.rs) | Frozen owned volume references and revisioned removal, restoration and confirmed purge. |
| [temporary](src/queries/environments/temporary.rs) | Temporary environment deadlines, their creating thread, and the owner's warning window. |

| [tokens](src/queries/tokens.rs) | What keeps an API token out beyond its scopes: the orgs that refused it, the longest life a place admits, and the one-use proofs a sensitive act spends. |
| [outsiders](src/queries/outsiders.rs) | People from outside a root tree who reach something in one of its orgs, and each way they do: entries and roles that name them, and those that name a group, an org's members or a role of another place that they belong to. |
| [second_step](src/queries/second_step.rs) | The second step an org can require, as sessions give it, and where a person was last seen inside an org: when a session last gave its second proof, whether any place requires one, and the one coalesced time of use per org and person. |
| [trace](src/queries/trace.rs) | Permanent event-time audit scope, captured names and minimal deletion lifecycle facts. |
| [queries/input_schedules](src/queries/input_schedules.rs) | Frozen future inputs and distinct civil occurrences, claimed and stamped in their owning transaction. |

`queries/session_dependencies.rs` projects accepted native session delivery from the durable command acknowledgement into its existing segment, preserving unknown legacy memberships and command ordering.
| `queries::search::cancellation` | Cancelling a search read by its backend and transaction tag without cancelling a reused connection. |

Search permission seeds reuse the exact frame with only the indexed candidate inventories and their dependencies.
Selected content batches retain those same frames and ancestry while omitting unrelated places. Native
scope identities and PostgreSQL planner estimates choose a scoped or global entrance without counting
corpus rows per request. Recent windows read covering metadata first and fetch text only after the window.

| `queries::search::candidates` | Indexed lexical candidate windows whose exact access and occurrence ranks are decided before payloads are returned. |

| `queries::search::sparse` | Conservative indexed candidate identities for a filtered nearest search, bounded before choosing an exact scan. |

| `queries::search::candidates::fields` | Native field projections keep their recency and text indexes visible to the candidate planner. |

| [queries::loss](src/queries/loss.rs) | Locked inventories and terminal records for explicitly confirmed permanent machine or environment loss. |

| [queries/notifications/operations](src/queries/notifications/operations.rs) | One renewable Inbox item per person for a reviewed operation on the same resource set. |

| [programs/admin](src/programs/admin.rs) | Bounded program and build pages, database counts and representative declarations for system administration. |

| [queries/threads/notices](src/queries/threads/notices.rs) | Indexed deferred notices keep append, retry and next-turn reads independent of queue history. |

| [tests/pending_notices](tests/pending_notices.rs) | Deferred notices preserve values and retries while reading and consuming only one indexed page. |

`tests/storage_scale.py` applies the complete old schema before upgrading populated queues, then checks the actual capacity and notice SQL at two disk-backed corpus sizes ten times apart.
