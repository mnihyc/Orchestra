# The HTTPS and WebSocket edge

The web edge listens for HTTP on `ORCH_FRONTEND_BIND` (`127.0.0.1:5180` by default). Choose how
people reach it below. Orch manages neither DNS nor ACME; an existing front can keep doing both.

## Ways in

| Way | Browser certificate | Connection to the web edge |
|---|---|---|
| Your front, such as apache2 with Let's Encrypt | Any certificate the device trusts | Proxy to the plain loopback listener; pass WebSockets |
| A CDN, such as Cloudflare | The CDN's certificate for the people's name | [Tunnel](https://developers.cloudflare.com/tunnel/get-started/) to the plain loopback listener, or your HTTPS front with [Full (strict)](https://developers.cloudflare.com/ssl/origin-configuration/ssl-modes/full-strict/) |
| Orch's own HTTPS | A leaf of this installation's existing authority, trusted by hand | Uncomment `ORCH_FRONTEND_HTTPS_BIND=0.0.0.0:443` in `.env`, above its `COMPOSE_FILE` line, then run `docker compose up -d` |

A Tunnel can reach the plain loopback listener; keep that listener private.
For a Tunnel to Orch's own HTTPS, configure its name and private authority in
[originServerName and caPool](https://developers.cloudflare.com/tunnel/reference/origin-parameters/);
leave certificate verification enabled.

## Addresses

`[system] public_url` is the address people open; `[edge] ports_domain` is the separate suffix for
published environment ports. The default machine list, `[[gateway.endpoints]]`, advertises only
`wss://gateway.example.com:9443` with `trust = "pinned"`. Replace the example with a reachable name or IP.
`ORCH_GATEWAY_PUBLISH` publishes that direct door. If its host port differs from 9443, set the same port in
the bundled `[gateway] managed_endpoint` so its Docker runtimes can reach it; keep `[node] gateway_url` on
the internal Compose port 9443. Add `wss://<people's-name>/link` with
`trust = "public"` to advertise the front too. The gateway accepts both doors at all times; the list
controls advertising and preference, not listener availability.

A machine's `[[endpoints]]` list overrides the system list. With no local list it receives a fresh
system list at every connection. Clear an existing local list to follow the system instead. A node's `[node] gateway_url` is its local scalar address override;
leave it empty to follow the system. An explicit connector list takes precedence over that scalar. Its units follow the node by default. `[gateway] managed_endpoint`
in that node's configuration can select another unit address. The bundled node explicitly uses
`gateway.orch.internal` and its Docker `gateway_host` mapping; keep those local settings.
Connector programs and Orch-served builds are downloaded through the machine's verified connection,
so a node need not reach the people's address to provision environments.

## Certificates

Machines accept either a certificate of the appropriate Orch service under the installation's own
authority, under any name or port, or a certificate the device trusts for the name dialled. The
connector, node, managed environments and `orchctl doctor` share this decision. The enrollment token
pins Orch's authority at first contact. The direct door expects the gateway identity; `/link` behind
Orch's own HTTPS expects the web edge identity. A machine's certificate cannot impersonate either.
An unknown-issuer refusal names the issuer; install that company's public authority in the device's
store when using a company front. Browsers and the curl/wget/PowerShell installers use device trust.

Orch's own HTTPS issues a certificate for `public_url`'s host and `*.<ports_domain>` from its existing
authority. The leaf renews automatically through a graceful web-edge reload; a restart adopts changed
names. Download the authority from `/api/v1/pki/ca.pem` or take `config/secrets/orch_ca_cert.pem` from
the installation and verify it through your administrator before trusting it on a device. Trusting
the authority once survives leaf renewal. Clicking past a browser warning may display pages, but does
not make a trusted origin for service workers; trust the authority to use browser push.
[Chromium's service worker security rules](https://www.chromium.org/Home/chromium-security/security-faq/service-worker-security-faq/)
require a secure context.

With your own front, a public certificate such as Let's Encrypt works, as does a company certificate
installed in the devices' stores. A ports domain needs its wildcard certificate; Let's Encrypt uses
[DNS-01 for wildcard issuance](https://letsencrypt.org/docs/challenge-types/). For outgoing calls by
the stack, put company authority PEMs in `config/ca-certificates/`, run `openssl rehash` there and
restart the callers. This supplements the bundled public roots without changing Compose or disabling
verification. Standalone processes use the host's certificate store.

For Cloudflare, keep WebSockets enabled. Its [WebSocket documentation](https://developers.cloudflare.com/network/websockets/)
explains that a CDN deployment may drop established sockets; Orch reconnects. Exclude `/api/` and
`/link/` from caching, browser challenges and script rewriting: machines cannot answer a browser
challenge, and the page's content policy rejects injected scripts. Name the actual proxies in
`trusted_proxy_cidrs`, using [Cloudflare's current published ranges](https://www.cloudflare.com/ips/)
where applicable. The direct 9443 door needs a DNS-only record or IP because it is outside
[Cloudflare's ordinary HTTPS proxy ports](https://developers.cloudflare.com/fundamentals/reference/network-ports/).
The ports wildcard must also be covered at the CDN: in a full setup, Universal SSL covers the zone
and one subdomain level; deeper names need the coverage described in
[Universal SSL limitations](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/).
A separate zone for the ports domain is one option.

## Machine identity and Enroll again

On the direct door the gateway verifies a machine through mutual TLS. Through `/link` it verifies a
per-connection signature over its fresh nonce, the machine id, dialled host and clock. A front that
ends TLS can read `/link` traffic just as it can read browser traffic; use a front you trust.

A machine offline beyond the certificate renewal grace, revoked, or restored after losing its disk
uses **Enroll again** on its existing page. Run that dialog's installer command on the machine.
A retained directory sends its existing id; only a token bound to that same machine may replace it.
The name, page, registrations, policy and local address overrides stay. Its old certificate retires
when the replacement is first used. A new-machine token on a retained directory is refused with a
pointer to Enroll again. Within the renewal grace, ordinary reconnection still renews automatically.
Example addresses block enrollment commands until `public_url` and `[[gateway.endpoints]]` are configured.

## What the front must do

- Proxy everything to the web edge's address.
- Pass WebSocket upgrades for `/api/v1/ws` (the realtime socket, and the terminals under it) and for
  `/link/v1/session` (the machines that come in through this front).
- Allow request bodies of 8 MiB. Nothing the product sends is larger: the browser and the connectors send bigger
  content in pieces of at most 4 MiB. (apache2 puts no limit on a proxied body unless you set one; nginx
  limits to 1 MiB unless `client_max_body_size` says more.)
- Not buffer or cache `/api/` and `/link/`.
- Keep idle connections for at least 60 s. The realtime socket is silent between a person's actions, and a front
  that cuts it earlier makes the page reconnect all the time.
- Forward the client's address in `X-Forwarded-For`, and say that the page was reached over HTTPS
  (`X-Forwarded-Proto: https`).
- Not remove the headers the web edge sets (below), and not add a looser content policy.

The bundled node uses the internal direct door. Other nodes and their environments may use either
advertised door or their own address list. Keep the direct door reachable only from intended machines.

## What the stack's own web edge does

On every response, errors included: `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`,
`X-Frame-Options: DENY`, a `Permissions-Policy` that denies camera, microphone, geolocation, payment and USB, and a
`Content-Security-Policy` that lets the page load only from its own origin (plus the WebAssembly the page runs
for typesetting documents, and `https:` for the film or sound that a document names by address, which the reader's
browser fetches only when asked). Files under `/assets/` are named by their content and kept for a year;
every other page is `no-store`, so that a new release is seen at once. `/internal/` answers 404, and
`/edge-check` answers a POST with 204 after reading its body, which is what `orchctl doctor` sends 8 MiB to. The
services behind it are reached over TLS under the system's own authority, checked by the service's name, and a
service that is made again by an update is found at its new address without restarting the web edge.

## Which proxies may describe a caller

The program defaults `[api] trusted_proxy_cidrs` to an empty list. A connection
outside the configured ranges is counted and recorded at its socket address;
its `X-Forwarded-For` and `Forwarded` headers cannot choose another address.
For a named proxy, Orch walks the chain from right to left, passes over named
proxies, and uses the first other address. An unreadable chain or one containing
only named proxies uses the connection address. IPv4 and its IPv4-mapped IPv6
form share one count. `Forwarded` is used when `X-Forwarded-For` is absent.

Compose creates its control network as `172.30.0.0/24` and gives that same range
to the API and gateway through `ORCH_API_TRUSTED_PROXY_CIDRS`. A private `/24`
leaves room for the stack's services and replicas while keeping trust on one
bridge. If that subnet is already in use, set `ORCH_CONTROL_CIDR` in `.env` to
an unused private subnet before first start; Compose uses it for both the bridge
and the default trust list.

Add the CIDRs of your HTTPS front and any CDN to the trust list, including the
control subnet. Use `/32` for one IPv4 address or `/128` for one IPv6 address.
For example, with a front that nginx sees at
`192.0.2.10` and a CDN in `198.51.100.0/24`, put this in the deployment's `.env`:

```dotenv
ORCH_API_TRUSTED_PROXY_CIDRS='["172.30.0.0/24", "192.0.2.10/32", "198.51.100.0/24"]'
```

Use the addresses nginx actually sees, which may be a Docker host bridge address
for a front on the host. A standalone installation sets the same list under
`[api]` in `orch.toml`. The API and gateway edge use it together; the pinned
gateway always uses the socket peer. Restart the affected services after changing
the list, and limit who can reach the proxy network.

Every named front must append its actual connection peer or replace the incoming
chain with that peer. The shipped nginx appends it, and apache2's `mod_proxy`
does too. A CDN's address ranges belong in the list only when it provides that
same guarantee; use the vendor's published ranges and keep them current.

Naming nothing makes every caller behind a front look like that front, so one
person's failed sign-ins can fill the count for everybody. Naming too much,
especially `0.0.0.0/0` or `::/0`, lets caller-written hops masquerade as trusted
proxies and destroys the caller boundary; chains made entirely of named hops
also collapse to the edge. Keep the list to the proxies that you control or trust.

## apache2

Enable the modules once (`a2enmod proxy proxy_http proxy_wstunnel rewrite headers ssl`) and add a site such as:

```apache
<VirtualHost *:443>
    ServerName orch.example.com

    SSLEngine on
    SSLCertificateFile    /etc/ssl/certs/orch.example.com.pem
    SSLCertificateKeyFile /etc/ssl/private/orch.example.com.key

    # Idle connections live at least a minute: apache2 cuts a proxied WebSocket after
    # ProxyTimeout, which is its Timeout (60 s) unless it is set.
    Timeout 120
    ProxyTimeout 120

    ProxyRequests Off
    ProxyPreserveHost On
    RequestHeader set X-Forwarded-Proto "https"
    # mod_proxy adds X-Forwarded-For itself.

    # The realtime socket and the machines' link are WebSockets.
    RewriteEngine On
    RewriteCond %{HTTP:Upgrade} =websocket [NC]
    RewriteRule ^/(api/v1/ws.*|link/v1/session)$ ws://127.0.0.1:5180/$1 [P,L]

    ProxyPass        / http://127.0.0.1:5180/
    ProxyPassReverse / http://127.0.0.1:5180/
</VirtualHost>
```

This file, with the TLS lines left out and on a loopback port, was put in front of a running stack in a container
of the official `httpd:2.4` image, and `orchctl doctor --url` was run against it from outside, signed in:

- As written, every check passed, the idle realtime socket included (held 60 s).
- With `Timeout 20` and `ProxyTimeout 20` the same command failed, naming the cause: `edge-ws … the connection went
  after 20.0 s with no close frame … the edge's idle limit is below 60 s; raise it (apache2 ProxyTimeout, nginx
  proxy_read_timeout)`.
- With apache2's own default, `Timeout 60`, the socket was held, but only just: the API itself ends a socket that has
  been silent for a minute, so the front's limit and the API's meet at the same instant. That is why the example sets
  120: the front should never be the first to let go.
- A body of 8 MiB went through as written. (`LimitRequestBody 1048576` in the virtual host did not stop a proxied
  body in that image, so the example has no such line; an nginx front with `client_max_body_size 1m` did stop it, and
  `edge-body` said `413: allow request bodies of 8 MiB`.)

## Checking a front

`orchctl doctor --url https://orch.example.com` tries the front the way a browser and a machine would, from
wherever it is run. The program is one static file: `orchctl-linux-x86_64-musl` of the release runs on any
Linux host, and `docker compose exec api /opt/orchctl/orchctl doctor --url …` runs it inside the stack. Each check prints a
line saying what was sent and what came back, and the exit status is 1 if any failed:

| Check | What is sent | What it shows |
|-------|--------------|---------------|
| `edge-page` | `GET /` | The application's page comes back, with the content policy and the four other security headers |
| `edge-api` | `GET /health` and `GET /api/v1/meta` | The API answers through the front, and which release it is |
| `edge-body` | `POST /edge-check` with 8 MiB | The front carries a body of that size whole: the web edge reads all of it before it answers 204 (one byte more is its own 413), so a front that refuses or cuts it is found |
| `edge-ws` | A WebSocket to `/api/v1/ws`, held without a word | The upgrade passes, and the front does not cut an idle socket |
| `edge-link` | A WebSocket to `/link/v1/session` | The machines' upgrade passes and the gateway answers with `orch-link.v1` |
| `edge-endpoint-N` | One connection to each endpoint the system advertises, with the trust it is listed with | A `pinned` endpoint is reached under the system's own authority (the gateway answers 403 to a caller with no machine certificate, which is how it asks for one); a `public` one through device trust for its name or the installation’s web edge identity |
| `edge-ports` | `GET /` at the name of an environment that does not exist under `[edge] ports_domain` | The wildcard name resolves and its certificate holds, the system answers it itself (401 or 404) and sets no sign-in of its own there; `part`, since streaming and the end of an entry need a real opening. Skipped without a ports domain |

An unsigned realtime socket is ended by the API itself after ten seconds, whatever the front does, so without
more the `edge-ws` line says `part`: the upgrade passed and the front held it until the API closed it. To hold it
for the full 60 s as a signed-in page would, give an access token of a browser session:

```sh
TOKEN=$(curl -s -H 'content-type: application/json' https://orch.example.com/api/v1/auth/login \
        -d '{"login":"you","password":"…"}' | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
ORCH_ACCESS_TOKEN=$TOKEN orchctl doctor --url https://orch.example.com --only edge
```

`--only` and `--skip` choose lines (`edge` is all of them), and `--resolve gateway.orch.internal=172.17.0.1`
reaches an advertised name that only the Docker host resolves. A `pinned` endpoint is checked against the system's authority, read from the secrets directory where the
command runs next to it (inside the stack, `/run/secrets`), else fetched from the system; a `public` one against
the device's public and machine roots, or the appropriate service under Orch's own authority. Inside the stack, the device roots include the public bundle and config/ca-certificates/.
