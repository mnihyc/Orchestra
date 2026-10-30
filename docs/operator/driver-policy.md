# Driver policy and host scope

A host is an administrative destination. A driver can manage the node’s own
machine or report one remote pool. Orch does not add the capacities of overlapping
drivers. A remote pool cannot be combined with another resource authority in the
same host registration. Register distinct hosts for distinct resource authorities.

The node’s machine policy lives in `config.toml` under its data directory. The
older driver, storage and hook tables in `orch.toml` are imported once; subsequent
policy changes use the machine policy. See [node policy import](node-hooks.md).

## Source and execution ceilings

`drivers.allowed` permits registered implementation identifiers. It does not
install or register an external driver. `drivers.images` sets image, registry,
build, archive, capability, runtime-class and privileged-execution ceilings for
every driver offering image sources. `drivers.processes` bounds templates,
accounts and launch programs for drivers offering local accounts.

For example, an operator can allow one registry and image while refusing builds,
archives and privileged execution:

```toml
[drivers.images]
images = ["registry.example.test/team/agent:1"]
registries = ["registry.example.test"]
capabilities = []
runtime_classes = []
privileged = false
builds = false
archives = false
```

Docker template bind sources must resolve inside this machine's
`environment.roots`. A symlink to a location outside those roots is refused.
Template cache volumes cannot use names beginning with `orch-`, which are
reserved for managed resources. Use a separate name for an external cache.

Builds and archives also require unrestricted image and registry rules because
their fetched or loaded image identities cannot all be checked in advance.
Older `drivers.docker` and `drivers.host` policy keys remain readable, preserving
their restrictions. Newly written policy uses `images` and `processes`; do not
supply both spellings for the same section.

## Driver-owned options

`drivers.configurations` is an open map keyed by the registered driver identifier.
Each value is an object checked against that driver’s declared configuration
schema before the driver is activated. A driver without a configuration schema
does not accept nonempty connection options. Use the implementation’s documented
keys; Orch supplies no common endpoint or authentication fields.

When the node reports a configuration schema, the machine's Policy page checks a
changed connection against it before sending the policy command and shows the
invalid paths without repeating supplied values. A registered built-in driver may
report its schema even when it cannot load; that report does not make the driver
available for scheduling. An unreported schema is unknown, not a successful check.

The machine file stores each value as JSON text so nulls and nested arrays survive
a reload. Earlier TOML object tables remain readable. When a drivers policy value
is handed over, its complete value is likewise JSON text under `policy.values`.
Link and HTTP policy values remain ordinary JSON objects.

Changing, adding or removing connection options requires reopening the driver.
Restart the node after saving them. Until then, new image, volume and unit work
that would use the changed connection is refused. Existing cleanup uses its
already-open client rather than following a changed endpoint.

## Observe support and resources

Read **Overview → Details → Drivers → Features** on the host. Unavailable operations
include their declared reasons. Support does not grant permission; host listeners
also need the local port policy and the host’s publish configuration.

A driver-scoped pool supplies its own capacity, load and named storages. Failed
or missing remote measurements stay unknown; the node’s local filesystem or CPU
is not substituted. A storage name can identify a storage class. Unknown size or
free space is shown as a dash, not zero.

The [driver development guide](../development/extensions/drivers.md) describes
the contract.

## Tool-cache publications

The node verifies a download's checksum before publishing it from private scratch
storage. New cache paths and marker files do not grant write access to other users.
A published version is kept in place while units use it; a concurrent publication
compares its checksum rather than replacing it.

An incomplete existing entry is preserved and the operation is refused. Check that
entry and the units using it before retrying. Publish a verified new version when
replacing a build, and retain the old version until its units no longer refer to it.
Changing permissions on an older, writable cache alone does not verify its contents.
