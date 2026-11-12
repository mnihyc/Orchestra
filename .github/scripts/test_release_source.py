"""Repackaging accepts completed release runs and refuses other sources."""

import unittest
import json
from pathlib import Path
from unittest.mock import patch

from release_source import check_source, has_frontend, observation_artifacts, require_observations


class SourceRunTests(unittest.TestCase):
    def test_a_failed_but_completed_release_run_can_supply_artifacts(self):
        run = {"id": 123, "path": ".github/workflows/release.yml@refs/heads/main", "event": "workflow_dispatch",
               "status": "completed", "conclusion": "failure"}
        check_source("123", run)

    def test_invalid_unfinished_or_unrelated_runs_are_refused(self):
        run = {"id": 123, "path": ".github/workflows/release.yml@refs/tags/v1.2.3", "event": "push", "status": "completed"}
        for run_id, change in [("0", {}), ("123; echo bad", {}), ("123", {"id": 124}),
                               ("123", {"path": ".github/workflows/ci.yml@main"}),
                               ("123", {"status": "in_progress"}), ("123", {"event": "pull_request"})]:
            with self.subTest(run_id=run_id, change=change), self.assertRaises(ValueError):
                check_source(run_id, {**run, **change})

    def test_an_old_or_expired_frontend_artifact_is_built_again(self):
        self.assertFalse(has_frontend([{"artifacts": [{"name": "native-a", "expired": False}]}]))
        self.assertFalse(has_frontend([{"artifacts": [{"name": "frontend-dist", "expired": True}]}]))
        self.assertTrue(has_frontend([{"artifacts": []}, {"artifacts": [{"name": "frontend-dist", "expired": False}]}]))

    def test_repackaging_cannot_reuse_another_candidates_or_missing_platform_evidence(self):
        run = {"id": 123, "path": ".github/workflows/release.yml", "event": "workflow_dispatch",
               "status": "completed", "head_sha": "a" * 40}
        check_source("123", run, "a" * 40)
        with self.assertRaisesRegex(ValueError, "another candidate"):
            check_source("123", run, "b" * 40)
        targets = ["aarch64-apple-darwin", "aarch64-pc-windows-msvc"]
        complete = [{"artifacts": [{"name": "inv15-" + target, "expired": False} for target in targets]}]
        observation_artifacts(complete, targets)
        for pages in [[{"artifacts": []}], [{"artifacts": [{"name": "inv15-" + target, "expired": True} for target in targets]}]]:
            with self.assertRaisesRegex(ValueError, "platform evidence"):
                observation_artifacts(pages, targets)

    def test_a_named_artifact_with_a_failed_observation_cannot_be_reused(self):
        target = "aarch64-pc-windows-msvc"
        pages = [{"artifacts": [{"name": "inv15-" + target, "expired": False}]}]
        def download(command, **_kwargs):
            directory = Path(command[command.index("--dir") + 1])
            directory.mkdir(parents=True)
            (directory / f"{target}.json").write_text(json.dumps({"format": 1, "target": target,
                "commit": "a" * 40, "status": "failed", "reason": "the revoked pair was accepted"}))
        with patch("release_source.catalogue", return_value=[(target, "windows", "aarch64", "candidate.exe")]), \
             patch("release_source.subprocess.run", side_effect=download), \
             self.assertRaisesRegex(ValueError, "revoked pair was accepted"):
            require_observations("123", "example/project", "a" * 40, pages)


if __name__ == "__main__":
    unittest.main()
