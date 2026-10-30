---
title: Choose where a machine keeps data
screens: /resources/templates, /resources/templates/$id, /resources/machines/$id, /resources/environments/$id, /resources/providers/$id
keywords: storage, disk, machine, host, template, env.storage, runtime.storage, workspace, home, state
---

# Choose where a machine keeps data

Every machine reports its named storages with their capacity and free space.
Read **Storage** on its page, then use those names in **Unit configuration**.
`env.storage` chooses where new workspace, home and connector state data is
made. Separate connector-state storage applies where the driver makes a unit;
a directory connector retains its private machine data directory and declares
workspace, home and mount storage roles. `runtime.storage` chooses a provider's
home and state data where its declared data roles support them. An omitted
member uses the machine's default. A name that machine does not offer is
refused; it never silently falls back to another disk.

For example, `{"workspace":"fast","home":"bulk","state":"fast"}` chooses
two named storages for an environment. A provider has no workspace, so use
`{"state":"fast"}` for its private connector state. A provider's separate
home selection is used where its data-source facts offer one.

Storage selects a place for new owned data. Existing directories retain their
identity, permissions and location. Data already made by Orch keeps its recorded
storage across restarts and template edits. Changing a storage does not move
that data: use a reviewed move or copy operation for an existing environment.

## Configure named storages

A node operator declares names and paths in `[[node.storage]]` in `orch.toml`.
A directory-running connector uses `[[storage]]` in its `config.toml`, with the
same `name`, `path` and `default` members. Paths must be existing directories
under the machine's allowed `environment.roots`; the machine checks that policy
before exposing or creating data. Exactly one declared storage is the default.

```toml
[[storage]]
name = "fast"
path = "/srv/work"
default = true
```

Without an explicit connector storage list, its allowed roots supply storage
names: `default` for the first root, then `root-2`, `root-3` and so on. The
connector makes its owned directories beneath those roots. With no allowed
root there is no storage to offer. Give disks explicit names if their ordering
may change. No connector state or credential path is exposed in the report.

## Replace host template paths

Host-process templates contain an existing **Run as user** account and commands,
and can be shared across machines. They no longer contain `workspace_base`,
`home_base` or `mounts_base` paths. Declare the old directories as named machine
storages, choose the workspace and home names in `env.storage`, and choose a
storage when adding each mount volume location. Remove the old members from the
template. Existing volume records retain their exact paths; Orch does not
relocate, adopt or purge directories while applying this configuration change.

The form explains the driver's fields. An example contains the account and
commands; it needs no knowledge of a particular host's filesystem layout.

## Set common defaults

Environment and provider runtime settings use the same schema for each common
member: limits, storage, placement needs, variables, secret names, update policy
and commands after start. They inherit from the system through the resource's
home and then its own values. Runtime limits include disk in GB, subject to the
machine's declared quota support. Provider runtimes have no workspace, work
mounts, environment shells or browsers, or environment idle warning.
