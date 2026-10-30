#!/usr/bin/env python3
"""Copy an already verified release and select a local registry image without altering payload bytes."""

import argparse
import json
from pathlib import Path
import re
import shutil


def prepare(source, out, reference, digest):
    listing = json.loads((source / "version.json").read_text())
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", digest):
        raise ValueError("the registry digest is not a SHA-256 content address")
    if out.exists() and any(out.iterdir()):
        raise ValueError("the evidence release directory must be empty")
    for asset in listing["assets"]:
        name = asset["name"]
        if name != Path(name).name or "/" in name or "\\" in name or name.startswith("."):
            raise ValueError("a release asset is not a plain file name")
    out.mkdir(parents=True, exist_ok=True)
    for asset in listing["assets"]:
        shutil.copyfile(source / asset["name"], out / asset["name"])
        asset["download_url"] = asset["name"]
    listing["images"] = [image for image in listing["images"] if image["name"] != "orch-env-base"]
    listing["images"].append({"name": "orch-env-base", "reference": reference, "digest": digest})
    (out / "version.json").write_text(json.dumps(listing, indent=2) + "\n")
    return listing


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--reference", required=True)
    parser.add_argument("--digest", required=True)
    args = parser.parse_args()
    prepare(args.source, args.out, args.reference, args.digest)
