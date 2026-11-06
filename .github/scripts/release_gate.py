#!/usr/bin/env python3
"""Check the clean tagged commit and release notes before packaging."""

import argparse
import os
from pathlib import Path
import re
import subprocess
import sys

# Hosting metadata is small; an unavailable API must refuse a gate rather than hang it.
API_SECONDS = 60
VERSION = re.compile(r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?\Z")


def version(tag):
    match = VERSION.fullmatch(tag)
    if not match:
        raise ValueError(f"{tag!r} is not vMAJOR.MINOR.PATCH with an optional prerelease")
    pre = match[4].split(".") if match[4] else []
    if any(part.isdecimal() and len(part) > 1 and part[0] == "0" for part in pre):
        raise ValueError("numeric prerelease identifiers have no leading zero")
    return tuple(map(int, match.group(1, 2, 3))), pre


def compare(left, right):
    a, ap = version(left)
    b, bp = version(right)
    if a != b:
        return (a > b) - (a < b)
    if not ap or not bp:
        return (not ap) - (not bp)
    for x, y in zip(ap, bp):
        if x == y:
            continue
        if x.isdecimal() and y.isdecimal():
            return (int(x) > int(y)) - (int(x) < int(y))
        if x.isdecimal() != y.isdecimal():
            return -1 if x.isdecimal() else 1
        return (x > y) - (x < y)
    return (len(ap) > len(bp)) - (len(ap) < len(bp))


def eligibility(tag, head, tagged):
    """Require a valid tag naming the checked-out commit."""
    version(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", head) or tagged != head:
        raise ValueError("the tag does not name the checked-out HEAD commit")


def command(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True, timeout=API_SECONDS).stdout.strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", default=os.environ.get("RELEASE_TAG", os.environ.get("GITHUB_REF_NAME")))
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"))
    args = parser.parse_args()
    try:
        if not args.tag or not args.repo:
            raise ValueError("--tag and --repo are required")
        head = command("git", "rev-parse", "HEAD")
        tagged = command("git", "rev-parse", f"refs/tags/{args.tag}^{{commit}}")
        if command("git", "status", "--porcelain"):
            raise ValueError("release inputs have uncommitted changes")
        if os.environ.get("GITHUB_EVENT_NAME") != "workflow_dispatch" and os.environ.get("GITHUB_SHA", head) != head:
            raise ValueError("the workflow and checked-out commit differ")
        eligibility(args.tag, head, tagged)
        notes = Path(".github/release-notes") / f"{args.tag}.md"
        if not notes.is_file() or not notes.read_text().strip():
            raise ValueError(f"{notes} is required before packaging")
        print(f"Packaging eligible: {args.tag}, commit {head}; publication separately waits for CI")
        if output := os.environ.get("GITHUB_OUTPUT"):
            with open(output, "a", encoding="utf-8") as file:
                file.write(f"tag={args.tag}\nversion={args.tag[1:]}\ncommit={head}\n")
        return 0
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(f"release gate: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
