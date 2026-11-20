# Restrict what a host's drivers may do

[简体中文](zh-CN/driver-policy.md)

Decide which images, registries, builds and local accounts the drivers of a host may use, and set the connection options a driver needs. At the end the node refuses anything outside your policy, and you can read what each driver supports.

A host is an administrative destination. A driver can manage the node's own machine or report one remote pool. Orch does not add the capacities of overlapping drivers, and a remote pool cannot share a host registration with another resource authority: register distinct hosts for distinct resource authorities.

## The bundled node

First start copies [config/node.toml](../../deploy/config/node.toml) to
`runtime/node/config.toml`, the bundled node's canonical machine policy. Repeating
first start and later updates preserve that file. It initially permits the Docker
driver, Dockerfile builds and image archives, with unrestricted image, registry,
capability and runtime-class rules. Privileged execution remains off.

Image builds are controlled by `[drivers.images] builds` in that machine file.
They also require `images = ["*"]` and `registries = ["*"]`, because a Dockerfile
can fetch base images. The node's live policy enforces these rules; set `builds = false`
there to refuse builds. The deployment's `config/orch.toml` keeps Docker connection
settings such as its socket and build arguments.

## Before you start

You need root on the host, `orch-node` installed there ([Install Orch](deployment.md)), and the registered identifiers of the drivers you will configure. The node's machine policy lives in `config.toml` under its data directory. Use `orch-node policy` to change the registered machine policy.

## 1. Choose the ceilings

| Key | What it limits |
|---|---|
| `drivers.allowed` | Which registered implementations may run. It does not install or register an external driver |
| `drivers.images` | Image, registry, build, archive, capability, runtime-class and privileged-execution ceilings for every driver that offers image sources |
| `drivers.processes` | Templates, accounts and launch programs for drivers that offer local accounts |

For example, to allow one registry and one image and refuse builds, archives and privileged execution:

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

Builds and archives also require unrestricted image and registry rules because
their fetched or loaded image identities cannot all be checked in advance.

## 2. Apply it

1. Read the policy that is in force: `orch-node policy show`.
2. Set the drivers value. The command replaces the whole value, so include the choices you keep:

   ```sh
   orch-node policy set drivers '{"images":{"images":["registry.example.test/team/agent:1"],"registries":["registry.example.test"],"capabilities":[],"runtime_classes":[],"privileged":false,"builds":false,"archives":false}}'
   ```

3. Restart the node when you changed connection options (below).
4. Check it: `orch-node policy show` prints the value, and a template that asks for something outside it is refused, with the reason on the environment.

The machine file stores each value as JSON text so nulls and nested arrays survive
a reload. When a drivers policy value is handed over, its complete value is likewise JSON text under `policy.values`.
Link and HTTP policy values remain ordinary JSON objects.

## 3. Set a driver's own options

`drivers.configurations` is an open map keyed by the registered driver identifier. Each value is an object that Orch checks against the driver's declared configuration schema before it activates the driver. A driver with no schema accepts no non-empty connection options. Use the implementation's documented keys: Orch supplies no common endpoint or authentication fields.

- When the node reports a schema, the machine's **Policy** page checks a changed connection against it before sending the command and shows the invalid paths without repeating the values you typed. A built-in driver may report its schema even when it cannot load, and that does not make it available for scheduling. An unreported schema is unknown, not a successful check.
- The machine file stores each value as JSON text so nulls and nested arrays survive a reload. Earlier TOML tables remain readable. A handed-over drivers value is likewise JSON text under `policy.values`. Link and HTTP policy values stay ordinary JSON objects.
- Changing, adding or removing connection options needs the driver to be opened again: **restart the node** after saving. Until then, new image, volume and container work that would use the changed connection is refused. Existing cleanup keeps using its already open client and does not follow a changed endpoint.

## Read what a host supports

Open **Overview → Details → Drivers → Features** on the host. Unavailable operations show their declared reasons. Support does not grant permission: host listeners also need the local port policy and the host's publish configuration.

A driver's own pool supplies its own capacity, load and named storages. A failed or missing measurement stays unknown, and the node's local filesystem or CPU is never put in its place. A storage name can identify a storage class. Unknown size or free space shows as a dash, never as zero. The contract is in the [driver development guide](../development/extensions/drivers.md).

## Publish a tool-cache build

The node checks a download's checksum before it publishes it from private scratch storage. New cache paths and marker files give no write access to other users. A version in use stays where it is, and a concurrent publication compares its checksum and does not replace it.

- An incomplete existing entry is kept and the operation is refused. Check that entry and the containers that use it before you retry.
- To replace a build, publish a verified new version and keep the old one until no container refers to it.
- Changing permissions on an older, writable cache does not verify its contents.

## If it does not work

| You see | What to do |
|---|---|
| A template is refused | It asks for an image, registry, build, archive or privilege outside `drivers.images`. Read the reason on the environment, then change the template or the policy |
| A connection change has no effect | Restart the node |
| A build or archive is refused although `builds` is true | The image and registry rules are not unrestricted. Builds and archives need that |
| The Policy page shows invalid paths | The value does not match the driver's schema. Fix the paths it lists |
