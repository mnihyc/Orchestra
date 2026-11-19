#!/usr/bin/env python3
"""Check that reused artifacts come from an earlier completed release workflow."""

import json
import os
import re
import subprocess

# The hosting API should fail rather than hold a release gate indefinitely.
API_SECONDS = 60


def check_source(run_id, run):
    """Failed release runs are eligible when they finished and left usable artifacts."""
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("the source run id must be a positive integer")
    if str(run.get("id")) != run_id or run.get("path", "").split("@", 1)[0] != ".github/workflows/release.yml":
        raise ValueError("the source is not a run of the release workflow")
    if run.get("event") not in {"push", "workflow_dispatch"} or run.get("status") != "completed":
        raise ValueError("the source release run has not completed")


def has_frontend(pages):
    """Older release runs did not upload a shared frontend artifact."""
    return any(artifact.get("name") == "frontend-dist" and not artifact.get("expired")
               for page in pages for artifact in page["artifacts"])


def main():
    run_id = os.environ["SOURCE_RUN_ID"]
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("the source run id must be a positive integer")
    repo = os.environ["GITHUB_REPOSITORY"]
    response = subprocess.run(["gh", "api", f"repos/{repo}/actions/runs/{run_id}"], check=True,
                              capture_output=True, text=True, timeout=API_SECONDS)
    check_source(run_id, json.loads(response.stdout))
    artifacts = subprocess.run(["gh", "api", "--paginate", "--slurp", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100"],
                               check=True, capture_output=True, text=True, timeout=API_SECONDS)
    frontend = has_frontend(json.loads(artifacts.stdout))
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            file.write(f"frontend={str(frontend).lower()}\n")
    print(f"Reusing native inputs from completed release run {run_id}; shared frontend available: {frontend}")


if __name__ == "__main__":
    main()
