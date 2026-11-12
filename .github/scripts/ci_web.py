"""Discover, partition and reconcile the complete browser suite on one checkout.

Case hashes are evidence only. Playwright selection always uses its canonical
project/file/title selectors. Nothing here changes test deadlines or workers.
"""
from __future__ import annotations

import argparse
from collections import Counter, defaultdict
import hashlib
import json
import math
import os
from pathlib import Path
import subprocess
import time

DEADLINE = 3600
# Existing production-server allowance, plus cold install/upload/teardown reserve.
# The second part is a planning allowance, not a measured hosted upper bound.
RESERVE = 600 + 120
PAYLOAD = DEADLINE - RESERVE
# Successful production controls. Unknown cases remain inferred, not certified.
DESKTOP = 94.571
PHONE = 12.478 + 6.382676
FUNCTIONAL = 11.713833333333334
LAYOUT = "e2e/layout-rules.spec.ts"
# This file resets its world in beforeEach and has no cross-case assertions.
# Isolating the three long scale cases avoids aggregating their quiet waits.
INDEPENDENT_SCALE = {"tests/performance/thread-scale.spec.ts"}
CRITICAL_ANNOTATIONS = {"skip", "serial", "fixme", "fail"}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def read(path):
    return json.loads(Path(path).read_text())


def write(path, value):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")


def annotations(values):
    return [{"type": a["type"], "description": a.get("description")} for a in values]


def cases(report):
    """The actual JSON reporter tree, including skipped and failed cases."""
    output = []

    def walk(suites, parents):
        for suite in suites:
            names = parents + [suite["title"]]
            for spec in suite.get("specs", []):
                if not names or names[0] != spec["file"]:
                    raise ValueError("Unexpected reporter file/title hierarchy")
                for test in spec["tests"]:
                    output.append({
                        "id": spec["id"], "project": test["projectName"],
                        "file": spec["file"], "titles": names[1:] + [spec["title"]],
                        "annotations": annotations(test.get("annotations", [])),
                        "expected": test["expectedStatus"],
                        "results": [{k: r.get(k) for k in ("status", "retry", "duration")}
                                    for r in test.get("results", [])],
                    })
            walk(suite.get("suites", []), names)

    walk(report.get("suites", []), [])
    return output


def identity(case):
    return (case["project"], case["file"], tuple(case["titles"]))


def declared(case):
    return {k: case[k] for k in ("id", "project", "file", "titles", "annotations", "expected")}


def selector(case, whole_file=False):
    parts = [f"[{case['project']}]", case["file"]]
    if not whole_file:
        parts += case["titles"]
    if any("›" in p or "\n" in p or p.strip() != p for p in parts):
        raise ValueError("Ambiguous canonical selector; retain a whole-file unit instead")
    return " › ".join(parts)


def source(root):
    def git(*args):
        return subprocess.check_output(["git", *args], cwd=root, text=True).strip()
    if git("diff", "--name-only", "HEAD", "--", "frontend", ".github/scripts/ci_web.py"):
        raise ValueError("Source changed after checkout")
    return {"head": git("rev-parse", "HEAD"),
            "frontend_tree": git("rev-parse", "HEAD:frontend"),
            "lock": digest((root / "frontend/package-lock.json").read_bytes())}


def unit(rows, kind, whole_file=True, weight=FUNCTIONAL, calibrated=False):
    return {"kind": kind, "cases": [declared(r) for r in rows],
            "selectors": list(dict.fromkeys(selector(r, whole_file) for r in rows)),
            "seconds": None if weight is None else sum(0 if r["expected"] == "skipped" else weight for r in rows),
            "calibration_required": not calibrated}


def partition(rows):
    """Keep files/serial groups intact; only reviewed independent seams may split."""
    if not rows or len({identity(r) for r in rows}) != len(rows):
        raise ValueError("Empty discovery or duplicate case identities")
    files = defaultdict(list)
    for row in rows:
        if row["project"] not in {"chromium", "development"}:
            raise ValueError("New project requires an explicit execution review")
        files[row["project"], row["file"]].append(row)
    groups, functional, development, presentations = [], [], [], defaultdict(list)

    def group(kind, units):
        return {"name": f"{kind}-{len(groups)+1:02d}", "kind": kind,
                "units": units, "seconds": None if any(u["seconds"] is None for u in units) else sum(u["seconds"] for u in units),
                "development": kind == "development",
                "calibration_required": any(u["calibration_required"] for u in units)}

    for (project, file), members in files.items():
        serial = any(a["type"] == "serial" for r in members for a in r["annotations"])
        if project == "development":
            # Whole source-harness files; no production-project substitution.
            development.append(unit(members, "development-file", weight=None))
        elif file == LAYOUT:
            if serial:
                raise ValueError("Layout gained serial semantics; review partition boundary")
            for row in members:
                if len(row["titles"]) == 1:
                    functional.append(unit([row], "manifest", whole_file=False))
                else:
                    presentation = row["titles"][0].removeprefix("signed-out ")
                    if not presentation.startswith("layout rules "):
                        raise ValueError("Unknown layout lifecycle")
                    presentations[presentation].append(row)
        elif file.startswith("tests/performance/"):
            if file in INDEPENDENT_SCALE and not serial:
                for row in members:
                    # Deliberately no healthy runtime forecast for scale cases.
                    groups.append(group("performance", [unit([row], "scale-case", False, weight=None)]))
            else:
                groups.append(group("performance", [unit(members, "performance-file", weight=None)]))
        else:
            functional.append(unit(members, "functional-file"))
    for title, members in presentations.items():
        weight = DESKTOP if "1440px" in title else PHONE if "400px" in title else None
        if weight is None:
            raise ValueError("New presentation requires healthy measurements")
        count = math.ceil(len(members) * weight / PAYLOAD)
        for index in range(count):
            selected = members[index*len(members)//count:(index+1)*len(members)//count]
            groups.append(group("layout", [unit(selected, "layout-rows", False, weight)]))
    if any(u["seconds"] > PAYLOAD for u in functional):
        raise ValueError("A whole functional file exceeds the forecast payload; review its lifecycle")
    # Passing cases fill at most half of the hour, so that a group whose failing cases are retried still
    # finishes and reports them within the deadline instead of being cancelled; a whole file larger than
    # that half keeps a group of its own.
    limit = max([PAYLOAD // 2] + [u["seconds"] for u in functional])
    count = max(1, math.ceil(sum(u["seconds"] for u in functional) / limit))
    while True:
        bins = [[] for _ in range(count)]
        for item in sorted(functional, key=lambda u: (-u["seconds"], u["selectors"])):
            target = min(bins, key=lambda b: sum(u["seconds"] for u in b))
            target.append(item)
        if all(sum(u["seconds"] for u in b) <= limit for b in bins):
            break
        count += 1  # At most one bin per whole file; no case is discarded.
    for selected in bins:
        if selected:
            groups.append(group("functional", selected))
    if development:
        groups.append(group("development", development))
    if len(groups) > 256:
        raise ValueError("GitHub matrix limit reached; review grouping, never truncate discovery")
    assigned = [r for g in groups for u in g["units"] for r in u["cases"]]
    if Counter(map(identity, assigned)) != Counter(map(identity, rows)):
        raise ValueError("Matrix omitted or duplicated a case")
    # Every file carrying serial annotations must remain on one runner.
    for key, members in files.items():
        if any(a["type"] == "serial" for r in members for a in r["annotations"]):
            owners = {g["name"] for g in groups for u in g["units"]
                      if any((r["project"], r["file"]) == key for r in u["cases"])}
            if len(owners) != 1:
                raise ValueError("Serial file was split")
    return groups


def plan(root, inventory, directory):
    text = (root / "frontend" / LAYOUT).read_text()
    if "test.beforeEach" not in text or "test.beforeAll" in text:
        raise ValueError("Layout isolation prerequisite is absent; do not shard its sequence")
    report = read(inventory)
    if report.get("errors"):
        raise ValueError("Discovery contains load errors")
    config = report["config"]
    if config["workers"] != 1 or config["fullyParallel"]:
        raise ValueError("The single mutable fake world requires one worker")
    rows = cases(report)
    groups = partition(rows)
    result = {"source": source(root), "cases": [declared(r) for r in rows], "groups": groups,
              "deadline_seconds": DEADLINE, "setup_reserve_seconds": RESERVE,
              "annotation_counts": dict(Counter(a["type"] for r in rows for a in r["annotations"])),
              "capacity": "Forecast only; every group requires hosted completion calibration. "
                          "Performance runtime timeouts/quiet waits remain unchanged and unmeasured."}
    directory = Path(directory)
    directory.mkdir(parents=True, exist_ok=True)
    for group in groups:
        lines = list(dict.fromkeys(s for u in group["units"] for s in u["selectors"]))
        (directory / (group["name"] + ".txt")).write_text("\n".join(lines) + "\n")
    write(directory / "plan.json", result)
    matrix = {"include": [{k: g[k] for k in ("name", "kind", "development", "calibration_required")}
                          for g in groups]}
    print(json.dumps({"cases": len(rows), "groups": len(groups), "annotations": result["annotation_counts"]}))
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a") as handle:
            handle.write("matrix=" + json.dumps(matrix, separators=(",", ":")) + "\n")
    return result


def files_in(directory):
    files = {}
    for path in sorted(directory.rglob("*")):
        if path.is_symlink():
            raise ValueError("Build artifact contains a symlink")
        if path.is_file():
            files[path.relative_to(directory).as_posix()] = digest(path.read_bytes())
    if not files:
        raise ValueError("Empty production artifact")
    return files


def seal(root, directory):
    manifest = {"source": source(root), "files": files_in(root / "frontend/dist")}
    if manifest["source"] != read(directory / "plan.json")["source"]:
        raise ValueError("Build and discovery sources differ")
    write(directory / "dist-manifest.json", manifest)


def verify(root, directory):
    manifest = read(directory / "dist-manifest.json")
    if manifest["source"] != source(root) or manifest["source"] != read(directory / "plan.json")["source"]:
        raise ValueError("Artifact belongs to another source/lock")
    if files_in(root / "frontend/dist") != manifest["files"]:
        raise ValueError("Production artifact changed or is incomplete")
    return digest((directory / "dist-manifest.json").read_bytes())


def expected_group(plan_data, name):
    return next(g for g in plan_data["groups"] if g["name"] == name)


def check_selected(expected, actual):
    if [declared(r) for r in actual] != expected:
        # Selection retains source declaration order; allocation list order never reorders tests.
        e = {identity(r): r for r in expected}
        a = {identity(r): declared(r) for r in actual}
        if e != a or len(actual) != len(expected):
            raise ValueError("Canonical selector round-trip changed the assigned cases")
    # Cross-file allocation may reorder units; within-file declaration order must not change.
    for file in {(r["project"], r["file"]) for r in expected}:
        e = [identity(r) for r in expected if (r["project"], r["file"]) == file]
        a = [identity(r) for r in actual if (r["project"], r["file"]) == file]
        if e != a:
            raise ValueError("Within-file case order changed")


def run(root, directory, name, reports):
    artifact = verify(root, directory)
    data = read(directory / "plan.json")
    group = expected_group(data, name)
    expected = [r for u in group["units"] for r in u["cases"]]
    env = dict(os.environ, ORCH_E2E_NO_DEV_SERVER="0" if group["development"] else "1",
               CI_WEB_SOURCE=data["source"]["head"], CI_WEB_VERIFIED_ARTIFACT=artifact)
    frontend = root / "frontend"
    command = ["node", "node_modules/playwright/cli.js", "test", "--config=e2e/playwright-ci.config.ts",
               "--test-list=" + str(directory / (name + ".txt"))]
    selected = subprocess.run(command + ["--list", "--reporter=json"], cwd=frontend, env=env,
                              check=True, capture_output=True, text=True)
    selected_report = json.loads(selected.stdout)
    if selected_report.get("errors"):
        raise ValueError("Selector discovery contains errors")
    check_selected(expected, cases(selected_report))
    reports.mkdir(parents=True, exist_ok=True)
    attempt = int(env.get("GITHUB_RUN_ATTEMPT", "1"))
    stem = name + f"-attempt-{attempt}"
    raw = reports / (stem + ".raw.json")
    env["PLAYWRIGHT_JSON_OUTPUT_NAME"] = str(raw)
    started = time.monotonic()
    # Emit each unexpected result immediately, so a later runner timeout cannot
    # hide every earlier assertion behind the list reporter's end-of-run summary.
    result = subprocess.run(command + ["--reporter=line,json", "--output=" + str(root / ".tmp/ci-web-results" / name)],
                            cwd=frontend, env=env, check=False)
    observed = read(raw) if raw.is_file() else {"errors": ["Missing actual report"]}
    receipt = {"group": name, "attempt": attempt, "source": source(root), "artifact": artifact,
               "returncode": result.returncode, "elapsed_seconds": time.monotonic()-started,
               "cases": cases(observed), "errors": observed.get("errors", []),
               "raw_sha256": digest(raw.read_bytes()) if raw.is_file() else None}
    write(reports / (stem + ".receipt.json"), receipt)
    verify(root, directory)
    return result.returncode


def reconcile(data, receipts):
    """Audit actual named cases, not just runner exit codes or test-list contents."""
    expected = {identity(r): r for r in data["cases"]}
    observed, groups = {}, Counter()
    errors, durations = [], {}
    newest = {}
    for receipt in receipts:
        key = receipt["group"]
        if key not in newest or receipt.get("attempt", 1) > newest[key].get("attempt", 1):
            newest[key] = receipt
        elif receipt.get("attempt", 1) == newest[key].get("attempt", 1):
            errors.append(key + ": duplicate report for one attempt")
    receipts = list(newest.values())
    for receipt in receipts:
        name = receipt["group"]
        groups[name] += 1
        if receipt["source"] != data["source"]:
            errors.append(name + ": wrong source")
        if name not in {g["name"] for g in data["groups"]}:
            errors.append(name + ": unknown group")
            continue
        group = expected_group(data, name)
        assigned = {identity(r) for u in group["units"] for r in u["cases"]}
        if {identity(r) for r in receipt["cases"]} != assigned:
            errors.append(name + ": actual report inventory differs from assignment")
        if receipt["returncode"] or receipt["errors"]:
            errors.append(name + ": runner failed or reported errors")
        durations[name] = receipt["elapsed_seconds"]
        for row in receipt["cases"]:
            key = identity(row)
            if key not in expected or key in observed:
                errors.append(name + ": unknown or duplicate actual case")
                continue
            observed[key] = row
            wanted = expected[key]
            if row["id"] != wanted["id"] or row["expected"] != wanted["expected"]:
                errors.append(name + ": identity/expected outcome changed")
            wanted_annotations = [a for a in wanted["annotations"] if a["type"] in CRITICAL_ANNOTATIONS]
            actual_annotations = [a for a in row["annotations"] if a["type"] in CRITICAL_ANNOTATIONS]
            if wanted_annotations != actual_annotations:
                errors.append(name + ": intentional annotations changed")
            results = row["results"]
            if not results or (wanted["expected"] == "skipped" and any(r["status"] != "skipped" for r in results)):
                errors.append(name + ": absent execution/intentional skip result")
            if wanted["expected"] != "skipped" and (not results or results[-1]["status"] != wanted["expected"]):
                errors.append(name + ": no successful expected execution")
    wanted_groups = Counter(g["name"] for g in data["groups"])
    if groups != wanted_groups:
        errors.append("Missing or duplicate matrix reports")
    if set(observed) != set(expected):
        errors.append("Actual report union omitted cases")
    if len({r["artifact"] for r in receipts}) != 1:
        errors.append("Matrix used different production artifacts; rerun all groups after rebuilding")
    return {"source": data["source"], "expected_cases": len(expected), "observed_cases": len(observed),
            "expected_groups": len(wanted_groups), "observed_groups": dict(groups),
            "annotations": data["annotation_counts"], "elapsed_seconds": durations, "errors": errors}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["plan", "seal", "run", "audit"])
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument("--directory", type=Path, default=Path(".tmp/ci-web"))
    parser.add_argument("--inventory", type=Path)
    parser.add_argument("--group")
    parser.add_argument("--reports", type=Path, default=Path(".tmp/ci-web-reports"))
    args = parser.parse_args()
    root = args.root.resolve()
    directory = (root / args.directory).resolve()
    reports = (root / args.reports).resolve()
    if args.action == "plan":
        plan(root, args.inventory, directory)
    elif args.action == "seal":
        seal(root, directory)
    elif args.action == "run":
        return run(root, directory, args.group, reports)
    else:
        data = read(directory / "plan.json")
        proof = reconcile(data, [read(p) for p in sorted(reports.rglob("*.receipt.json"))])
        if data["source"] != source(root):
            proof["errors"].append("Plan does not belong to this checkout")
        for path in reports.rglob("*.receipt.json"):
            receipt = read(path)
            raw = path.with_name(path.name.replace(".receipt.json", ".raw.json"))
            if not raw.is_file() or digest(raw.read_bytes()) != receipt["raw_sha256"]:
                proof["errors"].append(path.name + ": missing or wrong actual report")
            elif cases(read(raw)) != receipt["cases"]:
                proof["errors"].append(path.name + ": receipt differs from actual reporter inventory")
        manifest = digest((directory / "dist-manifest.json").read_bytes())
        latest = {}
        for receipt in (read(p) for p in reports.rglob("*.receipt.json")):
            name = receipt["group"]
            if name not in latest or receipt.get("attempt", 1) > latest[name].get("attempt", 1):
                latest[name] = receipt
        if any(r["artifact"] != manifest for r in latest.values()):
            proof["errors"].append("Reports do not belong to the current single build")
        write(directory / "coverage.json", proof)
        print(json.dumps(proof, indent=2))
        return int(bool(proof["errors"]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
