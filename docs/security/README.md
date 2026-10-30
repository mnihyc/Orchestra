# Security boundaries

Orch evaluates access at the server and enforces machine policy at the connector.
The tests linked below exercise these controls. The release runner must still
record its candidate checks, native-platform results and joined browser walks;
this page describes the boundary and its limits.

The server evaluates each action against the permission page of the object
it touches, the defaults that page follows, token scope and suspension.
Every resource has a home, an organization at any level, and is
used wherever its page gives access; it says where it comes from wherever it
is offered. Somebody from outside the root organization is named on a page,
of a thread, a topic, a project, a document or a resource, only by whoever
holds the row for that there, which as shipped is its Developers and admins,
or by an owner of the thing where a policy lets owners, which it does as
shipped; an organization that wants neither empties the row, locks it and
turns the policy off. Only an admin names the holders of that row, and no
default names anybody from outside.
Who owns a thing is a row of its page. Owners hold every permission of what
they own except naming people from outside, which the policy above gives
them; they name each other, and the last one cannot leave.
A person's own organization takes no other member and holds no organization
inside it as it is made; only a system administrator lifts either limit.
Projects can contain projects; the interface calls a nested project a folder.
An inherited role in a place reaches its folders and their contents. Sharing
one thread or document reaches that thing alone. Sharing a place offers two
different reaches: the place itself, or the place and its contents. Every grant
and agent share identifies its giver; holders share only what they hold, where
the place permits it, and managing the permission page is a separate right.
Watching a thread, driving its agent and setting its session up are separate
permissions. Driving is sending, steering and stopping. An input always needs
its sender's permission to drive the thread. Where the sender may not use its
provider, model and effort, the place can hold the input for approval, refuse
it, or run it on the first owner's access. A held input runs only after
somebody who may drive the thread and use that model chooses Run it; their
current access, usage and caps apply, and the author stays named. Changing the
model or parameters and approving an ordinary tool request require managing
the thread. A proposed plan can be answered by whoever may drive the thread
unless the place restricts plan replies to its owner and managers. Proceed
keeps the thread's approval mode.
An environment's workspace is the boundary, and a thread's directory is not.
Reading files or opening a terminal through one thread reaches the whole
environment; every page that gives either says so. Threads in it can also
share the user's home, process table, listening programs, browser profiles and
displays. A browser's stored sign-in is accessible to agents running in that
environment even though its profile is outside workspace previews and snapshots.
Work with different trust requirements needs separate environments.
Admin is inherited downward, from the system to its organizations, the
organizations inside them and on. A role held in a place reaches that place
and what lies inside it, and nothing above: a member of a department holds
nothing in the organization over it by being one. An admin holds what the pages of their place give Admins
and may change every page in it. Where a page leaves them out they can force
an act: it is confirmed with a line that says so, it puts them on the page
for what the act needs, and the audit row of that change is marked as a
override. The page shows them from then on, and nobody is notified. Nothing
gets an admin past what is closed to everybody: a row locked from above, a
subscription kept to its owner, a machine's local policy. A system
administrator is an Admin of every organization by that inheritance, in a
browser session only: an API token carries none of it, and nothing else
marks them out.
Archiving a project, a topic or a thread, and bringing it back, is a row of
its page, held by whoever manages it and by whomever the row names.
An environment is viewed or managed, with nothing between, since an agent
that runs there reaches all of it. Putting a thread into one, giving it
another directory there and taking it out take managing the environment, and
managing a thread gives none of that. One on a person's machine starts as
its owner's alone and is shared like any resource.
Suspending a user ends their sessions and tokens at once; suspending a person
in one organization leaves them nothing there. Neither passes their things on.
Connected machines enforce locally owned policy that the server cannot widen,
for administrators too, and it comes before every page. It is shown in the
application and changed on the machine; a key its owner handed over is
changed from the application by whoever manages what that key bounds.
Providers retain credential custody and renewal when sharing; handover is
exclusive to one environment and must be returned before it stops. In which
arrangement types a provider may be used, and so what of it may ever be in an
environment, is its owner's consent, given and seen before the provider
exists; as shipped a key never leaves its provider.
The selected driver and environment capabilities determine confinement;
a capability is decided on the side of the host it widens access to.
In an environment made from the shipped image the agent's account can become
root with `sudo` and no password, so that whoever works there installs what the
work needs; that is root in the unit and nowhere else, which is the boundary for
everything an agent runs there, and a template that sets `no_new_privileges`
takes it away. The browser there runs without its own sandbox where the
container cannot give one, which the environment says.
Harnesses are started with telemetry, error reports and self-update off. Orch
keeps its metrics and logs locally. Model requests, enabled notification
deliveries and browser navigation reach the endpoints chosen for that work;
operators enforce further network limits through a verified driver policy. The usage
ledger and the audit trail are kept for good.
A switch, a move or an update retires the old side only in the transaction
that commits the new one, after its health checks; anything earlier is undone.
None of them changes who owns the thread or the environment, or what its page
says, and nothing a unit runs is replaced except by such an update.
A thread holds nothing by itself and remembers nobody whose access it would
use. Whoever puts it on a provider or into an environment is checked for
that then; each input is checked against its author; an input that no person
sent is asked of the thread's first owner. A switch changes what a thread
runs on and nothing about whose it is, and asks only for what it changes. A
fork is its maker's, and starts with the page of the thread it was taken
from.
Moving a member to another organization of the tree changes their domain and
the role they hold there, and nothing they own; whoever moves them needs to
manage members where they are and where they go, and is shown what the
member stops and starts reaching before it happens.
Whoever makes a resource owns it, privately until shared; nothing is left
without an owner when a member leaves: what they owned alone passes to the
first owner of the organization it lies in.
Every act that needed a permission and changed something is audited in the
same transaction, and that row is the line in the history of the thing it
was done to.
How many turns run at once is bounded by a cap, as tokens and credits are,
and no role is exempt from one by being that role. A cap that is reached is
passed in one way that leaves it as it is: whoever holds the row for it where
the cap is defined lets one turn run, after reading what is used of how much.
The turn is counted, and each pass is a row beside the cap, an audit entry
and a line in the conversation. It is always somebody's act, under their
name: an input that nobody sent is never let past a limit, and an agent
cannot ask for it.
A sign-in or a token pasted into the browser is sealed at once and kept only
in the provider's runtime, which is placed on a host of the system or one
its owner manages, since whoever manages a host can read what is on it.
Nothing of anybody's work is in that runtime, and nothing of its host is
attached to it: its sign-in is on a volume of its own, made with the provider
and kept through every stop, recreate and update. An existing sign-in is
copied in, as text or as the path of a file inside the runtime; nothing
watches a file, and nothing is written back to one.
Agent profiles are written by an organization's administrators, because a
profile's configuration runs beside a credential. A template a member made
runs only where a host's page allows, and its risky members stay with
system administrators. An image built from a Dockerfile is built without
privileged features or secrets, and never runs its own entrypoint.
An endpoint without TLS is refused unless a policy allows it.
A system or an organization can refuse an agent's subscriptions by policy,
or accept them and keep each to the person who owns it; the dialog that shares
a subscription says whose plan the work will run on.
Deleting a provider removes its runtime and the volume its sign-in was kept in.
An environment takes another node's settings and secrets only where its
manager may create environments on that node. A destination of a project is
added by whoever makes rules there and is sent nothing from private places
below it and nothing people say in chats.
Chats reach an agent only through a deliberate forward or live share. Public
documents beside its topic and that topic's main threads have their own
policy: as shipped the agents may read, edit and create them, under their own
names. A place may narrow that policy to reading or to no implicit document
reach. Private documents and restricted places stay outside it. Further
access, including chats and other conversations, comes from the corresponding
policies and explicit shares. Whoever may message an agent can learn what it
can read. Each kind is written to by a right and a tool of its own: a share
that lets an agent write in a chat reaches no other agent, and one that lets
it send to a thread opens no chat. What an agent reads of another thread,
where it may, is what that thread's people see of it, and never its reasoning.
Orch checks every call of its own tools itself, whatever a harness makes of a
tool's description. What a place was shared with reaches the agents started
there later; what was made private inside what was shared is left out. A
search by an agent looks only at what that agent may read, within a bound in
size and in time. Whoever holds that row on an organization, its admins as
shipped, can inspect the direct and group chats among its people, which is
audited and announced to nobody.
A skill is executable content supplied by its managers. Publishing or enabling
one is a trust decision for the threads that receive it; archive validation
confines its files and does not establish that its instructions or scripts are
safe. Importing from an address or repository lets the chosen host fetch that
source with the host's network reach, under its local fetching policy. A
source address shown with `?…` keeps its private part with the skill: it is
never returned, its managers included.
A document is typed blocks and never markup. Two people who change one block
do not overwrite each other: the change that comes second is refused and
kept on screen beside the first. What an agent changed in a document is in
the document's history as anybody's writing is: the entry names it among
its authors, each changed block says who changed it, and an earlier version
is put back by whoever manages the document. A document that an agent
makes belongs to the first owner of its thread. An agent writes no comment;
it reads the discussions beside a document where it reads the document in
full. A film or a sound given as
an address is fetched by the reader's browser only once they chose to play
it. A PDF is typeset in the reader's browser from source the server wrote:
no service compiles a document, and the page fetches the compiler from the
system itself.
An agent that works with tools other than its harness's own says so on its
thread, with which tools. Orch's own tools are answered by the server and
are no part of any executor, so no executor can see the calls made to them.
`AGENTS.md` of a thread's directory is a file in the workspace: whoever may
write there, an agent included, can change it, and it is on screen where the
thread is set up. What must hold whatever the directory says is the thread's
standing instructions, a setting that no agent writes.
What an agent notes for itself is files in its thread's directory, read and
corrected by whoever may read and write that thread's files. No memory of a
harness's own is left where it would be shared by threads that share nothing
else: one that a harness keeps for its whole home is switched off, since
that home serves every thread of a provider.
An MCP server that a harness already has configured on a machine is used
only once somebody who manages the environment there imported it. Its
definition, with whatever tokens it holds, stays on that machine, and it
runs there as the machine's user, which the dialog that imports one says.
A harness is held to the session's MCP servers only when its adapter declares
and has proved strict MCP control. A harness that cannot do so reports that
limitation; importing a server gives the program the machine user's permissions.
A shell is its opener's alone: nobody else attaches to it or reads what is
on it, whoever they are, and whoever manages the environment can close it.
That a shell was opened, by whom and for how long is a line in its
environment's history. The last of what it printed is kept with that line
when it closes, as shipped, masked like every stored text, and can then be
read by its opener, by whoever manages the environment and by whoever reads
the audit trail there; such a read by another is itself recorded. Nothing in
a shell says so while it runs: what a place keeps is a setting of the place,
said where it is set and read in the history.
It is as strong as the environment: it reads what every agent's processes
there can. It ends when its opener loses what opening it took, and after a
time with nobody attached.
A file viewer never runs what it shows: HTML is shown as code, an SVG is
drawn as an image, and an archive is listed, never unpacked in the browser.
Threads
that share an environment share its files; a private view of a mount never
writes to the data underneath, and a workspace is never such a view.
Pointing a mount at a directory of a host is an act of that host's managers;
on a person's machine it is the owner's, inside the roots that machine allows.

Signing in counts wrong tries by the name that was tried and not by the
address they came from: eight of one proof in three hours close that proof
for that name, from every address alike, and a name nobody has is counted
like one somebody has. A browser its person signed in from is counted by
itself, so that guessing at a name closes it for strangers and not for its
person, who is told. A code sent by mail is a way in, or a second step, only
where every organization a person belongs to allows it; it is as safe as the
mailbox it goes to, which the page that switches it on says. An address is
changed with the password and counts once it is confirmed. Every answer
about signing in is the same whether a name exists or not.
Nothing leaves the application for a person until they switch a route on.
A channel is set up by the system, an organization or a person; an address
it reaches is checked against private networks unless a system administrator
listed it; its secrets are sealed, and a text it sends holds placeholders
from a closed list and no logic.
An invitation is a link to share, bounded by a number of people and by
time, or a personal link, good once and for its address alone.

An environment owns its doors: published HTTP, TCP and UDP ports, managed
browsers and displays. A thread is one route to them. WebSocket tickets are
short-lived, scoped and consumed once; losing access closes a viewer or driver.
A shared display may carry clipboard bytes to its watchers. Clipboard use
therefore has the same audience as that display, and its native round trip is
part of the release's display proof.

Stored masking replaces the exact secrets Orch knows at that place. It cannot
discover an arbitrary password typed into a shell or pasted into a document.
A place can deliberately retain a terminal's final output without masking;
people who can read that retained record can then read that text.

Machine reports are accepted only from the machine that owns their stream.
Reports from another machine are acknowledged and discarded without changing
content or a cursor, counted, and recorded for administrators at a bounded
rate. A session's segment retains the identity of its harness machine so late
reports cannot acquire the authority of its replacement.

The connection's peer address is authoritative unless it is a configured
trusted proxy. Forwarded addresses are then read from the right through that
trusted chain. The same derived address feeds sign-in limits, audit origin and
HTTP caller information. The application defaults to trusting no proxy; the
deployment must name its actual edge network.

Notifier connections check the address they will connect to and every redirect,
using the shared special-purpose address policy. Loopback, private, shared,
documentation, reserved and multicast ranges are not public destinations;
embedded IPv4 addresses are checked as IPv4. Explicit private-host exceptions
belong to the system's outbound policy. Provider endpoints have their own
TLS policy and are not promised the notifier's private-address fence.

A unit requiring a restricted network starts only after its driver has verified
the gateway/model/blob transport paths and the permitted destination set. A
network with no additional destinations uses the same restriction mechanism;
it does not mean an unrestricted host bridge. Unsupported enforcement refuses
creation, rather than falling back to an open network.

The operator supplies the HTTPS edge, keeps deployment secret files readable
only by their account, and retains the master key and database password in a
private backup. Stock deployment images are selected by digest, including the
image index shared by supported processors. The shipped services run one API
process: terminal, browser and display tickets and their live tunnels belong
to the process that made them. Replicating that process is not a supported
deployment in this release.

Native environment execution is currently declared for Linux. macOS and
Windows runtime paths are unverified and not offered. Plugins run with their
host account's authority, or their own service account when attached; Orch
does not sandbox them. See [extensions](../development/extensions/plugins.md)
for their local transport and checks.

Relevant regression coverage includes [machine reports](../../crates/control/engine/tests/),
[proxy addresses](../../crates/platform/config/src/client_address.rs),
[outbound destinations](../../crates/contracts/notifier/src/outbound.rs),
[workspace containment](../../crates/runtime/envhost/tests/),
[plugin checks](../../crates/platform/conformance/tests/) and
[deployment inputs](../../deploy/README.md). A test's existence is not a
passing release record; the release runner retains the actual results.
