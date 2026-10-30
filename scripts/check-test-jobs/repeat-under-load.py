#!/usr/bin/env python3
"""Runs named tests many times in a row while other cargo test commands load the machine, and says
which held every time.

A test that proves an order of events must hold however busy the machine is; a test that only held
because the machine was quiet proved a speed. This runs each candidate (default twenty times) while
four other `cargo test` commands run in loops beside it, after compiling every candidate first, so
that the runs measure the tests and not a build.

    python3 scripts/check-test-jobs/repeat-under-load.py --output-dir .tmp/repeat \\
        [--times 20] [--load "cargo test -p orch-types"]... [--wrap "<gate command>"] \\
        -- "orch-envhost lib git::tests::a_difference_longer_than_the_bound_is_cut_and_says_so" ...

A candidate is "<package> <target> <test name>": the target is `lib` for the package's own tests or
the name of an integration test program. Without candidates the bounds that the test kit corrected
are run: the Git byte bound, the two shells of the environment, the busy terminal and the two
guarded-write orders. `--wrap` puts a command in front of every cargo command (a gate that bounds
how many run at once on a shared machine). The environment is passed on unchanged: give the test
database's variables there. Exit 0 when every run of every candidate passed, 1 otherwise, 2 when a
candidate did not compile.
"""

import argparse
import json
import re
import shlex
import subprocess
import sys
import threading
import time
from pathlib import Path

# The corrected bounds of the test kit: each proves an order, which a loaded machine must not break.
DEFAULT_CANDIDATES = [
    "orch-envhost lib git::tests::a_difference_longer_than_the_bound_is_cut_and_says_so",
    "orch-envhost environment a_shell_opened_through_the_host_runs_where_it_was_asked_with_the_variables_and_is_listed_as_a_process",
    "orch-envhost environment detaching_ends_the_shells_of_the_environment_and_forgets_it",
    "orch-envhost terminal only_a_shell_with_a_command_running_counts_as_busy",
    "orch-api-core transaction_guards ordinary_changes_by_many_people_or_one_person_do_not_queue_on_identity",
    "orch-api-core transaction_guards authenticating_the_same_api_token_does_not_wait_for_an_open_action",
    "orch-envhost lib watch::tests::a_burst_of_changes_is_one_batch",
    "orch-envhost lib manifest::tests::a_manifest_of_two_hundred_thousand_entries_is_built_and_read_whole",
    "orch-envhost lib search::tests::a_pattern_that_is_too_large_or_wrong_is_refused_and_does_not_hang",
    "orch-envhost lib mcp_host::tests::a_server_that_dies_is_started_again_within_its_budget_and_then_given_up_on",
    "orch-api-core lib sign_in::passwords::tests::a_name_nobody_has_is_checked_with_the_work_of_one_somebody_has",
    "orch-api-core realtime_feeds a_row_that_is_announced_reaches_the_reader_at_once",
    "orch-api-core realtime_feeds a_notification_lost_between_commit_and_fan_out_is_repaired_by_the_next_sweep",
    "orch-api-core realtime_feeds two_server_instances_on_one_database_both_deliver",
    "orch-api-core document_feed a_reader_on_another_process_is_sent_the_same_rows_and_is_shown_who_is_writing",
    "orch-api-core document_readers the_engine_is_told_how_many_people_have_a_document_open_each_counted_once_and_none_who_left",
    "orch-api-core realtime_ephemeral ten_whole_lists_seed_readers_on_two_processes_with_the_last_complete_list",
    "orch-api-core realtime_diagnostics a_providers_row_models_and_permission_page_tell_its_reader_to_reload",
    "orch-api-core realtime_diagnostics every_sign_in_step_reaches_managers_and_no_step_reaches_a_viewer",
    "orch-api-core realtime_environment two_processes_share_one_machine_watch_until_the_last_departure",
    "orch-api-core realtime_environment a_signal_to_a_process_is_followed_by_a_snapshot_without_that_process",
    "orch-api-core realtime_threads deltas_arrive_and_the_stored_text_catches_up_with_the_same_content",
    "orch-api-core realtime_threads the_announcement_that_the_epoch_moved_closes_out_a_reader_at_once_without_any_row_being_written",
    "orch-api-core terminals_sockets a_browser_that_does_not_read_holds_up_its_own_shell_and_nothing_else_and_loses_nothing",
    "orch-engine caps fifty_threads_that_start_at_once_under_a_cap_of_three_make_three_turns",
    "orch-engine caps a_cap_on_turns_at_once_makes_threads_wait_and_releases_them_in_the_order_of_their_oldest_input",
    "orch-engine turns a_harness_that_does_not_answer_an_interrupt_is_forced_after_the_wait",
    "orch-engine usage_ledger a_lease_that_refused_a_model_fails_its_turn_with_that_reason_and_the_thread_goes_on",
    "orch-engine usage_ledger a_lease_that_refused_for_a_cap_fails_its_turn_with_that_reason_and_the_thread_goes_on",
    "orch-engine caps_models a_cap_on_a_provider_that_names_one_model_leaves_its_other_models_alone",
    "orch-engine caps_models a_cap_on_a_provider_that_names_none_covers_the_whole_of_it",
]
# Four commands of other packages that keep the processors busy, as a whole run of the workspace does.
DEFAULT_LOADS = [
    "cargo test -p orch-types",
    "cargo test -p orch-link",
    "cargo test -p orch-api-contract",
    "cargo test -p orch-settings",
]
# How long one run of one candidate may take before it is counted as hanging: a guard, not a bound
# the test is held to (a candidate that needs minutes on a loaded machine still passes).
RUN_TIMEOUT_SECONDS = 30 * 60
# How long compiling a candidate may take on a cold, loaded machine.
BUILD_TIMEOUT_SECONDS = 90 * 60
# A candidate selects one exact case; only its final libtest summary establishes that it ran.
LOG_TAIL_BYTES = 64 * 1024


def one_case_ran(log):
    with open(log, "rb") as file:
        file.seek(0, 2)
        file.seek(max(0, file.tell() - LOG_TAIL_BYTES))
        output = file.read().decode("utf-8", errors="replace")
    output = re.sub(r"\x1b\[[0-9;]*m", "", output)
    return bool(re.search(r"test result: ok\.\s+1 passed; 0 failed; 0 ignored;", output))


def candidate_command(spec, extra):
    package, target, name = spec.split(" ", 2)
    selector = ["--lib"] if target == "lib" else ["--test", target]
    return ["cargo", "test", "-p", package, *selector, *extra, "--", "--exact", name]


def build_command(spec):
    package, target, _ = spec.split(" ", 2)
    selector = ["--lib"] if target == "lib" else ["--test", target]
    return ["cargo", "test", "-p", package, *selector, "--no-run"]


def run(command, wrap, log, timeout):
    started = time.monotonic()
    with open(log, "w", encoding="utf-8") as output:
        process = subprocess.Popen(
            [*wrap, *command], stdout=output, stderr=subprocess.STDOUT, start_new_session=True
        )
        try:
            code = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()
            code = 124
    return code, round(time.monotonic() - started, 2)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--times", type=int, default=20)
    parser.add_argument("--load", action="append", default=None)
    parser.add_argument("--wrap", default="")
    parser.add_argument("candidates", nargs="*")
    arguments = parser.parse_args()
    if arguments.times < 1:
        parser.error("--times must execute at least one actual case")
    candidates = arguments.candidates or DEFAULT_CANDIDATES
    loads = arguments.load or DEFAULT_LOADS
    wrap = shlex.split(arguments.wrap)
    out = arguments.output_dir
    out.mkdir(parents=True, exist_ok=True)

    compiled = set()
    for index, spec in enumerate(candidates):
        selected = tuple(spec.split(" ", 2)[:2])
        if selected in compiled:
            continue
        code, seconds = run(build_command(spec), wrap, out / f"build-{index}.log", BUILD_TIMEOUT_SECONDS)
        if code != 0:
            print(f"did not compile ({code}, {seconds}s): {spec}", file=sys.stderr)
            return 2
        compiled.add(selected)

    stop = threading.Event()
    load_runs = []
    lock = threading.Lock()

    def keep_loading(index, command):
        round_number = 0
        while not stop.is_set():
            code, seconds = run(shlex.split(command), wrap, out / f"load-{index}-{round_number}.log", RUN_TIMEOUT_SECONDS)
            with lock:
                load_runs.append({"load": command, "round": round_number, "exit": code, "seconds": seconds})
            round_number += 1

    threads = [threading.Thread(target=keep_loading, args=(i, c), daemon=True) for i, c in enumerate(loads)]
    for thread in threads:
        thread.start()

    results = []
    try:
        for index, spec in enumerate(candidates):
            runs = []
            for attempt in range(arguments.times):
                log = out / f"run-{index}-{attempt}.log"
                code, seconds = run(candidate_command(spec, []), wrap, log, RUN_TIMEOUT_SECONDS)
                runs.append({"attempt": attempt, "exit": code, "seconds": seconds, "case_ran": one_case_ran(log) if code == 0 else False})
            passed = sum(1 for each in runs if each["exit"] == 0 and each["case_ran"])
            results.append({"candidate": spec, "passed": passed, "times": arguments.times, "runs": runs})
            print(f"{passed}/{arguments.times} {spec}")
    finally:
        stop.set()
        for thread in threads:
            thread.join()

    (out / "results.json").write_text(
        json.dumps({"candidates": results, "loads": load_runs}, indent=2), encoding="utf-8"
    )
    return 0 if all(each["passed"] == each["times"] for each in results) else 1


if __name__ == "__main__":
    sys.exit(main())
