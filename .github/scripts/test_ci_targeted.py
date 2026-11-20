"""The two targeted jobs preserve every selected check without sharing a Rust runner with web work."""

from contextlib import redirect_stderr, redirect_stdout
from io import StringIO
from pathlib import Path
import tempfile
import subprocess
import unittest
from unittest.mock import patch

import ci_targeted


class TargetedJobs(unittest.TestCase):
    def invoke(self, lines, group, **options):
        with tempfile.TemporaryDirectory() as directory:
            targets = Path(directory) / "targets"
            targets.write_text(lines, encoding="utf-8")
            with patch.object(ci_targeted, "run", return_value=True) as run, \
                    patch.object(ci_targeted, "prepare_native_fixtures", return_value=(True, {})) as prepare:
                result = ci_targeted.main(targets, group, **options)
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
        self.assertEqual(web_labels, ["npm ci", "web typecheck", "web lint", "choose package mirrors that answer",
                                      "install the browser", "web-e2e tests/e2e/history.spec.ts"])
        self.assertFalse(set(rust_labels) & {"web typecheck", "web lint", "install the browser"})
        self.assertTrue(all(call.args[1][0] not in {"cargo", ci_targeted.sys.executable} for call in web))
        self.assertEqual(sum(label == "npm ci" for label in web_labels), 1)

    def test_release_checks_can_run_without_publishing_a_tag(self):
        targets = "release-checks\nrelease-first-start\n"
        result, checks, fixtures = self.invoke(targets, "rust")
        self.assertEqual(result, 0)
        self.assertFalse(fixtures)
        self.assertEqual([call.args[1][-1] for call in checks], [
            "test_release_*.py", ".github/scripts/workflow_lint.py",
            "scripts/smoke/update-doctor/run.sh", "scripts/smoke/update-files/run.sh",
            "--skip-image", "scripts/smoke/first-start/run.sh",
        ])
        self.assertEqual(self.invoke(targets, "web"), (0, [], []))

    def test_the_installer_uses_the_same_runs_native_programs_on_a_fresh_runner(self):
        targets = "release-checks\nrelease-first-start\n"
        result, build, _ = self.invoke(targets, "rust", prepare_first_start=True)
        self.assertEqual(result, 0)
        commands = [call.args[1] for call in build]
        cli = next(cmd for cmd in commands if "--cli-only" in cmd)
        native = next(cmd for cmd in commands if "--native-connector" in cmd)
        self.assertFalse(any("scripts/smoke/first-start/run.sh" in cmd for cmd in commands))
        result, install, _ = self.invoke(targets, "install")
        self.assertEqual(result, 0)
        package, scenario = install
        command = package.args[1]
        self.assertEqual(command[command.index("--cli") + 1], str(Path(cli[cli.index("--out") + 1]) / "orchctl"))
        self.assertEqual(command[command.index("--connectors") + 1], native[native.index("--out") + 1])
        self.assertEqual(scenario.args[1], ["bash", "scripts/smoke/first-start/run.sh"])
        self.assertEqual(scenario.kwargs["env"]["ORCH_SMOKE_RELEASE_A"], command[command.index("--out") + 1])
        self.assertEqual(self.invoke("release-checks\n", "install"), (0, [], []))

    def test_only_an_assigned_installation_starts_the_release_builders(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "output"
            with patch.dict(ci_targeted.os.environ, {"GITHUB_OUTPUT": str(output)}):
                result, checks, _ = self.invoke("fmt\n# release-first-start\n", "rust", defer_first_start=True)
                self.assertEqual(result, 0)
                self.assertEqual([call.args[0] for call in checks], ["fmt"])
                self.assertFalse(output.exists())

                result, checks, _ = self.invoke("fmt\n  release-first-start  \n", "rust", defer_first_start=True)
                self.assertEqual(result, 0)
                self.assertEqual([call.args[0] for call in checks], ["fmt"])
                self.assertEqual(output.read_text().splitlines(), ["first_start=true"])

        with patch.dict(ci_targeted.os.environ, {"GITHUB_OUTPUT": ""}), redirect_stderr(StringIO()):
            self.assertEqual(self.invoke("release-first-start\n", "rust", defer_first_start=True), (2, [], []))

        result, inputs, _ = self.invoke("fmt\nrelease-first-start\n", "inputs")
        self.assertEqual(result, 0)
        self.assertEqual([call.args[0] for call in inputs],
                         ["build the installation CLI", "build the Linux connector and node"])
        self.assertEqual(self.invoke("fmt\n", "inputs"), (0, [], []))

    def test_the_installer_builds_its_cli_and_server_with_the_release_memory_settings(self):
        workflows = Path(__file__).resolve().parents[1] / "workflows"
        target_jobs = (workflows / "ci-targeted.yml").read_text().splitlines()
        release_jobs = (workflows / "release.yml").read_text().splitlines()

        def job(lines, name):
            start = lines.index(f"  {name}:") + 1
            end = next((i for i in range(start, len(lines)) if lines[i].startswith("  ") and
                        not lines[i].startswith("    ")), len(lines))
            return "\n".join(lines[start:end])

        for build, release in (("first-start-inputs", "cli"), ("first-start", "package")):
            with self.subTest(build=build):
                settings = job(release_jobs, release)
                targeted = job(target_jobs, build)
                for line in settings.splitlines():
                    if line.strip().startswith(("CARGO_BUILD_JOBS:", "timeout-minutes:")):
                        self.assertIn(line.strip(), targeted)
                self.assertLess(targeted.index("bash .github/scripts/ci_memory.sh"),
                                targeted.index("python3 .github/scripts/ci_memory_watch.py"))

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
        for target, variable in (("conformance", "ORCH_TEST_CONFORMANCE_NODE"), ("environment", "ORCH_TEST_CONNECTOR_BINARY")):
            with self.subTest(target=target), tempfile.TemporaryDirectory(dir=Path(__file__).parent) as directory:
                targets = Path(directory) / "targets"
                targets.write_text(f"test -p orch-node --test {target}\ntest -p orch-api-core --test forks\n", encoding="utf-8")
                environment = {variable: "/reported/static-fixture"}
                with patch.object(ci_targeted.FIXTURES, "prepare_fixtures", return_value=environment) as prepare, \
                        patch.object(ci_targeted, "run", return_value=True) as run:
                    self.assertEqual(ci_targeted.main(targets, "rust"), 0)
                prepare.assert_called_once_with([
                    ["cargo", "test", "-p", "orch-node", "--test", target],
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
                stdout, stderr = StringIO(), StringIO()
                with self.subTest(error=type(error).__name__), \
                        redirect_stdout(stdout), redirect_stderr(stderr), \
                        patch.object(ci_targeted.FIXTURES, "prepare_fixtures", side_effect=error), \
                        patch.object(ci_targeted, "run", return_value=True) as run:
                    self.assertEqual(ci_targeted.main(targets, "rust"), 1)
                    self.assertIn("FAILED: prepare native Docker fixtures", stdout.getvalue())
                    self.assertIn(f"native fixture preparation failed: {error}", stderr.getvalue())
                    self.assertTrue(any(call.args[1][:2] == ["cargo", "test"] for call in run.call_args_list))


if __name__ == "__main__":
    unittest.main()
