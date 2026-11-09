#!/usr/bin/env python3
"""Build a recipe-verified image on its native processor and publish only an approved, unused tag."""

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess

from release_ci import wait as wait_for_ci
from release_gate import command


def main():
    if os.environ.get("REHEARSAL") == "true" or os.environ.get("WARM_CACHES") == "true":
        raise ValueError("rehearsal and cache-only runs cannot publish images")
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", required=True, choices=["x86_64", "aarch64"])
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if platform.machine() != args.arch:
        raise ValueError("the image must be built on its native processor")
    version, tag = os.environ["ORCH_RELEASE_VERSION"], os.environ["ORCH_RELEASE_TAG"]
    repo = os.environ["GITHUB_REPOSITORY"].lower()
    reference = f"ghcr.io/{repo.split('/')[0]}/orch-env-base:{version}-{args.arch}"
    present = subprocess.run(["docker", "buildx", "imagetools", "inspect", reference], capture_output=True)
    if present.returncode == 0:
        raise ValueError(f"immutable image tag already exists: {reference}")
    subprocess.run(["cargo", "xtask", "image", "--version", version, "--fresh"], check=True)
    subprocess.run(["docker", "tag", f"orch-env-base:{version}", reference], check=True)
    commit = command("git", "rev-parse", "HEAD")
    # Building may outlast a CI rerun or a tag change. Recheck immediately before the first push.
    wait_for_ci(tag, commit, os.environ["GITHUB_REPOSITORY"], once=True)
    if json.loads(command("gh", "api", f"repos/{os.environ['GITHUB_REPOSITORY']}/commits/{tag}"))["sha"] != commit:
        raise ValueError("the tag no longer names the verified commit; no image published")
    subprocess.run(["docker", "login", "ghcr.io", "--username", os.environ["GITHUB_ACTOR"], "--password-stdin"],
                   input=os.environ["GH_TOKEN"], text=True, check=True)
    subprocess.run(["docker", "push", reference], check=True)
    index = json.loads(subprocess.run(["docker", "buildx", "imagetools", "inspect", reference, "--format", "{{json .Manifest}}"],
                                     check=True, capture_output=True, text=True).stdout)
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "image.json").write_text(json.dumps({"tag": tag, "commit": commit, "arch": args.arch,
        "image": {"name": "orch-env-base", "reference": reference, "digest": index["digest"]}}, indent=2) + "\n")


if __name__ == "__main__":
    main()
