# Update and roll back

[简体中文](zh-CN/upgrades.md)

How to install a release over another, what happens when it does not become ready, how to go back, and how to finish what a power cut left half done. A source checkout includes deployment scenarios under `scripts/smoke/`.

## Update

Each new key has the form `opat_<key id>_<secret><checksum>`. The ID is a public UUID, the secret contains 32 random bytes in unpadded base64url, and the final six base62 characters encode a CRC32 checksum. Use the UUID to find its list or audit entry and to revoke it. Keep the whole bearer private. The one-time issuance response is the only place to copy it. [API key format and offline checksum](api-with-a-key.md#account-api-key-format)

Browser push is set up automatically when its deployment key is usable; each person still chooses whether to enable notifications for their own browser.

Management → System shows the version of the server that runs, the latest release from the release address (this project's GitHub releases by default; `ORCH_RELEASE_URL` or `[release] url` change it), its notes and the exact command for the host. Lookups, failures included, share an hourly cache in the server. A lookup that failed is shown plainly beside the last one that worked, with its date. The page never runs the updater, and it gets no Docker socket and no write access to the release directories of the host.

An authenticated program below the accepted minimum can obtain its update grant without first
establishing an ordinary connection, after the server has held new turns and drained all work
using that machine, including every hosted unit. It receives no ordinary session or lifecycle
commands on the old build. A program with no common link protocol needs its local update command;
it never infers hosted-unit idleness from an empty local program list.

The first release requires a database made anew: databases from pre-release development builds are not
supported upgrade sources. After a release, its migrations remain immutable and later schema changes
are new numbered files; never rewrite an applied checksum to make an update start.

```sh
./update.sh v0.2.0 --from /path/to/the/release      # files in a directory
./update.sh v0.2.0                                  # or fetched from the release address
./update.sh latest
```

For `--from`, the directory needs `version.json`, the frontend archive, the server archive and the `orchctl`
binary for this host's architecture, as described in [Deployment](deployment.md#get-the-release), and should hold
`orch-deploy-<tag>.tar.gz`, from which the deployment files follow the release (without it nothing of them
follows, and the update says so). Before anything is stopped, the updater checks those payloads and any other
manifest-listed files present against `version.json`; it unpacks the payloads beside the running ones and decides
what becomes of the [deployment files](#deployment-files). If running work or incompatible runtimes are found,
`update.sh` says what the update does and asks whether to go on (`--yes` answers it when nobody can be asked).
Prerelease connectors on the minimum's numeric version are reported for a compatibility check. With no affected
work it proceeds without a question. If the work inventory cannot be read, it stops before stopping services:

```
2 environments and 1 managed provider will need an update before they work again (release v0.2.0 no longer accepts their connector).
4 threads are running. Their turns are not interrupted and their events arrive after the services are back; but if that takes longer than the 60 s that open exec tunnels wait, those tunnels are reset: the commands in them end, and the agent sees them fail.
Go on? [y/N]
```

The update then runs as one transaction. Every step is written to `runtime/update/journal` before it is taken:

1. The application services stop. The database keeps running.
2. A dump of the database is taken and read back: `runtime/update-backups/orch-<time>-before-<tag>.dump`.
3. The deployment files the release changes are written (the earlier ones are kept for a rollback).
4. `server`, `frontend` and `cli` become the new release and the old ones become `previous`.
5. The services are made again and started; the API applies the schema changes of the release.
6. `orchctl doctor --expect-version` runs in the API's container until it passes: the API answers and reports
   the release, the gateway is connected to it, the worker beats, and the node has a session. Five minutes are
   allowed (`ORCH_READY_TIMEOUT` changes it). `./update.sh doctor` runs the same checks once, at any time.

The update replaces the three payloads and the deployment files that follow the release (next section). It keeps
`.env`, `config/orch.toml` and every secret; it adds the port assertion key if an older complete secret set lacks it.
When a release needs a configuration change in `config/orch.toml`, its notes say so, and the new `orch-deploy`
bundle holds the file to compare with yours. Before enabling a [human check](human-check.md), the server checks the public edge. Run `orchctl doctor --url` to diagnose a retained edge configuration.

### Deployment files

The release's `orch-deploy` bundle holds the files this directory runs from: `compose.yml`,
`compose.frontend-https.yml`, `nginx.conf`, `frontend.sh`, `postgres/Dockerfile`, the scripts under
`hosts/runtimes/`, `update.sh` and a few pages and catalogues (`README.md`, `.env.example`, `install/`). They follow
the release. Yours stay yours: `.env`, everything under `config/` (`orch.toml`, the secrets, the authority PEMs) and
`runtime/` are never touched.

An update writes them in its own transaction, after the dump and before the services start under the new release,
and a rollback or `recover` puts them back. Each file is a file the product shipped, or your edit of one:

- The bundle lists the SHA-256 of every file of every release with a managed deployment bundle. A file that equals one of
  them is the product's and is replaced by the new release's.
- A file that equals none is yours. The files that have to agree with each other are one set (`compose.yml`,
  `compose.frontend-https.yml`, `nginx.conf`, `frontend.sh`, `postgres/Dockerfile`, `hosts/runtimes/*`), replaced
  whole when every member is a published file. If you edited any member, the whole set is kept, and each member that
  differs from the new release's gets the shipped file beside it as `<name>.new`. The other files are replaced or kept
  one by one.
- One line per file says what happened. `./update.sh status` lists every kept file until its `.new` file is gone:
  merge what you need, then remove the `.new` file. A later update writes a `.new` again only when the shipped file
  has changed since the one you were shown.

Each release also states the oldest generation of these files its programs work with (`deploy_files.minimum` in
`version.json` and in the bundle's `FILE_SETS`). It is raised only when a release needs a change in them, and the
release notes give the reason. If the files that would be in use, yours kept or older ones, are older than that, the
update stops before it stops anything and names the `.new` files to merge. When you have merged and removed them, the
update goes on. (The routes `/openapi/` and `/openapi/v1/ws` that the keys need, added to `nginx.conf` in v0.1.2, are the
kind of change that raises it.)

Before it replaces the set, the update checks three things, and keeps the set when one fails, saying in one line what
it found and what to do: the PostgreSQL major version of the new `postgres/Dockerfile` is the data directory's
(`PG_VERSION` in the selected PostgreSQL data directory; another major needs a dump and restore, the release notes say how); the new
`compose.yml` binds the same directories of `runtime/` and `config/` as the one in use and names the same project; and
every image it names is on this host, pulled during the check if it is not (a host without a registry loads them first,
see [Offline](offline.md)).

`update.sh` is replaced by the release's at the end of a successful update, in one rename, and is never rolled back:
the newer script performs a rollback to the earlier release and stays. An `update.sh` you edited is kept, with the
shipped one as `update.sh.new`. A rollback puts back the files the last update replaced, each only if it is still what
the update wrote; one you edited since stays, and the command names it.

#### Verifying a replacement updater

To replace the updater itself, use the standalone `update.sh` from a release whose notes give its SHA-256. Check it against the
digest in the notes, install it and run it. `<tag>` is the release, `<sha256>` the digest from its notes,
and `<base>` the release site (`ORCH_RELEASE_URL`, `https://github.com/mnihyc/Orchestra/releases` by default):

<!-- first-hop -->
```sh
cd /opt/orch/deploy && test ! -e runtime/update/journal \
  && curl -fsSL -o update.sh.next "<base>/download/<tag>/update.sh" \
  && echo "<sha256>  update.sh.next" | sha256sum -c - \
  && install -m 755 update.sh.next update.sh && rm update.sh.next
./update.sh <tag>
```
<!-- /first-hop -->

It refuses while an update is unfinished (run `./update.sh recover` with the script you have first), and leaves your
`update.sh` alone if the digest does not match (a leftover `update.sh.next` is safe to remove). The digest in the notes
is committed with the release's source before it is built, so it is a second copy of the checksum that does not come
with the download it checks. It is a checksum and not a signature: the notes and the file are both published through
the same repository, and the manifest's BLAKE3 hash of the file protects against damage only.

Databases from development builds made before release 0.1 are not supported sources for an update. After a release, its migrations stay as they are, and later changes of the schema are new numbered files. Never rewrite the checksum of a migration that was applied to make an update start.

### Update machines and nodes

The connector and node programs on machines use the same verified procedure to update themselves. The page of a machine or node, and its row actions, offer **Update now** to managers, and their hourly automatic check follows the setting `update.auto`.

- The server holds new turns and lets all the work that uses the machine finish, including every container on a node, before it allows the installation.
- The size and the SHA-256 are checked before the program is replaced in one step, and the service manager restarts it. The update stays visible until the new version says hello, or until it is refused or fails.
- Explicit web Update remains available with Auto off. Both the web action and the local commands `orch-connector update` and `orch-node update` leave the saved automatic-update policy in place.
- A node below the minimum still proves its certificate. Its rejection carries a grant to update only after the server held new turns and let all its containers finish, and the node gets no session or lifecycle commands on the old build. A node with no link protocol in common needs the local command, and it never decides alone that its hosted containers are idle.
- Managed connectors of environments and providers keep their own reviewed workflow, **Update**, which updates the image and the connector together.

### Keep unused releases

The system setting **Keep unused releases** starts at 3. After an update or a rollback that worked, each payload keeps that many more releases that are not in use, the newest first. `current` and `previous` always stay. Zero turns pruning off and keeps all releases. A read of the policy that is missing or invalid also keeps every release.

Only payload directories that are complete are eligible. Staging directories, symbolic links and directories it does not recognize stay. Dumps are never pruned automatically. Keep each dump with its manifest and the copy of the blobs that goes with it, as [Back up and restore](backup-and-restore.md) describes.

### What an update does to running work

| Component | During | After |
|---|---|---|
| Connected machines | Their sessions drop. Events spool locally. Tunnels wait out the grace of 60 s | They reconnect and send what they spooled. They update themselves if the release raised `connector_minimum`, or when they are idle |
| Managed environments and providers | They keep running. Turns of agent programs go on. Their connectors spool | They reconnect as they are. A container that now holds an older version says "Update available". Nothing in a container changes until it is updated. A container whose connector the release no longer accepts cannot reconnect and says "Update required" |
| Browsers | The socket closes | It reconnects, and offers a reload when the build changed |
| Turns in progress | Not interrupted | Their events arrive after the reconnection |

### When the release does not become ready

The services stop, the links are put back, the database is made again from the dump that was just taken, the old release starts and is verified, and `update.sh` ends with an error and says so:

```
update.sh: putting release v0.1.0 back
...
all 6 checks passed
update.sh: release v0.1.0 is running again, with the database as it was before; the dump is runtime/update-backups/orch-20261011T182533Z-before-v0.2.0.dump
update.sh: release v0.2.0 did not become ready; v0.1.0 is running again as it was
```

That is what the scenario shows with a release whose second change of schema cannot be applied. Its first change had already renamed the table of the users, and after the failure the table is back and the administrator signs in.

## Update agent programs

The versions of agent programs, their checks and how a version is pinned are in [Agent program versions](../user/agent-program-versions.md). Two facts matter for an update of Orch:

- A release can add test cases to the manifest of a program. A build whose stored check passed every case it had stays **Checked**, with the new cases pending ("Checked · N checks pending"). Within one look of the release schedule (every fifteen minutes; an interval of zero turns it off, like the reads of channels), the background runs the pending cases of every build kept that was checked before, whatever its age.
- A host whose node is of an earlier release does not have the new cases. Its check is not a failure: the build stays **Checked**, and the result says "not run by this node's version" until a host with the updated node runs them. Update standalone nodes together with the server. A check that could not run at all (no host, or a failed runner) leaves the last whole check as it was and says that the later check did not run. The builds that a release pins itself are not kept rows: the release certifies them with its manifest, and they show **Checked** from the day it is installed.

To use a build from outside the vendor channels, copy it to an internal file server, register its URL and SHA-256 in **Agent programs**, then run **Check**. Registering a build takes the details; the node fetches the program.

## Go back

```sh
./update.sh rollback --yes
```

swaps `current` and `previous`, and puts back the [deployment files](#deployment-files) the last update replaced (each only
where it still is what the update wrote; `update.sh` stays the newer one), with the same safeguards, and puts the database back as it was when the release
being entered was left, so that the payloads and the database state are compatible. **What was written since the
update is lost from the running system**: `update.sh` says so before it asks. It first keeps a dump of the
present database (`orch-<time>-before-rollback-from-<tag>.dump`), so that `./update.sh rollback` again goes
forward, to the release you left and with what was in it. Machines that were enrolled in between are unknown to
the restored database and have to be enrolled again; the bundled node, if it enrolled after the dump, is one of
them (a system that has been in use for a while has had it for long). Before stopping services, rollback reads the
previous server payload's `connectors/version.json` to check its connector minimum. If that manifest is missing or
does not match the installed record, restore the previous payload from its release files before retrying.

## After an interruption

A power cut or Ctrl-C leaves the journal open. An update or a rollback, or a first start that meets an update that was interrupted, refuses to run until it is dealt with:

```
update.sh: a update to v0.2.0 was interrupted (step activating); run ./update.sh recover first
```

`./update.sh status` says what is open, and `./update.sh recover` finishes it or undoes it:

| It stopped at | What `recover` does |
|---------------|---------------------|
| `begin`, `staged`, `stopping`, `dumping`, `filing`, `activating` | Nothing ran the new release and the database is as it was: the files and the links go back and the old release starts |
| `restoring_entered` (a rollback), `restoring`, `restarting` | The database was being replaced: it is made again from the dump of what was left, and the old release starts |
| `starting`, `verifying` | The new release may be fine: it is started and kept if it becomes ready and reports its release, undone like a failed update if not |
| `done` | The record is closed |

If a step such as stopping services or taking the backup fails, the journal can also stay open and services may be stopped. Resolve the cause that is reported, then run `./update.sh recover` to bring the previous release back.

A first start that was cut off is taken up again by repeating the original `first-start` command, with `--from` and any `--restore-database` option.
