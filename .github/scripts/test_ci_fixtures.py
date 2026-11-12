"""Native fixture selection and the static runner's Cargo/environment handoff."""

import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, call, patch

import ci_fixtures


class NativeFixtures(unittest.TestCase):
    def test_node_conformance_and_default_package_tests_need_a_static_runner_and_the_harness(self):
        for arguments in (("--test", "conformance"), (), ("--tests",), ("--all-targets",)):
            with self.subTest(arguments=arguments):
                command = ["cargo", "test", "--locked", "-p", "orch-node", *arguments]
                self.assertTrue(ci_fixtures.needs_conformance_runner([command]))
                images = [] if arguments == ("--test", "conformance") else [(ci_fixtures.BUSYBOX_IMAGE, "busybox:latest")]
                self.assertEqual(ci_fixtures.fixture_plan([command]), (images, True))

    def test_each_native_node_lab_target_prepares_its_connector_and_busybox_without_conformance(self):
        for target in ci_fixtures.NODE_LAB_TESTS:
            with self.subTest(target=target):
                planned = [["cargo", "test", "-p", "orch-node", "--test", target]]
                self.assertEqual(ci_fixtures.describe_fixtures(planned), {
                    "images": [(ci_fixtures.BUSYBOX_IMAGE, "busybox:latest")], "harness": False,
                    "node_runner": False, "node_connector": True,
                })

    def test_node_fixture_selection_covers_every_actual_lab_consumer(self):
        directory = Path(__file__).resolve().parents[2] / "apps/node/tests"
        consumers = {path.stem for path in directory.glob("*.rs") if "Lab::start()" in path.read_text()}
        self.assertEqual(ci_fixtures.NODE_LAB_TESTS, consumers)

    def test_library_other_targets_and_other_commands_cannot_borrow_the_conformance_requirement(self):
        for arguments in (("--lib",), ("--doc",), ("--bins",), ("--test", "drivers"),
                          ("--lib", "--", "--test", "conformance")):
            with self.subTest(arguments=arguments):
                planned = [["cargo", "test", "-p", "orch-node", *arguments],
                           ["cargo", "test", "-p", "orch-model-contract", "--test", "conformance", "--test", "environment"]]
                self.assertFalse(ci_fixtures.needs_conformance_runner(planned))
                self.assertFalse(ci_fixtures.needs_node_connector(planned))
                self.assertEqual(ci_fixtures.fixture_plan(planned), ([], False))

    def test_a_targeted_plan_exposes_the_prerequisite_without_starting_any_tool(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            targets = Path(directory) / "targets"
            targets.write_text("# native check\ntest -p orch-node --test conformance\nweb typecheck\n", encoding="utf-8")
            output = io.StringIO()
            with patch.object(sys, "argv", ["ci_fixtures.py", "--targets", str(targets), "--plan"]), \
                    patch.object(ci_fixtures.subprocess, "run", side_effect=AssertionError("unexpected tool")), \
                    patch.object(ci_fixtures.subprocess, "Popen", side_effect=AssertionError("unexpected build")), \
                    contextlib.redirect_stdout(output):
                self.assertEqual(ci_fixtures.main(), 0)
            self.assertEqual(json.loads(output.getvalue()), {"images": [], "harness": True, "node_runner": True, "node_connector": False})

    def cargo_process(self, messages, returncode=0):
        process = MagicMock()
        process.__enter__.return_value = process
        process.stdout = io.StringIO("".join(json.dumps(message) + "\n" for message in messages))
        process.wait.return_value = returncode
        return process

    def artifact(self, executable, *, name="orch-node", kind=None, tested=False):
        return {"reason": "compiler-artifact", "target": {"name": name, "kind": kind or ["bin"]},
                "profile": {"test": tested}, "executable": str(executable)}

    def test_build_uses_the_reported_executable_and_a_native_musl_compiler(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            runner = Path(directory) / "custom target" / "runner"
            runner.parent.mkdir()
            runner.write_text("protocol fixture only", encoding="utf-8")
            runner.chmod(0o755)
            process = self.cargo_process([
                self.artifact("/not-the-runner", name="build-script-build"),
                self.artifact("/node-unit-tests", tested=True),
                self.artifact("/node-library", kind=["lib"]),
                self.artifact(runner),
                {"reason": "build-finished", "success": True},
            ])
            with patch.dict(os.environ, {}, clear=True), \
                    patch.object(ci_fixtures.platform, "machine", return_value="x86_64"), \
                    patch.object(ci_fixtures.subprocess, "Popen", return_value=process) as build:
                self.assertEqual(ci_fixtures.conformance_runner(), str(runner.resolve()))
            self.assertEqual(build.call_args.args[0], [
                "cargo", "build", "--locked", "--target", "x86_64-unknown-linux-musl",
                "-p", "orch-node", "--bin", "orch-node", "--message-format=json",
            ])
            self.assertEqual(build.call_args.kwargs["env"]["CC_x86_64_unknown_linux_musl"], "musl-gcc")
            process.wait.assert_called_once_with()

    def test_build_preserves_an_explicit_compiler_and_selects_the_native_arm_target(self):
        process = self.cargo_process([self.artifact("/reported/node")])
        with patch.dict(os.environ, {"CC_aarch64_unknown_linux_musl": "custom-musl-cc"}, clear=True), \
                patch.object(ci_fixtures.platform, "machine", return_value="aarch64"), \
                patch.object(ci_fixtures.subprocess, "Popen", return_value=process) as build, \
                patch.object(ci_fixtures, "executable_path", return_value="/reported/node"):
            self.assertEqual(ci_fixtures.conformance_runner(), "/reported/node")
        command = build.call_args.args[0]
        self.assertEqual(command[command.index("--target") + 1], "aarch64-unknown-linux-musl")
        self.assertEqual(build.call_args.kwargs["env"]["CC_aarch64_unknown_linux_musl"], "custom-musl-cc")

    def test_connector_build_selects_its_own_actual_artifact_instead_of_the_conformance_node(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            connector = Path(directory) / "static-connector"
            connector.write_text("protocol fixture only", encoding="utf-8")
            connector.chmod(0o755)
            process = self.cargo_process([self.artifact("/unrelated/node"), self.artifact(connector, name="orch-connector")])
            with patch.dict(os.environ, {}, clear=True), \
                    patch.object(ci_fixtures.platform, "machine", return_value="x86_64"), \
                    patch.object(ci_fixtures.subprocess, "Popen", return_value=process) as build:
                self.assertEqual(ci_fixtures.node_connector(), str(connector.resolve()))
            self.assertEqual(build.call_args.args[0], [
                "cargo", "build", "--locked", "--target", "x86_64-unknown-linux-musl",
                "-p", "orch-connector", "--bin", "orch-connector", "--message-format=json",
            ])

    def test_failed_or_incomplete_builds_never_become_a_runner(self):
        for messages, returncode, error in (([self.artifact("/stale/node")], 1, subprocess.CalledProcessError),
                                            ([], 0, RuntimeError),
                                            ([self.artifact("/absent/node")], 0, FileNotFoundError)):
            for prepare in (ci_fixtures.conformance_runner, ci_fixtures.node_connector):
                selected = [dict(message, target={**message["target"], "name": "orch-connector"})
                            if prepare == ci_fixtures.node_connector else message for message in messages]
                with self.subTest(returncode=returncode, messages=selected, prepare=prepare.__name__), \
                        patch.dict(os.environ, {}, clear=True), \
                        patch.object(ci_fixtures.platform, "machine", return_value="x86_64"), \
                        patch.object(ci_fixtures.subprocess, "Popen", return_value=self.cargo_process(selected, returncode)):
                    with self.assertRaises(error):
                        prepare()

    def test_an_explicit_runner_is_reused_but_never_silently_replaced_if_missing(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            runner = Path(directory) / "runner"
            runner.write_text("protocol fixture only", encoding="utf-8")
            runner.chmod(0o755)
            for variable, prepare in (("ORCH_TEST_CONFORMANCE_NODE", ci_fixtures.conformance_runner),
                                      ("ORCH_TEST_CONNECTOR_BINARY", ci_fixtures.node_connector)):
                with self.subTest(variable=variable), patch.dict(os.environ, {variable: str(runner)}), \
                        patch.object(ci_fixtures.subprocess, "Popen", side_effect=AssertionError("unexpected build")):
                    self.assertEqual(prepare(), str(runner.resolve()))
                with patch.dict(os.environ, {variable: str(runner / "missing")}), \
                        patch.object(ci_fixtures.subprocess, "Popen", side_effect=AssertionError("unexpected fallback")):
                    with self.assertRaises(OSError):
                        prepare()

    def test_preparation_returns_the_runner_and_still_builds_the_pinned_harness(self):
        planned = [["cargo", "test", "-p", "orch-node", "--test", "conformance"]]
        with patch.object(ci_fixtures.subprocess, "run") as docker, \
                patch.object(ci_fixtures, "conformance_runner", return_value="/reported/node") as runner:
            environment = ci_fixtures.prepare_fixtures(planned)
        self.assertEqual(environment, {"ORCH_TEST_CONFORMANCE_NODE": "/reported/node"})
        runner.assert_called_once_with()
        command = docker.call_args.args[0]
        self.assertEqual(command[:4], ["docker", "build", "--tag", "orch-dev-harness"])
        self.assertTrue(any(argument.startswith("CODEX_VERSION=") for argument in command))
        self.assertTrue(any(argument.startswith("CLAUDE_VERSION=") for argument in command))

    def test_unrelated_tests_do_not_build_a_static_runner(self):
        with patch.object(ci_fixtures.subprocess, "run", side_effect=AssertionError("unexpected image")), \
                patch.object(ci_fixtures, "node_connector", side_effect=AssertionError("unexpected connector")), \
                patch.object(ci_fixtures, "conformance_runner", side_effect=AssertionError("unexpected runner")):
            self.assertEqual(ci_fixtures.prepare_fixtures([["cargo", "test", "-p", "orch-node", "--lib"]]), {})

    def test_a_single_node_lab_target_prepares_and_exports_its_real_connector(self):
        planned = [["cargo", "test", "-p", "orch-node", "--test", "milestone"]]
        with patch.object(ci_fixtures.subprocess, "run") as docker, \
                patch.object(ci_fixtures, "node_connector", return_value="/reported/connector") as connector, \
                patch.object(ci_fixtures, "conformance_runner", side_effect=AssertionError("unselected node build")):
            self.assertEqual(ci_fixtures.prepare_fixtures(planned), {"ORCH_TEST_CONNECTOR_BINARY": "/reported/connector"})
        connector.assert_called_once_with()
        self.assertEqual(docker.call_args_list, [call(["docker", "pull", ci_fixtures.BUSYBOX_IMAGE], check=True),
                                              call(["docker", "image", "tag", ci_fixtures.BUSYBOX_IMAGE, "busybox:latest"], check=True)])

    def test_unfiltered_node_tests_keep_both_static_fixtures_and_one_busybox_pull(self):
        planned = [["cargo", "test", "-p", "orch-node", "-p", "orch-driver-docker"]]
        with patch.object(ci_fixtures.subprocess, "run") as docker, \
                patch.object(ci_fixtures, "node_connector", return_value="/reported/connector") as connector, \
                patch.object(ci_fixtures, "conformance_runner", return_value="/reported/node") as runner:
            self.assertEqual(ci_fixtures.prepare_fixtures(planned), {
                "ORCH_TEST_CONFORMANCE_NODE": "/reported/node", "ORCH_TEST_CONNECTOR_BINARY": "/reported/connector",
            })
        connector.assert_called_once_with(); runner.assert_called_once_with()
        self.assertEqual(sum(item.args[0] == ["docker", "pull", ci_fixtures.BUSYBOX_IMAGE] for item in docker.call_args_list), 1)

    def test_a_failed_required_connector_build_cannot_publish_an_environment_or_a_green_skip(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            targets, output = Path(directory) / "targets", Path(directory) / "environment"
            targets.write_text("test -p orch-node --test environment\n", encoding="utf-8")
            output.write_text("EXISTING=kept\n", encoding="utf-8")
            with patch.object(sys, "argv", ["ci_fixtures.py", "--targets", str(targets)]), \
                    patch.dict(os.environ, {"GITHUB_ENV": str(output)}), \
                    patch.object(ci_fixtures.subprocess, "run"), \
                    patch.object(ci_fixtures, "node_connector", side_effect=subprocess.CalledProcessError(1, ["static-fixture-build"])):
                with self.assertRaises(subprocess.CalledProcessError):
                    ci_fixtures.main()
            self.assertEqual(output.read_text(), "EXISTING=kept\n")

    def test_whole_ci_exports_the_prepared_runner_for_the_next_test_step(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            targets, output = Path(directory) / "targets", Path(directory) / "environment"
            targets.write_text("test -p orch-node --test conformance\n", encoding="utf-8")
            output.write_text("EXISTING=kept\n", encoding="utf-8")
            with patch.object(sys, "argv", ["ci_fixtures.py", "--targets", str(targets)]), \
                    patch.dict(os.environ, {"GITHUB_ENV": str(output)}), \
                    patch.object(ci_fixtures, "prepare_fixtures", return_value={"ORCH_TEST_CONFORMANCE_NODE": "/reported/node", "ORCH_TEST_CONNECTOR_BINARY": "/reported/connector"}):
                self.assertEqual(ci_fixtures.main(), 0)
            self.assertEqual(output.read_text(), "EXISTING=kept\nORCH_TEST_CONFORMANCE_NODE=/reported/node\nORCH_TEST_CONNECTOR_BINARY=/reported/connector\n")


if __name__ == "__main__":
    unittest.main()
