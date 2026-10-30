#!/usr/bin/env python3
"""Claude Code subscription observations through its unmodified stream-json control channel."""

import time

from native_common import MODEL_SECONDS, READ_SECONDS, Native, Refusal, descriptor, main, number, window

MEMBERS = ["plan", "windows", "extra_usage", "limit_reached"]
WINDOWS = [("five_hour", "primary", 300, None), ("seven_day", "secondary", 10080, None),
           ("seven_day_opus", "secondary:opus", 10080, "Opus"), ("seven_day_sonnet", "secondary:sonnet", 10080, "Sonnet")]
WARNING_PERCENT = 75.0  # Same display warning as the native adapter, never a blocking decision.
ARGS = ["-p", "--input-format", "stream-json", "--output-format", "stream-json", "--verbose",
        "--strict-mcp-config", "--setting-sources", "", "--no-session-persistence"]


def usage_facts(value):
    windows = []
    for limit in value.get("limits", []):
        kind = limit.get("kind")
        if kind == "session":
            identifier, minutes, family = "primary", 300, None
        elif kind == "weekly_all":
            identifier, minutes, family = "secondary", 10080, None
        elif kind == "weekly_scoped":
            model = (limit.get("scope") or {}).get("model") or {}
            family = model.get("display_name") or model.get("id")
            if not isinstance(family, str):
                continue
            identifier, minutes = "secondary:" + family.lower(), 10080
        else:
            continue
        if not number(limit.get("percent")):
            raise Refusal("not_told", "the native usage window was not told", True)
        windows.append(window(identifier, minutes, limit["percent"], limit.get("resets_at"), family,
                              limit.get("is_active"), limit.get("severity"), WARNING_PERCENT))
    for key, identifier, minutes, family in WINDOWS:
        row = value.get(key)
        if row is None or any(known["id"] == identifier for known in windows):
            continue
        if not number(row.get("utilization")):
            raise Refusal("not_told", "the native usage window was not told", True)
        windows.append(window(identifier, minutes, row["utilization"], row.get("resets_at"), family, warning=WARNING_PERCENT))
    for row in value.get("model_scoped", []):
        family = row.get("display_name")
        if not isinstance(family, str) or not family.strip() or not number(row.get("utilization")):
            continue
        identifier = "secondary:" + family.lower()
        if not any(known["id"] == identifier for known in windows):
            windows.append(window(identifier, 10080, row["utilization"], row.get("resets_at"), family, warning=WARNING_PERCENT))
    result = {"windows": windows} if windows else {}
    extra = value.get("extra_usage")
    if isinstance(extra, dict) and isinstance(extra.get("is_enabled"), bool):
        result["extra_usage"] = {"enabled": extra["is_enabled"], "limit": extra.get("monthly_limit") if number(extra.get("monthly_limit")) else None,
                                 "used": extra.get("used_credits") if number(extra.get("used_credits")) else None,
                                 "used_percent": extra.get("utilization") if number(extra.get("utilization")) else None,
                                 "unit": extra.get("currency"), "decimal_places": extra.get("decimal_places"),
                                 "disabled_reason": extra.get("disabled_reason"), "limit_reached": extra.get("spend_limit_reached")}
    return result


def usage_answer(body):
    # get_usage's session costs are not account money. Read only its explicit plan envelope.
    result = usage_facts(body.get("rate_limits") or {}) if body.get("rate_limits_available") is True else {}
    plan = body.get("subscription_type")
    if isinstance(plan, str) and plan.strip():
        result["plan"] = plan.lower()
    return result


def rate_limit_facts(frame):
    if frame.get("type") != "rate_limit_event":
        return {}
    info, windows = frame.get("rate_limit_info") or {}, []
    unified = info.get("unifiedWindows")
    if not isinstance(unified, dict):
        return {}
    binding = info.get("rateLimitType")
    for key, identifier, minutes, family in WINDOWS:
        row = unified.get(key)
        if not isinstance(row, dict):
            continue
        if not number(row.get("utilization")):
            return {}
        active = key == binding if isinstance(binding, str) else None
        warning = info.get("surpassedThreshold")
        windows.append(window(identifier, minutes, row["utilization"] * 100, row.get("resetsAt"), family,
                              active, info.get("status") if active else None,
                              warning * 100 if number(warning) else WARNING_PERCENT))
    if not windows:
        return {}
    from native_common import instant
    result = {"windows": windows}
    # A warning is not a refusal, nor proof of recovery after a prior vendor refusal.
    if info.get("status") == "rejected":
        result["limit_reached"] = {"type": binding or "usage_limit", "resets_at": instant(info.get("resetsAt"))}
    if isinstance(info.get("overageEnabled"), bool):
        result["extra_usage"] = {"enabled": info["overageEnabled"], "limit": None, "used": None,
                                 "used_percent": None, "unit": None, "decimal_places": None,
                                 "disabled_reason": info.get("overageDisabledReason"), "limit_reached": None}
    return result


def model_suggestions(body):
    rows, seen, found = body.get("models", []), set(), []
    for row in [row for row in rows if row.get("value") != "default"] + [row for row in rows if row.get("value") == "default"]:
        value = row.get("value")
        name = row.get("resolvedModel", value)
        if not isinstance(value, str) or not isinstance(name, str) or row.get("disabled") is True or name in seen:
            continue
        seen.add(name)
        shown = (row.get("description") or "").split(" · ", 1)
        leading = shown[0].strip() if len(shown) == 2 else None
        display = leading if leading and len(leading) <= 40 and not leading.startswith("Use ") else row.get("displayName")
        params = {"input_images": True}
        efforts = [effort for effort in row.get("supportedEffortLevels", []) if isinstance(effort, str)]
        if efforts:
            params["efforts"] = efforts
        if row.get("supportsFastMode") is True:
            params["offered"] = {"service_tiers": [{"id": "fast", "label": "Fast"}]}
        found.append({"model_name": name, "display_name": name if value == "default" else display or name,
                      "parameters": params, "listed": True})
    return found


class ClaudePlan:
    description = descriptor("claude-plan-example", "Claude Code subscription example", MEMBERS)

    def __init__(self, command, at=None):
        self.command, self.at, self.native, self.initialized = command, at, None, None

    def connect(self, deadline):
        if self.native is None:
            self.native = Native(self.command + ARGS, stream=True)
            self.initialized = self.native.call("initialize", {}, deadline)
        return self.native

    def call(self, method, params):
        deadline = time.monotonic() + (MODEL_SECONDS if method in ("models.list", "x-endpoint.describe") else READ_SECONDS)
        try:
            if method == "facts.read":
                native = self.connect(deadline)
                try:
                    result = usage_answer(native.call("get_usage", {"skip_behaviors": True}, deadline))
                except Refusal as error:
                    if error.code != "not_told":
                        raise
                    result = {}
                for frame in native.observations:
                    result.update(rate_limit_facts(frame))
                return result
            if method in ("models.list", "x-endpoint.describe"):
                self.connect(deadline)
                models = {"models": model_suggestions(self.initialized or {})}
                if method == "models.list":
                    return models
                from native_endpoints import endpoint
                return endpoint("claude", params, models)
            raise Refusal("unsupported", "this example does not implement that method")
        finally:
            # As the built-in does, every Refresh observes a new native control process/sign-in view.
            self.close()

    def close(self):
        if self.native:
            self.native.close()
            self.native, self.initialized = None, None


if __name__ == "__main__":
    main("claude", ClaudePlan)
