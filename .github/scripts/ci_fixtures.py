#!/usr/bin/env python3
"""Prepare the Docker images required by the assigned native tests."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import shlex
import subprocess
import sys

SHARD_MODULE = importlib.util.spec_from_file_location("orch_ci_shards", Path(__file__).with_name("ci_test_shard.py"))
if SHARD_MODULE is None or SHARD_MODULE.loader is None:
    raise RuntimeError("the test shard planner is unavailable")
SHARDS = importlib.util.module_from_spec(SHARD_MODULE)
SHARD_MODULE.loader.exec_module(SHARDS)


ALPINE_IMAGE = "alpine:latest@sha256:be2656061b536c1883c4034790e21d0a09c5aaaf51968b89368a3c0371ea2c9a"
BUSYBOX_IMAGE = "busybox:latest@sha256:a46257e06c82ad85242d0d38077d0a52d9896389ca60a1a59d3463db586e5fa4"
NGINX_IMAGE = "nginx:1.30-alpine@sha256:0985e772fb9f729e6fa0980da05fca5d9c468e870eed43071545afa9d2e27d94"
HARNESS_PACKAGES = {"orch-adapter-codex", "orch-adapter-claude-code", "orch-executor-codex",
                    "orch-connector-core", "orch-driver-docker"}


def fixture_plan(planned):
    """Keep images off shards whose assigned cases never use them."""
    packages, targets = set(), set()
    for command in planned:
        for index, argument in enumerate(command[:-1]):
            if argument == "-p":
                packages.add(command[index + 1])
            elif argument == "--test":
                targets.add(command[index + 1])
    images = []
    if "orch-driver-docker" in packages or targets & {"karchive_delete", "restart"}:
        images.extend([(ALPINE_IMAGE, "alpine:latest"), (BUSYBOX_IMAGE, "busybox:latest")])
    if targets & {"forwarded_edge", "kfix_deploy"}:
        images.append((NGINX_IMAGE, "nginx:1.30-alpine"))
    return images, bool(packages & HARNESS_PACKAGES or targets & {"trace_walk", "native_executor_loss"})


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    selection = parser.add_mutually_exclusive_group(required=True)
    selection.add_argument("--shard")
    selection.add_argument("--targets", type=Path)
    parser.add_argument("--plan", action="store_true")
    args = parser.parse_args()
    if args.shard:
        metadata = json.loads(subprocess.run(
            ["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
            check=True, capture_output=True, text=True).stdout)
        planned = SHARDS.commands(metadata, args.shard)
    else:
        selected = [shlex.split(line, comments=True) for line in args.targets.read_text().splitlines()]
        planned = [["cargo", "test", *line[1:]] for line in selected if line and line[0] == "test"]
    images, harness = fixture_plan(planned)
    print(json.dumps({"images": images, "harness": harness}), flush=True)
    if args.plan:
        return 0
    for reference, alias in images:
        subprocess.run(["docker", "pull", reference], check=True)
        subprocess.run(["docker", "image", "tag", reference, alias], check=True)
    if harness:
        builds = json.loads(Path("deploy/harness-builds.json").read_text())
        versions = {entry["kind"]: entry["version"] for entry in builds
                    if entry.get("is_default") and entry["os"] == "linux" and entry["arch"] == "x86_64"}
        subprocess.run(["docker", "build", "--tag", "orch-dev-harness",
                        "--build-arg", f"CODEX_VERSION={versions['codex']}",
                        "--build-arg", f"CLAUDE_VERSION={versions['claude-code']}",
                        "--build-arg", f"HARNESS_UID={os.getuid()}", "images/harness-canary"], check=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
