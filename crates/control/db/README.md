# orch-db

PostgreSQL persistence for Orch: the pool and its transaction entry points, the queries that read and write each concern, the migrations, and the projections that the API and the engine read. Nothing here decides who may do what; it stores and reads what the layers above decide.

## What is in this package

| Directory | What it holds |
|---|---|
| [src](src) | The pool, the queries (`src/queries`), idempotency, audit, sequences, usage, secrets, seeds and reconciliation; the table below lists them |
| [tests](tests) | Test files, each named for the behavior it checks, run against a real database |
| [migrations](../../../migrations) | The numbered SQL files that make the schema. A released migration is never edited: a change is a new file, and `migrations/RELEASED` records the checksum of each one shipped at a tag |

## Build and test

```sh
cargo build -p orch-db
cargo test -p orch-db --test <name>     # one test file
cargo xtask verify                       # format, Clippy, build, docs and dependency checks
```

The tests run against a real PostgreSQL and make a database of their own for each test. They use the server named by `ORCH_TEST_DATABASE_URL`, or a PostgreSQL 17 on `127.0.0.1:55432` with the user and password `orch`. See [CONTRIBUTING.md](../../../CONTRIBUTING.md) for the checks every change passes.

## Module map

### Source

| Module | What it does |
|--------|--------------|
| [queries/thread_history_cuts/scale_tests](src/queries/thread_history_cuts/scale_tests.rs) | Million-row indexed uncertainty checks and batched turn totals over 1,000 threads. |
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
| [queries/imports_chats](src/queries/imports_chats.rs) | What bringing the messages of a chat needs beyond the ordinary queries of chats: which messages of a source are here already, the messages an import adds in one statement with the original time and author, and the taking back of them. |
| [queries/imports_threads](src/queries/imports_threads.rs) | What bringing a thread as history needs beyond the ordinary queries of threads: the rows of its one imported segment, its items and turns under identifiers an import derived, and the taking back of what an import added. |
| [queries/package](src/queries/package.rs) | What an Orch package says of the things in it besides their records: who made each place, folder and document and when, what a document is about, where it lies, and who the people are that a package names, with the address of those whose address its maker may read. |
| [queries/shares](src/queries/shares.rs) | What agents were given: copies with their delivery and revocation, and live shares by area. |
| [queries/resources](src/queries/resources.rs) | Machines, hosts with their owners, load and storages, providers with their endpoints, profiles, harnesses, and models with their two names and what each has enabled. |
| [queries/provider_facts](src/queries/provider_facts.rs) | Durable provider observation timing, discovered model contexts and confirmed reset receipts. |
| [queries/provider_credits](src/queries/provider_credits.rs) | Exact provider-level credit observations, current source generations, bounded group history and existing metrics retention; no ledger arithmetic. |
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
| [queries::loss](src/queries/loss.rs) | Locked inventories and terminal records for explicitly confirmed permanent machine or environment loss. |
| [queries/notifications/operations](src/queries/notifications/operations.rs) | One renewable Inbox item per person for a reviewed operation on the same resource set. |
| [programs/admin](src/programs/admin.rs) | Bounded program and build pages, database counts and representative declarations for system administration. |
| [queries/threads/notices](src/queries/threads/notices.rs) | Indexed deferred notices keep append, retry and next-turn reads independent of queue history. |

### Tests

| Test file | What it checks |
|-----------|----------------|
| [tests/usage_commit_order](tests/usage_commit_order.rs) | Usage cursors number each changed row at commit while retries, savepoints and cancellation preserve atomicity. |
| [tests/usage_commit_scale](tests/usage_commit_scale.rs) | A million ledger rows commit as one bounded-memory cursor range without changing their token totals. |
| [tests/participation_summaries](tests/participation_summaries.rs) | Participation keeps descendant assignments, exact deadlines and transactional moves without reading the owned inventory. |
| [tests/document_reference_storage/chunks](tests/document_reference_storage/chunks.rs) | Multi-chunk edits retain current block state and exact counters while refusing and rolling back later conflicts. |
| [tests/document_reference_storage](tests/document_reference_storage.rs) | Stored reference projections preserve an existing database and keep native edits, anchors and paging exact. |
| [document_containers](tests/document_containers.rs) | Native typed parent/namespace constraints and complete candidate continuation reads. |
| [tests/imports_storage](tests/imports_storage.rs) | Import page indexes bound examined rows, including cached generic plans and live node-order maintenance. |
| [tests/pending_notices](tests/pending_notices.rs) | Deferred notices preserve values and retries while reading and consuming only one indexed page. |

### Named in code

| Name | What it does |
|------|--------------|
| `queries::search::cancellation` | Cancelling a search read by its backend and transaction tag without cancelling a reused connection. |
| `queries::search::candidates` | Indexed lexical candidate windows whose exact access and occurrence ranks are decided before payloads are returned. |
| `queries::search::sparse` | Conservative indexed candidate identities for a filtered nearest search, bounded before choosing an exact scan. |
| `queries::search::candidates::fields` | Native field projections keep their recency and text indexes visible to the candidate planner. |
