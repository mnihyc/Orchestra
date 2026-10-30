---
id: provider-readme
title: Build your own provider source
section: build-your-own
link_prefix: provider-
screens: /resources/providers, /resources/templates, /resources/machines
keywords: provider, plugin, facts source, models source, reset, credential, program, address, SourceDescriptor, SourceAsk
---

# Build your own provider source

A provider keeps the credential that pays for model use. Its six elements
are separate: credential, access to the model, facts, models, resets and
runtime. Each has one implementation for a provider. A key or a native
sign-in describes the credential; it does not decide the layout of its
details or which models a source can list.

The interfaces you can implement are [facts](facts.md), [models](models.md),
[resets through those facts](resets.md) and [key presentation](credential.md).
They have the same JSON in a registered program and at an HTTP address,
except key presentation is a local program only. A source can implement any
subset it declares. Built-in readers use the same contracts. Your source
does not create an Orch provider, change permissions or enable a model.

The machine's Programs workflow runs `describe` and makes a best-effort `models.list`
preview for sources that declare it. This optional metadata call has empty `endpoints`,
`config` and `secrets`, and no provider or credential. A source that needs provider
context may refuse it; registration remains usable and a real provider later supplies
the normal `SourceAsk`. The preview uses the same bounded model-report normalizer as
provider reads. Registration, Test, newly loaded Reload entries and configured-program startup share it;
page reads never call a program. Suggested names stay separate from enabled models.

## Declare what your source does

Worked stdlib programs include the [Codex subscription](codex-plan-example.md) and
[Claude Code subscription](claude-plan-example.md). They keep the built-in native sign-in/request path
and describe its endpoint; neither introduces a proxy or a new sign-in.
The [OpenRouter key example](openrouter-key-example.md) reads a key's usage and budget/reset windows
while offering the vendor's existing Messages and Responses API endpoints.

Every source answers `describe` with a `SourceDescriptor`. Use a name that
is unique on its machine and is not `adapter`, `openrouter`, `deepseek` or
`endpoints`. `facts: null`, `models: null` and `presents: null` mean the
interface is unsupported; `{}` is a supported models/presentation
declaration. At least one interface must be implemented. Facts declares
each supported member in `reports`, its plan labels, whether it spends
listed resets, whether it asks for a key and an optional minimum read
interval. A source spending resets must report `available_resets`.

<!-- BEGIN provider.methods -->

Contract **1**.

| Method | Params schema | Result schema | Declaration | Deadline |
|--------|---------------|---------------|-------------|----------|
| `describe` | [describe-params](schema/describe-params.json) | [source-descriptor](schema/source-descriptor.json) | required | 10 s |
| `facts.read` | [source-ask](schema/source-ask.json) | [facts-report](schema/facts-report.json) | facts | 8 s |
| `models.list` | [source-ask](schema/source-ask.json) | [models-report](schema/models-report.json) | models | 30 s |
| `resets.spend` | [reset-spend-params](schema/reset-spend-params.json) | [reset-report](schema/reset-report.json) | facts.spends_resets | 30 s |
| `credential.present` | [present-ask](schema/present-ask.json) | [presented](schema/presented.json) | presents; keys, program only | 8 s |
<!-- END provider.methods -->

<!-- BEGIN provider.descriptor -->

**SourceDescriptor**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `name` | `string` | required | Its name: one of a kind among the programs of a machine, and among the built-in sources. |
| `title` | `string` | required | What it is called. |
| `summary` | `string` | required | What it is, in one sentence. |
| `contract` | `integer (uint32)` | required | The version of this contract it speaks. |
| `facts` | `SourceDescriptorFacts or null` | may be absent | What it tells of the facts; `null` when it implements no facts. |
| `models` | `SourceDescriptorModels or null` | may be absent | `null` when it lists no models. |
| `presents` | `SourceDescriptorPresents or null` | may be absent | `null` when it presents no credential. |
| `fields` | `Field[]` | may be absent | What it asks of a provider's managers. A secret field is kept as the provider secret `source:<element>:field:<name>`, written once and never shown again. |
| `docs_url` | `string or null` | may be absent | Where its own documentation is. |

**SourceDescriptorFacts**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `reports` | `FactMember[]` | may be absent | The members it can tell. A member it does not list is unsupported until it tells it; a name this contract does not know is left out. |
| `spends_resets` | `boolean` | may be absent | Whether it spends the resets it lists in `available_resets` (`resets.spend`). |
| `credential` | `SourceCredential` | may be absent | Whether it wants the provider's key to read. |
| `min_interval_secs` | `integer or null` | may be absent | The least time between two of its reads, in seconds, at least 60; `null` leaves it to Orch. |
| `plans` | `map of string` | may be absent | The plans it may name in `plan`, by id, with the name to show for each. |

**FactMember**

| Value | Meaning |
|-------|---------|
| `"plan"` | The plan's id. |
| `"windows"` | The usage windows of the plan. |
| `"limit_reached"` | A limit that has been reached. |
| `"ordinary_usage_allowed"` | Whether the vendor admits ordinary use now. |
| `"balances"` | What is left to be spent. |
| `"spend"` | What has been spent, over periods. |
| `"extra_usage"` | Spending beyond the plan. |
| `"available_resets"` | Resets that can be spent. |
| `"identity"` | Who the credential belongs to. |
| `"credential"` | How the credential was given and when it lapses. |
| `"credential_verified"` | Whether this read authenticated with the provider's credential. |

**SourceCredential**

| Value | Meaning |
|-------|---------|
| `"none"` | It reads without the key. |
| `"key"` | It wants the provider's key; it is given only where the provider's credential is a key (and, for an address, only where the provider's managers turned that on for it). |

**Field**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `name` | `string` | required | The member the answer is given under. |
| `label` | `string` | required | What the person reads beside it. |
| `type` | `FieldType` | required | The kind of value. |
| `required` | `boolean` | required | Whether an answer must be given. |
| `help` | `string or null` | may be absent | A sentence of help. |
| `choices` | `array or null` | may be absent | The choices, for a choice. |
| `default` | `JSON; see schema` | may be absent | What stands in the form before the person writes. |
| `max_bytes` | `integer or null` | may be absent | The most bytes an answer may have. |

**FieldType**

| Value | Meaning |
|-------|---------|
| `"text"` | One line of text. |
| `"secret"` | One line that is never shown again. |
| `"secret_text"` | Several lines that are never shown again, such as a key file. |
| `"choice"` | One of the field's choices. |
| `"boolean"` | Yes or no. |
| `"date"` | A calendar day, `YYYY-MM-DD`. |
| `"map"` | Names with values. |
| `"secret_map"` | Names with values that are never shown again. |

**Choice**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `value` | `string` | required | What is stored. |
| `label` | `string` | required | What is shown. |
<!-- END provider.descriptor -->

The complete shapes are [SourceDescriptor](schema/source-descriptor.json)
and [describe parameters](schema/describe-params.json). `fields` uses the
common form field shape in the schema: text, secret, secret_text, choice,
boolean, date, map or secret_map, with label, required, help, choices,
default and max_bytes. Secret values are entered once; the provider page
returns only which names are set. Do not put secrets in a descriptor,
configuration, extension member, stdout diagnostic or error.

## What a call receives

The keeping machine calls your source. [SourceAsk](schema/source-ask.json)
contains the provider's id/name, endpoint protocol/base_url pairs, the
source's nonsecret `config` and only its declared `secrets`. A requested
`credential.key` is supplied only for a key provider and, at an address,
only after its manager confirms sending that key to this host. A native
sign-in is never exported to a source. Do not request another provider's
secrets or infer an authorization from its name.

The keeper still supplies identity and credential alongside a custom facts
source. Your source may report issued/expiry/verification facts only when
it actually used the credential. A declared field is not authentication
proof. Your read never starts a model turn.

## Register a program

Programs speak the plugin transport: newline-delimited
JSON, concurrent request ids, `hello`, `describe`, bounded deadlines,
cancel and shutdown. stdout carries protocol lines only; put safe
diagnostics on stderr. Install the executable first, then have the machine
owner register it in local configuration:

```toml
[[plugin]]
kind = "provider"
name = "acme-plan"
command = ["python3", "/opt/orch-plugins/provider/plan.py", "--state-dir", "/var/lib/acme-demo"]
restart = "on-failure"
```

The worked [program example](facts.md#program-example) supplies the complete
handshake and dispatch. It is ordinary Python 3 and keeps one disposable
toy account in its chosen directory. Use its exact `hello` shape as your
starting point, then declare only the methods you implement.

For a managed provider, a trusted provider template carries its programs:

```json
{
  "plugins": [{
    "kind": "provider", "name": "acme-plan",
    "command": ["python3", "/opt/orch-plugins/provider/plan.py", "--state-dir", "/home/agent/acme-demo"],
    "env": {}, "restart": "on-failure"
  }],
  "after_up": []
}
```

That image must actually include Python and the script. The connector
receives the registrations in `ORCH_PLUGINS`; provider, translator, adapter
and executor kinds all load through this path. Environment templates do
not accept this list. A manager selects a registered program on the
provider's page; no API request installs an executable. Programs run with
the connector user's rights beside the credential, so trust the image
and the program. `ORCH_` variables are reserved.

## Serve an address

An address implements the same methods with a POST JSON envelope. It can
read facts, list models and spend the resets it listed; it cannot present
a key. [The address example](facts.md#address-example) implements a
bearer-protected service with the standard Python HTTP server.

```json
{"contract":1,"method":"describe","params":{}}
```

An HTTP 200 response is exactly one of `{"result": ...}` or
`{"error":{"code":"refused","message":"The read was refused","retryable":false}}`.
Send and accept `application/json`. Orch sends its connector User-Agent;
it keeps no cookies and never follows a redirect. Authorization is none,
a bearer token or one named header with a secret value. It is independent
of the optional provider key in params. The token goes only to this
address, stays write-only and is never part of a provider response.

HTTPS is accepted; plain HTTP requires `providers.allow_http_upstreams`
and is marked **Unencrypted** on the page. Calls originate on the keeping
machine, under its runtime network policy or the connected machine's
`provider.upstreams`. The source's own HTTP 401/403 means its address
authorization failed, not that the provider's model credential failed.

See [request](schema/address-request.json), [reply](schema/address-reply.json),
[source choice](schema/source-choice.json) and [sealed per-call source](schema/source-spec.json).
The choice schemas here describe the stored contract; the provider HTTP API
returns resolved default readers. Its SourceChoiceSet write body is separate:
send expected_revision, omit unchanged elements/secret values, use null to
remove a secret, and never copy the default reader's resolved name into a
write. The API's OpenAPI document describes those public bodies.
Preserving an omitted secret applies only to the unchanged source binding;
changing the address/program drops its previous authorization/secret values.

## Errors, bounds and versions

<!-- BEGIN provider.errors -->

| Error code | Retryable |
|------------|-----------|
| `unsupported` | false |
| `not_told` | false |
| `refused` | false |
| `timeout` | true |
| `unavailable` | true |
| `malformed` | false |
| `too_large` | false |
| `redirected` | false |
| `unauthorized` | false |
| `policy.upstream_not_allowed` | false |
| `credential_unavailable` | true |

The error object is [SourceError](schema/source-error.json). HTTP 3xx is redirected (never followed), 401/403 unauthorized, 429/5xx unavailable; another non-200 status or a wrong envelope is malformed. A safe error is kept beside this element, retaining the last facts/time.

**SourceError**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `code` | `SourceErrorCode` | required | What went wrong. |
| `message` | `string` | required | A sentence on it, in the source's words; every secret replaced by `[secret]` before it is kept. |
| `retryable` | `boolean` | may be absent | Whether asking again may succeed. |

**SourceErrorCode**

| Value | Meaning |
|-------|---------|
| `"unsupported"` | The method is not implemented. |
| `"not_told"` | The source could not tell now: its vendor was silent. |
| `"refused"` | The vendor refused this read. |
| `"timeout"` | No answer within the method's deadline. |
| `"unavailable"` | The program is down or restarting, the address cannot be reached, or it answered HTTP 5xx or 429. |
| `"malformed"` | The answer is not in its shape: the message names the first wrong member. |
| `"too_large"` | The answer is over its size, or a list over its cap. |
| `"redirected"` | An address answered with a redirection, which is never followed. |
| `"unauthorized"` | An address refused the authorization Orch holds for it (HTTP 401 or 403). |
| `"policy.upstream_not_allowed"` | The address is not among the upstreams a person's machine allows. |
| `"credential_unavailable"` | A key could not be presented for a model request. |
<!-- END provider.errors -->

<!-- BEGIN provider.bounds -->

| Bound | Value |
|-------|-------|
| `CONTRACT` | 1 contract version |
| `DESCRIBE_DEADLINE` | 10 seconds for describe |
| `READ_DEADLINE` | 8 seconds for facts.read / credential.present |
| `LIST_DEADLINE` | 30 seconds for models.list / resets.spend |
| `REPORT_MAX_BYTES` | 1048576 bytes per reply |
| `TEXT_MAX_CHARS` | 200 characters |
| `DECIMAL_MAX_CHARS` | 40 characters of decimal text |
| `ERROR_MESSAGE_MAX_CHARS` | 300 characters after redaction |
| `WINDOWS_MAX` | 64 windows |
| `BALANCES_MAX` | 32 balances |
| `SPEND_MAX` | 64 spend rows |
| `RESET_CREDITS_MAX` | 64 reset credits |
| `MODELS_MAX` | 10000 models |
| `MIN_INTERVAL_LEAST_SECS` | 60 seconds |
| `DESCRIBE_EVERY` | 86400 seconds |
| `PRESENTED_RENEW_BEFORE` | 60 seconds before expiry |
| `PRESENTED_REUSE_WITHOUT_EXPIRY` | 300 seconds |

Secret replacement text: `[secret]`. Amount syntax: `^-?[0-9]+(\.[0-9]+)?$`; UTC times use RFC 3339.
<!-- END provider.bounds -->

Contract 1 adds members without changing their meaning. Readers ignore
unknown members; known optional members may be absent. `x_` members of
declared extension-bearing objects are retained without interpretation.
Unknown report members do not become permissions or provider state.
Invalid list items and items beyond a cap are left out with a safe note,
while valid items are kept. An overlarge envelope is refused. Errors must
name the wrong member or safe reason, never the request or a token.

Use only the interfaces and credentials your vendor permits. Orch adds
no sign-in flow for a reserved vendor credential and ships no format
translator beyond the extension seam. A presenter authorizes keys only.

## Check before using a source

```sh
orchctl plugin check --fixture ./disposable-provider-fixture.json python3 examples/plugins/provider/plan.py --state-dir ./disposable-acme
orchctl plugin check --address https://plan.acme.example/provider --token-file ./source-token --fixture ./disposable-provider-fixture.json
```

The fixture must match the conformance module's public Fixture schema and
use a disposable account. A reset check requires explicit fixture consent
to spend its named test-only credit; the suite never silently consumes a
real account reset. A skipped rule is no proof that the interface works.
The same suite checks shapes, four states, models, replayable resets,
presentation where supported, quiet errors and time/size bounds for both
bindings. Installing, registering and selecting the source are separate
acts; the check changes no configuration.

The fixture is one JSON object; it contains no live credential in these
examples. This table is the input of the suite, distinct from the source's
wire requests:

| Field | Shape | Purpose |
|-------|-------|---------|
| facts | `[{ask: SourceAsk, expected: JSON}]` | Exact raw reports for absent, null, empty and value cases. |
| models | `[{ask: SourceAsk, expected: JSON}]` | Exact wrapped models reports for null, empty and nonempty cases. |
| reset | `{ask: SourceAsk, reset: {reset_id, idempotency_key}}` or null | Explicit consent for that disposable reset; null spends nothing and skips the rule. |
| present | `{ask: PresentAsk, expected: JSON or null}` or null | Disposable key exchange, optional exact result; null skips the rule. |
| refusal | `{ask: SourceAsk, present: PresentAsk or null}` or null | A controlled refusal carrying disposable secret values to check quiet errors. |
| now | UTC RFC 3339 string | Controlled clock for checking presentation expiry. |
| secrets | string array | Additional disposable values, such as an address token, that must not appear in results/errors. |

For a first, read-only example check, save this as
`disposable-provider-fixture.json`. It checks an exact absent report and
unsupported and empty model answers; the complete states, reset and presentation
rules remain Skipped until their controlled cases are supplied. It never
consumes a credit or uses a model:

```json
{
  "facts": [{"ask":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{"mode":"absent"},"secrets":{}},"expected":{}}],
  "models": [
    {"ask":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{"mode":"null"},"secrets":{}},"expected":{"models":null}},
    {"ask":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{"mode":"empty"},"secrets":{}},"expected":{"models":[]}}
  ],
  "reset": null, "present": null, "refusal": null,
  "now": "2026-10-15T00:00:00Z", "secrets": []
}
```

Use `--at 2026-10-15T00:00:00Z` on the example program/address when adding
exact normal/presentation cases. Build the expected raw answers from the
example's documented modes; do not default-fill missing members before
comparing. Add a reset fixture only for a fresh disposable directory and
its actually listed `acme-reset`. The full example tests exercise every
applicable rule through the real program and address bindings.

`cargo xtask contracts` generates these pages' marked tables, constants,
schemas and exact worked example bytes from their contract/example sources.
Its `--check` and page regression test refuse drift or a missing marker.
The generated schemas describe params/results; the prose supplies the
presence, authorization and effect rules a JSON validator cannot express.
