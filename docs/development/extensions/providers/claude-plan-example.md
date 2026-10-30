---
id: provider-claude-plan-example
title: A Claude Code subscription provider in Python
section: build-your-own
link_prefix: provider-
screens: /resources/providers, /resources/templates, /resources/machines
keywords: Claude Code, subscription, provider, Python, Messages, get_usage, stream-json
---

# A Claude Code subscription provider in Python

`examples/plugins/provider/claude_plan.py` in the source distribution speaks provider contract 1 over
the line-JSON plugin transport. It uses Python 3's standard library and the adjacent `native_common.py`,
`native_endpoints.py` and `plan.py` helpers. Nothing installs, registers or selects it automatically.

Run `python3 claude_plan.py` beside the built-in provider's existing credential, under the **same
`CLAUDE_CONFIG_DIR` and native environment**. It initializes unmodified Claude Code's stream-json control
channel and sends `get_usage` with `skip_behaviors: true`, without a prompt or a model turn. The sign-in
stays with Claude Code: this program reads no token file and makes no OAuth HTTP call. It adds no proxy,
sign-in or API-key mode. `--harness-command` selects a JSON argv; tests substitute a recorded process.

Its descriptor declares facts/models, `credential: none`, `presents: null`. The native keeper continues
to supply identity and credential facts independently. Choose the program for the existing provider's
facts/models; retain its native endpoint/sign-in arrangement. See [registration](README.md#register-a-program).

| Fact | Native source |
| --- | --- |
| Plan | `get_usage.subscription_type` |
| Windows and UTC resets | Explicit `rate_limits` only when `rate_limits_available` is true: session, weekly and model-family percentages/resets |
| Extra usage | Only the native enabled/limit/used/percent/unit/precision/reason/refusal members; absent precision stays unknown |
| Limit reached / binding window | `rate_limit_event` unified windows, thresholds and explicit `rejected`. Warning alone does not block work or prove recovery. |
| Models / limits | Initialize response: aliases resolved, duplicates/disabled entries excluded; efforts, image input and fast mode offered as the built-in does. Missing contexts/output/rates stay unknown. |
| Reset spending and account balances | Unsupported; no native operation or amount is invented |

The pinned 2.1.296 answers this request. The SDK names it
`usage_EXPERIMENTAL_MAY_CHANGE_DO_NOT_RELY_ON_THIS_API_YET`: a refusing/older build leaves facts **not read
yet**, without a token-based HTTP fallback or fabricated zeroes. Its session `total_cost_usd`/token
counters are not account credits/spending. The built-in reads the same control envelope and limit frames.

## Endpoint description

The example-only `x-endpoint.describe` method returns the following nonsecret description and current
model suggestions. It installs no endpoint and takes no credential into custody.

| Part | Value |
| --- | --- |
| Base / operation | Claude Code's `https://api.anthropic.com` + `POST /v1/messages` |
| Format | `anthropic-messages`, streamed Messages requests |
| Credential | `Authorization: Bearer <native access token>`, supplied by Claude Code, never an API-key field or a source-returned token |
| Required version / plan marker | `anthropic-version: 2023-06-01`; `anthropic-beta: oauth-2025-04-20`. Preserve additional native beta flags rather than replacing them. |
| Other headers | Preserve the harness's `x-app`, `user-agent` and per-request headers, except hop-by-hop and forwarder-owned headers. Their dynamic values stay native. |
| Models / limits | `models.list` suggestions with reported parameters; missing limits remain omitted |

The Messages authorization implementation (`extensions/protocols/anthropic-messages/src/authorization.rs`)
supplies a version only when absent and preserves native version/beta headers; the common
endpoint contract (`crates/contracts/model/src/endpoint.rs`) supplies Bearer presentation.
Do not introduce a server that intermediates a person's plan sign-in or paste that sign-in into source fields.

Recorded tests run this program with the adapter's exact 2.1.296 usage fixture and recorded turn limit
frames, compare facts/models to the built-in converters, and compare the endpoint contract to the Messages
implementation. No account/network is used; a live vendor proof is separate.
