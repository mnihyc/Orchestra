"""Exercise the updater's real release retention against isolated payload directories."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parents[3]


class ReleaseRetention(unittest.TestCase):
    def exercise(self, keep, failed=False):
        directory = tempfile.TemporaryDirectory(prefix="orchcustody-releases-")
        self.addCleanup(directory.cleanup)
        root = Path(directory.name)
        updater = root / "update.sh"
        # Load the actual functions without dispatching a deployment operation.
        source = (ROOT / "deploy/update.sh").read_text()
        updater.write_text(source.removesuffix('main "$@"\n'))
        releases = root / "runtime/server/releases"
        releases.mkdir(parents=True)
        for number in range(8):
            release = releases / f"release-{number}"
            release.mkdir()
            (release / ".complete").write_text("a" * 64 + "\n")
            (release / "program").write_text(f"program-{number}")
            os.utime(release, (number + 1, number + 1))
        # Protected links deliberately point to the oldest payloads.
        (releases.parent / "current").symlink_to("releases/release-0")
        (releases.parent / "previous").symlink_to("releases/release-1")
        (releases / "unrecognized").mkdir()
        (releases / ".staging-fixture").mkdir()
        (releases / ".staging-fixture/.complete").write_text("b" * 64 + "\n")
        outside = root / "other-installation"
        outside.mkdir()
        (outside / "important").write_text("keep")
        (releases / "external-link").symlink_to(outside)
        script = root / "run.sh"
        script.write_text(
            'set -euo pipefail\nsource "$1"\n'
            'compose() { printf "%s\\n" "$RETAINED"; }\n'
            'PAYLOADS=server\nprune_releases\n'
        )
        env = {**os.environ, "RETAINED": str(keep)}
        if failed:
            script.write_text(script.read_text().replace('compose() {', 'compose() { return 1;'))
        subprocess.run(["bash", str(script), str(updater)], env=env, check=True, capture_output=True, text=True)
        self.assertEqual((outside / "important").read_text(), "keep")
        self.assertTrue((releases / "external-link").is_symlink())
        self.assertTrue((releases / "unrecognized").is_dir())
        self.assertTrue((releases / ".staging-fixture/.complete").is_file())
        self.assertEqual((releases.parent / "current").readlink(), Path("releases/release-0"))
        self.assertEqual((releases.parent / "previous").readlink(), Path("releases/release-1"))
        return sorted(path.name for path in releases.glob("release-*"))

    def test_three_unused_releases_are_kept_in_addition_to_current_and_previous(self):
        self.assertEqual(self.exercise(3), ["release-0", "release-1", "release-5", "release-6", "release-7"])

    def test_zero_keeps_every_release(self):
        self.assertEqual(len(self.exercise(0)), 8)

    def test_the_setting_can_keep_more_than_the_shipped_count(self):
        self.assertEqual(len(self.exercise(20)), 8)

    def test_an_unreadable_policy_keeps_every_release(self):
        self.assertEqual(len(self.exercise(1, failed=True)), 8)

    def test_an_invalid_policy_keeps_every_release(self):
        self.assertEqual(len(self.exercise("not-a-number")), 8)


if __name__ == "__main__":
    unittest.main()
