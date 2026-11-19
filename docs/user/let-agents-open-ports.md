---
title: Let agents open ports without asking
screens: /thread/$threadId, /resources/environments/$id, /resources/templates/$id, /topic/$topicId, /project/$projectId, /approvals
keywords: how do I open a port, without asking, ports, port_place, port_host, port_link, capability.policy, ports.enabled, ports.max_open, ports.link_max_secs, requests.expiry_hours, network.allowed_hosts, template defaults
---

# Let agents open ports without asking

An environment's Ports panel shows services and who can reach them through
Orch. Watching a thread supplies no port authority by itself. **Ports View**
lets its holder use a service; **Ports Manage** decides wider openings.

## Choose the audience

Three environment capabilities permit an agent to open a kind of port:

| Capability | Who gets in | Initial policy |
|------------|-------------|----------------|
| `port_place` | Current viewers of the named org, project or topic | Ask |
| `port_host` | Anyone who reaches the host's declared listening address | Deny |
| `port_link` | Anyone holding the unexpired link | Deny |

These are gateway capabilities: they need no driver patch, restart or
recreate. The template decides which may ever be given; the shipped templates
allow all three. The place's capability policy still bounds them. A person
with Ports Manage also cannot open a kind that policy denies.

For example, an agent starts an application on TCP port **3000** and requests
that the project's current viewers can open it. Under Ask, its card offers
**Allow once**, **Grant to this environment**, or **Deny**. Read the named
project and audience before allowing. This gives none of the environment's
Files, Terminal, Browser or Display doors.

## Allow later openings

For **one environment**, open its Ports or Capabilities panel and grant
`port_place`. Later agent requests of that kind proceed without a card and
name the capability that permitted them. Their actual audience remains shown.

For **future environments of a template**, include `port_place` in the
template's base capabilities, within its allowed list. Existing environments
retain their own grants.

For **environments in a place**, set that place's capability policy to allow.
The example value for `capability.policy` is:

```json
{
  "port_place": { "decision": "allow", "approver_scope": "environment" }
}
```

An agent there can then open port 3000 for the project's viewers without
asking, while capacity, the door switch and current audience checks still
apply. Host openings also need the host's actual ranges and local ceiling.
There is no separate “agents' ports” policy or Always-rule list.

## Treat links as secrets

Giving `port_link` ahead permits links too; it is a high-risk choice because
the agent receives a secret usable by whoever holds it. The policy page and
card state this. For one asking request, **For me only** gives the link once
to the deciding person and tells the agent only its existence and lapse.
Never put a link secret into ordinary diagnostics or a public screenshot.

Closing an opening ends its live uses. Removing a capability stops future
automatic openings; existing openings retain their own lifetime until closed.
Request expiry and repeated denial use [How approvals work](how-approvals-work.md).

## Understand the outbound path

Port requests govern Orch's own doors. With **open** outbound network, a
program can connect to an outside tunnel service and publish its port 3000
without making an Orch link. Restricted egress with only the needed listed
destinations closes that unlisted outward path. Ports and Network explain
this while egress is open and link the egress setting. A port decision does
not narrow what the network policy already permits.

## Read ports from a program inside the environment

Processes Orch starts for an attached environment receive `ORCH_PORTS_URL`, its read-only loopback
endpoint. On a connected machine they also receive a separate `ORCH_PORTS_TOKEN`, used as a Bearer
credential. It belongs to this environment and is replaced after detach; do not share it or substitute
a person's token. A runtime Orch made needs no bearer on this local interface.

```sh
orch-connector ports list --look --limit 20
orch-connector ports address 3000
orch-connector ports whois --peer 127.0.0.1:43123 --listener-port 3000 --transport tcp
```

The commands print the same port facts and addresses as the Ports panel, without a link's secret.
A list's cursor continues its whole rows. They cannot create an opening, grant a capability or close
somebody's connection. `port forward` remains the separate command for carrying an admitted port.

For `whois`, use the exact peer returned by the program's accepted socket, before reading a PROXY
prefix; for UDP use the sender of the datagram. Include the actual listener and transport. A match
returns the current connection's two ends and a short-lived signed assertion. A non-Orch, closed or
reused peer returns JSON `null`. If a known peer cannot obtain a current assertion, the read fails;
retry rather than treating that as proof that the caller is unknown. Verify the assertion's signature,
issuer, own environment/port audience and time using the public keys at `/ports/assertion-keys`.
Those keys are public and require no bearer; the private signing key remains on the server.

The local interface permits 64 simultaneous lookups and 3,600 reads per rolling minute per environment.
An exceeded local limit answers 429 with `Retry-After`; a peer reply is at most 4,000 bytes. The CLI
uses this literal loopback endpoint directly, with no redirect or proxy, and a bounded wait.
