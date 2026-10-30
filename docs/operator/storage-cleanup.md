# Storage cleanup

## Unused node caches

`cleanup.unused_keep_days` starts at 7. Set it in system settings, or on one
host's Settings page. A system lock prevents host overrides. Zero disables
automatic cleanup of unused images and tool versions.

The server chooses what may go. The node inventories its managed objects and
rechecks each exact object before removing it. Running work, stopped units and
retained environments' restore references keep their dependencies in use.
Incomplete inventory or unknown restore references keep caches; an absent
machine is not successful cleanup.

The first complete observation starts the age of a cache with no use history.
Tool downloads and checks record use. A use after selection invalidates that
selection. Cancellation stops between irreversible items after the current
item is confirmed. The job retains each completed or failed result.

A dependency keeps its cache occupied until it stops or is removed. Its confirmed
end starts the unused interval. These exact node stamps guard cleanup. The library
uses reported operations for its Used figures.

Owned data follows its existing retention dates. Cache cleanup does not purge
volumes, retained installed files or a person's existing directories. The node
does not run a whole-host prune. Registry images without installation ownership
evidence stay outside destructive inventory.

## Unused releases and dumps

`cleanup.release_keep_count` starts at 3. Each payload retains that many unused
complete releases, in addition to `current` and `previous`. Zero keeps all.
The updater applies the policy after a successful update or rollback. A policy
read failure keeps everything.

Database dumps are not removed automatically. Keep each dump, its manifest and
its paired blob copy together. See [Backup and restore](backup-and-restore.md)
and [Upgrades and recovery](upgrades.md#keeping-unused-releases).

Run `./update.sh prune` on the server to preview unused releases and their sizes.
Add `--yes` to apply. It keeps the current and previous releases and follows the
same setting as automatic update cleanup.

Run `./update.sh dumps remove <name>` for one recorded local dump pair. It lists
the dump, manifest and paired copy, then asks for the dump name. `--yes` confirms
an already reviewed plan. An unfinished update, a changed copy or missing pairing
evidence blocks removal. Deleting a rollback pair also ends that recovery point.
The live content store stays in place.

System Storage reads the updater's protected metadata. Its cleanup rows offer
these exact commands to copy. Run them where the files live. The API process has
no write access to release or dump directories.

## Uninstalling an instance

Run `orch-node uninstall [--purge]` or `orch-connector uninstall [--purge]` with
the instance's existing `--data-dir` and, for the node, `--config`. Read the
fates, sizes and blockers first. `--plan` leaves the installation in place.
Type the enrolled name once, or use `--yes` for an already reviewed plan.

The installer records the exact program, configuration and service instance.
Only the last active connector instance removes a shared program. An unknown
or damaged ownership record blocks removal. Run an older installation's
installer once to record its ownership. Other active or retained instances
inside a data directory block its purge.

Uninstall stops the recorded service and requires exclusive access to that
instance before purging. A foreground process must be stopped first. Stable
lock files contain no identity or credentials and remain to prevent a new run
from entering during directory removal. Windows reports final program deletion
as scheduled until this command exits.

Node purge removes only the selected system-owned runtime inventory, in
dependency order. A failed or unreadable item keeps the node's files for a new
plan. A driver without read-only inventory appears as a blocker before a service
is stopped. Connector purge rechecks each recorded directory's identity. It never
adopts an existing folder or purges an allowed storage root.

## Uninstalling the stack

From `deploy/`, run `./update.sh uninstall [--purge]`. Read the stack and node
inventory, then type the system name. `--plan` previews; `--yes` confirms.
Without purge, the runtime and secrets remain for `docker compose up -d`.
With purge, node cleanup must finish before the stack, `runtime/` and
`config/secrets/` are removed. Configuration and `.env` remain.

The default final save is a new protected directory in `backups/`, outside the
purged runtime. It contains the database dump and manifest, the subsequent blob
copy, configuration, original keys and node identity. Applications are stopped
for this save. Execution volumes are excluded; preserve their separate copies
before confirming. `--no-backup` explicitly skips the final save. Protect and
encrypt the saved set before transferring it off this host.

## Reviewed switch batches

`switch.batch_threads` starts at 200 and bounds one reviewed batch. Raise the system setting for a larger selection. `switch.batch_parallel` independently bounds concurrent jobs. A provider-wide selection uses current Binding facts and ordinary Thread Manage authority; unrelated and inaccessible threads are never granted to the requester.

## Reviewed cleanup

Storage and resource menus use the same expiring cleanup plan and durable job. The job reloads the original sign-in or token and its recorded administration mode before each item. A changed occupancy fence, unknown dependency, current lifecycle owner or recovery promise blocks removal. Cancellation finishes the admitted native operation before stopping; each actual outcome remains in the job.

A mount keeps the completed cleanup result open until Close, even after its last kept-data row disappears. A kept-environment row retains its accepted job when the dialog is hidden. Confirmed terminal outcomes refresh the retained-data views; a failed or unconfirmed purge never becomes a restored environment locally.

## Immediate pauses

Disabling a provider, model, profile or environment interrupts its active turns without holding the queued inputs. Separate summary and embedding requests are cancelled and refused at admission after the resource is off. A durable noncancellable stop then settles the native units while keeping volumes. Deletion admits the same stop and retires the provider runtime after its turns stop. Native source and queue authority are checked again when work resumes.

An accepted summary keeps its paying provider/model and source profile/environment. Its stop stays attached to that request even if the resource is enabled before dispatch. Completed reported usage still follows the ordinary accounting and recovery path. A deliberate pause is shown as a skip, rather than a producer failure.

Enable leaves Run the waiting messages unchecked. Those messages are then held for their authors to Resume or switch. Selecting it wakes eligible messages only after the resource is available. Existing personal Stop holds remain held, and each author passes the normal model-use checks again. Selecting it also continues each latest resource-stopped turn through the shared visible Continue as its original author, preserving all original authority. A revoked author leaves a safe stopped-task result; no failed-call rule or guard attempt is created. The count opens the visible thread list and names when waiting began.

## Operation notice defaults

Reviewed moves, recovery, Declare lost and Delete host resolve `notify.operations` through the normal settings and locks. Its shipped value is `when_blocking`; `always` and `never` change the initial checkbox. The person still decides for the operation. A single environment or provider uses its own resolved setting. A host uses its home. A machine without a home, or recovery across several homes, uses the system setting. Move batches opened in a place use that place. Personal notification-kind choices still filter the recipient count and delivery.

## Removing a host

Delete host pauses scheduling and admits only the caller's authorized cascades. Other managers' environments wait for their managers; they do not block acceptance. The progress names remaining operations, those environments, or the original retained-file date. Cancel while waiting restores the earlier scheduling choice and leaves accepted child operations under their own controls.

Mount data defaults to Leave on the machine. Move uses the existing Copy transfer into a new destination location. Delete requires the mount's name. Existing borrowed directories are never erased. Retained environments default to their frozen recovery deadline. Early Purge now needs administration of their current home and the environment name. Unattributed owned volumes and layers default to Leave and need their own typed Delete consent.

Erase what Orch keeps there defaults on for reachable hosts. Its inventory read is an ordinary watched background job requiring Host Manage. An incomplete read blocks erasure. After the cascades, fresh exact node inventory and removal repeat ownership and dependency checks; general erasure excludes data rows set to Leave or Wait. A failed removal keeps the host registered. The result lists remaining objects with measured sizes or an unknown figure.

A declared-lost host is removed from Orch alone. Run `orch-node uninstall --purge` on the original machine when reachable to review the remaining owned objects. It uses the same inventory; it never prunes another system's objects.
