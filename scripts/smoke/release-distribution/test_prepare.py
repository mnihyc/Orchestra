"""A registry pull case never changes the release's payload bytes or the source listing."""

import json
from pathlib import Path
import tempfile
import unittest

from prepare import prepare


class DistributionFixtureTests(unittest.TestCase):
    def test_only_registry_and_relative_download_facts_change(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            source = root / "source"
            source.mkdir()
            payload = source / "orch-server-linux-x86_64.zip"
            payload.write_bytes(b"verified release bytes")
            listing = {"assets": [{"name": payload.name, "download_url": "https://release.test/file", "size": 22, "blake3": "a" * 64}],
                       "images": [{"name": "orch-env-base", "reference": "original", "digest": "sha256:" + "b" * 64}]}
            (source / "version.json").write_text(json.dumps(listing))
            result = prepare(source, root / "served", "127.0.0.1:41243/image:run", "sha256:" + "c" * 64)
            self.assertEqual((root / "served" / payload.name).read_bytes(), payload.read_bytes())
            self.assertEqual(json.loads((source / "version.json").read_text()), listing)
            self.assertEqual(result["assets"][0]["blake3"], "a" * 64)
            self.assertEqual(result["assets"][0]["download_url"], payload.name)
            self.assertEqual(result["images"][0]["reference"], "127.0.0.1:41243/image:run")

    def test_an_asset_path_cannot_read_outside_the_release(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            (root / "version.json").write_text(json.dumps({"assets": [{"name": "../other"}], "images": []}))
            with self.assertRaisesRegex(ValueError, "plain file"):
                prepare(root, root / "served", "registry/image:run", "sha256:" + "c" * 64)


if __name__ == "__main__":
    unittest.main()
