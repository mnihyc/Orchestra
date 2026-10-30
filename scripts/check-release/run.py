#!/usr/bin/env python3
"""Runs the whole proof of a release candidate on its exact commit and records what each part said.

    python3 scripts/check-release/run.py --output-dir .tmp/release-proof [--wrap "<gate>"] \\
        [--skip <step>]... [--only <step>]... [--with-deploy] [--install]

It records the candidate first (its commit, whether the tree is clean, the toolchains), then runs
every check in order, each with its own log, its exit and its time, and goes on after a failure:

    deny                               the locked dependency policy, sources and advisories
    fmt, clippy, doc, contracts        the Rust tree's static checks
    connector                          the connector the tests start, built first
    rust-tests                         every test program of the workspace (a real PostgreSQL)
    web-typecheck, web-lint,           the web application's checks and its unit and
    web-unit, web-build                component tests
    browser                            the browser suites (end to end, accessibility, performance)
    deploy                             the deployment scenarios on Docker (only with --with-deploy)

Each step says what it ran against: a real service, a real program, or the doubles of the tests.
The browser suites run against the fake API of the web application's tests: they prove the
screens, not the server. `--wrap` puts a gate in front of every heavy command; the environment is
passed on (give the test database's variables there). `summary.md` and `results.json` in the output
directory hold the record: the failing test programs and tests by name, never a guess from an older
log. Exit 0 when every step that ran passed.
"""

import argparse
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
FRONTEND = ROOT / "frontend"
# A step may take this long before it is counted as hanging: the workspace's tests take hours on a
# loaded machine, and a hang must still end the step rather than the proof.
STEP_TIMEOUT_SECONDS = 6 * 60 * 60
# The failures named in the summary, at most: the log holds every one.
FAILURES_SHOWN = 200

# (name, command, working directory, extra environment, heavy, what it runs against)
STEPS = [
    ("deny", ["cargo", "deny", "--locked", "check"], ROOT, {}, False, "the locked dependency graph and deny.toml"),
    ("fmt", ["cargo", "fmt", "--all", "--", "--check"], ROOT, {}, False, "the source"),
    (
        "clippy",
        ["cargo", "clippy", "--workspace", "--all-targets", "--locked", "--", "-D", "warnings"],
        ROOT,
        {},
        True,
        "the source",
    ),
    (
        "doc",
        ["cargo", "doc", "--workspace", "--no-deps", "--locked"],
        ROOT,
        {"RUSTDOCFLAGS": "-D warnings"},
        True,
        "the source",
    ),
    ("contracts", ["cargo", "xtask", "contracts", "--check"], ROOT, {}, True, "the generated contracts"),
    ("connector", ["cargo", "build", "-p", "orch-connector"], ROOT, {}, True, "the connector program"),
    (
        "rust-tests",
        ["cargo", "test", "--workspace", "--no-fail-fast", "--locked"],
        ROOT,
        {},
        True,
        "a real PostgreSQL; real connector, environment host and Docker where a test starts them",
    ),
    ("web-typecheck", ["npm", "run", "typecheck"], FRONTEND, {}, True, "the source"),
    ("web-lint", ["npm", "run", "lint"], FRONTEND, {}, True, "the source"),
    ("web-unit", ["npm", "run", "test"], FRONTEND, {}, True, "the doubles of the tests"),
    ("web-build", ["npm", "run", "build"], FRONTEND, {}, True, "the built application"),
    (
        "browser",
        ["npx", "playwright", "test"],
        FRONTEND,
        {},
        True,
        "the built application in a real browser, against the fake API of the tests",
    ),
    (
        "deploy",
        ["bash", "scripts/smoke/run-all.sh"],
        ROOT,
        {},
        True,
        "released stacks on Docker: real services and programs",
    ),
]


def facts():
    def said(command, cwd=ROOT):
        try:
            return subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=60).stdout.strip()
        except (OSError, subprocess.SubprocessError):
            return "unknown"

    changed = said(["git", "status", "--porcelain"])
    return {
        "commit": said(["git", "rev-parse", "HEAD"]),
        "clean": changed == "",
        "changed_paths": len(changed.splitlines()) if changed else 0,
        "rustc": said(["rustc", "-V"]),
        "cargo": said(["cargo", "-V"]),
        "cargo_deny": said(["cargo", "deny", "--version"]),
        "dependency_inputs": {
            name: hashlib.sha256((ROOT / name).read_bytes()).hexdigest()
            for name in ("deny.toml", "Cargo.lock", "frontend/package-lock.json")
        },
        "node": said(["node", "-v"]),
        "npm": said(["npm", "-v"]),
        "started": datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
    }


def rust_failures(log):
    """The test programs that failed and the tests that failed in them, from cargo's own lines."""
    text = log.read_text(encoding="utf-8", errors="replace")
    programs, failed, current = [], [], None
    for line in text.splitlines():
        running = re.match(r"\s*Running (\S+)", line)
        if running:
            current = running.group(1)
        result = re.match(r"test result: FAILED\. (\d+) passed; (\d+) failed", line)
        if result and current:
            programs.append({"program": current, "passed": int(result.group(1)), "failed": int(result.group(2))})
        test = re.match(r"test (\S+) \.\.\. FAILED", line)
        if test:
            failed.append(f"{current}: {test.group(1)}")
    return programs, failed


def run(name, command, cwd, extra, heavy, wrap, out):
    env = dict(os.environ)
    env.update(extra)
    log = out / f"{name}.log"
    started = time.monotonic()
    with open(log, "w", encoding="utf-8") as output:
        process = subprocess.Popen(
            [*(wrap if heavy else []), *command],
            cwd=cwd,
            env=env,
            stdout=output,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        try:
            code = process.wait(timeout=STEP_TIMEOUT_SECONDS)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            code = 124
    return code, round(time.monotonic() - started, 1), log


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--wrap", default="")
    parser.add_argument("--skip", action="append", default=[])
    parser.add_argument("--only", action="append", default=[])
    parser.add_argument("--with-deploy", action="store_true")
    parser.add_argument("--install", action="store_true", help="run npm ci first, from the lock file")
    arguments = parser.parse_args()
    out = arguments.output_dir
    out.mkdir(parents=True, exist_ok=True)
    wrap = shlex.split(arguments.wrap)
    record = {"candidate": facts(), "steps": []}
    steps = list(STEPS)
    if arguments.install:
        steps.insert(0, ("web-install", ["npm", "ci"], FRONTEND, {}, False, "the lock file"))
    for name, command, cwd, extra, heavy, against in steps:
        if (arguments.only and name not in arguments.only) or name in arguments.skip:
            continue
        if name == "deploy" and not arguments.with_deploy:
            continue
        code, seconds, log = run(name, command, cwd, extra, heavy, wrap, out)
        step = {"step": name, "command": " ".join(command), "against": against, "exit": code, "seconds": seconds}
        if name == "rust-tests":
            step["failed_programs"], step["failed_tests"] = rust_failures(log)
        record["steps"].append(step)
        print(f"{'ok  ' if code == 0 else 'FAIL'} {name} ({seconds}s, exit {code})", flush=True)
    record["finished"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    (out / "results.json").write_text(json.dumps(record, indent=2), encoding="utf-8")

    candidate = record["candidate"]
    lines = [
        f"# Proof of {candidate['commit']}",
        "",
        f"Started {candidate['started']}, finished {record['finished']}. Tree clean: {candidate['clean']}"
        f" ({candidate['changed_paths']} changed paths). {candidate['rustc']}; {candidate['cargo']};"
        f" node {candidate['node']}; npm {candidate['npm']}.",
        "",
        "| Step | Exit | Seconds | Against |",
        "|------|-----:|--------:|---------|",
    ]
    for step in record["steps"]:
        lines.append(f"| {step['step']} | {step['exit']} | {step['seconds']} | {step['against']} |")
    for step in record["steps"]:
        if step.get("failed_tests"):
            lines += ["", f"## Failed tests of {step['step']}", ""]
            lines += [f"- {name}" for name in step["failed_tests"][:FAILURES_SHOWN]]
    (out / "summary.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    return 0 if all(step["exit"] == 0 for step in record["steps"]) else 1


if __name__ == "__main__":
    sys.exit(main())
