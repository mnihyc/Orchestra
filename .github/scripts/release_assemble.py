#!/usr/bin/env python3
"""Join both server architectures, refusing mixed identities or differing common assets."""

import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def assemble(inputs, out):
    manifests = sorted(inputs.glob("*/version.json"))
    if len(manifests) != 2:
        raise ValueError("a release requires both Linux server architectures")
    listings = [json.loads(path.read_text()) for path in manifests]
    first = listings[0]
    for listing in listings[1:]:
        for key in ("project", "schema_version", "tag", "version", "commit", "link_protocol", "connector_minimum", "harness_builds", "images"):
            if listing[key] != first[key]:
                raise ValueError(f"the architecture manifests differ in {key}")
    if out.exists() and any(out.iterdir()):
        raise ValueError("the output directory must be empty")
    out.mkdir(parents=True, exist_ok=True)
    for path in manifests:
        for asset in json.loads(path.read_text())["assets"]:
            source, target = path.parent / asset["name"], out / asset["name"]
            if source.name != asset["name"] or source.name.startswith("."):
                raise ValueError("an asset is not a plain file name")
            if target.exists():
                if hashlib.file_digest(source.open("rb"), "sha256").digest() != hashlib.file_digest(target.open("rb"), "sha256").digest():
                    raise ValueError(f"the two builds disagree on shared asset {target.name}")
            else:
                shutil.copyfile(source, target)
    (out / "version.json").write_text(json.dumps(first, indent=2) + "\n")
    return first


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--base-url", required=True)
    args = parser.parse_args()
    assemble(args.inputs, args.out)
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.out.resolve()), "--manifest-only", "--base-url", args.base_url], check=True)
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.out.resolve()), "--check", "--require-all-connectors"], check=True)


if __name__ == "__main__":
    main()
