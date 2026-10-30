#!/usr/bin/env python3
"""Codex subscription facts and models through its unmodified app server; no model proxy or token read."""

from datetime import timedelta
import time

from native_common import CATALOGUE_BYTES, MODEL_SECONDS, READ_SECONDS, Native, Refusal, descriptor, instant, main, now, number, probe, window

MEMBERS = ["plan", "windows", "balances", "limit_reached", "ordinary_usage_allowed", "available_resets"]


def snapshot_facts(snapshot, at):
    result, windows = {}, []
    if isinstance(snapshot.get("planType"), str):
        result["plan"] = snapshot["planType"]
    for identifier in ("primary", "secondary"):
        native = snapshot.get(identifier)
        if not isinstance(native, dict) or not number(native.get("usedPercent")):
            continue
        minutes, used = native.get("windowDurationMins"), native["usedPercent"]
        reset = instant(native.get("resetsAt"))
        row = window(identifier, minutes, used, native.get("resetsAt"))
        row["label"] = ("window" if minutes is None else "weekly" if minutes == 10080 else
                        str(minutes // 1440) + "d" if minutes >= 1440 and minutes % 1440 == 0 else
                        str(minutes // 60) + "h" if minutes >= 60 and minutes % 60 == 0 else str(minutes) + "m")
        unstarted = bool(used == 0 and isinstance(minutes, int) and minutes > 0 and reset and
                         abs(int((now(reset) - (at + timedelta(minutes=minutes))).total_seconds())) <= 60)
        row["reset_not_started"] = unstarted if reset else None
        row["resets_at"] = None if unstarted else reset
        windows.append(row)
    if windows:
        result["windows"] = windows
    credits = snapshot.get("credits")
    if isinstance(credits, dict) and isinstance(credits.get("unlimited"), bool):
        result["balances"] = [{"id": "plan", "label": "Plan credits", "unit": "credits",
                               "remaining": credits.get("balance") if isinstance(credits.get("balance"), str) else None,
                               "total": None, "used": None, "unlimited": credits["unlimited"], "parts": None, "expires_at": None}]
    if "rateLimitReachedType" in snapshot:
        kind = snapshot["rateLimitReachedType"]
        resets = [row["resets_at"] for row in windows if row["resets_at"]]
        result["limit_reached"] = {"type": kind, "resets_at": min(resets) if resets else None} if isinstance(kind, str) else None
    return result


def reset_credits(value):
    if not isinstance(value, dict) or not isinstance(value.get("availableCount"), int) or value["availableCount"] < 0:
        return None
    credits = value.get("credits")
    if credits is not None:
        credits = [{"id": row["id"], "title": row.get("title"), "description": row.get("description"),
                    "reset_type": row["resetType"], "status": row["status"],
                    "granted_at": instant(row.get("grantedAt")), "expires_at": instant(row.get("expiresAt"))} for row in credits]
    return {"available_count": value["availableCount"], "credits": credits}


def model_suggestion(model):
    name = model.get("model") or model.get("id")
    if not isinstance(name, str):
        return None
    params, offered = {}, {}
    efforts = [row["reasoningEffort"] for row in model.get("supportedReasoningEfforts", []) if isinstance(row.get("reasoningEffort"), str)]
    if efforts:
        params["efforts"] = efforts
    if isinstance(model.get("defaultReasoningEffort"), str):
        params["default_effort"] = model["defaultReasoningEffort"]
    context = model.get("contextWindow")
    if isinstance(context, int) and context > 0:
        params["context_window"] = context
    if isinstance(model.get("inputModalities"), list):
        params["input_images"] = "image" in model["inputModalities"]
    tiers = [{"id": tier["id"], "label": tier.get("name", tier["id"])} for tier in model.get("serviceTiers", []) if isinstance(tier.get("id"), str)]
    if tiers:
        offered["service_tiers"] = tiers
    programs = [program for program in (model.get("availableAccessPrograms") or {}).get("cyber", []) if isinstance(program, str) and program != "standard"]
    if programs:
        offered["access_programs"] = programs
    maximum = model.get("maxContextWindow")
    if isinstance(maximum, int) and (not isinstance(context, int) or maximum > context):
        offered["context_max"] = maximum
    if offered:
        params["offered"] = offered
    return {"model_name": name, "display_name": model.get("displayName") or name, "parameters": params, "listed": True}


class CodexPlan:
    description = descriptor("codex-plan-example", "Codex subscription example", MEMBERS, resets=True)

    def __init__(self, command, at=None):
        self.command, self.at, self.native = command, at, None

    def connect(self, deadline):
        if self.native is not None and self.native.process.poll() is not None:
            self.close()
        if self.native is None:
            self.native = Native(self.command + ["app-server", "--listen", "stdio://"])
            try:
                self.native.call("initialize", {"clientInfo": {"name": "orch", "title": "Orch", "version": "0.1.0"},
                                 "capabilities": {"experimentalApi": True, "requestAttestation": False}}, deadline)
                self.native.write({"method": "initialized", "params": {}})
            except (Refusal, OSError):
                self.close()
                raise
        return self.native

    def facts(self, deadline):
        native = self.connect(deadline)
        auth = native.call("getAuthStatus", {"includeToken": False, "refreshToken": False}, deadline)
        if auth.get("authMethod") != "chatgpt":
            raise Refusal("refused", "this example requires Codex's existing ChatGPT sign-in")
        account = native.call("account/read", {"refreshToken": False}, deadline)
        result = {}
        plan = (account.get("account") or {}).get("planType")
        if isinstance(plan, str):
            result["plan"] = plan
        try:
            read = native.call("account/rateLimits/read", {}, deadline)
        except Refusal as error:
            if error.code != "not_told":
                raise
            read = {}
        at = now(self.at)
        snapshot = read.get("rateLimits") or {}
        result.update(snapshot_facts(snapshot, at))
        for limit, additional in (read.get("rateLimitsByLimitId") or {}).items():
            if additional == snapshot or limit == snapshot.get("limitId"):
                continue
            for row in snapshot_facts(additional, at).get("windows", []):
                row["id"] = limit + ":" + row["id"]
                result.setdefault("windows", []).append(row)
        for frame in native.observations:
            result.update(snapshot_facts(frame.get("params", {}).get("rateLimits", {}), at))
        native.observations.clear()
        resets = reset_credits(read.get("rateLimitResetCredits"))
        if resets is not None:
            result["available_resets"] = resets
        if isinstance(read.get("ordinaryUsageAllowed"), bool):
            result["ordinary_usage_allowed"] = read["ordinaryUsageAllowed"]
        return result

    def models(self, deadline):
        native, models, cursor, seen = self.connect(deadline), [], None, set()
        while True:
            page = native.call("model/list", {"includeHidden": False, "cursor": cursor, "limit": 100}, deadline)
            for model in page.get("data", []):
                row = model_suggestion(model)
                if row and model.get("hidden") is not True:
                    models.append(row)
            cursor = page.get("nextCursor")
            if not cursor:
                break
            if cursor in seen:
                raise Refusal("malformed", "the native model list repeated its cursor")
            seen.add(cursor)
        # As the built-in does, ask the harness's catalogue for contexts missing from model/list.
        try:
            catalogue = probe(self.command + ["debug", "models"], deadline, CATALOGUE_BYTES)
            known = {row["slug"]: row for row in catalogue.get("models", []) if isinstance(row.get("slug"), str)}
            for model in models:
                row = known.get(model["model_name"], {})
                context = row.get("context_window")
                if isinstance(context, int) and context > 0:
                    model["parameters"].setdefault("context_window", context)
                    maximum = row.get("max_context_window")
                    if isinstance(maximum, int) and maximum > model["parameters"]["context_window"]:
                        model["parameters"].setdefault("offered", {}).setdefault("context_max", maximum)
        except (Refusal, ValueError, TypeError, KeyError, OSError):
            pass  # A refused catalogue does not invent a model's window.
        return {"models": models}

    def call(self, method, params):
        deadline = time.monotonic() + (MODEL_SECONDS if method in ("models.list", "x-endpoint.describe") else READ_SECONDS)
        if method == "facts.read":
            return self.facts(deadline)
        if method == "models.list":
            return self.models(deadline)
        if method == "resets.spend":
            reset = params
            native = self.connect(deadline)
            answer = native.call("account/rateLimitResetCredit/consume",
                                 {"creditId": reset["reset_id"], "idempotencyKey": reset["idempotency_key"]}, deadline)
            # The native service owns idempotency across program restarts. Never generate another key.
            outcome = answer.get("outcome")
            if not isinstance(outcome, str):
                raise Refusal("not_told", "the native reset outcome was not told", True)
            try:
                facts = self.facts(deadline)
            except Refusal:
                facts = None  # A refused follow-up read must not hide a confirmed reset outcome.
            return {"outcome": outcome, "facts": facts}
        if method == "x-endpoint.describe":
            from native_endpoints import endpoint
            return endpoint("codex", params, self.models(deadline))
        raise Refusal("unsupported", "this example does not implement that method")

    def close(self):
        if self.native:
            self.native.close()
            self.native = None


if __name__ == "__main__":
    main("codex", CodexPlan)
