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
    servers = [
        [asset["name"] for asset in listing["assets"] if asset["name"].startswith("orch-server-linux-") and asset["name"].endswith(".zip")]
        for listing in listings
    ]
    if any(len(names) != 1 for names in servers) or {names[0] for names in servers} != {
        "orch-server-linux-x86_64.zip", "orch-server-linux-aarch64.zip"
    }:
        raise ValueError("the inputs must each contain a different Linux server architecture")
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
                with source.open("rb") as source_file, target.open("rb") as target_file:
                    same = hashlib.file_digest(source_file, "sha256").digest() == hashlib.file_digest(target_file, "sha256").digest()
                if not same:
                    raise ValueError(f"the two builds disagree on shared asset {target.name}")
            else:
                shutil.copyfile(source, target)
    (out / "version.json").write_text(json.dumps(first, indent=2) + "\n")
    return first


def assemble_checked(inputs, out, base_url):
    manifests = sorted(inputs.glob("*/version.json"))
    if len(manifests) != 2:
        raise ValueError("a release requires both Linux server architectures")
    # Verify downloaded artifacts against their original hashes before the combined manifest is rewritten.
    for manifest in manifests:
        subprocess.run(["cargo", "xtask", "package", "--out", str(manifest.parent.resolve()), "--check", "--require-all-connectors"], check=True)
    assemble(inputs, out)
    subprocess.run(["cargo", "xtask", "package", "--out", str(out.resolve()), "--manifest-only", "--base-url", base_url], check=True)
    subprocess.run(["cargo", "xtask", "package", "--out", str(out.resolve()), "--check", "--require-all-connectors"], check=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--inputs", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--base-url", required=True)
    args = parser.parse_args()
    assemble_checked(args.inputs, args.out, args.base_url)


if __name__ == "__main__":
    main()
