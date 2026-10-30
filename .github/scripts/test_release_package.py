"""Packaging installs the npm sources needed by the generated notices."""

from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import release_package


class PackageInputTests(unittest.TestCase):
    def test_notices_have_installed_sources_before_the_release_is_packaged_and_checked(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / "frontend").mkdir()
            (root / "frontend-input/dist").mkdir(parents=True)
            (root / "frontend-input/dist/index.html").write_text("frontend")
            (root / "inputs/native-linux").mkdir(parents=True)
            (root / "inputs/native-linux/version.json").write_text("{}")
            events = []
            def run(command, **_kwargs):
                if command[0] == "git":
                    return SimpleNamespace(stdout="a" * 40 + "\n")
                events.append(command)
                return SimpleNamespace(returncode=0)
            args = ["release_package.py", "--inputs", str(root / "inputs"), "--frontend", str(root / "frontend-input"),
                    "--out", str(root / "out")]
            with patch("sys.argv", args), patch.object(release_package.Path, "cwd", return_value=root), \
                 patch.dict(release_package.os.environ, {"ORCH_RELEASE_VERSION": "1.2.3", "ORCH_RELEASE_TAG": "v1.2.3"}), \
                 patch.object(release_package, "verify", side_effect=lambda *_: events.append("verified")), \
                 patch.object(release_package, "run_npm", side_effect=lambda _root, command: events.append(command)), \
                 patch.object(release_package.subprocess, "run", side_effect=run):
                release_package.main()
            self.assertEqual(events[0:2], ["verified", "npm ci"])
            self.assertIn("--reuse-frontend", events[2])
            self.assertIn("--require-all-connectors", events[2])
            self.assertIn("--check", events[3])


if __name__ == "__main__":
    unittest.main()
