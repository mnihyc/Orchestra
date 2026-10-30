# A node's lifecycle hooks

A node runs the executables its operator names in the machine policy's `hooks` at four moments of every unit it makes, and gives
each one JSON document on standard input that says what the unit is. They run with the node's own rights, so only the
host's operator keeps them locally or hands the key to the machine's managers. This folder holds one worked example.

| Example | What it shows |
|---------|---------------|
| [route-through-vpn.sh](route-through-vpn.sh) | Every unit's traffic to the outside sent through a VPN tunnel of the host, and dropped while the tunnel is down. |

## The moments

| Hook | Runs | When it fails or runs out of time |
|------|------|-----------------------------------|
| `pre_up` | the unit's network exists, the unit has not started | the unit does not start; the start fails with the first lines the hook printed |
| `post_up` | the unit runs | the unit is stopped again (its down hooks run); the start fails the same way |
| `pre_down` | the unit is about to stop or be removed | told in the history; the stop goes on |
| `post_down` | the unit has stopped, been removed, or ended by itself | told in the history |

```sh
orch-node policy set hooks '{"pre_up":"/etc/orch/hooks/check-tunnel.sh","post_up":"/etc/orch/hooks/route-through-vpn.sh","pre_down":"/etc/orch/hooks/route-through-vpn.sh","post_down":"/etc/orch/hooks/notify.sh","timeout_secs":30}'
```

Each run has thirty seconds; past it the hook's whole process group is killed. Previous-release hook tables
are still accepted and imported once, with the replacement command in the console. Set subsequent values
through the policy command. The initial compatible node configuration can also supply the network pool:

```toml
[node.docker.networks]     # every unit's own network is cut from here; Docker's own pools stay untouched
pool = "10.199.0.0/16"
prefix = 28
```

A hook is run directly, with no shell around it and none of the node's environment variables but the usual process
settings (`PATH`, `HOME`, `LANG`, ...). When the node starts it runs `post_up` again for every unit it finds running,
with `"replay": true`, so a hook must leave things as they are when what it does is done already. Each run is a line
of the history of the environment (or provider) the unit serves: the moment, why, the exit and how long it took,
never what the hook printed.

## The document

```json
{
  "version": 1,
  "event": "post_up",
  "reason": "start",
  "replay": false,
  "at": "2026-10-14T15:00:00.000Z",
  "host": { "system": "Orch", "name": "build-host-1" },
  "runtime": { "id": "0199…", "kind": "environment", "driver": "docker" },
  "environment": {
    "id": "0199…", "name": "payments", "path": "Acme / Platform / Payments",
    "template_id": "0199…", "template": "Default", "generation": 3
  },
  "provider": null,
  "unit": { "name": "orch-environment-3f2a9c1d0b44", "id": "c0ffee…" },
  "network": {
    "mode": "bridge",
    "egress": "open",
    "networks": [
      { "name": "orch-environment-3f2a9c1d0b44-net", "role": "unit",
        "subnet": "10.199.12.32/28", "gateway": "10.199.12.33", "address": "10.199.12.34" }
    ]
  },
  "ports": [ { "opening_id": "0199…", "host_port": 49160, "port": 3000, "transports": ["tcp"] } ]
}
```

- `reason` is `start`, `stop`, `recreate` (a unit replaced by a successor on the same volumes, or that successor's
  first start), `move` (an environment leaving its host, or its first start on the new one) or `delete`.
- `network.networks` lists every network of the unit's own with its role: `unit` for the unit's network, and for a
  restricted unit `egress` for the network its proxy goes out on (the unit's own network then has no `gateway`:
  its bridge has no address on the host). `address` is `null` while the unit does not run.
- `environment` is `null` for a provider's runtime, which has `provider` (`id`, `name`) instead.
- `ports` are the host openings this node holds for the environment.

## Routing through a VPN

`route-through-vpn.sh` is named for `post_up` and `pre_down`. For the subnet a unit's traffic leaves from it keeps the
host's specific routes (its own networks, Docker's networks, Orch's services) and sends everything else into a table
of its own, whose default route is the tunnel and, below it, an unreachable route: while the tunnel is down nothing
goes out. Let the tunnel keep its own route in that table (for WireGuard, `Table = 200` in its configuration), so that
it comes back with the tunnel.

The bundled node runs in a container without the right to change the host's routes, so the script changes them in a
short-lived helper container on the host's network; give the node's container `jq` and a Docker client (it already
has the Docker socket) and build the helper once:

```sh
printf 'FROM alpine:3.20\nRUN apk add --no-cache iproute2\n' | docker build -t orch-route-helper -
```

A node installed on the host itself needs only `jq`, `ip` and the right to change routes: set `DIRECT="yes"` in the
script.
