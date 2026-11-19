# Web application

The lint rule in [build/utility-classes.ts](build/utility-classes.ts) reads the application's
actual stylesheet with its installed Tailwind design system. A static utility candidate that
produces no CSS fails lint; semantic marker classes remain allowed. The explicit spacing scale
in `src/styles/tokens.css` also sizes controls, widths and list heights. Layout gaps and page/list/form
insets use 4/8/12/16/24 px. Half steps remain for borders, data geometry and first-line icon centring;
compound insets reserve actual icons, actions or editor handles rather than adding spacing steps.
The unit proof source checks both present and absent theme steps.

Visual chrome keeps sentence-case, normal-weight section labels, 16 px row/button icons and 20 px
rail icons with the application's one 1.75 stroke. Avatars and chart marks retain their own geometry.
Native text fields have the shared Input's 12 px inner margin; `input-leading-icon` reserves the
16 px glyph plus an 8 px text gap, and `input-trailing-action` clears the small action at the end.
Radii follow hierarchy: 4 px inline marks, 6 px controls/rows, 8 px contained surfaces and 12 px dialogs;
circles and status pills keep their semantic shape. Shared `ui/` controls own their corresponding joins.

[build/class-inputs.ts](build/class-inputs.ts) follows class attributes, shared class props,
class helpers and DOM class writes through their local definitions. Conditions, labels, data
attributes and ordinary metadata stay outside that scan. Only complete known utility shapes
are checked; selectors in `src/styles/` and named group/peer markers are accepted. Parser-level
regression sources cover these boundaries, local lookups and the actual missing theme steps.

The environment page and the thread's Environment panel share [McpServers](src/features/resources/environments/McpServers.tsx): current server states, masked failure output, and a manager's one-server Restart job. The selected server's failure budget resets; agent tools refresh live where supported and otherwise at their next turn.

| Shared presentation | Used by |
|---------------------|---------|
| [SandboxReadiness](src/features/resources/SandboxReadiness.tsx) | Machine readiness before a thread exists, blocked-thread explanations, and policy acts authorized by the same server comparison. |
| [CallsRules](src/features/settings/CallsRules.tsx) | Inherited recovery rule rows, ordered steps and explicit candidates in place settings. |
| [CallRecovery](src/features/thread/CallRecovery.tsx) | One recovery sentence for the live line, attention banner and handoff job progress. |

The foundation of the web application is built: the toolchain, the design tokens with the shared
controls drawn from them, the one client that speaks to the API, the catalogues of both languages,
the screens a person meets before they are signed in (first administrator, sign-in by each proof,
registration, invitation, recovery, a new password, confirming an address), the workbench that
stands round every other screen (rail, bars, groups of tabs, status line, the tree of an org kept
current by its live feed), and the live connection itself. On top of it the places of an org are
built: the left bar finished (menus on every row by what the caller may do there, Starred, Recent,
archived places on request, the switcher with the orgs inside orgs), the org home at every level,
the project as a list or a board of its topics, the topic with its overview and tabs, and the
members of an org with both forms of invitation. And on top of those the screen of a thread: its
conversation (every kind of item, kept live from the socket, with growing text drawn as it arrives),
its header, its box with the queue, steering and Stop, the requests of a harness decided in place, the
banners of a lost line and of what blocks a thread, and the dialog that makes a thread in a topic.
Documents are built too: a page of blocks written in place by several people at once, with comments,
history, moving, export, and the list beside a place (`src/features/documents/`). And the settings: a
person's own pages (profile, security with the second step, API tokens, what the interface keeps), an
org's at every level, a project's and a topic's, with the editor of every key of the registry and the
channels and destinations that send what happens out of the application.
Beside them is what concerns the person themselves: their Inbox with its counters on the rail and the
left bar, the approvals that wait for them, and how they want to be told.
Every other screen has its route from the start and says, inside the workbench, what it will show and
that it is not built yet; the feature directories below hold their modules as documentation-only
placeholders until then.

What the places do, and what they leave to others:

- Making an org inside an org, a project and a topic; renaming; describing; archiving and bringing
  back; deleting, with "Recently deleted" for the days it can be brought back; moving a project or an
  org with what changes said first; starring. Every act is shown as pending until the server has
  answered, except a person's own marks (star, bell), which are shown at once and put back where the
  server refuses.
- Controls are on or off by what the API says the caller holds on the place (`access`), never by a
  role's name. A row of the left bar offers only what the caller may do; a screen keeps the act
  switched off and says what it takes and whom to ask; a refusal from the server lists what is
  missing as the server gives it.
- A project's topics as a list and as a board (needs attention, running, idle, done), narrowed by
  status, tag, attention and a search that the address keeps; a card moves into or out of Done by
  drag or by its menu.
- A topic shows its threads as the live tree knows them, the environments it names and the documents
  written beside it; its Settings tab is the settings editor of the topic and where it sends what happens
  (`src/features/settings/`), and its Files tab says that it is not built yet, because its data is another part's. The act that starts a thread opens the dialog described below.
- Folders: a project may lie in a project, which the screens call a folder (a folder glyph; a project
  has a glyph of its own). The left bar nests them to any depth; a row that is closed carries the
  approvals that wait in what it hides. A project's screen, and a folder's, which is the same screen,
  stands its folders above its topics as cards, with "New folder" beside "New topic" for whoever opens
  topics there; its Activity takes in its folders and says so. A project, a folder and a topic are
  moved by one dialog that lists the places of the tree that can hold what is moved, reads the server's
  plan (who gains and loses what, which settings resolve differently, what stops it, the depth of
  folders among it) before anything moves, and the tree follows. A topic names the folders it lies in.
  The tree does not drag, so a move is by menu alone, and it draws no unread marks: a row of the tree
  carries no read position.
- Members of an org: the list by name and a page at a time, the invitations still good, an
  invitation as a link for one address or as a link to share (cut to the org's own rules), and a
  role changed.
- What the system does not answer yet is shown as absent rather than dead: the finder's names (the
  left bar's Recent falls back to the places this device opened where they are not answered), and access
  requests, which the system does not take yet. The left bar and the screens follow the live tree, and every act
  of the person's own asks for what changed once more, so that it shows at once whether or not the
  live connection is up.

What the thread screen does, and what it leaves to others:

- The conversation is a list that draws only the rows in view, so a thread of five thousand items
  opens at its end and scrolls as one of ten does (`tests/performance` measures it). The agent's work
  is shown step by step in the turn that runs and the last one that is over, and folded to one line in
  the others, or all one way, as the reader chooses (Steps, Summary, Everything). A command shows its
  output as it prints, a change its difference with the changed words marked, a request for approval
  its options; a "New" line stands before what the reader had not seen when they opened the thread;
  a reader who is not at the end is told how many rows came below.
- One live feed per thread, loaded by sequence: a frame that does not follow the last is not
  believed but made up for from the server's own answer, a row is replaced only by a newer one, so
  nothing is shown twice or missing after a gap or a reconnect; growing text is joined piece by piece
  (text in code points, output in bytes) and a piece that begins beyond what is held asks for the item
  again. The header shows who is looking, and says when the line is lost or what is shown may be old.
- A message is sent with the value that makes a repeat safe; while a turn runs it queues, steers or
  interrupts, and the queue can be edited, put in order and taken back; Stop ends the turn. The box
  keeps a draft per thread on the server. Attaching files is shown as not available: the server takes
  no uploads yet.
- Who may only watch sees the conversation, no box, and what they would need to work with the thread.
  A thread that is blocked says why in its header and above the box.
- The dialog that makes a thread is built from the lists the API answers (the providers a person may run
  work on and why the others cannot be, the agent profiles, the models, the environments they manage
  and the machines they connected): the server answers a thread's binding options only for a thread
  that exists. A profile whose harness has not reported what it is is listed and not chosen.
- The model control at the box (and in the header) shows what the thread runs with, which is its own
  state and never what the provider allows: the model and its effort, and beside them Speed, Access and
  Context where the model's managers switched them on at the provider, each opening on what the thread
  has now and saying where that comes from, when it takes effect and, for a context, the sizes open, a
  field for another number, the adapter's words on where the conversation is shortened and the steps of the
  model's rate. A change of model drops what the new one lacks and says so; somebody who only drives the
  thread reads it and is told who can change it. `/model`, `/effort`, `/speed`, `/access` and `/context`
  in the box open it or make the choice, and the footer of a turn says what the turn ran with.
- An item's menu brings it to the thread's chat as a quote, which stands above the chat's box until the
  person sends it, or begins picking items: each item then has a box, `Shift` takes everything between
  two, and a bar in place of the message box forwards what is picked to the thread's chat, quotes it
  into the box written to the agent, discusses it in chat or copies its words. An item that messages
  of the thread's chat quote carries their number, which brings the chat to the front.
- Left to later parts: the lanes of sub-agents, side threads and forks (and forking from the last
  item picked), the History view, the inspector panels, files and code, export, finding in the
  conversation, the other forms of copying a selection, and the page of approvals.

What resources do, and what they leave to others (machines, providers, agent profiles, environments,
templates and hosts, under `/resources`):

- Each kind has a list that narrows by what is typed and by its own filters, both kept in the
  address, and a page with its tabs. What a person may do on a page is read from what the API says
  they hold (`access`); a refusal lists what is missing and whom to ask. Making something asks the
  server, which decides: a person who may not make one is told what is missing after the try.
- Machines: Add machine gives the single-use command for each system (the browser's own first), with
  the switch that hands the limits to Orch, and waits for the machine, saying when it has enrolled. A
  machine's page shows its facts, what is registered from it (an agent profile for a harness found
  there, a provider whose key is typed there, an environment as a directory, a refused directory
  shown under its field) and its local policy as enforced there: changed from here only for the keys
  its owner handed over, otherwise with the command to run on the machine.
- Providers: by Mine, I can use, I manage and, for an admin, all of the org; a page with Overview,
  Runtime (read only), Key (or Sign in), Models, How agents run, Threads, Settings, Access and Danger
  zone as the server serves them; Add provider on an API key in three steps.
- A provider's Models tab keeps what each model may be run with there apart from what any thread runs
  with: the groups of its models above the list, each model with its groups, efforts, context (its own
  window and the largest a manager opened), the speeds and access programs that are switched on and what
  its source offered and nobody switched on. The row editor ("What this model can be run with here") is where
  a manager switches a speed, an access program or a larger context on, says what threads start with (all
  of the context as shipped), puts the model in groups, sees the caps that cover it and gives its rate with
  steps for large prompts; what the server refuses is shown at the field. Adding a limit opens the usage
  screens' form once they provide it.
- Agent profiles and environments: lists and pages; an environment that Orch made is started,
  stopped and kept awake from its page, which follows the job, and is made from a template on a host
  from the list. Templates: a list, a page with its image, confinement and needs, a recipe edited
  as JSON under the revision that was read (a change made meanwhile is not overwritten), and a dialog
  that begins from what the driver's schema asks for. Hosts: a list, a page with capacity read
  against what environments declare they need, storages, drivers, what runs on it (stop, remove after
  naming), whether it takes new environments and what it does when full, and the command that
  enrolls one.
- Left out, and why: the History tab of each (the server lists history for nodes only), signing in
  to a subscription through Orch and starting, stopping or updating a provider's runtime (not served
  yet; the page says so), usage and caps, tools and MCP servers, mounts, importing a template, a
  host's load over time (it waits for the shared time-series chart). The Access tab shows what the
  reader holds and why, and opens the permission page of the resource.

Chats among people are built too. The chat of every org, project, topic and thread stands in the
Chat view of the workbench (a thread's, with its topic's and its project's beside it), and the
direct and group chats of a root org are the Messages screen:

- The conversation: messages in the order of the chat's own sequence, grouped by author and day,
  with the line where the new ones begin; the draft is kept behind the person's back and what is
  sent shows at once, as sent or as "Not sent" with a way to try again, never lost and never sent
  twice; replies, edits and deletes shown as such, reactions, pins, `@` mentions with `@all`, who is
  typing and who is looking, the bell (every message, mentions only, nothing), and the read position,
  set two seconds after the newest message has been on screen in a tab that is in front.
- Where it stands: the Chat view beside every org home, project, topic and thread, which can be
  expanded over the screen and opened from an address (`?panel=chat&chat=…&message=…`); the Messages
  screen with the list of chats and their unread counts, a new direct chat or group, a group's people
  (somebody from outside the org joins as a guest, who sees that chat and nothing else of the org),
  and, for an admin, a switch to the chats among other people and to every place's chat, read-only,
  with the sentence that says that reading is silent.
- Quoting and forwarding: an item of a thread is quoted into its chat as a card that leads back to
  the item; messages are picked (a range by Shift) and forwarded to chats and people in one act that
  succeeds for every place or for none.
- What the system does not serve yet is shown as absent or switched off, not as dead: attaching files
  and images (the paperclip says so and the text is kept), forwarding to an agent's thread, and the
  search of chats.

What access does, and what it leaves out:

- The permission page of any thing, in two sizes in one dialog (Share in the menu of every node opens it; other
  screens call `openAccess`): the Owners line, three lines of who can view, take part and manage, with names and
  counts and the people on request, adding and removing, what goes with a permission, and a page that follows its
  place, is changed, or is put back; and the full form by area, with the advanced rows, Administration, the end and
  options of an entry, and a lock shown with where it was set. A change is made against the revision that was read,
  and a page that somebody else changed is shown as it now is, never overwritten.
- A place's People (who holds which role) and Defaults (what everything made inside it starts from, with a second
  column for work on a person's machine), and what a person without access sees: what is missing, whom to ask, and,
  for an admin of the place, the act itself behind a confirmation.
- What the system does not answer yet is said, not left empty: what a change reaches, asking for access through
  Orch, sharing with agents, and the history, effective access and people behind a chip for anything but a node.

What the settings do, and what they leave to others:

- A person's own pages: the name with the picture it makes, the address changed with its confirmation
  (the old one stays the one codes go to until then), language, time zone, theme and density; the password
  (which ends the other sessions), the second step switched on with an authenticator app (a code to scan
  and the secret in words, drawn here without a library) and with a code by mail where the person's orgs
  allow it, linked sign-ins, sessions ended; API tokens made with the permissions ticked, a place and a
  time, whose secret is shown once; and what the interface keeps for the person (saved arrangements,
  viewers, tutorials, hints) and what this browser keeps. Notifications and usage are other parts'.
- An org's settings, the same for a root org and a department: general (name, description, address,
  Move to…, Delete and Recently deleted, each saying what it does first), departments, members (a member
  moved to another domain with what changes said first, held out and let back with the reason kept, removed
  with what is theirs listed and what becomes of each resource asked; invitations in both forms by the org's
  rules), groups, tags, service accounts, the audit trail, channels and where what happens is sent. The
  permission page's defaults, the usage screen, the providers' models overview and the editor of the text a
  channel sends are other parts' and are given their place with a sentence saying so.
- The settings editor draws the keys the server serves, by group, from their schemas: the value in force,
  where it comes from (default, a place above, here), a lock set here or above, "Reset to inherited", and
  the secrets of the place, written and never shown. A switch or a choice is written at once and shown as
  pending until the server answers; anything longer is saved or cancelled. What a reader may not change
  is shown as it is, with what it takes and whom to ask. The titles and sentences of the keys are the server's
  and are English only.

The views beside a thread (`src/features/inspector`, `src/features/files`) are real for the workspace
and the runtime: Files, a lazy tree of the thread's directory drawn by the rows in view and kept equal to
the disk (by the live feed where the server serves it, by asking again every few seconds where it does
not), with the search by name and inside files, the menu of a row, and making, renaming, copying, moving,
deleting and uploading; a file opens as a tab in the code view (CodeMirror, each language fetched when a
file of it is first opened), edited and saved on the version that was read, with unsaved text kept on the
device for seven days and a file that changed underneath shown with Compare, Keep mine and Take theirs;
images, Markdown, archives, sound and film, differences and files with no preview have viewers of their own
(PDF pages do not: there is no PDF library in the build). Changes and Git list what was changed and show the
difference; Terminal is a person's own shells (xterm on a socket of its own), taken up again with what scrolled
past, one screen at a time, with the choice of which to close at the limit; Processes and Resources show the
table with signals and the small charts with their limits; Outline, Environment (where it runs, start and stop,
the idle control, the two switches its managers hold) and Parameters (the model, the effort and the parameters the
agent declares) are built in part. Not built: snapshots, mounts, the instruction file, memory, knowledge, people
and diagnostics, the status bar's branch, cursor and shells parts, and the Follow switch.

What the Inbox, the approvals and the ways a person is told do, and what they leave to others:

- The person's own live feed is followed for as long as the shell is up: the counters (what is unread,
  what waits for a decision) are on the rail, the left bar and the bar of a small screen, and move as
  things happen, on every screen. The counters are loaded first and then the newest page of rows, so
  that a row written between the two is never lost; a gap is made up for by loading the Inbox again.
  The feed's other news (an approval decided elsewhere, a route changed) is told to whoever registered
  for it, and the lists that depend on it read again. The feed is subscribed once its first load has
  succeeded, so against an older server without the notification requests the counters stay hidden and
  the approvals list does not move by itself.
- The Inbox shows what the person was told, newest first, in their own days, with an icon and a name
  for every kind of notification, where it happened and when. A click opens what it is about and reads
  it; its menu reads one, everything from one place or one chat; "Mark all read" counts the rows the
  page has not loaded too. Filters (All, Unread, Mentions) are kept in the address; older rows come a
  page at a time, and the unread ones far down are asked for by themselves. Opening a thread reads what
  the person was told about it, and what arrives while it is open. A read mark is shown at once and put
  back where the server refuses.
- Approvals lists what waits for the person, of the three kinds, and decides it in place with the
  choices of the card in a conversation: the options a harness gives, the questions it asks, the form an
  MCP server asks for; a capability with its risk, the environment and host, what it would do and a note;
  access requests, one card for what a person asked together with a row for each thing (every permission
  ticked, fewer to give, the models of a provider, when it ends, a note), where what opens a terminal or
  reaches a machine is said in words. What was decided meanwhile stays where it stood as its line and
  says who answered; a decision that comes too late says who got there first. Answering waits for the
  live connection. "Requested by me" and "History" are the other two lists; the kind and "everything I
  could decide" are in the address.
- The notification settings explain the three levels and keep what the person chose about approvals,
  chats and quiet hours. Outside Orch is a card for each way out that can reach them, each off until
  they switch it on, with what it carries, how long a notification stays unread before it goes and how
  often at most a message goes, Send a test, and what is missing before it can work (a confirmed
  address, a browser that takes push, a bot that was started) with the way to give it: the link of a
  bot with its QR code and the wait for Start, push in this browser through the service worker, and a
  webhook of the person's own.
- Left to others or not possible yet: the number of messages a chat row stands for (the row carries
  none), the list of the browsers that take push (no request lists them) and the key that browsers
  subscribe with (the server does not send it yet), and the text of a webhook (the text editor is
  another part's).

The stack is React 19, TypeScript 5 (strict) and Vite 8, with TanStack Router (routes from the files
under `src/routes`) and TanStack Query, Zustand for what the live connection keeps, Tailwind CSS 4
on the tokens of `src/styles/tokens.css`, Radix primitives wrapped once in `src/ui`, lucide icons,
i18next with English and Simplified Chinese, and Inter and JetBrains Mono served from the build. The
workbench is a layout tree of its own, with its borders drawn by react-resizable-panels and its tabs
moved with @dnd-kit; there is no docking library. Vitest and Testing Library run the unit and
component tests, Playwright with axe the end-to-end ones. Documents are written in Tiptap (on
ProseMirror), with KaTeX for formulas and Mermaid for diagrams, both loaded only on a page that has
one; CodeMirror (the code view) and xterm (the terminal) are in, each loaded when first used. What later
features need (pdf.js, Shiki, uPlot, the Typst compiler) is added when the feature that uses it is built.

## Working on it

Node as `.nvmrc` says, npm only. `npm ci`, then:

| Command | Does |
|---------|------|
| `npm run dev` | The development server. It sends `/api` and `/health` to the API at `ORCH_DEV_API` (`http://127.0.0.1:18080` when unset) |
| `npm run typecheck`, `npm run lint`, `npm run test`, `npm run build` | The four checks every change passes |
| `npm run test:e2e` | Playwright against the built application (built once when the run begins, served by `vite preview`) with the fake API of `tests/doubles` behind it; the five tests tagged `@development` (the gallery of the shared controls exists in a development build only; two reach a module by its source path) run in a project of their own against a development server the run starts beside it; `ORCH_E2E_DEV=1` runs every test against the development server instead, for work by hand; `npx playwright install chromium` once before |
| `node tests/doubles/server.ts` | The fake API by itself (`127.0.0.1:41871`), to look at the application without a system behind it: `ORCH_DEV_API=http://127.0.0.1:41871 npm run dev`, then sign in as `ada` with `correct horse battery` (the team's org, with orgs inside it, projects, topics, threads, members and invitations, and what the threads run on: machines, providers with their models, agent profiles, environments, hosts and templates, all one world for every screen, see `tests/doubles/README.md`; `vera` may only read, `dora` is a developer); a thread is given a conversation by `POST /__double/thread/scene` with `{"thread": <its id>, "name": "history" \| "running" \| "waiting" \| "failures" \| "long"}` |

The no-argument lint discovers the files accepted by the ESLint configuration and checks
them in bounded, separate processes. This keeps typed lint state within the memory available
on CI runners; every batch uses the same rules and any failing batch fails the command.
Explicit arguments retain ESLint's direct command-line behavior.

`/dev/ui` shows every shared control in both themes, both densities and both languages; it exists
in a development build only. The fake API also holds people's chats: `ORCH_DEV_API=http://127.0.0.1:41871 npm run dev`,
sign in as `ada`, and open a topic or `/org/acme/messages`; a second person (`dora`) in another browser sees
what `ada` writes as it is written. `src/generated/` and `src/routeTree.gen.ts` are written by generators
and never edited by hand.

What the finder, the search and the help do, and what they leave to others:

- One field (`Mod+K`, or the magnifier in the rail) finds everything that has a name, as it is typed:
  places, people and resources from the server (asked for 150 ms after the last key, each kind on its own
  so that none crowds another out), settings, commands and help from what the page holds. A sign at the start
  keeps it to one kind of answer (`>` `@` `:` `?` `/`); the groups stand in the order of how well they answer;
  Enter opens here, `Mod+Enter` in a new browser tab. A project in a project is a folder and is listed with
  the way to it; nothing a person may not see is listed; a command they may not use is listed, switched off,
  with the reason. A screen adds commands with `useRegisterCommands` while it is in view; the shell
  registers the ones that hold everywhere, the acts of the place in view (the entries of its own menu) and a
  go-to for each screen whose route the router has.
- The page of `/search` keeps its whole state in its address, finds what was said in conversations, chats and
  documents, shows each result with where it lies and the match marked, and opens it at the hit. The same search
  kept to one thread is the bar under a thread's header ("Find in the conversation"), which lights the words.
- A "?" beside a term opens a card of up to four parts; the help view (`F1`, the status bar, a hint's "More", the
  finder) says what the screen in front is for, lists its hints and articles, searches the guide and lists the keys
  that are bound; `/help` and `/help/<article>` are the same help as pages. The list of keys is drawn from the one
  book the shell listens with, so a key that is bound is listed. A tip shown unasked is closed for good.
- Not built: tutorials, what is new in a release, opening a result beside, finding inside a chat or a document,
  and the rows of permission pages (found once a page says how it opens a row). The two search requests are not
  served by the development world yet: everything was built and tested against the fake API.

## Shared foundation

`src/lib/routeIdentity.ts` gives every parameterized route the same screen identity, so navigating to another thing resets its local form and effects. `src/api/provider-plan.ts` and `src/api/provider-runtime.ts` use the generated provider, ledger and runtime-plan contracts.

| Home | Responsibility |
|------|----------------|
| `src/main.tsx`, `src/app.tsx`, `src/shell/` | Startup, provider composition, the workbench as the shell, banners and shortcuts |
| `src/routes/` | Authentication, hierarchy, resources, settings and management route entries |
| `src/api/` | HTTP transport, auth lifecycle and typed domain clients |
| `src/api/personActions.ts` | Explicit person notices and the actions available beside one readable thing. |
| `src/api/firstSteps.ts` | One current read of observed setup facts for the chosen org through the existing authenticated transport |
| `src/api/machinePrograms.ts` | Stored machine program snapshots and one acknowledged native registry mutation |
| `src/realtime/` | Socket recovery, cursors, deltas, repairs and terminals |
| `src/realtime/stores/` | Server-backed thread/topic/environment/tree/user/chat/document state, resource change hints and ephemeral watch state |
| [src/generated/](src/generated/README.md) | Generated API types; reserved homes for the permission and settings contracts |
| [src/ui/](src/ui/README.md) | Shared accessible primitives and domain-neutral controls |
| `src/lib/` | Formatting, Markdown, ANSI, time, graphemes and shortcuts |
| `src/lib/commandLine.ts` | Executable/argv input and readable round-trip quoting, with no shell interpretation |
| [src/help/](src/help/README.md) | The user guide read from `docs/user/` into the build, loaded on first use; the tutorials |
| `src/i18n/` | English and Simplified Chinese catalogues, held to the same keys by a test |
| `src/styles/` | The design tokens in both themes and both densities, and the base styles |
| `public/` | The application's mark and manifest; font and push service-worker reservations |
| `tests/` | Unit, component, end-to-end and accessibility tests with the fake API they run against; homes for visual and performance validation |

## Feature areas

| Home | Planned coverage |
|------|------------------|
| [workbench](src/features/workbench/README.md) | The workbench: areas, views and arrangements |
| [auth](src/features/auth/README.md) | Authentication, setup and invitations |
| [onboarding](src/features/onboarding/README.md) | First-run guidance |
| [navigation](src/features/navigation/README.md) | Organization and work navigation |
| [finder](src/features/finder/README.md) | The finder and the commands screens offer |
| [help](src/features/help/README.md) | Hints and help |
| [search](src/features/search/README.md) | Authorized global search |
| [org](src/features/org/README.md) | Organization home, the same at every level |
| [project](src/features/project/README.md) | Project topics and environments |
| [everything](src/features/everything/README.md) | The reader's paged hierarchy, search and filters in one main-area page |
| [fleet](src/features/fleet/README.md) | Fleet: environments, provider runtimes and recycling |
| [topic](src/features/topic/README.md) | Topic: threads, environments and collaboration |
| [announcements](src/features/announcements/README.md) | Configurable human broadcasts |
| [thread](src/features/thread/README.md) | Shared conversation screen |
| [timeline](src/features/timeline/README.md) | Conversation rendering and navigation |
| [composer](src/features/composer/README.md) | Inputs and delivery queue |
| [inspector](src/features/inspector/README.md) | The views of a thread: workspace, runtime and people |
| [files](src/features/files/README.md) | Files and code |
| [access](src/features/access/README.md) | Access: the permission page |
| [resources](src/features/resources/README.md) | Providers, profiles, environments, hosts, connected machines and templates |
| [settings](src/features/settings/README.md) | Personal and hierarchical configuration |
| [admin](src/features/admin/README.md) | System management |
| [start](src/features/start/README.md) | Type-and-go and explicit binding |
| [fork](src/features/fork/README.md) | Fork, switch, move and update: built in change and side-threads |
| [change](src/features/change/README.md) | Changing where and on what work runs: fork, switch, environment update, move and recycle, several at once |
| [side-threads](src/features/side-threads/README.md) | Side threads: asked on the side, shown beside the conversation, kept or closed |
| [inbox](src/features/inbox/README.md) | Notification inbox |
| [approvals](src/features/approvals/README.md) | All approval kinds |
| [usage](src/features/usage/README.md) | Usage at three depths, caps, rates and passing a limit |
| [chat](src/features/chat/README.md) | Chats among people |
| [forward](src/features/forward/README.md) | Selecting and forwarding |
| [documents](src/features/documents/README.md) | Documents written in blocks |
| [source](src/features/source/README.md) | A turn's files restored with an undo, Commit and Push, a thread's links, observed work, comments on lines |
| [browser](src/features/browser/README.md) | An environment's browsers, watched and driven a press apart |
| [vnc](src/features/vnc/README.md) | An environment's displays, drawn by noVNC, watched and worked in |

Each feature README lists its individual screens, panels and controls. User
workflows must pair visible state with committed server state, including
pending, denied, blocked, stale, offline and recovery paths. The conversation
surface is chat-first with expandable tool activity; what stands beside it,
and whether it or the code is in the middle, is each person's arrangement.

There are no structural tests or placeholder passing tests: what is tested
is what a person can do and see, and what the client promises of the wire.
The feature areas that are still placeholders are no evidence of anything;
what is built is exercised in a real browser, in both themes.

Management (`/admin/<section>`) is for a system admin in a browser session: its entry is in the account menu of whoever is one and nowhere else, and the address of a section read by anybody else shows the refusal. Each section is its own chunk, its filters and span in the address; a section whose part is not joined yet is not listed and its address is no screen ([System management](src/features/admin/README.md)).

The trace screens share one filtered audit trail in org, project and topic settings and Management. Person entries keep the selected place. [Trace and kept things](src/features/trace/README.md) documents the renderer and pending server contracts. `src/api/trace.ts` serves its reads and downloads.
- `src/api/ports.ts`: The Ports door requests: current snapshots, guarded openings, stored publication plans and entry codes. */
- [Ports](src/features/ports/README.md) — stable environment port addresses and deliberate named-place, link and host audiences; observations, policies, current guards and explicit host review. The test double verifies the screen behavior; actual TCP/UDP carrier checks are separate.
- `src/shell/screenPath.ts`: The authorized hierarchy plus the screen's own leaf, as one typed path for the title bar. */
- `src/shell/TitleBar.tsx`: The one title-bar position for the linked screen path at every window width. */
- `src/lib/viewChoices.ts`: A person's view choices on this device, shared by layouts, routes, disclosures and windows. */

- `src/shell/availability.ts`: Built screens offered by navigation; direct addresses retain their truthful unavailable state. */
- `src/api/firstSteps.ts`: One read of observed setup facts for the currently chosen org. */
- `src/api/day100.ts`: Atomic writes of one personal project view, a favorite view, revision-checked topic status changes and server-coloured inline tags.

- `src/api/documentNoticeJobs.ts`: Active discussion notification work uses bounded pages and the existing job progress contract.
