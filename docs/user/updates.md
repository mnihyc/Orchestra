---
group: operations
title: Update Orch and its machines
screens: /resources/machines, /resources/machines/$id, /resources/hosts, /resources/hosts/$id, /resources/environments, /resources/environments/$id, /admin/$section
keywords: Update now, update available, updating, version, release, upgrade, node, connector, update.auto, orch-node update, orch-connector update, server, SSH
summary: Compare running and recommended builds, then update machines, nodes or managed units.
---

# Update Orch and its machines

A machine or node compares its **running version** with the build recommended
by its own Orch server. **Update available** marks a newer recommendation in
the list; the page names both versions. A downloaded file is not reported as
the running version: that changes when the new process reconnects.

## Machines and nodes

People with Machine Manage, or Host Manage for a node, can choose **Update
now** on its page or in the row's actions. Orch holds new turns and waits
until every running turn using the machine ends. For a node this includes
turns in every unit it hosts. The program downloads its platform's build from
its own server, checks its size and SHA-256, and swaps the executable
atomically. Its service manager starts the new program. Existing turns are
not interrupted by this procedure.

The row says **Updating…** while the request waits, downloads, installs and
reconnects. It clears when the new process reports its version. A refused or
failed update keeps its reason on the row and page. A damaged download leaves
the running program in place. Requests, including automatic ones, are audited.

Physical connectors and nodes also request the recommended build automatically
on their hourly check while `update.auto` permits it. They use the same drain,
verification and installation as the button. An internal connector in an
environment or provider runtime does not replace itself.

## When local policy refuses

The person at the machine controls automatic updates with `update.auto`. Explicit Update remains available while Auto is off and does not enable automatic updates. The operator can also request the same verified update locally:

```sh
orch-connector update
orch-node update
```

Use the command for the program on that machine. It leaves the saved `update.auto` value unchanged. Below the server's accepted minimum build, an authenticated program can obtain an idle update grant without a normal connection. The server first holds new turns and waits for all work using that machine, including every hosted unit. A machine disconnected for another reason must be able to reach its server again. An empty local program list is not proof that the hosted units are idle. If the programs no longer speak a common link protocol, the operator runs the local update command on that machine.

## Environments and provider runtimes

An environment keeps its existing **Update** workflow. Its preflight shows
the image, connector and other pending changes and the running work that
blocks replacement. The environment manager reviews that plan and starts its
job. Updating the host node does not silently replace a unit's image or its
connector. Provider runtimes likewise take their connector through their own
runtime update or next start. A directory's connector belongs to its physical
machine and follows that machine's policy.

## The server

**Management → System** shows the running server version, the latest release
read from the operator's release address, its notes link, and the exact command
to run on the server's host, for example:

```sh
./update.sh v1.2.3
```

The default address is this repository's GitHub releases. An operator can set
`[release] url` in `config/orch.toml` or `ORCH_RELEASE_URL` to use another
compatible release source. Reads are cached by the server for at least an
hour, including failed lookups. Scheduled notices remain controlled by
`release.check_days`; opening System reads the same hourly cache. If a lookup
fails, its reason is shown beside any previously successful, dated result.

The server has no web button that installs its own update. Such a button would
need authority over the host's files or Docker socket. The operator runs the
transactional `update.sh` command on that host; it verifies the release, takes
the database backup and rolls back if the new services fail readiness. The
operator's upgrades and recovery guide covers backups and rollback.

The web edge stays available while the API updates its database. An open page
shows the active migration, completed steps, any row counts the database can
report, and elapsed time, then reconnects when the server is ready. The same
steps are printed in the API's update log. Counts remain unknown when the
database cannot report them.
