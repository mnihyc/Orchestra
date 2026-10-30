"""Runner admission checks: a successful test process is not sufficient native evidence."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


RUNNERS = Path(__file__).resolve().parents[1]


class RunnerEvidenceTests(unittest.TestCase):
    def run_runner(self, runner, output, *, image=0, cargo=0):
        with tempfile.TemporaryDirectory() as temporary:
            directory = Path(temporary)
            binary = directory / "bin"
            binary.mkdir()
            docker = binary / "docker"
            docker.write_text('#!/bin/sh\nexit "$CANARY_TEST_IMAGE_STATUS"\n')
            docker.chmod(0o755)
            command = binary / "cargo"
            command.write_text(
                '#!/bin/sh\nprintf "%s\\n" "$CANARY_TEST_OUTPUT"\n'
                'exit "$CANARY_TEST_CARGO_STATUS"\n'
            )
            command.chmod(0o755)
            evidence = directory / "evidence"
            environment = {
                **os.environ,
                "PATH": f"{binary}{os.pathsep}{os.environ['PATH']}",
                "CANARY_TEST_IMAGE_STATUS": str(image),
                "CANARY_TEST_CARGO_STATUS": str(cargo),
                "CANARY_TEST_OUTPUT": output,
            }
            arguments = [
                "bash",
                str(RUNNERS / runner),
                "--image",
                "synthetic-harness",
                "--record",
                str(evidence),
            ]
            if runner == "failures":
                arguments.extend(["--adapter", "codex"])
            result = subprocess.run(
                arguments, env=environment, capture_output=True, text=True, timeout=10,
                check=False,
            )
            kept = {
                file.name: file.read_text()
                for file in evidence.glob("*.log")
            }
            return result.returncode, result.stdout + result.stderr, kept

    def test_missing_image_is_incomplete_for_both_runners(self):
        for runner in ["conformance", "failures"]:
            with self.subTest(runner=runner):
                status, _, _ = self.run_runner(runner, "", image=1)
                self.assertEqual(status, 2)

    def test_conformance_requires_each_adapters_native_startup(self):
        for output in [
            "",
            "CONFORMANCE passed codex startup 1.0.0",
            "CONFORMANCE passed another-adapter startup 1.0.0",
        ]:
            with self.subTest(output=output):
                status, _, _ = self.run_runner("conformance", output)
                self.assertEqual(status, 1)

    def test_conformance_distinguishes_complete_incomplete_and_failed(self):
        startup = (
            "CONFORMANCE passed codex startup 1.0.0\n"
            "CONFORMANCE passed claude-code startup 1.0.0"
        )
        for extra, cargo, expected in [
            ("", 0, 0),
            ("\nCONFORMANCE fixture_only codex question 1.0.0", 0, 2),
            ("\nCONFORMANCE failed codex question 1.0.0", 0, 1),
            ("\nCONFORMANCE informational codex dynamic_tools 1.0.0", 0, 0),
            ("\nCONFORMANCE fixture_only codex question 1.0.0", 101, 1),
        ]:
            with self.subTest(extra=extra, cargo=cargo):
                status, _, kept = self.run_runner("conformance", startup + extra, cargo=cargo)
                self.assertEqual(status, expected)
                self.assertEqual(kept["codex.log"].strip(), startup + extra)
                self.assertIn("claude-code.log", kept)

    def test_failure_table_requires_native_rows_and_retains_the_process_failure(self):
        for output, cargo, expected in [
            ("SKIPPED native rig unavailable", 0, 2),
            ("same codex refusal (1.0.0)\nSKIPPED another case", 0, 2),
            ("", 0, 1),
            ("WORDS codex refusal (1.0.0)", 0, 1),
            ("same claude-code refusal (1.0.0)", 0, 1),
            ("same codex refusal (1.0.0)", 0, 0),
            ("CHANGED codex refusal (1.0.0)", 0, 1),
            ("same codex refusal (1.0.0)", 101, 1),
            ("SKIPPED native rig unavailable", 101, 1),
        ]:
            with self.subTest(output=output, cargo=cargo):
                status, _, kept = self.run_runner("failures", output, cargo=cargo)
                self.assertEqual(status, expected)
                self.assertEqual(kept["codex.log"].strip(), output)


if __name__ == "__main__":
    unittest.main()
