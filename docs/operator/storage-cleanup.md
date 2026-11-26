# Clean up storage and uninstall

Free the disk that Orch keeps on its hosts and on the server, and remove an instance or the whole stack. At the end you know what is removed automatically, what you remove by hand, and what stays.

People manage kept data from the screens: see [Storage cleanup](../user/storage-cleanup.md). This page covers the settings and the commands you run on the machines.

## Before you start

- Run the `update.sh` commands on the server, in `deploy/`. The API process has no write access to the release and dump directories, so the **Storage** screen in **Management** shows you the exact command to copy instead of running it.
- Back up first if you are unsure ([Back up and restore](backup-and-restore.md)).
- Cleanup previews its plan before it removes anything. Read it.

## Clean up unused caches on nodes

Orch removes unused images and tool versions on a host after `cleanup.unused_keep_days` (7 by default). Set it in the system settings, or on one host's **Settings** page; a lock at the system prevents a host's override. Zero turns automatic cleanup of unused images and tool versions off.

1. Open **Management → Storage** (or the host's page) and read what is unused and for how long.
2. To change the period, edit `cleanup.unused_keep_days`.
3. To clean up now, choose the cleanup action and review the plan. The same plan and the same job are behind every cleanup menu in **Storage** and **Resources**.

**How it decides.**
- The server chooses what may go. The node lists the objects it manages and checks each exact object again before it removes it.
- Running work, stopped containers and the restore references of retained environments keep their dependencies in use. A dependency keeps its cache occupied until it stops or is removed, and its confirmed end starts the unused interval.
- The first complete observation starts the age of a cache that has no use history. Tool downloads and checks record use, and a use after the plan was chosen cancels that selection.
- An incomplete inventory or an unknown restore reference keeps the caches. A machine that is absent is not a successful cleanup.
- A job reloads the original sign-in or token and its recorded administration mode before each item. A changed occupancy fence, an unknown dependency, a current lifecycle owner or a recovery promise blocks removal. Cancelling finishes the operation in progress and then stops; each result stays in the job.

**What it never does.** Cache cleanup does not purge volumes, installed files that are kept, or a person's existing directories. The node does not run a whole-host prune. Registry images without evidence that Orch installed them stay out of the inventory that can be removed. Data that Orch owns follows its own retention dates.

## Clean up unused releases and dumps

`cleanup.release_keep_count` (3 by default) is how many unused complete releases each payload keeps, besides `current` and `previous`. Zero keeps all. The updater applies it after a successful update or rollback. If it cannot read the setting, it keeps everything.

1. Preview the unused releases and their sizes:

   ```sh
   ./update.sh prune
   ```

2. Apply it with `./update.sh prune --yes`. It keeps the current and previous releases and follows the same setting as the automatic cleanup after an update.
3. Database dumps are never removed automatically. To remove one recorded local dump pair:

   ```sh
   ./update.sh dumps remove <name>
   ```

   It lists the dump, its manifest and the paired blob copy, then asks you to type the dump name. `--yes` confirms a plan you have already reviewed.

Keep each dump, its manifest and its paired blob copy together. An unfinished update, a changed copy or missing pairing evidence blocks removal. Deleting a rollback pair also ends that recovery point. The live content store stays where it is. [Update and roll back](upgrades.md#keep-unused-releases) describes the rollback side.

## Uninstall a connector or a node

1. Read the plan first. It shows the fates, the sizes and anything that blocks the removal, and leaves the installation in place:

   ```sh
   orch-node uninstall --plan
   orch-connector uninstall --plan
   ```

   Use the instance's existing `--data-dir` and, for the node, `--config`.
2. Run `orch-node uninstall [--purge]` or `orch-connector uninstall [--purge]` with the same options. Type the enrolled name once, or pass `--yes` for a plan you have reviewed.

**What it does.**
- The installer records the exact program, configuration and service instance. Only the last active connector instance removes the shared program. An unknown or damaged ownership record blocks removal; run an older installation's installer once to record it.
- It stops the recorded service and needs exclusive access to the instance before it purges. Stop a foreground process first. Small lock files hold no identity or credentials and stay, so that a new run cannot enter while the directory is removed. On Windows the final deletion of the program is reported as scheduled until the command exits.
- A node purge removes only the system-owned runtime inventory you selected, in dependency order. A failed or unreadable item keeps the node's files for a new plan. A driver that cannot list its objects without changing them shows up as a blocker before any service is stopped.
- A connector purge checks each recorded directory's identity again. It never adopts an existing folder and never purges an allowed storage root. Other active or retained instances inside a data directory block its purge.

## Uninstall the stack

1. From `deploy/`, preview: `./update.sh uninstall --plan`.
2. Read the stack and node inventory, then run `./update.sh uninstall [--purge]` and type the system name (`--yes` confirms a plan you reviewed).

Without `--purge` the runtime and the secrets stay, and `docker compose up -d` brings the stack back. With `--purge`, node cleanup must finish first. Then the stack, `runtime/` and `config/secrets/` are removed; the configuration and `.env` remain.

**The final save.** By default the command first makes a new protected directory in `backups/`, outside the purged runtime. It holds the database dump and manifest, the blob copy made after it, the configuration, the original keys and the node identity. The applications are stopped for this save. Execution volumes are not included, so keep their separate copies before you confirm. `--no-backup` skips the final save on purpose. Protect and encrypt the saved set before you move it off this host.

## Remove a host

**Delete** on a host, with its choices for environments, mount data and what Orch keeps there, is described for the people who do it in [Recover work after a machine is lost](../user/recover-lost-work.md#remove-a-host). Two facts matter to an operator. A host that was declared lost is removed from Orch alone; run `orch-node uninstall --purge` on the original machine, when it can be reached, to review the objects it still owns. And the inventory read that erases what Orch keeps is an ordinary watched job that needs **Host Manage**; an incomplete read blocks erasure, and a failed removal keeps the host registered.

## Settings that belong to these reviews

| Setting | Default | What it does |
|---|---|---|
| `cleanup.unused_keep_days` | 7 | Days before unused images and tool versions are removed; 0 turns it off |
| `cleanup.release_keep_count` | 3 | Unused complete releases kept besides `current` and `previous`; 0 keeps all |
| `switch.batch_threads` | 200 | The largest reviewed batch of threads that one switch can take; raise it at the system for a larger selection |
| `switch.batch_parallel` | 4 | How many jobs of a batch run at once |
| `notify.operations` | `when_blocking` | The first state of **Notify** for reviewed moves, recovery, **Declare lost** and **Delete host**: `always` and `never` change the checkbox, and the person still decides |

A provider-wide selection for a switch uses the current bindings and the person's own authority to manage each thread; unrelated threads, and threads the person cannot reach, are never added. A single environment or provider uses its own `notify.operations`. A host uses its home. A machine without a home, or a recovery across several homes, uses the system's. A move batch opened in a place uses that place's.

Pausing a provider, model, agent profile or environment is a person's act and is explained in [Pause, switch and remove](../user/pause-and-switch.md).
