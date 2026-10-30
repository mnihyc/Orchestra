#!/usr/bin/env python3
"""Identify the default branch build used solely to warm release caches."""

import os
import subprocess

from release_gate import version


def check_request(tag, event, ref, default_branch, source_run, publish_images):
    """A cache warm-up cannot create a release or consume another run."""
    version(tag)
    if event != "workflow_dispatch" or not default_branch or ref != f"refs/heads/{default_branch}":
        raise ValueError("cache warm-up must be dispatched from the default branch")
    if source_run or publish_images:
        raise ValueError("cache warm-up cannot repackage or publish images")


def main():
    tag = os.environ["RELEASE_TAG"]
    check_request(tag, os.environ["GITHUB_EVENT_NAME"], os.environ["GITHUB_REF"], os.environ["DEFAULT_BRANCH"],
                  os.environ.get("SOURCE_RUN_ID", ""), os.environ.get("PUBLISH_IMAGES") == "true")
    commit = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    if commit != os.environ["GITHUB_SHA"]:
        raise ValueError("cache warm-up must build the dispatched branch commit")
    status = subprocess.run(["git", "status", "--porcelain"], check=True, capture_output=True, text=True).stdout
    if status:
        raise ValueError("cache warm-up needs a clean checkout")
    with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
        output.write(f"tag={tag}\nversion={tag[1:]}\ncommit={commit}\n")
    print(f"Warming release caches for {tag} at {commit}; no draft or images will be published")


if __name__ == "__main__":
    main()
