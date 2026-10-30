# Upgrades and recovery

How a release is installed over another, what happens when it does not become ready, how to go back, and how to
finish what a power cut left half done. A source checkout includes deployment scenarios under `scripts/smoke/`.

## Updating

Management → System shows the running server version, the latest release from
`[release] url` / `ORCH_RELEASE_URL` (this repository's GitHub releases by
default), its notes and the exact host command. Lookups, including failures,
share an hourly server cache. A failed lookup is shown plainly alongside the
last successful dated result. The page never runs the updater or receives a
Docker socket or write access to the host's release directories.

Physical `orch-connector` and `orch-node` programs use the same verified
self-update procedure. Their machine/node page and row actions offer Update
now to managers, and their hourly automatic check follows `update.auto`.
The server holds new turns and drains all work using the machine, including
every unit on a node, before granting installation. Size and SHA-256 are
checked before an atomic executable swap; the service manager restarts it.
Updating remains visible until a new-version hello, or its refusal/failure.
Local `update.auto = false` refuses web requests and names `orch-connector
update` or `orch-node update`, which the operator can run as explicit local
consent. Those commands leave the policy intact. A node below the accepted
minimum still proves its certificate; its rejection carries an idle update
grant only after the server has held new turns and drained all its units.
It receives no ordinary session or lifecycle commands on the old build.
A node with no common link protocol needs the local command; it never infers
hosted-unit idleness locally.
Managed environment/provider connectors retain their own reviewed runtime
Update workflow, which updates the image and connector together.

The first release requires a database made anew: databases from pre-release development builds are not
supported upgrade sources. After a release, its migrations remain immutable and later schema changes
are new numbered files; never rewrite an applied checksum to make an update start.

```sh
./update.sh v0.2.0 --from /path/to/the/release      # files in a directory
./update.sh v0.2.0                                  # or fetched from ORCH_RELEASE_URL
./update.sh latest
```

For `--from`, the directory needs `version.json`, the frontend archive, the server archive and the `orchctl`
binary for this host's architecture, as described in [Deployment](deployment.md#get-the-release). Before anything
is stopped, the updater checks those payloads and any other manifest-listed files present against `version.json`;
it unpacks the payloads beside the running ones. If running work or incompatible runtimes are found,
`update.sh` says what the update does and asks whether to go on (`--yes` answers it when nobody can be asked).
Prerelease connectors on the minimum's numeric version are reported for a compatibility check. With no affected
work it proceeds without a question. If the work inventory cannot be read, it stops before stopping services:

```
2 environments and 1 provider runtimes will need an update before they work again (release v0.2.0 no longer accepts their connector).
4 threads are running. Their turns are not interrupted and their events arrive after the services are back; but if that takes longer than the 60 s that open exec tunnels wait, those tunnels are reset: the commands in them end, and the agent sees them fail.
Go on? [y/N]
```

Then, as one transaction whose every step is written to `runtime/update/journal` before it is taken:

1. The application services are stopped. The database keeps running.
2. A dump of the database is taken and read back: `runtime/update-backups/orch-<time>-before-<tag>.dump`.
3. `server`, `frontend` and `cli` become the new release and the old ones become `previous`.
4. The services are made again and started; the API applies the schema changes of the release.
5. `orchctl doctor --expect-version` runs in the API's container until it passes: the API answers and reports
   the release, the gateway is connected to it, the worker beats, and the node has a session. Five minutes are
   allowed (`ORCH_READY_TIMEOUT` changes it).

The update replaces the three payloads. It keeps `compose.yml`, `nginx.conf`, `.env`, `config/orch.toml` and
existing secrets; it adds the port assertion key if an older complete secret set lacks it. When a release needs a
configuration change, its notes say so, and the new `orch-deploy` bundle holds the file to compare with yours.

### Keeping unused releases

System settings → **Keep unused releases** (`cleanup.release_keep_count`) starts
at 3. After a successful update or rollback, each payload keeps that many
additional unused releases, newest first. `current` and `previous` always stay.
Zero disables pruning and keeps all releases. A missing or invalid policy read
also keeps every release.

Only complete payload directories are eligible. Staging directories, symbolic
links and unrecognized directories stay. Dumps are never pruned automatically.
Keep each dump with its manifest and paired blob copy, as described in
[Backup and restore](backup-and-restore.md).

### What an update does to running work

| Component | During | After |
|-----------|--------|-------|
| Connected machines | Their sessions drop; events spool locally; tunnels wait out the 60 s grace | They reconnect and send what they spooled; they update themselves if the release raised `connector_minimum`, or when idle |
| Managed runtimes | Keep running; harness turns continue; their connectors spool | They reconnect as they are. Each unit that now holds something older says "Update available"; nothing in a unit changes until it is updated. A unit whose connector the release no longer accepts cannot reconnect and says "Update required" |
| Browsers | The socket closes | It reconnects, and offers a reload when the build changed |
| Turns in progress | Not interrupted | Their events arrive after reconnection |

### When the release does not become ready

The services are stopped, the links are put back, the database is made again from the dump that was just taken,
the old release starts and is verified, and `update.sh` exits non-zero saying so:

```
update.sh: putting release v0.1.0 back
...
all 6 checks passed
update.sh: release v0.1.0 is running again, with the database as it was before; the dump is runtime/update-backups/orch-20261011T182533Z-before-v0.2.0.dump
update.sh: release v0.2.0 did not become ready; v0.1.0 is running again as it was
```

That is what the scenario shows with a release whose second schema change cannot be applied: its first change
had already renamed the users' table, and after the failure the users' table is back and the administrator signs in.

## Agent program versions

Management → Agent programs lists each program and its builds, channel and
check result. Open a program to inspect its capabilities and version policy.
Select a status to inspect its interface
and native cases, named by the behavior they prove. Disabled features say
what happens instead, with the runner's diagnostic underneath. Native checks need an eligible host and the preloaded
isolated-check image. They use a scripted model, without model credentials
or external network access. Untested results name the missing prerequisite.
Native-import conformance exports and imports into a fresh program home, or,
for Codex, which keeps a thread's history in its own store, forks a
conversation in its home and reopens it in a copy of the home. A build that
fails it loses native import; Carry names that build and uses neutral import
where available. Otherwise, or when history will not
fit, recent history goes in the handoff notice with the full transcript as a
file. Profile overrides cannot restore the disabled import.

Choose Shipped, Newest checked, Newest (untested), or Exact. Exact accepts a
known version or fetches that exact vendor release with its checksum through
the configured Release mirror. A newly fetched build finishes its background
check before Use version is offered. Failed or untested completed targets
remain choices with a warning; checksum mismatches are refused.

System administrators set the system policy. Org and project settings
managers can set their own values; environment managers can pin an individual
environment. Ancestor locks apply. A profile's exact pin takes priority and
refuses a different running program, naming both versions. Changing any
value leaves the existing unit running until its reviewed update.

Select Update N environments… to review eligible units together, or Update… on selected
environments, providers or a host's Runtimes tab. The plan names each target
check, newer or older changes, skips, blockers and stopped work. Choose Wait
or Interrupt now once for the batch and review its running-turn count and
Notify audience. Environment and provider pools have separate confirmations
because their jobs can hold the same threads. Confirmed jobs continue after
Hide. Connected machines keep their owner's installed program.

For a build outside the vendor channels, copy it to an internal file server
and register its URL and SHA-256 in Agent programs, then run Check. Build
registration accepts metadata; the executable is fetched by the node.
See [Agent program versions](../user/agent-program-versions.md) for the
individual pin, downgrade and running-environment workflows.

## Going back

```sh
./update.sh rollback --yes
```

swaps `current` and `previous` with the same safeguards, and puts the database back as it was when the release
being entered was left, so that the payloads and the database state are compatible. **What was written since the
update is lost from the running system**: `update.sh` says so before it asks. It first keeps a dump of the
present database (`orch-<time>-before-rollback-from-<tag>.dump`), so that `./update.sh rollback` again goes
forward, to the release you left and with what was in it. Machines that were enrolled in between are unknown to
the restored database and have to be enrolled again; the bundled node, if it enrolled after the dump, is one of
them (a system that has been in use for a while has had it for long). Before stopping services, rollback reads the
previous server payload's `connectors/version.json` to check its connector minimum. If that manifest is missing or
does not match the installed record, restore the previous payload from its release files before retrying.

## After an interruption

A power cut or a Ctrl-C leaves the journal open. An update or rollback, or a first start encountering an
interrupted update, refuses to run until it is dealt with:

```
update.sh: a update to v0.2.0 was interrupted (step activating); run ./update.sh recover first
```

`./update.sh status` says what is open, and `./update.sh recover` finishes or undoes it:

| It stopped at | What `recover` does |
|---------------|---------------------|
| `begin`, `staged`, `stopping`, `dumping`, `activating` | Nothing ran the new release and the database is as it was: the links go back and the old release starts |
| `restoring_entered` (a rollback), `restoring`, `restarting` | The database was being replaced: it is made again from the dump of what was left, and the old release starts |
| `starting`, `verifying` | The new release may be fine: it is started and kept if it becomes ready and reports its release, undone like a failed update if not |
| `done` | The record is closed |

If a step such as stopping services or taking the backup fails, the journal can also remain open and services may
be stopped. Resolve the reported cause, then run `./update.sh recover` to bring the previous release back.

A first start that was cut off is taken up again by repeating the original `first-start` command, including
`--from` and any `--restore-database` option.
