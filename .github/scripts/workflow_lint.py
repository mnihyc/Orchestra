#!/usr/bin/env python3
"""Lint the pinned workflow files in an isolated tooling image."""

from pathlib import Path
import subprocess

# 1.7.8 adds the Intel macOS 15 runner labels: https://github.com/rhysd/actionlint/releases/tag/v1.7.8
# The maintainer's multi-platform index; the tooling cannot contact the network while linting.
IMAGE = "rhysd/actionlint:1.7.8@sha256:96d4a8c87dbbfb3bdd324f8fdc285fc3df5261e2decc619a4dd7e8ee52bbfd46"

if __name__ == "__main__":
    root = Path.cwd().resolve()
    subprocess.run(["docker", "run", "--rm", "--network", "none", "--volume", f"{root}:/work:ro", "--workdir", "/work", IMAGE,
                    "-color", *[str(path.relative_to(root)) for path in sorted((root / ".github/workflows").glob("*.y*ml"))]], check=True)
