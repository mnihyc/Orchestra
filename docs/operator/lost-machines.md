# Recover after a host is lost

Get work running again after a host or machine will not come back: back up what Orch does not, restore the volumes on a new host, and re-create the environments there. At the end the environments run on a new host with their threads, and the old machine cannot rejoin by mistake.

People declare the loss, choose where each environment goes, and tell the people affected from the screens. That workflow is in [Recover work after a machine is lost](../user/recover-lost-work.md). This page covers what you do on the hosts.

## What Orch keeps and what it does not

Orch does not back up execution hosts. The database holds conversations, metadata and file listings. The blob store holds snapshots and file contents that were copied or opened on purpose; it is not a complete workspace backup. These can be lost with the host:

- files that were never cached, and homes that were never backed up;
- the agent programs' own sessions, and the layers of private mounts;
- installed files and provider credentials that existed only there.

Keep host backups separately, together with the database, the blob store and the installation's secrets ([Back up and restore](backup-and-restore.md)).

## 1. Before you declare the host lost

1. If the old machine might return, isolate it and stop its orphaned containers before you reconnect anything. Its old identities are revoked for good and cannot enroll again.
2. If the old disk is readable, inspect or back up the surviving files locally.
3. Let people review the plan in the screens. Allow the background counts to finish before they accept it: a count that is still pending does not prove that data is missing.

Declaring a host lost permanently revokes the identities of the affected machines. Orch retires their runtime records without contacting the missing machine, ends waiting jobs as **Lost**, and closes the promises to keep data as lost while keeping their recorded sizes. That does not claim any file was deleted. A lost host can be deleted afterwards. A lost or removed default environment receives no new threads, and the usual fallback chooses another.

## 2. Restore the volumes on the new host

Do this before anyone chooses **Re-create**.

1. Restore the environment's workspace and home volumes onto the destination Docker daemon or host storage. Keep their names and the protected ownership metadata.
2. Check that each restored volume carries this installation's `orch.system` label and the exact `orch.environment_id` of its environment. The plan checks both, as well as the driver and the storage. A volume with the same name and another owner is refused.
3. Restore the data of shared mounts separately, and register its new location before you attach the mount ([Choose where a machine keeps data](../user/choose-machine-storage.md)).

**What you will see.** Re-create keeps matching data and reports its size when the driver can measure it. Where no matching volume exists, the new workspace receives only the file contents that Orch still holds. The plan lists the missing files, and the full list is also written to `.orch/handoff/missing-files.txt` in each thread's directory. Without a workspace listing, the plan says the coverage is unknown. A fresh home receives an indexed conversation file and the same bounded handoff that switching uses.

The new connector enrolls with fresh state: credentials of the missing connector stay revoked. A provider whose container was on the lost host becomes unavailable until its manager re-creates it elsewhere; subscription credentials that existed only there may need a new sign-in.

## 3. Enroll a replacement machine

Enroll a replacement with a new identity ([Connect a machine](../user/connect-a-machine.md)). Do not reuse the old one. Remove old containers and volumes only after you have checked the recovered work and any outside backups.

## What happens to work that was running

- Threads keep their conversation and their environment binding. Their banner offers **Run elsewhere…**.
- An input whose delivery was never confirmed before the loss stays visibly failed, for its author to retry. The handoff file names that recorded attempt; it does not claim the agent program finished it, and Orch does not send those words again by itself.
- A mount's content comparison that depends on a machine declared lost ends as **Lost**, in the same transaction, even when the environment lives elsewhere. Reopening its plan keeps that outcome; a new **Check** is a deliberate act.
- An imported MCP server keeps its identity, its enablement and its permission page. With no machine offering it, it says **No machine**. A later machine offers it again by itself only when it reports the same name and structure: the transport, the command or address, and the names of its environment variables. Values and secrets are not part of that. A different definition stays **Found** until a manager who may import from that machine chooses **Use this machine's**; finding a definition never turns it on.

## If it does not work

| You see | What to do |
|---|---|
| Re-create refuses a volume | It has another owner or a missing label. Restore it under the right name and labels, or let Re-create make a new one |
| The plan shows missing files | Read `.orch/handoff/missing-files.txt`, restore those files from a host backup, or accept the loss |
| The old machine tries to connect | Its identity is revoked. Isolate it; enroll a replacement under a new identity |
| A batch partly failed | Members succeed or fail independently and committed members stay. Choose **Retry failed** |
| Removing a host waits | It waits for its independent jobs and every kept-data deadline. It never shortens retention. Cancelling keeps the host registered and the cascades that finished |

Recovery acts on each environment's own permissions. Managing a host does not let a person recover environments they do not manage; their managers recover those. An administrator's override names the permissions it adds, adds them to the pages when confirmed, and marks the audit trail.
