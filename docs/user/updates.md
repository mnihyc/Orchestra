---
group: operations
title: Update Orch and its machines
screens: /resources/machines, /resources/machines/$id, /resources/hosts, /resources/hosts/$id, /resources/environments, /resources/environments/$id, /admin/$section
keywords: Update now, update available, updating, version, release, upgrade, node, connector, update.auto, orch-node update, orch-connector update, server, SSH
summary: Compare the running and the recommended builds, then update machines, nodes or managed environments.
---

# Update Orch and its machines

A machine or a node compares its **running version** with the build that its own Orch server recommends. **Update available** marks a newer recommendation in the list, and the page names both versions. A file that was downloaded is not reported as the running version. That changes when the new process reconnects. This page shows how to update a machine or a node, what a machine can refuse, how environments and managed providers update, and how the server itself is updated.

## Machines and nodes

People with **Machine Manage**, or **Host Manage** for a node, can choose **Update now** on its page or in the actions of its row.

1. Orch holds new turns and waits until every running turn that uses the machine ends. For a node this includes the turns in every container it hosts.
2. The program downloads the build for its platform from its own server, checks its size and its SHA-256, and replaces the executable in one step.
3. The service manager starts the new program. This does not interrupt turns that exist.

The row says **Updating…** while the request waits, downloads, installs and reconnects. It clears when the new process reports its version. An update that is refused or fails keeps its reason on the row and on the page. A download that is damaged leaves the running program in place. Requests are recorded in the audit trail, automatic ones included.

Connectors and nodes on machines also request the recommended build by themselves on their hourly check, while the setting for automatic updates allows it. They use the same wait, verification and installation as the button. A connector inside an environment or a managed provider does not replace itself.

## When local policy refuses

The person at the machine controls automatic updates with `update.auto`. Explicit Update remains available while Auto is off and does not enable automatic updates. The operator can also request the same verified update locally:

```sh
orch-connector update
orch-node update
```

Use the command for the program on that machine. It leaves the saved `update.auto` value unchanged. Below the server's accepted minimum build, an authenticated program can obtain an idle update grant without a normal connection. The server first holds new turns and waits for all work using that machine, including every hosted unit. A machine disconnected for another reason must be able to reach its server again. An empty local program list is not proof that the hosted units are idle. If the programs no longer speak a common link protocol, the operator runs the local update command on that machine.

Below the minimum build that the server accepts, an update is required before normal work can resume. A node can get that update while its usual connection is refused, after the server has waited for every turn it hosts to end. It never takes an empty local list of agent programs to mean that its containers are idle. If the programs no longer speak a link protocol in common, the operator uses `orch-node update` on the host.

## Environments and managed providers

An environment keeps its own **Update** workflow. Its check beforehand shows the image, the connector and the other changes that are pending, and the running work that blocks a replacement. The manager of the environment reviews that plan and starts its job. Updating the node of the host does not replace the image or the connector of a container quietly. Managed providers likewise take their connector through the update of their container or at their next start. The connector of a directory belongs to its machine and follows the policy of that machine.

## The server

**Management → System** shows the version of the server that runs, the latest release read from the release address of the operator, a link to its notes, and the exact command to run on the host of the server, for example:

```sh
./update.sh v1.2.3
```

The default address is the GitHub releases of this project. An operator can set `[release] url` in `config/orch.toml`, or `ORCH_RELEASE_URL`, to use another release source that is compatible. The server caches reads for at least an hour, lookups that failed included. Scheduled notices stay under the control of `release.check_days`, and opening **System** reads the same hourly cache. If a lookup fails, its reason is shown beside the last result that succeeded, with its date.

A system administrator can choose **Update now** in **Management → System**.
The server files are updated by a job on the node bundled with this installation,
using the release’s existing `update.sh` fetch, verification, backup, restart,
health check and rollback. A second press joins the same running job. The page
keeps looking during the restart and shows the retained updated or rolled-back
outcome when the API returns, including after a page reload. Automatic checks
and notices never install a server release by themselves.

If the bundled node is missing, offline, or has no access to the host’s local
Docker, the row names that condition and keeps the operator command above.
The shell command remains the same updater and recovery path; `./update.sh recover`
resumes an interrupted journal. The API accepts no host command, executable path
or Docker socket and never replaces its own container.

The web edge stays available while the API updates its database. A page that is open shows the migration that is active, the steps completed, any counts of rows that the database can report, and the time elapsed, then reconnects when the server is ready. The same steps are printed in the update log of the API. Counts stay unknown when the database cannot report them.
