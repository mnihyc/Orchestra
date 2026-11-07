"""Pure compiler-event/provenance controls. No Cargo or compiler is executed."""
from copy import deepcopy
import io
import hashlib
import json
from pathlib import Path
from types import SimpleNamespace
import unittest
from unittest.mock import MagicMock, patch

from build_capture import artifact_events, build_target, builder_invocation, capture, produced_path, selection
from qa import build_record

REPO = Path('/owned/source')
PROGRAM = Path('/owned/target/release/orch-api')
COMMAND = ['cargo', 'build', '--release', '--locked', '--message-format=json', '-p', 'orch-api', '--features', 'orch-api/test-observers']
FINISHED = {'reason': 'build-finished', 'success': True}


def artifact():
    return {'reason': 'compiler-artifact', 'package_id': 'path+file:///owned/source/apps/api#orch-api@0.1.0',
        'manifest_path': str(REPO / 'apps/api/Cargo.toml'), 'target': {'name': 'orch-api', 'kind': ['bin']},
        'profile': {'test': False, 'opt_level': '3', 'debug_assertions': False, 'overflow_checks': False}, 'features': ['default', 'test-observers'], 'fresh': False, 'executable': str(PROGRAM)}


def receipt():
    return {'producer': 'wss_qa_cargo_capture_v1', 'source': 'a' * 40, 'worktree': str(REPO),
        'source_unchanged': True, 'exit_code': 0, 'command': COMMAND, 'successful_capture': True,
        'compiler': 'rustc observed-version\nhost: x86_64-unknown-linux-gnu', 'build_finished': FINISHED,
        'build_target': 'x86_64-unknown-linux-gnu',
        'execution': {'producer_worktree': str(REPO), 'command': COMMAND, 'target_mount': None, 'host_target': None, 'image_id': None},
        'artifacts': {'orch-api': {'source': 'a' * 40, 'path': str(PROGRAM), 'sha256': 'b' * 64,
            'compiler_artifact': artifact(), 'host_path': str(PROGRAM), 'build_target': 'x86_64-unknown-linux-gnu'}}} | {boundary: {'head': 'a' * 40, 'diff_sha256': hashlib.sha256(b'').hexdigest(), 'untracked_source': {}}
        for boundary in ('source_before', 'source_after')}


class BuildCapture(unittest.TestCase):
    def validate(self, record, program=PROGRAM, digest='b' * 64):
        with patch('qa.read', return_value=record), patch('qa.sha', return_value=digest), \
                patch.object(Path, 'resolve', autospec=True, side_effect=lambda path, **_: path):
            return build_record(Path('/owned/receipt.json'), 'a' * 40, 'orch-api', program)

    def test_actual_emitted_path_hash_feature_source_and_compiler_are_bound(self):
        result = self.validate(receipt())
        self.assertTrue(result['artifact_bound'])
        self.assertEqual(result['artifact']['path'], str(PROGRAM))
        self.assertEqual(result['artifact']['compiler_artifact']['features'], ['default', 'test-observers'])
        self.assertEqual(result['selection'], 'explicit_test_observers')

    def test_successful_command_cannot_retrofit_old_different_output(self):
        for change in ('legacy', 'other_hash', 'other_source', 'other_worktree', 'debug_profile', 'wrong_target',
                'missing_feature', 'other_feature', 'test_executable', 'failed_finish', 'failed_capture', 'missing_compiler'):
            record = deepcopy(receipt()); item = record['artifacts']['orch-api']; event = item['compiler_artifact']
            program, digest = PROGRAM, 'b' * 64
            if change == 'legacy': record.pop('producer'); record.pop('artifacts')
            if change == 'other_hash': digest = 'c' * 64
            if change == 'other_source': item['source'] = 'c' * 40
            if change == 'other_worktree': record['execution']['producer_worktree'] = '/owned/old-source'
            if change == 'debug_profile': event['profile']['opt_level'] = '0'
            if change == 'wrong_target': record['build_target'] = 'x86_64-unknown-linux-musl'
            if change == 'missing_feature': event['features'] = ['default']
            if change == 'other_feature': event['features'].append('unknown')
            if change == 'test_executable': event['profile']['test'] = True
            if change == 'failed_finish': record['build_finished'] = {'reason': 'build-finished', 'success': False}
            if change == 'failed_capture': record['successful_capture'] = False
            if change == 'missing_compiler': record.pop('compiler')
            with self.subTest(change=change), self.assertRaises(ValueError): self.validate(record, program, digest)

    def test_explicit_copy_preserves_original_compiler_path_and_requires_its_captured_sha(self):
        copied = Path('/owned/qa-inputs/orch-api')
        result = self.validate(receipt(), copied)
        self.assertEqual(result['artifact']['path'], str(PROGRAM))
        self.assertEqual(result['selected_input']['path'], str(copied))
        self.assertEqual(result['selected_input']['binding'], 'byte_identical_to_captured_compiler_output')
        with self.assertRaises(ValueError): self.validate(receipt(), copied, 'c' * 64)

    def test_release_and_native_role_targets_follow_the_existing_package_builder(self):
        compiler = 'rustc observed\nhost: x86_64-unknown-linux-gnu'
        self.assertEqual(build_target(COMMAND, ['orch-api'], compiler), 'x86_64-unknown-linux-gnu')
        connector = ['cargo', 'build', '--release', '--locked', '--message-format=json', '-p', 'orch-connector',
            '--features', 'orch-connector/test-observers', '--target', 'x86_64-unknown-linux-musl']
        self.assertEqual(build_target(connector, ['orch-connector'], compiler), 'x86_64-unknown-linux-musl')
        for command, packages in [(COMMAND + ['--target', 'x86_64-unknown-linux-musl'], ['orch-api']),
                (connector[:-2], ['orch-connector']), (connector[:-1] + ['aarch64-unknown-linux-musl'], ['orch-connector'])]:
            with self.assertRaises(ValueError): build_target(command, packages, compiler)
        for command in ([part for part in COMMAND if part != '--release'], COMMAND + ['--profile', 'dev'], COMMAND + ['--config', 'build.target=other']):
            with self.assertRaises(ValueError): selection(command)

    def test_host_capture_maps_only_existing_builder_mounts_and_needs_no_in_image_python(self):
        args = SimpleNamespace(builder_image='sha256:' + '1' * 64, target_dir=Path('/owned/cache/target'),
            cargo_home=Path('/owned/cache/cargo-home'), counters=Path('/owned/counters'),
            release_version='0.1.0', release_tag='v0.1.0')
        with patch.object(Path, 'resolve', autospec=True, side_effect=lambda path, **_: path), \
                patch.object(Path, 'is_dir', return_value=True):
            invocation = builder_invocation(args, REPO, COMMAND)
        command = invocation['command']
        self.assertEqual(command[-len(COMMAND):], COMMAND)
        self.assertIn('/owned/source:/work:ro', command)
        self.assertIn('/owned/cache/target:/target', command)
        self.assertNotIn('python3', command)
        row = {'path': '/target/release/orch-api', 'compiler_artifact': artifact()}
        self.assertEqual(produced_path(row, invocation, 'x86_64-unknown-linux-gnu'), Path('/owned/cache/target/release/orch-api'))
        with self.assertRaises(ValueError): produced_path(row | {'path': '/unrelated/orch-api'}, invocation, 'x86_64-unknown-linux-gnu')

    def test_compiler_stream_needs_unique_executable_and_successful_completion(self):
        self.assertEqual(artifact_events([artifact(), FINISHED], ['orch-api'], REPO)['orch-api']['path'], str(PROGRAM))
        for events in ([artifact()], [FINISHED], [artifact(), artifact(), FINISHED], [artifact(), FINISHED, FINISHED]):
            with self.assertRaises(ValueError): artifact_events(events, ['orch-api'], REPO)
        for command in (['cargo', 'check'], COMMAND + ['--all-features'], COMMAND + ['--message-format=json'],
                [part for part in COMMAND if part != '--message-format=json']):
            with self.assertRaises(ValueError): selection(command)

    def test_producer_hashes_only_own_successful_stream_artifact_with_frozen_boundaries(self):
        expected = receipt()
        child = MagicMock()
        child.__enter__.return_value = child
        child.stdout = io.StringIO(json.dumps(artifact()) + '\n' + json.dumps(FINISHED) + '\n')
        child.wait.return_value = 0
        args = SimpleNamespace(repo=REPO, source='a' * 40, output=Path('/owned/receipt.json'), command=['--', *COMMAND])
        with patch.object(Path, 'resolve', autospec=True, side_effect=lambda path, **_: path), \
                patch.object(Path, 'exists', return_value=False), patch('build_capture.source_state', return_value=expected['source_before']), \
                patch('build_capture.subprocess.check_output', return_value='rustc controlled-fixture\nhost: x86_64-unknown-linux-gnu'), \
                patch('build_capture.subprocess.Popen', return_value=child) as launch, \
                patch('build_capture.sha', return_value='b' * 64) as digest, patch('build_capture.write') as output:
            capture(args)
            launch.assert_called_once_with(COMMAND, cwd=REPO, stdout=-1, text=True)
            digest.assert_called_once_with(PROGRAM)
            record = output.call_args.args[1]
            self.assertTrue(record['successful_capture'])
            self.assertEqual(record['artifacts'], expected['artifacts'])
            self.assertFalse(record['complete_wss_release_gate'])

    def test_container_capture_hashes_mapped_output_from_its_actual_cargo_event(self):
        event = artifact() | {'manifest_path': '/work/apps/api/Cargo.toml', 'executable': '/target/release/orch-api'}
        child = MagicMock(); child.__enter__.return_value = child; child.wait.return_value = 0
        child.stdout = io.StringIO(json.dumps(event) + '\n' + json.dumps(FINISHED) + '\n')
        args = SimpleNamespace(repo=REPO, source='a' * 40, output=Path('/owned/receipt.json'), command=COMMAND,
            builder_image='sha256:' + '1' * 64, target_dir=Path('/owned/cache/target'),
            cargo_home=Path('/owned/cache/cargo-home'), counters=Path('/owned/counters'),
            release_version='0.1.0', release_tag='v0.1.0')
        with patch.object(Path, 'resolve', autospec=True, side_effect=lambda path, **_: path), \
                patch.object(Path, 'is_dir', return_value=True), patch.object(Path, 'exists', return_value=False), \
                patch('build_capture.source_state', return_value=receipt()['source_before']), \
                patch('build_capture.subprocess.check_output', return_value='rustc observed\nhost: x86_64-unknown-linux-gnu'), \
                patch('build_capture.subprocess.Popen', return_value=child) as launch, \
                patch('build_capture.sha', return_value='b' * 64) as digest, patch('build_capture.write') as output:
            capture(args)
            record = output.call_args.args[1]
            self.assertEqual(launch.call_args.args[0], record['execution']['command'])
            digest.assert_called_once_with(Path('/owned/cache/target/release/orch-api'))
            item = record['artifacts']['orch-api']
            self.assertEqual(item['path'], '/target/release/orch-api')
            self.assertEqual(item['host_path'], '/owned/cache/target/release/orch-api')
            self.assertEqual(record['execution']['producer_worktree'], '/work')
            self.assertTrue(record['successful_capture'])
        bound = self.validate(record, Path('/owned/copied-input/orch-api'))
        self.assertEqual(bound['artifact']['path'], '/target/release/orch-api')
        self.assertEqual(bound['selected_input']['path'], '/owned/copied-input/orch-api')

    def test_failed_build_or_changed_source_never_emits_a_successful_capture(self):
        for outcome in ('cargo_failure', 'changed_source', 'missing_artifact'):
            child = MagicMock(); child.__enter__.return_value = child
            child.wait.return_value = 1 if outcome == 'cargo_failure' else 0
            events = [FINISHED] if outcome == 'missing_artifact' else [artifact(), FINISHED]
            child.stdout = io.StringIO(''.join(json.dumps(row) + '\n' for row in events))
            original = receipt()['source_before']
            changed = original | {'diff_sha256': 'changed'} if outcome == 'changed_source' else original
            args = SimpleNamespace(repo=REPO, source='a' * 40, output=Path('/owned/receipt.json'), command=COMMAND)
            with patch.object(Path, 'resolve', autospec=True, side_effect=lambda path, **_: path), \
                    patch.object(Path, 'exists', return_value=False), patch('build_capture.source_state', side_effect=[original, changed]), \
                    patch('build_capture.subprocess.check_output', return_value='rustc controlled-fixture\nhost: x86_64-unknown-linux-gnu'), \
                    patch('build_capture.subprocess.Popen', return_value=child), \
                    patch('build_capture.sha', return_value='b' * 64), patch('build_capture.write') as output:
                with self.subTest(outcome=outcome), self.assertRaises(ValueError): capture(args)
                self.assertFalse(output.call_args.args[1]['successful_capture'])


if __name__ == '__main__':
    unittest.main()
