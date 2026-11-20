#!/usr/bin/env python3
"""Wait for the exact commit's full CI run before allowing publication.

The deployment scenarios are not a job of that run: the release workflow runs them against its own package
(`scenarios.yml`), and `assemble` waits for them.
"""

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


def latest_run(commit, runs, branch=None):
    """Select the most recent full push or manual CI run of the commit, optionally on the rehearsal branch."""
    eligible = [run for run in runs if run.get("head_sha") == commit
                and (branch is None or run.get("head_branch") == branch) and run.get("event") in {"push", "workflow_dispatch"}]
    return max(eligible, key=lambda run: run["id"]) if eligible else None


def status(tag, commit, runs, *, same_ref=False):
    """Return the green run, wait for an unfinished one, or refuse a finished failure."""
    run = latest_run(commit, runs, tag if same_ref else None)
    if run is None or run.get("status") != "completed":
        return None
    if run.get("conclusion") != "success":
        raise ValueError(f"CI run {run['id']} for {tag} completed with {run.get('conclusion') or 'no conclusion'}; nothing published")
    return run


def fetch(repo, commit):
    response = subprocess.run(
        ["gh", "api", "--paginate", "--slurp", f"repos/{repo}/actions/workflows/ci.yml/runs?head_sha={commit}&per_page=100"],
        check=True, capture_output=True, text=True, timeout=API_SECONDS)
    return [run for page in json.loads(response.stdout) for run in page["workflow_runs"]]


def dispatch(repo, tag):
    subprocess.run(["gh", "workflow", "run", "ci.yml", "--repo", repo, "--ref", tag],
                   check=True, timeout=API_SECONDS)


def wait(tag, commit, repo, once=False, clock=time.monotonic, sleep=time.sleep, *, start_missing=False, same_ref=False):
    deadline = clock() + (0 if once else WAIT_SECONDS)
    missing_before = False
    while True:
        runs = fetch(repo, commit)
        run = status(tag, commit, runs, same_ref=same_ref)
        if run is not None:
            print(f"CI run {run['id']} attempt {run.get('run_attempt', 1)} is green for {tag} ({commit})", flush=True)
            return run
        remaining = deadline - clock()
        if remaining <= 0:
            raise ValueError(f"CI for {tag} ({commit}) has not completed green before the deadline; nothing published")
        if start_missing:
            # Allow a simultaneous branch push one polling interval to register before dispatching.
            missing = latest_run(commit, runs) is None
            if missing and missing_before:
                dispatch(repo, tag)
                start_missing = False
            missing_before = missing
        print(f"Waiting for CI for {tag} ({commit}); up to {int(remaining)} seconds remain", flush=True)
        sleep(min(POLL_SECONDS, remaining))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--ref", "--tag", dest="ref", required=True, help="exact CI head branch or release tag")
    parser.add_argument("--commit", required=True)
    parser.add_argument("--repo", default=os.environ.get("GITHUB_REPOSITORY"), required=False)
    parser.add_argument("--start-missing", action="store_true", help="dispatch CI on the release tag if no run exists")
    parser.add_argument("--same-ref", action="store_true", help="require the dispatched rehearsal branch")
    parser.add_argument("--once", action="store_true", help="recheck immediately before publication")
    args = parser.parse_args()
    try:
        if not args.repo:
            raise ValueError("--repo is required")
        wait(args.ref, args.commit, args.repo, args.once, start_missing=args.start_missing, same_ref=args.same_ref)
        return 0
    except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
        print(f"::error::release CI: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
