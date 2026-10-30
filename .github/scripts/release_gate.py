#!/usr/bin/env python3
"""Refuse a release tag unless it names this clean commit, advances published versions and has green CI.

A tag listed in `.github/release-ci-waivers.txt` with its reason may go without green CI; every other check holds.
"""

import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import sys

# Hosting metadata is small; an unavailable API must refuse a gate rather than hang it.
API_SECONDS = 60
# Tags released without waiting for CI, one per line with the reason beside it, committed with the release.
WAIVERS = Path(".github/release-ci-waivers.txt")
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


def waivers(text):
    """The listed tags and their reasons; a line without a reason is refused."""
    listed = {}
    for number, line in enumerate(text.splitlines(), 1):
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        tag, _, reason = line.strip().partition(" ")
        version(tag)
        if not reason.strip():
            raise ValueError(f"waiver line {number} for {tag} gives no reason")
        listed[tag] = reason.strip()
    return listed


def eligibility(tag, head, tagged, releases, runs, waived=None):
    """The green CI run's id, or None when the tag's waiver stands in for a run that is missing or not green."""
    version(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", head) or tagged != head:
        raise ValueError("the tag does not name the checked-out HEAD commit")
    for release in releases:
        if release.get("draft"):
            continue
        old = release.get("tag_name", "")
        if VERSION.fullmatch(old) and compare(tag, old) <= 0:
            raise ValueError(f"{tag} does not advance the published release {old}")
    relevant = [run for run in runs if run.get("head_sha") == head and run.get("event") in {"push", "pull_request", "workflow_dispatch"}]
    latest = max(relevant, key=lambda run: run["id"]) if relevant else None
    if latest and latest.get("status") == "completed" and latest.get("conclusion") == "success":
        return latest["id"]
    if waived:
        return None
    if not latest:
        raise ValueError("there is no CI run for the tagged commit")
    raise ValueError("the most recent CI run for the tagged commit is not completed and green")


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
        releases = json.loads(command("gh", "api", "--paginate", "--slurp", f"repos/{args.repo}/releases"))
        releases = [release for page in releases for release in page]
        runs = json.loads(command("gh", "api", f"repos/{args.repo}/actions/workflows/ci.yml/runs?head_sha={head}&per_page=100"))["workflow_runs"]
        waived = waivers(WAIVERS.read_text()).get(args.tag) if WAIVERS.is_file() else None
        run = eligibility(args.tag, head, tagged, releases, runs, waived)
        notes = Path(".github/release-notes") / f"{args.tag}.md"
        if not notes.is_file() or not notes.read_text().strip():
            raise ValueError(f"{notes} is required before packaging")
        if run is None:
            print(f"::warning::{args.tag} is released without a green CI run: {waived}")
        basis = f"CI run {run}" if run is not None else f"CI waived ({waived})"
        print(f"Eligible: {args.tag}, commit {head}, {basis}; no publication performed")
        if output := os.environ.get("GITHUB_OUTPUT"):
            with open(output, "a", encoding="utf-8") as file:
                file.write(f"tag={args.tag}\nversion={args.tag[1:]}\ncommit={head}\n")
        return 0
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(f"release gate: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
