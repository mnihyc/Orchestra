#!/usr/bin/env python3
"""Observe the packaged candidate on its native runner and retain two required platform records."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import math
import os
from pathlib import Path
import platform
import re
import subprocess
import sys
import time

from release_contract import catalogue

ROOT = Path(__file__).resolve().parents[2]
NAMES = {"connected_program_attachment", "connector_credential_lifecycle"}
CASE = "platform_observations::observe_candidate_platform"
# Includes a first native debug test link plus four existing 60-second process deadlines.
# A runner can override this budget; it does not change any connector policy.
DEFAULT_TIMEOUT = 1800


def identification():
    return {"system": platform.system(), "release": platform.release(),
            "version": platform.version(), "machine": platform.machine()}


def validate_record(record, target, commit):
    if not isinstance(record, dict):
        raise ValueError("the observation must be a JSON object")
    if record.get("format") != 1 or record.get("target") != target or record.get("commit") != commit:
        raise ValueError("the observation is not for this exact candidate and native target")
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("the observation needs a full candidate commit")
    if record.get("status") != "passed":
        raise ValueError("the native platform observation failed: " + str(record.get("reason", "incomplete")))
    native_os = "windows" if "windows" in target else "macos" if "darwin" in target else "linux"
    native_arch = target.split("-", 1)[0]
    connector = record.get("connector", {})
    if not isinstance(connector, dict):
        raise ValueError("the observation has no connector identity")
    if not connector.get("identity") or not re.fullmatch(r"[0-9a-f]{64}", connector.get("sha256", "")):
        raise ValueError("the observation has no measured candidate connector identity and hash")
    own_platform = record.get("platform", {})
    if not isinstance(own_platform, dict):
        raise ValueError("the observation has no native platform identification")
    if any(not own_platform.get(key) for key in ["system", "version", "machine"]):
        raise ValueError("the observation has no platform identification")
    if (record.get("native_os"), record.get("native_arch")) != (native_os, native_arch):
        raise ValueError("the observation ran on another operating system or processor")
    rows = record.get("observations", [])
    if not isinstance(rows, list) or len(rows) != len(NAMES) or any(not isinstance(row, dict) for row in rows) or {row.get("name") for row in rows} != NAMES:
        raise ValueError("both distinct native observations are required")
    for row in rows:
        if row.get("status") != "passed" or row.get("commit") != commit or row.get("platform") != own_platform:
            raise ValueError("an observation failed or lost its candidate/platform provenance")
        if not isinstance(row.get("elapsed_ms"), (float, int)) or isinstance(row["elapsed_ms"], bool) or (isinstance(row["elapsed_ms"], float) and not math.isfinite(row["elapsed_ms"])) or row["elapsed_ms"] < 0:
            raise ValueError("an observation has no finite native elapsed time")
        if not isinstance(row.get("evidence"), list) or not row["evidence"] or any(not isinstance(line, dict) for line in row["evidence"]):
            raise ValueError("an observation has no evidence lines")
    attachment = next(row for row in rows if row["name"] == "connected_program_attachment")
    lifecycle = next(row for row in rows if row["name"] == "connector_credential_lifecycle")
    if not {"enrolled", "scripted_program_described", "scripted_program_started"}.issubset({line.get("event") for line in attachment["evidence"]}):
        raise ValueError("the connected program was not described and started by the candidate")
    events = lifecycle["evidence"]
    if not any(line.get("event") == "session_rejected" and line.get("code") == "revoked" for line in events):
        raise ValueError("the revoked credential was not actually refused")
    renewals = [line for line in events if line.get("event") == "renewal_issued" and line.get("new_public_key") is True]
    ready = {line.get("fingerprint") for line in events if line.get("event") == "session_ready"}
    if not renewals or not any(line.get("fingerprint") in ready and line.get("previous_fingerprint") != line.get("fingerprint") for line in renewals):
        raise ValueError("the automatically renewed key was not accepted for a native session")
    return record


def candidate(root, native, target, commit):
    rows = catalogue(root, "TARGETS")
    selected = next((row for row in rows if row[0] == target), None)
    if selected is None:
        raise ValueError("the observation target is absent from the candidate catalogue")
    name = selected[3]
    manifest = json.loads((native / "version.json").read_text(encoding="utf-8"))
    if manifest.get("commit") != commit:
        raise ValueError("the native build is from another candidate commit")
    asset = next((asset for asset in manifest["assets"] if asset["name"] == name), None)
    binary = native / name
    if asset is None or binary.is_symlink() or not binary.is_file():
        raise ValueError("the packaged candidate connector is missing or linked")
    hasher = hashlib.sha256()
    with binary.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            hasher.update(chunk)
    digest = hasher.hexdigest()
    if asset.get("size") != binary.stat().st_size or asset.get("sha256") != digest:
        raise ValueError("the candidate connector differs from its native build manifest")
    output = subprocess.run([str(binary.resolve()), "version"], check=True, text=True, capture_output=True, timeout=30)
    lines = output.stdout.splitlines()
    if not lines:
        raise ValueError("the candidate version command reported no identity")
    identity = lines[0]
    if not identity.startswith(f"connector {manifest['version']} (build ") or f"[{manifest['tag']}]" not in identity:
        raise ValueError("the running connector reports another release identity")
    return binary.resolve(), {"name": name, "sha256": digest, "identity": identity}


def observe(args):
    began = time.monotonic()
    record = {"format": 1, "target": args.target, "commit": args.commit, "platform": identification(),
              "started_at": datetime.now(timezone.utc).isoformat(), "status": "failed",
              "observations": [{"name": name, "status": "failed", "commit": args.commit,
                                "platform": identification(), "elapsed_ms": None, "evidence": []}
                               for name in sorted(NAMES)]}
    args.out.mkdir(parents=True, exist_ok=True)
    path = args.out / f"{args.target}.json"
    raw = args.out / f"{args.target}.raw.json"
    try:
        if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
            raise ValueError("the observation needs the full candidate commit")
        head = subprocess.run(["git", "-C", str(ROOT), "rev-parse", "HEAD"], check=True, text=True, capture_output=True).stdout.strip()
        if head != args.commit:
            raise ValueError("the observation checkout is not the pinned candidate")
        binary, record["connector"] = candidate(ROOT, args.native.resolve(), args.target, args.commit)
        if raw.exists():
            raise ValueError("a previous native report exists; choose a fresh observation directory")
        env = {**os.environ, "ORCH_PLATFORM_CONNECTOR": str(binary), "ORCH_PLATFORM_REPORT": str(raw.resolve()),
               "ORCH_PLATFORM_PYTHON": sys.executable}
        completed = subprocess.run(["cargo", "test", "--locked", "-p", "orch-connector", "--test", "program",
                                    "--target", args.target, CASE, "--", "--ignored", "--exact", "--nocapture"],
                                   cwd=ROOT, env=env, timeout=args.timeout, check=False)
        if not raw.is_file():
            raise ValueError("the native observe case produced no result (a missing or skipped case is not an observation)")
        measured = json.loads(raw.read_text(encoding="utf-8"))
        rows = measured.get("observations") if isinstance(measured, dict) else None
        if not isinstance(rows, list) or len(rows) != len(NAMES) or any(not isinstance(row, dict) for row in rows) or {row.get("name") for row in rows} != NAMES:
            raise ValueError("the native result has no complete observation records")
        record.update({key: measured.get(key) for key in ["native_os", "native_arch", "observations"]})
        for row in record["observations"]:
            row.update({"commit": args.commit, "platform": record["platform"]})
        if completed.returncode:
            reasons = "; ".join(str(row.get("reason", "native test failed")) for row in record["observations"] if row.get("status") != "passed")
            raise ValueError(reasons or f"native observation exited {completed.returncode}")
        record["status"] = "passed"
        validate_record(record, args.target, args.commit)
    except Exception as error:  # Always retain a failed result before propagating the native failure.
        record.update({"status": "failed", "reason": str(error)})
        if isinstance(record.get("observations"), list):
            for row in record["observations"]:
                if row.get("status") != "passed":
                    row["reason"] = str(error)
    finally:
        record.update({"finished_at": datetime.now(timezone.utc).isoformat(), "elapsed_ms": round((time.monotonic() - began) * 1000)})
        path.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")
    if record["status"] != "passed":
        raise ValueError(record["reason"])
    print(f"Observed both native platform cases on {args.target} at {args.commit}; evidence: {path}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--commit", default=os.environ.get("COMMIT"), required="COMMIT" not in os.environ)
    parser.add_argument("--native", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=DEFAULT_TIMEOUT)
    args = parser.parse_args()
    if not math.isfinite(args.timeout) or args.timeout <= 0:
        parser.error("--timeout must be a finite positive runner budget")
    observe(args)


if __name__ == "__main__":
    main()
