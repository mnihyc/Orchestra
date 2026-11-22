"""New public history is checked through merges and across the whole pushed range."""

from pathlib import Path
import subprocess
import tempfile
import unittest

from commit_messages import findings, first_violation, pushed_range


class PublicMessages(unittest.TestCase):
    def test_internal_words_are_refused_in_subjects_and_bodies(self):
        for word in ("lanes", "batch", "trials", "assembled", "integrator", "register",
                     "the owner", "C1112", "D-12345", "DOCS-UPDATE", "T6-RED-FILES"):
            with self.subTest(word=word):
                self.assertIsNotNone(first_violation(f"Document recovery.\n\nUsed in {word}."))
        self.assertIsNone(first_violation("Merge the file relay’s verification."))
        self.assertIsNone(first_violation("Describe JSON-RPC errors and AES-GCM keys in UTF-8."))
        self.assertIsNotNone(first_violation("Merge branch 'wt/implementation'"))

    def test_new_branches_start_at_the_public_base_and_other_updates_use_their_exact_base(self):
        event = {"repository": {"default_branch": "main"}, "before": "0" * 40, "created": True}
        self.assertEqual(pushed_range(event, "head"), ("refs/remotes/origin/main", "head"))
        self.assertEqual(pushed_range({**event, "before": "old"}, "head"), ("old", "head"))
        self.assertEqual(pushed_range({**event, "ref": "refs/heads/main"}, "head"), (None, "head"))
        pull = {"pull_request": {"base": {"sha": "base"}, "head": {"sha": "topic"}}}
        self.assertEqual(pushed_range(pull, "merge"), ("base", "topic"))

    def test_new_commits_on_either_side_of_a_merge_are_checked_once(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            def git(*args):
                return subprocess.check_output(["git", "-C", str(root), *args], text=True).strip()
            def commit(message):
                git("commit", "-q", "--allow-empty", "-m", message)
                return git("rev-parse", "HEAD")
            git("init", "-q", "-b", "main")
            git("config", "user.name", "Contributor")
            git("config", "user.email", "test@example.test")
            base = commit("Old assembly message.")
            git("switch", "-qc", "format")
            side = commit("Explain targets.\n\nPassed the sixth trial.")
            git("switch", "-q", "main")
            main = commit("Publish the next batch.")
            git("merge", "-q", "--no-ff", "format", "-m", "Merge the format documentation.")
            self.assertEqual({sha for sha, _ in findings(root, base, "HEAD")}, {main, side})
