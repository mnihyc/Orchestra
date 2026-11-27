# Back up and restore Orch

Keep a copy of an Orch system that you can put back, and prove that you can. At the end you have a daily backup that runs by itself, a copy of it off the host, and a restore you have practised on an isolated host.

A recoverable set is: one database dump and its manifest, a blob copy made after that dump, the configuration and the original secret keys, and any execution volumes your recovery needs. A database alone does not restore the keys or the machines' identities.

Examples below use the default data paths. If you set `ORCH_DATA_DIR`, copy the contents of that directory for the paired file backup. When restoring, put those contents in the new installation's selected server directory. Check both data paths in the restored `.env` before first start. Use empty, separate destinations on a practice host.

## Before you start

You need an installed stack ([Install Orch](deployment.md)), room off the host for each set (the database size plus the blobs plus the volumes you keep), and a second host, or at least an empty directory on one, for the practice restore.

## What to keep

| What | How | How often |
|------|-----|-----------|
| The database | `./update.sh backup`: a custom-format dump, read back before it is kept, in `runtime/update-backups/`, taken without stopping anything | Daily. One is also taken before every update and rollback |
| Blobs, `runtime/data/` | A separate complete copy made after its paired dump, finished within `retention.blob_grace_hours` of that dump's start (24 hours by default). Immutable blobs may be copied live, but a failed or partial copy is not a backup | After every manual, pre-update or rollback dump. Keep each copy as long as its dump |
| `config/`, including `config/secrets/`, and `.env` | A copy, encrypted, off the host | After the first start and after any change |
| `runtime/node/` | A copy. It is the bundled node's identity; the token it enrolled with is spent, so a node that loses it cannot enroll again | After the first start |
| Managed volumes: workspaces, homes, a provider's `state` volume with its subscription logins | The driver's own volume backup on each host. Orch's source snapshots cover chosen paths in a thread directory, not a whole workspace, home, provider state, mount or outside effect | By your policy |

**Keep the master key.** Losing `config/secrets/orch_master_key` makes the API keys, authenticator secrets, mail passwords and bot tokens in a dump unreadable. Losing `orch_ca_key.pem` means enrolling every machine again. The master key cannot be rotated: a new random key does not re-encrypt the existing secrets, so keep the original with every database that needs it.

## 1. Take a backup

`./update.sh backup` checks a database dump, writes the matching `.manifest.json` and publishes a small protected record that **Management** reads. Pre-update saves use the same writer. It does not copy blobs, configuration, secrets or volumes. The API imports its record at start and before Management reads it, including after a save made while the API was stopped. Its completion proves the database pair only, not the separate copies below.

1. From `deploy/`, take the dump and note the name it prints.
2. Copy the dump, its manifest and the blobs as one pair. After a manual dump, for example:

   ```sh
   umask 077
   ./update.sh backup
   dump=$(ls -t runtime/update-backups/*-backup.dump | head -n 1)
   pair="/backup/orch/$(basename "${dump%.dump}")"
   install -d -m 700 "$pair"
   cp -p -- "$dump" "${dump%.dump}.manifest.json" "$pair/"
   cp -a -- runtime/data "$pair/data"
   ```

3. Check that the blob copy succeeded and finished within the grace. If it missed the window, take a new dump and a new copy, and do not call the old pair complete.
4. Copy the config, `.env` and `runtime/node/` (encrypted) off the host, and the pair with them.

**What you will see.** `backup` ends with the dump's name in `runtime/update-backups/` and a published record. `docker compose exec api /opt/orchctl/orchctl doctor` and **Management** show its age; `backup.warn_days` (2 by default) warns on the age of the last success. A failed attempt never makes the last success newer.

If the dump pair finished but publishing its record failed, run `./update.sh backup --repair-record <relative-dump-name>`. It checks the existing pair and repairs only its metadata, keeping its original cause and time. It does not run `pg_dump` again.

## 2. Run it every day

Put the steps in a script and run it from cron or a systemd timer, as the account that owns `deploy/`. This is an example, not a shipped file: adapt the paths and the retention to your policy.

```sh
#!/bin/sh
# /usr/local/sbin/orch-backup.sh
set -eu
cd /opt/orch/deploy
umask 077
./update.sh backup
dump=$(ls -t runtime/update-backups/*-backup.dump | head -n 1)
pair="/backup/orch/$(basename "${dump%.dump}")"
install -d -m 700 "$pair"
cp -p -- "$dump" "${dump%.dump}.manifest.json" "$pair/"
cp -a -- runtime/data "$pair/data"
```

```cron
# /etc/cron.d/orch-backup: every day at 02:30, as the account orch
30 2 * * * orch /usr/local/sbin/orch-backup.sh
```

Then alert on it: [Monitor an installation](monitoring.md#3-set-the-first-alerts) has the alert for a missing or old backup.

`update.sh` never deletes a dump by itself, because the dumps taken before updates are what a rollback goes back to. The directory grows by the size of the database each time. Retention removes a dump, its manifest and its paired blobs together: do not prune blob copies earlier, and do not overwrite an older pair with today's data. Include the dumps kept for rollback in the policy. This pairing protects the references between the database and the blobs, not a live outside database or a host volume: use that application's own backup too.

## 3. Remove a local pair

1. After you have checked a copy under `runtime/update-backups/`, record its pairing at once, with the dump's exact name and the copy's directory name:

   ```sh
   ./update.sh backup --repair-record <dump-name> --paired-blobs <copy-directory>
   ```

   It records the directory's identity and keeps the dump's original snapshot, cause and time. It makes no new dump or copy, and the dump’s recorded `retention.blob_grace_hours` window still applies.
2. Review the pair and remove it: `./update.sh dumps remove <dump-name>`. Type the name once. The dump, the manifest and the recorded copy go together. `--yes` applies a plan you have already reviewed.

A missing or changed pairing stays blocked. Copies you keep elsewhere need their own retention procedure. Removing the pair used for a rollback also removes that recovery point. An interrupted removal keeps its checkpoint for another review and never claims the pair is still restorable.

## 4. Put a system back

On a clean directory of the same release (a later release also works; the API applies its schema changes when it starts):

```sh
mkdir -p /opt/orch && tar -xzf orch-deploy-v0.1.0.tar.gz -C /opt/orch && cd /opt/orch/deploy
cp -a /backup/config/. config/                  # the original secrets come back with it
install -m 600 .env.example .env
install -m 600 /backup/.env .env
mkdir -p runtime && cp -a /backup/data runtime/data && cp -a /backup/node runtime/node
./update.sh first-start --from /path/to/the/release --restore-database /backup/orch.dump
```

Use the blob directory of this exact dump's completed pair, not the newest copy you have. `first-start` finds the secrets complete and makes none, starts PostgreSQL, puts the dump in the database before the stack starts, takes its own first dump of what it restored, starts the stack, and waits until `orchctl doctor` passes. (`--restore-database` is accepted by `first-start` and by no other command.) Machines reconnect with their existing certificates, because the authority is the same. Restore the relevant host-volume backups separately: a thread's source snapshot helps recover its recorded selection but does not replace the volume and sign-in backups.

- `.env` holds this directory's path (`ORCH_HOME`). Restore at the same path, or change that line.
- Whatever was written after the dump is not in it. That is also true of a rollback; [Update and roll back](upgrades.md) says what a rollback loses.
- The restore copy makes `.env` mode 0600 even if the backup lost its permissions. Keep the restored secret files at 0600 in `config/secrets/` at 0700. The updater corrects `.env` before reading it, or stops with `chmod 600 -- .env` when its owner must do that. `orchctl doctor --only secrets --env-file .env --secrets-dir config/secrets` names exposed files.

## 5. Practise a restore

Do this before you need it, and again after you change how you back up.

1. Take an isolated host or an empty directory, and a copy of one complete set.
2. Do not point your real DNS name at it, and do not let machines reach it: the restored system holds your machines' identities and would accept them.
3. Run the steps of section 4 against the set.
4. Check the result:
   - `./update.sh first-start` ends with `orchctl doctor` passing.
   - You can sign in as an administrator, and **Management → System** shows the release you expect.
   - A thread opens with its conversation, and a file that you know was recorded is there.
   - The age of the backup in **Management** is the restored dump's, not a new one.
5. Write down how long it took and what was missing. Remove the drill system afterwards.

## If it does not work

| You see | What to do |
|---|---|
| `update.sh` refuses `.env` | Run `chmod 600 -- .env` as its owner |
| `first-start` does not restore the database | Check the path you gave to `--restore-database`, and that the dump is a complete one from `runtime/update-backups/` (with its manifest) |
| Machines do not reconnect | The authority differs from the one that issued their certificates: restore `config/` from the same backup, not a new one |
| API keys, mail or bot tokens do not work after a restore | `orch_master_key` is not the original. It cannot be replaced; restore the original |
| The backup is reported as old | The last success is older than `backup.warn_days`. Run `./update.sh backup` and read its error |
