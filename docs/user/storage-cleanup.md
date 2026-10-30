---
title: Storage cleanup
screens: /storage, /resources/providers/$id, /resources/agents/$id, /resources/environments/$id, /resources/templates/$id, /resources/hosts/$id, /resources/machines/$id, /resources/mounts/$id, /resources/skills/$id, /admin, /org/$slug, /project/$projectId, /topic/$topicId
keywords: storage, cleanup, caches, images, tool versions, retention, restore, unused releases, Off, cleanup.unused_keep_days, cleanup.release_keep_count, uninstall, purge, orch-node, orch-connector
---

# Storage cleanup

## Unused caches

Open a host's **Settings** and find **Clean unused caches**. The system starts
with seven days. A host manager can choose another value unless the system
administrator locked it. **Reset to inherited** follows the system again.

Zero shows **Off** and switches automatic cache cleanup off. Focus the field
to edit its numeric value. A cache that a unit or a retained environment still
needs stays. Unknown use is kept until Orch can establish what needs it.

Orch checks caches when the node joins and with its regular storage report.
It confirms each removal with that node. A machine that does not answer is
not counted as cleaned.

## Kept data

Files kept for restore follow their own recovery dates. Changing cache cleanup
does not shorten those dates. A stopped unit still keeps its volumes in use.
Zero for **Removed files** makes future removals eligible for immediate purge;
it does not change a retention window that was already promised.

A size shown as **–** has not been measured. The host's **Storage** sections
show retained files and layers with their existing recovery state.

## Unused releases

A system administrator changes **Keep unused releases** in system settings.
It starts at three unused releases for each payload. The current and previous
releases always stay. Zero shows **Off** and keeps all releases.

Cleanup runs after a successful update or rollback. A policy that cannot be
read keeps every release. Database dumps are kept until someone removes them.

## Uninstall

On the machine, run `orch-node uninstall` or `orch-connector uninstall`.
The command lists what goes and what stays. Type the enrolled host or machine
name once. `--plan` previews it. `--yes` confirms without prompting.

Without `--purge`, the data and identity stay. With `--purge`, the node removes
this system's runtime objects and data. The connector removes its data and the
folders it made. It keeps existing work folders. Another instance's data inside
the selected directory blocks that directory's purge.

A shared connector program stays until its last active instance is removed.
On Windows, that last removal finishes after the command exits. Installer
receipts identify the files to remove. An older installation without a receipt
needs its installer run once before uninstall.

For the bundled stack, use `deploy/update.sh uninstall`. Add `--purge` to remove
its runtime and secrets after node cleanup. It first saves a final database dump,
manifest, blob copy, configuration and node identity in `deploy/backups/`.
Execution volumes need separate backups. `--no-backup` skips this final save.
The command asks for the system name. Configuration and `.env` stay.

## Storage report

Open Storage in your account menu. Choose your own storage, a place, a host, or system storage in administrator mode. The figures, breakdowns and records use the same filters. Press a breakdown row to narrow the report. Clear filters returns to the full scope.

Each record says why it stays and its known end. Kept by lists named dependents only where you may see them; other dependents stay counts. A missing size is a dash. An away or incomplete inventory stays visible as an unavailable source. The page never treats an unknown inventory as empty.

Where shows the place and the host or machine that keeps the item. Open their names to inspect them. Hidden locations stay unnamed. On a narrow screen, these facts appear under the item.

The report joins host objects, removed environment files, displaced mount data and skill archives. System storage also shows the database, content, uploads and exports. The report keeps its categorical filters on this device and in its address. Typed locator text is not saved.

## Clean up kept items

A mount’s cleanup result stays open until you close it, including when its last kept item is removed.

Choose **Clean up…** in a resource menu, or select records in Storage and choose **Clean up…**. Both open the same plan.

Each row shows its size, why it is kept, and its recovery date. Open **Kept by** to follow its dependents. Hidden names stay a count. A blocked row stays in the plan with its reason.

Choose **Keep**, **Remove**, **Stop and remove**, or **Purge now** where offered. Stopping a unit keeps its volumes unless those volumes are selected too. **Purge now** erases kept data and asks you to type the plan's name. A manager cannot shorten an environment's recovery window without administration of its current home.

The job keeps running after you close the dialog. **Cancel remaining** waits for the active removal to finish, then stops before the next item. The result lists what was removed, kept, blocked or failed. Removed items stay removed after cancellation.

A kept environment's row keeps the accepted job's progress after **Hide**. Kept-data views refresh from the server's confirmed result.

## Server files

System Storage also lists the updater's releases and dump pairs. Their size and
kept reason come from protected server metadata. A missing source stays visible
as unrecorded or unavailable.

Their cleanup rows show a command to copy and run on the server. Orch's API
does not erase these files. `update.sh prune` previews unused releases;
`--yes` applies the release policy. Current and previous releases stay. Zero
for **Keep unused releases** keeps all releases.

`update.sh dumps remove <name>` reviews one dump, its manifest and its recorded
local blob copy. Type the dump name to confirm. An unknown or changed pairing
blocks removal. Dumps are never removed automatically. Refresh Storage after
running the command.
