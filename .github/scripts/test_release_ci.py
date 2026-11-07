"""A release waits for the exact tagged CI and refuses failed or stale results."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

from release_ci import status, wait
from release_publish import check_published_manifest, main as publish, reconcile_latest

COMMIT = "a" * 40
TAG = "v1.2.3"


def run(**changes):
    return {"id": 12, "head_sha": COMMIT, "head_branch": TAG, "event": "push",
            "status": "completed", "conclusion": "success", "run_attempt": 1, **changes}


class CiWaitTests(unittest.TestCase):
    def test_a_tag_push_waits_through_missing_and_queued_ci_then_passes(self):
        with patch("release_ci.fetch", side_effect=[[], [run(status="queued", conclusion=None)], [run()]]), \
                patch("release_ci.WAIT_SECONDS", 10), patch("release_ci.POLL_SECONDS", 1):
            now = [0]
            def sleep(seconds):
                now[0] += seconds
            self.assertEqual(wait(TAG, COMMIT, "example/orch", clock=lambda: now[0], sleep=sleep)["id"], 12)
            self.assertEqual(now[0], 2)

    def test_timeout_refuses_to_publish_without_any_ci_run(self):
        with patch("release_ci.fetch", return_value=[]), patch("release_ci.WAIT_SECONDS", 2):
            now = [0]
            def sleep(seconds):
                now[0] += seconds
            with self.assertRaisesRegex(ValueError, "deadline; nothing published"):
                wait(TAG, COMMIT, "example/orch", clock=lambda: now[0], sleep=sleep)

    def test_failed_cancelled_and_retried_runs_do_not_inherit_an_old_success(self):
        for conclusion in ("failure", "cancelled", "timed_out"):
            with self.subTest(conclusion=conclusion), self.assertRaisesRegex(ValueError, conclusion):
                status(TAG, COMMIT, [run(), run(id=13, conclusion=conclusion)])
        self.assertIsNone(status(TAG, COMMIT, [run(), run(id=13, status="queued", conclusion=None)]))
        self.assertIsNone(status(TAG, COMMIT, [run(run_attempt=2, status="in_progress", conclusion=None)]))
        self.assertEqual(status(TAG, COMMIT, [run(run_attempt=2)])["run_attempt"], 2)

    def test_other_commits_tags_and_pull_requests_cannot_satisfy_the_gate(self):
        self.assertIsNone(status(TAG, COMMIT, [run(head_branch="main"), run(head_sha="b" * 40), run(event="pull_request")]))


class PublicationTests(unittest.TestCase):
    def test_latest_is_repaired_after_an_older_concurrent_publication(self):
        pages = [[{"tag_name": "v1.2.3", "draft": False}, {"tag_name": "v1.2.4", "draft": False}]]
        answers = [json.dumps(pages), json.dumps({"tag_name": "v1.2.3"}),
                   json.dumps(pages), json.dumps({"tag_name": "v1.2.4"})]
        with patch("release_publish.run", side_effect=answers), patch("release_publish.subprocess.run") as execute:
            reconcile_latest("example/orch")
            execute.assert_called_once_with(["gh", "release", "edit", "v1.2.4", "--repo", "example/orch", "--latest"], check=True)

    def test_a_rerun_accepts_only_verified_added_image_facts(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            listing = {"tag": TAG, "version": "1.2.3", "commit": COMMIT, "images": []}
            (directory / "version.json").write_text(json.dumps(listing))
            remote = json.dumps({**listing, "images": [{"name": "orch-env-base", "digest": "sha256:index"}]}).encode()
            asset = {"name": "version.json", "id": 1, "digest": "sha256:" + hashlib.sha256(remote).hexdigest()}
            with patch("release_publish.subprocess.run", return_value=subprocess.CompletedProcess([], 0, remote)):
                check_published_manifest({"assets": [asset]}, directory, "example/orch")
                with self.assertRaisesRegex(ValueError, "digest"):
                    check_published_manifest({"assets": [{**asset, "digest": "sha256:wrong"}]}, directory, "example/orch")
            wrong = json.dumps({**listing, "commit": "b" * 40}).encode()
            with patch("release_publish.subprocess.run", return_value=subprocess.CompletedProcess([], 0, wrong)):
                with self.assertRaisesRegex(ValueError, "differs"):
                    check_published_manifest({"assets": [{**asset, "digest": "sha256:" + hashlib.sha256(wrong).hexdigest()}]}, directory, "example/orch")

    def run_publish(self, failed_ci=False, tag=TAG, published_tag="v1.2.2"):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            (directory / "version.json").write_text(json.dumps({"tag": tag, "version": tag[1:], "commit": COMMIT}))
            commands = []
            def execute(args, **kwargs):
                commands.append(args)
                if args[:2] == ["gh", "api"]:
                    return subprocess.CompletedProcess(args, 1, "", "not found (HTTP 404)")
                return subprocess.CompletedProcess(args, 0, "", "")
            def api(*args):
                if "/commits/" in args[-1]:
                    return json.dumps({"sha": COMMIT})
                return json.dumps([[{"tag_name": published_tag, "draft": False, "prerelease": False}]] if published_tag else [[]])
            argv = ["release_publish.py", "--release", str(directory), "--repo", "example/orch", "--images", str(directory)]
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"PUBLISH_IMAGES": "false"}), \
                    patch("release_publish.subprocess.run", side_effect=execute), patch("release_publish.run", side_effect=api), \
                    patch("release_publish.reconcile_latest"), \
                    patch("release_publish.wait_for_ci", side_effect=ValueError("CI failed") if failed_ci else None) as ci:
                if failed_ci:
                    with self.assertRaisesRegex(ValueError, "CI failed"):
                        publish()
                else:
                    publish()
                ci.assert_called_once_with(tag, COMMIT, "example/orch", once=True)
            return commands

    def test_v012_publishes_automatically_after_its_own_green_ci_without_a_version_floor(self):
        commands = self.run_publish(tag="v0.1.2", published_tag=None)
        create = next(command for command in commands if command[:3] == ["gh", "release", "create"])
        self.assertEqual(create[3], "v0.1.2")
        self.assertIn("--verify-tag", create)
        self.assertIn("--latest=true", create)
        self.assertIn(".github/release-notes/v0.1.2.md", create)

    def test_green_ci_publishes_notes_and_verified_assets_as_latest(self):
        commands = self.run_publish()
        create = next(command for command in commands if command[:3] == ["gh", "release", "create"])
        self.assertIn("--latest=true", create)
        self.assertIn("--verify-tag", create)
        self.assertIn(".github/release-notes/v1.2.3.md", create)
        self.assertNotIn("--draft", create)

    def test_ci_failure_at_the_final_recheck_never_creates_a_release(self):
        commands = self.run_publish(failed_ci=True)
        self.assertFalse(any(command[:2] == ["gh", "release"] for command in commands))

    def test_a_rerun_completes_an_interrupted_draft_without_replacing_assets(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            manifest = json.dumps({"tag": TAG, "version": "1.2.3", "commit": COMMIT})
            (directory / "version.json").write_text(manifest)
            (directory / "payload").write_bytes(b"verified")
            draft = {"draft": True, "assets": [{"name": "version.json",
                     "digest": "sha256:" + hashlib.sha256(manifest.encode()).hexdigest()}]}
            commands = []
            def execute(args, **kwargs):
                commands.append(args)
                if args[:2] == ["gh", "api"]:
                    return subprocess.CompletedProcess(args, 0, json.dumps(draft), "")
                return subprocess.CompletedProcess(args, 0, "", "")
            def api(*args):
                return json.dumps({"sha": COMMIT}) if "/commits/" in args[-1] else json.dumps([[]])
            argv = ["release_publish.py", "--release", str(directory), "--repo", "example/orch", "--images", str(directory)]
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"PUBLISH_IMAGES": "false"}), \
                    patch("release_publish.subprocess.run", side_effect=execute), patch("release_publish.run", side_effect=api), \
                    patch("release_publish.wait_for_ci"), patch("release_publish.reconcile_latest"):
                publish()
            upload = next(command for command in commands if command[:3] == ["gh", "release", "upload"])
            self.assertIn(str(directory / "payload"), upload)
            self.assertNotIn("--clobber", upload)
            edit = next(command for command in commands if command[:3] == ["gh", "release", "edit"])
            self.assertIn("--draft=false", edit)

    def test_approved_images_update_only_the_published_manifest_after_ci(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            directory, images = root / "release", root / "images"
            directory.mkdir()
            manifest = json.dumps({"tag": TAG, "version": "1.2.3", "commit": COMMIT, "images": []})
            (directory / "version.json").write_text(manifest)
            (directory / "payload").write_bytes(b"verified")
            for arch in ("x86_64", "aarch64"):
                path = images / arch
                path.mkdir(parents=True)
                (path / "image.json").write_text(json.dumps({"arch": arch, "tag": TAG, "commit": COMMIT,
                    "image": {"reference": "example/" + arch, "digest": "sha256:" + arch}}))
            release = {"draft": False, "assets": [
                {"name": "version.json", "digest": "sha256:" + hashlib.sha256(manifest.encode()).hexdigest()},
                {"name": "payload", "digest": "sha256:" + hashlib.sha256(b"verified").hexdigest()}]}
            commands = []
            def execute(args, **kwargs):
                commands.append(args)
                if args[:2] == ["gh", "api"]:
                    return subprocess.CompletedProcess(args, 0, json.dumps(release), "")
                if args[:4] == ["docker", "buildx", "imagetools", "inspect"]:
                    return subprocess.CompletedProcess(args, 1, "", "")
                return subprocess.CompletedProcess(args, 0, "", "")
            def api(*args):
                if args[0] == "docker":
                    return json.dumps({"digest": "sha256:index"})
                return json.dumps({"sha": COMMIT}) if "/commits/" in args[-1] else json.dumps([[]])
            argv = ["release_publish.py", "--release", str(directory), "--repo", "example/orch", "--images", str(images)]
            env = {"PUBLISH_IMAGES": "true", "GITHUB_ACTOR": "actor", "GH_TOKEN": "test-token"}
            with patch.object(sys, "argv", argv), patch.dict(os.environ, env), \
                    patch("release_publish.subprocess.run", side_effect=execute), patch("release_publish.run", side_effect=api), \
                    patch("release_publish.reconcile_latest"), patch("release_publish.wait_for_ci") as ci:
                publish()
                self.assertEqual(ci.call_count, 2)
            uploads = [command for command in commands if command[:3] == ["gh", "release", "upload"]]
            self.assertEqual(len(uploads), 1)
            self.assertIn(str(directory / "version.json"), uploads[0])
            self.assertIn("--clobber", uploads[0])
            self.assertNotIn(str(directory / "payload"), uploads[0])
            self.assertEqual(json.loads((directory / "version.json").read_text())["images"][0]["digest"], "sha256:index")


if __name__ == "__main__":
    unittest.main()
