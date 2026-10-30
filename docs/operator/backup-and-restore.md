# Backup and restore

Keep a recoverable set: one database dump and its manifest, a subsequent blob copy, the configuration and
original secret keys, and any execution volumes your recovery needs. Take the blob copy after **every**
dump, complete it within `[retention] blob_grace_hours` of that dump's start (24 hours by default), and
retain that copy for as long as its dump. A current blob copy cannot stand in for the copy of an older dump.

`./update.sh backup` validates a database dump, writes the matching `.manifest.json` and publishes a small
protected Overview record. Pre-update saves use the same writer. This command does **not** copy blobs,
configuration, secrets or execution volumes. The API imports its record at startup and before Management
reads it, including after a save made while the API was stopped. Its completion proves the database pair,
not completion of the separate copies below.

If the dump pair completed but publishing its Overview record failed, use `./update.sh backup --repair-record <relative-dump-name>`. This verifies the existing pair and repairs only its metadata, preserving its original manual/pre-update cause and completion time. It does not run `pg_dump` again. A failed attempt never makes the last successful backup newer; `backup.warn_days` warns on that success's age (two days by default).

Test a restore of the complete set on an isolated host before relying on it. Preserve the original authority
and machine identity so machines can reconnect; a database alone does not restore those identities or keys.

## What to keep

| What | How | How often |
|------|-----|-----------|
| The database | `./update.sh backup`: a custom-format dump, read back before it is kept, in `runtime/update-backups/`, taken without stopping anything | Daily; one is also taken before every update and rollback |
| Blobs, `runtime/data/` | A separate complete copy after its paired dump, completed within `retention.blob_grace_hours` from that dump's start; immutable blobs may be copied live, but a failed/incomplete copy is not a backup | After every manual, pre-update or rollback dump; retain each copy as long as that dump |
| `config/`, including `config/secrets/`, and `.env` | A copy, encrypted, off the host | After the first start and after any change |
| `runtime/node/` | A copy. It is the bundled node's identity; the token it enrolled with is spent, so a node that loses it cannot enroll again | After the first start |
| Managed volumes: workspaces, homes, a provider's `state` volume with its subscription logins | The driver's own volume backup on each host. Orch's source snapshots cover selected paths in a thread directory, not an entire workspace, home, provider state, mount or external effect | By your policy |

Losing `config/secrets/orch_master_key` makes the API keys, authenticator secrets, mail passwords and bot
tokens in a dump unreadable, and losing `orch_ca_key.pem` means enrolling every machine again: the dump alone
is not a backup of the system. **The master key cannot be rotated.** Replacing it with a new random key
does not re-encrypt existing secrets; keep the original key with every database that needs it.

A backup job must pair the exact dump it just made with its blob copy. For example, after noting the dump
name printed by `backup` (replace the angle-bracket placeholders with that exact name and your destination):

```sh
umask 077
./update.sh backup
dump='runtime/update-backups/<exact-dump-name>.dump'
pair='/backup/orch/<this-dumps-identifier>'
install -d -m 700 "$pair"
cp -p -- "$dump" "${dump%.dump}.manifest.json" "$pair/"
cp -a -- runtime/data "$pair/data"
```

`update.sh` never deletes a dump automatically: the ones taken before updates are what a rollback goes back to. The
directory grows by the size of the database each time. Verify that the blob copy succeeded and finished
within the grace before declaring this pair complete. If it misses that window, take a new dump and a new
copy; do not call the old pair complete. Copy the set off the host. Retention removes a dump, its manifest
and its paired blobs together; do not prune blob copies earlier or overwrite an older pair with today's data.
Include dumps kept for rollback in this policy. This pairing protects database/blob references, not the
consistency of a live external database or a host volume; use that application's backup method too.

## Removing a local pair

For a local copy under `runtime/update-backups/`, record its pairing immediately
after verifying the copy. Use the dump's exact name and a relative copy-directory
name. For example, `./update.sh backup --repair-record <dump-name> --paired-blobs <copy-directory>`.
Registration records the directory's identity and preserves the original dump's
snapshot, cause and time. It makes no new dump or copy. The existing
`retention.blob_grace_hours` window still applies.

Run `./update.sh dumps remove <dump-name>` to review that local pair and type its
name once. The dump, manifest and recorded copy go together. `--yes` applies an
already reviewed plan. A missing or changed pairing stays blocked. Copies kept
elsewhere need their own operator's retention procedure. Removing the pair used
for rollback also removes that recovery point. An interrupted removal keeps its
checkpoint for another review; it never claims the pair remains restorable.

## Putting a system back

On a clean directory of the same release (a later release also works; the API applies the schema changes it
brings when it starts):

```sh
mkdir -p /opt/orch && tar -xzf orch-deploy-v0.1.0.tar.gz -C /opt/orch && cd /opt/orch/deploy
cp -a /backup/config/. config/                  # the original secrets come back with it
install -m 600 .env.example .env
install -m 600 /backup/.env .env
mkdir -p runtime && cp -a /backup/data runtime/data && cp -a /backup/node runtime/node
./update.sh first-start --from /path/to/the/release --restore-database /backup/orch.dump
```

`first-start` finds the secrets complete and makes none, starts PostgreSQL, puts the dump in the database
before the stack starts (this is what `--restore-database` is for; it is accepted by `first-start` and by no
other command), takes its own first dump of what it restored, starts the stack, and waits until
`orchctl doctor` passes. Machines reconnect with their existing certificates, because the authority is the
same.

The restore copy creates `.env` with mode 0600 even if a backup lost its permissions. Keep the restored
secret files at mode 0600 in `config/secrets/` at mode 0700. The updater corrects `.env` before reading it,
or refuses with `chmod 600 -- .env` when its owner must do that. Run
`orchctl doctor --only secrets --env-file .env --secrets-dir config/secrets` to name exposed files.

Use the blob directory belonging to this exact dump's completed pair, not the newest copy available.
Restore the relevant host-volume backups separately. A thread source snapshot can help recover its
recorded selection, but does not replace the volume and sign-in backups in the table.

Two things to know:

- `.env` holds this directory's path (`ORCH_HOME`). Put the system back at the same path, or change that line.
- Whatever was written after the dump is not in it. That is the case for a rollback too, and
  [the upgrade guide](upgrades.md) says what is lost with it.
