# orch-api-contract

HTTP request and response contracts, with route metadata.

`resource_facts` defines reported-use units, history coverage, current dependents and their privacy
projection shared by resource lists, storage and destructive plans. Physical artifacts have
dependencies and an exact cleanup fence; they do not have popularity figures.

Org summaries distinguish domain `member_count` from optional `people_count`, the native deduplicated
active people in the branch and its inherited/group roles. Missing older counts remain absent rather
than becoming a false zero. Normal OpenAPI and TypeScript generation carries this same projection.

Implemented. Every request of the API is declared here once, with who may
make it and the types of its query, body and reply, as they are on the
wire; the server binds handlers only to these declarations, and the
OpenAPI document and the browser's types are generated from them. Unit
tests round-trip the intricate shapes against JSON written by hand. The export of a place
is declared in `exports`: the request, the plan that says what an archive would hold and what stands in its way,
the archive and the job that writes it, and the chat's single export; its refusals are three error codes.
Node views include icon colours, tree summaries count following documents, and
machine enrollment can retain an existing identity. Personal state accepts the
sidebar, and child lists can select a kind.

`TreeThread` optionally carries `waiting` (approval or answer, with this reader's current decision
authority) and `blocked_cause` (a classified sandbox, approval-mode or sign-in refusal). The existing
authorized HTTP/live tree shares these compact observations; no request content or raw diagnostic
text is added to it. An absent personal flag never gives a viewer “your approval” wording. These
members are generated with the other contracts by the integrator, never edited into browser types.

Finder results include a person's username and avatar, the settings registry carries its group and texts, and tree rows carry an org's full address. The generated document and browser types expose the same members.

The browser's `GET /me/bootstrap` returns `MeBootstrapReply`: person, interface state, ordinary
organizations, the session's admin mode, visible favorites, personal chat states and notification
summary. Its members reuse the individual read contracts; trees and chat contents are opened separately.

| Module | Planned responsibility |
| [calls](src/calls.rs) | Model-call recovery state and the controls available to a thread's readers and drivers. |
|--------|------------------------|
| [common](src/common.rs) | Pagination, errors, identifiers and HTTP conventions. |
| [messages](src/messages.rs) | Public error and native failure sentences from the shared interface catalogue and alias table; both languages resolve every declared code. |
| [native](src/native.rs) | Native session command observations and installed browser facts shared by adapter and link payloads. |
| [machine_programs](src/machine_programs.rs) | Agent/provider program registration, its real declaration and policy authority, and observed instance installation commands. |
| [auth](src/auth.rs) | Setup, registration, signing in by a password, by a code sent by mail and through an identity provider, the second step and what a sign-in still lacks, the change of an address, and recovery. |
| [users](src/users.rs) | Users with their state; invitations as a link to share or a personal link for one address, within what a root org allows of each; and what an invitation shows before it is accepted. |
| [nodes](src/nodes.rs) | Hierarchy with orgs inside orgs as deep as the system allows, a person's own org and the two limits of a root org, tags, archive, node views, a place's Activity, and a project or a department moved inside its tree with what that changes. |
| [settings](src/settings.rs) | Settings, secrets metadata and parameter views. |
| [access](src/access.rs) | Roles held in a place, permission pages with their owners and defaults pages with their previews, what a page's reader may give and what a place's page covers, a holder as a name and a count with its people on request, the change of a page that names owners and holders at a choice, who may name somebody from outside, what a caller lacks for an action and whether they may force it, requests for all of it at once, effective access views, and the lines of a thing's history. |
| [ownership](src/ownership.rs) | What ending a membership touches and where each thing goes, a member's domain and the move that changes it with what they lose, gain and keep, the deletion of an org that lies in another, and what was deleted and can be restored. |
| [threads](src/threads.rs) | Thread sync; items with the markers that are the thread's history; each step of the agent's work as a display for its kind beside the call as its harness made it; inputs and turns with model and effort, the phase a turn is in and the limits each was let past; segments; side threads; a thread's directory and where its agent's notes are kept; its lock and input access; and exports at three depths. |
| [binding](src/binding.rs) | Binding options (provider and the agents it offers, model and effort, existing and new environments) and arrangement views with their type, executor, the tool set that the environment's offers come to, whether Orch's own tools are offered, and isolation facts. |
| [forks](src/forks.rs) | Fork, switch and move plans: history up to any turn, files, the agent's notes, fidelity, estimates, options, who owns the result and who of the people on the page may go on, what is locked and what is left behind; the update plan of a provider's runtime. |
| [topics](src/topics.rs) | Topic sync, announcements with their category and whom they went to, and the environments a topic's threads use. |
| [workspace](src/workspace.rs) | Files with rename, copy and the search inside them, what an archive holds, uploads in chunks that go on where they stopped, snapshots with their archives, and cached views; the text a new thread's `AGENTS.md` is to hold; jobs with their phases, percentage and what a failed one undid. |
| [source](src/source.rs) | Source preparation, worktrees, recorded file restoration and Git publication. |
| [environment](src/environment.rs) | Environments as resources: owners, home and address, what the caller holds, who reaches into it, configuration that is changed while it runs, what it declares it needs, what its exec node offers, the MCP servers found there and those of its own, the threads in it, its history, pending updates, removal and its reason, places with their warnings, storages, fleet and recycle. |
| [terminals](src/terminals.rs) | Shells that are their opener's alone: sessions attached or left running, the ticket that takes one to another screen, the choice of which to close at a person's limit, and the diagnostic terminal. |
| [processes](src/processes.rs) | Process and resource views. |
| [ports](src/ports.rs) | Environment port targets, wider openings, admitted uses and live caller facts. |
| [approvals](src/approvals.rs) | Harness, capability and access requests and decisions. |
| [resources](src/resources.rs) | Providers with their setup (sign-in methods, options, endpoints, presets), the arrangement types their owner allows and their runtime with its pending updates; models with the name that is sent and the name that is shown, and the speeds and access programs each offers and has enabled; profiles; hosts with what they do when full; machines with their policy and who changes each key; templates of both kinds with their sources, their imports and how an exec node is launched; and MCP servers: presets, and those imported from a harness's own configuration. |
| [trust](src/trust.rs) | A workspace's own harness configuration: whether a harness may use the `.codex` or `.claude` its environment's workspace holds. |
| [instructions](src/instructions.rs) | What a thread's agent reads as its instructions, source by source (Orch's introduction, the guide to its tools, each place's standing instructions, the files its harness reads), as the last session was opened with them and as the reader may see them; and the standing instructions as they stand now. |
| [context](src/context.rs) | What a turn's tokens are made of and what fills a thread's context: each harness's report carried whole, and Orch's own estimate by part where a harness reports none. |
| [model_inputs](src/model_inputs.rs) | What Orch supplied to a model, as it was sent: the text a session was opened with and each input's text, block by block, recorded before it was sent and marked once its sending ended. |
| [mounts](src/mounts.rs) | Mounts, their locations, and how each is attached to an environment: read-only, shared or as a private view. |
| [skills](src/skills.rs) | Skills: versions with their manifest and origin, the sources a version comes from, drafts, and the skills a thread is given with those left out and why. |
| [notifications](src/notifications.rs) | The Inbox and its preferences; the channels out of the application that the system, a root org and a person set up, with their texts; a person's routes through them, each off until switched on, with how long a notification stays unread first and how often a message goes; push subscriptions; and the destinations of a node. |
| [admin](src/admin.rs) | Management views: users and their state with what a new person's own org starts as and which proofs are closed at their name, a reset link that may switch a second step off, root orgs and their two limits, both pools of runtimes, health, stored metric series and PKI. |
| [downloads](src/downloads.rs) | Connector downloads and release manifest views. |
| [search](src/search.rs) | Transcript and node search; and the finder's names: places, people, resources, documents and threads from a few letters. |
| [links](src/links.rs) | A document's Links section: what refers to it, what it refers to, the graph around it, and which tabs it shows. |
| [everything](src/everything.rs) | The reader's complete hierarchy, read a level at a time, or searched by name with its authorized paths. |
| [usage](src/usage.rs) | The usage ledger read at three depths (one's own, a place's totals, a place's reports): figures, series over a span split by a dimension, and records, in tokens and credits; the rate card and the catalogue of estimates behind it, with which of them a figure came from; caps on tokens, on credits and on turns running at once; and the limits in a turn's way, with who may let it past each. |
| [performance](src/performance.rs) | One arithmetic rule for neutral model-request boundaries, hidden reasoning and streaming eligibility. |
| [chats](src/chats.rs) | Chats, messages, states, attachments and forwarding. |
| [documents](src/documents.rs) | Documents: the twenty block types and their inline content, the operations that change them, comments, versions cut by the clock with what each changed and who wrote it, who last changed each block, files, limits, Markdown in, and Markdown, HTML and Typst out with what an export is asked to hold. |
| [document_tree](src/document_tree.rs) | Typed document-folder navigation and atomic document-tree organization. |
| [share](src/share.rs) | Sharing with agents: copies, live shares by area to read or to write (for documents to edit, which on a place also makes them), of a thing, of a place with what is in it or of a place by itself, taking back, and an agent's reach as a tree whose entries say what they are and how they are counted. |
| [exports](src/exports.rs) | Exports: a chat taken away as a file, and one archive of what a person may read in a place, with the plan that says beforehand what it would hold and what stands in its way, the job that writes it, and the archive fetched until it is removed. |
| [personal](src/personal.rs) | Favorites, recents, read state and drafts; and what the interface keeps for a person: named arrangements, hints closed and tutorials finished, and the viewer chosen for a kind of file. |
| [first_steps](src/first_steps.rs) | Observed first steps in a readable org and the caller's portable setup choices. |
| [directory](src/directory.rs) | Org groups, members with their domain, their roles, where each is held and suspensions, service identities and labels. |
| [storage](src/storage.rs) | One retained-storage report and reviewed cleanup plans across resources and machines. |
| [resource_facts](src/resource_facts.rs) | Shared current dependents and reported use of resources, with explicit history coverage. |
| [routes](src/routes.rs) | Route metadata for OpenAPI and permission declarations. |
| [openapi](src/openapi.rs) | Pure OpenAPI rendering from routes, with caller-supplied additional wire schemas; shared by the API server and contract tooling. |
| [background](src/background.rs) | Current thread command rosters, reviewed exact End requests and factual outcomes. |
| [thread_move](src/thread_move.rs) | Reviewed reparenting of a main conversation while its owner, native history and workspace stay. |
| [browsers](src/browsers.rs) | Browsers of an environment, their named profiles, and tickets to watch or drive them. |
| [displays](src/displays.rs) | Displays of an environment and single-use tickets to watch or work in them. |
| [input_schedules](src/input_schedules.rs) | Private authored input schedules, civil timing and visible occurrence outcomes. |
| [loss](src/loss.rs) | Explicit permanent loss and independent recovery plans for environments and their machines. |

| [library](src/library.rs) | Authorized library facts for resource pages and immutable skill versions. |

| [program_admin](src/program_admin.rs) | Paged administration of agent programs, their immutable builds and the capabilities proved for them. |
