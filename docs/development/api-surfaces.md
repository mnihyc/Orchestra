# HTTP surfaces and continuations

Browser requests use `/api/v1` with the session access bearer (`oat_`). Account-key requests use
`/openapi/v1` with a personal or service API key (`opat_`). Each surface refuses the other credential
kind. Browser refresh/logout retain their existing session cookie and CSRF requirements. A key
surface does not authenticate a browser cookie.

Both mounts use the same registered business handlers, operation definitions, ordinary privileges,
key restrictions and current-state checks. A prefix grants no authority. The public OpenAPI document
at `/openapi/v1/openapi.json` describes the key business surface. The browser's full contract is an
unpublished generator input for frontend types; there is no browser OpenAPI endpoint.

`orchctl api` prefixes relative operation paths with `/openapi/v1`. Explicit `/api/...` and
`/openapi/...` paths remain as supplied and remain subject to server credential checks. A command
using a browser access token must explicitly name its browser path.

## Existing realtime protocol

Use `/api/v1/ws` for browser sessions and `/openapi/v1/ws` for account keys. Both speak the existing
`orch.v1` frames. The HTTP upgrade fixes the credential surface. Initial `auth` and every `reauth`
must use that kind; a connection cannot switch from a browser session to a key or back, even for the
same account. Key reauthentication retains its existing same-key origin and accepted scope ceiling.
Subscriptions, delivery checks, revocation and actual credential expiry keep their existing rules.

## Existing ticket continuations

A ticket is redeemed under the same prefix as the request that created it. The existing ticket and
wire protocol remain unchanged. The prefix is private transport-origin metadata, not an ACL or a
new grant. A wrong-prefix attempt does not consume a ticket usable under its original prefix.

| Continuation path under that prefix | Existing protocol |
|---|---|
| `/ws/terminals/{terminal_id}` | `orch.terminal.v1` |
| `/ws/agent-terminals/{thread_id}` | `orch.terminal.v1` |
| `/ws/browsers/{browser_id}` | `orch.browser.v1` |
| `/ws/displays/{display_id}` | `orch.display.v1` |
| `/ws/ports/{use_id}` | `orch.port.v1` |

These are ticket protocols, not additional bearer-authenticated business operations. Keep the
existing ticket query, single-use/expiry checks and current caller/grant checks. Public port links
are still inspected/used at `/api/v1/port-links/...`; their resulting port socket uses `/api/v1`.
The connector port-forward helper selects the matching use/socket prefix for a browser bearer or
API key, and retains the public route for a link proof.

Login, registration, OIDC callbacks, recovery, public metadata, CA/download endpoints and public
port-link URLs keep their existing `/api/v1` paths. Session/device cookies keep `/api/v1/auth` scope.
An IDP configured through a key still receives a browser OIDC callback URL.

Machine `/link/v1` and service mTLS `/internal` protocols keep their own credentials and paths.
Internal downloads retain their existing public `/api/v1/downloads` subtree. Agent/tool contexts do
not acquire an HTTP surface. Stable port hosts retain their separate code/cookie/proof protocol.
