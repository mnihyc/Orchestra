"""Release gates refuse misleading publication and preserve verified architecture inputs."""

import json
import hashlib
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch
import subprocess

from release_assemble import assemble, assemble_checked
from release_gate import compare, eligibility
from release_ci import status
from release_publish import check_release_assets, check_existing_release, is_latest, main as publish

HEAD = "a" * 40


class ReleaseGateTests(unittest.TestCase):
    def green(self, **changes):
        return {"id": 7, "head_sha": HEAD, "head_branch": "v1.2.3", "event": "push", "status": "completed", "conclusion": "success", **changes}

    def test_the_tag_must_name_this_commit_and_latest_follows_version_order(self):
        eligibility("v1.2.3", HEAD, HEAD)
        for tag, tagged in [("1.2.3", HEAD), ("v1.2.3", "b" * 40)]:
            with self.subTest(tag=tag, tagged=tagged), self.assertRaises(ValueError):
                eligibility(tag, HEAD, tagged)
        self.assertLess(compare("v1.2.3-rc.9", "v1.2.3-rc.10"), 0)
        self.assertGreater(compare("v1.10.0", "v1.9.9"), 0)
        self.assertTrue(is_latest("v1.2.3", [{"tag_name": "v1.2.2", "draft": False}]))
        self.assertFalse(is_latest("v1.2.3", [{"tag_name": "v1.3.0", "draft": False}]))
        self.assertFalse(is_latest("v1.2.3-rc.1", []))

    def test_an_older_green_run_cannot_hide_a_later_failed_or_unfinished_run(self):
        self.assertIsNone(status("v1.2.3", HEAD, [self.green(), self.green(id=8, status="in_progress", conclusion=None)]))
        for conclusion in ("failure", "cancelled"):
            with self.subTest(conclusion=conclusion), self.assertRaisesRegex(ValueError, conclusion):
                status("v1.2.3", HEAD, [self.green(), self.green(id=8, conclusion=conclusion)])
        self.assertIsNone(status("v1.2.3", HEAD, [self.green(head_sha="b" * 40)]))

    def test_ci_must_match_both_the_tag_and_its_commit(self):
        self.assertIsNone(status("v1.2.3", HEAD, []))
        self.assertIsNone(status("v1.2.3", HEAD, [self.green(head_branch="main")]))
        self.assertIsNone(status("v1.2.3", HEAD, [self.green(event="pull_request")]))
        self.assertEqual(status("v1.2.3", HEAD, [self.green()])["id"], 7)

    def test_completing_a_release_checks_its_tag_not_the_tag_creation_default(self):
        draft = {"draft": True, "target_commitish": "main"}
        check_existing_release(draft, HEAD, HEAD)
        with self.assertRaises(ValueError):
            check_existing_release(draft, "b" * 40, HEAD)

    def test_an_existing_published_release_is_refused_before_any_image_is_published(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            (directory / "version.json").write_text(json.dumps({"tag": "v1.2.3", "version": "1.2.3", "commit": HEAD}))
            def command(args, **_kwargs):
                if args[0] == "cargo":
                    return subprocess.CompletedProcess(args, 0)
                if args[0] == "gh":
                    return subprocess.CompletedProcess(args, 0, json.dumps({"draft": False, "assets": []}))
                self.fail(f"an image was touched before the release was checked: {args}")
            argv = ["release_publish.py", "--release", str(directory), "--repo", "example/orch", "--images", str(directory)]
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"PUBLISH_IMAGES": "true"}), \
                    patch("release_publish.subprocess.run", side_effect=command), patch("release_publish.run", return_value=json.dumps({"sha": HEAD})):
                with self.assertRaisesRegex(ValueError, "asset set"):
                    publish()

    def test_an_existing_draft_has_exactly_the_verified_assets(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            (directory / "version.json").write_text("{}")
            (directory / "payload").write_bytes(b"release bytes")
            digest = "sha256:" + hashlib.sha256(b"release bytes").hexdigest()
            manifest_digest = "sha256:" + hashlib.sha256(b"{}").hexdigest()
            draft = {"assets": [{"name": "version.json", "digest": manifest_digest}, {"name": "payload", "digest": digest}]}
            check_release_assets(draft, directory)
            with self.assertRaisesRegex(ValueError, "asset set"):
                check_release_assets({"assets": draft["assets"] + [{"name": "unreviewed"}]}, directory)
            with self.assertRaisesRegex(ValueError, "asset set"):
                check_release_assets({"assets": draft["assets"][:-1]}, directory)
            self.assertEqual(check_release_assets({"assets": draft["assets"][:-1]}, directory, partial=True), ["payload"])
            (directory / "payload").write_bytes(b"different bytes")
            with self.assertRaisesRegex(ValueError, "release asset differs"):
                check_release_assets(draft, directory)

    def test_an_unavailable_release_api_does_not_start_image_publication(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            (directory / "version.json").write_text(json.dumps({"tag": "v1.2.3", "version": "1.2.3", "commit": HEAD}))
            def command(args, **_kwargs):
                if args[0] == "cargo":
                    return subprocess.CompletedProcess(args, 0)
                if args[0] == "gh":
                    return subprocess.CompletedProcess(args, 1, "", "service unavailable")
                self.fail(f"an image was touched while release status was unknown: {args}")
            argv = ["release_publish.py", "--release", str(directory), "--repo", "example/orch", "--images", str(directory)]
            with patch.object(sys, "argv", argv), patch.dict(os.environ, {"PUBLISH_IMAGES": "true"}), \
                    patch("release_publish.subprocess.run", side_effect=command):
                with self.assertRaisesRegex(ValueError, "could not be checked"):
                    publish()


class ArchitectureAssemblyTests(unittest.TestCase):
    def inputs(self, directory):
        root = directory / "inputs"
        listing = {"project": "orch", "schema_version": 1, "tag": "v1.2.3", "version": "1.2.3", "commit": HEAD,
                   "link_protocol": {"current": 1, "minimum": 1}, "connector_minimum": "0.1.0", "harness_builds": [], "images": []}
        for arch in ("x86_64", "aarch64"):
            part = root / arch
            part.mkdir(parents=True)
            server = f"orch-server-linux-{arch}.zip"
            (part / server).write_bytes(arch.encode())
            (part / "orch-frontend-dist.tar.gz").write_bytes(b"same browser bytes")
            (part / "version.json").write_text(json.dumps({**listing, "assets": [{"name": server}, {"name": "orch-frontend-dist.tar.gz"}]}))
        return root

    def test_both_server_architectures_and_the_identical_browser_are_preserved(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            root, out = self.inputs(directory), directory / "release"
            result = assemble(root, out)
            self.assertEqual(result["commit"], HEAD)
            self.assertEqual((out / "orch-server-linux-x86_64.zip").read_bytes(), b"x86_64")
            self.assertEqual((out / "orch-server-linux-aarch64.zip").read_bytes(), b"aarch64")
            self.assertEqual((out / "orch-frontend-dist.tar.gz").read_bytes(), b"same browser bytes")

    def test_a_mixed_commit_or_differing_shared_asset_is_refused(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            root = self.inputs(directory)
            path = root / "aarch64/version.json"
            listing = json.loads(path.read_text())
            path.write_text(json.dumps({**listing, "commit": "b" * 40}))
            with self.assertRaisesRegex(ValueError, "commit"):
                assemble(root, directory / "mixed")
            path.write_text(json.dumps(listing))
            (root / "aarch64/orch-frontend-dist.tar.gz").write_bytes(b"another build")
            with self.assertRaisesRegex(ValueError, "shared asset"):
                assemble(root, directory / "different")

    def test_two_packages_for_the_same_processor_do_not_make_a_complete_release(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            root = self.inputs(directory)
            path = root / "aarch64/version.json"
            listing = json.loads(path.read_text())
            listing["assets"][0]["name"] = "orch-server-linux-x86_64.zip"
            path.write_text(json.dumps(listing))
            with self.assertRaisesRegex(ValueError, "different Linux server architecture"):
                assemble(root, directory / "duplicate")

    def test_a_damaged_download_is_refused_before_a_new_manifest_can_bless_it(self):
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            root = self.inputs(directory)
            out = directory / "release"
            with patch("release_assemble.subprocess.run", side_effect=subprocess.CalledProcessError(1, "check")):
                with self.assertRaises(subprocess.CalledProcessError):
                    assemble_checked(root, out, "https://release.example/v1.2.3")
            self.assertFalse(out.exists())


if __name__ == "__main__":
    unittest.main()
