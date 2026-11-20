# Run the HTTPS front

Put an HTTPS front in front of Orch's web edge so that people reach Orch over HTTPS and machines can connect. At the end `orchctl doctor --url` passes every line against your address.

The web edge listens for plain HTTP on `ORCH_FRONTEND_BIND` (`127.0.0.1:5180` by default). Orch manages neither DNS nor ACME; a front you already run can keep doing both.

## Before you start

You need:

- A running stack ([Install Orch](deployment.md)).
- A DNS name for people, such as `orch.example.com`, and a certificate for it that your devices trust.
- A decision about environment ports: if people will open ports that environments publish, a second DNS suffix (the **ports domain**) with a wildcard record and a wildcard certificate.
- A front: apache2, nginx, Caddy, a CDN tunnel, or Orch's own HTTPS.

## Long-distance access

Use a trusted HTTPS front that negotiates HTTP/2 for people far from the API. Plain HTTP/1.1 has a small per-origin connection pool, so asset transfers can queue an API read; TLS with HTTP/2 multiplexes them. Orch’s own HTTPS listener already enables `http2 on`. If another front or CDN ends TLS, check the browser-facing connection there. Nginx documents the required [TLS and ALPN support](https://nginx.org/en/docs/http/ngx_http_v2_module.html).

Check the actual address, using the installation’s public authority when it is not in the device trust store:

```sh
curl --http2 --cacert /path/to/orch-ca.pem --silent --show-error --output /dev/null --write-out '%{http_version}\n' https://orch.example/
```

`2` confirms HTTP/2. Keep certificate verification enabled; this certificate is public trust material, not the authority’s private key. A warm reload reuses content-addressed assets, while the page starts its cookie session and first authenticated read before the application scripts compete for connections. A stable 700 ms line is normal; the connection indicator warns for measured worsening relative to that person’s own usual.

Orch’s own HTTPS also enables HTTP/3 beside HTTP/2. The HTTPS overlay publishes TCP and UDP at the same configured address; allow that UDP port through an external firewall. `Alt-Svc` names the port the browser actually opened, including a nonstandard published port. The shipped nginx includes the QUIC module; reachability still depends on the path to the browser. See nginx’s [HTTP/3 module](https://nginx.org/en/docs/http/ngx_http_v3_module.html) and [QUIC requirements](https://nginx.org/en/docs/quic.html). An external front may offer HTTP/3 to browsers while keeping HTTP/2 or HTTP/1.1 to the origin.

Management → Settings → **HTTPS transport** (`edge.transport`) controls the built-in TLS edge. Defaults are a 16 MiB shared session cache, a one-day resumption lifetime, session tickets, early safe reads, HTTP/2 and HTTP/3, a one-day protocol advertisement, and a one-second local reload check. Saved edits publish only after commit and reload nginx gracefully; idle checks read local file identities, without a database poll or network request. Turning HTTP/3 off clears its previous advertisement. These are defaults, with no fixed upper cap; a cache of 0 disables that cache. An external TLS front keeps its own transport settings.

Session resumption saves the repeat handshake. nginx manages the session ticket keys; with the shared cache enabled it rotates them across workers. TLS early data can be replayed, so the edge forwards `Early-Data: 1` and the API answers every early write with **425** before authentication or business effects. Stateful sign-in redirects also wait for the handshake even though their method is GET. Repeat the action after the handshake; ordinary safe reads can use early data. See nginx’s [session and early-data directives](https://nginx.org/en/docs/http/ngx_http_ssl_module.html) and [the HTTP early-data protocol](https://www.rfc-editor.org/rfc/rfc8470.html). No ticket key or private certificate is exposed by the setting.

When the configured web address is HTTP, Management’s installation facts say beside it that it has no certificate and starts more slowly over a long line. This describes that address; it does not infer whether another reverse proxy has a certificate on a different address.

## 1. Choose a way in

| Way | Browser certificate | Connection to the web edge |
|---|---|---|
| Your front, such as apache2, nginx or Caddy with Let's Encrypt | Any certificate the device trusts | Proxy to the plain loopback listener; pass WebSockets |
| A CDN, such as Cloudflare | The CDN's certificate for the people's name | [Tunnel](https://developers.cloudflare.com/tunnel/get-started/) to the plain loopback listener, or your HTTPS front with [Full (strict)](https://developers.cloudflare.com/ssl/origin-configuration/ssl-modes/full-strict/) |
| Orch's own HTTPS | A leaf of this installation's authority, trusted by hand | Uncomment `ORCH_FRONTEND_HTTPS_BIND=0.0.0.0:443` in `.env`, above its `COMPOSE_FILE` line, then run `docker compose up -d` |

Keep the plain loopback listener private. A Tunnel can reach it. For a Tunnel to Orch's own HTTPS, set its name and private authority in [originServerName and caPool](https://developers.cloudflare.com/tunnel/reference/origin-parameters/) and leave certificate verification on.

## 2. Set the addresses

In `orch.toml`:

1. Set `[system] public_url` to the address people open.
2. Set `[edge] ports_domain` to the suffix for published environment ports, if you use ports.
3. Replace the example in `[[gateway.endpoints]]`. The default list holds only `wss://gateway.example.com:9443` with `trust = "pinned"`, and Orch tells no machine an example address: until you replace it, **Add machine** makes no command and environments cannot connect.
4. To advertise the front as well, add `wss://<people's-name>/link` with `trust = "public"`. The gateway accepts both ways in at all times; the list controls advertising and preference, not whether a listener is open.
5. If you publish the direct way in on a host port other than 9443 (`ORCH_GATEWAY_PUBLISH`), set the same port in `[gateway] managed_endpoint` so Docker runtimes can reach it. Keep `[node] gateway_url` on the internal Compose port 9443.

**Who follows which list.**
- A machine's own `[[endpoints]]` list overrides the system list. With no local list it receives a fresh system list at every connection. Clear a local list to follow the system again.
- A node's `[node] gateway_url` is a local address override for that node; leave it empty to follow the system. An explicit connector list takes precedence over it.
- Managed environments follow their node's list; `[gateway] managed_endpoint` in the node's configuration can choose another address for them.
- Keep the bundled node's `gateway_url` as `wss://gateway:9443`, the gateway's Compose service name. Keep `gateway.orch.internal` and the Docker `gateway_host` mapping that environments are told through `managed_endpoint`.
- The first time, a node asks its `gateway_url` for the system's list and enrolls through an advertised entrance that answers, or at that address when none is advertised.
- Connector programs and the builds Orch serves are downloaded through the machine's verified connection, so a node need not reach the people's address to make environments.

## 3. Set up the front

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

Forward `/api/v1/` for browser sessions and `/openapi/v1/` for API keys unchanged,
including `/api/v1/ws` and `/openapi/v1/ws`. A proxy must preserve the path prefix
and support WebSocket upgrades on both roots. An update replaces `deploy/nginx.conf` with the
release's when it is a file the product shipped, and keeps one you edited, writing the shipped
one beside it as `nginx.conf.new`; merge the key-route blocks into an edited file (see
[deployment files](upgrades.md#deployment-files)). Installer downloads stay at `/api/v1/downloads/`.

For Cloudflare, keep WebSockets enabled. Its [WebSocket documentation](https://developers.cloudflare.com/network/websockets/)
explains that a CDN deployment may drop established sockets; Orch reconnects. Exclude `/api/`,
`/openapi/` and `/link/` from caching, browser challenges and script rewriting: machines cannot answer a browser
challenge, and the page's content policy rejects injected scripts. Name the actual proxies in
`trusted_proxy_cidrs`, using [Cloudflare's current published ranges](https://www.cloudflare.com/ips/)
where applicable. The direct 9443 door needs a DNS-only record or IP because it is outside
[Cloudflare's ordinary HTTPS proxy ports](https://developers.cloudflare.com/fundamentals/reference/network-ports/).
The ports wildcard must also be covered at the CDN: in a full setup, Universal SSL covers the zone
and one subdomain level; deeper names need the coverage described in
[Universal SSL limitations](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/).
A separate zone for the ports domain is one option.

The front must:

- Proxy everything to the web edge's address.
- Pass WebSocket upgrades for `/api/v1/ws` (browser sessions), `/openapi/v1/ws` (account keys), their ticket continuations, and `/link/v1/session` (machines that come in through this front). Forward `/api/v1/` and `/openapi/v1/` unchanged, keeping the path prefix, and route both to the same API service. Installer downloads stay at `/api/v1/downloads/`.
- Allow request bodies of 8 MiB. Nothing Orch sends is larger: the browser and the connectors send bigger content in pieces of at most 4 MiB. apache2 puts no limit on a proxied body unless you set one; nginx limits it to 1 MiB unless `client_max_body_size` says more.
- Not buffer or cache `/api/`, `/openapi/` and `/link/`.
- Keep idle connections for at least 60 seconds. The realtime socket is silent between a person's actions, and a front that cuts it sooner makes the page reconnect all the time.
- Forward the client's address in `X-Forwarded-For`, and say the page was reached over HTTPS with `X-Forwarded-Proto: https`.
- Keep the headers the web edge sets (see the end of this page) and not add a looser content policy.

The updater keeps an existing `deploy/nginx.conf`. When you update an older installation, merge the shipped key-route blocks into it.

### apache2

Enable the modules once (`a2enmod proxy proxy_http proxy_wstunnel rewrite headers ssl`) and add a site:

```apache
<VirtualHost *:443>
    ServerName orch.example.com

    SSLEngine on
    SSLCertificateFile    /etc/ssl/certs/orch.example.com.pem
    SSLCertificateKeyFile /etc/ssl/private/orch.example.com.key

    # apache2 cuts a proxied WebSocket after ProxyTimeout, which is its
    # Timeout (60 s) unless set. The API ends a socket silent for a minute,
    # so the front should never be the first to let go.
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

This file, without the TLS lines and on a loopback port, passed every `orchctl doctor --url` check against a running stack, including the idle socket held for 60 seconds. With `Timeout 20` and `ProxyTimeout 20` the check failed and named the cause. A `LimitRequestBody 1048576` line did not stop a proxied body, so the example has none.

### nginx

```nginx
map $http_upgrade $connection_upgrade { default upgrade; '' close; }

server {
    listen 443 ssl;
    server_name orch.example.com;

    ssl_certificate     /etc/ssl/certs/orch.example.com.pem;
    ssl_certificate_key /etc/ssl/private/orch.example.com.key;

    client_max_body_size 8m;
    proxy_read_timeout   120s;
    proxy_send_timeout   120s;

    location / {
        proxy_pass http://127.0.0.1:5180;
        proxy_http_version 1.1;
        proxy_buffering off;
        proxy_set_header Host $host;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection $connection_upgrade;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto https;
    }
}
```

An nginx front with `client_max_body_size 1m` did fail the body check (`edge-body`) with a 413. The configuration above was written from the list of requirements and was not run against a stack for this page: run the check below.

### Caddy

```caddyfile
orch.example.com {
    reverse_proxy 127.0.0.1:5180 {
        flush_interval -1
    }
}
```

Caddy gets its certificate from Let's Encrypt, passes WebSocket upgrades, keeps the `Host` header and adds `X-Forwarded-For` and `X-Forwarded-Proto` by itself. It sets no limit on a request body. This was also not run against a stack for this page: run the check below.

### Cloudflare

- Keep WebSockets on. A CDN may drop established sockets ([Cloudflare's note on WebSockets](https://developers.cloudflare.com/network/websockets/)); Orch reconnects.
- Exclude `/api/`, `/openapi/` and `/link/` from caching, browser challenges and script rewriting. Machines cannot answer a browser challenge, and the page's content policy rejects injected scripts.
- Put Cloudflare's published ranges in `trusted_proxy_cidrs` ([the current list](https://www.cloudflare.com/ips/)).
- The direct 9443 way in needs a DNS-only record or an IP address, because it is outside [Cloudflare's ordinary HTTPS proxy ports](https://developers.cloudflare.com/fundamentals/reference/network-ports/).
- The ports wildcard must be covered at the CDN too. Universal SSL covers the zone and one subdomain level; deeper names need the coverage in [Universal SSL limitations](https://developers.cloudflare.com/ssl/edge-certificates/universal-ssl/limitations/). A separate zone for the ports domain is one option.

## 4. Trust the certificates

Machines accept either a certificate of the right Orch service under the installation's own authority (under any name or port), or a certificate the device trusts for the name it dialled. The connector, the node, managed environments and `orchctl doctor` make this decision the same way.

- The enrollment token pins Orch's authority at first contact. The direct way in expects the gateway's identity; `/link` behind Orch's own HTTPS expects the web edge's identity. A machine's certificate cannot impersonate either.
- An unknown-issuer refusal names the issuer. When you use a company front, install that company's public authority in the device's store. Browsers and the curl, wget and PowerShell installers use device trust.
- **Your own front** works with a public certificate such as Let's Encrypt or a company certificate in the devices' stores. A ports domain needs its wildcard certificate; Let's Encrypt issues wildcards through [DNS-01](https://letsencrypt.org/docs/challenge-types/).
- **Orch's own HTTPS** issues a certificate for `public_url`'s host and `*.<ports_domain>` from the installation's authority. The leaf renews itself through a graceful web-edge reload; a restart adopts changed names. Download the authority from `/api/v1/pki/ca.pem`, or take `config/secrets/orch_ca_cert.pem` from the installation, and verify it with your administrator before trusting it on a device. Trusting it once survives renewals. Clicking past a browser warning may show pages but does not make a trusted origin for service workers, so trust the authority to use browser push ([Chromium's service worker rules](https://www.chromium.org/Home/chromium-security/security-faq/service-worker-security-faq/) require a secure context).
- **Calls the stack makes** to your internal services: put company authority PEMs in `config/ca-certificates/`, run `openssl rehash` there and restart the callers. This adds to the bundled public roots without changing Compose or turning verification off. Programs that run on their own use the host's store.

## 5. Name the proxies that may describe a caller

By default `[api] trusted_proxy_cidrs` is empty. A connection from outside the listed ranges is counted and recorded at its socket address, and its `X-Forwarded-For` and `Forwarded` headers cannot choose another address. For a named proxy, Orch walks the chain from right to left, skips named proxies, and uses the first other address. An unreadable chain, or one of only named proxies, uses the connection address. IPv4 and its IPv4-mapped IPv6 form share one count. `Forwarded` is read when `X-Forwarded-For` is absent.

Compose creates its control network as `172.30.0.0/24` and gives that range to the API and gateway through `ORCH_API_TRUSTED_PROXY_CIDRS`. If the subnet is already in use, set `ORCH_CONTROL_CIDR` in `.env` to an unused private subnet before the first start; Compose uses it for the bridge and the default list.

1. Add the CIDRs of your front and any CDN, keeping the control subnet. Use `/32` for one IPv4 address and `/128` for one IPv6 address. With a front that nginx sees at `192.0.2.10` and a CDN in `198.51.100.0/24`, put this in `.env`:

   ```dotenv
   ORCH_API_TRUSTED_PROXY_CIDRS='["172.30.0.0/24", "192.0.2.10/32", "198.51.100.0/24"]'
   ```

   Use the addresses the proxy really sees, which may be a Docker host bridge address for a front on the host. A standalone installation sets the same list under `[api]` in `orch.toml`.
2. Restart the affected services. The API and the gateway edge use the list together; the pinned gateway always uses the socket peer.
3. Limit who can reach the proxy network.

Every named front must append its connection peer to the chain, or replace the incoming chain with that peer. The shipped nginx and apache2's `mod_proxy` both append it. List a CDN's ranges only if it gives the same guarantee, and keep the vendor's published ranges current.

Naming nothing makes every caller look like the front, so one person's failed sign-ins can fill the count for everybody. Naming too much, especially `0.0.0.0/0` or `::/0`, lets hops that callers write pass as trusted proxies and ends the caller boundary. Keep the list to proxies you control or trust.

## 6. Check the front

`orchctl doctor --url https://orch.example.com` tries the front the way a browser and a machine would, from wherever you run it. `orchctl-linux-x86_64-musl` from the release is one static file that runs on any Linux host. To run it inside the stack: `docker compose exec api /opt/orchctl/orchctl doctor --url …`. Each check prints a line with what was sent and what came back; the exit status is 1 if any check failed.

| Check | What is sent | What it shows |
|-------|--------------|---------------|
| `edge-page` | `GET /` | The application's page comes back with the content policy and the four other security headers |
| `edge-api` | `GET /health` and `GET /api/v1/meta` | The API answers through the front, and which release it is |
| `edge-body` | `POST /edge-check` with 8 MiB | The front carries a body of that size whole: the web edge reads all of it before it answers 204 (one byte more is its own 413) |
| `edge-ws` | A WebSocket to `/api/v1/ws`, held without a word | The upgrade passes and the front does not cut an idle socket |
| `edge-link` | A WebSocket to `/link/v1/session` | The machines' upgrade passes and the gateway answers with `orch-link.v1` |
| `edge-endpoint-N` | One connection to each endpoint the system advertises, with the trust it is listed with | A `pinned` endpoint is reached under the system's own authority (the gateway answers 403 to a caller with no machine certificate, which is how it asks for one); a `public` one through device trust for its name or the installation’s web edge identity |
| `edge-human-check` | `GET /api/v1/meta` and `GET /` | Where a [human check](human-check.md) is on, the content policy lets the page reach its service; the line fails and names the hosts that are missing, with the line to put in a `nginx.conf` kept from an earlier release. Every `Content-Security-Policy` header the address gives is read, so a front of your own that adds or replaces one is seen; what only some visitors are given is not. Where none is on, the policy needs nothing, and the line says so |
| `edge-ports` | `GET /` at the name of an environment that does not exist under `[edge] ports_domain` | The wildcard name resolves and its certificate holds, the system answers it itself (401 or 404) and sets no sign-in of its own there; `part`, since streaming and the end of an entry need a real opening. Skipped without a ports domain |

The API ends an unsigned realtime socket after ten seconds whatever the front does, so without more the `edge-ws` line says `part`: the upgrade passed and the front held it until the API closed it. To hold it for the full 60 seconds as a signed-in page would, give the check a browser session's access token:

```sh
TOKEN=$(curl -s -H 'content-type: application/json' https://orch.example.com/api/v1/auth/login \
        -d '{"login":"you","password":"…"}' | sed -n 's/.*"access_token":"\([^"]*\)".*/\1/p')
ORCH_ACCESS_TOKEN=$TOKEN orchctl doctor --url https://orch.example.com --only edge
```

`--only` and `--skip` choose lines (`edge` is all of them). `--resolve gateway.orch.internal=172.17.0.1` reaches an advertised name that only the Docker host resolves. A `pinned` endpoint is checked against the system's authority, read from the secrets directory when the command runs next to it (inside the stack, `/run/secrets`), otherwise fetched from the system. A `public` one is checked against the device's public and machine roots, or the right service under Orch's own authority. Inside the stack the device roots include the public bundle and `config/ca-certificates/`.

## If it does not work

| You see | Cause and fix |
|---|---|
| `edge-ws` fails: `the edge's idle limit is below 60 s` | The front cuts idle sockets too early. Raise apache2 `ProxyTimeout`, or nginx `proxy_read_timeout`, to 120 seconds |
| `edge-body` says `413: allow request bodies of 8 MiB` | The front limits bodies. Set nginx `client_max_body_size 8m` or the equivalent |
| `edge-link` fails | The front does not pass the upgrade for `/link/v1/session`, or caches or buffers `/link/` |
| An unknown-issuer refusal on a machine | The device does not trust your front's authority. Install the company's public authority in its store |
| **Add machine** shows no command | `public_url` or `[[gateway.endpoints]]` still hold an example address. Replace it |
| Every caller shares one sign-in count | The front is not in `trusted_proxy_cidrs`, so every caller looks like the front. Add it |
| The page reconnects all the time | The front cuts idle connections under 60 seconds |

## Enroll a machine again

A machine that was offline longer than the certificate renewal grace, was revoked, or came back after losing its disk uses **Enroll again** on its existing page. Run that dialog's installer command on the machine. A retained data directory sends its existing identity, and only a token bound to that machine may replace it. The name, page, registrations, policy and local address overrides stay, and the old certificate retires when the new one is first used. A new-machine token on a retained directory is refused with a pointer to **Enroll again**. Inside the renewal grace, an ordinary reconnection renews the certificate by itself.

On the direct way in the gateway verifies a machine by mutual TLS. Through `/link` it verifies a signature over a fresh nonce, the machine id, the dialled host and the clock. A front that ends TLS can read `/link` traffic as it reads browser traffic, so use a front you trust. Keep the direct way in reachable only from the machines you intend.

## What the stack's own web edge does

On every response, errors included, it sets `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer`, `X-Frame-Options: DENY`, a `Permissions-Policy` that denies camera, microphone, geolocation, payment and USB, and a `Content-Security-Policy` that lets the page load only from its own origin (plus the WebAssembly the page runs to typeset documents, and `https:` for a video or sound a document names by address, which the reader's browser fetches only when asked). Files under `/assets/` are named by their content and kept for a year; every other page is `no-store`, so a new release is seen at once. `/internal/` answers 404, and `/edge-check` answers a POST with 204 after reading its body. The services behind it are reached over TLS under the system's own authority, checked by the service's name, and a service that an update makes again is found at its new address without restarting the web edge.
