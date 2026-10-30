#!/usr/bin/env python3
"""Build the browser application once and record every file and its release identity."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess


NODE_IMAGE = "node:24.16.0-bookworm@sha256:40ad9f3064e67d6860b4bc3fe1880b2953934fd6320ada990e45fe0efa6badd7"  # node:24.16.0-bookworm


def run_npm(root, command, variables=()):
    """Run the pinned Node image with a persistent npm download cache."""
    node = (root / "frontend/.nvmrc").read_text().strip().removeprefix("v")
    if NODE_IMAGE.split("@", 1)[0] != f"node:{node}-bookworm":
        raise ValueError("the pinned packaging image must match frontend/.nvmrc")
    npm_cache = root / ".tmp/npm-cache"
    npm_cache.mkdir(parents=True, exist_ok=True)
    subprocess.run(["docker", "run", "--rm", "--user", f"{os.getuid()}:{os.getgid()}",
                    "--env", "npm_config_cache=/npm-cache", *variables, "--volume", f"{npm_cache}:/npm-cache",
                    "--volume", f"{root}:/work", "--workdir", "/work/frontend",
                    NODE_IMAGE, "sh", "-ec", command], check=True)


def files_in(dist):
    """Record the size and SHA-256 of each regular file, refusing links and special files."""
    if not dist.is_dir() or dist.is_symlink() or not (dist / "index.html").is_file():
        raise ValueError("the frontend build has no index.html")
    files = {}
    for path in sorted(dist.rglob("*")):
        if path.is_symlink():
            raise ValueError(f"the frontend contains a symbolic link: {path}")
        if path.is_dir():
            continue
        if not path.is_file():
            raise ValueError(f"the frontend contains a non-regular file: {path}")
        with path.open("rb") as source:
            digest = hashlib.file_digest(source, "sha256").hexdigest()
        files[path.relative_to(dist).as_posix()] = {"size": path.stat().st_size, "sha256": digest}
    return files


def verify(directory, version, tag, commit):
    """Refuse a different release, missing file, changed byte or extra frontend file."""
    listing = json.loads((directory / "frontend.json").read_text())
    if (listing.get("version"), listing.get("tag"), listing.get("commit")) != (version, tag, commit):
        raise ValueError("the frontend belongs to another release or commit")
    if listing.get("files") != files_in(directory / "dist"):
        raise ValueError("the frontend files do not match their manifest")
    print(f"Verified frontend for {tag} at {commit}: {len(listing['files'])} files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--out", type=Path, required=True)
    args = parser.parse_args()
    root = Path.cwd()
    identity = json.loads(subprocess.run(["cargo", "xtask", "identity", "frontend"], check=True, capture_output=True, text=True).stdout)
    if (identity["version"], identity["tag"]) != (os.environ["ORCH_RELEASE_VERSION"], os.environ["ORCH_RELEASE_TAG"]):
        raise ValueError("the frontend build identity differs from the release")
    commit = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
    variables = ["--env", f"ORCH_RELEASE_VERSION={identity['version']}", "--env", f"ORCH_RELEASE_TAG={identity['tag']}",
                 "--env", f"ORCH_FRONTEND_BUILD_NUMBER={identity['build_number']}"]
    run_npm(root, "npm ci && npm run build", variables)
    dist = root / "frontend/dist"
    listing = {"version": identity["version"], "tag": identity["tag"], "commit": commit, "files": files_in(dist)}
    args.out.mkdir(parents=True, exist_ok=False)
    shutil.copytree(dist, args.out / "dist")
    (args.out / "frontend.json").write_text(json.dumps(listing, indent=2) + "\n")
    verify(args.out, identity["version"], identity["tag"], commit)


if __name__ == "__main__":
    main()
