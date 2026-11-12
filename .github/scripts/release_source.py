#!/usr/bin/env python3
"""Check that reused artifacts come from an earlier completed release workflow."""

import json
import os
import re
import subprocess
from pathlib import Path
import tempfile

from release_contract import catalogue
from release_observe import validate_record

# The hosting API should fail rather than hold a release gate indefinitely.
API_SECONDS = 60


def check_source(run_id, run, commit=None):
    """Failed release runs are eligible when they finished and left usable artifacts."""
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("the source run id must be a positive integer")
    if str(run.get("id")) != run_id or run.get("path", "").split("@", 1)[0] != ".github/workflows/release.yml":
        raise ValueError("the source is not a run of the release workflow")
    if run.get("event") not in {"push", "workflow_dispatch"} or run.get("status") != "completed":
        raise ValueError("the source release run has not completed")
    if commit is not None and run.get("head_sha") != commit:
        raise ValueError("the source release run observed another candidate commit")


def has_frontend(pages):
    """Older release runs did not upload a shared frontend artifact."""
    return any(artifact.get("name") == "frontend-dist" and not artifact.get("expired")
               for page in pages for artifact in page["artifacts"])


def observation_artifacts(pages, targets):
    available = {artifact["name"] for page in pages for artifact in page["artifacts"] if not artifact.get("expired")}
    missing = {f"inv15-{target}" for target in targets} - available
    if missing:
        raise ValueError("the source run has no current platform evidence: " + ", ".join(sorted(missing)))


def require_observations(run_id, repo, commit, pages):
    root = Path(__file__).resolve().parents[2]
    targets = [target for target, native_os, *_ in catalogue(root, "TARGETS") if native_os != "linux"]
    observation_artifacts(pages, targets)
    with tempfile.TemporaryDirectory(prefix="orch-release-observations-") as name:
        for target in targets:
            directory = Path(name) / target
            subprocess.run(["gh", "run", "download", run_id, "--repo", repo, "--name", f"inv15-{target}", "--dir", str(directory)],
                           check=True, capture_output=True, text=True, timeout=API_SECONDS)
            validate_record(json.loads((directory / f"{target}.json").read_text(encoding="utf-8")), target, commit)


def main():
    run_id = os.environ["SOURCE_RUN_ID"]
    if not re.fullmatch(r"[1-9][0-9]*", run_id):
        raise ValueError("the source run id must be a positive integer")
    repo = os.environ["GITHUB_REPOSITORY"]
    response = subprocess.run(["gh", "api", f"repos/{repo}/actions/runs/{run_id}"], check=True,
                              capture_output=True, text=True, timeout=API_SECONDS)
    check_source(run_id, json.loads(response.stdout), os.environ["COMMIT"])
    artifacts = subprocess.run(["gh", "api", "--paginate", "--slurp", f"repos/{repo}/actions/runs/{run_id}/artifacts?per_page=100"],
                               check=True, capture_output=True, text=True, timeout=API_SECONDS)
    pages = json.loads(artifacts.stdout)
    require_observations(run_id, repo, os.environ["COMMIT"], pages)
    frontend = has_frontend(pages)
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            file.write(f"frontend={str(frontend).lower()}\n")
    print(f"Reusing native inputs from completed release run {run_id}; shared frontend available: {frontend}")


if __name__ == "__main__":
    main()
