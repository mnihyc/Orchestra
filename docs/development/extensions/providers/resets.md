---
id: provider-resets
title: Implement provider reset consumption
section: build-your-own
link_prefix: provider-
screens: /resources/providers
keywords: resets.spend, spend_reset, ResetSpendParams, ResetReport, available_resets, idempotency_key, Use a reset
---

# Implement provider reset consumption

Reset consumption belongs to the [facts source](facts.md) that listed
the reset. There is no separate reset-source selector. Declare
`facts.spends_resets: true` and include `available_resets` in `reports`.
`facts.read` lists the available count and the reset credits it knows,
with each source-defined id, title, description, reset_type, status,
grant time and expiry. Null/empty/unknown remain distinct as on the
facts page. An unsupported source answers `unsupported` to `resets.spend`.

<!-- BEGIN provider.resets -->

**reset-spend-params**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `provider` | `SourceAskProvider` | required | The provider. |
| `endpoints` | `SourceAskEndpoint[]` | may be absent | Its endpoints; none for a provider signed in through a harness. |
| `config` | `object` | may be absent | What the provider's managers gave for the source's fields that are no secrets, by field. |
| `secrets` | `map of string` | may be absent | The values of the source's secret fields, by field: those it declared, and no other. |
| `credential` | `SourceAskCredential or null` | may be absent | The provider's key: only where the source wants it, the credential is a key, and, for an address, the provider's managers turned on sending it there. Never a sign-in. |
| `reset_id` | `string` | required | The reset to spend, by the id the source listed it with in `available_resets`. |
| `idempotency_key` | `string` | required | The same key given twice spends once. |

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

**reset-report**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `outcome` | `string` | required | The outcome, in the source's own words. |
| `facts` | `FactsReport or null` | may be absent | The facts it read after spending, where it read them. |

**FactsReport**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `identity` | `string or null` | may be absent | Who the credential belongs to, at most 200 characters. |
| `credential` | `CredentialFacts or null` | may be absent | How the credential was given, what it is, and when it lapses. |
| `credential_verified` | `boolean or null` | may be absent | True only where this read authenticated with the provider's credential; it is kept only from a read that was given the key. |
| `plan` | `string or null` | may be absent | The plan's id, at most 200 characters; the name shown for it is the one the source's `plans` give. |
| `windows` | `array or null` | may be absent | The usage windows of the plan, at most 64, each by its `id`: how much is used in percent, and when it starts over. |
| `limit_reached` | `LimitReached or null` | may be absent | A limit of the plan that has been reached, with when it lifts: new turns wait until then, or until a read admits ordinary use. |
| `ordinary_usage_allowed` | `boolean or null` | may be absent | Whether the vendor admits ordinary use now: false holds new turns, true lifts that. |
| `balances` | `array or null` | may be absent | What is left to be spent, at most 32, each by its `id` and in its own unit; amounts are decimal text. |
| `spend` | `array or null` | may be absent | What has been spent over periods, at most 64, each by its `id`, with the period's limit where it has one; amounts are decimal text. |
| `extra_usage` | `ExtraUsage or null` | may be absent | Spending beyond the plan, in the vendor's unit. |
| `available_resets` | `AvailableResets or null` | may be absent | The resets that can be spent, with their credits, at most 64; a source that spends them declares `spends_resets`. |

**CredentialFacts**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `method` | `string or null` | may be absent | A sign-in method's id; `null` for a key. |
| `yields` | `CredentialYield` | required | What the credential is. |
| `expires_at` | `string or null` | may be absent | When a token lapses, where that is known. |
| `issued_at` | `string or null` | may be absent | When the credential was issued, where its own claims say so. |

**CredentialYield**

| Value | Meaning |
|-------|---------|
| `"login"` | A sign-in the harness renews. |
| `"token"` | A token used as it is. |
| `"key"` | An API key. |
| `"none"` | None. |

**UsageWindow**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `id` | `string` | required | The vendor's id for the window. |
| `label` | `string` | required | What it is called: `5h`, `weekly`. |
| `duration_minutes` | `integer or null` | may be absent | How long it is, in minutes; `null` where it has no fixed length. |
| `used_percent` | `number (double)` | required | How much of it is used, from 0 to 100. A vendor may report a fraction. |
| `resets_at` | `string or null` | may be absent | When it starts over. |
| `model_family` | `string or null` | may be absent | The family of models this window covers; null means the whole plan. |
| `is_active` | `boolean or null` | may be absent | Whether the vendor marks this as the binding window. |
| `reset_not_started` | `boolean or null` | may be absent | The source reports an unstarted or not yet fixed reset, distinct from an unknown reset. |
| `warning_percent` | `number or null` | may be absent | The vendor's warning mark, in percent, where it names one. |
| `severity` | `string or null` | may be absent | The vendor's severity for display, without a notification. |

**LimitReached**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `type` | `string` | required | Which limit, as the vendor names it. |
| `resets_at` | `string or null` | may be absent | When it is lifted. |

**Balance**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `id` | `string` | required | The source's name for it, one of a kind among the balances it tells: `plan`, `account`, a currency. |
| `label` | `string or null` | may be absent | What it is called for people, where the source names it. |
| `unit` | `string` | required | The vendor's unit: a currency such as `USD` or `CNY`, or `credits`. |
| `remaining` | `string or null` | may be absent | What is left. |
| `total` | `string or null` | may be absent | What there was in all. |
| `used` | `string or null` | may be absent | What has been spent of it. |
| `unlimited` | `boolean or null` | may be absent | Whether it has no limit. |
| `parts` | `array or null` | may be absent | What it is made of, where the vendor splits it. |
| `expires_at` | `string or null` | may be absent | When it lapses, where it does. |

**BalancePart**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `label` | `string` | required | What the vendor calls the part. |
| `amount` | `string` | required | How much it is, as decimal text in the balance's unit. |

**Spend**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `id` | `string` | required | The source's name for the row, one of a kind among the rows it tells. |
| `label` | `string or null` | may be absent | What it is called for people, where the source names it. |
| `unit` | `string` | required | The vendor's unit. |
| `period` | `SpendPeriod` | required | The period it covers. |
| `period_minutes` | `integer or null` | may be absent | For a period of its own length, how long it is, in minutes. |
| `used` | `string or null` | may be absent | What has been spent in the period. |
| `limit` | `string or null` | may be absent | The most that may be spent in it. |
| `remaining` | `string or null` | may be absent | What is left of the limit. |
| `resets_at` | `string or null` | may be absent | When the period starts over. |

**SpendPeriod**

| Value | Meaning |
|-------|---------|
| `"day"` | A day, by the vendor's clock. |
| `"week"` | A week, by the vendor's clock. |
| `"month"` | A month, by the vendor's clock. |
| `"total"` | Everything so far. |
| `"window"` | A window of its own length, given as `period_minutes`. |

**ExtraUsage**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `enabled` | `boolean` | required | Whether extra spending is enabled. |
| `limit` | `number or null` | may be absent | The spending limit, where known. |
| `used` | `number or null` | may be absent | What has been used, where known. |
| `used_percent` | `number or null` | may be absent | How much is used, in percent. |
| `unit` | `string or null` | may be absent | The vendor's currency or unit, where supplied. |
| `decimal_places` | `integer or null` | may be absent | The scale the vendor uses for its amounts. |
| `disabled_reason` | `string or null` | may be absent | Why it is disabled, when the vendor says. |
| `limit_reached` | `boolean or null` | may be absent | Whether its spending limit is reached. |

**AvailableResets**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `available_count` | `integer (int64)` | required | The count, which may be greater than the detail list's length. |
| `credits` | `array or null` | may be absent | Null means only the count is known; an empty list means no details were returned. |

**ResetCredit**

| Member | JSON shape | Presence | Meaning |
|--------|------------|----------|---------|
| `id` | `string` | required | The vendor's opaque identifier. |
| `title` | `string or null` | may be absent | The vendor's title, where supplied. |
| `description` | `string or null` | may be absent | What the vendor says the reset begins anew. |
| `reset_type` | `string` | required | The type of reset, in the vendor's vocabulary. |
| `status` | `string` | required | Whether the vendor says it is available, redeeming, redeemed or unknown. |
| `granted_at` | `string or null` | may be absent | When it was granted. |
| `expires_at` | `string or null` | may be absent | When it lapses, or null when it has no known end. |
<!-- END provider.resets -->

The parameters are [ResetSpendParams](schema/reset-spend-params.json):
ordinary SourceAsk members plus `reset_id` and `idempotency_key` at the
same level. The result is [ResetReport](schema/reset-report.json), with
an outcome in your source's own words and optional sparse facts read
after spending. Do not nest the ask or silently choose another reset.

```json
{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{},"secrets":{},"reset_id":"acme-reset","idempotency_key":"0199c1a0-0000-7000-8000-000000000002"}
```

## Spend once, including after a lost reply

Persist the idempotency identity and its result with the effect. Repeating
the same key returns that result and spends once, including after a
process restart. Do not use an in-memory set that forgets on restart or
retry with a new key after a timeout. A key replay cannot select a
different reset. The outcome says what actually happened: consumed,
already consumed or unavailable; do not invent a successful reset.

Orch's human browser manager first reviews the exact listed reset id,
available count, resulting count and effects the source reported. Cancel
is the default; consumption needs the explicit Use this reset act.
The API checks the shown id/count before calling your source, but cannot
promise an atomic predicate in a vendor API that has none. Recheck the
vendor's state as its protocol requires. An unknown result remains
unknown; Orch keeps the same request identity to check it. No automatic
budget rule or agent request consumes a credit.

When the reset or subsequent facts admit ordinary use, waiting work can
continue under its existing caps. A reset does not alter Orch's own caps
or ask a model to run. Orch records the authorized who/id/time, without
your token or credential.

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

The complete Acme file persists one disposable reset and its replay
result in its account directory. Its real conformance fixture explicitly
permits that test-only credit to be consumed. Use a fresh disposable
directory; never point the test fixture at a real person's reset.

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

## Address example

[The Python address service](facts.md#address-example) uses the same
persistent reset implementation. POST this envelope with the address's
write-only bearer/named-header authorization:

```json
{"contract":1,"method":"resets.spend","params":{"provider":{"id":"0199c1a0-0000-7000-8000-000000000001","name":"Acme example"},"endpoints":[],"config":{},"secrets":{},"reset_id":"acme-reset","idempotency_key":"0199c1a0-0000-7000-8000-000000000002"}}
```

HTTP 200 wraps `result: {outcome, facts?}` or the shared error. Retries
reuse that idempotency key. HTTPS/allowed Unencrypted HTTP, no redirects,
no cookies, source authorization and explicit key consent follow the
[shared address rules](README.md#serve-an-address). A native sign-in
never leaves its keeper. See [request](schema/address-request.json) and
[reply](schema/address-reply.json).
