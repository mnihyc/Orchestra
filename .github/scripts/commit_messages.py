"""Check the subject and body of each commit introduced by a push or pull request."""

import json
import os
from pathlib import Path
import re
import subprocess
import sys


# Keep the vocabulary in one place. Internal work identifiers are uppercase compounds.
PRIVATE_WORDS = re.compile(
    r"\b(?:lanes?|batch(?:es)?|trials?|assembly|assemblies|assembled|integrators?|registers?|the\s+owner)\b"
    r"|\b[CD]-?\d+\b", re.IGNORECASE)
WORK_ID = re.compile(r"\b[A-Z][A-Z0-9]+(?:-[A-Z][A-Z0-9]*)+\b")
# Standard formats and protocols also have uppercase hyphenated names.
PUBLIC_FORMATS = {"AES-GCM", "JSON-RPC", "JSON-LD", "USB-C", "HTTP-POST", "HTTP-GET"}
GENERATED_MERGE = re.compile(r"^Merge (?:remote-tracking branch |branch |commit ['\"]?[0-9a-f]{7,40}\b)")


def first_violation(message):
    for pattern in (PRIVATE_WORDS, WORK_ID, GENERATED_MERGE):
        for hit in pattern.finditer(message):
            if pattern is not WORK_ID or hit.group() not in PUBLIC_FORMATS:
                return hit.group()
    return None


def findings(root, base, head):
    args = ["git", "-C", str(root), "log", "-z", "--format=%H%x00%B", head]
    if base:
        args.append("^" + base)
    raw = subprocess.check_output([*args, "--"]).decode("utf-8", errors="replace")
    fields = iter(raw.split("\0")[:-1])
    return [(sha, word) for sha, message in zip(fields, fields, strict=True)
            if (word := first_violation(message))]


def pushed_range(event, head):
    if "pull_request" in event:
        pr = event["pull_request"]
        return pr["base"]["sha"], pr["head"]["sha"]
    before = event.get("before")
    if before and before != "0" * 40:
        return before, head
    default = event["repository"]["default_branch"]
    if event.get("created") and event.get("ref") == f"refs/heads/{default}":
        return None, head
    return f"refs/remotes/origin/{default}", head


def main():
    event = json.loads(Path(os.environ["GITHUB_EVENT_PATH"]).read_text(encoding="utf-8"))
    base, head = pushed_range(event, os.environ["GITHUB_SHA"])
    # After a force push the previous tip may be reachable only by its object ID.
    if base and re.fullmatch(r"[0-9a-f]{40}", base):
        if subprocess.run(["git", "cat-file", "-e", base + "^{commit}"]).returncode:
            subprocess.run(["git", "fetch", "--no-tags", "origin", base], check=True)
    violations = findings(Path.cwd(), base, head)
    for sha, word in violations:
        print(f"{sha}: describe the product change in the commit message (matched {word!r}).")
    return int(bool(violations))


if __name__ == "__main__":
    sys.exit(main())
