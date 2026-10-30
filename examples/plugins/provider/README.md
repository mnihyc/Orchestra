# A made-up provider, as a program or an address

Acme is a disposable example account. It reports a five-hour and weekly usage
window with reset times, a USD balance, monthly spend, one reset credit and two
model suggestions. It reaches a limit while the file `limited` exists in its
state directory. Spending the example reset takes that flag away and consumes
the credit once; the same idempotency key returns the same saved result after a
lost reply or a process restart. No real vendor, money, model or account is used.

Both programs are plain Python 3 with only the standard library:

| File | Role |
|------|------|
| [plan.py](plan.py) | Plugin kind `provider`, line JSON on stdin/stdout, including a toy keys-only credential exchange. |
| [address.py](address.py) | The same facts, models and resets through `http.server`, protected by a bearer token. It imports the adjacent `plan.py`; keep the two files together. |
| [codex_plan.py](codex_plan.py) | Codex's native app-server account/limits/reset/models calls, beside the existing native sign-in. [Guide](../../../docs/development/extensions/providers/codex-plan-example.md). |
| [claude_plan.py](claude_plan.py) | Claude Code's native stream-json `get_usage`, limit frames and models, beside the existing native sign-in. [Guide](../../../docs/development/extensions/providers/claude-plan-example.md). |
| [openrouter_key.py](openrouter_key.py) | An API key with usage/budget/reset windows, optional management-key account credit, model proposals and direct Messages/Responses endpoint descriptions. [Guide](../../../docs/development/extensions/providers/openrouter-key-example.md). |

The two native-sign-in examples also need `native_common.py`, `native_endpoints.py` and `plan.py`
alongside them. `x-endpoint.describe` is their example-only endpoint description method; it starts no
HTTP server and never returns a token. Keep the native harness's credential/request path, as each guide
explains. Missing facts/limits stay unknown, and no source or model is installed/offered automatically.

Each state directory is one fake account, owned by **one** program/server process
at a time. Use a fresh directory for an independent test. The examples create
only their account state there, with no installation or account enrollment.

## Program

```sh
python3 plan.py --state-dir /tmp/orch-acme-example
```

Send one JSON object per line. Answers retain the request `id`; stdout carries
no log messages. The first exchange negotiates the common plugin transport:

```json
{"id":0,"method":"hello","params":{"kind":"provider","transport":[1,1],"contract":[1,1],"host_version":"0.1.0"}}
{"id":1,"method":"facts.read","params":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{},"secrets":{}}}
```

`describe` declares facts members, model listing and reset spending independently.
It reports no identity or credential-verification proof. Its facts read needs no
provider key. `ping`, unknown methods, `cancel` and `shutdown` use the common
transport; unknown methods answer `unsupported`, and secret fields are never
quoted in a refusal.

An operator who chooses to use this program registers it beside the credential,
for example in a connector's local configuration (or in a provider template's
declared programs where the image already includes Python and these files):

```toml
[[plugin]]
kind = "provider"
name = "acme-plan"
command = ["python3", "/opt/orch-examples/provider/plan.py", "--state-dir", "/tmp/orch-acme-example"]
```

Nothing in the repository registers it automatically. Choosing a source of
details does not offer or enable either model without the ordinary model acts.

## Address

Give the server a file containing one example bearer token, kept private:

```sh
umask 077
printf '%s' 'example-token' > /tmp/acme-token
python3 address.py --state-dir /tmp/orch-acme-address --token-file /tmp/acme-token --port 41080 --ready-file /tmp/acme-ready.json
```

It binds loopback only. `--port 0` asks the kernel for a free port; the ready file
is published atomically with the actual final URL and no token. The server also
writes that nonsecret URL as one JSON readiness line on stdout. This HTTP
example is for an internal network: Orch must permit plain HTTP for that provider,
and on a person's machine its local `provider.upstreams` must permit the address.
Otherwise the connector refuses it. Use the address's bearer authorization; no
provider key needs to leave the keeping machine for these toy facts.

Requests to the final `/provider` address are `POST` with JSON:

```json
{"contract":1,"method":"facts.read","params":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{},"secrets":{}}}
```

Send `Content-Type: application/json`, `Accept: application/json` and
`Authorization: Bearer <token>`. A successful reply is `{"result": ...}`;
a refused method is `{"error":{"code":"unsupported","message":"...","retryable":false}}`.
A missing/wrong token gets HTTP 401. It issues no redirects, cookies or access
logs. `credential.present` is deliberately absent and refused on this binding.
The example accepts at most one MiB per JSON request; its replies are small.

## Controlled behavior and effects

The `config.mode` field supports `normal`, `absent`, `null`, `empty` and `refused`.
For facts these keep a missing member apart from an explicit null, an empty
list, and a value. For models the answers are explicit null (unsupported), an
empty list, or two suggestions. `demo_secret` is an optional declared secret
field for testing that refusals remain quiet. The source never returns it.

Create `limited` in the chosen state directory to simulate a limit. The next
read reports `ordinary_usage_allowed: false` and the five-hour `limit_reached`;
removing it admits ordinary use. `resets.spend` takes the ordinary `SourceAsk`
members plus `reset_id: acme-reset` and one `idempotency_key`. The first call
consumes its only credit and removes that directory's marker; a replay returns
its saved outcome and facts, and a different key after it is spent is refused.
Use only a disposable directory for conformance reset fixtures.

The program-only `credential.present` call accepts the provider, config and
secrets, a toy `credential.key`, and `endpoint {protocol, base_url}`. It derives an `authorization` header with an
`acme-demo-` HMAC token, expiring five minutes after the read. It authorizes no
real service, makes no network request and never obtains or presents a vendor's
sign-in credential. It is an illustration of the keys-only seam.

`--at 2026-10-15T00:00:00Z` fixes the clock for exact fixtures; omit it normally.
The existing provider conformance suite exercises the same traits for the actual
program bridge and connector address client, with separate temporary accounts.
It checks declared shapes/states, decimal amounts, models, one reset replay,
headers/expiry where declared, quiet errors and bounds. Test source and example
files are not evidence that the suite or the release's placement walks ran.
