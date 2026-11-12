"""Pure cleanup controls; no file deletion, Docker or released process is executed."""
import importlib.util
import json
import re
from pathlib import Path
import shlex
import subprocess
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location('smoke_cleanup', Path(__file__).parents[1] / 'cleanup.py')
cleanup = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(cleanup)


class LiteralRemoval(unittest.TestCase):
    def test_only_literal_absolute_path_is_passed_to_rm(self):
        path = Path("/owned/project/.tmp/run ' $(printf should-not-run) `false`")
        lines = cleanup.removal_script(path).splitlines()
        self.assertEqual(lines[:2], ['#!/usr/bin/env bash', 'set -euo pipefail'])
        self.assertEqual(shlex.split(lines[2]), ['rm', '-r', '--', str(path)])
        # Syntax only: bash -n executes none of the generated commands.
        subprocess.run(['bash', '-n'], input=cleanup.removal_script(path), text=True, check=True)

    def test_relative_removal_is_refused(self):
        with self.assertRaises(ValueError):
            cleanup.removal_script(Path('relative/run'))

    def test_existing_run_is_never_removed_or_reused(self):
        with patch.object(cleanup, 'checked_path', return_value=Path('/owned/run')), \
                patch.object(Path, 'exists', return_value=True), \
                patch.object(Path, 'mkdir') as mkdir, patch.object(subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'already has files'):
                cleanup.prepare(Path('/owned'), Path('/owned/base'), 'run', 'linkload')
            mkdir.assert_not_called()
            run.assert_not_called()

    def test_evidence_symlink_is_rejected_before_any_creation_or_read(self):
        for link in ('/owned/project/base/evidence', '/owned/project/base/evidence/run'):
            with patch.object(Path, 'resolve', lambda self, **_: self), \
                    patch.object(subprocess, 'check_output', return_value='/owned/project/.git\n'), \
                    patch.object(Path, 'is_symlink', lambda self: str(self) == link), \
                    patch.object(Path, 'mkdir') as mkdir, patch.object(Path, 'read_text') as read:
                for action in (cleanup.prepare, cleanup.execute):
                    with self.assertRaisesRegex(ValueError, 'evidence path'):
                        action(Path('/owned/project'), Path('/owned/project/base'), 'run', 'linkload')
                mkdir.assert_not_called()
                read.assert_not_called()


class TeardownFailures(unittest.TestCase):
    def test_shared_compose_helpers_cannot_follow_an_inherited_unrelated_project(self):
        source = (Path(__file__).parents[1] / 'lib.sh').read_text()
        compose = re.search(r'^compose\(\).*$', source, re.M)[0]
        update = re.search(r'^update_sh\(\).*$', source, re.M)[0].replace('./update.sh', 'updater_stub')
        script = '''
STACK=.; PROJECT=owned; SMOKE_COMPOSE_FILES=./owned-qa.yml; EDGE=http://owned
export COMPOSE_PROJECT_NAME=unrelated COMPOSE_FILE=unrelated.yml
docker() { printf 'docker:%s:%s\n' "$COMPOSE_PROJECT_NAME" "$COMPOSE_FILE"; }
updater_stub() { printf 'updater:%s:%s\n' "$COMPOSE_PROJECT_NAME" "$COMPOSE_FILE"; }
''' + compose + '\n' + update + '\ncompose ps\nupdate_sh first-start\nprintf "caller:%s:%s\\n" "$COMPOSE_PROJECT_NAME" "$COMPOSE_FILE"\n'
        result = subprocess.run(['bash', '-c', script], capture_output=True, text=True, check=True)
        self.assertEqual(result.stdout.splitlines(), ['docker:owned:./owned-qa.yml',
            'updater:owned:./compose.yml', 'caller:unrelated:unrelated.yml'])

    def finish(self, compose_status=0, docker_status=0, remaining='', failed=0, keep=0, image_status=0):
        source = (Path(__file__).parents[1] / 'lib.sh').read_text()
        function = 'finish() {' + source.split('finish() {', 1)[1].split('# --- Through the web edge', 1)[0]
        stubs = '''
FAILED=PREVIOUS_FAILED; ORCH_SMOKE_KEEP=KEPT_MODE; CHECKED=0; STACK=/owned/run/deploy; PROJECT=owned; SMOKE_REPO=/owned/repo
SMOKE_BASE=/owned/base; SMOKE_RUN=run; SMOKE_SCENARIO=linkload
stop_site() { :; }; say() { :; }; fail() { FAILED=$((FAILED+1)); }
compose() { return COMPOSE_STATUS; }
docker() { printf '%s' REMAINING; return DOCKER_STATUS; }
python3() { printf 'FILE_DELETE\n'; }
smoke_cleanup_owned_image() { return IMAGE_STATUS; }
'''.replace('COMPOSE_STATUS', str(compose_status)).replace('DOCKER_STATUS', str(docker_status)) \
            .replace('REMAINING', shlex.quote(remaining)).replace('PREVIOUS_FAILED', str(failed)).replace('KEPT_MODE', str(keep)) \
            .replace('IMAGE_STATUS', str(image_status))
        # Every resource/file command is a shell test double. No Docker or rm runs.
        return subprocess.run(['bash', '-c', stubs + function + '\nfinish; status=$?; printf "CLEANUP=%s\\n" "$SMOKE_CLEANUP_COMPLETE"; exit "$status"\n'], capture_output=True, text=True)

    def test_compose_failure_retains_files(self):
        result = self.finish(compose_status=41)
        self.assertNotEqual(result.returncode, 0)
        self.assertNotIn('FILE_DELETE', result.stdout)

    def test_inventory_failure_or_remaining_resource_retains_files(self):
        for result in (self.finish(docker_status=42), self.finish(remaining='owned-container')):
            self.assertNotEqual(result.returncode, 0)
            self.assertNotIn('FILE_DELETE', result.stdout)

    def test_only_confirmed_absence_reaches_file_cleanup(self):
        result = self.finish()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout.count('FILE_DELETE'), 1)

    def test_owned_image_cleanup_failure_retains_fixture_files(self):
        result = self.finish(image_status=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn('CLEANUP=0', result.stdout)
        self.assertNotIn('FILE_DELETE', result.stdout)

    def test_failed_checks_still_clean_while_keep_preserves_shared_semantics(self):
        failed = self.finish(failed=1)
        self.assertNotEqual(failed.returncode, 0)
        self.assertIn('CLEANUP=1', failed.stdout)
        self.assertEqual(failed.stdout.count('FILE_DELETE'), 1)
        kept = self.finish(keep=1)
        self.assertEqual(kept.returncode, 0)
        self.assertIn('CLEANUP=0', kept.stdout)
        self.assertNotIn('FILE_DELETE', kept.stdout)

    def test_tampered_command_cannot_execute(self):
        record = '{"work":"/owned/run","script":"/owned/base/evidence/run/linkload/cleanup-files.sh"}'
        with patch.object(cleanup, 'checked_path', return_value=Path('/owned/run')), \
                patch.object(Path, 'is_symlink', return_value=False), \
                patch.object(Path, 'read_text', side_effect=[record, 'rm -rf /unexpected']), \
                patch.object(subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'differs'):
                cleanup.execute(Path('/owned'), Path('/owned/base'), 'run', 'linkload')
            run.assert_not_called()


class FinalReceipts(unittest.TestCase):
    def run_trap(self, *, prepared=1, complete=1, failed=0):
        source = (Path(__file__).parent / 'world.sh').read_text()
        function = 'proof_done() {' + source.split('proof_done() {', 1)[1]
        # Route the two receipt writes to the captured pipe. Every external
        # command is a test double; the uncreated fixture paths stay in-project.
        function = function.replace('> "$EVIDENCE/stack.log"', '> /dev/null') \
            .replace('> "$EVIDENCE/cleanup.json"', '') \
            .replace('> "$EVIDENCE/runner.json"', '')
        stubs = f'''
FAILED={failed}; CHECKED=1; SMOKE_STACK_PREPARED={prepared}
SMOKE_REPO=.; SMOKE_BASE=.tmp/in-memory-proof; SMOKE_RUN=run; SCENARIO=linkload
WORK=.tmp/in-memory-proof/run; EVIDENCE=.tmp/in-memory-proof/evidence
STACK=.tmp/in-memory-proof/run/deploy; WSS_GATE_QUALIFIED=true
python3() {{ return 0; }}; docker() {{ return 0; }}; compose() {{ return 0; }}
finish() {{ SMOKE_CLEANUP_COMPLETE={complete}; return {0 if complete else 1}; }}
'''
        result = subprocess.run(['bash', '-c', stubs + function + '\ntrue; proof_done\n'],
            capture_output=True, text=True)
        return result, [json.loads(line) for line in result.stdout.splitlines() if line.startswith('{')]

    def test_any_final_failure_cannot_retain_qualified_true(self):
        result, records = self.run_trap(failed=1)
        self.assertNotEqual(result.returncode, 0)
        self.assertTrue(records[0]['complete'], 'functional failure does not invent cleanup failure')
        self.assertFalse(records[1]['wss_release_gate_qualified'])
        self.assertNotEqual(records[1]['final_exit'], 0)

    def test_partial_setup_never_claims_complete_cleanup(self):
        result, records = self.run_trap(complete=0)
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(records[0]['complete'])
        self.assertFalse(records[1]['wss_release_gate_qualified'])
        before_ownership, records = self.run_trap(prepared=0, complete=0)
        self.assertNotEqual(before_ownership.returncode, 0)
        self.assertEqual(records, [], 'failed preflight must not overwrite earlier run evidence')


if __name__ == '__main__':
    unittest.main()
