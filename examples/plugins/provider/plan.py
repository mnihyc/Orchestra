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
