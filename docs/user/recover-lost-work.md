---
group: operations
title: Recover work after a machine is lost
screens: /resources/hosts, /resources/hosts/$id, /resources/environments, /resources/environments/$id, /resources/machines, /resources/machines/$id, /resources/mcp/$id, /resources/mounts/$id, /resources/providers/$id, /thread/$threadId, /topic/$topicId, /project/$projectId, /org/$slug, /inbox, /settings/notifications
keywords: lost, Declare lost, Recover, Move all, Re-create, Switch threads, destination environment, Carry, Notify, MCP, mounts, managed provider, default environment, delete host
summary: Declare that a machine is lost for good, choose where its work goes, and tell the people it affects.
---

# Recover work after a machine is lost

When a host, a machine or an environment will not come back, you declare it lost, move its work elsewhere and tell the people it affects. This page goes through those steps, then what happens to servers, mounts and managed providers, and how to remove a host.

## Declare lost

Use **Declare lost…** in the **⋯** menu of a host, an environment or a machine that will not return. The managers of the host or environment can do this. The owner of a personal machine can declare that machine lost.

1. Read the affected environments, threads, kept data, mounts, managed providers and jobs. Private work appears as a count.
2. Choose **Notify**, type the exact name, then confirm.

Orch revokes the identities of the affected machines for good. It sends no cleanup command to the machine that is missing. Jobs that depended on it end as **Lost**. The result keeps the consequences you reviewed visible until you close it.

Threads keep their conversation and their environment. Their banner says **Blocked: its environment was lost** and offers **Run elsewhere…**. An input whose delivery was never confirmed stays failed, and its author decides whether to retry it.

## Recover environments

Open **Recover…** from the menu of the lost resource or from its Inbox item. You can also select several environments together. On a host that still answers, **Move all…** uses the same review.

Choose the **Destination host** that all share, or leave **Automatic** selected. Each row can choose another host. You need to manage that environment, and to have your own access to the destination host and the template. Environments you do not manage appear as a count, and their owners recover them.

Choose an action for each row:

- **Move** copies an environment that still answers. It keeps the same environment and its threads. A copied home lets local agents resume. When the home cannot travel, they continue from the recorded conversation through **Carry**.
- **Re-create** provisions the lost environment again. Its page, its owner, its settings and its threads stay. The review checks the volumes of the workspace and the home that were restored. Otherwise it copies only the contents of files that Orch still holds. Read the count of missing files before you confirm. **File listing unavailable** means that the coverage is not known.
- **Switch threads** sends the threads to the destination environment that the next section describes.

Counts run in the background. Confirming waits for the counts that are required and for the current review. A size that is unknown shows as **–**. A row that is blocked does not stop the other rows that can run. One progress view shows the independent jobs. **Retry failed** selects the members that failed or were lost. Cancelling keeps the work that was completed.

An administrator uses the existing administrative act on purpose. The review names the permissions that would be added, and the audit trail records the bypass.

## Choose a switch destination

**Destination environment** can name an environment that exists, or **One new shared environment** for the whole batch. For a new one, select its name and host. A normal batch of threads can also keep the environment of each thread. Destinations that are lost, removed or turned off are left out.

Review the destination and the consequences for the files that are copied before you confirm. The batch uses that environment by name, even when its threads keep their choices of provider and model.

## Tell the affected people

Each confirmation has its own **Notify N people** choice. **Operation notices** (`notify.operations`) sets its default: never initially, or when work is blocked or always if you choose that policy. You can change the choice before confirming. The setting follows its place and any system lock. A recovery across several places uses the system default.

- The number counts the people who will receive that kind of notice under their own notification settings. Hover, focus or press the count to open the common list of people. It shows the names you may see and a count for the rest. Select a person to open their card.
- One Inbox item groups the affected work of the operation for each recipient, and updates stay in that item. Turning **Notify** off suppresses it.
- Choose the kinds you receive in **Settings → Notifications → Kinds**. Changes are saved automatically, and a save that fails shows its reason. This changes your own choices. The **Notify** choice of another person cannot turn on a kind that you turned off.

## Servers and mounts

An MCP server that was imported keeps its definition, its settings and its permissions after its machine is lost or deleted. Without another offering that is ready, its page says **No machine**.

A later machine offers that same import at once only if it reports the same name and definition: the transport, the command or address, and the names of the environment variables. Values and secrets are not compared. The page shows the offering and its time. A different definition stays **Found**. A manager of the server who may import from that machine can choose **Use this machine's** to record the new definition. Finding a server alone never turns it on.

A mount keeps its definition and its choice of being on, with no current location. Locations that are lost show **Lost** and their last known size, and a size that was never counted shows **–**. Add a location on another host before you attach the mount there. No cleanup is tried on the lost machine.

A check of the content of a mount that depends on a machine declared lost ends as **Lost**. It starts no further read by itself. Choose **Check** for a new comparison once the locations it needs are available.

## Managed providers

A managed provider whose container was lost keeps its settings and its permissions. The files in its container are unavailable. Its manager can choose **Re-create** in the container settings, review the destination and the **Notify** choice, then confirm. Credentials that existed only on the old machine may need to be signed in again.

## Remove a host

**Delete…** reviews four groups together: environments, locations of mounts, MCP servers and managed providers. By default, environments are recovered first. Removing them instead uses their usual window for kept data. Only the environments you may manage are yours to change.

- **Data of a mount.** Each mount's data has its own choice: **Leave on the machine**, **Move** or **Delete**. **Leave** is the default. **Move** uses the usual **Copy** transfer into a new location on another host, and you choose its destination. Directories that already existed and that Orch was given stay intact, also after copying. **Delete** asks for the exact name of the mount. Copies that were kept earlier have their own rows.
- **Files of environments that were removed** default to **Wait until** their existing date. An administrator of the current home can choose **Purge now** and type the name of the environment. Other managers can purge after the date. Neither deleting nor a retry shortens that date by itself.
- **Erase what Orch keeps there** starts on. It lists the containers, images, builds, networks and tool versions that Orch owns, with their sizes. Data that is left, or that waits, is excluded. Volumes and layers that cannot be attributed have their own choice of **Leave** or a **Delete** you confirm by typing. An inventory that is incomplete blocks erasing, and a size that is unknown shows as **–**.
- **Servers and providers.** The MCP servers stop being offered there, and the resources imported stay. Managed providers stay unavailable unless you choose to re-create them elsewhere.

Read the blockers and any deadline for kept data, choose **Notify**, type the name of the host, then confirm. The host pauses, and the job does your part first. Environments of other people do not prevent the start. The progress line names the recoveries that remain, the environments that their managers must recover or remove, or the date of the kept data. Cancelling while it waits keeps the host registered, and the cascades that were accepted continue.

The result lists the objects left on the machine and their sizes. A host that is lost is removed from Orch alone. Run `orch-node uninstall --purge` on the machine, when it can be reached, to review and remove the objects it still owns.

## When a default is lost

A default environment that is lost or removed receives no new threads. Orch uses the usual fallback for choosing an environment. When **Notify** is selected, the managers of the topic that is affected get a notice to choose another default.
