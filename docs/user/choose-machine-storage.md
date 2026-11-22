---
group: resources
title: Choose where a machine keeps data
screens: /resources/templates, /resources/templates/$id, /resources/machines/$id, /resources/environments/$id, /resources/providers/$id
keywords: storage, disk, machine, host, template, env.storage, runtime.storage, workspace, home, state
summary: Use the named storages of a machine for the workspace, the home and the state of its environments and providers.
---

# Choose where a machine keeps data

Every machine reports its named storages with their capacity and the space that is free. Read **Storage** on its page, then use those names in **Container configuration**.

- The storage setting of an environment (`env.storage`) chooses where new data for the workspace, the home and the state of the connector is made. The state of a connector has its own storage where the driver makes a container. A connector that runs on a directory keeps its own private data directory on the machine and declares roles of storage for the workspace, the home and mounts.
- The storage setting of a managed provider (`runtime.storage`) chooses its home and its state, where its declared roles of data support them.
- A member that is left out uses the default of the machine. A name that the machine does not offer is refused. It never falls back quietly to another disk.

For example, `{"workspace":"fast","home":"bulk","state":"fast"}` chooses two named storages for an environment. A provider has no workspace, so use `{"state":"fast"}` for the private state of its connector. A separate home for a provider is used where its sources of data offer one.

Storage selects a place for new data that Orch owns. Directories that exist keep their identity, their permissions and their location. Data that Orch made already keeps the storage recorded for it, across restarts and edits of the template. Changing a storage does not move that data. To move the data of an environment that exists, use a move or a copy that you review first.

## Configure named storages

Nodes and directory-running connectors declare named paths in `[[storages]]` in
their data-directory `config.toml`. A node operator can use `orch-node policy set storages`
to write that same canonical policy. Each entry has `name`, `path` and `default` members. Paths must be existing directories
under the machine's allowed `environment.roots`; the machine checks that policy
before exposing or creating data. Exactly one declared storage is the default.

```toml
[[storages]]
name = "fast"
path = "/srv/work"
default = true
```

Without a list of storages for the connector, its allowed roots give the names: `default` for the first root, then `root-2`, `root-3` and so on. The connector makes the directories it owns beneath those roots. With no allowed root there is no storage to offer. Give disks names of their own if their order may change. The report exposes no path of the state of the connector and no credential.

## Replace host template paths

Templates for processes on a host hold an account that exists (**Run as user**) and commands, and can be shared across machines. They no longer hold the paths `workspace_base`, `home_base` or `mounts_base`.

1. Declare the old directories as named storages of the machine.
2. Choose the names for the workspace and the home in the storage setting of the environment.
3. Choose a storage when you add the location of each mount volume.
4. Remove the old members from the template.

Records of volumes that exist keep their exact paths. Orch does not relocate, adopt or purge directories while it applies this change of configuration. The form explains the fields of the driver. An example holds the account and the commands, and needs no knowledge of the layout of the files of a particular host.

## Set common defaults

Environments and managed providers use the same schema for each member they share: limits, storage, what placement needs, variables, secret names, the policy for updates and commands after start. They inherit from the system through the home of the resource, then from its own values. Limits on the runtime include disk in GB, subject to the quota the machine declares it supports. Managed providers have no workspace, no mounts for work, no shells or browsers of an environment, and no warning that an environment is idle.

## Data directory inventory

**Storage** of a machine also shows the observation of the data directory of the connector. The time of the observation says how old that report is. Each component listed has its apparent size, or a dash when it could not be measured. Scans that are incomplete, components that cannot be read or are links, and entries that are not listed stay visible as facts. An inventory that is unavailable differs from a directory that was fully inspected and is empty. These rows give no operands of the file system and no permission to clean up.

The sealed store shows only its immediate safe name and its size. It has no preview, no **Open**, no download, no name of a nested file and no credential value. The capacity of storage stays a separate report from these apparent sizes of files.
