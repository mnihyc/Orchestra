#!/usr/bin/env python3
"""Measure a compiled search target on its isolated synthetic corpus; never build it here."""

import argparse
import datetime
import json
import os
from pathlib import Path
import subprocess
import sys
from urllib.parse import urlsplit


CASE = "a_million_document_corpus_reports_words_fused_and_agent_latency"
PREPARE = "prepare_an_isolated_search_measurement_corpus"
QUERIES = ["english-common", "english-rare", "chinese-two", "chinese-one", "phrase"]


def compiled_program(path):
    """Accept either Cargo JSON artifacts or a previously saved executable manifest."""
    source = path.read_text()
    try:
        entries = json.loads(source)
    except json.JSONDecodeError:
        entries = [json.loads(line) for line in source.splitlines() if line.startswith("{")]
    if not isinstance(entries, list):
        entries = [entries]
    candidates = set()
    for entry in entries:
        if entry.get("name") == "search_meaning" and entry.get("exe"):
            candidates.add(entry["exe"])
        if (
            entry.get("reason") == "compiler-artifact"
            and entry.get("target", {}).get("name") == "search_meaning"
            and entry.get("executable")
        ):
            candidates.add(entry["executable"])
    if len(candidates) != 1:
        raise ValueError("the manifest must name one compiled search_meaning test target")
    return Path(candidates.pop())


def memory_available():
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError):
        pass
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=["debug", "release"], required=True)
    compiled = parser.add_mutually_exclusive_group()
    compiled.add_argument("--program", type=Path)
    compiled.add_argument("--programs", type=Path)
    parser.add_argument("--database-url", default=os.environ.get("ORCH_SEARCH_SCALE_URL"))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--reader", choices=["all", "small"])
    parser.add_argument("--query", choices=QUERIES)
    parser.add_argument("--cache-state", choices=["cold", "warm", "unspecified"], default="unspecified")
    parser.add_argument("--trace", action="store_true", help="diagnostic phase logging; disclose its overhead")
    parser.add_argument("--plan-markers", action="store_true", help="label actual database plans by sample; diagnostics only")
    parser.add_argument("--cold-container", help="restart an explicitly selected bind-mounted fixture before each API and agent sample")
    parser.add_argument("--prepare", action="store_true", help="seed a new empty isolated database before measuring it")
    parser.add_argument("--finish-index", action="store_true", help="finish the missing generation index of an otherwise complete native fixture")
    parser.add_argument("--documents", type=int, default=1_000_000, help="preparation size; a multiple of 1000 (default: one million)")
    args = parser.parse_args()
    setup = args.prepare or args.finish_index
    if args.prepare and args.finish_index:
        parser.error("choose fresh preparation or finishing its missing index")
    if args.database_url is None:
        parser.error("name the isolated corpus with --database-url or ORCH_SEARCH_SCALE_URL")
    try:
        address = urlsplit(args.database_url)
        isolated = address.hostname == "127.0.0.1" and 41000 <= (address.port or 0) <= 41999 and address.port != 41532
    except ValueError:
        isolated = False
    if not isolated:
        parser.error("use an isolated loopback corpus on ports 41000–41999, never the shared test database")
    if setup and (args.reader or args.query or args.trace or args.plan_markers or args.cold_container):
        parser.error("prepare the corpus separately from selecting or tracing a measurement")
    if setup and (not address.path.startswith('/orchprobe_search_') or args.documents < 1000 or args.documents % 1000):
        parser.error("preparation needs an empty orchprobe_search_ database and a multiple of 1000 documents")
    if not setup and args.program is None and args.programs is None:
        parser.error("measurement needs an already compiled target via --program or --programs")
    if args.cold_container and args.cache_state == "warm":
        parser.error("cold restarts cannot be labelled as a warm cache")
    if args.finish_index and (args.program is not None or args.programs is not None):
        parser.error("finishing a fixture index uses psql, without a compiled target")
    if setup and args.program is None and args.programs is None:
        if args.output.exists():
            parser.error("choose a new output directory to preserve previous measurements")
        args.output.mkdir(parents=True)
        # Isolated execution keeps the same fixture SQL usable while the Rust build place is occupied.
        import importlib.util
        specification = importlib.util.spec_from_file_location("search_corpus_prepare", Path(__file__).with_name("prepare.py"))
        assert specification is not None and specification.loader is not None
        preparer = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(preparer)
        started = datetime.datetime.now(datetime.timezone.utc).isoformat()
        result = (preparer.finish_index if args.finish_index else preparer.prepare)(args.database_url, args.documents, args.output)
        (args.output / "environment.json").write_text(json.dumps({
            "started_at": started, "finished_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
            "mode": "finish_index" if args.finish_index else "prepare", "preparation_engine": "psql", "build_performed": False,
            "initial_cache_state": "unspecified", "program_exit": result,
        }, indent=2) + "\n")
        if result == 0:
            print(f"Prepared {args.documents} documents with native search storage")
        return result
    try:
        program = args.program or compiled_program(args.programs)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        parser.error(str(error))
    if not program.is_file() or not os.access(program, os.X_OK):
        parser.error("the compiled test program must exist and be executable")
    if args.output.exists():
        parser.error("choose a new output directory to preserve previous measurements")
    args.output.mkdir(parents=True)
    report = args.output.resolve() / "measurements.json"
    env = os.environ.copy()
    for key in ["ORCH_SEARCH_SCALE_READER", "ORCH_SEARCH_SCALE_QUERY", "ORCH_SEARCH_SCALE_TRACE", "ORCH_SEARCH_SCALE_DOCUMENTS",
                "ORCH_SEARCH_SCALE_CONTROL_DIR", "ORCH_SEARCH_SCALE_PLAN_MARKERS"]:
        env.pop(key, None)
    env.update(
        RUST_MIN_STACK="16777216",
        ORCH_SEARCH_SCALE_URL=args.database_url,
        ORCH_SEARCH_SCALE_REPORT=str(report),
        ORCH_SEARCH_SCALE_CACHE_STATE=args.cache_state,
    )
    if args.reader:
        env["ORCH_SEARCH_SCALE_READER"] = args.reader
    if args.query:
        env["ORCH_SEARCH_SCALE_QUERY"] = args.query
    if args.trace:
        env["ORCH_SEARCH_SCALE_TRACE"] = "1"
    if args.plan_markers:
        env["ORCH_SEARCH_SCALE_PLAN_MARKERS"] = "1"
    controller = None
    if args.cold_container:
        import importlib.util
        specification = importlib.util.spec_from_file_location("search_cold_control", Path(__file__).with_name("cold.py"))
        assert specification is not None and specification.loader is not None
        module = importlib.util.module_from_spec(specification)
        specification.loader.exec_module(module)
        try:
            controller = module.Controller(args.cold_container, address, args.output / "cold-control")
        except (OSError, ValueError, subprocess.SubprocessError) as error:
            parser.error(str(error))
        env["ORCH_SEARCH_SCALE_CONTROL_DIR"] = str(controller.directory.resolve())
        env["ORCH_SEARCH_SCALE_CACHE_STATE"] = "cold"
    if args.prepare:
        env["ORCH_SEARCH_SCALE_DOCUMENTS"] = str(args.documents)
    metadata = {
        "started_at": datetime.datetime.now(datetime.timezone.utc).isoformat(),
        "requested_profile": args.profile,
        "program": program.name,
        "cpu_count": os.cpu_count(),
        "load_started": list(os.getloadavg()) if hasattr(os, "getloadavg") else None,
        "available_memory_started": memory_available(),
        "initial_cache_state": "cold" if controller is not None else args.cache_state,
        "phase_logging": args.trace,
        "build_performed": False,
        "mode": "prepare" if args.prepare else "measure",
        "plan_markers": args.plan_markers,
        "cold_container": args.cold_container,
    }
    with (args.output / "output.log").open("w") as output:
        if controller is not None:
            controller.start()
        try:
            result = subprocess.run(
                [str(program.resolve()), PREPARE if args.prepare else CASE, "--ignored", "--exact", "--nocapture", "--test-threads=1"],
                cwd=Path(__file__).resolve().parents[2] / "crates/control/api",
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.STDOUT,
                check=False,
            )
        finally:
            if controller is not None:
                controller.finish()
    metadata.update(
        finished_at=datetime.datetime.now(datetime.timezone.utc).isoformat(),
        load_finished=list(os.getloadavg()) if hasattr(os, "getloadavg") else None,
        available_memory_finished=memory_available(),
        program_exit=result.returncode,
        cold_restarts=controller.events if controller is not None else [],
        data_binding=controller.binding if controller is not None else None,
    )
    rows = json.loads(report.read_text()) if report.exists() else []
    metadata["measured_profiles"] = sorted({row.get("profile", "unknown") for row in rows})
    (args.output / "environment.json").write_text(json.dumps(metadata, indent=2) + "\n")
    if result.returncode:
        return result.returncode
    if not rows or metadata["measured_profiles"] != [args.profile]:
        print("No matching-profile measurements; inspect the report and compiled artifact", file=sys.stderr)
        return 2
    if controller is not None and (
        any(row.get("postgres_restart_between_samples") is not True for row in rows)
        or len(controller.events) != sum((row["warmups"] + row["observations"]) * 2 for row in rows)
        or any(event["ok"] is not True for event in controller.events)
    ):
        print("No verified cold sampling; inspect the target protocol and coordinator report", file=sys.stderr)
        return 2
    if args.plan_markers and any(row.get("plan_markers") is not True for row in rows):
        print("The compiled target does not support sample plan markers", file=sys.stderr)
        return 2
    if args.prepare:
        if len(rows) != 1 or rows[0].get("prepared") is not True:
            print("No prepared corpus; inspect the output before measuring", file=sys.stderr)
            return 2
        print(f"Prepared {rows[0]['documents']} documents with native search storage")
        return 0
    for row in rows:
        print(f"{row['reader']}/{row['query']}: " + json.dumps({
            key: row.get(key) for key in ["words_ms", "fused_ms", "search_shared_ms", "meaning_unavailable"]
        }))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
