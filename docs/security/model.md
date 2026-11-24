# What Orch guarantees and where it stops

Orch decides access at the server and enforces a machine's own policy at the connector. This page says what each control guarantees and where its limit is, for people who decide whether to trust Orch and for administrators who set it up. What an operator must do to keep these guarantees is in [Secure an installation](../operator/security.md).

The tests listed [at the end](#where-the-controls-are-tested) exercise these controls. Each release keeps the results of its own checks, on each platform and in the browser.

## Who may do what

The server checks each action against the permission page of the thing it touches, the defaults that page follows, the scope of the key and suspension.

- **Where a resource is used.** Every resource has a home, an organization at any level. It is used wherever its page gives access, and it says where it comes from wherever it is offered.
- **Naming people from outside.** Somebody from outside the root organization can be named on the page of a thread, topic, project, document or resource only by whoever holds the row for that there (as shipped, its Developers and admins), or by an owner of the thing where a policy lets owners (as shipped). An organization that wants neither empties the row, locks it and turns the policy off. Only an admin names the holders of that row, and no default names anybody from outside.
- **Owners.** Who owns a thing is a row of its page. Owners hold every permission of what they own except naming people from outside, which the policy above gives them. They name each other, and the last owner cannot leave. Whoever makes a resource owns it, privately until shared.
- **A person's own organization** takes no other member and holds no organization inside it when it is made. Only a system administrator lifts either limit.
- **Folders.** A project can contain projects. The interface calls a nested project a folder. An inherited role in a place reaches its folders and their contents.
- **Sharing.** Sharing one thread or document reaches that thing alone. Sharing a place offers two reaches: the place itself, or the place and its contents. Every grant and agent share names its giver. Holders share only what they hold, where the place permits it. Managing the permission page is a separate right.
- **Archiving.** Archiving a project, topic or thread, and bringing it back, is a row of its page, held by whoever manages it and by whoever the row names.
- **The record.** Every act that needed a permission and changed something is audited in the same transaction, and that row is the line in the history of the thing it was done to.

## Admins and what they can override

- **Downward only.** Admin is inherited downward, from the system to its organizations and the organizations inside them. A role held in a place reaches that place and what lies inside it, and nothing above. A member of a department holds nothing in the organization over it by being one.
- **What an admin holds.** An admin holds what the pages of their place give Admins, and may change every page in it.
- **Forcing an act.** Where a page leaves an admin out, they can force an act. Orch confirms it with a line that says so, puts them on the page for what the act needs, and marks the audit row of that change as an override. The page shows them from then on, and nobody is notified.
- **What no admin passes.** A row locked from above, a subscription kept to its owner and a machine's local policy are closed to everybody.
- **Private pages.** A system administrator uses the inherited private-access override through the browser's **As admin** mode. Automation keys can perform selected administrative operations with the account's current standing. They keep their operation and target bounds, and they do not receive the private-access override.
- **Suspending a person** ends their sessions and tokens at once. Suspending a person in one organization leaves them nothing there. Neither passes their things on.
- **Moving a member** to another organization of the tree changes their domain and the role they hold there, and nothing they own. Whoever moves them needs to manage members where they are and where they go, and is shown what the member stops and starts reaching before it happens.
- **When a member leaves.** Nothing is left without an owner: what they owned alone passes to the first owner of the organization it lies in.

## What a thread can do, and whose access it uses

- **Separate permissions.** Watching a thread, driving its agent and setting its session up are three permissions. Driving is sending, steering and stopping. An input always needs its sender's permission to drive the thread.
- **A sender who may not use the model.** Where the sender may not use the provider, model and effort, the place can hold the input for approval, refuse it, or run it on the first owner's access. A held input runs only after somebody who may drive the thread and use that model chooses **Run it**. Their current access, usage and caps apply, and the author stays named.
- **Plans and approvals.** Changing the model or its parameters, and approving a tool request, take managing the thread. Whoever may drive the thread can answer a proposed plan, unless the place restricts plan replies to its owner and managers. **Proceed** keeps the thread's approval mode.
- **A thread holds no access.** A thread holds nothing by itself and remembers nobody whose access it would use. Whoever puts it on a provider or into an environment is checked for that at that moment. Each input is checked against its author. An input that no person sent is asked of the thread's first owner.
- **Switches and forks.** A switch changes what a thread runs on and nothing about whose it is, and asks only for what it changes. A fork is its maker's, and starts with the page of the thread it was taken from.
- **Switches, moves and updates** retire the old side only in the transaction that commits the new one, after its health checks. Anything earlier is undone. None of them changes who owns the thread or the environment, or what its page says. Nothing an environment runs is replaced except by such an update.

## Limits on turns, tokens and credits

How many turns run at once is bounded by a cap, as tokens and credits are, and no role is exempt from a cap by being that role.

- A cap that is reached is passed in one way, which leaves the cap as it is. Whoever holds the row for it, where the cap is defined, lets one turn run, after reading what is used of how much.
- The turn is counted. Each pass is a row beside the cap, an audit entry and a line in the conversation.
- It is always somebody's act, under their name. An input that nobody sent is never let past a limit, and an agent cannot ask for it.

## Environments

- **The environment is the boundary.** Not a thread's directory. Reading files or opening a terminal through one thread reaches the whole environment, and every page that gives either says so. Threads in it can also share the home directory of the environment's account, the process table, listening programs, browser profiles and displays. A browser's stored sign-in is accessible to agents running in that environment even though its profile is outside file previews and snapshots. Work with different trust requirements needs separate environments.
- **View or manage, nothing between.** An agent that runs in an environment reaches all of it. Putting a thread into one, giving it another directory there and taking it out take managing the environment, and managing a thread gives none of that. An environment on a person's machine starts as its owner's alone and is shared like any resource.
- **Confinement.** The selected driver and the environment's capabilities determine confinement. A capability is decided on the side of the host it widens access to.
- **Root in the shipped image.** In an environment made from the shipped image the agent's account can become root with `sudo` and no password, so that whoever works there installs what the work needs. That is root inside the environment's container and nowhere else, and the container is the boundary for everything an agent runs there. A template that sets `no_new_privileges` takes it away. The browser there runs without its own sandbox where the container cannot give one, and the environment says so.
- **Restricted networks.** An environment that needs a restricted network starts only after its driver has verified the transport paths to the gateway, the models and the blobs, and the permitted destinations. A network with no additional destinations uses the same mechanism, and does not mean an unrestricted host bridge. If the driver cannot enforce a restriction, creating the environment is refused. It never falls back to an open network.
- **Doors.** An environment owns its doors: published HTTP, TCP and UDP ports, managed browsers and displays. A thread is one route to them. WebSocket tickets are short-lived, scoped and consumed once. Losing access closes a viewer or driver.
- **Clipboard.** A shared display may carry clipboard bytes to its watchers, so clipboard use has the same audience as that display. Its round trip is part of the release's display checks.
- **Another node's settings.** An environment takes another node's settings and secrets only where its manager may create environments on that node.

## Machines

- **A machine's own policy.** Connected machines enforce locally owned policy that the server cannot widen, for administrators too, and it comes before every page. The application shows it and the machine changes it. A key its owner handed over is changed from the application by whoever manages what that key bounds.
- **Reports.** Machine reports are accepted only from the machine that owns their stream. Reports from another machine are acknowledged and discarded without changing content or a cursor, counted, and recorded for administrators at a bounded rate. A session's segment keeps the identity of its agent program's machine, so late reports cannot acquire the authority of its replacement.
- **Where a caller comes from.** The connection's peer address is authoritative unless it is a configured trusted proxy. Forwarded addresses are then read from the right, through that trusted chain. The same derived address feeds sign-in limits, audit origin and the caller information of HTTP requests. The application trusts no proxy unless the deployment names its edge network.

## Providers and credentials

- **Custody.** Providers retain credential custody and renewal when shared. A handover is exclusive to one environment and must be returned before it stops.
- **Where a provider may be used.** In which kinds of arrangement a provider may be used, and so what of it may ever be in an environment, is its owner's consent. The owner gives it, and sees it, before the provider exists. As shipped, a key never leaves its provider.
- **Sign-ins and tokens.** A sign-in or a token pasted into the browser is sealed at once and kept only in the provider's own container. That container is placed on a host of the system or on one its owner manages, since whoever manages a host can read what is on it.
- **What is in that container.** Nothing of anybody's work is in it, and nothing of its host is attached to it. Its sign-in is on a volume of its own, made with the provider and kept through every stop, recreate and update. A sign-in you already have is copied in, as text or as the path of a file inside the container. Nothing watches a file, and nothing is written back to one.
- **Subscriptions.** A system or an organization can refuse an agent's subscriptions by policy, or accept them and keep each to the person who owns it. The dialog that shares a subscription says whose plan the work will run on.
- **Endpoints.** An endpoint without TLS is refused unless a policy allows it. Provider endpoints have their own TLS policy and are not promised the private-address check that notification connections have.
- **Deleting a provider** removes its container and the volume its sign-in was kept in.
- **Who writes what runs beside a credential.** Agent profiles are written by an organization's administrators, because a profile's configuration runs beside a credential. A template a member made runs only where a host's page allows, and its risky members stay with system administrators. An image built from a Dockerfile is built without privileged features or secrets, and never runs its own entrypoint.
- **Masking.** Stored masking replaces the exact secrets Orch knows at that place. It cannot discover an arbitrary password typed into a shell or pasted into a document.

## What agents can reach

- **Chats.** Chats reach an agent only through a deliberate forward or a live share. A share that lets an agent write in a chat reaches no other agent, and one that lets it send to a thread opens no chat.
- **Documents.** Public documents beside a topic and that topic's main threads have their own policy. As shipped, agents may read, edit and create them, under their own names. A place may narrow that policy to reading, or to no implicit document reach. Private documents and restricted places stay outside it.
- **Further access.** Chats and other conversations are reached through their own policies and through shares. Whoever may message an agent can learn what it can read.
- **Another thread.** What an agent reads of another thread, where it may, is what that thread's people see of it, and never its reasoning.
- **Later threads.** What a place was shared with reaches the agents started there later. What was made private inside what was shared is left out.
- **Search.** An agent's search looks only at what that agent may read, within a bound in size and in time.
- **Orch's tools.** Orch checks every call of its own tools itself, whatever an agent program makes of a tool's description. Orch's tools are answered by the server and are no part of any executor, so no executor can see the calls made to them.
- **Other tools.** An agent that works with tools other than its program's own says so on its thread, with which tools.
- **Chat inspection.** A row of an organization's page lets its holders (its admins as shipped) inspect the direct and group chats among its people. Inspecting is audited and announced to nobody.
- **Project destinations.** A notification destination of a project is added by whoever makes rules there. It is sent nothing from private places below it, and nothing people say in chats.
- **`AGENTS.md`.** `AGENTS.md` in a thread's directory is a file in the workspace. Whoever may write there, an agent included, can change it, and it is on screen where the thread is set up. What must hold whatever the directory says is the thread's standing instructions, a setting that no agent writes.
- **What an agent notes for itself** is files in its thread's directory, read and corrected by whoever may read and write that thread's files. An agent program's own memory is not left where threads that share nothing else would share it: a memory that a program keeps for its whole home is switched off, since that home serves every thread of a provider.
- **MCP servers.** An MCP server that an agent program already has configured on a machine is used only once somebody who manages the environment there has imported it. Its definition, with whatever tokens it holds, stays on that machine, and it runs there as the machine's account, which the dialog that imports one says. An agent program is held to the session's MCP servers only when it declares strict MCP control and Orch has checked that it keeps to it. One that cannot do so reports that limit. Importing a server gives the program the permissions of the machine's account.
- **Skills.** A skill is executable content supplied by its managers. Publishing or enabling one is a trust decision for the threads that receive it. The check of an archive confines its files. It does not establish that its instructions or scripts are safe. Importing from an address or repository lets the chosen host fetch that source with the host's network reach, under its local fetching policy. A source address shown with `?…` keeps its private part with the skill, and never returns it, not even to its managers.
- **Agent programs' own reports.** Agent programs are started with telemetry, error reports and self-update off.

## Documents

- **Blocks, not markup.** A document is typed blocks and never markup.
- **Two people, one block.** Two people who change one block do not overwrite each other. The change that comes second is refused and kept on screen beside the first.
- **What an agent changed** is in the document's history, as anybody's writing is. The entry names it among its authors, each changed block says who changed it, and whoever manages the document can put an earlier version back.
- **Who owns an agent's document.** A document that an agent makes belongs to the first owner of its thread.
- **Comments.** An agent writes no comment. It reads the discussions beside a document wherever it reads the document in full.
- **Video and audio.** A video or an audio clip given as an address is fetched by the reader's browser only once they chose to play it.
- **PDF.** A PDF is typeset in the reader's browser from source the server wrote. No service compiles a document, and the page fetches the compiler from the system itself.

## Shells and files

- **A shell is its opener's alone.** Nobody else attaches to it or reads what is on it, whoever they are. Whoever manages the environment can close it.
- **What is kept.** That a shell was opened, by whom and for how long, is a line in its environment's history. As shipped, the last of what it printed is kept with that line when it closes, masked like every stored text. Its opener, whoever manages the environment and whoever reads the audit trail there can read it. A read by another person is itself recorded.
- **What a place keeps** is a setting of the place, said where it is set and read in the history. A shell does not say so while it runs. A place can deliberately retain a terminal's final output without masking, and people who can read that retained record can then read that text.
- **Its strength.** A shell is as strong as the environment: it reads what every agent's processes there can. It ends when its opener loses the access that opening it took, and after a time with nobody attached.
- **File viewer.** A file viewer never runs what it shows. HTML is shown as code, an SVG is drawn as an image, and an archive is listed and never unpacked in the browser.
- **Shared files.** Threads that share an environment share its files. A private view of a mount never writes to the data underneath, and a workspace is never such a view.
- **Mounts.** Pointing a mount at a directory of a host is an act of that host's managers. On a person's machine it is the owner's, inside the roots that machine allows.

## Signing in

- **Wrong tries.** They are counted by the name that was tried, not by the address they came from, and for each kind of proof apart: a password, a mailed code or an authenticator's code. As shipped, eight wrong tries of one kind in three hours close that kind for that name, from every address alike. A name nobody has is counted like one somebody has.
- **A known browser.** A browser its person signed in from is counted by itself. Guessing at a name closes it for strangers and not for its person, who is told.
- **A code by mail** is a way in, or a second step, only where every organization a person belongs to allows it. It is as safe as the mailbox it goes to, which the page that switches it on says.
- **An address** is changed with the password and counts once it is confirmed.
- **What an answer tells.** Every answer about signing in is the same whether a name exists or not.

## Notifications and invitations

- **Nothing leaves without a route.** Nothing leaves the application for a person until they switch a route on.
- **Channels.** A channel is set up by the system, an organization or a person. An address it reaches is checked against private networks unless a system administrator listed it. Its secrets are sealed, and a text it sends holds placeholders from a closed list and no logic.
- **Outbound addresses.** Notification connections check the address they will connect to, and every redirect, against the shared special-purpose address policy. Loopback, private, shared, documentation, reserved and multicast ranges are not public destinations, and embedded IPv4 addresses are checked as IPv4. Exceptions for private hosts belong to the system's outbound policy.
- **Invitations.** An invitation is a link to share, bounded by a number of people and by time, or a personal link, good once and for its address alone.

## What leaves your installation

Agent programs are started with telemetry, error reports and self-update off. Orch keeps its metrics and logs locally. Model requests, notification deliveries you switched on and browser navigation reach the endpoints chosen for that work. Operators enforce further network limits through a verified driver policy. The usage ledger and the audit trail are kept for good. [Running without internet access](../operator/offline.md) lists every outgoing connection.

## Plugins and platforms

- **Plugins** run with their host account's authority, or with their own service account when attached. Orch does not sandbox them. See [plugins](../development/extensions/plugins.md) for their local transport and checks.
- **Platforms.** Environments that run directly on a machine are available on Linux. The macOS and Windows paths are not tested and are not offered.

## Where the controls are tested

These tests cover [machine reports](../../crates/control/engine/tests/), [proxy addresses](../../crates/platform/config/src/client_address.rs), [outbound destinations](../../crates/contracts/notifier/src/outbound.rs), [environment containment](../../crates/runtime/envhost/tests/), [plugin checks](../../crates/platform/conformance/tests/) and [deployment inputs](../../deploy/README.md). That a test exists does not say it passed. Each release keeps the results.
