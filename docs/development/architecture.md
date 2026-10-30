# Repository architecture

Orch separates durable control-plane state, provider credentials and
conversations, and the environments in which threads run their tools. Contracts define their
seams. Applications compose replaceable implementations around those seams.
The [component inventory](components.md) links the complete scaffold and
each package's module responsibilities.

## Source layout

| Location | Responsibility |
|----------|----------------|
| [apps/](../../apps/README.md) | Executable configuration, composition and startup |
| [crates/foundation/](../../crates/foundation/README.md) | Shared primitives and compiled build identity |
| [crates/contracts/](../../crates/contracts/README.md) | Neutral schemas, commands, events, descriptors, catalogues and interfaces |
| [crates/platform/](../../crates/platform/README.md) | Configuration, telemetry, PKI, link transport, plugin supervision and the contract suites |
| [crates/control/](../../crates/control/README.md) | Persistence, access, settings, engine, API, gateway and worker |
| [crates/runtime/](../../crates/runtime/README.md) | Connector, environment host and node |
| [crates/tooling/](../../crates/tooling/README.md) | Build support, verification, generators, release tooling and behavioral test support |
| [extensions/](../../extensions/README.md) | Concrete implementations behind contracts |
| [frontend/](../../frontend/README.md) | Static web application, routes, features and shared controls |
| [deploy/](../../deploy/README.md) | Operator configuration, installation, Compose, proxy and updater |
| [images/](../../images/README.md) | Default managed environment image |
| [migrations/](../../migrations/README.md) | Canonical database migrations |
| [examples/plugins/](../../examples/plugins/README.md) | External driver and summarizer examples |
| [build/](../../build/README.md) | Component build counters |
| [scripts/smoke/](../../scripts/smoke/README.md) | Deployment behavior scenarios |
| [docs/](../README.md) | Developer, operator, security and user documentation |
| `.github/` | CI and release automation |

There is one Cargo workspace and one dependency lockfile. Contract libraries
define the wire types and extension traits; the service composition roots,
control libraries, runtime libraries and React screens implement the product
around them. Some concrete extension modules remain reserved and are labelled
in their package READMEs. Generated API artifacts come from the contracts.

## Dependency direction

| Source layer | May depend on workspace libraries in |
|--------------|--------------------------------------|
| foundation | foundation |
| contracts | foundation, contracts |
| platform | foundation, contracts, platform |
| control | foundation, contracts, platform, control |
| runtime | foundation, contracts, platform, runtime |
| extensions | foundation, contracts, platform telemetry |
| apps | All production library layers |
| tooling | All library layers |

Contracts are the meeting point between control-plane and machine-side
implementations. Neither imports the other. Composition roots in `apps/`
select extensions and pass them through contracts. Development dependencies
may cross layers to exercise real behavior; build-support is a build-only
helper. Cargo rejects cycles. These are review guidelines; there is no custom
architecture checker or test of implementation shape.

The suite that every implementation of a contract passes is a platform
library, `orch-conformance`, and not test support: the operator's
`orchctl plugin check` runs it against an external implementation. Test
doubles stay in tooling. An extension has no use for the link, the PKI or the
plugin transport, so it depends on telemetry alone among the platform
libraries. Packaging and every component's build identity belong to xtask.

One concept has one home. Shared types belong to the boundary they describe;
shared mechanisms belong to platform libraries. Settings and permission
catalogues are contracts; their decision engines belong in control libraries.
There is no catch-all common crate. Placeholder packages acquire dependencies
only when real contracts and behavior need them.

## Product ownership

Organizations contain projects, topics and threads, and may contain
organizations. There is one kind, the same at every level: one that lies inside
another is called a department, and has projects, people, pages, settings,
groups and admins of its own, and departments of its own, as deep as the system
allows (eight levels as shipped). The one at the top is the root organization;
what belongs to it as a whole is the short names of its tree, its direct and
group chats, and who counts as coming from outside. Every member has a domain,
the organization they were put in: they are a member of it and of every
organization above it, and a role held somewhere reaches that place and what
lies inside it, never upward. A project can lie in an organization or another
project; nested projects are shown as folders, with the same pages, settings
and inherited access. A topic lies directly in a project or folder and can move
to another. A topic is a subject of work: it holds threads and the defaults they start
from. A thread binds a provider, a profile, a model with its reasoning effort,
and one environment, in which it has a directory of its own. That directory is
to the thread what a working directory is to a process: where its agent starts,
and not a boundary. Threads bound to one environment share its files, processes
and network, the home and browser profiles and displays. The environment is
their shared execution boundary; threads that must
not see each other's data get environments of their own. Subagents are observed
harness-managed streams, not independently bound Orch agents. Settings resolve
down one hierarchy, from the system through the organizations a thing lies in,
its project and its topic to the thing concerned (a thread, an environment or a
provider), whose own value comes last, and a value may be locked on the way.

Every person also has an organization of their own, made with them and empty.
As made it takes no other member and holds no organization inside it: it is
where somebody keeps projects and documents that are theirs, and shares one
page at a time. Both limits are values of the organization, set when it is made
and changed by a system administrator.

Access is stated on the thing it is about. Every organization, project, topic,
thread and document, and every resource, has a permission page: rows that are
permissions, and entries that name who holds each one, as a role held in a
place, a group or a person. A permission is an area and a level. A thread has
its conversation and chat areas at View, Interact or Manage. Files, terminals,
ports, browsers and displays are areas of the environment: a thread's views
use those environment permissions. Files and terminals have View and Manage;
resource areas can have View, Use and Manage as their contract declares. A
level includes those below it, and no area implies another. Documents
are an area of every node. Beside the levels a page has advanced rows, each for
one act that a team may want to give apart from them: forking a thread,
archiving a thread, a topic or a project, making a document, commenting on one,
seeing the usage totals of a place, naming somebody from outside the
organization, letting a turn past a usage limit. Who owns a thing is a row as
well: the Owners row names one person or several, the first of whom stands for
it, and the last cannot leave it.

A page stores only what somebody changed. Every other row follows the defaults
of the place the thing is in, and those follow the place above, up to the
system: a change to a default reaches every page that follows it, says first
who gains and who loses what, and a default can be locked so that nothing below
departs from it. Viewer, Interactor, Developer and Admin are names that a
person holds in a place and that pages refer to; what a role can do is what the
pages give it. As shipped, Viewers watch, take part in chats, read documents,
comment on them and write documents of their own, and may fork a thread into
one of their own; Interactors drive agents and write the documents of the
place; Developers open topics and manage them, set threads up and manage them,
their files and their terminals, and name people from outside; Admins make
projects and organizations and manage them. The defaults differ by where work
runs: on a person's own machine they leave everything beyond watching to the
thread's owners. Three choices (can view, can interact, can manage) name
several rows at once for everyday use, and a page that gives a permission
offers what it is no use without, such as the model a thread runs on.

A few rules are not rows, each because no page could sensibly change it. Admin
is the highest role and is inherited downward, from the system to its
organizations, the organizations inside them and on: the admins of a place hold
what its pages give Admins and may always change the pages in it. Where a page
leaves an admin out they can force an act there, which is the one thing an
admin has beyond the pages: the act is confirmed with a line that says so, it
puts them on the page by the ordinary change of a page, and the audit row of
that change is marked as a bypass. Every act is checked against whoever does
it, and an input needs its sender's own use of the model. An environment is
viewed or managed, with nothing between. A person's machine enforces its own
local policy before every page. Somebody suspended holds nothing.

Providers, profiles, environments, hosts, templates, MCP servers and mounts are
resources. Each has owners and a home, an organization at any level: a member
makes a resource in their domain or in an organization inside it. The home
decides whose defaults its page follows, whose admins administer it and which
policies govern it; it does not decide where the resource may be used. A
resource is used wherever its page gives access, by work of any organization,
with nothing copied, and wherever it is offered outside its home its address
says where it comes from. By default a page names the roles of its own tree of
organizations. Naming people, groups or roles from outside it, on the page of a
thread, a topic, a project, a document or a resource, takes a row of its own,
which as shipped the Developers of the place hold; the owners of a thing may do
the same where a policy lets them, as it does when shipped. Whoever is named
holds what the row gives like anybody: the separation between organizations is
what the pages say.

An environment is owned by a person: whoever created it from a template, or
whoever connected the machine it is a directory on. Its own page has two
levels: who sees it, and who manages it, which is everything about it and
everything about where threads run in it: putting a thread into it, giving a
thread another directory or another confinement there, taking it out.
Managing a thread gives none of that. Its ports, browser sessions and displays
belong to the environment, and a thread supplies a route to their views and
controls. Nothing lies between, because an agent
that runs in an environment reaches all of it. One on a person's machine
starts as its owner's alone and is shared like any resource: its owner names
who else sees it and who else manages it. What people do through one thread
that runs there, reading its files or opening a terminal, is on that
thread's page, and what those permissions reach is the whole environment,
which each page says. Creating an environment from a template needs the use
of a host and of the template. A person's machine adds its own local policy,
which comes first and holds for everyone. It is changed on the machine; a
key its owner handed over is changed from the application by whoever manages
what that key bounds.

An agent is told three things that are no person's message. Orch's preamble
says what Orch is and whose word counts, in paragraphs that only the system
may reword and no place may leave out, and a block of its own says where the
agent is. The thread's standing instructions are a setting, inherited
from the organizations down, and go with every session. And `AGENTS.md` at the
root of the thread's directory is a file of the workspace like any other:
people and agents write it, both built-in harnesses read it by themselves where
they run in the directory, and Orch shows it and lets it be edited where a
thread is created and set up, and otherwise has no part in it.

What an agent notes for itself from one session to the next is files as well,
in `.orch/memory` of its thread's directory: an index and the notes. That is
the one place for it, whichever harness wrote it, because the directory is what
lasts as the work lasts: the notes stay through every stop, recreate and
update, move with the environment, are found by the next harness after a
switch, and are read and corrected by people. A harness whose own memory can be
pointed at a directory keeps it there; one whose memory is a single store for
its whole home has it switched off, since that home serves every thread of a
provider; wherever the harness does not keep the notes, the agent does, with
its tools. A sign-in belongs to a provider, a session to a thread on one
harness, and notes and instructions to the directory the work is in.

Driving an agent and setting its session up are two levels. Interact on a
thread drives it: sending input, which starts a turn or waits for the next,
steering a running turn, stopping it, and ordering what waits. Manage sets
it up: its model and effort, its parameters, its instructions, its approval
mode and the answers to what the agent asks before it acts, and its
provider, by a switch. Forking is a row of its own, which as shipped
everybody who can read a thread holds. Giving input also needs the sender's
own use of the provider, with the model and effort the thread runs at. A refusal names
everything that is missing, and one request asks each owner for their part.

Whoever makes a thing is its first owner, and it is theirs alone until they
share it or name another owner. Members may make providers, hosts, templates,
MCP servers and mounts of their own, where their organization allows it; agent
profiles are made by its administrators, because a profile's configuration runs
beside a credential. An owner is a member: when somebody leaves, what they
owned with others stays with the others, and what they owned alone passes to
the first owner of the organization it lies in, or leaves with their own
computer. A thread holds nothing by itself and remembers nobody whose access it
would use: whoever puts it on a provider or into an environment is checked for
that then, each input is checked against its author, and an input that no
person sent is asked of the thread's first owner. Moving work is not taking it:
a switch, a move and an update change neither who owns a thread or an
environment nor what its page says. A switch asks only for what it changes:
managing the thread and one's own use of the new provider, or managing the
environments concerned. A fork is its maker's, and starts with the page of the
thread it was taken from. A member is moved to another organization of the tree
in one act, which changes their domain and the role they hold there and nothing
they own; a project and a department are moved inside their tree the same way,
with what changes said first. What was deleted can be restored until it is
purged. Access is withdrawn at three boundaries: a system administrator
suspends a user everywhere, an organization's administrators suspend a person
in that organization, and whoever may change a page takes a name off it.
Suspending closes a door at once without deciding what becomes of a person's
things.

Every act that needed a permission and changed something leaves a line in the
history of the thing it was done to: who did it, what, and when. A thread shows
its lines between the items of its conversation; a topic, a project and an
organization at any level show theirs, with the management of what is inside
them; a resource and a document show their own. The line is the audit row of
the act, so there is one record.

An environment is given the variables and secrets of the node it takes its
settings from, and may hold its own, written by whoever manages it. Giving
it another such node needs the right to create environments there.

A notification leaves the application through a channel, which is one use of
a notifier: a mail server, a Telegram bot, a webhook, push, or a kind somebody
installed. A channel is set up by the system for everybody, by an organization
for its members, or by a person for themself, from the fields its kind
declares; the system's mail server is a channel like any other. What goes
through a channel to a person is that person's route: off until they switch it
on, with how long a notification stays unread first and how often a message
goes. A Telegram bot is asked what it was sent and never listened for, so
nothing of a system has to be reachable from outside for it. An organization,
a project or a topic also has destinations for what happens there. The Inbox
is state the engine writes with each event, not a channel.

Who may register is a policy: anybody, only somebody with an invitation (as
shipped), or nobody. An invitation is a link to share, bounded by a number of
people and by time, or a personal link for one address. Signing in takes one
proof, or two for a person who has a second step: a password, a code sent to a
confirmed address where every organization of that person allows it, and a
code of an authenticator app. Wrong tries are counted by the name that was
tried and not by the address they came from, each proof by itself, and a
browser its person signed in from by itself, so that guessing at a name
closes it for strangers and not for its person.

Every part that talks over a network is built for a slow link, by one profile
that the specifications and the tests share: what a person types is local
first, what they did shows at once as pending and never as done, a request
that got no answer is safe to repeat, uploads go in chunks that continue,
streamed text is merged for a reader that is behind, and nothing is closed for
being slow.

A provider is set up in more than one way, and the ways are declared rather
than written into screens. An adapter lists its sign-in methods with the fields
each asks for; a model format lists what one of its endpoints may configure
(how the key is sent, fixed and secret headers, query parameters); presets fill
the form for known vendors. The page that adds a provider is rendered from
those declarations, so an implementation added from outside appears in it like
a built-in one. What a person pastes for a sign-in is sealed at once and kept
only in the provider's runtime, on a volume that is made with the provider and
stays through every stop, recreate and update, so a sign-in is given once. An
existing sign-in is copied in by a method of its own, as text or as the path of
a file inside the runtime; copying it in again is that same method, run by a
person or by a script, and nothing watches a file or writes one back. Which
agents' subscriptions a system or an organization accepts, and whether one may
be used by anyone but its owner, are policies; an API key is always the other
way in. What an agent, its subscription and its plans are called comes from its
adapter's descriptor, and a subscription of any other product is an adapter
written against the same contract.

A provider keeps its credential and its conversations in a runtime of its
own, and a login belongs to a provider, never to an environment. Managed
runtimes form two pools, made and looked after in the same way: environments,
where work runs, and provider runtimes, where credentials are kept. A unit of
either pool is made from a template of its kind, on a host its owner may
use. A provider's runtime is made when the provider is added and kept for as
long as the provider exists; a provider on a person's machine has none,
since that machine is its runtime. Each provider/profile pair has its own
harness home, with one process or
one for each conversation as the harness is built. Agent profiles have no
runtime of their own. A provider's runtime stops when it is idle, and a
stopped one is started from time to time so that its harness renews its
sign-in.

How an agent is placed for a thread is its arrangement, and there is one list
of five types. Separated: the harness runs with its provider and reaches its
tools through an executor in the environment, which holds nothing of the
provider. Key-separated: the harness runs in the environment and calls a
forwarder, which adds the key where the key is kept. Credential-shared: the
harness runs in the environment on a copy of the credential, which the provider
issues and renews. Handed-over: the login itself moves to one environment until
it is returned, and a failed return prevents stopping that environment.
Together: one machine that is both places. An adapter declares the types its
harness can run in, in its own order of preference; a provider's owner says in
which of them the provider may be used, which is their consent to what each
type puts into an environment; a policy may narrow and order them; and a
person's machine keeps a list of its own. Every environment has one exec node,
the connector there, and it is kept simple: it says what it offers a harness
that runs elsewhere, as a list of the environment's own, set by its template or
on its machine. The entries are a harness's own exec server, or one of the
three tool sets of Orch's executor: tools with the names and inputs of a
harness's own, plain ones, or none of its own, so that the agent acts through
the environment's MCP servers alone. A session takes the first offer its
harness can use, and nobody chooses for a thread. Orch's own tools belong to
the server: they are offered beside every executor, unless the environment says
otherwise, and are part of none. The shared-tree readers list, read, search and
download whatever kind was shared. The other tools act on that kind: a document
is edited or managed, a chat is written in,
another thread's agent is handed something, everybody on the work is told, the
environment is asked for more or its browser is used. Each is defined by the same rules: a name that
says the verb and the kind, a closed schema that asks for nothing Orch knows,
an answer that carries what the next call needs, a refusal that says what to
do instead, and a truthful statement of whether it only reads. Several connectors run on one computer, each
with its own directory. Core decisions use declarations, not
implementation-name branches.

An MCP server reaches an agent in one of two ways. A preset is written in Orch
and runs in whatever environment enables it. A server that a harness already
has configured, on a person's machine or in a unit's home, is found, then
imported, then enabled: found is a name reported by the adapter; imported is a
copy of its definition kept by the connector on that machine, with a page in
Orch like any resource; enabled is the same switch a preset has. The
definition, with whatever tokens it holds, never leaves its machine, so such a
server runs there alone; and a harness that Orch starts is held to the servers
of its session, so nothing is used that nobody imported.

Nothing Orch starts reports to its vendor: harnesses are started with their
telemetry and self-update switched off. The server stores the metrics of its
own services and of its hosts, with the usage ledger and the audit trail;
logs stay on the machine that wrote them. None of it leaves the system. The
ledger, its rollups and the audit trail are kept for good; metrics are kept
by the hour for good and by the minute for a month.

This release runs one API process. Live tickets and browser, display and
terminal tunnels are retained by that process; no multi-process routing or
shared live-ticket store is promised. The API hosts authorization, authoritative state and the engine's machine work,
jobs and schedules. The worker handles transactional outbox delivery, what
waits on a person's routes, and database-only upkeep. The gateway is a stateless relay. The node owns
managed runtime lifecycle, tool caching and reconciliation through drivers. The
connector hosts provider, agent and environment roles; envhost supplies
reusable workspace, process, executor, Git and terminal services across
supported operating systems.

The web application is a workbench, in the manner of the code editors people
work in all day: areas with borders that are dragged, and views that go where
their reader wants them, so that a thread is read with its conversation in the
middle and its files beside it, or worked in with the code in the middle and
the conversation beside it. A file opens as what it is, in a code view with
viewers by kind of file, editing with highlighting, a search inside files, and
a menu under the right mouse button; the list of viewers is where a further
kind of file is added. One finder reaches everything that has a name, every
setting, every row of a page, every command and the help. Help is where the
question arises: a hint beside a term or a choice says what it is, which one
fits and where it is set, and the user guide ships inside the application with
tutorials that run on the real screens. A shell in an environment is its
opener's alone and stays while they are away, so that a person works beside an
agent in the same files; what they want the agent to know, they send.

PostgreSQL and blobs preserve product state. Connector streams spool, commands
journal their results, and tunnels have bounded recovery. Browser stores converge
through sequenced updates and HTTP repair. Every visible workflow must correspond
to committed state and observable effects, with explicit pending and failure states.

People talk to each other in chats: one beside every organization, project,
topic and thread, open to everyone on it, and direct and group chats inside a
root organization, which whoever holds that row on an organization can inspect
for the people of it. A chat is separate from an agent's thread in history,
composer and read state. Forwarding copies selected messages, with their files,
to several chats, people and threads at once; an agent receives the files in
its workspace. Sharing acts on a whole chat, thread, topic, project or
document. For people it is a shortcut over the permission page: it names them
there, tells them with a direct message when the sharer asks for that, and does
so for several things at once. For agents it is a copy written into the agent's
workspace or a live share: by area (agents' conversations, people's chats,
documents), of one thing or of a whole place, with the agents of one thread or
of a whole place, to read or to write, until the share is taken back.
What was shared is one tree, which an agent lists, reads, searches and
downloads from. Writing differs by kind: a document is edited, and editing a
place's documents is also what makes documents there; a chat is written in; a
thread's agent is handed something. Whatever is read has one written form,
made by one renderer: a conversation, a chat and a document are each a
sequence of parts, read as a range at one of three depths, and the same text
is what an export, a copy, a forward and a handoff to another agent hold. Of
another thread the depths are the ones its people read it at: in brief, as it
was said with a line for each step, and in full with what each step was given
and what came back. So
a topic can be kept whose agent is given the other topics to read and writes
what they came to. Beyond its own thread an agent reads only what a policy for
neighbouring threads allows and what a person shared with it. Threads of one
topic may always send to each other.

People write things down in documents: pages of blocks beside any organization,
project, topic or thread, written in place by several people at once, with
comments beside the text, files, and a history. There are twenty kinds of
block, tables, columns, formulas and diagrams among them. Who reads, comments,
writes and manages them is rows of the node's page and of the document's own,
which start as open as the chat. What is stored is the blocks; Markdown is a
habit of typing and a format at the edges. What an agent changed is never
hidden in a document: a version is cut by the clock while a document is
written, each entry of its history names its authors, people and agents
alike, and each changed block says who changed it. A document is written out
as JSON with the sync's block structure, as Markdown, which says as much of it as Markdown can, as one HTML file, as
Typst source, and as a PDF that the reader's own browser typesets from that
source, so that no service compiles documents.

An environment and the machine it runs on are separate. People choose the
place, a host they may use or a machine of their own, with its free capacity
shown, and can move later. Fork is the one mechanism: a fork makes a new
thread with a choice of history and files; a switch changes a thread's
provider or environment by forking in place; a move takes an environment to
another host with everything in it. A switch, a move, a recreate and an update
run by one procedure: a plan with its parameters; a confirmation, which locks
the thread or the environment; durable phases that report a percentage;
health checks that spend no token; one committing transaction; and removal of
the old side scheduled at once and done in the background. Anything that
fails or is cancelled before the commit is undone. An environment that has
fallen behind its template, image or tools says so, and so does a provider's
runtime; either is updated the same way, with the old unit kept until the new
one has passed its checks. Nothing a unit runs changes by itself: not when it
restarts, and not when the system is updated.

Where a container's file system comes from is a choice of three, for the
templates of both pools alike: an image by reference, a Dockerfile built on
the host, or an image archive. A template can
be imported from any of them: the import proposes a filled-in form with a note
for everything it could not carry over, and saves nothing. A unit's main
process is Orch's own, so an image's entrypoint is not run. Each driver
declares its sources and its imports; a host's operator can turn building off.

The workspace is always plain storage, written in place, and the
environment's own: a volume made with it, or the directory registered on a
person's machine. It is what lasts and what travels: it stays through a
recreate and an update, and a move takes it to the new host. Mounts are data
that lives on particular machines: inherited from organization to project to
topic to one environment, attached read-only or shared, never copied, and
kept after a move only where the new host has them too. A private view
extends read-only where the host can make one: the data stays untouched and
one environment's changes stay in a layer of its own. A host's named
storages decide which disk each volume lands on.

A provider's requests are relayed unchanged in the format they arrive in,
OpenAI Responses or Anthropic Messages. Translation between formats is left
to external implementations of the translator contract.

Every use of a model is a row of a ledger that says what it ran on: the model
with its effort, the provider, the agent, the kind of environment, the place
and the person. Use is counted in tokens and in credits, which are tokens at
a rate: a provider's own, the rate card an organization writes, or, where
neither says, an estimate from a catalogue that ships as data, with the page
and the day each figure was read. A figure that rests on an estimate is marked
as one, and a model without any rate is counted in tokens and shown as
unrated. A model has a name that is sent and a name that is shown; rates,
caps and allowlists name models by one pattern rule. Orch records usage and
bills nobody.
What was used is one screen at three depths: one's own, in full, always; the
totals of a place, over time and by each of those dimensions, for whom its row
names; and the reports, with the records behind every figure.

Usage caps bound consumption in tokens or credits, and how many turns run at
once, by scope, model or family of models, kind of agent, provider, person or
role. None exists by default. A cap on turns at once is the one way concurrency
is limited: the turn that finds no room waits its place. How firmly a cap on
tokens or credits holds follows from who reports usage, which each arrangement
declares. A cap that is reached holds for everybody it covers, with one way
past that leaves it as it is: a limit bypass. Whoever holds the row for it
where the cap is defined (as shipped the admins of a place, and a provider's
owners and managers) reads what is used of how much and lets one turn run; the
turn is counted, and the pass is recorded on the cap, in the audit trail and in
the conversation.

Access checks are centralized and apply at the relevant action: the page of
the object, the defaults it follows, token scopes and suspension. A system
administrator is an Admin of every organization by inheritance, in a browser
session only: a token carries none of it, and nothing else marks them out.
Models are explicitly allowlisted; discovery only suggests choices. Behavior-changing defaults
belong to settings, configuration, data or versioned contracts.

## Current validation boundary

`cargo xtask verify` checks formatting, compilation, Clippy, rustdoc and
dependencies. CI additionally compiles native platform and musl targets.
Compilation is followed by contract and package behavior tests, PostgreSQL
integration tests, frontend checks and browser/deployment scenarios. Native
harness cases need their pinned binaries and actual target runners. A compile
or a constructed double does not certify a platform, sign-in or release stack.
