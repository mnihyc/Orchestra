#!/usr/bin/env python3
"""Verify release assets and publish them after the tagged commit's CI is green."""

import argparse
from functools import cmp_to_key
import hashlib
import json
import os
from pathlib import Path
import subprocess

from release_ci import wait as wait_for_ci
from release_gate import VERSION, compare, version as parse_version

# Concurrent publishers each re-read after a repair; five attempts bound a persistent conflict.
LATEST_ATTEMPTS = 5

def run(*command):
    return subprocess.run(command, check=True, text=True, capture_output=True).stdout.strip()


def check_existing_release(release, tagged_commit, commit):
    """Never use a release whose tag now names another commit."""
    if tagged_commit != commit:
        raise ValueError("the existing release tag does not name this commit")


def check_release_assets(release, directory, *, partial=False, image_update=False):
    """Match remote bytes; a draft may have missing uploads and images may update only the manifest."""
    remote = {asset["name"]: asset for asset in release["assets"]}
    local = {asset.name: asset for asset in directory.iterdir()}
    if not set(remote) <= set(local) or (not partial and set(remote) != set(local)):
        raise ValueError("the release has a different asset set")
    for name in remote:
        if name == "version.json" and image_update:
            continue
        asset = local[name]
        with asset.open("rb") as file:
            digest = "sha256:" + hashlib.file_digest(file, "sha256").hexdigest()
        if remote[name].get("digest") != digest:
            raise ValueError(f"release asset differs: {name}")
    return sorted(set(local) - set(remote))


def is_latest(tag, releases):
    """Only the highest published stable version is the Latest release."""
    if parse_version(tag)[1]:
        return False
    return not any(not release.get("draft") and not release.get("prerelease")
                   and VERSION.fullmatch(release.get("tag_name", ""))
                   and compare(release["tag_name"], tag) > 0 for release in releases)


def reconcile_latest(repo):
    """Repair a concurrent publication that marked an older release Latest."""
    for _ in range(LATEST_ATTEMPTS):
        pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{repo}/releases?per_page=100"))
        tags = [release["tag_name"] for page in pages for release in page
                if not release.get("draft") and not release.get("prerelease")
                and VERSION.fullmatch(release.get("tag_name", ""))]
        if not tags:
            return
        newest = max(tags, key=cmp_to_key(compare))
        current = json.loads(run("gh", "api", f"repos/{repo}/releases/latest"))["tag_name"]
        if current == newest:
            return
        subprocess.run(["gh", "release", "edit", newest, "--repo", repo, "--latest"], check=True)
    raise ValueError("could not settle Latest after concurrent publications; rerun the release workflow")


def check_published_manifest(release, directory, repo):
    """Allow separately approved image facts while checking the exact remote bytes."""
    asset = next(item for item in release["assets"] if item["name"] == "version.json")
    response = subprocess.run(["gh", "api", "-H", "Accept: application/octet-stream",
                               f"repos/{repo}/releases/assets/{asset['id']}"],
                              check=True, capture_output=True)
    if "sha256:" + hashlib.sha256(response.stdout).hexdigest() != asset.get("digest"):
        raise ValueError("the published manifest digest does not match its bytes")
    remote = json.loads(response.stdout)
    local = json.loads((directory / "version.json").read_text())
    remote_images, local_images = remote.pop("images"), local.pop("images")
    if remote != local or any(image not in remote_images for image in local_images):
        raise ValueError("the published manifest differs from the verified release")


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
    release = None
    if existing.returncode == 0:
        release = json.loads(existing.stdout)
        # target_commitish is for creating a missing tag; it is not the commit of an existing tag.
        tagged_commit = json.loads(run("gh", "api", f"repos/{args.repo}/commits/{tag}"))["sha"]
        check_existing_release(release, tagged_commit, commit)
        check_release_assets(release, args.release, partial=release["draft"],
                             image_update=publish_images or not release["draft"])
        if not release["draft"] and not publish_images:
            check_published_manifest(release, args.release, args.repo)
            wait_for_ci(tag, commit, args.repo, once=True)
            reconcile_latest(args.repo)
            print(f"Release {tag} already exists with the same verified assets")
            return
    if publish_images:
        wait_for_ci(tag, commit, args.repo, once=True)
        tagged_commit = json.loads(run("gh", "api", f"repos/{args.repo}/commits/{tag}"))["sha"]
        if tagged_commit != commit:
            raise ValueError("the tag no longer names the verified commit; no image published")
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
    wait_for_ci(tag, commit, args.repo, once=True)
    tagged_commit = json.loads(run("gh", "api", f"repos/{args.repo}/commits/{tag}"))["sha"]
    if tagged_commit != commit:
        raise ValueError("the tag no longer names the verified commit; nothing published")
    pages = json.loads(run("gh", "api", "--paginate", "--slurp", f"repos/{args.repo}/releases?per_page=100"))
    latest = is_latest(tag, [item for page in pages for item in page])
    if release is not None:
        missing = check_release_assets(release, args.release, partial=release["draft"], image_update=publish_images)
        if missing:
            subprocess.run(["gh", "release", "upload", tag, *[str(args.release / name) for name in missing], "--repo", args.repo], check=True)
        if publish_images and "version.json" not in missing:
            subprocess.run(["gh", "release", "upload", tag, str(listing_path), "--repo", args.repo, "--clobber"], check=True)
        if release["draft"]:
            subprocess.run(["gh", "release", "edit", tag, "--repo", args.repo, "--draft=false", "--verify-tag",
                            f"--latest={str(latest).lower()}", "--notes-file", str(notes)], check=True)
    else:
        prerelease = ["--prerelease"] if parse_version(tag)[1] else []
        subprocess.run(["gh", "release", "create", tag, "--repo", args.repo, "--verify-tag", "--target", commit,
                        f"--latest={str(latest).lower()}", *prerelease, "--title", f"Orch {version}",
                        "--notes-file", str(notes), *map(str, sorted(args.release.iterdir()))], check=True)
    reconcile_latest(args.repo)
    print(f"Release {tag} published with verified assets")


if __name__ == "__main__":
    main()
