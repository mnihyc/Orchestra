#!/usr/bin/env python3
"""Report sampled whole-runner memory, including Docker builds, around a command."""

import argparse
from dataclasses import dataclass
import json
from pathlib import Path
import subprocess
from threading import Event, Thread


# One-second samples bound the log to a before/after summary while catching sustained
# compiler pressure. They are whole-runner usage, not an individual process's peak RSS.
SAMPLE_SECONDS = 1
FIELDS = ("MemTotal", "MemAvailable", "SwapTotal", "SwapFree")


def parse_memory(text):
    values = {}
    for line in text.splitlines():
        name, separator, value = line.partition(":")
        if separator and name in FIELDS:
            parts = value.split()
            if len(parts) != 2 or parts[1] != "kB":
                raise ValueError(f"invalid memory field {name}")
            values[name] = int(parts[0]) * 1024
    if any(name not in values for name in FIELDS):
        raise ValueError("memory accounting is unavailable")
    if any(value < 0 for value in values.values()):
        raise ValueError("negative memory accounting")
    if values["MemAvailable"] > values["MemTotal"] or values["SwapFree"] > values["SwapTotal"]:
        raise ValueError("inconsistent memory accounting")
    return values


def read_memory():
    return parse_memory(Path("/proc/meminfo").read_text())


@dataclass
class MemoryUsage:
    samples: int = 0
    read_errors: int = 0
    ram_total_bytes: int | None = None
    swap_total_bytes: int | None = None
    initial_ram_used_bytes: int | None = None
    initial_swap_used_bytes: int | None = None
    sampled_peak_ram_used_bytes: int | None = None
    sampled_peak_swap_used_bytes: int | None = None

    def sample(self):
        try:
            values = read_memory()
        except (OSError, ValueError):
            self.read_errors += 1
            return
        ram_used = values["MemTotal"] - values["MemAvailable"]
        swap_used = values["SwapTotal"] - values["SwapFree"]
        if not self.samples:
            self.ram_total_bytes = values["MemTotal"]
            self.swap_total_bytes = values["SwapTotal"]
            self.initial_ram_used_bytes = ram_used
            self.initial_swap_used_bytes = swap_used
        self.samples += 1
        self.sampled_peak_ram_used_bytes = max(self.sampled_peak_ram_used_bytes or 0, ram_used)
        self.sampled_peak_swap_used_bytes = max(self.sampled_peak_swap_used_bytes or 0, swap_used)

    def report(self, stage):
        print("CI memory " + json.dumps({
            "stage": stage,
            "scope": "whole runner, including builder containers",
            "sample_interval_seconds": SAMPLE_SECONDS,
            **vars(self),
        }, sort_keys=True), flush=True)


def run(command):
    usage = MemoryUsage()
    stop = Event()
    usage.sample()
    usage.report("before command")

    def sample_until_stopped():
        while not stop.wait(SAMPLE_SECONDS):
            usage.sample()

    sampler = Thread(target=sample_until_stopped, daemon=True)
    sampler.start()
    try:
        # Keep the command's argv, environment, output and working directory intact.
        result = subprocess.run(command, check=False)
        return result.returncode if result.returncode >= 0 else 128 - result.returncode
    finally:
        stop.set()
        sampler.join()
        usage.sample()
        usage.report("after command")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if not args.command:
        parser.error("a command is required")
    return run(args.command)


if __name__ == "__main__":
    raise SystemExit(main())
