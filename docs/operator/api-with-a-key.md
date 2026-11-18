# Use the API with a key

Call Orch's HTTP API from a script or another program with an account key, and know which surface to use for what. At the end a request with your key returns an answer, and you know which paths belong to the key surface and which stay with the browser.

## Before you start

You need:

- An account key. A person makes a personal key, and a service account has its own, in **Settings → Tokens**. Keys start with `opat_`. Each key is limited by the operation, action and target rules you give it, and by the permissions of its account at the time of the call.
- The address of the system, such as `https://orch.example.com`.
- The [OpenAPI document](../openapi.json), which describes the key surface. A running system also serves it at `/openapi/v1/openapi.json`.

## 1. Call it

1. Send the key as a bearer token to the `/openapi/v1` prefix:

   ```sh
   curl -H "Authorization: Bearer $ORCH_KEY" https://orch.example.com/openapi/v1/me
   ```

2. Or use the command line that ships with Orch. It takes the address in `ORCH_URL` and the key in `ORCH_TOKEN`, and prints the answer as it came back:

   ```sh
   ORCH_URL=https://orch.example.com ORCH_TOKEN=opat_… orchctl api GET /me
   ORCH_URL=https://orch.example.com ORCH_TOKEN=opat_… orchctl api GET '/usage?scope=me'
   ```

   The method is `GET`, `POST`, `PUT`, `PATCH` or `DELETE`; a body goes in `--data` as JSON, `@file` or `-` for standard input. A relative path gets the `/openapi/v1` prefix. A path that starts with `/api/...` or `/openapi/...` is used as given and is still checked by the server. The exit status is 0 for an answer in the 200s, the first digit of the status (3, 4 or 5) otherwise, and 1 when no answer came or the request was wrong before it was sent. A body can be at most 8 MiB. Use `--cacert FILE` to trust the authority of a system that runs under its own.

**What you will see.** The JSON answer for that operation, and the status code. A refusal says which credential or permission is missing.

## The two surfaces

| | Browser | Key |
|---|---|---|
| Prefix | `/api/v1` | `/openapi/v1` |
| Credential | The session's access token (`oat_`) | A personal or service API key (`opat_`) |
| Realtime socket | `/api/v1/ws` | `/openapi/v1/ws` |
| Published description | None. The browser's contract is not published | The OpenAPI document |

Each surface refuses the other credential. A key surface does not accept a browser cookie. Browser refresh and sign-out keep their session cookie and CSRF rules.

Both prefixes reach the same operations, with the same permissions, key restrictions and checks of current state. **A prefix grants no authority.** A command that uses a browser access token must name its browser path in full.

## Open a realtime connection

Both sockets speak the same `orch.v1` frames. The HTTP upgrade fixes which kind of credential the connection uses: the first `auth` and every `reauth` must use that kind, and a connection cannot switch between a browser session and a key, even for the same account. A key's reauthentication keeps its same-key origin and the scope ceiling it was accepted with. Subscriptions, delivery checks, revocation and the real expiry of the credential follow their usual rules.

## Follow a ticket

An operation that opens a terminal, a browser, a display or a port gives a one-use ticket. Redeem it under the same prefix as the request that made it. The prefix is only a record of where the request came from: it is not a permission, and a wrong-prefix attempt does not use up a ticket that is still good under its own prefix.

| Path under that prefix | Protocol |
|---|---|
| `/ws/terminals/{terminal_id}` | `orch.terminal.v1` |
| `/ws/agent-terminals/{thread_id}` | `orch.terminal.v1` |
| `/ws/browsers/{browser_id}` | `orch.browser.v1` |
| `/ws/displays/{display_id}` | `orch.display.v1` |
| `/ws/ports/{use_id}` | `orch.port.v1` |

These are ticket protocols, not further operations with a bearer. The ticket query, its single use and expiry, and the checks of the caller and the grant stay in force. Public port links are still read and used at `/api/v1/port-links/...`, and the port socket they lead to uses `/api/v1`. The port-forward helper of the connector picks the matching prefix for a browser token or a key and keeps the public route for a link's proof.

## What stays under `/api/v1`

Sign-in, registration, OIDC callbacks, recovery, public metadata, the downloads (`/api/v1/downloads`) and the certificate authority, and public port-link addresses keep their `/api/v1` paths. Session and device cookies stay in the scope of `/api/v1/auth`. An identity provider set up through a key still gets a browser OIDC callback address. The machine protocol at `/link/v1` and the service protocols under `/internal` keep their own credentials and paths. Agent tool contexts get no HTTP surface. The stable hosts of ports keep their own code, cookie and proof protocol.

## If it does not work

| You see | What to do |
|---|---|
| A browser token is refused on `/openapi/v1` | The key surface takes keys only. Use an `opat_` key, or call `/api/v1` with the session token |
| A key is refused on `/api/v1` | The browser surface takes session tokens only. Use `/openapi/v1` |
| An operation the account may do is refused | The key's own rules (operation, action, target) do not allow it. Make a key that does, or change its rules |
| A ticket is refused | It was redeemed under another prefix than the request that made it, or it has been used or has expired. Ask for a new one |
| `orchctl api` exits 1 | No answer came, or `ORCH_URL` or `ORCH_TOKEN` is missing. The message says which |
