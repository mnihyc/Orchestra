#!/usr/bin/env python3
"""Join approved native image digests, check the final listing and create an immutable draft release."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess


def run(*command):
    return subprocess.run(command, check=True, text=True, capture_output=True).stdout.strip()


def check_existing_draft(draft, publish_images, tagged_commit, commit):
    """Only complete a draft whose still-existing tag resolves to this release's commit."""
    if not draft["draft"] or not publish_images or tagged_commit != commit:
        raise ValueError("an existing release is never replaced; only the approved image facts of this exact draft may be completed")


def check_draft_assets(draft, directory):
    """Require the same asset set and unchanged bytes, apart from the manifest being completed."""
    remote = {asset["name"]: asset for asset in draft["assets"]}
    local = {asset.name: asset for asset in directory.iterdir()}
    if set(remote) != set(local):
        raise ValueError("the draft has a different asset set")
    for name, asset in local.items():
        if name == "version.json":
            continue
        with asset.open("rb") as file:
            digest = "sha256:" + hashlib.file_digest(file, "sha256").hexdigest()
        if remote[name].get("digest") != digest:
            raise ValueError(f"draft asset differs: {name}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--release", type=Path, required=True)
    parser.add_argument("--repo", required=True)
    parser.add_argument("--images", type=Path, required=True)
    args = parser.parse_args()
    listing_path = args.release / "version.json"
    listing = json.loads(listing_path.read_text())
    tag, version, commit = (listing[key] for key in ("tag", "version", "commit"))
    publish_images = os.environ.get("PUBLISH_IMAGES") == "true"
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.release.resolve()), "--check", "--require-all-connectors"], check=True)
    existing = subprocess.run(["gh", "api", f"repos/{args.repo}/releases/tags/{tag}"], capture_output=True, text=True)
    if existing.returncode != 0 and "(HTTP 404)" not in existing.stderr:
        raise ValueError("the existing release could not be checked")
    draft = None
    if existing.returncode == 0:
        draft = json.loads(existing.stdout)
        # target_commitish is for creating a missing tag; it is not the commit of an existing tag.
        tagged_commit = json.loads(run("gh", "api", f"repos/{args.repo}/commits/{tag}"))["sha"]
        check_existing_draft(draft, publish_images, tagged_commit, commit)
        check_draft_assets(draft, args.release)
    if publish_images:
        reference = f"ghcr.io/{args.repo.lower().split('/')[0]}/orch-env-base:{version}"
        # The protected release-images environment is approved before this job receives its token.
        subprocess.run(["docker", "login", "ghcr.io", "--username", os.environ["GITHUB_ACTOR"], "--password-stdin"],
                       input=os.environ["GH_TOKEN"], text=True, check=True)
        records = [json.loads(path.read_text()) for path in sorted(args.images.glob("*/image.json"))]
        if {record["arch"] for record in records} != {"x86_64", "aarch64"} or len(records) != 2:
            raise ValueError("both approved native image builds are required")
        if any(record["tag"] != tag or record["commit"] != commit for record in records):
            raise ValueError("an image belongs to another release or commit")
        if subprocess.run(["docker", "buildx", "imagetools", "inspect", reference], capture_output=True).returncode == 0:
            raise ValueError(f"immutable image index already exists: {reference}")
        subprocess.run(["docker", "buildx", "imagetools", "create", "--tag", reference,
                        *[f"{record['image']['reference']}@{record['image']['digest']}" for record in records]], check=True)
        digest = json.loads(run("docker", "buildx", "imagetools", "inspect", reference, "--format", "{{json .Manifest}}"))["digest"]
        listing["images"].append({"name": "orch-env-base", "reference": reference, "digest": digest})
        listing_path.write_text(json.dumps(listing, indent=2) + "\n")
    subprocess.run(["cargo", "xtask", "package", "--out", str(args.release.resolve()), "--check", "--require-all-connectors"], check=True)
    notes = Path(".github/release-notes") / f"{tag}.md"
    if not publish_images:
        # This draft is reviewable but is not a claim that the default image has been published.
        staged = args.release.parent / "draft-notes.md"
        staged.write_text(notes.read_text() + "\n\nDefault environment image publication is pending operator approval. This draft must not be published until its image digest is recorded.\n")
        notes = staged
    if draft is not None:
        subprocess.run(["gh", "release", "upload", tag, str(listing_path), "--repo", args.repo, "--clobber"], check=True)
        subprocess.run(["gh", "release", "edit", tag, "--repo", args.repo, "--notes-file", str(notes)], check=True)
    else:
        subprocess.run(["gh", "release", "create", tag, "--repo", args.repo, "--draft", "--verify-tag", "--target", commit,
                        "--title", f"Orch {version}", "--notes-file", str(notes), *map(str, sorted(args.release.iterdir()))], check=True)
    print(f"Draft {tag} created; publication remains an operator decision")


if __name__ == "__main__":
    main()
