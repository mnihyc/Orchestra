"""The updater compares registry and local image identities without misleading warnings."""

import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[3]
UPDATER = ROOT / "deploy/update.sh"


def images(reference, digest, present, digest_image=""):
    # Load the real updater functions without running its main command or changing a deployment.
    source = UPDATER.read_text()
    assert source.endswith('\nmain "$@"\n')
    source = source.removesuffix('\nmain "$@"\n')
    fixture = r'''
mget() {
  case "$1" in
    images.0.reference) printf '%s\n' "$TEST_REFERENCE" ;;
    images.0.digest) printf '%s\n' "$TEST_DIGEST" ;;
    *) return 1 ;;
  esac
}
docker() {
  [ "$1" = image ] && [ "$2" = inspect ] || return 1
  if [ "$3" = --format ]; then
    if [ "$5" = "$TEST_REFERENCE" ]; then printf '%s\n' "$TEST_PRESENT"
    elif [ "$5" = "$TEST_REFERENCE@$TEST_DIGEST" ] && [ -n "$TEST_DIGEST_IMAGE" ]; then printf '%s\n' "$TEST_DIGEST_IMAGE"
    else return 1; fi
  else
    [ "$3" = "$TEST_REFERENCE" ]
  fi
}
ensure_images
'''
    env = dict(os.environ, TEST_REFERENCE=reference, TEST_DIGEST=digest, TEST_PRESENT=present,
               TEST_DIGEST_IMAGE=digest_image, ORCH_OFFLINE_IMAGE_MAP="")
    with tempfile.TemporaryDirectory() as directory:
        script = Path(directory) / "updater.sh"
        script.write_text(source + "\n" + fixture)
        return subprocess.run(["bash", str(script)], cwd=ROOT, env=env,
                              capture_output=True, text=True, check=True)


class UpdaterImages(unittest.TestCase):
    def test_a_digest_qualified_registry_image_does_not_warn_about_its_different_config_id(self):
        digest = "sha256:" + "a" * 64
        result = images(f"example.test/base:1@{digest}", digest, "sha256:" + "b" * 64)
        self.assertEqual(result.stderr, "")

    def test_a_registry_index_is_matched_by_the_image_it_resolves_to(self):
        digest = "sha256:" + "a" * 64
        image = "sha256:" + "b" * 64
        self.assertEqual(images("example.test/base:1", digest, image, image).stderr, "")
        self.assertIn("and the release names", images("example.test/base:1", digest, image, "sha256:" + "c" * 64).stderr)

    def test_a_different_pinned_digest_is_still_reported(self):
        actual = "sha256:" + "a" * 64
        wanted = "sha256:" + "b" * 64
        self.assertIn("and the release names", images(f"example.test/base:1@{actual}", wanted, "sha256:" + "c" * 64).stderr)


if __name__ == "__main__":
    unittest.main()
