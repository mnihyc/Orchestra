#!/usr/bin/env python3
"""Package a Linux server with verified native and frontend inputs and check its archives."""

import argparse
import os
from pathlib import Path
import shutil
import subprocess

from release_frontend import files_in, run_npm, verify


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--frontend", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    root = Path.cwd()
    commit = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    verify(args.frontend, os.environ["ORCH_RELEASE_VERSION"], os.environ["ORCH_RELEASE_TAG"], commit)
    dist = root / "frontend/dist"
    if dist.exists():
        if files_in(dist) != files_in(args.frontend / "dist"):
            raise ValueError("the locally built frontend differs from the verified input")
    else:
        shutil.copytree(args.frontend / "dist", dist)
    # The notice generator reads the installed files of the packages in the build's inventory.
    run_npm(root, "npm ci")
    command = ["cargo", "xtask", "package", "--out", str(args.out.resolve()), "--require-all-connectors", "--reuse-frontend", "--skip-image"]
    dirs = sorted(path.parent for path in args.inputs.glob("*/version.json"))
    if not dirs:
        raise ValueError("no native connector manifests downloaded")
    for directory in dirs:
        command.extend(["--connectors", str(directory.resolve())])
    subprocess.run(command, check=True)
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.out.resolve()), "--check", "--require-all-connectors"], check=True)


if __name__ == "__main__":
    main()
