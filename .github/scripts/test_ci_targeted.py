"""The two targeted jobs preserve every selected check without sharing a Rust runner with web work."""

from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch

import ci_targeted


class TargetedJobs(unittest.TestCase):
    def invoke(self, lines, group):
        with tempfile.TemporaryDirectory() as directory:
            targets = Path(directory) / "targets"
            targets.write_text(lines, encoding="utf-8")
            with patch.object(ci_targeted, "run", return_value=True) as run, \
                    patch.object(ci_targeted, "prepare_native_fixtures", return_value=(True, {})) as prepare:
                result = ci_targeted.main(targets, group)
        return result, run.call_args_list, prepare.call_args_list

    def test_independent_jobs_keep_every_target_and_web_never_starts_rust_or_fixtures(self):
        targets = "test -p orch-api-core --test skills\ncontracts\nweb typecheck\nweb lint\nweb-e2e tests/e2e/history.spec.ts\n"
        rust_result, rust, rust_fixtures = self.invoke(targets, "rust")
        web_result, web, web_fixtures = self.invoke(targets, "web")
        self.assertEqual((rust_result, web_result), (0, 0))
        rust_labels = [call.args[0] for call in rust]
        web_labels = [call.args[0] for call in web]
        self.assertEqual(len(rust_fixtures), 1)
        self.assertEqual(web_fixtures, [])
        self.assertIn("test -p orch-api-core --test skills", rust_labels)
        self.assertIn("contracts", rust_labels)
        self.assertEqual(web_labels, ["npm ci", "web typecheck", "web lint", "install the browser", "web-e2e tests/e2e/history.spec.ts"])
        self.assertFalse(set(rust_labels) & {"web typecheck", "web lint", "install the browser"})
        self.assertTrue(all(call.args[1][0] not in {"cargo", ci_targeted.sys.executable} for call in web))
        self.assertEqual(sum(label == "npm ci" for label in web_labels), 1)

    def test_group_with_no_selected_work_succeeds_without_starting_any_tool(self):
        for targets, group in (("web typecheck\n", "rust"), ("fmt\n", "web")):
            result, calls, fixtures = self.invoke(targets, group)
            self.assertEqual(result, 0)
            self.assertEqual(calls, [])
            self.assertEqual(fixtures, [])

    def test_unknown_or_empty_targets_fail_before_any_job_can_hide_them(self):
        for targets in ("# no work\n", "web typecheck\nunknown check\n"):
            for group in ("all", "rust", "web"):
                result, calls, fixtures = self.invoke(targets, group)
                self.assertEqual(result, 2)
                self.assertEqual(calls, [])
                self.assertEqual(fixtures, [])

    def test_every_targeted_test_receives_the_prepared_native_runner_in_the_same_process(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            targets = Path(directory) / "targets"
            targets.write_text("test -p orch-node --test conformance\ntest -p orch-api-core --test forks\n", encoding="utf-8")
            environment = {"ORCH_TEST_CONFORMANCE_NODE": "/reported/static-node"}
            with patch.object(ci_targeted.FIXTURES, "prepare_fixtures", return_value=environment) as prepare, \
                    patch.object(ci_targeted, "run", return_value=True) as run:
                self.assertEqual(ci_targeted.main(targets, "rust"), 0)
            prepare.assert_called_once_with([
                ["cargo", "test", "-p", "orch-node", "--test", "conformance"],
                ["cargo", "test", "-p", "orch-api-core", "--test", "forks"],
            ])
            tests = [call for call in run.call_args_list if call.args[1][:2] == ["cargo", "test"]]
            self.assertEqual(len(tests), 2)
            for call in tests:
                self.assertEqual(call.kwargs["env"], environment)

    def test_a_fixture_failure_keeps_the_targeted_run_red_and_preserves_its_selected_checks(self):
        with tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
            targets = Path(directory) / "targets"
            targets.write_text("test -p orch-node --test conformance\n", encoding="utf-8")
            errors = [subprocess.CalledProcessError(1, ["fixture-build"]),
                      ValueError("invalid fixture manifest"), KeyError("missing pinned version")]
            for error in errors:
                with self.subTest(error=type(error).__name__), \
                        patch.object(ci_targeted.FIXTURES, "prepare_fixtures", side_effect=error), \
                        patch.object(ci_targeted, "run", return_value=True) as run:
                    self.assertEqual(ci_targeted.main(targets, "rust"), 1)
                self.assertTrue(any(call.args[1][:2] == ["cargo", "test"] for call in run.call_args_list))


if __name__ == "__main__":
    unittest.main()
