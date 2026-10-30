"""Shared bounded stdlib transport for the two native-sign-in provider examples.

The child is the unmodified harness, never a shell, HTTP proxy or credential reader.
Only protocol JSON goes to stdout; vendor diagnostics and credentials are not echoed.
"""

import argparse
import json
import math
import os
import queue
import subprocess
import sys
import threading
import time
from datetime import datetime, timezone

from plan import Refusal, encoded, stamp

# The source contract allows one MiB per reply. Leave a second for the host's 8/30-second deadlines.
MAX_BYTES = 1024 * 1024
READ_SECONDS = 7
MODEL_SECONDS = 29
CATALOGUE_BYTES = 8 * MAX_BYTES  # The Codex adapter's bounded `debug models` catalogue.
MAX_NOTIFICATIONS = 32  # Bounded partial observations, never conversation events or model output.
CLOSE_SECONDS = 0.25  # Two cleanup waits still fit in the one-second margin before the host deadline.


def now(text=None):
    return datetime.fromisoformat(text.replace("Z", "+00:00")) if text else datetime.now(timezone.utc)


def instant(value):
    try:
        if isinstance(value, str):
            return stamp(datetime.fromisoformat(value.replace("Z", "+00:00")))
        if isinstance(value, int) and not isinstance(value, bool):
            return stamp(datetime.fromtimestamp(value, timezone.utc))
    except (ValueError, OverflowError, OSError):
        pass
    return None


def number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool) and math.isfinite(value) and value >= 0


def window(identifier, minutes, used, reset=None, family=None, active=None, severity=None, warning=None):
    return {"id": identifier, "label": "5h" if minutes == 300 else "weekly", "duration_minutes": minutes,
            "used_percent": float(used), "resets_at": instant(reset), "model_family": family,
            "is_active": active, "reset_not_started": None, "warning_percent": warning, "severity": severity}


class Native:
    def __init__(self, command, stream=False):
        self.stream = stream
        self.next_id = 0
        self.frames = queue.Queue(MAX_NOTIFICATIONS)
        self.observations = []
        # Inherit the same native sign-in environment; never turn a plan into API-key mode.
        env = os.environ.copy()
        for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
            env.pop(key, None)
        if stream:
            env["CLAUDE_CODE_DISABLE_BACKGROUND_TASKS"] = "1"  # Same read-only probe as the built-in.
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                        stderr=subprocess.DEVNULL, env=env)
        threading.Thread(target=self._read, daemon=True).start()

    def _read(self):
        try:
            while True:
                line = self.process.stdout.readline(MAX_BYTES + 1)
                if not line or len(line) > MAX_BYTES:
                    break
                frame = json.loads(line)
                self.frames.put_nowait(frame)
        except (ValueError, OSError, queue.Full):
            pass
        finally:
            # A full queue is itself a refusal; its next consumer drains before seeing the close.
            try:
                self.frames.put_nowait(None)
            except queue.Full:
                self.process.kill()

    def write(self, frame):
        self.process.stdin.write((encoded(frame) + "\n").encode())
        self.process.stdin.flush()

    def call(self, method, params, deadline):
        if deadline <= time.monotonic():
            raise Refusal("timeout", "the native read deadline passed", True)
        self.next_id += 1
        request_id = str(self.next_id)
        if self.stream:
            self.write({"type": "control_request", "request_id": request_id,
                        "request": {"subtype": method, **params}})
        else:
            self.write({"id": request_id, "method": method, "params": params})
        while True:
            left = deadline - time.monotonic()
            if left <= 0:
                raise Refusal("timeout", "the native harness did not answer in time", True)
            try:
                frame = self.frames.get(timeout=left)
            except queue.Empty:
                raise Refusal("timeout", "the native harness did not answer in time", True) from None
            if frame is None:
                raise Refusal("unavailable", "the native harness ended before its answer", True)
            if self.stream and frame.get("type") == "control_response":
                response = frame.get("response") or {}
                if response.get("request_id") == request_id:
                    if response.get("subtype") != "success":
                        raise Refusal("not_told", "the native harness did not tell this fact", True)
                    return response.get("response")
            elif not self.stream and str(frame.get("id")) == request_id:
                if "error" in frame:
                    raise Refusal("not_told", "the native harness did not tell this fact", True)
                return frame.get("result")
            if frame.get("method") == "account/rateLimits/updated" or frame.get("type") == "rate_limit_event":
                if len(self.observations) >= MAX_NOTIFICATIONS:
                    raise Refusal("unavailable", "too many unread native observations", True)
                self.observations.append(frame)
            elif "id" in frame and "method" in frame:
                # A facts/model read grants no permission to run a tool or a turn.
                self.write({"id": frame["id"], "error": {"code": -32601, "message": "unsupported"}})

    def close(self):
        if self.process.poll() is None:
            self.process.terminate()
        try:
            self.process.wait(timeout=CLOSE_SECONDS)
        except subprocess.TimeoutExpired:
            self.process.kill()
            self.process.wait(timeout=CLOSE_SECONDS)
        for stream in (self.process.stdin, self.process.stdout):
            stream.close()


def descriptor(name, title, members, resets=False):
    return {"name": name, "title": title, "summary": "Reads the same native sign-in and observations as the built-in harness.",
            "contract": 1, "facts": {"reports": members, "spends_resets": resets, "credential": "none",
                                     "min_interval_secs": 60, "plans": {}},
            "models": {}, "presents": None, "fields": [], "docs_url": None}


def probe(command, deadline, maximum=MAX_BYTES):
    """Read one bounded native CLI JSON answer, with no prompt and no input credential."""
    env = os.environ.copy()
    for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY"):
        env.pop(key, None)
    process = subprocess.Popen(command, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.DEVNULL, env=env)
    done = queue.Queue(1)
    def read():
        try:
            done.put(process.stdout.read(maximum + 1))
        except (ValueError, OSError):
            done.put(b"")
    threading.Thread(target=read, daemon=True).start()
    try:
        raw = done.get(timeout=max(0, deadline - time.monotonic()))
        if len(raw) > maximum:
            raise Refusal("too_large", "the native catalogue exceeded its read bound")
        process.wait(timeout=max(0, deadline - time.monotonic()))
        if process.returncode:
            raise Refusal("not_told", "the native command did not tell its facts", True)
        return json.loads(raw)
    except (queue.Empty, subprocess.TimeoutExpired):
        raise Refusal("timeout", "the native command did not answer in time", True) from None
    finally:
        if process.poll() is None:
            process.kill()
        process.wait(timeout=CLOSE_SECONDS)
        process.stdout.close()


def main(kind, make):
    parser = argparse.ArgumentParser(description="An example provider beside an existing native sign-in; no proxy or new sign-in.")
    parser.add_argument("--harness-command", help="JSON argv of the native harness; normally its installed command")
    parser.add_argument("--at", help="fixed UTC observation time for recorded-answer tests")
    args = parser.parse_args()
    command = json.loads(args.harness_command) if args.harness_command else [kind]
    if not isinstance(command, list) or not command or not all(isinstance(arg, str) and arg for arg in command):
        parser.error("--harness-command is a nonempty JSON argv")
    source = make(command, args.at)
    serve(source)


def serve(source):
    """Serve a declared source over the ordinary line-JSON plugin transport."""
    methods = ["describe", "facts.read", "models.list", "x-endpoint.describe"]
    if source.description["facts"]["spends_resets"]:
        methods.append("resets.spend")
    if source.description.get("presents") is not None:
        methods.append("credential.present")
    try:
        while True:
            line = sys.stdin.buffer.readline(MAX_BYTES + 1)
            if not line:
                break
            if len(line) > MAX_BYTES:
                break
            request = {}
            try:
                request = json.loads(line)
                if not isinstance(request, dict):
                    continue
                method, identifier = request.get("method"), request.get("id")
                if method == "shutdown":
                    break
                if identifier is None:
                    continue
                params = request.get("params") or {}
                if method == "hello":
                    if params.get("kind") != "provider" or any(not (params[key][0] <= 1 <= params[key][1]) for key in ("transport", "contract")):
                        raise Refusal("unsupported_version", "provider transport and contract 1 are required")
                    result = {"kind": "provider", "name": source.description["name"], "version": "0.1.0",
                              "transport": 1, "contract": 1, "methods": methods, "descriptor": source.description}
                elif method == "describe":
                    result = source.description
                elif method == "ping":
                    result = {}
                else:
                    result = source.call(method, params)
                reply = {"id": identifier, "result": result}
                if len(encoded(reply).encode()) > MAX_BYTES:
                    raise Refusal("too_large", "the native report exceeds the provider reply bound")
            except Refusal as error:
                reply = {"id": identifier, "error": error.wire()}
            except (ValueError, TypeError, KeyError, OSError):
                # Never echo a native response, input, environment or exception carrying a sign-in.
                reply = {"id": request.get("id") if isinstance(request, dict) else None,
                         "error": Refusal("malformed", "the native read did not have its expected shape", True).wire()}
            sys.stdout.write(encoded(reply) + "\n")
            sys.stdout.flush()
    finally:
        source.close()
