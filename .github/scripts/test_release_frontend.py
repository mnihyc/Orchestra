"""A shared frontend must be the complete build of the exact release and commit."""

import json
from pathlib import Path
import tempfile
import unittest

from release_frontend import files_in, verify


class FrontendInputTests(unittest.TestCase):
    def input(self, root):
        dist = root / "dist"
        (dist / "assets").mkdir(parents=True)
        (dist / "index.html").write_text("<html>Orch</html>")
        (dist / "assets/app.js").write_text("application")
        listing = {"tag": "v1.2.3", "version": "1.2.3", "commit": "a" * 40, "files": files_in(dist)}
        (root / "frontend.json").write_text(json.dumps(listing))
        return dist

    def test_a_matching_frontend_is_accepted(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.input(root)
            verify(root, "1.2.3", "v1.2.3", "a" * 40)

    def test_mixed_releases_and_changed_missing_or_extra_files_are_refused(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            dist = self.input(root)
            with self.assertRaisesRegex(ValueError, "another release or commit"):
                verify(root, "1.2.3", "v1.2.3", "b" * 40)
            app = dist / "assets/app.js"
            for change in (lambda: app.write_text("changed"), lambda: app.rename(root / "saved-app.js"), lambda: (dist / "extra").write_text("extra")):
                app.write_text("application")
                change()
                with self.assertRaisesRegex(ValueError, "do not match"):
                    verify(root, "1.2.3", "v1.2.3", "a" * 40)

    def test_symbolic_links_are_refused(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            dist = self.input(root)
            (dist / "link").symlink_to("index.html")
            with self.assertRaisesRegex(ValueError, "symbolic link"):
                verify(root, "1.2.3", "v1.2.3", "a" * 40)

    def test_the_hidden_inventory_needed_for_notices_is_verified(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            dist = self.input(root)
            inventory = dist / ".vite/notice-packages.json"
            inventory.parent.mkdir()
            inventory.write_text('["node_modules/example"]')
            listing = json.loads((root / "frontend.json").read_text())
            listing["files"] = files_in(dist)
            (root / "frontend.json").write_text(json.dumps(listing))
            self.assertIn(".vite/notice-packages.json", listing["files"])
            verify(root, "1.2.3", "v1.2.3", "a" * 40)
            inventory.rename(root / "missing-inventory.json")
            with self.assertRaisesRegex(ValueError, "do not match"):
                verify(root, "1.2.3", "v1.2.3", "a" * 40)


if __name__ == "__main__":
    unittest.main()
