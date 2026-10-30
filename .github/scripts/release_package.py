#!/usr/bin/env python3
"""Package a Linux server with every verified native connector input and check its archives."""

import argparse
import json
import os
from pathlib import Path
import subprocess


NODE_IMAGE = "node:24.16.0-bookworm@sha256:40ad9f3064e67d6860b4bc3fe1880b2953934fd6320ada990e45fe0efa6badd7"  # node:24.16.0-bookworm

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    root = Path.cwd()
    # Match the exact repository Node version, without installing runner packages.
    node = (root / "frontend/.nvmrc").read_text().strip().removeprefix("v")
    if NODE_IMAGE.split("@", 1)[0] != f"node:{node}-bookworm":
        raise ValueError("the pinned packaging image must match frontend/.nvmrc")
    identity = json.loads(subprocess.run(["cargo", "xtask", "identity", "frontend"], check=True, capture_output=True, text=True).stdout)
    variables = ["--env", f"ORCH_RELEASE_VERSION={identity['version']}", "--env", f"ORCH_RELEASE_TAG={identity['tag']}",
                 "--env", f"ORCH_FRONTEND_BUILD_NUMBER={identity['build_number']}"]
    subprocess.run(["docker", "run", "--rm", "--user", f"{os.getuid()}:{os.getgid()}",
                    "--env", "npm_config_cache=/tmp/npm-cache", *variables, "--volume", f"{root}:/work", "--workdir", "/work/frontend",
                    NODE_IMAGE, "sh", "-ec", "npm ci && npm run build"], check=True)
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
