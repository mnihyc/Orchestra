# Node hooks and unit networks

The host operator can run a program before and after a unit starts or stops. This is the place to connect the
host's routing, firewall or monitoring policy to a unit's life. The hook runs with the node's rights. A kept
value is changed on the machine; handing over this key lets its machine managers change it from Orch.

Keep absolute executable paths in the node's machine policy:

```sh
orch-node policy set hooks '{"pre_up":"/etc/orch/hooks/check-tunnel.sh","post_up":"/etc/orch/hooks/route-through-vpn.sh","pre_down":"/etc/orch/hooks/route-through-vpn.sh","timeout_secs":30}'
orch-node policy show
```

The command writes the node's data-directory `config.toml`, with explicit local ownership. On upgrade,
the previous `[node.hooks]`, `[ports]`, `[[node.storage]]` and driver tables are still accepted and imported
once as kept policy values. The console names the replacement command. Later edits use `orch-node policy`;
the old file is no second source and cannot overwrite a newer local policy value.

For a new deployment's initial network pool, the compatible node configuration still accepts:

```toml
[node.docker.networks]
pool = "10.199.0.0/16"
prefix = 28
```

All hooks are optional. Each program is executed directly, with one JSON document on standard input.
The pool must avoid your existing host networks. Every Docker unit has its own subnet from this pool;
a restricted unit's proxy also has its own outward network. Docker's global address pools are unchanged.
An older open container leaves Orch's old common bridge at its next start, retaining that same container
and its own files. An offline node is not proof that a unit stopped.

| Moment | What has happened | If the hook fails |
|--------|-------------------|-------------------|
| pre_up | The unit network exists; the unit has not started | It does not start. Its managers see the bounded first lines. |
| post_up | The unit started | Its down hooks run and the unit stops; start fails. |
| pre_down | The unit is about to stop or be removed | Failure is reported; stopping continues. |
| post_down | It stopped, was removed, or ended by itself | Failure is reported. Removed units use the captured former networks. |

The timeout applies to the entire process group. Output is drained, bounded to 16 KiB per stream, and is
not copied into history. The hook inherits ordinary process settings such as PATH and LANG, without
the node's other environment variables. A run is a `unit.hook_ran` history line on its environment or
provider, with moment, reason, exit, timeout/failure, duration and time.

The document has version 1, event, reason, replay and time; host/system name; runtime id, kind and driver;
unit name and driver id; environment id, name, full place path, template and generation, or provider id
and name. Its network lists actual names, roles, subnets, gateways and unit/proxy addresses. Gateways
and addresses can be null; the unit's own isolated network has no host gateway. Its ports are the host
openings this node currently holds, with opening id, host number, inside number and transports.

Reasons are start, stop, recreate, move and delete. A new successor's first start uses why it was made;
ordinary later starts use start. After node recovery, post_up runs again with `replay: true`, reason start.
There is no separate node_start reason. Make hooks idempotent: replay must leave an already installed
route or rule in the same state. A unit found to have ended receives its down observation at reconciliation.

The [worked VPN example](../../examples/node-hooks/README.md) includes the document and the script.
Its post_up routes traffic from the unit's subnet through the host's tunnel. An unreachable fallback in
the same routing table keeps traffic closed while the tunnel is down; pre_down removes only this script's
routes. The bundled node lacks NET_ADMIN, so the example uses a short-lived helper on the host network
through the node's Docker socket. Give it the documented Docker client and jq; the helper supplies iproute2.
A node installed directly on the host can run the route program there with the host's required rights.
This recipe is host policy, not a new Orch egress mode.

Before using your hook, observe its document with a non-changing program on an environment you control.
Verify its actual subnet and address, a non-zero pre_up refusing start, a failed post_up stopping the unit,
down failure leaving stop available, and replay producing no duplicate policy. The executable and network
tests in the node and Docker crates cover these observations; the integrator runs their native proof.

A host port's number is separately reconciled by the API every thirty seconds. Stopped, disabled, failed
and removed release its number and retain it as preferred metadata; recreating, moving and offline keep it.
On start, an automatic allocation can take another free number, while a fixed allocation waits for its own.
The Ports page shows the actual number and state.
