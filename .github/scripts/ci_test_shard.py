#!/usr/bin/env python3
"""Run a disjoint share of every workspace test target without hiding failures."""

import argparse
import json
import subprocess
import sys


# Splitting the largest packages keeps the linked programs below the disk available
# on a hosted runner; the remaining packages are balanced by test-target count.
PARTS = {"api": ("orch-api-core", 6), "engine": ("orch-engine", 3)}
OTHER_SHARDS = 4


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
            result.append(common + ["-p", package_name, "--lib", "--bins", "--examples"])
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shard", required=True)
    args = parser.parse_args()
    raw = subprocess.run(["cargo", "metadata", "--locked", "--no-deps", "--format-version=1"],
                         check=True, capture_output=True, text=True).stdout
    planned = commands(json.loads(raw), args.shard)
    failed = False
    for command in planned:
        print("+", " ".join(command), flush=True)
        if subprocess.run(command, check=False).returncode:
            failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
