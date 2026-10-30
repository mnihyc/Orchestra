---
title: Recover work after a machine is lost
screens: /resources/hosts, /resources/hosts/$id, /resources/environments, /resources/environments/$id, /resources/machines, /resources/machines/$id, /resources/mcp/$id, /resources/mounts/$id, /resources/providers/$id, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /inbox, /settings/notifications
keywords: lost, Declare lost, Recover, Move all, Re-create, Switch threads, destination environment, Carry, Notify, MCP, mounts, provider runtime, default environment, delete host
---

# Recover work after a machine is lost

## Declare lost

Use **Declare lost…** in a host, environment or machine's **⋯** menu when it will not return. The host or environment's managers can do this. A personal machine's owner can declare that machine lost.

Read the affected environments, threads, kept data, mounts, provider runtimes and jobs. Private work appears as a count. Choose **Notify**, type the exact name, then confirm. Orch revokes the affected machine identities permanently. It sends no cleanup command to the missing machine. Jobs that depended on it end **Lost**. The result keeps the reviewed consequences visible until you close it.

Threads keep their conversation and environment. Their banner says **Blocked: its environment was lost** and offers **Run elsewhere…**. An input whose delivery was never confirmed stays failed. Its author decides whether to retry it.

## Recover environments

Open **Recover…** from the lost resource's menu or its Inbox item. You can also select several environments together. On an answering host, **Move all…** uses the same review.

Choose the common **Destination host**, or leave **Automatic** selected. Each row can choose another host. You need management of that environment and your own access to the destination host and template. Environments you do not manage appear as a count. Their owners recover them.

Choose an action for each row:

- **Move** copies an answering environment. It keeps the same environment and its threads. A copied home allows local agents to resume. When the home cannot travel, they continue from the recorded conversation through Carry.
- **Re-create** provisions the lost environment again. Its page, owner, settings and threads stay. The review checks restored workspace and home volumes. Otherwise it copies only file contents Orch still holds. Read the missing-file count before confirming. **File listing unavailable** means coverage is unknown.
- **Switch threads** sends the threads to the destination environment described below.

Counts run in the background. Confirmation waits for the required counts and the current review. Unknown size is **–**. A blocked row does not prevent other executable rows from running. One progress view shows the independent jobs. **Retry failed** selects failed or lost members. Cancelling keeps completed work.

An administrator uses the existing administrative act explicitly. The review names the permission additions, and the audit records the bypass.

## Choose a switch destination

**Destination environment** can name an existing environment, or **One new shared environment** for the whole batch. Select its name and host when making a new one. A normal thread batch can also keep each thread's environment. Lost, removed and disabled destinations are excluded.

Review the destination and file-copy consequences before confirming. The batch uses that named environment even when its threads keep their provider and model choices.

## Tell the affected people

Each confirmation has its own **Notify N people** choice. **Operation notices** (`notify.operations`) sets its default: when work is blocked, always, or never. You can change the choice before confirming. The setting follows its place and any system lock. A recovery across several places uses the system default.

The number counts people who will receive that kind under their own notification settings. Hover, focus or press the count to open the common people list. It shows the names you may see and a count for the rest. Select a person to open their card. One Inbox item groups the operation's affected work for each recipient. Updates stay in that item. Turning Notify off suppresses it.

Choose the kinds you receive in **Settings → Notifications → Kinds**. Changes save automatically; a failed save shows its reason. This changes your own choices. Another person's Notify choice cannot turn on a kind you have turned off.

## Servers and mounts

An imported MCP server keeps its definition, settings and permissions after its machine is lost or deleted. Without another ready offering its page says **No machine**.

A later machine offers that same import immediately only if it reports the same name and definition: transport, command or address, and environment-variable names. Values and secrets are not compared. The page shows the offering and its time. A different definition stays **Found**. A server manager who may import from that machine can choose **Use this machine's** to record the new definition. Finding a server alone never enables it.

A mount keeps its definition and enabled choice with no current location. Lost locations show **Lost** and their last known size; an uncounted size is **–**. Add a location on another host before attaching the mount there. No cleanup is attempted on the lost machine.

A mount content check that depends on a declared-lost machine ends **Lost**. It starts no further read automatically. Choose **Check** for a new comparison once the required locations are available.

## Provider runtimes

A provider whose runtime was lost keeps its settings and permissions. Its runtime files are unavailable. Its manager can choose **Re-create** in the runtime configuration, review the destination and Notify choice, then confirm. Credentials that existed only on the old machine may need signing in again.

## Remove a host

**Delete…** reviews four groups together: environments, mount locations, MCP servers and provider runtimes. Environments recover first by default. Removing them instead uses their ordinary kept-data window. Only environments you may manage are yours to change.

Each mount's data has its own choice: **Leave on the machine**, **Move**, or **Delete**. Leave is the default. Move uses the ordinary Copy transfer into a new location on another host. Choose its destination. Existing directories Orch was given stay intact, including after copying. Delete asks for the exact mount name. Earlier kept copies have their own rows.

Removed environments' files default to **Wait until** their existing date. A current home administrator can choose **Purge now** and type the environment's name. Other managers can purge after the date. Neither deletion nor a retry shortens that date by itself.

**Erase what Orch keeps there** starts on. It lists owned units, images, builds, networks and tool versions with their sizes. Leave and Wait data are excluded. Unattributed volumes and layers have their own Leave or typed Delete choice. An incomplete inventory blocks erasure. Unknown size is **–**.

The MCP servers stop being offered there; imported resources stay. Providers stay unavailable unless you choose to re-create them elsewhere.

Read the blockers and any kept-data deadline, choose Notify, type the host's name, then confirm. The host pauses and the job does your part first. Other people's environments do not prevent starting. The progress line names the remaining recoveries, the environments their managers must recover or remove, or the date of the kept data. Cancelling while waiting keeps the host registered; accepted cascades continue.

The result lists objects left on the machine and their sizes. A lost host is removed from Orch alone. Run `orch-node uninstall --purge` on the machine when it can be reached to review and remove its remaining owned objects.

## When a default is lost

A lost or removed default environment receives no new threads. Orch uses the ordinary environment-selection fallback. When Notify is selected, the affected topic's managers receive a notice to choose another default.
