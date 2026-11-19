"""Repackaging accepts completed release runs and refuses other sources."""

import unittest

from release_source import check_source, has_frontend


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


if __name__ == "__main__":
    unittest.main()
