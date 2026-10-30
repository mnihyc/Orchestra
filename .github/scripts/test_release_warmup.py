"""Cache warm-up accepts only a manual default-branch run without publication."""

import unittest

from release_warmup import check_request


class WarmupTests(unittest.TestCase):
    def test_a_future_tag_can_warm_the_default_branch_without_being_created(self):
        check_request("v1.2.3", "workflow_dispatch", "refs/heads/main", "main", "", False)

    def test_automated_other_branch_repackage_and_publish_requests_are_refused(self):
        valid = ("v1.2.3", "workflow_dispatch", "refs/heads/main", "main", "", False)
        for position, value in [(0, "not-a-tag"), (1, "push"), (2, "refs/heads/other"), (3, ""), (4, "123"), (5, True)]:
            request = list(valid)
            request[position] = value
            with self.subTest(position=position), self.assertRaises(ValueError):
                check_request(*request)


if __name__ == "__main__":
    unittest.main()
