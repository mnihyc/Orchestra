# Run regional gateways

Put a gateway near each group of machines, let each machine pick the gateway it connects to, and let two machines that work together meet at the gateway that suits both. At the end machines in different regions connect to a nearby gateway, and the bytes between two of them pass through one gateway and never through the API.

Most installations need one gateway, which the install already makes: skip this page unless machines in several regions are slow to connect, or tool traffic between machines crosses an ocean.

## Before you start

You need:

- A running system ([Install Orch](deployment.md)) with its HTTPS front ([Run the HTTPS front](edge.md)).
- A host in each region where a gateway can run, and for each a DNS name that machines can reach on the gateway's published port.
- The system's API and its gateway already updated to the release you run on the others. Update the API and the gateways first, and connectors and installers after, then offer regional choices to people.

The API stays in one place and is the control plane for every machine. Gateways do not connect to one another and do not connect to PostgreSQL. Each gateway has one outgoing connection to the API.

## How the choice works

There are two separate choices.

- **The primary gateway of a machine** carries its control traffic and the connections people make to it (terminals, browsers, displays and ports). A machine either follows **Automatic**, or is **selected** onto one gateway.
  - **Automatic** measures the gateways from the machine being installed, when it enrolls and at each reconnect of its primary, and picks the lowest measured round trip (the median of three verified round trips). It never moves a healthy connection.
  - A **selected** gateway is always honoured: its addresses may fail over between that gateway's pinned and public entrances, but the machine never uses another gateway. If it is unavailable, the machine stays offline until it returns or an administrator changes the choice. A selected gateway that is removed from the catalogue shows as unavailable and is never reset to Automatic.
- **The tunnel gateway of a pair** carries the bytes between two machines that work together, for example an agent program on one machine and its tools on another. The API chooses it for each pair, and it never changes either machine's primary choice. By default it is the gateway with the lowest total of the round trips measured from both machines, as long as both measurements are fresh (within the link's dead-peer time, 60 seconds by default). If a measurement is missing, the executor's primary gateway is used. A missing measurement is never counted as zero. Set `gateways.tunnel_strategy` to `executor` to keep new tunnels on the executor's gateway; the default is `lowest_combined_rtt`. A change affects new tunnels only. An open tunnel keeps its plan.

A machine keeps one primary session. For each other gateway it needs for a tunnel, it opens one tunnel-only session, shared by every thread that uses it. That session carries tunnel traffic only: it cannot send events, commands, policy, enrollment or blob requests, and it never makes a machine look online. Unused tunnel-only sessions close after `[link] tunnel_session_idle_secs` (60 by default); live channels and threads that still use them keep them open.

## 1. Deploy a gateway in each region

Run one more `orch-gateway` on the regional host with:

1. A unique `gateway_id` in its `[gateway]` configuration, matching the `id` you publish below.
2. Its listeners: `pinned_bind` (published, default `[::]:9443`) and, if you route a public front to it, `edge_bind`.
3. `backplane_url` pointing at the API's internal mutual-TLS endpoint (`wss://<api-host>:8444/internal/backplane` for the shipped layout), so the gateway can reach the API from its region.
4. Its own service certificate and key (`service_gateway_cert.pem`, `service_gateway_key.pem`) and the authority's public certificate. The CA's private key stays on the API. The gateway renews its own certificate while it runs.
5. If a public front serves a regional gateway, route that gateway's WebSocket and its companion HTTPS to the same instance: session tokens and channel maps are local to a gateway.

`orchctl compose-secrets` makes the first certificate of each service of the Compose stack. The release has no command that issues one for a gateway running on another host.

## 2. Publish the catalogue

The catalogue lists each gateway with a stable ID, a label, a region and one or more entrances. In `config/orch.toml`, replace the single legacy list with entries:

```toml
[[gateway.instances]]
id = "compose-gateway"
label = "Europe"
region = "Europe"
endpoints = [{ url = "wss://eu.example.com:9443", trust = "pinned" }]

[[gateway.instances]]
id = "asia"
label = "Asia tools"
region = "Asia"
endpoints = [{ url = "wss://asia.example.com:9443", trust = "pinned" }]
```

Rules, checked at start and on every edit:

- Each `id` is unique, made of letters, digits, `-`, `_` or `.`, and equal to that gateway's own `gateway_id`. `automatic` is reserved.
- Each gateway has a non-empty label and region, and at least one entrance. The region is a description, not a location rule.
- An entrance is a `wss` or `https` address with `trust` of `pinned` or `public`, and no credentials, query or fragment. One entrance cannot belong to two gateways. Several entrances of one gateway are aliases of it, not separate choices.
- With no `[[gateway.instances]]`, the existing `gateway_id` and `[[gateway.endpoints]]` form one entry, labelled by the ID.

System administrators can also edit the catalogue in **Management → Gateways**, or through `GET` and `PUT /api/v1/gateways` (a full ordered replacement; a stale revision is refused with a 409 and no content). An edit is kept as a system override that wins after an API restart. Editing the catalogue advertises choices. It deploys no gateway and moves no connected machine.

## 3. Connect machines

1. In **Add machine** or **Add host**, set **Gateway** to **Automatic** or to one of the advertised labels. The command gets `--gateway automatic` or `--gateway <id>` (`-Gateway` on Windows).
2. Run the command on the machine. When the installer can ask, it lists the labels, regions and measured round trips; the measurements come from that machine, not from your browser.
3. Without a terminal, a fresh install uses Automatic unless you gave a flag. Enrolling again without a flag keeps the choice already saved. Starting the service never asks.

The choice is saved in the connector's `config.toml` as `[connector.gateway]` (`selection = "automatic"`, or `selection = "selected"` with `gateway_id`). It follows the ID, so renaming a label or changing an address keeps it. An unknown ID is refused before the enrollment token is used. To change it later, use the connector's local console or reinstall with the flag; the web screens cannot change it.

An explicit `[[endpoints]]` list in the connector's configuration still overrides the system's list for the primary, and Automatic then measures only that list.

## Check that it works

- A machine's page, in its **Connector** card, shows the primary gateway it reported and how it was chosen; with **Automatic** it also shows each gateway's measurement and when it was taken. **Not measured** is not zero.
- `orch-connector doctor` shows through which route each gateway address is reached.
- A thread's **Tunnel gateway** view names the gateway the tool tunnel uses, the rule that chose it and the measurements; **Connecting** and **Not reported** mean it has not attached yet.
- `GET /api/v1/machines/{machine_id}/gateway` and `GET /api/v1/threads/{thread_id}/gateway` return the same facts.

## What happens when something fails

| What fails | What happens |
|---|---|
| The chosen tunnel gateway cannot take one end | The frozen plan for the pair tries a gateway both ends can reach, ordered by their combined round trip. If none works, the work is kept, the screen names the labels it tried, and the code is `tunnel.gateway_unreachable`. Neither primary changes |
| A gateway process stops | The tunnels through it reset and the usual recovery of interrupted work applies. The next opening uses the fallback order |
| A socket drops but the gateway lives | The connection resumes within 60 seconds with its byte counts, as on a single gateway |
| The API cannot be reached from a gateway | The gateway refuses new machine sessions, keeps existing machine tunnels going, and cannot admit a new tunnel attachment. Ports close. Blobs, downloads and the connections people make through the API wait for it |
| A selected primary gateway is down | Its machines stay offline until it returns or the choice changes; Automatic machines pick again at their next reconnect |

## What this does not do

- It does not connect gateways to one another, and it does not send machine-to-machine bytes through the API. A tunnel crosses exactly one gateway, so being near the executor changes which long path is paid; it does not shorten the distance or compress traffic.
- It does not move a healthy primary session, and it does not resume a tunnel on a different gateway after its gateway is lost.
- People's terminals, browsers, displays and through-Orch ports stay on the machine's primary gateway and the API.
- It is designed for up to five gateways. With five, a machine holds at most five live sessions in all: one primary and one tunnel-only session for each other gateway it uses, not five per thread.
