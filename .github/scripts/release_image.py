#!/usr/bin/env python3
"""Build a recipe-verified image on its native processor and publish only an approved, unused tag."""

import argparse
import json
import os
from pathlib import Path
import platform
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--arch", required=True, choices=["x86_64", "aarch64"])
    parser.add_argument("--out", required=True, type=Path)
    args = parser.parse_args()
    if platform.machine() != args.arch:
        raise ValueError("the image must be built on its native processor")
    version, tag = os.environ["ORCH_RELEASE_VERSION"], os.environ["ORCH_RELEASE_TAG"]
    repo = os.environ["GITHUB_REPOSITORY"].lower()
    reference = f"ghcr.io/{repo.split('/')[0]}/orch-env-base:{version}-{args.arch}"
    subprocess.run(["docker", "login", "ghcr.io", "--username", os.environ["GITHUB_ACTOR"], "--password-stdin"],
                   input=os.environ["GH_TOKEN"], text=True, check=True)
    present = subprocess.run(["docker", "buildx", "imagetools", "inspect", reference], capture_output=True)
    if present.returncode == 0:
        raise ValueError(f"immutable image tag already exists: {reference}")
    subprocess.run(["cargo", "xtask", "image", "--version", version, "--fresh"], check=True)
    subprocess.run(["docker", "tag", f"orch-env-base:{version}", reference], check=True)
    subprocess.run(["docker", "push", reference], check=True)
    index = json.loads(subprocess.run(["docker", "buildx", "imagetools", "inspect", reference, "--format", "{{json .Manifest}}"],
                                     check=True, capture_output=True, text=True).stdout)
    commit = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    args.out.mkdir(parents=True, exist_ok=True)
    (args.out / "image.json").write_text(json.dumps({"tag": tag, "commit": commit, "arch": args.arch,
        "image": {"name": "orch-env-base", "reference": reference, "digest": index["digest"]}}, indent=2) + "\n")


if __name__ == "__main__":
    main()
