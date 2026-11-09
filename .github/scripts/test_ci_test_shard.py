"""Test plans cover every workspace target once and continue after failures."""

from collections import Counter
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, call, patch

import ci_test_shard
import ci_fixtures


def package(name, tests):
    root = f"/workspace/{name}"
    targets = [{"name": name.replace("-", "_"), "kind": ["lib"], "test": True, "doctest": True,
                "src_path": root + "/src/lib.rs", "crate_types": ["lib"]}]
    targets += [{"name": f"case_{n}", "kind": ["test"], "test": True, "doctest": False,
                 "src_path": root + f"/tests/case_{n}.rs", "crate_types": ["bin"]} for n in range(tests)]
    targets.append({"name": "disabled", "kind": ["test"], "test": False, "doctest": False,
                    "src_path": root + "/tests/disabled.rs", "crate_types": ["bin"]})
    return {"id": name + "#0.1.0", "name": name, "version": "0.1.0", "authors": [],
            "manifest_path": root + "/Cargo.toml", "targets": targets}


class ShardTests(unittest.TestCase):
    def metadata(self):
        return {"packages": [package("orch-api-core", 17), package("orch-engine", 8),
                             *(package(f"other-{n}", n) for n in range(1, 9))],
                "target_directory": "/workspace/target"}

    def test_large_package_targets_are_disjoint_and_cover_every_enabled_test(self):
        metadata = self.metadata()
        for kind, (package_name, count) in ci_test_shard.PARTS.items():
            selected = []
            for index in range(count):
                commands = ci_test_shard.commands(metadata, f"{kind}-{index}")
                integration = commands[0]
                selected += [integration[i + 1] for i, part in enumerate(integration) if part == "--test"]
                self.assertTrue(all("--no-fail-fast" in command for command in commands))
                self.assertEqual(len(commands), 3 if index == 0 else 1)
                if index == 0:
                    self.assertIn("--lib", commands[1])
                    self.assertIn("--doc", commands[2])
            expected = ci_test_shard.integration_targets(next(p for p in metadata["packages"] if p["name"] == package_name))
            self.assertEqual(Counter(selected), Counter(expected))

    def test_only_the_engine_unit_command_uses_its_database_fixture_capacity(self):
        metadata = self.metadata()
        shards = [f"{kind}-{index}" for kind, (_, count) in ci_test_shard.PARTS.items()
                  for index in range(count)]
        shards += [f"rest-{index}" for index in range(ci_test_shard.OTHER_SHARDS)]
        limited = [(shard, command) for shard in shards
                   for command in ci_test_shard.commands(metadata, shard)
                   if any(argument.startswith("--test-threads") for argument in command)]
        self.assertEqual(limited, [("engine-0", [
            "cargo", "test", "--locked", "--no-fail-fast", "-p", "orch-engine",
            "--lib", "--bins", "--examples", "--", f"--test-threads={97 // 32}",
        ])])

    def test_other_packages_are_each_assigned_once(self):
        metadata = self.metadata()
        selected = []
        for index in range(ci_test_shard.OTHER_SHARDS):
            command, = ci_test_shard.commands(metadata, f"rest-{index}")
            self.assertIn("--no-fail-fast", command)
            selected += [command[i + 1] for i, part in enumerate(command) if part == "-p"]
        expected = [p["name"] for p in metadata["packages"] if p["name"].startswith("other-")]
        self.assertEqual(Counter(selected), Counter(expected))

    def test_one_build_keeps_every_original_package_and_target_selection(self):
        metadata = self.metadata()
        shards = [f"{kind}-{index}" for kind, (_, count) in ci_test_shard.PARTS.items()
                  for index in range(count)] + [f"rest-{index}" for index in range(ci_test_shard.OTHER_SHARDS)]
        for shard in shards:
            with self.subTest(shard=shard):
                original = ci_test_shard.commands(metadata, shard)
                build, after = ci_test_shard.execution_plan(metadata, shard)
                self.assertEqual(build.count("--no-run"), 1)
                self.assertIn("--message-format=json", build)
                self.assertNotIn("--keep-going", build, "cargo test rejects the build-only keep-going flag")
                self.assertIn("--no-fail-fast", build)
                self.assertNotIn("--test-threads=3", build)
                for selector in ("-p", "--test"):
                    old = [original[0][i + 1] for i, flag in enumerate(original[0][:-1]) if flag == selector]
                    new = [build[i + 1] for i, flag in enumerate(build[:-1]) if flag == selector]
                    self.assertEqual(new, old)
                if shard in {"api-0", "engine-0"}:
                    self.assertTrue(all(flag in build for flag in ("--lib", "--bins", "--examples")))
                    self.assertEqual(after, original[1:] if shard == "engine-0" else original[2:])

    def test_rest_doctests_are_not_lost_or_requested_for_binary_only_packages(self):
        metadata = self.metadata()
        metadata["packages"][-1]["targets"] = [{"name": "only_bin", "kind": ["bin"], "test": True, "doctest": False}]
        documented = []
        for index in range(ci_test_shard.OTHER_SHARDS):
            _, after = ci_test_shard.execution_plan(metadata, f"rest-{index}")
            for command in after:
                self.assertEqual(command[:4], ["cargo", "test", "--locked", "--no-fail-fast"])
                self.assertEqual(command[-1], "--doc")
                documented += [command[i + 1] for i, flag in enumerate(command[:-1]) if flag == "-p"]
        self.assertEqual(Counter(documented), Counter(f"other-{index}" for index in range(1, 8)))

    def test_artifacts_ignore_build_tools_and_dependencies_and_keep_each_selected_test_once(self):
        metadata = self.metadata()
        package = metadata["packages"][0]
        unit, case = package["targets"][:2]
        program = {"name": "fixture-cli", "kind": ["bin"], "test": True, "doctest": False,
                   "src_path": "/workspace/orch-api-core/src/main.rs", "crate_types": ["bin"]}
        def artifact(target, executable, tested=True, owner=package):
            return {"reason": "compiler-artifact", "package_id": owner["id"], "target": target,
                    "profile": {"test": tested}, "executable": executable}
        messages = [artifact(unit, "/workspace/target/debug/deps/units"),
                    artifact(case, "/workspace/target/debug/deps/case_0"),
                    artifact(case, "/workspace/target/debug/deps/case_0"),
                    artifact(unit, None, False),
                    artifact(program, "/workspace/target/debug/fixture-cli", False),
                    artifact(metadata["packages"][1]["targets"][0], "/unselected", owner=metadata["packages"][1]),
                    {"reason": "build-script-executed", "package_id": package["id"],
                     "env": [["DECLARED_BY_BUILD", "present"]], "linked_paths": []},
                    {"reason": "build-finished", "success": False}]
        process = MagicMock()
        process.__enter__.return_value = process
        process.stdout = iter(json.dumps(message) + "\n" for message in messages)
        process.wait.return_value = 1
        build, _ = ci_test_shard.execution_plan(metadata, "api-0")
        with patch.object(ci_test_shard.subprocess, "Popen", return_value=process), \
                patch.object(Path, "read_text", return_value='[package]\nname="fixture"\n'):
            code, binaries, scripts, programs = ci_test_shard.build_binaries(metadata, "api-0", build)
        self.assertEqual(code, 1)
        self.assertEqual([binary["executable"] for binary in binaries], [
            "/workspace/target/debug/deps/units", "/workspace/target/debug/deps/case_0"])
        self.assertEqual(scripts[package["id"]]["env"], [["DECLARED_BY_BUILD", "present"]])
        self.assertEqual(programs[package["id"]], {"fixture-cli": "/workspace/target/debug/fixture-cli"})

    def test_direct_execution_retains_package_binary_library_and_fixture_context(self):
        metadata = self.metadata()
        package = metadata["packages"][0]
        binary = {"package": package, "target": package["targets"][1],
                  "executable": "/workspace/target/debug/deps/case_0", "harness": True}
        scripts = {package["id"]: {"env": [["DECLARED_BY_BUILD", "yes"]],
                                   "linked_paths": ["native=/workspace/target/debug/build/native/out", "native=/outside"]}}
        with patch.dict(os.environ, {"ORCH_TEST_DATABASE_URL": "postgres://fixture", "RUST_MIN_STACK": "33554432",
                                     "LD_LIBRARY_PATH": "/existing"}):
            environment = ci_test_shard.binary_environment(binary, metadata, scripts,
                {package["id"]: {"fixture-cli": "/workspace/target/debug/fixture-cli"}}, "/rust/lib")
        self.assertEqual(environment["ORCH_TEST_DATABASE_URL"], "postgres://fixture")
        self.assertEqual(environment["RUST_MIN_STACK"], "33554432")
        self.assertEqual(environment["CARGO_MANIFEST_DIR"], "/workspace/orch-api-core")
        self.assertEqual(environment["CARGO_MANIFEST_PATH"], package["manifest_path"])
        self.assertEqual(environment["CARGO_BIN_EXE_fixture-cli"], "/workspace/target/debug/fixture-cli")
        self.assertEqual(environment["DECLARED_BY_BUILD"], "yes")
        self.assertEqual(environment["LD_LIBRARY_PATH"].split(os.pathsep), [
            "/workspace/target/debug/build/native/out", "/workspace/target/debug/deps",
            "/workspace/target/debug", "/rust/lib", "/existing"])

    def test_timeout_names_only_reported_running_cases_and_retains_queued_diagnostics(self):
        progress = ci_test_shard.Progress()
        for case in ("completed", "hanging", "queued"):
            progress.listing(case + ": test")
        progress.observe("test completed ... ok")
        progress.observe("test hanging has been running for over 60 seconds")
        output = io.StringIO()
        with patch("sys.stdout", output):
            progress.failure("adapter::scale", 2700, "exceeded 2700s")
        text = output.getvalue()
        self.assertIn("test hanging ... FAILED", text)
        self.assertIn("still-running cases reported by the harness: hanging", text)
        self.assertIn("not completed (running or queued): queued", text)
        self.assertNotIn("test queued ... FAILED", text)
        self.assertIn("test result: FAILED. 1 passed; 1 failed", text)
        self.assertIn("adapter::scale", text)

    def test_stop_kills_descendants_even_after_their_parent_exits_on_term(self):
        process = SimpleNamespace(pid=12345, wait=MagicMock(return_value=0))
        with patch.object(ci_test_shard.os, "killpg") as kill:
            ci_test_shard.stop_group(process)
        self.assertEqual(kill.call_args_list, [call(12345, signal.SIGTERM), call(12345, signal.SIGKILL)])
        self.assertEqual(process.wait.call_args_list, [call(timeout=ci_test_shard.STOP_GRACE_SECONDS), call()])

    def test_nonpositive_or_nonfinite_timeouts_are_refused(self):
        for value in ("0", "-1", "nan", "inf", "not-a-number"):
            with self.subTest(value=value), self.assertRaises(ci_test_shard.argparse.ArgumentTypeError):
                ci_test_shard.positive_seconds(value)
        self.assertEqual(ci_test_shard.positive_seconds("1.5"), 1.5)

    def test_a_binary_timeout_keeps_the_cargo_log_shape_and_does_not_retry_the_case(self):
        metadata = self.metadata()
        package = metadata["packages"][0]
        binary = {"package": package, "target": package["targets"][1],
                  "executable": "/workspace/target/debug/deps/case_0", "harness": True}
        def execute(command, cwd, environment, timeout, observe, echo=True):
            self.assertEqual(cwd, Path(package["manifest_path"]).parent)
            if "--list" in command:
                observe("hanging: test")
                return 0, False, 0.001
            observe("test hanging has been running for over 60 seconds")
            return -signal.SIGKILL, True, timeout
        output = io.StringIO()
        with patch.object(ci_test_shard, "execute", side_effect=execute) as run, patch("sys.stdout", output):
            self.assertFalse(ci_test_shard.run_binary(binary, metadata, {}, {}, "/rust/lib", 2700))
        self.assertEqual(run.call_count, 2, "one listing and one execution, never a retry")
        self.assertEqual(run.call_args_list[1].args[0], [binary["executable"]])
        self.assertIn("Running tests/case_0.rs (", output.getvalue())
        self.assertIn("test hanging ... FAILED", output.getvalue())
        self.assertIn("test result: FAILED.", output.getvalue())

    def test_a_custom_harness_is_run_once_without_a_libtest_listing(self):
        metadata = self.metadata()
        package = metadata["packages"][0]
        binary = {"package": package, "target": package["targets"][1],
                  "executable": "/workspace/target/debug/deps/custom", "harness": False}
        with patch.object(ci_test_shard, "execute", return_value=(0, False, 0.01)) as run, \
                patch("sys.stdout", io.StringIO()):
            self.assertTrue(ci_test_shard.run_binary(binary, metadata, {}, {}, "/rust/lib", 2700))
        self.assertEqual(run.call_count, 1)
        self.assertEqual(run.call_args.args[0], [binary["executable"]])

    def test_a_slow_case_that_finishes_is_not_reported_still_running(self):
        progress = ci_test_shard.Progress()
        progress.listing("earlier: test")
        progress.listing("later: test")
        progress.observe("test earlier has been running for over 60 seconds")
        progress.observe("test earlier ... ok")
        progress.observe("test later has been running for over 60 seconds")
        output = io.StringIO()
        with patch("sys.stdout", output):
            progress.failure("adapter::next", 2700, "exceeded 2700s")
        self.assertIn("still-running cases reported by the harness: later", output.getvalue())
        self.assertNotIn("test earlier ... FAILED", output.getvalue())

    def test_progress_parsing_is_bounded_without_truncating_the_forwarded_log(self):
        lines = []
        console = io.StringIO()
        with patch("sys.stdout", console):
            output = ci_test_shard.Output(lines.append)
            output.write(b"x" * ci_test_shard.PROGRESS_LINE_LIMIT)
            output.write(b"x\n")
            output.write(b"test next ... ok\n")
            output.write(b"", final=True)
        self.assertEqual(lines, ["test next ... ok"])
        self.assertEqual(len(console.getvalue()), ci_test_shard.PROGRESS_LINE_LIMIT + len("x\ntest next ... ok\n"))

    @unittest.skipUnless(sys.platform == "linux", "the shard workflow uses Linux process groups")
    def test_a_real_timeout_kills_its_term_ignoring_child_group_and_returns(self):
        child = ("import os,signal,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                 "print('child-ready='+str(os.getpid()),flush=True); time.sleep(60)")
        parent = ("import os,signal,subprocess,sys,time; signal.signal(signal.SIGTERM,signal.SIG_IGN); "
                  "print('parent='+str(os.getpid()),flush=True); "
                  f"subprocess.Popen([sys.executable,'-I','-c',{child!r}]); time.sleep(60)")
        lines = []
        try:
            with patch.object(ci_test_shard, "STOP_GRACE_SECONDS", 0.1):
                code, expired, _ = ci_test_shard.execute(
                    [sys.executable, "-I", "-c", parent], Path.cwd(), dict(os.environ), 5, lines.append, echo=False)
            self.assertTrue(expired)
            self.assertEqual(code, -signal.SIGKILL)
            ready = [int(line.partition("=")[2]) for line in lines if line.startswith("child-ready=")]
            self.assertEqual(len(ready), 1, "the child really started and installed its TERM handler")
            state = Path(f"/proc/{ready[0]}/stat")
            until = time.monotonic() + 1
            while state.exists() and time.monotonic() < until:
                try:
                    if state.read_text().rpartition(") ")[2].split()[0] == "Z":
                        break
                except FileNotFoundError:
                    break
                time.sleep(0.01)
            try:
                last = state.read_text().rpartition(") ")[2].split()[0]
            except FileNotFoundError:
                pass
            else:
                self.assertEqual(last, "Z",
                                 "an ignored TERM cannot leave the child running after the binary timeout")
        finally:
            # The observed group belongs only to the Python fixture started above.
            for line in lines:
                if line.startswith("parent="):
                    try:
                        os.killpg(int(line.partition("=")[2]), signal.SIGKILL)
                    except ProcessLookupError:
                        pass

    @unittest.skipUnless(sys.platform == "linux", "the shard workflow uses Linux process groups")
    def test_a_binary_that_closes_its_output_still_has_a_deadline(self):
        script = "import os,time; print('test running ...',flush=True); os.close(1); os.close(2); time.sleep(60)"
        lines = []
        with patch.object(ci_test_shard, "STOP_GRACE_SECONDS", 0.1):
            _, expired, _ = ci_test_shard.execute(
                [sys.executable, "-I", "-c", script], Path.cwd(), dict(os.environ), 5, lines.append, echo=False)
        self.assertTrue(expired)
        self.assertEqual(lines, ["test running ..."])

    def test_native_engine_targets_prepare_the_pinned_harness(self):
        ordinary = [["cargo", "test", "-p", "orch-engine", "--test", "failures"]]
        self.assertEqual(ci_fixtures.fixture_plan(ordinary), ([], False))
        for target in ("trace_walk", "native_executor_loss", "background_wake_native"):
            with self.subTest(target=target):
                native = [["cargo", "test", "-p", "orch-engine", "--test", target]]
                self.assertEqual(ci_fixtures.fixture_plan(native), ([], True))

    def test_invalid_shards_are_refused(self):
        for shard in ("api-6", "engine-3", "rest-4", "other-0", "api-x"):
            with self.subTest(shard=shard), self.assertRaises(ValueError):
                ci_test_shard.commands(self.metadata(), shard)

    def test_a_failed_program_does_not_hide_the_unit_or_documentation_tests(self):
        metadata = SimpleNamespace(stdout=json.dumps(self.metadata()))
        results = [metadata, SimpleNamespace(stdout="/toolchain/lib"), SimpleNamespace(returncode=0)]
        binaries = [{"name": "integration"}, {"name": "unit"}]
        with patch("sys.argv", ["ci_test_shard.py", "--shard", "api-0"]), \
                patch.object(ci_test_shard, "build_binaries", return_value=(0, binaries, {}, {})), \
                patch.object(ci_test_shard, "run_binary", side_effect=[False, True]) as binary, \
                patch.object(ci_test_shard.subprocess, "run", side_effect=results) as run:
            self.assertEqual(ci_test_shard.main(), 1)
            self.assertEqual(binary.call_count, 2)
            self.assertEqual(run.call_count, 3)
            self.assertEqual(run.call_args.args[0], [
                "cargo", "test", "--locked", "--no-fail-fast", "-p", "orch-api-core", "--doc"])


if __name__ == "__main__":
    unittest.main()
