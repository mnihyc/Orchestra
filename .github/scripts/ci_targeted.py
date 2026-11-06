"""Run the checks one .ci-targets file names, every one even after a failure, and fail when any failed.

Each non-empty line that is not a comment is one check:
  test <cargo test arguments>   e.g. `test -p orch-api-core --test admin_mode`; the connector is built once first
  clippy | doc | fmt            the whole CI's lint, documentation and formatting checks
  web <npm script>              e.g. `web lint`, `web typecheck`, `web test`, `web build`; `npm ci` runs once first
  web-e2e                       the end-to-end suites against the tests' fake API
"""

import os
import shlex
import subprocess
import sys

ROOT = os.getcwd()
FRONTEND = os.path.join(ROOT, "frontend")


def run(label, command, cwd=ROOT, env=None):
    print(f"::group::{label}", flush=True)
    code = subprocess.run(command, cwd=cwd, env={**os.environ, **(env or {})}).returncode
    print("::endgroup::", flush=True)
    print(f"{'PASSED' if code == 0 else 'FAILED'}: {label}", flush=True)
    return code == 0


def main(path):
    lines = [line.strip() for line in open(path, encoding="utf-8")]
    checks = [line for line in lines if line and not line.startswith("#")]
    if not checks:
        print(f"{path} names no check", file=sys.stderr)
        return 2
    results = []
    if any(check.startswith("test ") for check in checks):
        results.append(("prepare native Docker fixtures", run(
            "prepare native Docker fixtures", [sys.executable, "-I", "-B", ".github/scripts/ci_fixtures.py", "--targets", path])))
    connector = False
    npm = False
    for check in checks:
        word, _, rest = check.partition(" ")
        if word == "test":
            if not connector:
                connector = True
                results.append((
                    "build the connector the tests start",
                    run("build the connector", ["cargo", "build", "--locked", "-p", "orch-connector"]),
                ))
            results.append((check, run(check, ["cargo", "test", "--locked", "--no-fail-fast", *shlex.split(rest)])))
        elif word == "clippy":
            results.append((check, run(check, ["cargo", "clippy", "--workspace", "--all-targets", "--locked",
                                               "--keep-going", "--", "-D", "warnings"])))
        elif word == "doc":
            results.append((check, run(check, ["cargo", "doc", "--workspace", "--no-deps", "--locked", "--keep-going"],
                                       env={"RUSTDOCFLAGS": "-D warnings"})))
        elif word == "fmt":
            results.append((check, run(check, ["cargo", "fmt", "--all", "--check"])))
        elif word in ("web", "web-e2e"):
            if not npm:
                npm = True
                results.append(("npm ci", run("npm ci", ["npm", "ci"], cwd=FRONTEND)))
            if word == "web":
                results.append((check, run(check, ["npm", "run", *shlex.split(rest)], cwd=FRONTEND)))
            else:
                run("install the browser", ["npx", "playwright", "install", "--with-deps", "chromium"], cwd=FRONTEND)
                results.append((check, run(check, ["npx", "playwright", "test", *shlex.split(rest)], cwd=FRONTEND)))
        else:
            print(f"unknown check: {check}", file=sys.stderr)
            results.append((check, False))
    print("\nSummary:")
    for label, passed in results:
        print(f"  {'PASSED' if passed else 'FAILED'}  {label}")
    return 0 if all(passed for _, passed in results) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
