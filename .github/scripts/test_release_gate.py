"""Release gates refuse misleading publication and preserve verified architecture inputs."""

import json
from pathlib import Path
import tempfile
import unittest

from release_assemble import assemble
from release_gate import compare, eligibility, waivers

HEAD = "a" * 40


class ReleaseGateTests(unittest.TestCase):
    def green(self, **changes):
        return {"id": 7, "head_sha": HEAD, "event": "push", "status": "completed", "conclusion": "success", **changes}

    def test_the_tag_must_name_this_commit_and_advance_published_versions(self):
        self.assertEqual(eligibility("v1.2.3", HEAD, HEAD, [{"tag_name": "v1.2.2", "draft": False}], [self.green()]), 7)
        for tag, tagged, old in [("1.2.3", HEAD, "v1.2.2"), ("v1.2.3", "b" * 40, "v1.2.2"),
                                 ("v1.2.3", HEAD, "v1.2.3"), ("v1.2.3-rc.1", HEAD, "v1.2.3")]:
            with self.subTest(tag=tag, tagged=tagged, old=old), self.assertRaises(ValueError):
                eligibility(tag, HEAD, tagged, [{"tag_name": old, "draft": False}], [self.green()])
        self.assertLess(compare("v1.2.3-rc.9", "v1.2.3-rc.10"), 0)
        self.assertGreater(compare("v1.10.0", "v1.9.9"), 0)

    def test_an_older_green_run_cannot_hide_a_later_failed_or_unfinished_run(self):
        for later in [self.green(id=8, status="in_progress", conclusion=None), self.green(id=8, conclusion="failure")]:
            with self.subTest(later=later), self.assertRaisesRegex(ValueError, "most recent CI"):
                eligibility("v1.2.3", HEAD, HEAD, [], [self.green(), later])
        with self.assertRaisesRegex(ValueError, "no CI run"):
            eligibility("v1.2.3", HEAD, HEAD, [], [self.green(head_sha="b" * 40)])

    def test_only_a_listed_tag_goes_without_green_ci_and_it_keeps_every_other_check(self):
        listed = waivers("# tag and reason\nv1.2.3 built beside its first CI run\n\n")
        self.assertEqual(listed, {"v1.2.3": "built beside its first CI run"})
        reason = listed.get("v1.2.3")
        self.assertIsNone(eligibility("v1.2.3", HEAD, HEAD, [], [], reason))
        self.assertIsNone(eligibility("v1.2.3", HEAD, HEAD, [], [self.green(conclusion="failure")], reason))
        self.assertEqual(eligibility("v1.2.3", HEAD, HEAD, [], [self.green()], reason), 7)
        with self.assertRaisesRegex(ValueError, "no CI run"):
            eligibility("v1.2.4", HEAD, HEAD, [], [], listed.get("v1.2.4"))
        with self.assertRaisesRegex(ValueError, "HEAD"):
            eligibility("v1.2.3", HEAD, "b" * 40, [], [], reason)
        with self.assertRaisesRegex(ValueError, "advance"):
            eligibility("v1.2.3", HEAD, HEAD, [{"tag_name": "v1.2.3", "draft": False}], [], reason)
        with self.assertRaisesRegex(ValueError, "no reason"):
            waivers("v1.2.3\n")


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


if __name__ == "__main__":
    unittest.main()
