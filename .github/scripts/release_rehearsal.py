#!/usr/bin/env python3
"""Pin a branch snapshot for complete release assembly without creating a tag or publishing."""

import os
import re
import subprocess

from release_gate import API_SECONDS, command, notes_path, version


def check_request(tag, event, ref, dispatched, head, existing_tag, *, warm=False, images=False):
    version(tag)
    if event != "workflow_dispatch" or not ref.startswith("refs/heads/"):
        raise ValueError("a rehearsal must be dispatched from a snapshot branch")
    if not re.fullmatch(r"[0-9a-f]{40}", head) or dispatched != head:
        raise ValueError("a rehearsal must build the dispatched snapshot SHA")
    if warm or images:
        raise ValueError("a rehearsal cannot warm another branch or publish images")
    if existing_tag is not None and existing_tag != head:
        raise ValueError("the requested version tag already names another commit; choose an unused version")


def main():
    tag = os.environ["RELEASE_TAG"]
    version(tag)
    head = command("git", "rev-parse", "HEAD")
    existing = subprocess.run(["git", "rev-parse", "--verify", "--quiet", f"refs/tags/{tag}^{{commit}}"],
                              capture_output=True, text=True, timeout=API_SECONDS)
    if existing.returncode not in (0, 1):
        raise ValueError("the candidate tag could not be checked")
    ref = os.environ["GITHUB_REF"]
    check_request(tag, os.environ["GITHUB_EVENT_NAME"], ref, os.environ["GITHUB_SHA"], head,
                  existing.stdout.strip() if existing.returncode == 0 else None,
                  warm=os.environ.get("WARM_CACHES") == "true", images=os.environ.get("PUBLISH_IMAGES") == "true")
    if command("git", "status", "--porcelain"):
        raise ValueError("a rehearsal needs a clean snapshot checkout")
    notes_path(tag)
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"tag={tag}\nversion={tag[1:]}\ncommit={head}\nci_ref={ref.removeprefix('refs/heads/')}\n")
    print(f"Rehearsing {tag} at {head}; no tag, release or image publication")


if __name__ == "__main__":
    main()
