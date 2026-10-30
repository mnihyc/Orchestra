#!/usr/bin/env python3
"""An OpenRouter API-key provider example with subscription-like spending windows; stdlib only."""

import argparse
from datetime import timedelta
from decimal import Decimal
import json
import math
from pathlib import Path
import time
from urllib import error, request
from urllib.parse import urlsplit

from native_common import MAX_BYTES, MODEL_SECONDS, READ_SECONDS, descriptor, instant, now, number, serve
from plan import Refusal, encoded, stamp

ORIGIN = "https://openrouter.ai"
MODELS_BYTES = 16 * MAX_BYTES  # The connector forwarder's whole-response bound for model discovery.
CONTEXT_MAX = (1 << 63) - 1  # The model suggestion contract's positive i64 context length.
PERIODS = [("total", "usage"), ("day", "usage_daily"), ("week", "usage_weekly"), ("month", "usage_monthly")]
LABELS = {"workspace_default_member": "Workspace member default", "workspace_default_key": "Workspace key default",
          "member": "Member", "api_key": "Key", "workspace": "Workspace"}


def decimal(value):
    return format(Decimal(str(value)).normalize(), "f") if finite(value) else None


def finite(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value)


def reset_at(interval, at):
    midnight = at.replace(hour=0, minute=0, second=0, microsecond=0)
    if interval == "daily":
        return stamp(midnight + timedelta(days=1))
    if interval == "weekly":
        return stamp(midnight + timedelta(days=7 - midnight.weekday()))
    if interval == "monthly":
        return stamp(midnight.replace(year=at.year + (at.month == 12), month=at.month % 12 + 1, day=1))
    return None


def budget_id(budget):
    identifier = "budget:" + budget["scope"]
    suffix = budget.get("budget_id") if isinstance(budget.get("budget_id"), str) else budget.get("guardrail_id")
    return identifier + ":" + suffix if isinstance(suffix, str) else identifier


def balance(identifier, label, total, used, remaining):
    return {"id": identifier, "label": label, "unit": "USD", "total": decimal(total), "used": decimal(used),
            "remaining": decimal(remaining), "unlimited": None, "parts": None, "expires_at": None}


def facts_from_key(body, at, credits=None):
    data = body.get("data")
    if not isinstance(data, dict) or not all(number(data.get(field)) for _, field in PERIODS):
        raise Refusal("not_told", "OpenRouter did not tell the key's spending", True)
    expiry = instant(data.get("expires_at")) if isinstance(data.get("expires_at"), str) else None
    if data.get("expires_at") is not None and expiry is None:
        raise Refusal("malformed", "the key's expiry has another shape")
    result = {"credential": {"method": None, "yields": "key", "expires_at": expiry, "issued_at": None},
              "credential_verified": True, "spend": []}
    for period, field in PERIODS:
        result["spend"].append({"id": "key:" + period, "label": None, "unit": "USD", "period": period,
                                "period_minutes": None, "used": decimal(data[field]),
                                "limit": decimal(data.get("limit")) if period == "total" and number(data.get("limit")) else None,
                                "remaining": decimal(data.get("limit_remaining")) if period == "total" and number(data.get("limit_remaining")) else None,
                                "resets_at": None})
    if isinstance(data.get("is_free_tier"), bool):
        result["spend"][0]["x_openrouter_is_free_tier"] = data["is_free_tier"]
    for period, field in PERIODS:
        value = data.get("byok_" + field)
        if number(value):
            result["spend"].append({"id": "byok:" + period, "label": "BYOK", "unit": "USD", "period": period,
                                    "period_minutes": None, "used": decimal(value), "limit": None,
                                    "remaining": None, "resets_at": None})
    raw_budgets = data.get("budgets")
    budgets = [budget for budget in (raw_budgets if isinstance(raw_budgets, list) else []) if isinstance(budget, dict) and isinstance(budget.get("scope"), str)
               and finite(budget.get("limit_usd")) and finite(budget.get("spend_usd"))]
    effective = data.get("effective_budget")
    if not isinstance(effective, dict) or not isinstance(effective.get("scope"), str) or not finite(effective.get("limit_usd")) or not finite(effective.get("spend_usd")):
        effective = None
    windows, balances = [], []
    def add_window(identifier, label, budget, free=False):
        interval = budget.get("reset_interval")
        used, limit = budget.get("spend_usd"), budget.get("limit_usd")
        if not finite(used) or not finite(limit) or not isinstance(interval, str):
            return
        active = None
        if isinstance(effective, dict) and not free:
            active = (effective.get("scope") == "key_limit" if identifier == "key" and key_budget is None else
                      all(budget.get(key) == effective.get(key) for key in ("scope", "budget_id", "guardrail_id")))
        windows.append({"id": identifier, "label": label + ", " + interval,
                        "duration_minutes": {"daily": 1440, "weekly": 10080, "monthly": None}.get(interval),
                        "used_percent": max(0, min(100, used / limit * 100)) if limit > 0 else 100.0,
                        "resets_at": instant(budget.get("resets_at")) or reset_at(interval, at),
                        "model_family": None, "is_active": active, "reset_not_started": None,
                        "warning_percent": None, "severity": None})
    key_budget = next((budget for budget in budgets if budget.get("scope") == "key_limit" and isinstance(budget.get("reset_interval"), str)), None)
    if key_budget:
        add_window("key", "Key limit", key_budget)
    elif finite(data.get("limit")):
        if isinstance(data.get("limit_reset"), str):
            if finite(data.get("limit_remaining")):
                add_window("key", "Key limit", {"scope": "key_limit", "reset_interval": data.get("limit_reset"),
                           "limit_usd": data["limit"], "spend_usd": data["limit"] - data["limit_remaining"]})
        else:
            balances.append(balance("key", "Key limit", data["limit"], None, data.get("limit_remaining")))
    for budget in budgets:
        scope = budget.get("scope")
        if not isinstance(scope, str) or scope == "key_limit":
            continue
        label = LABELS.get(scope, scope) + " budget"
        if budget.get("reset_interval") is None:
            balances.append(balance(budget_id(budget), label, budget.get("limit_usd"), budget.get("spend_usd"), budget.get("remaining_usd")))
        else:
            add_window(budget_id(budget), label, budget)
    free = data.get("free_model_daily_requests")
    if isinstance(free, dict):
        add_window("free_requests", "Free model requests", {"reset_interval": "daily", "spend_usd": free.get("used"), "limit_usd": free.get("limit")}, free=True)
    # Present [] is a known empty list, unlike an unsupported account-credit group.
    result["windows"] = windows
    account = (credits or {}).get("data") or {}
    if number(account.get("total_credits")) and number(account.get("total_usage")):
        total, used = account["total_credits"], account["total_usage"]
        remaining = Decimal(str(total)) - Decimal(str(used))
        balances.append(balance("account", "Credits", total, used, float(remaining)))
    if balances:
        result["balances"] = balances
    return result


class NoRedirect(request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class OpenRouterKey:
    description = descriptor("openrouter-key-example", "OpenRouter key example", ["windows", "spend", "credential", "credential_verified"])
    description["summary"] = "Reads an OpenRouter key's UTC budgets, spending and optional account credit, beside its Messages and Responses endpoints."
    description["facts"]["credential"] = "key"
    description["presents"] = {}

    def __init__(self, at=None, recording=None):
        self.at, self.recording = at, recording
        self.http = request.build_opener(NoRedirect())

    def get(self, path, key, deadline):
        if self.recording:
            recorded = json.loads(Path(self.recording).read_text())
            response = recorded.get(path)
            if response is None:
                raise Refusal("unsupported", "that read is unavailable to this recorded key")
            return response
        remaining = deadline - time.monotonic()
        if remaining <= 0:
            raise Refusal("timeout", "the vendor read deadline passed", True)
        maximum = MODELS_BYTES if path == "/api/v1/models" else MAX_BYTES
        req = request.Request(ORIGIN + path, headers={"Authorization": "Bearer " + key, "Accept": "application/json"})
        try:
            with self.http.open(req, timeout=min(remaining, 1)) as response:
                chunks, size = [], 0
                while True:
                    if deadline <= time.monotonic():
                        raise Refusal("timeout", "the vendor read deadline passed", True)
                    chunk = response.read1(min(maximum + 1 - size, 65536))
                    if not chunk:
                        break
                    chunks.append(chunk); size += len(chunk)
                    if size > maximum:
                        raise Refusal("too_large", "the vendor report exceeds the provider reply bound")
                return json.loads(b"".join(chunks))
        except error.HTTPError as failure:
            if path == "/api/v1/credits" and failure.code in (401, 403):
                raise Refusal("unsupported", "account credits require a management key") from None
            raise Refusal("refused", "the vendor refused this key read") from None

    @staticmethod
    def key(params):
        key = (params.get("credential") or {}).get("key")
        if not isinstance(key, str) or not key or any(ord(char) < 32 for char in key):
            raise Refusal("refused", "this example needs the provider's API key")
        return key

    def models(self, key, deadline):
        body = self.get("/api/v1/models", key, deadline)
        found, seen = [], set()
        for row in body.get("data", []):
            identifier = row.get("id")
            if not isinstance(identifier, str) or not identifier or identifier in seen:
                continue
            seen.add(identifier)
            parameters = {}
            context = row.get("context_length")
            try:
                if isinstance(context, bool):
                    raise ValueError("a boolean is not a context")
                context = int(context.strip()) if isinstance(context, str) else int(math.floor(float(context) + 0.5))
                if 0 < context <= CONTEXT_MAX:
                    parameters["offered"] = {"context_max": context}
            except (TypeError, ValueError, OverflowError):
                pass
            prices = row.get("pricing") or {}
            def price(name):
                try:
                    value = Decimal(str(prices[name]))
                    return format((value * 1000000).normalize(), "f") if value.is_finite() and value >= 0 else None
                except (KeyError, ValueError, ArithmeticError):
                    return None
            incoming, outgoing = price("prompt"), price("completion")
            if incoming is not None and outgoing is not None:
                parameters["rate"] = {"input_per_mtok": incoming, "output_per_mtok": outgoing,
                                      "cached_input_per_mtok": price("input_cache_read") or incoming}
                if price("input_cache_write") is not None:
                    parameters["rate"]["cache_write_per_mtok"] = price("input_cache_write")
            display = next((row[field].strip() for field in ("name", "display_name") if isinstance(row.get(field), str) and row[field].strip()), identifier)
            found.append({"model_name": identifier, "display_name": display, "parameters": parameters, "listed": True})
        return {"models": found}

    def call(self, method, params):
        key = self.key(params)
        if method != "credential.present":
            allowed = False
            for endpoint in params.get("endpoints", []):
                parsed = urlsplit(endpoint.get("base_url", ""))
                if parsed.scheme == "https" and parsed.hostname == "openrouter.ai" and parsed.port in (None, 443) and not parsed.username and not parsed.password and not parsed.query and not parsed.fragment and parsed.path.rstrip("/") in ("", "/api", "/api/v1"):
                    allowed = True
            if not allowed:
                raise Refusal("unsupported", "the provider has no documented OpenRouter endpoint")
        deadline = time.monotonic() + (MODEL_SECONDS if method in ("models.list", "x-endpoint.describe") else READ_SECONDS)
        if method == "facts.read":
            read = self.get("/api/v1/key", key, deadline)
            try:
                credits = self.get("/api/v1/credits", key, deadline)
            except (Refusal, OSError):
                credits = None  # Ordinary keys cannot read account credit; keep the successful key facts.
            return facts_from_key(read, now(self.at), credits)
        if method == "models.list":
            return self.models(key, deadline)
        if method == "credential.present":
            endpoint = params["endpoint"]
            parsed = urlsplit(endpoint["base_url"])
            if parsed.scheme != "https" or parsed.hostname != "openrouter.ai" or parsed.port not in (None, 443) or parsed.username or parsed.password or parsed.path.rstrip("/") not in ("/api", "/api/v1") or parsed.query or parsed.fragment:
                raise Refusal("refused", "the key is presented only to OpenRouter's documented endpoints")
            return {"headers": {"authorization": "Bearer " + key}, "expires_at": None}
        if method == "x-endpoint.describe":
            return {"endpoints": [{"protocol": "openai-responses", "base_url": ORIGIN + "/api/v1", "path": "/responses",
                                    "body": {"store": False, "stream": True},
                                    "options": {"auth": "bearer", "headers": {"content-type": "application/json"}}},
                                   {"protocol": "anthropic-messages", "base_url": ORIGIN + "/api", "path": "/v1/messages",
                                    "options": {"auth": "bearer", "headers": {"content-type": "application/json", "anthropic-version": "2023-06-01"}}}],
                    "presents": {"header": "authorization", "scheme": "Bearer"}, **self.models(key, deadline)}
        raise Refusal("unsupported", "this example does not implement that method")

    def close(self):
        pass


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--at", help="fixed UTC time for recorded-answer tests")
    parser.add_argument("--recording", help="offline disposable response fixture; never used with a real key")
    args = parser.parse_args()
    serve(OpenRouterKey(args.at, args.recording))
