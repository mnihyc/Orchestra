# orch-engine

The durable orchestration of Orch, hosted inside the API. An input becomes a turn, and a turn becomes a conversation. A person's input is queued, checked against everything that stands between it and an agent program, turned into a turn with a lease and a segment, carried to the agent program on the commands of the machine link, and what the agent program does comes back as events that are applied exactly once. Around that core sit forks and switches, approvals, summaries, environments and their lifecycle, imports and exports, and the tools Orch gives agents.

Everything the engine remembers is in the database and changes by compare-and-set, so a process that is restarted at any moment finds its work where it was left and goes on. What it needs of the rest of the system it asks through three small traits: `commands::Commands` (send a command to a machine and learn how it ended), `access::ResourceAccess` (may this person use this provider, profile or environment) and `live::LiveSink` (growing text, forwarded at once). Decisions about nodes it asks of [orch-authz](../authz/README.md).

## What is in this package

| Directory | What it holds |
|---|---|
| [src](src) | The engine, one module for each concern; the table below lists them |
| [tests](tests) | Test files, each named for the behavior it checks, run against a real database and a scripted machine link |
| [examples](examples) | `tool_catalogue`: prints the catalogue of Orch's own agent tools from the engine's definitions |
| [data](data) | `token-density.json`, the estimate of how text becomes tokens |

## Build and test

```sh
cargo build -p orch-engine
cargo test -p orch-engine --test <name>            # one test file
cargo run -p orch-engine --example tool_catalogue  # print the tool catalogue as JSON
cargo xtask verify                                  # format, Clippy, build, docs and dependency checks
```

The tests run against a real PostgreSQL and make a database of their own for each test. They use the server named by `ORCH_TEST_DATABASE_URL`, or a PostgreSQL 17 on `127.0.0.1:55432` with the user and password `orch`. See [CONTRIBUTING.md](../../../CONTRIBUTING.md) for the checks every change passes.

## Module map

### Source

| Module | What it does |
|--------|--------------|
| [calls](src/calls.rs) | Durable model-call recovery, guarded per subject and executed under the interrupted input author's authority. |
| [agent_history](src/agent_history.rs) | Managed native history inventories, idle removal and durable reconstruction fences. |
| [agent_history/recovery](src/agent_history/recovery.rs) | A missing native session resumes from a frozen conversation through the switch handoff helpers. |
| [agent_history/notices](src/agent_history/notices.rs) | Recovery handoffs stay with their native recipient and become history after delivery. |
| [agent_history/sides](src/agent_history/sides.rs) | Recovered sides keep their original source grants and their own recipient admission. |
| [loss](src/loss.rs) | Reviewed permanent loss closes unreachable work atomically and preserves its recoverable identity. |
| [recovery](src/recovery.rs) | Lost environments are provisioned again on reviewed owned volumes or verified cached workspace bytes. |
| [operation_access](src/operation_access.rs) | Loss and relocation check ordinary authority and make reviewed administrative self-grants through permission pages. |
| [jobs/host_delete](src/jobs/host_delete.rs) | A host is removed only after its reviewed independent cascades and unchanged retained-data promises finish. |
| [jobs/mcp_offering](src/jobs/mcp_offering.rs) | A rediscovered native MCP definition is copied on its own machine before its retained resource is offered again. |
| [notifications/operations](src/notifications/operations.rs) | Reviewed operation audiences and a single renewable notification for each affected person. |
| [provider_recovery](src/provider_recovery.rs) | A provider whose physical runtime is lost is explicitly provisioned again under its manager's own destination access. |
| [ports](src/ports.rs) | Current port policy, reviewed admission production and irreversible reconciliation. |
| [port_capabilities](src/port_capabilities.rs) | Port audiences admitted by the environment's capability grants and current policy. |
| [recycle](src/recycle.rs) | Idle fleet selection and recycling through the ordinary credential-returning stop path. |
| [engine](src/engine.rs) | The engine: what it is made of, how it is started, and how work is found. |
| [error](src/error.rs) | What can go wrong inside the engine, and does not belong to one of its requests. |
| [access](src/access.rs) | What the engine asks of the access engine about resources. |
| [commands](src/commands.rs) | What the engine asks of the machine link: send a command, and learn how it ended. |
| [live](src/live.rs) | What the engine tells the realtime side the moment it knows it. |
| [config](src/config.rs) | The waits and the pace of the engine, from the deployment's configuration. |
| [context_fill](src/context_fill.rs) | What fills a thread's context and what its turns used beyond their totals: each harness's report kept with its turn, Orch's own estimate by part where a harness breaks nothing down, and a count asked of the harness on request. |
| [ids](src/ids.rs) | The identifiers of the commands the engine sends. |
| [resolved](src/resolved.rs) | The settings a thread runs with, read once. |
| [unit](src/unit.rs) | The transaction a change of a thread is made in. |
| [views](src/views.rs) | A thread's rows as the wire has them, and what the readers of a thread are given. |
| [binding](src/binding.rs) | Arrangements by type: what an adapter, a provider and an environment make possible; what the provider's owner, the entry, policy and the machine allow; the executor, and the tool set that the environment's offers come to for the harness, taken at each session; default binding and fallbacks. |
| [programs](src/programs.rs) | What an agent program and its executors are called, as payloads carry them: the kind with its name in each language, and an executor with its tool set. |
| [token_density](src/token_density.rs) | What a model's tokenizer comes to, from the catalogue of what was measured, for the estimates where nobody counted. |
| [preflight](src/preflight.rs) | Atomic checks before a turn: each act against whoever does it (the author of an input; the thread's first owner for an input no person sent, which waits without holding the others up), availability, model, effort and confinement; the limits an input was let past left out. |
| [preparation](src/preparation.rs) | An input's files written before a turn, a lease or a model request exists; an input whose files cannot be had is held, each file named with why, until its author recovers it. |
| [trust](src/trust.rs) | A workspace's own harness configuration: asked once of the people who manage the environment when a harness finds one (Trust, Keep it out), never of an agent; the threads that meet it wait; the answer remembered for the environment and the harness, changed on its page, and carried by every session opened after it. |
| [caps](src/caps.rs) | Usage caps: coverage, per-person precedence, room in tokens, in credits and in turns running at once, held inputs and waiting threads released in order, stopping turns and notices; and the limit bypass, by which one turn is let past caps that are reached, an entry for each person who did, checked again at its start and recorded. |
| [rates](src/rates.rs) | The rate a turn is counted at: a provider's own, the rate card, or the shipped catalogue of estimates, in that order; one rule for every pattern that names models; the credits counted from it, marked where they are an estimate; and the tokens that have no rate. |
| [models](src/models.rs) | A provider's models: the name that is sent and the name that is shown, what an endpoint's list gives of each, the one pattern rule that allowlists, caps and rates share, and the speeds and access programs a manager enables one by one. |
| [native_actions](src/native_actions.rs) | Effective native catalogues are read from exact open sessions and never inferred from names or files. |
| [leases](src/leases.rs) | Per-turn provider authorization, forwarder allowance and closure. |
| [credentials/sharing](src/credentials/sharing.rs) | Issue and renew shared copies while provider custody remains. |
| [credentials/handover](src/credentials/handover.rs) | Exclusive custody transfer, return, retry and lost-credential handling. |
| [turns](src/turns.rs) | Turn state transitions, the phase a turn is in as its thread's status line says it, interrupt and timeouts. |
| [unit_subjects](src/unit_subjects.rs) | What a unit's request tells its host's operator hooks about what the unit serves, and why the unit was made. |
| [inputs](src/inputs.rs) | Queue, steer, interrupt and stop by whoever may drive the thread, attribution and the author's own access, forwarded messages, pending announcements and share notices, and inputs no person sent, asked of the thread's first owner. |
| [delivery](src/delivery.rs) | Files of an input, shared copies and what an agent downloads written into the workspace; large ones as visible jobs. |
| [initial_files](src/initial_files.rs) | Authorized initial workspace selections delivered before a new environment becomes runnable. |
| [jobs/restart](src/jobs/restart.rs) | A durable same-unit environment restart that keeps every file and lets queued inputs continue. |
| [manifest](src/manifest.rs) | Lists of a tree: the zstd-compressed JSON Lines blob a machine uploads when a tree changed, which the API keeps, reads and writes. |
| [selection](src/selection.rs) | Bounded workspace selections shared by file-copy plans and cached manifests. |
| [semantic](src/semantic.rs) | Search by meaning: what an org chose, its text made into vectors through the API provider it named, and those vectors kept and searched. |
| [related](src/related.rs) | Related documents, chats and threads ranked by current-generation meaning, shared place and typed references. |
| [links](src/links.rs) | Which sources refer to a document, and the graph around it, found only through what the reader may read, in an order a cursor continues. |
| [semantic/choice](src/semantic/choice.rs) | What an org chose for search by meaning: whether it is on, which provider and model make its vectors and who named them, the disk it may take, and how it cuts, finds and joins. |
| [semantic/embed](src/semantic/embed.rs) | Texts made into vectors through an org's embeddings provider: the named writer's present right to use it, its state and caps, the command to its runtime, and the use counted under its own purpose. |
| [semantic/vectors](src/semantic/vectors.rs) | The arithmetic of vectors: their length, how alike two are, and a vector shortened to fewer numbers. |
| [semantic/chunks](src/semantic/chunks.rs) | Cutting what a source says into pieces of about one size, each the text of one vector. |
| [semantic/plan](src/semantic/plan.rs) | What a change of the embeddings named for an org means for the vectors it has, and whether a new set fits. |
| [semantic/indexer](src/semantic/indexer.rs) | The indexer: an org's text cut into pieces and made into vectors in the background, newest first, pausing with its reason before the disk share is passed, and a new generation built beside the one that answers. |
| [semantic/state](src/semantic/state.rs) | Where search by meaning stands for an org, as an admin reads it: what it chose, whether the database can keep vectors, the generations, progress with an estimate, the disk, and why it waits. |
| [semantic/query](src/semantic/query.rs) | Results by meaning for a query: the query made into a vector with an org's named embeddings, and the pieces of the generation that answers nearest to it, for the caller to check against what the reader may read and to join with results by words. |
| [semantic/fusion](src/semantic/fusion.rs) | Reciprocal-rank fusion: one order made of the order by words and the order by meaning. |
| [volume_carry](src/volume_carry.rs) | Measured owned-volume selectors and durable whole-volume carry before a destination unit first mounts its private layers. |
| [instructions](src/instructions.rs) | What an agent is told that is no person's message: the preamble and the thread's standing instructions with every session; and `AGENTS.md` of a new thread's directory, written once where its maker wrote one and never otherwise. |
| [instruction_sources](src/instruction_sources.rs) | Instruction source refreshes from native reports, kept in order without adding a model input. |
| [model_inputs](src/model_inputs.rs) | What Orch supplies to a model, recorded before it is sent and settled once its sending ends: a session's opening text and each input's text (a turn's start, a steer), block by block, as `model_inputs` keeps it. |
| [memory](src/memory.rs) | Where an agent's notes are kept (its thread's directory) and by whom (its harness, or the agent with its tools), said to the adapter at each session; taken along by a switch, and by a fork that takes history or asks for them; nothing where the setting is off. |
| [sharing](src/sharing.rs) | Sharing with agents: a copy written in the written form of each thing, or a live share by area held as entries on pages, to read or to write, of a thing, of a place with what is in it or of a place by itself; taking either back. |
| [material](src/material.rs) | Current topic-document policy and first-reading paths through the ordinary shared-tree resolver; no bodies in context. |
| [exports](src/exports.rs) | The export of a place: what a person may take away of the conversations, chats and documents in an org, a project or folder, a topic or a thread, planned before anything is made, asked for as a job, and kept as an archive for them alone to fetch for a day. |
| [export_layout](src/export_layout.rs) | How an archive is laid out: a directory for each place, by its name, and in it `conversations/<title>` for each thread, `chat` and `docs/<title>`, with the files and the versions of a thing beside it; every name its own among its neighbours, and a place that its reader may not see, named by nothing. |
| [imports](src/imports.rs) | Importing into a place: what a person brings from an Orch package or from plain files, planned before anything is made, asked for as a job, kept as a report and taken back as a whole. |
| [jobs/export](src/jobs/export.rs) | The job that writes an archive of what a person may read in a place: each thing read as that person at the moment it is written, laid out as the tree an agent is given, streamed into a zip beside the disk and put in the store of content, with an index and a file that says what it holds. |
| [jobs/import](src/jobs/import.rs) | The job that brings an import: the source opened and every record and file of a package checked against the hashes of its manifest, the plan made as the person who asked, the objects made in batches that ask the access engine again for each kind they make, and a report with a row for each object. |
| [copies](src/copies.rs) | The files of a copy given to an agent: a conversation, a chat or a document in its written form, or a place with everything in it that the person who shares may read, laid out as the shared tree is, and an index of them that counts what was left out. |
| [document_tree](src/document_tree/mod.rs) | Document-only containers and atomic organization using current person or agent authority. |
| [document_tree/types](src/document_tree/types.rs) | Typed document destinations, actors, complete operation plans and replay receipts. |
| [document_tree/error](src/document_tree/error.rs) | Safe document-tree refusals, distinct from native storage and authorization failures. |
| [document_tree/access](src/document_tree/access.rs) | Batch document authority through the native person pages or the calling agent's own reach. |
| [document_tree/read](src/document_tree/read.rs) | Batched typed document entries and redacted ancestry for person and agent navigation. |
| [document_tree/create](src/document_tree/create.rs) | Guarded text headers and ordinary destination ownership without agent Manage grants. |
| [document_tree/prepare](src/document_tree/prepare.rs) | Complete bounded membership, scope locks and authorization before document-tree mutation. |
| [document_tree/audience](src/document_tree/audience.rs) | Current audience sets for truthful operation receipts, separate from the acting agent's authority. |
| [document_tree/copying](src/document_tree/copying.rs) | Fresh document-tree material through normal headers, content batches and independent file pins. |
| [document_tree/operations](src/document_tree/operations.rs) | Atomic document-tree acts, exact deletion cohorts and actor-scoped durable replay. |
| [approval_rules](src/approval_rules.rs) | Narrow answers allowed for one thread: kept by Orch where the adapter verifies the whole matcher, bound to the thread's binding, answering a later request offering the same matcher once, taken back by a person under the thread's lock, ended by a change of binding, never copied to a fork or a side thread, never written into a harness's settings. |
| [approvals](src/approvals.rs) | Harness, access and capability decisions; access requests asked for and decided part by part, each sent to the nearest who can decide. |
| [approval_review](src/approval_review.rs) | Native reviewer model policy, session facts and reported review usage; independent of boundary requests. |
| [providers](src/providers.rs) | Provider runtimes as a pool of their own: made when a provider is added and kept as long, started, stopped, stopped when idle, and started while stopped so that a sign-in is renewed; sign-in on the provider, kept on its own volume through every successor; the arrangement types its owner allows, and what its runtime has fallen behind on. |
| [provider_policy](src/provider_policy.rs) | Provider address policies checked at binding, switch destinations and every turn preflight. |
| [provider_runtime](src/provider_runtime.rs) | A managed provider's runtime through its life: started when a thread needs it and by hand, stopped by hand and when idle, retired with the provider and taken away, and what a turn waits for while the unit comes up. |
| [provider_setup](src/provider_setup.rs) | A provider's setup as its adapter declares it: the template and the host its runtime is made from, a sign-in with sealed values that wait for the runtime, a sign-in copied in again by the same method, its state, and a token's end. |
| [provider_sources](src/provider_sources.rs) | Provider source specs keep secrets beside the credential and observations attached to the unchanged choice. |
| [request_actions](src/request_actions.rs) | Engine requests accepted in the caller's action transaction before commands are sent to machines. |
| [provider_fact_reads](src/provider_fact_reads.rs) | Provider observation cadence from durable read attempts and recent bound-thread activity. |
| [harnesses](src/harnesses.rs) | The harness of each provider and profile, as one process or one for each conversation, and session supervision. |
| [sessions](src/sessions.rs) | Open, resume, fork for a side thread, segment and lost-session recovery. |
| [adoption](src/adoption.rs) | Taking in a session that a person began in a harness by hand: the thread that is attached to it, the environment it stands in, and the directory it works in. |
| [side_threads](src/side_threads.rs) | Side threads: a quick aside from a thread, set up as its parent is and starting from its history at any message; asked of the parent's page; how many may be open, closed by hand or when idle, and kept by an ordinary fork. |
| [environments](src/environments.rs) | Environments owned by people: creation with what is inherited, a topic's default, threads put into them by whoever manages them and their directories, what each is given (variables and secrets of its home node and its own), what it declares it needs, what its exec node offers, settings changed while it runs, lifecycle, idle stop, keep awake and recycle. |
| [history_pages](src/history_pages.rs) | A history read a page at a time, turn by turn, from its live tables or from a captured cut, oldest or newest first. |
| [hosting](src/hosting.rs) | What making a unit asks of the rest of the system: the token its connector enrolls with, the address and the authority it trusts, and where the programs it runs come from. |
| [reports](src/reports.rs) | What a host and an environment report: the state of its units, what a host has and uses, how far a job is, and what an environment is. |
| [mount_copies](src/mount_copies.rs) | What a move, fork or switch plan decides for each mount, and the copies of a mount's data its job makes. |
| [mounts](src/mounts.rs) | Mounts resolved for an environment from its home node and its own configuration, in read-only, shared or private mode; changes applied or marked stale. |
| [mounts/live](src/mounts/live.rs) | Applying resolved mount changes to a running unit, with explicit pending-recreate reasons on refusal. |
| [mcp](src/mcp.rs) | The MCP servers of a session: presets, servers imported from a harness's own configuration, which run from the copy their machine keeps, and servers an environment was given as its own; found, imported, enabled; and one that is enabled where its definition is not, unavailable with that reason. Accepted external revisions and Orch tool catalogues have separate typed session slots: a successful declared live reload acknowledges only the external revision; adapters without live reload resume with the new set at the next turn. |
| [skills](src/skills.rs) | The skills a thread is given: what its places name in `agents.skills` (off, the latest version or a pinned one) and, where allowed, its starter's own skills, each only while the one who named it may use it; and the versions a turn was given. |
| [skills/front_matter](src/skills/front_matter.rs) | A `SKILL.md`'s front matter read in the Agent Skills format: a name of lower-case letters, digits and single hyphens, a description, and what makes either refused. |
| [skills/archive](src/skills/archive.rs) | A skill's files read from a tar or zip archive or a list of files, refused with the reason, and written as one canonical archive with its manifest (path, size, SHA-256, executable bit). |
| [skills/versions](src/skills/versions.rs) | A skill's immutable versions: an archive stored once, its name kept from version to version and unique in its home. |
| [skills/resolve](src/skills/resolve.rs) | `agents.skills` and `agents.personal_skills` resolved for a thread, with each skill left out given its reason. |
| [shells](src/shells.rs) | A person's shells in an environment: theirs alone, kept while nobody is attached, one screen at a time, how many a person and an environment may have and which a person gives up for one more, the end of one that nobody came back to, and the last of what it printed kept in the environment's history. |
| [parcels](src/parcels.rs) | The files of a copy kept until the thread's next turn writes them into its workspace, a delivered copy taken out again, and a file an agent downloads written where it asked: the engine's own keeper, over the store of content and the machines. |
| [paste](src/paste.rs) | Folded text is verified from its private blob and sent inline only within the remaining context budget. |
| [places](src/places.rs) | Where an environment can run: the hosts its creator may use, capacity, what the units there declare they need (which reserves nothing), free room, when a host counts as full and whether it then warns or refuses, storages, mounts present, and scheduling. |
| [capabilities](src/capabilities.rs) | Requests decided on the host's side by scope, revocation and recreation consequences. |
| [tools/announce](src/tools/announce.rs) | What an agent tells the others: a notice to the threads of its topic or, where allowed, of its project, by the delivery, fallback and wake settings; or a milestone, which goes to the people who follow its thread unless it says the topic. |
| [tools/request_capability](src/tools/request_capability.rs) | Asynchronous environment capability requests. |
| [tools/source](src/tools/source.rs) | Fresh own-tool calls retain the exact selected source, logical reference and native effect stages. Writes and download target Read/Export/content recheck at durable admission after external preparation; retries rejoin their original reservations, commands, deadlines and input pins. Quiescence waits for actual completion, a successful original-key read or exact owned-runtime removal. Canonical completed answers remain factual replays. |
| [tools/tree](src/tools/tree.rs) | The shared tree as paths: how a place, a thread, a conversation, a chat, a document and its versions are named, how an agent's reach is laid out as directories and files, and how a path is found again. |
| [tools/tree/loading](src/tools/tree/loading.rs) | Reach-shaped candidates for an agent's tree, with ancestors kept as paths rather than content scopes. |
| [tools/documents](src/tools/documents.rs) | A document as an agent reads and changes it: its blocks written as Markdown by the one renderer, cut into the pieces that a change touches, and read back into blocks by the one reader. |
| [tools/shared](src/tools/shared.rs) | The shared tree: what is in an agent's reach as paths, from everything it may read or from its own topic: neighbouring threads by policy, the documents, chats and conversations people shared, and the files each carries; listed with what each entry is, how it is counted and whether it may be written. |
| [tools/references](src/tools/references.rs) | References in what an agent writes and reads: found in its Markdown, resolved from its own thread into typed references with their parts, told back in two lists, and written out for each agent that reads them in its own paths. |
| [tools/read](src/tools/read.rs) | Read one entry of the shared tree: a range of its parts at one of three depths, in its written form, within a bound of bytes. |
| [tools/search](src/tools/search.rs) | Search everything in an agent's reach by a word or a bounded pattern, narrowed by kind, place or name: what the database holds first, then inside files, within bounds that are said when they are reached. |
| [tools/edit](src/tools/edit.rs) | Make a document, replace one, or change part of one, by one command a call: on the state the agent read, under its name, and in the document's history like anybody's writing. |
| [tools/chat_message](src/tools/chat_message.rs) | Write in a chat that was shared with the agent for writing: under its name, with files, and as a reply to a message by its number. |
| [tools/thread_message](src/tools/thread_message.rs) | Hand text and files to the agent of another thread, one of the same topic or one shared for writing: stored and delivered as an announcement with one target. |
| [tools/workspace_files](src/tools/workspace_files.rs) | Files an agent attaches: read by its environment's machine, verified in the blob store, and limited as people's uploads are. |
| [tools/definitions](src/tools/definitions.rs) | Orch's tools as an agent is offered them: names, descriptions, closed schemas, titles and hints; a refusal as an answer with a code and what to do instead; and one rate for all of them. |
| [tools/texts](src/tools/texts.rs) | The shipped descriptions of Orch's tools, as an agent reads them. |
| [tools/goal](src/tools/goal.rs) | The `goal` tool: the agent reads the goal of its thread and reports where it stands. |
| [tools/manage](src/tools/manage.rs) | Create document folders, copy, move, delete or restore complete document trees. Only document deletion requires a complete preview followed by its confirmation token; document creation, copying, moving and restoration execute directly. Thread and chat management retain their two-call preview and confirmation. Every operation checks the entire selection and changes all of it or none. |
| [tools/download](src/tools/download.rs) | Copy an entry of the shared tree into the agent's workspace: a file as it is, anything else in its written form or as records; large ones as visible jobs. |
| [continuations](src/continuations.rs) | Authored prompt versions keep an immutable prefix and their own suffix in the same thread; browsing is separate from selecting execution. |
| [continuations/admission](src/continuations/admission.rs) | Current author, View/Interact/provider scope, actual input boundary, queue and execution compare-and-set before durable acceptance. |
| [continuations/recipe](src/continuations/recipe.rs) | Retained operation, command, source and prepared-native identities; retries cannot replace an unknown native creation with another child. |
| [continuations/projection](src/continuations/projection.rs) | Bounded visible-prompt metadata and read-only neighboring versions, including actual delivered item anchors and retained preparation status. |
| [jobs/continuation](src/jobs/continuation.rs) | Claimed quiescence, exact native preparation and one selection commit; ordinary failure recovers the recorded original conversation without restoring files. |
| [jobs/document_notices](src/jobs/document_notices.rs) | Bounded, restartable discussion notification fan-out with current reading checks. |
| [jobs/runner](src/jobs/runner.rs) | Durable phases with weights and a percentage, serialization, cancellation, undo and recovery. |
| [jobs/upload_watch](src/jobs/upload_watch.rs) | How far the content a machine puts up for a job's command has come, read once a second from the system's record of the upload and shown in the job's progress, with a checkpoint in the plan. |
| [jobs/measurement](src/jobs/measurement.rs) | Reusable background counts, retained with their observation time and cancelled when their last viewer leaves. |
| [jobs/provider_provision](src/jobs/provider_provision.rs) | A provider's managed runtime as jobs of the runner: made when the provider is saved as managed, started again when a thread needs it, a person asks, or the sign-in it holds is to be looked at, and taken away with the provider. |
| [jobs/provision](src/jobs/provision.rs) | Environment and provider runtime provisioning, with the image phase: pulled, built or loaded. |
| [jobs/after_up](src/jobs/after_up.rs) | Frozen template-first after-start commands and exactly one output-free history line per outcome. |
| [jobs/capability_change](src/jobs/capability_change.rs) | Durable live capability decisions, applied on the prepared unit before they are announced as available. |
| [kept_images](src/kept_images.rs) | Kept-image inventory and cleanup authorized by current units, archived ownership and durable active jobs. |
| [cache_cleanup](src/cache_cleanup.rs) | Server-selected unused cache cleanup with fresh node use guards and durable per-object results. |
| [storage](src/storage.rs) | Retained-storage facts, physical-byte accounting and report breakdowns shared by every cleanup surface. |
| [resource_facts](src/resource_facts.rs) | Batched current dependents, reported resource uses and permission projections shared by lists and plans. |
| [resource_facts queries](src/resource_facts.sql) | Indexed bindings, explicit giving scopes and retained attachments without inherited receiver counts. |
| [jobs/kept_images](src/jobs/kept_images.rs) | A durable answer to the node's startup/hourly kept-image inventory, retaining current and archived ownership. |
| [updates/live](src/updates/live.rs) | Accepted resource limits applied to the existing unit, with completed acknowledgments defining pending updates. |
| [jobs/recreate](src/jobs/recreate.rs) | Recreate and update, of an environment and of a provider's runtime: a successor on the same volumes, the old unit kept until the checks pass and brought back otherwise. |
| [jobs/snapshot](src/jobs/snapshot.rs) | Selected tree hashing, upload and pinning. |
| [jobs/batch](src/jobs/batch.rs) | Independent switches, moves and updates with bounded concurrency. |
| [jobs/fork](src/jobs/fork.rs) | The one job behind fork and switch: lock, provision, copy, history, verify, commit, cleanup; everything undone before the commit; history up to any turn, and handed to another agent in the conversation's written form; the agent's notes taken along as the plan says; a switch that keeps owners and page and asks only for what it changes, and a fork that is its maker's with the page of its source. |
| [jobs/environment_move](src/jobs/environment_move.rs) | An environment taken to another host with its identity kept: lock, successor, copy, verify, commit; undone before the commit. |
| [jobs/estimates](src/jobs/estimates.rs) | Measured and assumed transfer estimates. |
| [fork_plan](src/fork_plan.rs) | What a fork or a switch would do, worked out before anything does it: who will own the result, how the history travels and what of it is kept, where the files and the agent's notes go, what stands still and what is left behind, how long it takes, and everything that stops it. |
| [fork_source](src/fork_source.rs) | Exact saved file points and own-worktree choices in fork plans and jobs. |
| [fork_history](src/fork_history.rs) | The history of a thread as it travels to another agent: the turns asked for, what each method keeps, converts and drops, the neutral archive that an adapter imports, and the transcript a summary hands to an agent that is to read it. Ask Keep copies an owned cut in a separate transaction so the source can accept questions during a large copy. |
| [jobs/health](src/jobs/health.rs) | Health checks of a new side, which spend no token. |
| [jobs/mcp_restart](src/jobs/mcp_restart.rs) | One environment MCP server restarted with its own budget cleared; sessions that declare live reload refresh without touching a turn, and other sessions take the change on their next same-conversation resume. |
| [jobs/remove](src/jobs/remove.rs) | Removal of an environment or a retired unit, with a handed-over credential returned first. |
| [locks](src/locks.rs) | The lock a switch, move, recreate or update holds on a thread, an environment or a provider's threads: what it waits for, queues, cancels and refuses. |
| [updates](src/updates.rs) | What an environment or a provider's runtime has fallen behind on, what a release requires of it, and updating a stopped one when it starts where its owner asked for that. |
| [updates/observations](src/updates/observations.rs) | Reading pending runtime changes together, without planning jobs or requiring replacement authority. |
| [network](src/network.rs) | Resolve runtime egress policies and enforce the durable restriction at every shared start and release gate. |
| [reaper](src/reaper.rs) | The schedule that finds what is due for removal (environments, those unused for as long as their setting says among them, retired runtimes, a deleted provider's runtime, what a purged org has on hosts), gives notice, and retries with growing pauses. |
| [goals](src/goals.rs) | A thread's goal: set, held, edited and cleared by whoever drives the thread, kept going by Orch's own loop, and ended when its condition holds. |
| [goals/drive](src/goals/drive.rs) | The goal loop: what a turn that ended comes to for the goal, and the continuation that follows. |
| [goals/check](src/goals/check.rs) | Checking a report of "met": the helper model is asked outside any transaction, and its answer is applied under the thread's lock to the goal as it stands then. |
| [goals/text](src/goals/text.rs) | What Orch says to an agent at each turn of its goal. |
| [summaries](src/summaries.rs) | Title/description/tag triggers, manual override races, limits and usage. |
| [summaries/completion](src/summaries/completion.rs) | Summary calls through the selected API provider with the setting writer's current authority and actual usage. |
| summaries/documents | Automatic document descriptions share the configured producer, current access, pacing and usage records. |
| [summaries/handoff](src/summaries/handoff.rs) | A genuine bounded summary beside the indexed full transcript a new agent can verify. |
| [summaries/budget](src/summaries/budget.rs) | How much a call of the summary model is sent: the place's share of the model's window. |
| [summaries/estimate](src/summaries/estimate.rs) | What a summary of a conversation will send to the summary model and cost, worked out before it is confirmed. |
| [summaries/goal_check](src/summaries/goal_check.rs) | The helper model's check of an agent's report of "met". |
| [ingest](src/ingest.rs) | Idempotent application of durable machine events. |
| [raw_events](src/raw_events.rs) | The frames of a harness's own protocol, kept for diagnosis: masked of anything that must not be stored, bounded, and read back newest first. |
| [redaction](src/redaction.rs) | Secret and pattern masking across streaming boundaries. |
| [projections](src/projections.rs) | Attention, search, unread and tree projections, and the names the finder looks through. |
| [notifications](src/notifications.rs) | Recipient selection; the channels a notification may go out through and each person's routes, with what waits for an unread delay or a frequency limit; what a node's destination is sent; and transactional inbox and outbox creation. |
| [history](src/history.rs) | The line every act leaves in the history of the thing it was done to: a marker in a thread's timeline and its History view, an entry of a place's Activity, a line on a resource's page, and for an environment what was done in it: shells, files written, an agent's changes turn by turn. |
| [source](src/source.rs) | Source readiness, before-turn file recording and watcher-only end observations. |
| [source_retention](src/source_retention.rs) | Expired recorded-state metadata and temporary plans release pins while native journals keep recovery roots. |
| [jobs/source](src/jobs/source.rs) | Durable source preparation and Git publication jobs. |
| [jobs/workspace_upload](src/jobs/workspace_upload.rs) | A completed private upload written as a durable job whose cancellation joins its machine command. |
| [jobs/machine_move](src/jobs/machine_move.rs) | The directory/unit lifecycle adapters of the same move job: data ownership, machine policy, fresh generations, complete data and recorded-state transfer, verified commit and rollback. |
| [jobs/restore](src/jobs/restore.rs) | Recorded-file restore jobs retain writer holds until native apply or rollback is complete. |
| [git_work](src/git_work.rs) | Explicitly requested Git work facts, shared by threads using the same files. |
| [written](src/written.rs) | The written form of a conversation: turns as parts, a range of them, and three depths (in brief, as it was said with a line for each step, and in full with what each step was given and what came back); one renderer for what an agent reads, a download, a copy, an export, a forward and the file a switch hands on. |
| [archive](src/archive.rs) | Archive, bringing back, soft-delete and restore orchestration; the deletion of an org at any level: where its members go, and what becomes of each of its resources. |
| [membership](src/membership.rs) | Ending a membership: what the person owned alone passed to the first owner of the org it lies in, what they owned with others left to the others, and what is on their machines unregistered; a member's move to another domain; what waits while a thread's first owner is suspended or may not use its provider. |
| [membership/actions](src/membership/actions.rs) | Applying membership and home resource endings under their accepted authority, without deleting personal machine files. |
| [schedule](src/schedule.rs) | Machine-dependent expiry, idle, retention and summary schedules, the start of a stopped provider's runtime to renew its sign-in, the shells that nobody came back to, the samples of the API's, the gateway's and the hosts' metrics, and the reaper's tick. |
| [reconciliation](src/reconciliation.rs) | Recover turns, sessions, jobs, locks and leases. |
| [tools/browser_definition](src/tools/browser_definition.rs) | The browser tool's closed schema and truthful effect hints. |
| [browsers](src/browsers.rs) | Browser lifecycle, profile handouts, idle closure and history without page content. |
| [displays](src/displays.rs) | Display lifecycle checks the environment door and audits only safe committed facts. |
| [background](src/background.rs) | Volatile native command observations, caller-bound reviews and captured End targets. |
| [thread_move](src/thread_move.rs) | Moving the same idle conversation and its side paths while preserving its native work and queue. |
| [jobs/background_end](src/jobs/background_end.rs) | Durable exact native End targets with current authority and truthful partial outcomes. |
| [tools/browser](src/tools/browser.rs) | The browser tool's four commands, checked against the calling thread's own environment. |
| [tools/display](src/tools/display.rs) | The display tool opens or announces local screens and closes only the calling thread's own. |
| [retention](src/retention.rs) | Environment removal acceptance freezes every owned volume and its locked retention policy. |
| [temporary_environments](src/temporary_environments.rs) | Temporary environment warning, Keep and cleanup through the ordinary leased removal jobs. |
| [retained](src/jobs/retained.rs) | Restore and purge jobs reserve the same frozen files and deletion revision. |
| [audit_facts](src/audit_facts.rs) | A person's committed thread operations leave one factual audit row. |
| [tools/search/related](src/tools/search/related.rs) | Related search uses the shared weighted ranking and the agent's exact current tree for every delivered path. |
| [tools/search/referenced](src/tools/search/referenced.rs) | Referenced-by search lists what refers to a document, only through what the agent may read, with the lines around the mentions. |

### Tests

| Test file | What it checks |
|-----------|----------------|
| [call_recovery tests](tests/call_recovery.rs) | Synthetic failed turns prove safe repetition, visible authored continuations, configured failover and durable loop stops. |
| [call_recovery/auxiliary tests](tests/call_recovery/auxiliary.rs) | Setting-writer accounting, caller revocation and auxiliary compatibility through the same recovery policies. |
| [forks/call_recovery tests](tests/forks/call_recovery.rs) | A recovery uses the ordinary verified switch while carrying the failed turn's retained partial work. |
| [shared_resolution](tests/shared_resolution.rs) | Real tool calls resolve paths faithfully, refuse ambiguous writes and keep same-named hidden content private. |
| [trace_writers](tests/trace_writers.rs) | Real database tests of committed approval, Stop, cancellation, provisioning and frozen agent responsibility. |
| [trace_retention](tests/trace_retention.rs) | Real database and job tests of frozen windows, stopped restoration, expiry reservations and failed purge promises. |
| [data_directories](tests/data_directories.rs) | A borrowed data root survives recreate and a move exposes its explicit-copy refusal. |
| [related_shared](tests/related_shared.rs) | Related search admits only the agent's current shares and preserves the shared path grammar. |
| [referenced_shared](tests/referenced_shared.rs) | Referenced-by search lists only what the agent may read now, with the lines around the mentions, and leaves no trace of the rest. |
| [document_summaries](tests/document_summaries.rs) | Automatic document descriptions follow the selected producer, preserve writers and share pacing and usage. |
| [custody_scale tests](tests/custody_scale.rs) | Resource counts stay bounded by current threads over million-row queues and retained histories. |
| [goals](tests/goals.rs) | Goal mode: Orch keeps a thread working, turn after turn, in the name of the person who set the goal, until the agent's report that it is met is taken, a limit that is set is reached, or a person stops it; and it waits for whatever asks a person. |

### Named in code

| Name | What it does |
|------|--------------|
| `tools::search::indexed` | The indexed root-search window and query vector, prepared concurrently before the agent's selected reach is hydrated. |
| `jobs/cleanup.rs` | Confirmed storage cleanup, current authority and durable item outcomes |
| `resource_pause.rs` | Immediate accepted interrupts and durable unit stops, keeping paused data |
| `resource_facts/activity.rs` | Exact live turn, queue and stopped-turn dependencies with the shared privacy projection |
| `resource_facts/library.rs` | Durable reported invocations and creation memberships join the shared resource facts reader. |
| `resource_facts/library_lifetime.sql` | Indexed lifetime figures for inventory ordering without materializing dependency names or people. |
| `resource_facts/custody.rs` | Host departure reads its current dependents and retained data once from their existing records. |
| `jobs/host_delete/custody.rs` | Host erasure preserves every Leave row and recovery promise while removing exact owned objects. |
