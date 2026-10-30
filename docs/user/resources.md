---
title: Resources and movement
screens: /resources, /resources/agents/$id, /resources/machines/$id, /resources/providers/$id, /resources/environments/$id, /thread/$threadId
keywords: environment, provider, home, browser, display, port, files, terminal, folders, fork, move, connect a machine, install, service, foreground, data directory, remove, multiple connectors
---

# Resources and movement

An environment is the place where work runs. Threads in one environment share
its workspace, home, processes, browser profiles and displays. Opening Files
or a terminal from a thread reaches that whole environment; the thread's
directory is where its agent starts. Use separate environments for work that
must not share those things.

Ports, browsers and displays belong to the environment. A thread's views are
ways to reach them, subject to the permissions shown there. A browser profile
can retain a sign-in: other agents in the same environment may read it even
when it is outside the workspace's file list. Clipboard content on a shared
display can reach its watchers.

Projects contain topics and further projects, called folders in the tree.
A topic can move between them. A role in a place follows its folders and
contents; sharing one document or thread does not share its folder. A place's
sharing dialog distinguishes the place itself from the place with its contents.

## Delete an environment

Open the environment's menu and choose **Delete**. Its confirmation says what
will go before asking for its name. For a unit made from a template, acceptance
queues the removal job immediately. Orch stops and removes the unit, then keeps
its owned volumes for the retention window frozen when you confirmed. That
window starts when the unit is actually removed, so an offline host cannot use
up the recovery time. Existing directories and borrowed mounts are kept.

A registered directory loses its registration; its files stay on its machine.
A temporary unit has no ordinary volume retention window; read its explicit
owned-volume warning before confirming. Read
[Temporary environments](temporary-environments.md) for automatic cleanup,
the one-hour notice, Keep and export.

## Providers and models

A provider supplies model access through an API key or a harness's own sign-in.
It keeps its credential in its provider runtime or on its connected computer.
Its page distinguishes saved setup from a ready provider, shows the models it
may serve, and reports only the plan windows, credits and lapse the vendor
actually provides. Unknown credit or expiry is shown as unknown.

**Refresh** reads the provider's current facts and available models. A newly
offered model appears as a suggestion for its manager to enable; a gone offer
is marked. Each fact says when it was read. Orch also reads a minute after each
reset time the vendor reports, so a new window appears without a manual click.
For example, a reported 14:00 reset schedules a read at 14:01. A failed read
keeps its error and time visible; an unknown balance or read time stays unknown.
There is no separate Test, and Refresh enables no model by itself.

Model managers open available speeds, access programs and context. A thread
then chooses from those offers; a model entry's allowed maximum is separate
from the thread's current setting. The routing name reaches the provider and
the display name reaches people. A changed name or disabled model is checked
again when the next turn starts.
The provider's model editor names the known access programs **Daybreak Blue**,
**Daybreak Red** and **Standard**. Other program names appear as supplied.

## How agents run

The agent's setup explains where it runs, who runs its tools and where its sign-in stays.
**Run with provider** keeps the agent with the provider; Orch runs its tools in
the environment. **Forward model calls** runs the agent in the environment and
sends its model requests through Orch. **Copy sign-in access** lends a permitted
token copy; **Move sign-in** lends the complete sign-in exclusively until it is
returned. **Use local sign-in** uses what is already in the environment.
The available choices follow the provider owner's consent,
policy, the adapter's declared abilities and current machine/host facts.

Providers own their sign-in and renewal. A managed provider runtime retains its
home through idle stops and recreations and is started for needed renewal.
An imported credential is copied here; a refreshed external token can be
imported again by its manager or the provider import command. If a login copy
is also renewed elsewhere, follow the sign-in method's guidance about a single
renewing copy. A failed exclusive return prevents the environment from stopping
until its manager resolves or explicitly declares the loss.

## Profiles, hosts and computers

A profile configures an agent: its version, options, variables and supported
setups. Its declared fields provide the form, choices and help. Profiles do
not own another runtime. Configuration trust controls whether project settings,
hooks, plugins or servers can be loaded; instruction files do not grant that
trust by themselves.

The profile's technical details name executors and tool sets from their declarations.
Opening those details reads any executor names the profile does not already supply.
An unsuccessful read has **Try again**; an unreported name stays unavailable.
Long arrangement and tool-set lists show one page at a time.
When a confirmed program can run in the selected environment, its local arrangements are tried before
separated tools. The provider's allowed arrangements and any place preference still apply.
A failed start distinguishes an unreachable tool environment from an agent-program error.

A host makes managed runtimes from templates; the chooser shows what is free
and what the template needs. A connected computer supplies the roles and roots
its owner configured. It begins as that owner's resource and may be shared.
Its local policy is changed on the computer and cannot be widened by a server
administrator. Enroll again renews its enrollment while keeping its registered
identity when that action is offered. Native environment operation is currently
declared for Linux; another platform's unverified capability is not offered.

On the computer's **Policy** tab, executor offers come from the executors and
tool sets it reports. Their names and descriptions come from those declarations.
Opening that editor reads the machine's offers; a failed read has a retry.
A saved offer that is no longer reported can be kept or cleared. Older saved
aliases stay selected under their declared offer names. Saving changes only
keys that the computer's owner has handed over.

## Enroll by role

Open **Add machine** and choose what this computer will do. Environment, Agent and Provider can
be combined; Node is a separate choice with its own program and enrollment. Choose **User service**
or **Foreground**, and give another instance a name if this computer already has a connector.
For Node, the background choice is **System service**. Use **Show command**, then run the complete
command on that computer; the program is checked against the system's release before it is installed.

A machine's home defaults to your own organization. You can choose another organization where
you may make machines. The enrollment token keeps that home, and the machine starts private to
you: belonging to the same organization alone does not give another member access. Choosing a
different home makes a new command; use the command currently shown.

### Environment role

**Environment** — let people work in folders of this machine. Its environment settings bound file
writes, terminals, browsers, displays, tools and port audiences in those directories.

### Agent role

**Agent** — run agents here. It has its own harness and build-fetch settings.

### Provider role

**Provider** — keep provider keys here. Its sign-in, upstream and arrangement settings are separate
from the environment and agent settings.

For each selected connector role, **Hand over its settings to Orch** starts off. Turning it on hands
that role's policy section over; locally kept values still win. The separate **Dedicated** switch
declares a VM or computer that holds nothing else. That is local consent, never a policy permission
Orch can turn on later.

The command changes with every choice. On Linux or macOS it can include
`--roles environment,agent --delegate environment,agent --dedicated`; Windows uses
`-Roles`, `-Delegate` and `-Dedicated`. `--delegate-policy` / `-DelegatePolicy` hands every policy key
over instead. Use the complete command: its one-use token is bound to those choices.
The connector saves roles and role sections (`environment.*`, `agent.*`, `provider.*`) only after
trusted enrollment. An older token without role choices keeps the previous default behavior.

### Where a connector runs

A connector runs as **your account**, without sudo or administrator rights, and works in the
folders you register; it is independent of the terminal's current folder. On Linux and macOS,
the program is `~/.local/bin/orch-connector`; data is `~/.local/share/orch-connector` on Linux
(or under `XDG_DATA_HOME`) and `~/Library/Application Support/Orch/connector` on macOS. On Windows,
the program is `%LOCALAPPDATA%\Orch\orch-connector.exe` and data is `%LOCALAPPDATA%\Orch\connector`.

**User service** means a systemd user unit on Linux, a launchd agent on macOS or a logon task on
Windows. Linux stops at logout unless an operator enables lingering; the macOS agent and Windows
task run while you are logged in. Stop or start with `orch-connector service stop` /
`orch-connector service start`; on Windows, invoke `& "$env:LOCALAPPDATA\Orch\orch-connector.exe"`
before `service stop` or `service start`. **Foreground** adds `--no-service` (`-NoService` on Windows):
the dialog's complete command installs and starts the foreground program. If running the installer directly,
after installation, run the command the installer prints, ending in `run`. It stops with Ctrl+C.
Stop any existing service first; selecting foreground does not remove it.

Several connectors can run side by side. **Another instance, named** chooses a separate data
directory, identity and service. For a manually edited command, use `--data-dir <absolute path>`
(`-DataDir` in the Windows installer). Use the same `--data-dir` before later program subcommands
for that instance, including `service stop` or `service uninstall`; omit it for an installation
made without it. The program file is shared, so its other running instances use an update after
they restart. A node may run beside a connector, but cannot be multiplied this way.

### Node role

**Node** — let Orch make environments here. A node runs as its own program, `orch-node`; to have both
on one computer, add it as a host and as a machine. Choose its organization, then run the host's
command on the Linux host. Its handover switch adds the node installer's `--delegate-policy` flag.
The connector refuses the Node role. A node also keeps environment settings as the ceilings of its
units; its driver, storage, hook and resource settings bound what templates may request.

The node runs as **root**, with `/usr/local/bin/orch-node`, configuration `/etc/orch/orch.toml`,
and data `/var/lib/orch-node`. **System service** keeps it running as `orch-node.service`;
stop or start with `systemctl stop orch-node.service` / `systemctl start orch-node.service`.
**Foreground** adds `--no-service`; after installation run
`/usr/local/bin/orch-node --config /etc/orch/orch.toml run` as root, stopping with Ctrl+C.
The dialog includes that foreground start in its complete command.
Stop an existing service first. One node runs per machine: share the host through **Access**
to serve other organizations, rather than installing a second node.

### Remove a connector or node

For a connector, revoke its machine in Orch, then run `orch-connector service uninstall`,
including the same `--data-dir` selection for a named instance. This stops and removes the
service while keeping data and identity. Inspect its data before deleting it; keep other
instances, including any nested `instances/` directories, and registered work folders.
Remove the shared program only after the last connector is gone.

For a node, first remove its environments and volumes and then the host in Orch. As root,
run `systemctl disable --now orch-node.service`, remove `/etc/systemd/system/orch-node.service`,
then run `systemctl daemon-reload`. Inspect and remove this node's program and data; keep
`/etc/orch/orch.toml` if another Orch program uses it. A foreground installation has no service
to remove. Removing only a service keeps enrollment and stored data; it does not remove work.

### Change roles locally

The machine page shows `orch-connector roles set environment,agent,provider`. Edit the list and run
it on that computer, then restart the connector. Its authenticated reconnect reports the selected
roles. The page has no remote role-edit action. To add Node, perform the separate host enrollment;
`roles set node` explains which program to use.

## Environments and their doors

An environment manager controls placing a thread, changing its directory or
confinement and removing it. Managing a thread does not confer that authority.
An environment can have several thread views at once; a thread's permissions
are still checked for each route to the shared environment.
The environment page names the driver and runtime together when both are known.

Ports publishes a listening program through HTTP, raw TCP or UDP according to
the offered plan. The environment owns the endpoint and its audience. Read the
caller/recipient disclosure, expiry and outside-sharing choice before opening
one. A link is a separate way in; the page shows its kind and intended recipient.
Never assume a published port belongs privately to the thread that opened it.

A browser uses a kept profile; joining the same profile shares its tabs and
sign-ins. Watch and control are separate permissions. A display can also carry
clipboard data to watchers. The view shows current capabilities and limitations,
including whether a browser sandbox is available. Closing an owned browser or
display ends its owned process; an announced existing one has its own lifecycle.

## Mounts and tool servers

A mount attaches data in the mode shown: shared writes reach the source,
read-only permits no writes, and a private view keeps this environment's changes
apart from the source. Its location and mode are shown in a fork or move plan;
a detached entry explains why it cannot follow. A private workspace is not
created by choosing a subdirectory in a shared environment.

MCP presets run in environments that enable them. A server found in a harness's
own computer configuration is imported deliberately, with its definition and
secrets kept on that computer. Enable the imported server for the environment
that should use it. The adapter reports whether it can restrict a session to
its given servers; an unavailable strict boundary is visible.

Each listed server has a current state. On failure, read its exit and masked
last output, then use **Restart** with Environment Manage to start that server
alone. For a whole environment, **Restart…** first names what will stop and
keeps every file in the same unit. Both operations show job progress and their
actual result. [The examples and waiting rule](change-a-running-environment.md#restart-one-tool-server)
explain when to choose each.

## Templates and updates

A template proposes a runtime through a driver-specific form. Some drivers
can import an image, Dockerfile or archive; importing proposes the form and
does not create a runtime. Confinement, resource limits, network restriction,
mounts and private views depend on verified driver capabilities. An unsupported
restricted network refuses the plan instead of starting open.

Update or recreate acts on the environment or provider runtime whose page you
manage. Read its plan and effects on running threads; progress follows the
durable job. The old side is retired only after the new side is checked at the
commit point. Cancellation before that point keeps the source authoritative;
the job explains recovery when a phase cannot be undone.

## Fork and move plans

Fork makes a new thread; switch preserves the current thread. The plan names
history fidelity, destination choices, file selections, home copy and mounts,
including why an entry is detached. No history is an explicit loss of all
source conversation. For a live large tree, choose exactly the files to copy
and read their estimated bytes/time; Git state is refreshed only when requested.

Requested home and private-layer transfers need their reviewed transfer path.
If that path is unavailable, the plan blocks the requested copy instead of
silently leaving those changes behind or copying the mount's underlying data.

Each resource has owners and a home, shown with its containing place. It is used
where its permission page grants access rather than only where it was created.
Deleting or moving it requires Manage and does not borrow another person's
authority from a thread that happens to use it. Read
[sharing](workflows.md#share-a-thing-or-a-place) before granting access and
[settings and usage](settings-and-usage.md#limits) for limits on its work.
