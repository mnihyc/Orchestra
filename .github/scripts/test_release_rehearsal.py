"""A snapshot rehearsal never becomes tag, release or image publication."""

import unittest

from release_rehearsal import check_request
from release_warmup import check_request as check_warmup
from release_ci import status

HEAD = "a" * 40


class RehearsalTests(unittest.TestCase):
    def test_v012_rehearses_the_snapshot_without_a_tag_and_version_remains_configurable(self):
        for tag in ("v0.1.2", "v0.2.0"):
            with self.subTest(tag=tag):
                check_request(tag, "workflow_dispatch", "refs/heads/release-rehearsal-20261109", HEAD, HEAD, None)

    def test_a_future_version_can_rehearse_an_exact_untagged_snapshot(self):
        check_request("v1.2.3", "workflow_dispatch", "refs/heads/release-rehearsal", HEAD, HEAD, None)

    def test_automated_tag_different_sha_and_publication_requests_are_refused(self):
        valid = ("v1.2.3", "workflow_dispatch", "refs/heads/release-rehearsal", HEAD, HEAD, None)
        for index, value in [(0, "not-a-version"), (1, "push"), (2, "refs/tags/v1.2.3"), (3, "b" * 40), (5, "b" * 40)]:
            request = list(valid)
            request[index] = value
            with self.subTest(index=index), self.assertRaises(ValueError):
                check_request(*request)
        for flags in ({"warm": True}, {"images": True}):
            with self.subTest(flags=flags), self.assertRaises(ValueError):
                check_request(*valid, **flags)
        with self.assertRaises(ValueError):
            check_warmup("v1.2.3", "workflow_dispatch", "refs/heads/main", "main", "", False, rehearsal=True)

    def test_only_full_ci_for_this_snapshot_ref_and_sha_qualifies_a_rehearsal(self):
        green = {"id": 1, "head_sha": HEAD, "head_branch": "release-rehearsal", "event": "workflow_dispatch",
                 "status": "completed", "conclusion": "success"}
        self.assertEqual(status("release-rehearsal", HEAD, [green])["id"], 1)
        self.assertIsNone(status("release-rehearsal", HEAD, [{**green, "head_branch": "main"}]))
        self.assertIsNone(status("release-rehearsal", HEAD, [{**green, "head_sha": "b" * 40}]))
        # A branch's green rehearsal can never qualify the later release tag.
        self.assertIsNone(status("v1.2.3", HEAD, [green]))

    def test_a_branch_push_without_deployment_cannot_replace_the_manual_snapshot_ci(self):
        manual = {"id": 1, "head_sha": HEAD, "head_branch": "release-rehearsal", "event": "workflow_dispatch",
                  "status": "completed", "conclusion": "success"}
        self.assertEqual(status("release-rehearsal", HEAD, [manual], require_dispatch=True)["id"], 1)
        push = {**manual, "id": 2, "event": "push"}
        self.assertIsNone(status("release-rehearsal", HEAD, [manual, push], require_dispatch=True))
        with self.assertRaisesRegex(ValueError, "failure"):
            status("release-rehearsal", HEAD, [manual, {**push, "conclusion": "failure"}], require_dispatch=True)


if __name__ == "__main__":
    unittest.main()
