#!/usr/bin/env python3
"""Prepare the images and static executables required by the assigned native tests."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
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
NODE_LAB_TESTS = {"environment", "images", "isolation_enrollment", "lifecycle", "milestone", "uploads"}


def includes_node_tests(planned, targets):
    """Keep the package and target selection together, before libtest arguments."""
    for command in planned:
        if command[:2] != ["cargo", "test"]:
            continue
        arguments = command[2:command.index("--")] if "--" in command else command[2:]
        packages = {arguments[index + 1] for index, argument in enumerate(arguments[:-1])
                    if argument in {"-p", "--package"}}
        if "orch-node" not in packages:
            continue
        tests = {arguments[index + 1] for index, argument in enumerate(arguments[:-1]) if argument == "--test"}
        other_targets = {"--lib", "--bin", "--bins", "--example", "--examples", "--bench", "--benches", "--doc"}
        if (targets.intersection(tests) or "--tests" in arguments or "--all-targets" in arguments
                or not tests and not other_targets.intersection(arguments)):
            return True
    return False


def needs_conformance_runner(planned):
    return includes_node_tests(planned, {"conformance"})


def needs_node_connector(planned):
    return includes_node_tests(planned, NODE_LAB_TESTS)


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
    elif needs_node_connector(planned):
        images.append((BUSYBOX_IMAGE, "busybox:latest"))
    if targets & {"forwarded_edge", "kfix_deploy"}:
        images.append((NGINX_IMAGE, "nginx:1.30-alpine"))
    return images, bool(needs_conformance_runner(planned) or packages & HARNESS_PACKAGES or targets & {
        "trace_walk", "native_executor_loss", "background_wake_native",
    })


def target_commands(path):
    selected = [shlex.split(line, comments=True) for line in Path(path).read_text().splitlines()]
    return [["cargo", "test", *line[1:]] for line in selected if line and line[0] == "test"]


def describe_fixtures(planned):
    images, harness = fixture_plan(planned)
    return {"images": images, "harness": harness, "node_runner": needs_conformance_runner(planned),
            "node_connector": needs_node_connector(planned)}


def executable_path(value):
    path = Path(value).resolve(strict=True)
    if not path.is_file() or not os.access(path, os.X_OK):
        raise RuntimeError("a native fixture must be an executable file")
    return str(path)


def static_program(package, binary):
    targets = {"x86_64": "x86_64-unknown-linux-musl", "amd64": "x86_64-unknown-linux-musl",
               "aarch64": "aarch64-unknown-linux-musl", "arm64": "aarch64-unknown-linux-musl"}
    target = targets.get(platform.machine().lower())
    if target is None:
        raise RuntimeError("native fixtures need a supported Linux musl target")
    command = ["cargo", "build", "--locked", "--target", target,
               "-p", package, "--bin", binary, "--message-format=json"]
    environment = dict(os.environ)
    environment.setdefault("CC_" + target.replace("-", "_"), "musl-gcc")
    print("+", " ".join(command), flush=True)
    runner = None
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True, encoding="utf-8",
                          errors="replace", env=environment) as process:
        for line in process.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="", flush=True)
                continue
            if not isinstance(message, dict):
                print(line, end="", flush=True)
                continue
            if message.get("reason") == "compiler-message":
                diagnostic = message["message"]
                print(diagnostic.get("rendered") or diagnostic["message"], end="", flush=True)
            elif (message.get("reason") == "compiler-artifact"
                  and message.get("target", {}).get("name") == binary
                  and message["target"].get("kind") == ["bin"]
                  and not message.get("profile", {}).get("test") and message.get("executable")):
                runner = message["executable"]
        code = process.wait()
    if code:
        raise subprocess.CalledProcessError(code, command)
    if runner is None:
        raise RuntimeError(f"Cargo did not report the built {binary} executable")
    return executable_path(runner)


def conformance_runner():
    configured = os.environ.get("ORCH_TEST_CONFORMANCE_NODE")
    return executable_path(configured) if configured else static_program("orch-node", "orch-node")


def node_connector():
    configured = os.environ.get("ORCH_TEST_CONNECTOR_BINARY")
    return executable_path(configured) if configured else static_program("orch-connector", "orch-connector")


def prepare_fixtures(planned):
    description = describe_fixtures(planned)
    print(json.dumps(description), flush=True)
    images, harness = description["images"], description["harness"]
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
    environment = {}
    if description["node_runner"]:
        environment["ORCH_TEST_CONFORMANCE_NODE"] = conformance_runner()
    if description["node_connector"]:
        environment["ORCH_TEST_CONNECTOR_BINARY"] = node_connector()
    return environment


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
        planned = target_commands(args.targets)
    if args.plan:
        print(json.dumps(describe_fixtures(planned)), flush=True)
        return 0
    environment = prepare_fixtures(planned)
    if environment:
        if any("\n" in value or "\r" in value for value in environment.values()):
            raise RuntimeError("native fixture paths must fit on one environment line")
        lines = "".join(f"{key}={value}\n" for key, value in environment.items())
        if os.environ.get("GITHUB_ENV"):
            with open(os.environ["GITHUB_ENV"], "a", encoding="utf-8") as output:
                output.write(lines)
        else:
            print("Use this fixture environment when running the tests:\n" + lines, end="", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
