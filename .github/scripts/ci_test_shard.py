#!/usr/bin/env python3
"""Run a disjoint share of every workspace test target without hiding failures."""

import argparse
import codecs
import json
import math
import os
from pathlib import Path
import re
import selectors
import shutil
import signal
import subprocess
import sys
import time
import tomllib


# Splitting the largest packages keeps the linked programs below the disk available
# on a hosted runner; the remaining packages are balanced by test-target count.
PARTS = {"api": ("orch-api-core", 6), "engine": ("orch-engine", 3)}
OTHER_SHARDS = 4
# Completed scale binaries take up to 30 minutes on hosted runners. Leave 50%
# headroom while bounding a single hang within the 180-minute shard budget.
DEFAULT_BINARY_TIMEOUT_SECONDS = 45 * 60
STOP_GRACE_SECONDS = 5
# Listing the harness's case names does not execute them; bound startup as well.
LIST_TIMEOUT_SECONDS = 60
# Forward all output, but retain at most one pipe buffer for parsing progress.
PROGRESS_LINE_LIMIT = 64 * 1024
LIB_KINDS = {"lib", "rlib", "dylib", "cdylib", "staticlib", "proc-macro"}


def integration_targets(package):
    return sorted(target["name"] for target in package["targets"]
                  if "test" in target["kind"] and target["test"])


def other_groups(packages):
    """Assign each remaining package once, largest test-target count first."""
    excluded = {package for package, _ in PARTS.values()}
    remaining = [package for package in packages if package["name"] not in excluded]
    groups = [[] for _ in range(OTHER_SHARDS)]
    loads = [0] * OTHER_SHARDS
    def weight(package):
        return max(1, sum(bool(target["test"]) + bool(target["doctest"]) for target in package["targets"]))
    for package in sorted(remaining, key=lambda item: (-weight(item), item["name"])):
        index = min(range(OTHER_SHARDS), key=lambda group: (loads[group], group))
        groups[index].append(package["name"])
        loads[index] += weight(package)
    return groups


def commands(metadata, shard):
    """Return Cargo commands covering this shard's tests and no other shard's."""
    packages = metadata["packages"]
    kind, _, number = shard.partition("-")
    if not number.isdecimal():
        raise ValueError(f"invalid test shard {shard!r}")
    index = int(number)
    common = ["cargo", "test", "--locked", "--no-fail-fast"]
    if kind in PARTS:
        package_name, count = PARTS[kind]
        if index >= count:
            raise ValueError(f"invalid test shard {shard!r}")
        package, = (item for item in packages if item["name"] == package_name)
        targets = integration_targets(package)[index::count]
        if not targets:
            raise ValueError(f"test shard {shard} has no integration targets")
        tests = common + ["-p", package_name]
        for target in targets:
            tests += ["--test", target]
        result = [tests]
        if index == 0:
            # Target selection suppresses Cargo's default unit, example and doc tests.
            units = common + ["-p", package_name, "--lib", "--bins", "--examples"]
            if package_name == "orch-engine":
                # The current CI PostgreSQL service has 97 usable connections; each fixture
                # reserves 32, so only three database-owning unit cases fit at once.
                units += ["--", "--test-threads=3"]
            result.append(units)
            result.append(common + ["-p", package_name, "--doc"])
        return result
    if kind == "rest" and index < OTHER_SHARDS:
        assigned = other_groups(packages)[index]
        if not assigned:
            raise ValueError(f"test shard {shard} has no packages")
        result = common[:]
        for package in assigned:
            result += ["-p", package]
        return [result]
    raise ValueError(f"invalid test shard {shard!r}")


def execution_plan(metadata, shard):
    """Build the selected executables once; Cargo still owns doc/engine unit runs."""
    selected = commands(metadata, shard)
    build = selected[0][:]
    after = []
    if shard.startswith("rest-"):
        assigned = {build[i + 1] for i, argument in enumerate(build[:-1]) if argument == "-p"}
        docs = [package["name"] for package in metadata["packages"]
                if package["name"] in assigned and any(target["doctest"] for target in package["targets"])]
        if docs:
            doc = ["cargo", "test", "--locked", "--no-fail-fast"]
            for package in docs:
                doc += ["-p", package]
            after.append(doc + ["--doc"])
    elif len(selected) > 1:
        build += ["--lib", "--bins", "--examples"]
        if shard == "engine-0":
            after.append(selected[1])
        after.append(selected[2])
    # cargo test takes no --keep-going (cargo build does); --no-fail-fast runs every test binary that builds.
    return build + ["--no-run", "--message-format=json"], after


def custom_harness(package, target):
    if target["kind"] == ["test"]:
        return True
    if not target.get("test"):
        return False
    manifest = tomllib.loads(Path(package["manifest_path"]).read_text())
    if LIB_KINDS.intersection(target["kind"]):
        entries = [manifest.get("lib", {})]
    else:
        entries = manifest.get(target["kind"][0], [])
    return any(entry.get("name", target["name"]) == target["name"] and entry.get("harness") is False
               for entry in entries)


def build_binaries(metadata, shard, command):
    packages = {package["id"]: package for package in metadata["packages"]}
    assigned = {command[i + 1] for i, argument in enumerate(command[:-1]) if argument == "-p"}
    binaries, scripts, programs = {}, {}, {}
    print("+", " ".join(command), flush=True)
    with subprocess.Popen(command, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace") as process:
        for line in process.stdout:
            try:
                message = json.loads(line)
            except json.JSONDecodeError:
                print(line, end="", flush=True)
                continue
            if not isinstance(message, dict):
                print(line, end="", flush=True)
                continue
            reason = message.get("reason")
            if reason == "compiler-message":
                print(message["message"].get("rendered") or message["message"]["message"], end="", flush=True)
            elif reason == "build-script-executed":
                scripts[message["package_id"]] = message
            elif reason == "compiler-artifact" and message.get("executable"):
                package = packages.get(message["package_id"])
                if package is None or package["name"] not in assigned:
                    continue
                target = message["target"]
                if target["kind"] == ["bin"] and not message["profile"]["test"]:
                    programs.setdefault(package["id"], {})[target["name"]] = message["executable"]
                if not message["profile"]["test"] and not custom_harness(package, target):
                    continue
                if shard == "engine-0" and package["name"] == "orch-engine" and target["kind"] != ["test"]:
                    continue
                key = (package["id"], tuple(target["kind"]), target["name"], message["executable"])
                binaries[key] = {"package": package, "target": target, "executable": message["executable"],
                                 "harness": message["profile"]["test"]}
            elif reason not in {"compiler-artifact", "build-finished"}:
                print(line, end="", flush=True)
        code = process.wait()
    ordered = sorted(binaries.values(), key=lambda binary: (
        binary["package"]["name"], binary["target"]["kind"] == ["test"], binary["target"]["src_path"]))
    return code, ordered, scripts, programs


def binary_environment(binary, metadata, scripts, programs, rustlib):
    """Retain the caller's fixtures and Cargo's package/bin/dynamic-library context."""
    package, target = binary["package"], binary["target"]
    environment = dict(os.environ)
    manifest = Path(package["manifest_path"])
    environment.update(CARGO=os.environ.get("CARGO") or shutil.which("cargo") or "cargo",
                       CARGO_MANIFEST_DIR=str(manifest.parent), CARGO_MANIFEST_PATH=str(manifest),
                       CARGO_CRATE_NAME=target["name"].replace("-", "_"))
    version = package["version"]
    major, minor, patch = version.split(".", 2)
    environment.update(CARGO_PKG_VERSION=version, CARGO_PKG_VERSION_MAJOR=major,
                       CARGO_PKG_VERSION_MINOR=minor, CARGO_PKG_VERSION_PATCH=re.split(r"[-+]", patch)[0],
                       CARGO_PKG_VERSION_PRE=version.partition("+")[0].partition("-")[2],
                       CARGO_PKG_AUTHORS=":".join(package["authors"]))
    for key in ("name", "description", "homepage", "repository", "license", "license_file", "rust_version", "readme"):
        environment["CARGO_PKG_" + key.upper()] = package.get(key) or ""
    if target["kind"] == ["bin"] or target["kind"] == ["example"] and target["crate_types"] == ["bin"]:
        environment["CARGO_BIN_NAME"] = target["name"]
    else:
        environment.pop("CARGO_BIN_NAME", None)
    for name, executable in programs.get(package["id"], {}).items():
        environment["CARGO_BIN_EXE_" + name] = executable
    for name, value in scripts.get(package["id"], {}).get("env", []):
        environment[name] = value
    target_dir = Path(metadata["target_directory"])
    output_dir = Path(binary["executable"]).parent
    profile_dir = output_dir.parent if output_dir.name in {"deps", "examples"} else output_dir
    paths = []
    for script in scripts.values():
        for linked in script.get("linked_paths", []):
            kind, equals, path = linked.partition("=")
            path = Path(path if equals and kind in {"native", "framework", "dependency", "crate", "all"} else linked)
            if path.is_relative_to(target_dir):
                paths.append(str(path))
    paths += [str(profile_dir / "deps"), str(profile_dir), rustlib]
    if environment.get("LD_LIBRARY_PATH"):
        paths.append(environment["LD_LIBRARY_PATH"])
    environment["LD_LIBRARY_PATH"] = os.pathsep.join(dict.fromkeys(paths))
    return environment


def stop_group(process):
    """Signal only the process group created for this executable, never a pattern."""
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        process.wait()
        return
    try:
        process.wait(timeout=STOP_GRACE_SECONDS)
    except subprocess.TimeoutExpired:
        pass
    # A parent can exit on TERM while a descendant ignores it and keeps stdout open.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait()


class Output:
    def __init__(self, observe, echo=True):
        self.observe, self.echo = observe, echo
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        self.partial = ""
        self.skipping = False

    def write(self, data, final=False):
        text = self.decoder.decode(data, final=final)
        if self.echo:
            print(text, end="", flush=True)
        for piece in text.splitlines(keepends=True):
            self.partial += piece
            if len(self.partial) > PROGRESS_LINE_LIMIT:
                self.partial, self.skipping = "", True
            if piece.endswith("\n"):
                if not self.skipping:
                    self.observe(self.partial.rstrip("\r\n"))
                self.partial, self.skipping = "", False
        if final and self.partial and not self.skipping:
            self.observe(self.partial)
            self.partial = ""


def execute(command, cwd, environment, timeout, observe, echo=True):
    output = Output(observe, echo)
    timed_out = False
    began = time.monotonic()
    with subprocess.Popen(command, cwd=cwd, env=environment, stdout=subprocess.PIPE,
                          stderr=subprocess.STDOUT, start_new_session=True, bufsize=0) as process:
        os.set_blocking(process.stdout.fileno(), False)
        with selectors.DefaultSelector() as ready:
            ready.register(process.stdout, selectors.EVENT_READ)
            try:
                while ready.get_map():
                    remaining = began + timeout - time.monotonic()
                    if remaining <= 0:
                        raise subprocess.TimeoutExpired(command, timeout)
                    for key, _ in ready.select(remaining):
                        data = os.read(key.fd, PROGRESS_LINE_LIMIT)
                        if data:
                            output.write(data)
                        else:
                            ready.unregister(key.fileobj)
                process.wait(timeout=max(0, began + timeout - time.monotonic()))
            except subprocess.TimeoutExpired:
                timed_out = True
            finally:
                stop_group(process)
                # Drain already-written diagnostics without waiting for an unrelated
                # detached process that might have inherited the pipe.
                drain_until = time.monotonic() + STOP_GRACE_SECONDS
                while ready.get_map() and time.monotonic() < drain_until:
                    events = ready.select(0)
                    if not events:
                        break
                    for key, _ in events:
                        data = os.read(key.fd, PROGRESS_LINE_LIMIT)
                        if data:
                            output.write(data)
                        else:
                            ready.unregister(key.fileobj)
                output.write(b"", final=True)
    return process.returncode, timed_out, time.monotonic() - began


class Progress:
    def __init__(self):
        self.cases, self.completed, self.running = set(), {}, set()
        self.summary = False

    def listing(self, line):
        if line.endswith((": test", ": benchmark")):
            self.cases.add(line.rsplit(": ", 1)[0])
        else:
            # Keep startup/dynamic-loader errors visible even when listing fails.
            print(line, flush=True)

    def observe(self, line):
        line = re.sub(r"\x1b\[[0-9;]*m", "", line)
        self.summary |= line.startswith("test result:")
        result = re.fullmatch(r"test (.+) \.\.\. (ok|FAILED|ignored(?:,.*)?|bench:.*)", line)
        if result and (result[1] in self.cases or result[1] in self.running):
            self.completed[result[1]] = result[2]
            self.running.discard(result[1])
        waiting = re.fullmatch(r"test (.+) has been running for over \d+ seconds", line)
        if waiting and waiting[1] not in self.completed:
            self.running.add(waiting[1])

    def failure(self, label, elapsed, reason):
        print(f"\n::error::{label}: {reason}", flush=True)
        running = sorted(self.running.difference(self.completed))
        for case in running:
            print(f"test {case} ... FAILED", flush=True)
        if running:
            print("still-running cases reported by the harness: " + ", ".join(running), flush=True)
        pending = sorted(self.cases.difference(self.completed, running))
        for case in pending:
            print("not completed (running or queued): " + case, flush=True)
        if not running and not pending:
            print("no running case was named; the binary stopped during startup or teardown", flush=True)
        passed = sum(status == "ok" for status in self.completed.values())
        failed = sum(status == "FAILED" for status in self.completed.values())
        ignored = sum(status.startswith("ignored") for status in self.completed.values())
        # The extra failure is this runner's binary-level timeout/exit, not an
        # invented assertion from an unstarted case. Keep routing's summary prefix.
        print(f"test result: FAILED. {passed} passed; {failed + max(1, len(running))} failed; "
              f"{ignored} ignored; 0 measured; 0 filtered out; finished in {elapsed:.2f}s (binary failure)", flush=True)


def run_binary(binary, metadata, scripts, programs, rustlib, timeout):
    package, target = binary["package"], binary["target"]
    cwd = Path(package["manifest_path"]).parent
    source = os.path.relpath(target["src_path"], cwd)
    display = source if target["kind"] == ["test"] else "unittests " + source
    print(f"     Running {display} ({os.path.relpath(binary['executable'])})", flush=True)
    label = package["name"] + "::" + target["name"]
    environment = binary_environment(binary, metadata, scripts, programs, rustlib)
    progress = Progress()
    began = time.monotonic()
    try:
        if binary["harness"]:
            code, expired, elapsed = execute([binary["executable"], "--list", "--format=terse"],
                                              cwd, environment, min(timeout, LIST_TIMEOUT_SECONDS), progress.listing, echo=False)
            if code or expired:
                progress.failure(label, elapsed, "test-case listing timed out" if expired else f"test-case listing exited {code}")
                return False
        remaining = timeout - (time.monotonic() - began)
        if remaining <= 0:
            progress.failure(label, time.monotonic() - began, f"exceeded {timeout:g}s")
            return False
        code, expired, _ = execute([binary["executable"]], cwd, environment, remaining, progress.observe)
        elapsed = time.monotonic() - began
        if expired or code and not progress.summary:
            progress.failure(label, elapsed, f"exceeded {timeout:g}s" if expired else f"exited {code}")
        return not expired and code == 0
    except OSError as error:
        progress.failure(label, time.monotonic() - began, f"could not run: {error}")
        return False


def positive_seconds(value):
    try:
        seconds = float(value)
    except ValueError as error:
        raise argparse.ArgumentTypeError("the binary timeout must be positive seconds") from error
    if not math.isfinite(seconds) or seconds <= 0:
        raise argparse.ArgumentTypeError("the binary timeout must be finite and positive")
    return seconds


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard", required=True)
    parser.add_argument("--binary-timeout-seconds", type=positive_seconds,
                        default=os.environ.get("ORCH_CI_TEST_BINARY_TIMEOUT_SECONDS", str(DEFAULT_BINARY_TIMEOUT_SECONDS)))
    args = parser.parse_args()
    raw = subprocess.run(["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
                         check=True, capture_output=True, text=True).stdout
    metadata = json.loads(raw)
    build, after = execution_plan(metadata, args.shard)
    code, binaries, scripts, programs = build_binaries(metadata, args.shard, build)
    failed = code != 0
    rustlib = subprocess.run([os.environ.get("RUSTC", "rustc"), "--print", "target-libdir"],
                             check=True, capture_output=True, text=True).stdout.strip()
    print(f"Each test binary has a {args.binary_timeout_seconds:g}s limit.", flush=True)
    for binary in binaries:
        if not run_binary(binary, metadata, scripts, programs, rustlib, args.binary_timeout_seconds):
            failed = True
    for command in after:
        print("+", " ".join(command), flush=True)
        if subprocess.run(command, check=False).returncode:
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
