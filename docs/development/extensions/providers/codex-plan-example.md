---
id: provider-codex-plan-example
title: A Codex subscription provider in Python
section: build-your-own
link_prefix: provider-
screens: /resources/providers, /resources/templates, /resources/machines
keywords: Codex, subscription, provider, Python, Responses, app-server, resets
---

# A Codex subscription provider in Python

`examples/plugins/provider/codex_plan.py` in the source distribution is a custom provider example,
using provider contract 1 over the line-JSON plugin transport and Python 3's standard library. Keep
the adjacent `native_common.py`, `native_endpoints.py` and `plan.py` helpers with it. Nothing installs,
registers or offers this source automatically.

Run `python3 codex_plan.py` beside the native credential, under the **same `CODEX_HOME` and native
environment** as the built-in provider. The unmodified Codex binary uses and renews the existing sign-in.
This program reads no token/login file, runs no sign-in and opens no HTTP server. It never switches a
plan to API-key mode. `--harness-command` selects the installed binary as a JSON argv; tests replace it
with a recorded-answer process. `--at` is only a reproducible test clock.

The descriptor declares facts/models/reset spending, `credential: none`, `presents: null`. The native
keeper still supplies identity and credential independently. Select this program for the existing
provider's facts/models; retain the native endpoint and credential arrangement. See
[Register a program](README.md#register-a-program).

| Fact | Native source |
| --- | --- |
| Plan | `account/read`, then `account/rateLimits/read` |
| Usage windows / UTC resets | Primary/secondary and additional limit ids from that read and `account/rateLimits/updated`; an unstarted reset is distinct from unknown |
| Plan credits | The native decimal balance in vendor credits, not Orch credits |
| Limit reached / ordinary usage allowed | Only explicit vendor refusal/permission, never inferred from a percentage |
| Resets | Native ids, count, types, status and expiry; `account/rateLimitResetCredit/consume` receives the same `creditId` and `idempotencyKey` on every retry, including after an example restart |
| Models and limits | Paginated `model/list`, hidden entries excluded, plus bounded `codex debug models` for missing contexts. Efforts, image input, tiers, access programs and larger contexts remain suggestions. |

A read initializes the app server as the built-in does and starts no model turn. Absent facts are not
read yet; undeclared facts are unsupported. Missing balance/catalogue data never becomes zero money or
a guessed context limit. Replies and native waits are bounded by the source contract.

## Endpoint description

The extra example method `x-endpoint.describe` returns a nonsecret description and the current model
suggestions. It does not create an endpoint or present a credential. The existing native harness makes
requests with the same sign-in.

| Part | Value |
| --- | --- |
| Base / operation | `https://chatgpt.com/backend-api/codex` + `POST /responses` |
| Format / body | `openai-responses`; `store: false`, `stream: true` |
| Credential | `Authorization: Bearer <native access token>`, supplied by Codex, never returned by this source |
| Request headers | Preserve Codex's `chatgpt-account-id`, `originator`, `user-agent` and its current routing/beta/session headers. Account-specific and per-turn values remain with the harness. The ordinary forwarder retains them while presenting its selected credential. |
| Models / limits | The exact current `models.list` parameters; omitted context/output/rate limits remain unknown. Offered tiers/programs/extended context are never enabled automatically. |

The base is the pinned harness's
[`CHATGPT_CODEX_BASE_URL`](https://github.com/openai/codex/blob/rust-v0.162.1/codex-rs/model-provider-info/src/lib.rs).
The protocol follows the Codex adapter's forwarded Responses declaration and the
endpoint contract (`crates/contracts/model/src/endpoint.rs` in the source distribution). This example refuses a different
destination. Do not put a native sign-in in an API-key field.

Conformance tests start the actual program with recorded app-server answers, compare its facts/models
to the built-in converters, check reset idempotency across restart and compare the endpoint protocol,
header scheme and models to the same wire implementation. They need no account or network and do not
establish live vendor behavior.
