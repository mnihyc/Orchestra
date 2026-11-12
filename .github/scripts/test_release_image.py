"""A long native image build cannot publish after its exact-tag CI or tag changes."""

import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

import release_image
import release_publish

COMMIT = "a" * 40
TAG = "v1.2.3"
REPO = "example/orch"


class NativeImagePublicationTests(unittest.TestCase):
    def exercise(self, *, green=True, moved=False):
        events = []

        def execute(args, **kwargs):
            events.append(list(args))
            if args[:4] == ["docker", "buildx", "imagetools", "inspect"]:
                if "--format" not in args:
                    return subprocess.CompletedProcess(args, 1, "", "not found")
                return subprocess.CompletedProcess(args, 0, json.dumps({"digest": "sha256:" + "c" * 64}), "")
            return subprocess.CompletedProcess(args, 0, "", "")

        def command(*args):
            events.append(list(args))
            if args == ("git", "rev-parse", "HEAD"):
                return COMMIT
            self.assertEqual(args, ("gh", "api", f"repos/{REPO}/commits/{TAG}"))
            return json.dumps({"sha": "b" * 40 if moved else COMMIT})

        def ci(tag, commit, repo, *, once):
            events.append(["ci", tag, commit, repo, once])
            if not green:
                raise ValueError("exact-tag CI is no longer green")

        env = dict(ORCH_RELEASE_VERSION="1.2.3", ORCH_RELEASE_TAG=TAG, GITHUB_REPOSITORY=REPO,
                   GITHUB_ACTOR="release-test", GH_TOKEN="fixture-only", REHEARSAL="false", WARM_CACHES="false")
        with tempfile.TemporaryDirectory() as name:
            argv = ["release_image.py", "--arch", "x86_64", "--out", name]
            with patch("sys.argv", argv), patch.dict(os.environ, env), \
                    patch.object(release_image.platform, "machine", return_value="x86_64"), \
                    patch.object(release_image.subprocess, "run", side_effect=execute), \
                    patch.object(release_image, "command", side_effect=command), \
                    patch.object(release_image, "wait_for_ci", side_effect=ci):
                if green and not moved:
                    release_image.main()
                    self.assertEqual(json.loads((Path(name) / "image.json").read_text())["commit"], COMMIT)
                else:
                    with self.assertRaisesRegex(ValueError, "no longer"):
                        release_image.main()
                    self.assertFalse((Path(name) / "image.json").exists())
        return events

    def test_a_newer_non_green_ci_prevents_login_and_the_first_architecture_push(self):
        events = self.exercise(green=False)
        self.assertTrue(any(event[:2] == ["cargo", "xtask"] for event in events))
        self.assertFalse(any(event[:2] in (["docker", "login"], ["docker", "push"]) for event in events))

    def test_a_tag_moved_during_the_native_build_prevents_the_first_push(self):
        events = self.exercise(moved=True)
        self.assertTrue(any(event[0] == "ci" for event in events))
        self.assertFalse(any(event[:2] in (["docker", "login"], ["docker", "push"]) for event in events))

    def test_approved_green_image_push_follows_build_ci_and_tag_rechecks(self):
        events = self.exercise()
        build = next(i for i, event in enumerate(events) if event[:2] == ["cargo", "xtask"])
        ci = next(i for i, event in enumerate(events) if event[0] == "ci")
        tag = next(i for i, event in enumerate(events) if event[:2] == ["gh", "api"])
        push = next(i for i, event in enumerate(events) if event[:2] == ["docker", "push"])
        self.assertLess(build, ci)
        self.assertLess(ci, tag)
        self.assertLess(tag, push)
        self.assertEqual(events[ci], ["ci", TAG, COMMIT, REPO, True])

    def test_rehearsal_and_cache_modes_refuse_both_publishers_before_any_command(self):
        for mode in ("REHEARSAL", "WARM_CACHES"):
            for publisher in (release_image.main, release_publish.main):
                with self.subTest(mode=mode, publisher=publisher.__module__), \
                        patch.dict(os.environ, {"REHEARSAL": "false", "WARM_CACHES": "false", mode: "true"}), \
                        patch("subprocess.run") as run:
                    with self.assertRaisesRegex(ValueError, "cannot publish"):
                        publisher()
                    run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
