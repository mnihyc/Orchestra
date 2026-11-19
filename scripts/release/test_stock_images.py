"""An unpinned stock tag cannot pass the offline release gate."""

from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).with_name("stock-images.py")
TREE = SCRIPT.resolve().parents[2]


class ReleaseInputs(unittest.TestCase):
    def test_an_unpinned_tag_is_refused_without_querying_a_registry(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for relative in ("deploy/compose.yml", "deploy/postgres/Dockerfile", "images/orch-env-base/Dockerfile", ".github/workflows/ci.yml"):
                target = root / relative
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(TREE / relative, target)

            def run():
                return subprocess.run([sys.executable, "-I", "-B", str(SCRIPT), "--root", str(root), "--check"],
                                      capture_output=True, text=True, check=False)

            self.assertEqual(run().returncode, 0)
            database = root / "deploy/postgres/Dockerfile"
            pinned = database.read_text()
            database.write_text(re.sub(r"@sha256:[0-9a-f]{64}", "", pinned, count=1))
            self.assertEqual(run().returncode, 1, "the database base image is checked too")
            database.write_text(pinned)
            database.rename(database.with_suffix(".saved"))
            missing = run()
            self.assertEqual(missing.returncode, 1)
            self.assertIn("the database build recipe is missing", missing.stderr)
            database.with_suffix(".saved").rename(database)
            compose = root / "deploy/compose.yml"
            compose.write_text(re.sub(r"@sha256:[0-9a-f]{64}", "", compose.read_text(), count=1))
            refused = run()
            self.assertEqual(refused.returncode, 1)
            self.assertIn("needs a tag and a sha256 index digest", refused.stderr)


PIN = '@sha256:' + 'a' * 64


class EveryImageInput(unittest.TestCase):
    def run_check(self, root):
        return subprocess.run([sys.executable, '-I', '-B', str(SCRIPT), '--root', str(root), '--check'], capture_output=True, text=True, check=False)

    def put(self, root, name, text):
        path=root/name
        path.parent.mkdir(parents=True,exist_ok=True)
        path.write_text(text)
        return path

    def test_a_new_recipe_cannot_hide_outside_the_stock_directory(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            recipe=self.put(root,'tools/network/another.Dockerfile','FROM alpine:3.22\n')
            result=self.run_check(root)
            self.assertEqual(result.returncode,1)
            self.assertIn('tools/network/another.Dockerfile',result.stderr)
            recipe.write_text('FROM alpine:3.22'+PIN+'\n')
            self.assertEqual(self.run_check(root).returncode,0)

    def test_a_workflow_container_and_a_docker_action_both_need_pins(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            path=self.put(root,'.github/workflows/new.yaml','jobs:\n  build:\n    container: ubuntu:24.04\n    steps:\n      - uses: docker://busybox:1.37\n')
            result=self.run_check(root)
            self.assertEqual(result.returncode,1)
            self.assertIn('ubuntu:24.04',result.stderr)
            self.assertIn('busybox:1.37',result.stderr)
            path.write_text(path.read_text().replace('ubuntu:24.04','ubuntu:24.04'+PIN).replace('busybox:1.37','busybox:1.37'+PIN))
            self.assertEqual(self.run_check(root).returncode,0)

    def test_python_and_shell_selectors_are_distinct_from_mounts_ports_and_labels(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            python=self.put(root,'tools/start.py','import subprocess\nsubprocess.run(["docker","run","--rm","--label","role:worker","-p","127.0.0.1:41999:80","-v","source:/work","busybox:1.37","true"])\n')
            shell=self.put(root,'tools/start.sh','#!/bin/bash\nBASE_IMAGE="ubuntu:24.04"\ndocker run --rm "$BASE_IMAGE" true\n')
            result=self.run_check(root)
            self.assertEqual(result.returncode,1)
            self.assertIn('busybox:1.37',result.stderr)
            self.assertIn('ubuntu:24.04',result.stderr)
            self.assertNotIn('role:worker needs',result.stderr)
            self.assertNotIn('source:/work needs',result.stderr)
            python.write_text(python.read_text().replace('busybox:1.37','busybox:1.37'+PIN))
            shell.write_text(shell.read_text().replace('ubuntu:24.04','ubuntu:24.04'+PIN))
            self.assertEqual(self.run_check(root).returncode,0)

    def test_a_local_operator_helper_cannot_fall_back_to_a_registry(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            path=self.put(root,'hooks/helper.sh','#!/bin/bash\nHELPER_IMAGE="local-helper"\ndocker run --rm "$HELPER_IMAGE" true\n')
            self.assertEqual(self.run_check(root).returncode,1)
            path.write_text(path.read_text().replace('--rm','--rm --pull=never'))
            self.assertEqual(self.run_check(root).returncode,0)

    def test_an_image_recipe_does_not_exempt_other_uses_of_that_name(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            self.put(root, 'images/orch-env-base/Dockerfile', 'FROM alpine:3.22' + PIN + '\n')
            self.put(root, 'images/orch-env-dev/Dockerfile', 'ARG BASE_IMAGE=orch-env-base:dev\nFROM ${BASE_IMAGE}\n')
            script = self.put(root, 'tools/new.sh', '#!/bin/bash\ndocker run --rm orch-env-base:other true\n')
            result = self.run_check(root)
            self.assertEqual(result.returncode, 1)
            self.assertIn('orch-env-base:other needs', result.stderr)
            script.write_text('#!/bin/bash\ndocker run --rm --pull=never local-helper true\n')
            self.assertEqual(self.run_check(root).returncode, 0)

    def test_native_python_helper_calls_need_pins_and_local_only_flags_are_explicit(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            path=self.put(root,'tools/scenario.py','IMAGE="local-harness"\nself.container("database","postgres:17-alpine")\nself.run("docker","run","--rm",IMAGE,"true")\n')
            result=self.run_check(root)
            self.assertEqual(result.returncode,1)
            self.assertIn('postgres:17-alpine',result.stderr)
            self.assertIn('local-harness',result.stderr)
            path.write_text(path.read_text().replace('postgres:17-alpine','postgres:17-alpine'+PIN).replace('"--rm"','"--rm","--pull=never"'))
            self.assertEqual(self.run_check(root).returncode,0)

    def test_multistage_names_do_not_make_a_new_external_base_look_pinned(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            path=self.put(root,'nested/Dockerfile','ARG BASE_IMAGE=debian:bookworm-slim'+PIN+'\nFROM ${BASE_IMAGE} AS build\nFROM build AS final\n')
            self.assertEqual(self.run_check(root).returncode,0)
            path.write_text(path.read_text()+'FROM alpine:3.22 AS alpine\n')
            self.assertEqual(self.run_check(root).returncode,1)

    def test_a_recipe_built_after_another_starts_from_its_stage_and_nothing_else(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            self.put(root,'images/first/Dockerfile',f'FROM debian:bookworm-slim{PIN} AS first-base\n')
            second=self.put(root,'images/second/Dockerfile','FROM first-base\nRUN true\n')
            self.assertEqual(self.run_check(root).returncode,0)
            self.put(root,'tools/Dockerfile','FROM first-base\n')
            self.assertEqual(self.run_check(root).returncode,1,'only the image recipes share their stages')
            (root/'tools/Dockerfile').unlink()
            second.write_text('FROM other-base\n')
            self.assertEqual(self.run_check(root).returncode,1)

    def test_recorded_test_data_is_not_an_image_input_but_other_json_is(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            self.put(root,'crates/db/tests/fixtures/old-template.json','{"image": "orch-env-base:0.1.0"}\n')
            self.assertEqual(self.run_check(root).returncode,0)
            self.put(root,'crates/db/src/seed.json','{"image": "orch-env-base:0.1.0"}\n')
            self.assertEqual(self.run_check(root).returncode,1)

    def test_an_unknown_docker_option_cannot_hide_the_next_image(self):
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            self.put(root,'tools/new.sh','#!/bin/bash\ndocker run --unknown-new-flag alpine:3.22 true\n')
            result=self.run_check(root)
            self.assertEqual(result.returncode,1)
            self.assertIn('unsupported Docker option',result.stderr)


if __name__ == "__main__":
    unittest.main()
