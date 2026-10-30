#!/usr/bin/env python3
"""Retain dependency inputs beside a released-stack experiment, without claiming a check ran."""

import argparse
import hashlib
import json
import shutil
from pathlib import Path

INPUTS = ("deny.toml", "Cargo.lock", "frontend/package-lock.json")


def verified_result(record, commit, inputs):
    """A previous green check counts only for the exact clean candidate and dependency inputs."""
    candidate = record.get("candidate", {})
    return (
        candidate.get("clean") is True
        and candidate.get("commit") == commit
        and candidate.get("dependency_inputs") == inputs
        and any(step.get("step") == "deny" and step.get("exit") == 0 for step in record.get("steps", []))
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--proof", type=Path)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    inputs = {}
    for name in INPUTS:
        path = args.repo / name
        inputs[name] = hashlib.sha256(path.read_bytes()).hexdigest()
        shutil.copyfile(path, args.output / Path(name).name)
    commit = json.loads(args.manifest.read_text())["commit"]
    record = {
        "candidate_commit": commit,
        "dependency_inputs": inputs,
        "candidate_check": "unperformed",
        "reported_round_check": {
            "observer": "integrator",
            "tool": "cargo-deny 0.20.2",
            "bans": "ok", "licenses": "ok", "sources": "ok",
            "advisories": "not reported by this check",
            "candidate_commit": None,
            "note": "Round copy identity was not supplied with this report; it is not candidate attestation.",
        },
    }
    if args.proof:
        proof = json.loads(args.proof.read_text())
        accepted = verified_result(proof, commit, inputs)
        record["candidate_check"] = "passed" if accepted else "unmatched proof; unperformed"
        shutil.copyfile(args.proof, args.output / "release-proof.json")
        log = args.proof.parent / "deny.log"
        if log.is_file():
            shutil.copyfile(log, args.output / "deny.log")
        record["cargo_deny"] = proof.get("candidate", {}).get("cargo_deny", "unknown")
    (args.output / "result.json").write_text(json.dumps(record, indent=2) + "\n")


if __name__ == "__main__":
    main()
