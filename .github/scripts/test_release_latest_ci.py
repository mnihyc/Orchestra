"""Already-published reruns cannot edit Latest after a newer non-green CI."""
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from release_publish import main as publish

COMMIT = "a" * 40
TAG = "v1.2.3"
REPO = "example/orch"


class PublishedLatestCiTests(unittest.TestCase):
    def exercise_latest(self, status, conclusion, green=False):
        # TMPDIR is project-local for the isolated test invocation.
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            manifest = json.dumps(dict(tag=TAG, version="1.2.3", commit=COMMIT, images=[])).encode()
            (directory / "version.json").write_bytes(manifest)
            release = dict(tag_name=TAG, draft=False, prerelease=False,
                           assets=[dict(name="version.json", id=1,
                                        digest="sha256:" + hashlib.sha256(manifest).hexdigest())])
            current_latest = ["v1.2.2"]
            commands = []

            def execute(args, **kwargs):
                commands.append(list(args))
                if args[:2] == ["gh", "api"]:
                    if args[-1].endswith("/releases/assets/1"):
                        return subprocess.CompletedProcess(args, 0, manifest, b"")
                    if args[-1].endswith("/releases/tags/" + TAG):
                        return subprocess.CompletedProcess(args, 0, json.dumps(release), "")
                if args[:3] == ["gh", "release", "edit"]:
                    current_latest[0] = args[3]
                return subprocess.CompletedProcess(args, 0, "", "")

            def api(*args):
                if args[-1].endswith("/commits/" + TAG):
                    return json.dumps(dict(sha=COMMIT))
                if args[-1].endswith("/releases?per_page=100"):
                    return json.dumps([[release]])
                if args[-1].endswith("/releases/latest"):
                    return json.dumps(dict(tag_name=current_latest[0]))
                raise AssertionError(args)

            old = dict(id=12, head_sha=COMMIT, head_branch=TAG, event="push",
                       status="completed", conclusion="success", run_attempt=1)
            latest = dict(old, id=13, status=status, conclusion=conclusion)
            argv = ["release_publish.py", "--release", str(directory), "--repo", REPO,
                    "--images", str(directory)]
            refused = None
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"PUBLISH_IMAGES": "false"}), \
                    patch("release_publish.subprocess.run", side_effect=execute), \
                    patch("release_publish.run", side_effect=api), \
                    patch("release_ci.fetch", return_value=[old, latest]):
                try:
                    publish()
                except ValueError as error:
                    refused = str(error)
            mutations = [command for command in commands if command[:2] == ["gh", "release"]]
            if green:
                self.assertIsNone(refused)
                self.assertEqual(mutations, [["gh", "release", "edit", TAG, "--repo", REPO, "--latest"]])
                self.assertEqual(current_latest[0], TAG)
                self.assertEqual((directory / "version.json").read_bytes(), manifest)
                return
            self.assertEqual(mutations, [], "non-green exact-tag CI must prevent every Latest/publication edit")
            self.assertIsNotNone(refused, "the newer non-green CI must refuse this already-published fastpath")
            self.assertIn("nothing published", refused)
            self.assertEqual(current_latest[0], "v1.2.2")

    def test_published_release_cannot_edit_latest_after_newer_failed_exact_tag_ci(self):
        self.exercise_latest("completed", "failure")

    def test_published_release_cannot_edit_latest_while_newer_exact_tag_ci_is_in_progress(self):
        self.exercise_latest("in_progress", None)

    def test_published_release_with_current_green_ci_repairs_latest_without_replacing_assets(self):
        self.exercise_latest("completed", "success", green=True)


if __name__ == "__main__":
    unittest.main()
