---
group: resources
title: Resources and movement
screens: /resources, /resources/agents/$id, /resources/machines/$id, /resources/providers/$id, /resources/environments/$id, /thread/$threadId
keywords: environment, provider, home, browser, display, port, files, terminal, folders, fork, move, connect a machine, install, service, foreground, data directory, remove, multiple connectors
summary: Understand environments, providers, agent profiles and machines before you change where work runs.
---

# Resources and movement

An environment is where work runs. Threads in one environment share its workspace, its home, its processes, its browser profiles and its displays. Opening **Files** or a terminal from a thread reaches the whole environment. The directory of a thread is only where its agent starts. Use separate environments for work that must not share those things.

Ports, browsers and displays belong to the environment. The views of a thread are ways to reach them, with the permissions the thread shows. A browser profile can keep a sign-in, so other agents in the same environment may read it even though it is not in the list of files of the workspace. What is in the clipboard of a shared display can reach everyone who watches it.

Projects hold topics and further projects, which the tree calls folders. A topic can move between them. A role in a place reaches its folders and their contents. Sharing one document or thread does not share its folder. The sharing dialog of a place tells the place itself from the place with its contents.

## Delete an environment

Open the menu of the environment and choose **Delete**. The confirmation says what will go before it asks for the name.

- For a container made from a template, confirming queues the removal at once. Orch stops and removes the container, then keeps the volumes it owns for the retention window that was fixed when you confirmed. The window starts when the container is really removed, so a host that is offline cannot use up the time for recovery. Directories that already existed and mounts that were borrowed are kept.
- A directory you registered loses its registration. Its files stay on its machine.
- A temporary environment has no retention window for its volumes. Read the warning about the volumes it owns before you confirm. [Temporary environments](temporary-environments.md) explains the automatic cleanup, the notice one hour before, **Keep** and export.

## Providers and models

A provider supplies model access through an API key or through the sign-in of an agent program. It keeps its credential in its own container, or on the connected machine. Its page tells a saved setup from a provider that is ready, shows the models it may serve, and reports only the plan windows, credits and expiry that the vendor really gives. A credit or an expiry that is unknown is shown as unknown.

**Refresh** reads the current details of the provider and its available models. A model that is newly offered appears as a suggestion for a manager to turn on, and one that is gone is marked. Each detail says when it was read. Orch also reads a minute after each reset time that the vendor reports, so a new window shows without a click. For example, a reset reported for 14:00 schedules a read at 14:01. A read that failed keeps its error and its time visible, and a balance or a read time that is unknown stays unknown. There is no separate **Test**, and **Refresh** never turns on a model by itself.

Managers of models open the speeds, access programs and context sizes that are available. A thread then chooses among those offers. The maximum that an entry of a model allows is separate from the setting of the thread now. The routing name reaches the provider and the display name reaches people. A name that changed, or a model that was turned off, is checked again when the next turn starts. The model editor of the provider names the known access programs **Daybreak Blue**, **Daybreak Red** and **Standard**. Other program names appear as they were supplied.

## How agents run

The setup of an agent says where it runs, who runs its tools and where its sign-in stays.

- **Run with provider** keeps the agent with the provider, and Orch runs its tools in the environment.
- **Forward model calls** runs the agent in the environment and sends its model requests through Orch.
- **Copy sign-in access** lends a copy of a token that is allowed.
- **Move sign-in** lends the whole sign-in to one place until it is returned.
- **Use local sign-in** uses what is already in the environment.

What is offered follows the consent of the owner of the provider, the policy, what the agent program declares it can do and the current facts of the machine or host.

Providers own their sign-in and its renewal. A managed provider keeps its home through idle stops and recreations, and is started when a renewal is needed. A credential that was imported is copied here. A token from outside that was refreshed can be imported again by its manager, or with the import command of the provider. If a copy of a sign-in is also renewed somewhere else, follow the guidance of the sign-in method about keeping one renewing copy. A return that failed on a sign-in that was moved prevents the environment from stopping until its manager settles it, or declares the loss.

## Profiles, hosts and computers

An agent profile configures an agent: its version, options, variables and the setups it supports. Its declared fields give the form, the choices and the help. A profile does not own another container. **Configuration trust** controls whether project settings, hooks, plugins or servers can be loaded. Instruction files do not give that trust on their own.

- **Profile** lists the saved **Configuration keys** without their values. **Show all** opens a long list, and nested names stay distinct from a key that has a dot in its name.
- The JSON editor keeps the options that the program does not show as ordinary controls. Declared model parameters still obey the choices that the provider has turned on and the limits of the program when a turn is prepared, also on the sign-in of a program. A profile cannot bypass them.
- The technical details of a profile name executors and tool sets from their declarations. Opening them reads the executor names the profile does not already give. A read that failed has **Try again**, and a name not reported stays unavailable. Long lists of arrangements and tool sets show one page at a time.

A host makes managed environments from templates, and the chooser shows what is free and what the template needs. A connected machine supplies the roles and the folders its owner set up. It begins as a resource of its owner, and it can be shared. Its local policy is changed on the machine, and an administrator of the server cannot widen it. **Enroll again** renews its enrollment and keeps its registered identity, when that action is offered. Environments that run directly on a machine are available on Linux, and the capability of another platform that is not tested is not offered.

On the **Policy** tab of a machine, the offers of executors come from the executors and tool sets it reports, with their names and descriptions. Opening the editor reads the offers of the machine, and a failed read has a retry. A saved offer that is no longer reported can be kept or cleared. Older saved aliases stay chosen under the names of their declared offers. Saving changes only the keys that the owner of the machine has handed over.

## Enroll by role

Connecting a machine, with its roles, its account and how it runs, is in [Connect a machine](connect-a-machine.md). The roles in short:

- **Environment**: let people work in folders of this machine. Its environment settings bound file writes, terminals, browsers, displays, tools and who a port is open to in those folders.
- **Agent**: run agents here, with its own settings for agent programs and for fetching builds.
- **Provider**: keep the keys of providers here. Its sign-in, upstream and arrangement settings are separate from the environment and agent settings.
- **Node**: let Orch make environments here. A node is its own program, `orch-node`, on a Linux host. To have both a connector and a node on one computer, add it as a host and as a machine. A node keeps environment settings as the ceilings of its containers: its driver, storage, hook and resource settings bound what templates may ask for.

The home of a machine defaults to your own organization, and you can choose another organization where you may make machines. The enrollment token keeps that home, and the machine starts private to you: being in the same organization does not give another member access.

For each role you choose, **Hand over its settings to Orch** starts off. Turning it on hands that role's policy over to Orch, and values kept locally still win. The separate **Dedicated** switch declares a virtual machine or computer that holds nothing else. That is local consent, never a permission that Orch can turn on later. The command changes with every choice, so use the complete command that is shown: its token can be used once and is bound to those choices.

To change the roles later, the page of the machine shows the command `orch-connector roles set environment,agent,provider`. Edit the list, run it on that computer and restart the connector. Its reconnect reports the roles chosen. The page has no action to edit roles remotely. To add **Node**, enroll the host separately.

## Environments and their ways in

A manager of the environment controls placing a thread, changing its directory or its confinement, and removing it. Managing a thread does not give that. An environment can have several views of threads open at once, and the permissions of a thread are checked for each route to the shared environment. The page of the environment names the driver and the runtime together when both are known.

**Ports** publishes a program that listens, through HTTP, raw TCP or UDP, according to the plan offered. The environment owns the endpoint and its audience. Read what is told to callers and recipients, the expiry and the choice about sharing outside before you open one. A link is a separate way in, and the page shows its kind and its recipient. Never assume that a port that was published belongs privately to the thread that opened it.

A browser uses a kept profile, and joining the same profile shares its tabs and its sign-ins. Watching and controlling are separate permissions. A display can also carry clipboard data to those who watch it. The view shows the current capabilities and limits, including whether a browser sandbox is available. Closing a browser or a display that Orch owns ends its process. One that was announced and already existed has its own lifecycle.

## Mounts and tool servers

A mount attaches data in the mode shown. A shared mount sends writes to the source, a read-only one allows no writes, and a private view keeps the changes of this environment apart from the source. A fork or a move plan shows the location and mode of a mount, and a mount that is detached says why it cannot follow. A private workspace is not made by choosing a subdirectory in a shared environment.

MCP presets run in the environments that turn them on. A server found in the own configuration of an agent program on a computer is imported on purpose, and its definition and its secrets stay on that computer. Turn the imported server on for the environment that should use it. The agent program reports whether it can limit a session to the servers it was given. A strict boundary that is not available is visible.

Each server in the list has a current state. When one fails, read its exit and its last output, which is masked, then use **Restart** with Environment Manage to start that server alone. For a whole environment, **Restart…** first names what will stop, and keeps every file in the same container. Both show the progress of the job and the real result. [The examples and the rule for waiting](change-a-running-environment.md#restart-one-tool-server) explain when to choose each.

## Templates and updates

A template proposes a runtime through a form that depends on the driver. Some drivers can import an image, a Dockerfile or an archive. Importing proposes the form and does not create a runtime. Confinement, limits on resources, restriction of the network, mounts and private views depend on the capabilities of the driver that were verified. A restricted network that is not supported refuses the plan; it never starts open.

**Update** or **Recreate** acts on the environment or the managed provider whose page you manage. Read the plan and its effects on running threads. The progress follows the job, which is kept. The old side is retired only after the new side was checked at the commit point. Cancelling before that point keeps the source in charge, and the job explains how to recover when a phase cannot be undone.

## Fork and move plans

**Fork** makes a new thread, and **Switch** keeps the current one. The plan names how much of the history is kept, the choices of destination, the files chosen, the copy of the home and the mounts, including why an entry is detached. **None** as the history means all of the conversation of the source is lost, on purpose. For a large tree that is live, choose exactly the files to copy and read their estimated bytes and time. The state of Git is refreshed only when you ask.

A request to copy a home or a private layer needs its reviewed way of transfer. If that way is unavailable, the plan blocks the copy. It never leaves those changes behind quietly, and it never copies the data underneath a mount instead.

Each resource has owners and a home, shown with the place that contains it. It is used where its permission page gives access, not only where it was made. Deleting or moving it needs **Manage** and does not borrow the permission of another person from a thread that happens to use it. Read [sharing](workflows.md#share-a-thing-or-a-place) before you give access, and [settings and usage](settings-and-usage.md#limits) for limits on its work.

A resource you can read keeps the actions to manage it visible. Without **Manage**, the action stays disabled and names that requirement. Opening or copying its link still works. Actions that depend on a state, and the reviews before deleting, keep their usual checks.
