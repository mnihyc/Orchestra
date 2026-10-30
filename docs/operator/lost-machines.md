# Recover after a machine is lost

Use **Declare lost…** when a machine will not return, or an environment's unit or volumes have gone. Its managers can declare a host or environment lost; a personal machine's owner can declare that machine lost. Orch first lists the affected environments, threads, retained data, mounts, provider runtimes and waiting jobs. Confirm by typing its name.

Declaration permanently revokes the affected machine identities. It retires their runtime records without contacting the missing machine, ends waiting jobs as **Lost**, and closes retained-data promises as lost while preserving their recorded sizes. This does not claim that files were deleted. Shared mount locations are removed from the current choices; lost volume locations retain their last known sizes. A provider whose runtime was there becomes unavailable; its managers can use the ordinary runtime re-creation controls to provision it elsewhere. Subscription credentials that existed only there may need signing in again.

Threads keep their conversation and environment binding. Their banner offers **Run elsewhere…**. A lost or removed default environment receives no new threads: the normal environment-selection fallback applies. When **Notify** is selected, the topic's managers receive a notice to choose another default. A lost host can be deleted after declaration.

## Recover several environments

Open **Recover…** from the Inbox, select environments in the fleet, or use **Move all…** on a host. Review each row and choose one common destination host, or leave **Automatic** selected. A row can override that host.

- **Move** copies an answering environment to another host and keeps its identity. A local agent resumes from its copied home. When the home cannot travel, it continues from Orch's recorded conversation. The plan names which method applies.
- **Re-create** provisions a lost environment again. Its identifier, page, owner, configuration and threads remain. Matching restored workspace and home volumes are reused; otherwise, the new workspace receives only file contents that Orch still holds. The plan shows missing files. Their full list is also written to `.orch/handoff/missing-files.txt` in each thread's directory. Without a workspace listing, coverage is unknown. A fresh home receives an indexed conversation file and the same bounded Carry handoff used by switching.
- **Switch threads** places the environments' threads into an existing environment or one new environment shared by the whole batch. Review the destination and its file-copy consequences first.

One progress bar covers the batch. Its concurrency follows the ordinary batch setting. Members succeed or fail independently; cancellation leaves committed members in place. Retry selects the members that failed.

## Restore volumes from a host backup

Orch does not back up execution hosts. The database holds conversations, metadata and file listings. Its blob store holds snapshots and file contents that were explicitly copied or opened; these are not a complete workspace backup. Uncached files, unbacked homes, native sessions, private mount layers, installed files and provider credentials can be lost with the host. Keep host backups separately, together with the database, blob store and installation secrets.

Before confirming Re-create, restore the environment's workspace and home volumes onto the destination daemon or host storage, keeping their names and protected ownership metadata. The plan checks both this installation's `orch.system` label and the exact `orch.environment_id`, as well as the driver and storage. A same-named volume with another owner is refused. Re-create preserves matching data and reports its size when the driver can measure it. The new connector uses fresh enrollment and state: credentials from the missing connector remain revoked. Restore shared mount data separately and register its new location before attaching it.

If the old machine returns, isolate it and stop the orphaned units before reconnecting anything. Its old identities remain revoked and cannot be enrolled again. Inspect or back up surviving files locally, restore wanted data to the chosen host, and enroll any replacement machine with a new identity. Remove old units and volumes only after verifying the recovered work and any external backups.

Telling people is a choice in each confirmation. **Notify N people** counts the people who will receive that kind under their own notification preferences. Hover, focus or press the count to see the names you may see and a count for the others. A bulk operation writes one Inbox item for each person, listing their affected work. Updates to the operation stay in that item. Turning Notify off suppresses those items.

Recover acts on each environment's own permissions. Managing a host does not let you recover environments you do not manage. Those environments appear as a count; their managers recover them. You also need your own access to the destination host and template. An administrative bypass identifies the permissions you will be named in, adds you to those pages when the act is confirmed, and marks the audit.

An imported MCP server keeps its identity, enablement and permission page when its offering machine is lost or removed. With no remaining offering it says **No machine**. A later machine in the same home can offer it again only when it reports the same native name and structural definition: transport, command or address, and environment-variable names. Variable and secret values do not form that identity. A different definition stays **Found** until a manager who may import from that machine chooses **Use this machine's**; that act records the new definition. An older import with no recorded structural identity requires this explicit choice once. Merely finding a definition never enables it.

Mount definitions also remain enabled without locations. Lost volume locations keep their last measured size, or an unknown size if never measured, and no cleanup command is sent to a lost machine. Add a location on a new host to attach the mount there again.

Removing a host reviews its remaining environments, mount locations, MCP offerings and provider runtimes together. Environments recover first by default; removing them instead uses their ordinary kept-data window. Locations are forgotten by default; deleting an answering host's owned mount volume also requires the mount's exact name. Imported MCP resources stay when the host stops offering them. Providers are left unavailable by default, or explicitly re-created elsewhere. The deletion job waits for its independent jobs and every existing kept-data deadline before revoking the host. Cancelling it leaves the host registered and keeps cascades that already completed.

An input whose delivery was never confirmed before permanent loss remains visibly failed for its author to retry. Recovery's Carry file identifies that recorded attempt; it does not claim that the original harness completed it or automatically resend those words.


## What people review

The [user guide](../user/recover-lost-work.md) explains the declaration, per-environment recovery, Notify recipients, native server offerings and host deletion. Its Chinese edition ships under the same Help article. The questions beside these controls open the relevant section.

Allow background counts to finish before accepting a current plan. A pending count does not prove that data is absent. An unsupported optional home can use the recorded Carry path; required workspace and private-state work still needs its checks. Host deletion follows the reviewed cascade choices, child jobs and every existing retained-data deadline. It does not shorten retention to remove a host sooner.

An open mount content comparison depends on its source and destination machines. Declaring either lost closes that comparison as **Lost** in the same transaction, even when its environment lives elsewhere. Reopening its plan retains the outcome; a new **Check** is explicit.
