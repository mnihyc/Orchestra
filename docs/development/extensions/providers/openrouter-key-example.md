---
id: provider-openrouter-key-example
title: An OpenRouter key provider in Python
section: build-your-own
link_prefix: provider-
screens: /resources/providers, /resources/templates, /resources/machines
keywords: OpenRouter, provider, Python, key, credits, budget, usage, reset, Messages, Responses
---

# An OpenRouter key provider in Python

`examples/plugins/provider/openrouter_key.py` in the source distribution is a custom provider example,
with the same facts as the built-in OpenRouter reader and direct API endpoints. It uses Python 3's standard
library and the adjacent `native_common.py`/`plan.py` helpers. Nothing installs, registers or selects it.
It creates no HTTP server and implements provider contract 1 over line JSON.

Run `python3 openrouter_key.py` beside the credential, register the program as described in the
[provider guide](README.md#register-a-program), then select it for a key provider's facts/models. Its
descriptor asks for that provider's key; do not put a key in command arguments or source fields.
`credential.present` supplies the key only to OpenRouter's documented endpoint. Native subscription
sign-ins are not API keys and do not belong in this example. `--recording`/`--at` are offline test controls,
never a way to validate or use a real account.

## Facts and models

`GET /api/v1/key` supplies per-key usage, limit/reset, applicable budgets and the binding budget.
Resetting limits become windows with the next UTC boundary: midnight daily, Monday midnight weekly,
and the next month's first midnight monthly. The source preserves an explicit vendor reset instant.
Free-model requests form a daily request-count window; this does not turn requests into tokens or USD.
Spend rows retain the vendor's periods/units and decimal text. Missing facts stay unread; no amount,
reset or expiry is guessed. See OpenRouter's [limits guide](https://openrouter.ai/docs/api_reference/limits)
and [current-key reference](https://openrouter.ai/docs/api/api-reference/api-keys/get-current-api-key).

The four `byok:*` USD spend rows retain BYOK usage over the vendor's total, UTC day, Monday-based UTC week
and UTC month. A missing BYOK amount is omitted, never replaced by zero. The literal `is_free_tier`
boolean is preserved as `x_openrouter_is_free_tier` on `key:total`; it does not invent a subscription plan.

Account credit from `/api/v1/credits` is management-key-only. A refusal leaves the successful key read
intact and the unsupported account group absent, rather than reporting zero credit or failing the key.
See the [credits reference](https://openrouter.ai/docs/api/api-reference/credits/get-remaining-credits).
Changing budgets or spending a reset is not supported. Model suggestions come from `/api/v1/models`,
including reported context on offer and exact per-million prices; a manager still enables them.

## Everything needed for a model request

`x-endpoint.describe` is an example-only metadata method. It returns both endpoint descriptions and
their models/limits, without returning the key or installing an endpoint. `credential.present` is the
ordinary keys-only source interface; both formats present `Authorization: Bearer <OpenRouter key>`.

| Harness | Protocol and request | Required headers / body |
| --- | --- | --- |
| Codex | `openai-responses`; base `https://openrouter.ai/api/v1`, `POST /responses` | `Authorization: Bearer …`, `Content-Type: application/json`; `store: false`, full history each request, no `previous_response_id`; streaming is supported |
| Claude Code | `anthropic-messages`; base `https://openrouter.ai/api`, `POST /v1/messages` | `Authorization: Bearer …`, `Content-Type: application/json`, `anthropic-version: 2023-06-01`; preserve the harness's current request/beta headers. No Anthropic plan OAuth marker is manufactured for an OpenRouter key. |

These values follow OpenRouter's [Responses guide](https://openrouter.ai/docs/api_reference/responses/overview)
and [Claude Code guide](https://openrouter.ai/docs/guides/coding-agents/claude-code-integration). The latter
sets `ANTHROPIC_BASE_URL=https://openrouter.ai/api` and `ANTHROPIC_AUTH_TOKEN` to the OpenRouter key, with
`ANTHROPIC_API_KEY` explicitly empty to avoid conflicting credentials. In Orch, the existing key/forwarder
arrangement performs that handoff; this program introduces no second proxy or sign-in. Model IDs are the
exact IDs from OpenRouter, not aliases invented by the source. Unsupported limits stay omitted.

HTTP reads use the fixed documented HTTPS origin, refuse redirects and bound reply size/time; refusals
never quote the key. The conformance test runs this actual program with recorded HTTP bodies, uses those
same bodies in the real built-in source through a named HTTP double, and compares facts at UTC reset
boundaries and model suggestions. It uses no network/account. Live key and harness turns are separate proof.
