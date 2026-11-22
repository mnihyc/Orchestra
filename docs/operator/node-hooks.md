# Run a program when a container starts or stops

Connect a host's routing, firewall or monitoring policy to the life of the containers that Orch makes on it. At the end a program of yours runs before and after each container starts and stops, and you have checked that it does what you meant. The hook runs with the node's rights.

## Before you start

You need:

- Root on the host that runs the node, and the node's configuration ([Install Orch](deployment.md)).
- An absolute path to each executable you want to run. Hooks are optional, and each program is run directly, with one JSON document on standard input.
- A network pool for managed containers that does not overlap your existing host networks (see below).

## 1. Choose the moments

| Moment | What has happened | If the hook fails |
|--------|-------------------|-------------------|
| `pre_up` | The container's network exists; the container has not started | It does not start. Its managers see the first lines of output, bounded |
| `post_up` | The container started | Its down hooks run and the container stops; the start fails |
| `pre_down` | The container is about to stop or be removed | The failure is reported; stopping continues |
| `post_down` | It stopped, was removed, or ended by itself | The failure is reported. A removed container uses the networks it had |

## 2. Set the hooks

Keep the paths in the node's machine policy:

```sh
orch-node policy set hooks '{"pre_up":"/etc/orch/hooks/check-tunnel.sh","post_up":"/etc/orch/hooks/route-through-vpn.sh","pre_down":"/etc/orch/hooks/route-through-vpn.sh","timeout_secs":30}'
orch-node policy show
```

The command writes the node's data-directory `config.toml`, with explicit local ownership. Hooks,
ports, storages and driver policy use this one file and the `orch-node policy` commands. Opening policy
imports no values from the deployment file and changes no delegation.

The node's operational Docker network settings can be declared as:

```toml
[node.docker.networks]
pool = "10.199.0.0/16"
prefix = 28
```

Every Docker container gets its own subnet from this pool, and a restricted container's proxy has its own outward network. Docker's global address pools are unchanged. An older open container leaves Orch's old common bridge at its next start, keeping the same container and its files. An offline node does not prove that a container stopped.

## 3. Know what a hook receives

- The timeout applies to the whole process group. Output is drained, bounded to 16 KiB per stream, and not copied into history. The hook inherits ordinary process settings such as `PATH` and `LANG`, without the node's other environment variables.
- Every run is a `unit.hook_ran` line in the history of its environment or provider, with the moment, the reason, the exit, a timeout or failure, the duration and the time.
- The document has version 1, the event, the reason, `replay` and the time; the host and system name; the runtime id, kind and driver; the container's name and driver id; and the environment's id, name, full place path, template and generation, or the provider's id and name. Its network lists the actual names, roles, subnets, gateways and the addresses of the container and its proxy. A gateway or an address can be null, and the container's own isolated network has no host gateway. Its ports are the host openings this node holds now, each with the opening id, the host number, the inside number and the transports.
- The reasons are `start`, `stop`, `recreate`, `move` and `delete`. A stop by a person, a stop of an idle container and a recycle are all `stop`; the container's next start is `start`. A new successor's first start uses why it was made, and ordinary later starts use `start`.
- After the node recovers, `post_up` runs again with `replay: true` and reason `start`. There is no separate node-start reason. A container found to have ended gets its down observation when the node reconciles.

**Make hooks idempotent.** A replay must leave a route or rule that is already installed in the same state.

## 4. Check your hook

Before you rely on it, observe the document with a hook that changes nothing, on an environment you control. Check:

1. The actual subnet and address in the document.
2. A non-zero `pre_up` stops the container from starting.
3. A failed `post_up` stops the container.
4. A failing down hook leaves **Stop** available.
5. A replay produces no duplicate policy.

The tests of the node and Docker crates cover the same behaviour.

## Example: route a container's traffic through a VPN

The [worked VPN example](../../examples/node-hooks/README.md) holds the document and the script. Its `post_up` routes traffic from the container's subnet through the host's tunnel. An unreachable fallback in the same routing table keeps traffic closed while the tunnel is down, and `pre_down` removes only this script's routes. The bundled node lacks `NET_ADMIN`, so the example uses a short-lived helper on the host network through the node's Docker socket: give it the Docker client and `jq` it documents, and the helper supplies `iproute2`. A node installed directly on the host can run the route program there with the rights it needs. This recipe is host policy, not a new Orch way of controlling outgoing traffic.

## Host ports

The API reconciles a host port's number every thirty seconds. A stopped, disabled, failed or removed environment releases its number and keeps it as preferred metadata; recreating, moving and an offline node keep it. On start, an automatic allocation can take another free number, while a fixed allocation waits for its own. The **Ports** page shows the actual number and state.

## If it does not work

| You see | What to do |
|---|---|
| A container never starts | `pre_up` failed. Read the first lines of its output in the history of the environment |
| The container starts and then stops | `post_up` failed: its down hooks ran. Fix the program, then start again |
| A hook times out | It hit `timeout_secs`; the whole process group is stopped. Raise the timeout or make the program faster |
| A route or rule appears twice | The hook is not idempotent. Make it leave an already installed route as it is |
