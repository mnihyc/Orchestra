#!/usr/bin/env python3
"""Wait for the exact ref and commit's full CI run before allowing publication."""

import argparse
import json
import os
import subprocess
import sys
import time

# Bound the existing CI wait; long deployment jobs or runner queues may require a rerun after CI finishes.
WAIT_SECONDS = 5 * 60 * 60
# A half minute keeps the wait visible without repeatedly calling the hosting API.
POLL_SECONDS = 30
# Fail closed if the hosting API is unavailable rather than hanging indefinitely.
API_SECONDS = 60


def latest_run(tag, commit, runs):
    """Select the most recent push or manual CI run for this tag and commit."""
    eligible = [run for run in runs if run.get("head_sha") == commit
                and run.get("head_branch") == tag and run.get("event") in {"push", "workflow_dispatch"}]
    return max(eligible, key=lambda run: run["id"]) if eligible else None


def status(tag, commit, runs, *, require_dispatch=False):
    """Return the green run, wait for an unfinished one, or refuse a finished failure."""
    run = latest_run(tag, commit, runs)
    if run is None or run.get("status") != "completed":
        return None
    if run.get("conclusion") != "success":
        raise ValueError(f"CI run {run['id']} for {tag} completed with {run.get('conclusion') or 'no conclusion'}; nothing published")
    # Branch pushes skip the deployment scenarios; snapshot rehearsals require the manual full run.
    if require_dispatch and run.get("event") != "workflow_dispatch":
        return None
    return run


def fetch(repo, commit):
    response = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={commit}&per_page=100"],
        check=True, capture_output=True, text=True, timeout=API_SECONDS)
    return [run for page in json.loads(response.stdout) for run in page["workflow_runs"]]


def wait(tag, commit, repo, once=False, clock=time.monotonic, sleep=time.sleep, *, require_dispatch=False):
    deadline = clock() + (0 if once else WAIT_SECONDS)
    while True:
        run = status(tag, commit, fetch(repo, commit), require_dispatch=require_dispatch)
        if run is not None:
            print(f"CI run {run['id']} attempt {run.get('run_attempt', 1)} is green for {tag} ({commit})", flush=True)
            return run
        remaining = deadline - clock()
        if remaining <= 0:
            raise ValueError(f"CI for {tag} ({commit}) has not completed green before the deadline; nothing published")
        print(f"Waiting for CI for {tag} ({commit}); up to {int(remaining)} seconds remain", flush=True)
        sleep(min(POLL_SECONDS, remaining))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", "--tag", dest="ref", required=True, help="exact CI head branch or release tag")
    parser.add_argument("--commit", required=True)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"), required=False)
    parser.add_argument("--once", action="store_true", help="recheck immediately before publication")
    parser.add_argument("--require-dispatch", action="store_true", help="require the manual CI run that includes deployment on a snapshot branch")
    args = parser.parse_args()
    try:
        if not args.repo:
            raise ValueError("--repo is required")
        wait(args.ref, args.commit, args.repo, args.once, require_dispatch=args.require_dispatch)
        return 0
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(f"::error::release CI: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
