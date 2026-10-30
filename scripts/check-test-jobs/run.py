#!/usr/bin/env python3
"""Keep test process exits and inspect or clean only exactly owned database leases."""

import argparse
from contextlib import ExitStack, contextmanager
import fcntl
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import time
from urllib.parse import parse_qsl, unquote, urlsplit
import uuid

DEFAULT_URL = "postgres://orch:orch@127.0.0.1:55432/postgres"
# Database administration can include a slow checkpoint and server-wide barrier.
ADMIN_TIMEOUT_SECONDS = 300
# Give an owned test process time to stop before sending a forceful signal.
STOP_GRACE_SECONDS = 5
# Twenty looks per second reap a finished child promptly without busy waiting.
STOP_POLL_SECONDS = 0.05
# Fifteen minutes covers a selected job's deployed waits and database cleanup.
DEFAULT_JOB_TIMEOUT_SECONDS = 900
# libpq's default TCP port when an address does not provide one.
POSTGRES_DEFAULT_PORT = 5432
# A command that succeeds but does not acknowledge cleanup fails as a job.
CLEANUP_EXIT = 2
# Summary text is evidence about tests; it never substitutes for a process exit.
RESULT = re.compile(r"test result: (\w+)\. (\d+) passed; (\d+) failed; (\d+) ignored")


@contextmanager
def locked(path, exclusive=False, nonblocking=False):
    with path.open("a+", encoding="utf-8") as file:
        operation = fcntl.LOCK_EX if exclusive else fcntl.LOCK_SH
        if nonblocking:
            operation |= fcntl.LOCK_NB
        fcntl.flock(file, operation)
        file.seek(0)
        try:
            yield file
        finally:
            fcntl.flock(file, fcntl.LOCK_UN)


def records_for(directory, run_id):
    server = urlsplit(os.environ.get("ORCH_TEST_DATABASE_URL", DEFAULT_URL))
    wanted = uuid.UUID(run_id)
    owned = []
    for path in sorted(directory.glob("**/records/*.json")):
        with locked(path) as file:
            record = json.load(file)
        if uuid.UUID(record["run_id"]) != wanted:
            continue
        expected = "orch_test_" + uuid.UUID(path.stem).hex
        if record["database"] != expected:
            raise ValueError("owned lease has an invalid generated database name")
        if (record["server_host"], record["server_port"]) != (
            server.hostname, server.port or POSTGRES_DEFAULT_PORT
        ):
            raise ValueError("owned lease names a different database server")
        owned.append((path, record))
    return owned


def sql(statement):
    environment = dict(os.environ)
    address = urlsplit(environment.get("ORCH_TEST_DATABASE_URL", DEFAULT_URL))
    parameters = {"host": unquote(address.hostname or "localhost"),
                  "port": str(address.port or POSTGRES_DEFAULT_PORT),
                  "dbname": unquote(address.path.lstrip("/")) or "postgres"}
    if address.username is not None:
        parameters["user"] = unquote(address.username)
    if address.password is not None:
        environment["PGPASSWORD"] = unquote(address.password)
    parameters.update(parse_qsl(address.query))
    if "password" in parameters:
        environment["PGPASSWORD"] = parameters.pop("password")
    # libpq does not expand a URI supplied through PGDATABASE. Give it parsed
    # connection parameters while keeping the password out of process arguments.
    def quoted(value):
        return "'" + value.replace("\\", "\\\\").replace("'", "\\'") + "'"
    for name in parameters:
        if not re.fullmatch(r"[a-z_]+", name):
            raise ValueError("unsupported test database connection option")
    connection = " ".join(name + "=" + quoted(value) for name, value in parameters.items())
    result = subprocess.run(
        ["psql", "-X", "-A", "-t", "-d", connection, "-v", "ON_ERROR_STOP=1", "-c", statement],
        env=environment, capture_output=True, text=True, timeout=ADMIN_TIMEOUT_SECONDS,
    )
    if result.returncode:
        raise RuntimeError("owned database inspection failed: " + result.stderr.strip())
    return result.stdout.strip().splitlines()


def present_databases(owned):
    if not owned:
        return []
    # Each name was checked against its generated UUID before it reaches SQL.
    names = ",".join("'" + record["database"] + "'" for _, record in owned)
    return sql("SELECT datname FROM pg_database WHERE datname IN (" + names + ") ORDER BY datname")


def inspect(directory, run_id):
    owned = records_for(directory, run_id)
    return {
        "run_id": run_id,
        "owned_databases": [record["database"] for _, record in owned],
        "remaining_databases": present_databases(owned),
        "cleanup_errors": [
            {"database": record["database"], "state": record["state"], "error": record["error"]}
            for _, record in owned if record["state"] not in {"dropped", "cleaned_after_exit"}
        ],
    }


def cleanup(directory, run_id):
    owned = records_for(directory, run_id)
    # Serialize with admission, and refuse any slot still held by a live fixture.
    # No process is signalled here and no database outside these records is read.
    with ExitStack() as locks:
        scopes = sorted({path.parent.parent for path, _ in owned})
        for scope in scopes:
            locks.enter_context(locked(scope / "budget.lock", exclusive=True))
        slots = sorted({path.parent.parent / f"slot-{record['slot']}.lock" for path, record in owned})
        for slot in slots:
            locks.enter_context(locked(slot, exclusive=True, nonblocking=True))
        existing = set(present_databases(owned))
        removed = []
        for path, record in owned:
            if record["database"] in existing:
                sql("DROP DATABASE " + record["database"] + " WITH (FORCE)")
                removed.append(record["database"])
            record["state"] = "cleaned_after_exit" if record["database"] in existing else "dropped"
            record["error"] = None
            pending = path.with_suffix(".pending")
            with pending.open("w", encoding="utf-8") as file:
                json.dump(record, file)
                file.flush()
                os.fsync(file.fileno())
            pending.replace(path)
        return {**inspect(directory, run_id), "exact_cleanup": removed}


def stop_group(process):
    try:
        os.killpg(process.pid, signal.SIGTERM)
    except ProcessLookupError:
        return
    deadline = time.monotonic() + STOP_GRACE_SECONDS
    while time.monotonic() < deadline:
        process.poll()
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            break
        time.sleep(STOP_POLL_SECONDS)
    else:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
    process.wait()


def run(args, directory, output):
    run_id = str(uuid.uuid4())
    # Preserve ownership before the command starts, even if the runner itself
    # is stopped before it can write the final exit and inspection result.
    (output / "run.json").write_text(json.dumps({
        "run_id": run_id, "runner_pid": os.getpid(),
    }) + "\n", encoding="utf-8")
    environment = dict(os.environ, ORCH_TEST_DATABASE_RUN_ID=run_id,
                       ORCH_TEST_DATABASE_LEASE_DIR=str(directory))
    began = time.monotonic()
    timed_out = False
    with (output / "output.log").open("w", encoding="utf-8") as log:
        process = subprocess.Popen(args.command, env=environment, stdout=log,
                                   stderr=subprocess.STDOUT, start_new_session=True)
        try:
            process.wait(timeout=args.timeout_seconds)
        except subprocess.TimeoutExpired:
            timed_out = True
        finally:
            # Test children belong to this group even if their parent ended first.
            stop_group(process)
    process_exit = process.returncode
    text = (output / "output.log").read_text(encoding="utf-8", errors="replace")
    results = [dict(verdict=m[0], passed=int(m[1]), failed=int(m[2]), ignored=int(m[3]))
               for m in RESULT.findall(text)]
    report = {
        "run_id": run_id,
        "process_exit": process_exit,
        "timed_out": timed_out,
        "test_results": results,
        "elapsed_seconds": round(time.monotonic() - began, 3),
    }
    try:
        report.update(inspect(directory, run_id))
        cleanup_ok = not report["remaining_databases"] and not report["cleanup_errors"]
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        report["inspection_error"] = str(error)
        cleanup_ok = False
    if timed_out:
        verdict = 124
    elif process_exit:
        verdict = process_exit if process_exit > 0 else 128 - process_exit
    elif not cleanup_ok:
        verdict = CLEANUP_EXIT
    else:
        verdict = 0
    report["exit"] = verdict
    return report, verdict


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--lease-dir", type=Path)
    # A complete job, including deployed hosting waits and cleanup, may need minutes.
    parser.add_argument("--timeout-seconds", type=float, default=DEFAULT_JOB_TIMEOUT_SECONDS)
    action = parser.add_mutually_exclusive_group()
    action.add_argument("--inspect-run", type=str)
    action.add_argument("--cleanup-run", type=str)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    if args.command[:1] == ["--"]:
        args.command = args.command[1:]
    output = args.output_dir.resolve()
    output.mkdir(parents=True, exist_ok=True)
    directory = (args.lease_dir or Path(os.environ.get(
        "ORCH_TEST_DATABASE_LEASE_DIR", str(Path(tempfile.gettempdir()) / "orch-test-databases")
    ))).resolve()
    if args.inspect_run or args.cleanup_run:
        run_id = str(uuid.UUID(args.inspect_run or args.cleanup_run))
        report = cleanup(directory, run_id) if args.cleanup_run else inspect(directory, run_id)
        verdict = CLEANUP_EXIT if report["remaining_databases"] or report["cleanup_errors"] else 0
        report["exit"] = verdict
    else:
        if not args.command or args.timeout_seconds <= 0:
            parser.error("a command and a positive job timeout are required")
        report, verdict = run(args, directory, output)
    (output / "result.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(report))
    return verdict


if __name__ == "__main__":
    sys.exit(main())
