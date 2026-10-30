# Doubles validation

- `performance.ts`: Literal measured request fixtures and merged percentile/error answers for the fake Usage routes; preserves missing times and cancellations.
- `request-row.ts`: Stateless request metadata for log/pause/navigation regressions.
- `request-feeds.ts`: Native machine/job/fork/health snapshots and changed rows read through each recipient's existing authorized fake readers, including raw GET handlers; opt-in read-only MCP/mount rows for complete route budget coverage. Job progress is produced by its simulated worker, without browser polling.

- `first-steps.ts`: First-steps facts read from the one fake world's providers, work, members and ordinary documents. */
- `everything.ts`: Visible inventory branches, safe shared paths, filters, sort and cursor pages for members and administrators; `/__double/everything/rows` supplies deliberate inventory scenarios.
- `announcements.ts`: One place-level GET/POST path, using the existing access, paths, privacy and settings rules for orgs, projects and topics.

The fake API: a small server that answers the requests the screens make from memory, in the shapes of
the generated types, and plays the live connection for a tree and for a thread. The end-to-end tests
steer it through requests under `/__double` that no real system has (put it into a scenario, read what
it "mailed", change a tree, lose a frame, drop every connection, make a machine enroll, or enroll again, play a whole
conversation by name). `node tests/doubles/server.ts` runs it by itself.

It also holds people's chats: the chats of the places of the tree, direct chats and groups with
guests, messages with replies, edits, reactions and pins, each person's state and draft, forwarding
that is all or nothing, and the live feed of each chat with who is looking and who is typing. The
tests write into it as another person would, through `/__double/chats/...` (say something, type,
make a chat, switch a place's chat off, put an item into a thread for a message to quote, take away a
person's right to read threads: the denial of permissions that the places keep).

It holds the permission pages too: the catalogue of permissions (the real one, doors and Export rows included), a page
for every thing and the defaults of every place with the locks they ship with, a stale revision answered with the page as
it now is, what each person may give on each page (`gives`, `can_give`, the policy that keeps sharing to owners), the
head-count of each role, what a place covers, who reaches into an environment, sharing with agents (copies and shares
that stay, with their limits) and asking for access with its approvals. Previews and requests are served; a test can
switch either off. Its controls are under `/__double/access`.

And documents: blocks, batches of operations with their conflicts, versions, discussions and export, and the live
feed of a document with who has it open and who is writing. Its controls are under `/__double/documents`.

It plays the settings as well: the person's own pages (the second step, tokens, sessions), the members
of an org with moving, holding out and ending, the registry of keys with the value in force at a place,
where it comes from and what a lock above does, the secrets of a place that are written and never
given back, channels and destinations with the answer of a far side that fails, service accounts and
the audit trail. The tags of a place are kept by the places and the groups of an org by the permission pages,
which name them: the settings screens ask them there.

A model's entry at its provider is played with what a source offers, what its managers opened and what a thread starts with,
and the thread's parameters follow it: a speed, an access program and a context exist for a thread only where the entry opened them
(and, for a program, where the place switched its feature on), and a thread's own choices are the object `agent.params`. The
end-to-end tests write an entry anew and switch the place's feature through `/__double/model/*`, and set a thread's choices and a
lock above them through `/__double/thread/set`.

It is no test of anything by itself. Add behavior to it as features need it; do not assert file
layout, import choices, component shape or other implementation details.

## One world

- `trace.ts`: The permanent scoped trail and lifecycle facts of the shared fake world. */

There is one world, not one fake per screen: a request has one answer whichever screen asks it, and a
test stands on the same people, places, providers, agent profiles, models, environments, machines and
threads as every other test. Each kind of thing is kept by one class that the `World` holds beside the
others, and the requests of that kind are answered there and nowhere else.

| File | Keeps | Answers |
|------|-------|---------|
| `bootstrap.ts`, `screen-openings.ts` | no independent state; reuse native readers | shell Meta, admitted chat scopes/tail/private draft, independently authorized first document pages |
| `world.ts` | the people and how they sign in, sessions, the orgs and the tree of each, invitations | signing in and out, the account, invitations, an org's tree and its live feed |
| `places.ts` | what a node is besides its row (owner, slug, description, tags, roles), members, Activity | nodes, topics, tags, members and invitations; folders (a project made in a project, to eight deep) and the moves of a project, a folder and a topic with their plans |
| `resources.ts` | machines, providers with their models, agent profiles, environments, hosts, templates, and what each thread runs on (`bindings`) | everything the pages of resources ask, and the lists the dialog that starts a thread chooses among |
| `threads.ts`, `thread-rows.ts`, `thread-scenes.ts` | the rows of each thread (items, turns, inputs, segments), drafts, approvals | threads and what is done in them, the thread the dialog makes, the feed of a thread |
| `context.ts` | nothing of its own: it reads the turns `threads.ts` keeps | what fills a thread's context, what each turn used, and what a turn was counted at |
| `chats.ts`, `chat-rows.ts` | the chat of every place, direct chats and groups with their guests, messages, each person's state and draft | chats and forwarding, the feed of a chat with who is looking and who is typing |
| `people.ts`, `people-import.ts` | current readers and typed audiences, read fences, policy/hint projections, personal reactions and independent imported/copied writing | the served people additions, delegated to the same chat, thread-input, document and access producers; preview prepares without accepting |
| `access.ts`, `catalogue.ts` | the permission page of every thing (nodes, documents, chats, resources) and the defaults of every place, the permission catalogue, who holds what | the pages and their changes, People and Defaults, effective access, history, the people behind a chip |
| `documents.ts` | the pages of blocks written beside a node, their files, versions and discussions, what a seeded one is said to weigh for an archive, who has each open and who is writing in it | the documents' requests, their feed; `forSearch` for the finder and the search, `readable` and `rendered` for the export, which writes the file |
| `settings.ts` | what a person sets for themself (the second step, sessions, tokens, what the interface keeps), the keys of the registry set at each place and the secrets, channels and destinations, service accounts, who is held out of an org, the audit trail | those requests, the settings of every node but a thread's, the members of an org on top of the rows the places make; the secrets that were written, for a test (`/__double/settings/secrets`) |
| `shares.ts` | what was shared with agents: live shares as entries on the pages, copies with their size, the limit, the policy that lets agents read their neighbours | sharing with agents, what was shared of a thing and what a place's agents know, what a thread's agent may read |
| `requests.ts` | requests for access and the approvals they make (every approval of kind access, also one a test put in with who may decide it), how many may wait | asking for access, the card of a request, its decision with fewer permissions than asked, taking it back; what it changes is announced to everybody connected |
| `user-feed.ts` | who follows each person's own feed, and the number of the last row of the Inbox each was sent | the only subscription to the `user` feed: the rows of the Inbox with its counters (a frame can be made to get lost) and the events every part tells a person (`world.users.tell`, `tellAll`) |
| `notifications.ts` | each person's Inbox (rows with a number that only grows, a read mark that restamps the row, counters over them), the capability approvals, what a person chose about being told, the route to each channel, the webhooks people made for themselves and push in a browser | the Inbox, the list of everything that waits for a person (the harness approvals are the thread double's and the access ones the access double's, each decided where it is kept), the channels a person can route to (the system's and the orgs' are the settings double's); its controls are under `/__double/notifications` (`row`, `many`, `state`, `approval/*`, `route/linked`, `channels`, `me`, `mail/verified`, `push/state`) |
| `export.ts` | the archives of each person, the jobs that write them, the plan of a place against its limits, the threads of a topic with what the caller holds on each, and who may read a document and not take it away | the seven requests of exports, a thread, a chat and a document as one file (with what each was asked with kept for a test to read; what the document says is the documents double's) and a topic's state; its controls are under `/__double/export` (limits, a held or failing job, progress, an Export row taken from somebody, archives of every state) |
| `search.ts` | the places each person went to lately, the names a finder may list (read from the tree, the people, the chats and the lists of resources, so that only what a caller may see is named) and the documents (read from the documents double) | the names, the search of what was written (the items of threads and the messages of chats are read from their worlds), and the recents |
| `workspace.ts`, `terminal-socket.ts` | the files of each environment in memory with a hash that names each content, what the threads' tool calls changed, the one repository at the root of the workspace, the processes and the load of the environment, and each person's shells | the workspace of an environment as the real API serves it: listing (live, or from the last list when the environment is away), reading in ranges (a range of a file with nothing in it is refused, as the real server refuses it), writing on the version that was read (a stale write is refused with the file as it stands), making, renaming, copying, deleting, search and an archive listed, Git (the whole difference of the repository, any other path refused), signals to a process, the outline of a thread (its parameters are the thread double's: its model and effort, the provider's models, and what its agent declares), the live feeds of directories, processes and measurements (switchable off: the real server has none yet) and the sockets of the shells (a ticket, a tiny shell that runs `echo`, `pwd`, `colors` and `exit`, a second screen taking the shell, a shell closed for being left); its controls are under `/__double/workspace` (put files in, change them as an agent would, take the environment away, a directory of twenty thousand files, a file of twenty thousand lines, print into a shell) |
| `starts.ts` | every request that changed something, with its body, in order | what a thread of a topic or of a project could be bound to before it exists (`GET /nodes/{id}/binding/options`, from the lists of `resources.ts` and the defaults of `settings.ts`, judged for one environment), a topic's default environment, and `/__double/start` (write a default into the settings, say what was asked) |

`fixtures.ts` has the seeds every file starts from and the identifiers of what is seeded, so that a
test can name them: `ACME` for the places of the team's tree, `RUNS_ON` for the machine, the providers,
the agent profiles and the environments, `USUAL_MODEL` for the model the team's threads run on.

### The team the world starts as

People: `ada` (administers Acme and owns most of what is below), `grace`, `dora` (a developer of
Acme), `vera` (may only read), `linus`, `margaret`, `bolted` (locked) and the others of `world.ts`.
Providers: `OpenAI key` (a key on Ada's laptop, four models: one that takes efforts, one that takes
none, one that is switched off), `Codex subscription`, `Team Anthropic` (Grace's, which Ada and Dora
may use), `Fresh key` (needs a model) and `Claude subscription` (its limit is reached). Environments:
`Workspace` (on Ada's laptop; most threads work there), `Docs checkout` (its machine is offline) and
`Sandbox` (made from a template on `host-1`, stopped). Each thread of the tree runs on something
(`resources.bindingOf`); the thread that is held back by a limit runs on the provider whose limit it
is, so the thread, the page of its environment and the page of its provider tell one story.

Shop is a project of Acme that has folders: Checkout in it, Payments in that, a topic in each and a
thread in the inner topic (`ACME.shop`, `.checkout`, `.payments`, `.cartRewrite`, `.cardFees`,
`.feeTable`). Moving something into Payments has a plan with people, a role change and a setting
decided by a lock at Checkout; a move to anywhere else changes nothing. They are a project of their
own, and not in Web, because a screen that is read by its coordinates (the board of Web is dragged by
them) would otherwise find a card of a folder above its columns.

### Adding to the world without a second one

1. A new kind of thing gets one class in a file of its own, held by `World` as a field beside `places`,
   `resources` and `threads`, made again by `World.reset` so that each test begins from the same state.
   It is one line of `World.parts`, with the name its controls go by, and the server offers it, in the
   order of that list, every request (`handle(person, method, path, query, body)`, which gives an
   answer or `null`), every control (`control(path, body)`, for what only a test can do: say something
   as another person, make a machine enroll; it is asked as `/__double/<name>/...`) and every frame
   of the live connection (`frame(subscriber, person, frame)`, which says whether the feed is its
   own; `gone(subscriber)` when a connection ends). A part has only the members it uses. What a
   person is to be told wherever they look goes to `world.users.tell(username, data)`.
2. What two parts both speak of is kept once, in the file of the part whose screens own it, and the
   other reads it from there (`world.resources.bindingOf`, `.modelsOf`, `.refsOf`): it never keeps a
   list of its own. A list a part needs that nobody has yet is added to the file that owns the kind,
   with the seeds in `resources.ts` or `fixtures.ts`, not in the file of the screen that asked.
3. A list answers as the real server answers it: the same filters (`scope`, `holding`), the same
   refusals with what is missing and whom to ask, the same revisions. A screen written for the fake
   and not for the real thing is the first thing the real thing breaks.
4. A new seed that a test must name gets its identifier in `fixtures.ts`; the test takes it from there
   and never writes the identifier or the name of something seeded twice. When a part needs more of
   the world than there is (a provider with a model that takes no effort, a thread that is blocked),
   it is added to the seeds, and the tests of the other parts are read once for what they assumed.
5. A test's expectation is of this world. A test written against a smaller one of its own is changed
   to the names above and keeps the rule it states.

- `ports.ts`: Ports component double: current observed listeners, guarded safe openings and deliberately transient secrets. */
- `browsers.ts`: The browsers and profiles of each environment, and a socket that serves real JPEGs made there and notes what reached the page (or was dropped on a ticket to watch); controls under `/__double/browsers` (`received`, `dialog`, `chooser`, `quality`, `open-tab`, `end`, `change`, `place`, `list`).
- `displays.ts`: The displays of each environment, and a socket that speaks RFB 3.8 (no password, raw pixels) to the real viewer and notes what acted on the display; controls under `/__double/displays` (`received`, `change`, `cut`, `end`, `place`, `list`).

- `ports-wire.ts`: Exact Ports wire projections for the one component-double world; no carrier or native socket success is claimed. */
- `craft.ts`: Additional readable document permission pages for journeys through the shared access controls. */
- `admin.ts`: The system's own screens: its state and measures, root orgs, sign-in, hosts, runtimes, capabilities, harnesses, storage and authority.
- `change.ts`: What is asked to be changed (a fork, a switch, several switches, an environment's update and move, a recycle), the jobs that carry it out and the locks they hold, and the side threads of a thread.

It plays the system's own screens as well (`admin.ts`), for a system admin and refused to anybody else with the
same refusal: how the system stands and the series it draws charts from, the root orgs with their limits, the
settings and the identity providers, the hosts, runtimes, capabilities, harness builds, agent profiles and
plugins, the storage and the authority; the people and the trail of the system are the trace's. A test makes a
part stop answering, or says whether the server has stored anything about itself, through
`/__double/admin/state`.
- `day100.ts`: Ordered setting-part resolution and fixtures using the settings and places of the one fake world; controlled under `/__double/day100/setting`.
- `day100-settings.ts`: Pure ordered merging and higher-lock flags for the shared settings store.

- `usage.ts`: one ledger for scoped figures, records, caps, rates and prompts; CSV uses the modern raw Accept path.
- `away.ts`: extends the same notifications, routes and preferences for device hashes, retained outcomes, grouped reads and Undo.
