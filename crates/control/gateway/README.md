# orch-gateway-core

Stateless machine gateway and tunnel relay.

Implemented. The package is the gateway that machines connect to: it takes
their connections on the pinned port and on the port behind the public edge,
accepts one session for each machine once the system says that machine may
have one, passes their events and results up the backplane and the system's
commands and acknowledgements down it, relays tunnels between two machines
and the terminals that end at the system, and hands enrollment, renewal and
content requests to the system together with who is asking. It keeps nothing
that has to outlast it. It is tested with real sockets on loopback, a real
database, the API's real internal listener and the link's own machine over real
TLS: enrollment, a thousand events across a restart of the gateway and one of
the system, commands that wait for a machine, revocation, a session replaced
by a newer, tunnels that survive a reconnect, renewal, and what the system
does while the gateway is away and the gateway while the system is.

The example `by_hand_machine` is a machine for trying a running deployment: it
enrolls with a token and then holds a session as a connector would, through
either port, and answers the commands it is sent without running them.

| Module | Planned responsibility |
|--------|------------------------|
| [listeners](src/listeners.rs) | Pinned mutual-TLS and public-trust edge listeners. |
| [forwarded_address_gateway](src/forwarded_address_gateway.rs) | The gateway sees the front's real client address through the same trusted hops as the API. |
| [authentication](src/authentication.rs) | Certificate and signed-handshake checks with API authorization. |
| [sessions](src/sessions.rs) | Active machine sessions and replacement. |
| [routing](src/routing.rs) | Forward events, commands and ephemeral reports. |
| [relay](src/relay.rs) | Ticket-authorized machine tunnels and API terminal tunnels. |
| [backplane](src/backplane.rs) | API connection, recovery and heartbeat reporting. |
| [content](src/content.rs) | Authenticated content proxy to API storage. |
| [tests/content_support](tests/content_support/mod.rs) | Fixture-only content refusal shared by gateway test worlds; transfer tests supply real content routes. |
| [liveness](src/liveness.rs) | Session heartbeat and connection shutdown. |
| [service_renewal](src/service_renewal.rs) | The gateway's own certificate renewed while it runs: a look at start and every hour, a new key certified by the system once two thirds of the life have passed, shown from the next handshake. |
