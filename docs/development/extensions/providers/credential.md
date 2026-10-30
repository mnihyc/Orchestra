---
id: provider-credential
title: Implement key presentation
section: build-your-own
link_prefix: provider-
screens: /resources/providers, /resources/templates, /resources/machines
keywords: credential.present, CredentialPresenter, PresentAsk, Presented, headers, expires_at, credential_unavailable
---

# Implement key presentation

A locally registered provider program may implement `credential.present`
for a **key**. Its descriptor has `presents: {}`; unsupported is null.
This is an exchange or signing step before Orch's ordinary model request,
not a way to obtain or forward a native sign-in. Respect the vendor's
credential terms. There is no address binding for presentation: a manager's
typed reporting address does not authorize every model request.

<!-- BEGIN provider.credential -->

**present-ask**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `provider` | `SourceAskProvider` | required | The provider. |
| `endpoint` | `SourceAskEndpoint` | required | The endpoint the model request goes to. |
| `config` | `object` | may be absent | What the provider's managers gave for the source's fields that are no secrets. |
| `secrets` | `map of string` | may be absent | The values of the source's secret fields. |
| `credential` | `SourceAskCredential` | required | The key to present. |

**SourceAskProvider**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `id` | `string (uuid)` | required | Its identifier. |
| `name` | `string` | required | Its name. |

**SourceAskEndpoint**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `protocol` | `string` | required | The protocol it speaks, such as `openai-responses` or `anthropic-messages`. |
| `base_url` | `string` | required | Its address. |

**SourceAskCredential**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `key` | `string` | required | The provider's key. |

**presented**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `headers` | `map of string` | required | The headers a model request carries in place of the key's own presentation, by name. |
| `expires_at` | `string or null` | may be absent | When they stop being good; `null` where the source does not say. |
<!-- END provider.credential -->

[PresentAsk](schema/present-ask.json) contains provider id/name, endpoint
protocol/base_url, source config, declared secrets and `credential.key`.
The key belongs only on this local channel. Return
[Presented](schema/presented.json): a map of HTTP header names to secret
values and an optional UTC `expires_at`.

```json
{"headers":{"authorization":"Bearer example-exchanged-token"},"expires_at":"2026-10-15T00:05:00Z"}
```

These are fictitious values. Do not emit the real key, headers or full
request in diagnostics. Your result goes to the forwarder, never the
page, a timeline, audit row or error. It supplies key presentation, not
arbitrary routing or hop-by-hop headers; leave the HTTP framing and
destination to the configured endpoint.

Orch reuses the headers until a minute before their reported expiry,
or for five minutes when expiry is unknown. A failed presentation
returns `credential_unavailable` in the call's protocol (HTTP 502 where
the forwarder supplies HTTP), without changing the provider's credential
status. Do not report an optional exchange outage as model authorization
proof or make the provider unusable through another credential path.

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

## Python program example

This complete Acme program derives a toy HMAC token from the key and
configured endpoint and gives it a five-minute life. It contacts no
vendor and authorizes no real service. Its credential method illustrates
the local keys-only seam; facts and models in the same example do not
need that key. Register it as [a program](README.md#register-a-program)
on the keeping machine or in a trusted provider template that includes
Python and the script.

<!-- BEGIN example.program -->

```python
#!/usr/bin/env python3
"""A made-up Acme account, shared by the provider program and address examples.

Plain Python 3. No network, real vendor account, credential store or additional module is used.
One --state-dir is one toy account. Its `limited` file simulates the vendor reaching a limit.
"""

import argparse
from datetime import datetime, timedelta, timezone
import hashlib
import hmac
import json
import os
from pathlib import Path
import sys

TRANSPORT_VERSION = 1
CONTRACT_VERSION = 1
NAME = "acme-plan"
VERSION = "0.1.0"
METHODS = ["describe", "facts.read", "models.list", "resets.spend", "credential.present"]
MEMBERS = ["plan", "windows", "limit_reached", "ordinary_usage_allowed", "balances", "spend", "available_resets"]
DESCRIPTOR = {
    "name": NAME, "title": "Acme example plan", "summary": "A made-up account with usage windows, money and one reset.",
    "contract": CONTRACT_VERSION,
    "facts": {"reports": MEMBERS, "spends_resets": True, "credential": "none",
              "min_interval_secs": 60, "plans": {"acme": "Acme"}},
    "models": {}, "presents": {},
    "fields": [
        {"name": "mode", "label": "Example answer", "type": "choice", "required": False,
         "help": "Exercise normal, absent, null, empty or refused answers in a disposable example.",
         "choices": [{"value": value, "label": value} for value in ("normal", "absent", "null", "empty", "refused")],
         "default": "normal", "max_bytes": None},
        {"name": "demo_secret", "label": "Example secret", "type": "secret", "required": False,
         "help": "A test value that is never returned, logged or quoted in an error.",
         "choices": None, "default": None, "max_bytes": 200},
    ],
    "docs_url": None,
    "x_example": True,
}

# The plan and the example's small local request bound, not production vendor limits.
FIVE_HOURS = timedelta(hours=5)
WEEK = timedelta(days=7)
TOKEN_LIFETIME = timedelta(minutes=5)
MAX_REQUEST_BYTES = 1024 * 1024
MAX_ID_CHARS = 200


class Refusal(Exception):
    def __init__(self, code, message, retryable=False):
        super().__init__(message)
        self.code, self.message, self.retryable = code, message, retryable

    def wire(self):
        return {"code": self.code, "message": self.message, "retryable": self.retryable}


def encoded(value):
    return json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":"), allow_nan=False)


def utc(text):
    at = datetime.fromisoformat(text.replace("Z", "+00:00"))
    if at.tzinfo is None or at.utcoffset() != timedelta(0):
        raise ValueError("the example clock must be UTC")
    return at


def stamp(at):
    return at.isoformat(timespec="milliseconds").replace("+00:00", "Z")


class Acme:
    def __init__(self, directory, at=None):
        self.directory = Path(directory)
        self.directory.mkdir(parents=True, exist_ok=True)
        self.state_file = self.directory / "account.json"
        self.limit_file = self.directory / "limited"
        self.fixed = utc(at) if at else None
        self.started = self.now()
        self.state = json.loads(self.state_file.read_text()) if self.state_file.exists() else {"spent": False, "reply": None}

    def now(self):
        return self.fixed or datetime.now(timezone.utc)

    def mode(self, params):
        mode = (params.get("config") or {}).get("mode", "normal")
        if mode == "refused":
            # Deliberately never include params: that object can hold a key and secret fields.
            raise Refusal("refused", "Acme refused this example read")
        if mode not in ("normal", "absent", "null", "empty"):
            raise Refusal("malformed", "config.mode is not an example mode")
        return mode

    def facts(self, params):
        mode = self.mode(params)
        if mode == "absent":
            return {}
        if mode == "null":
            return {member: None for member in MEMBERS}
        if mode == "empty":
            return {"plan": None, "windows": [], "limit_reached": None, "ordinary_usage_allowed": None,
                    "balances": [], "spend": [], "available_resets": None}
        now = self.now()
        # Fixed reset boundaries within the lifetime of this demo process, advanced after expiry.
        five = self.started + FIVE_HOURS * (max(0, (now - self.started) // FIVE_HOURS) + 1)
        weekly = self.started + WEEK * (max(0, (now - self.started) // WEEK) + 1)
        monthly = datetime(now.year + (now.month == 12), now.month % 12 + 1, 1, tzinfo=timezone.utc)
        limited = self.limit_file.exists()
        spent = self.state["spent"]
        return {
            "plan": "acme",
            "windows": [
                {"id": "five-hour", "label": "5 hours", "duration_minutes": 300,
                 "used_percent": 100.0 if limited else 35.0, "resets_at": stamp(five), "is_active": True},
                {"id": "weekly", "label": "Weekly", "duration_minutes": 10080,
                 "used_percent": 62.5, "resets_at": stamp(weekly), "is_active": False},
            ],
            "limit_reached": {"type": "five-hour", "resets_at": stamp(five)} if limited else None,
            "ordinary_usage_allowed": not limited,
            "balances": [{"id": "account", "label": "Acme balance", "unit": "USD", "remaining": "12.50",
                          "total": "20.00", "used": "7.50", "unlimited": False,
                          "parts": [{"label": "Granted", "amount": "2.50"}, {"label": "Topped up", "amount": "10.00"}]}],
            "spend": [{"id": "month", "label": "Monthly spend", "unit": "USD", "period": "month",
                       "used": "7.50", "limit": "50.00", "remaining": "42.50", "resets_at": stamp(monthly)}],
            "available_resets": {"available_count": 0 if spent else 1, "credits": [
                {"id": "acme-reset", "title": "One example reset", "description": "Starts the five-hour window again.",
                 "reset_type": "five-hour", "status": "redeemed" if spent else "available",
                 "granted_at": stamp(self.started), "expires_at": None}]},
        }

    def models(self, params):
        mode = self.mode(params)
        if mode in ("absent", "null"):
            return {"models": None}
        if mode == "empty":
            return {"models": []}
        return {"models": [
            {"model_name": "acme-small", "display_name": "Acme Small", "parameters": {"context_window": 32000}, "listed": True},
            {"model_name": "acme-large", "display_name": "Acme Large", "parameters": {"context_window": 128000}, "listed": True},
        ]}

    def reset(self, params):
        self.mode(params)
        reset, key = params["reset_id"], params["idempotency_key"]
        if not isinstance(key, str) or not key or len(key) > MAX_ID_CHARS:
            raise Refusal("malformed", "idempotency_key needs a short nonempty identifier")
        previous = self.state["reply"]
        if previous and previous["key"] == key:
            if previous["reset_id"] != reset:
                raise Refusal("refused", "that idempotency key already names a different reset")
            return previous["report"]
        if reset != "acme-reset" or self.state["spent"]:
            raise Refusal("refused", "that example reset is not available")
        self.limit_file.unlink(missing_ok=True)
        self.state["spent"] = True
        self.started = self.now()
        result = {"outcome": "Acme spent its one example reset.", "facts": self.facts({})}
        self.state["reply"] = {"key": key, "reset_id": reset, "report": result}
        # One serial program/server owns this directory. Replacement keeps a lost reply replayable.
        temporary = self.state_file.with_suffix(".new")
        with temporary.open("w") as output:
            os.chmod(temporary, 0o600)
            output.write(encoded(self.state))
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, self.state_file)
        return result

    def present(self, params):
        self.mode(params)
        key = (params.get("credential") or {}).get("key")
        endpoint = params["endpoint"]
        if not isinstance(key, str) or not key:
            raise Refusal("credential_unavailable", "the toy exchange needs an example key", True)
        # A toy exchange only: this derived token authorizes no real service or vendor account.
        token = hmac.new(key.encode(), endpoint["base_url"].encode(), hashlib.sha256).hexdigest()
        return {"headers": {"authorization": "Bearer acme-demo-" + token}, "expires_at": stamp(self.now() + TOKEN_LIFETIME)}

    def call(self, method, params, address=False):
        if method == "describe":
            return {**DESCRIPTOR, "presents": None} if address else DESCRIPTOR
        if method == "facts.read":
            return self.facts(params)
        if method == "models.list":
            return self.models(params)
        if method == "resets.spend":
            return self.reset(params)
        if method == "credential.present" and not address:
            return self.present(params)
        if method == "ping":
            return {}
        raise Refusal("unsupported", "this example does not implement that method")


def hello(params):
    transport = min(params["transport"][1], TRANSPORT_VERSION)
    contract = min(params["contract"][1], CONTRACT_VERSION)
    if transport < params["transport"][0] or contract < params["contract"][0]:
        raise Refusal("unsupported_version", "this example speaks transport 1 and provider contract 1")
    return {"kind": "provider", "name": NAME, "version": VERSION, "transport": transport, "contract": contract,
            "methods": METHODS, "descriptor": DESCRIPTOR}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--state-dir", required=True, help="an owned directory for this one fake account")
    parser.add_argument("--at", help="fixed UTC clock for reproducible tests; normally the real clock")
    args = parser.parse_args()
    account = Acme(args.state_dir, args.at)
    for line in sys.stdin:
        try:
            request = json.loads(line)
        except ValueError:
            continue
        if not isinstance(request, dict):
            continue
        method = request.get("method")
        if "id" not in request:
            if method == "shutdown":
                return
            continue
        try:
            params = request.get("params") or {}
            result = hello(params) if method == "hello" else account.call(method, params)
            reply = {"id": request["id"], "result": result}
        except Refusal as error:
            reply = {"id": request["id"], "error": error.wire()}
        except (ValueError, KeyError, TypeError, AttributeError):
            reply = {"id": request["id"], "error": Refusal("malformed", "the example received malformed parameters").wire()}
        except OSError:
            reply = {"id": request["id"], "error": Refusal("unavailable", "the example account state is unavailable", True).wire()}
        sys.stdout.write(encoded(reply) + "\n")
        sys.stdout.flush()


if __name__ == "__main__":
    main()
```
<!-- END example.program -->

The [HTTP example](facts.md#address-example) explicitly declares
`presents: null` and refuses this method. Do not add an HTTPS example
that sends a key to a presentation service; such a binding is unsupported.
The provider conformance suite checks headers/expiry only where declared,
and marks presentation Skipped for an address rather than Passed.
