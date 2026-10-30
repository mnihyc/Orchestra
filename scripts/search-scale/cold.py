#!/usr/bin/env python3
"""Coordinate cold samples by restarting only an explicitly selected, bind-mounted search fixture."""

import datetime
import json
from pathlib import Path
import subprocess
import threading
import time


# These bound fixture coordination, outside the unchanged measured request budgets.
RESTART_TIMEOUT_SECS = 60
READY_TIMEOUT_SECS = 20
POLL_SECS = 0.1


class Controller:
    """Restart the verified fixture once for each unique request from the compiled measurement target."""

    def __init__(self, container, address, directory):
        if not container.startswith("orchprobe-search-"):
            raise ValueError("cold sampling requires an explicitly named orchprobe-search- fixture")
        inspected = subprocess.run(
            ["docker", "inspect", container], check=True, text=True, capture_output=True, timeout=10,
        )
        facts = json.loads(inspected.stdout)[0]
        published = facts["NetworkSettings"]["Ports"].get("5432/tcp") or []
        if not any(item["HostIp"] == "127.0.0.1" and int(item["HostPort"]) == address.port for item in published):
            raise ValueError("the cold container must publish the fixture database's exact loopback port")
        data = next((value.split("=", 1)[1] for value in facts["Config"].get("Env", [])
                     if value.startswith("PGDATA=")), "/var/lib/postgresql/data")
        self.binding = next((item["Source"] for item in facts["Mounts"]
                             if item["Type"] == "bind" and item["Destination"] == data), None)
        if self.binding is None:
            raise ValueError("the cold fixture must bind-mount its data directory")
        self.container = container
        self.user = address.username or "postgres"
        self.database = address.path.lstrip("/")
        self.directory = Path(directory)
        self.directory.mkdir()
        self.events = []
        self.stop = threading.Event()
        self.thread = threading.Thread(target=self._run, daemon=True)

    def start(self):
        """Start one coordinator before the compiled target is launched."""
        self.thread.start()

    def finish(self):
        """Wait for any in-flight restart before returning control to the caller."""
        self.stop.set()
        self.thread.join(RESTART_TIMEOUT_SECS + READY_TIMEOUT_SECS + 5)
        if self.thread.is_alive():
            raise RuntimeError("the owned fixture coordinator did not stop")

    def _restart(self):
        subprocess.run(["docker", "restart", self.container], check=True, capture_output=True,
                       timeout=RESTART_TIMEOUT_SECS)
        deadline = time.monotonic() + READY_TIMEOUT_SECS
        while time.monotonic() < deadline:
            ready = subprocess.run(
                ["docker", "exec", self.container, "pg_isready", "-U", self.user, "-d", self.database],
                capture_output=True, timeout=3,
            )
            if ready.returncode == 0:
                return
            time.sleep(POLL_SECS)
        raise TimeoutError("the owned database did not become ready")

    def _run(self):
        seen = set()
        while not self.stop.is_set():
            for request in sorted(self.directory.glob("*.request")):
                if request.name in seen:
                    continue
                seen.add(request.name)
                started = time.monotonic()
                event = {"sample": request.stem, "container": self.container,
                         "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat()}
                try:
                    self._restart()
                    event["ok"] = True
                except (OSError, ValueError, subprocess.SubprocessError, TimeoutError) as error:
                    event.update(ok=False, error=str(error))
                event["elapsed_ms"] = (time.monotonic() - started) * 1000
                self.events.append(event)
                request.with_suffix(".ready").write_text(json.dumps(event) + "\n")
            self.stop.wait(POLL_SECS)
