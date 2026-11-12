"""Pure QA provenance and snapshot rejection controls; no Docker or filesystem mutation."""
from copy import deepcopy
from pathlib import Path
import hashlib
import unittest
from unittest.mock import patch
import uuid

from qa import ROLES, REQUIRED_QUEUES, build_record, environment, validate_snapshot


def preflight():
    return {'source': 'a' * 40, 'version': '0.1.0', 'tag': 'v0.1.0', 'max_commands': 8,
        'programs': {role: {'sha256': '1' * 64} for role in ROLES}}


def snapshot(role='api'):
    instance, machine = str(uuid.uuid4()), str(uuid.uuid4())
    return {'scope': 'explicit_test_observers_qa_process', 'state': 'running', 'complete_wss_release_gate': False,
        'identity': {'run': 'ownedqa', 'component': role, 'machine': machine, 'instance': instance, 'process': 11,
            'declared_source_commit': 'a' * 40, 'executable_sha256': '1' * 64, 'max_commands': 8,
            'build': {'component': role, 'version': '0.1.0', 'tag': 'v0.1.0'}},
        'clock_domain': instance, 'observations': {'actual_connector_machine': machine,
            'queues': [{'label': label, 'receiver_closed': False, 'drained_retirement': None} for label in REQUIRED_QUEUES[role]]}}


class QAInputs(unittest.TestCase):
    def clean(self):
        return {boundary: {'head': 'a' * 40, 'diff_sha256': hashlib.sha256(b'').hexdigest(), 'untracked_source': {}}
            for boundary in ('source_before', 'source_after')}
    def test_build_receipts_keep_normal_and_explicit_qa_selection_distinct(self):
        record = {'source': 'a' * 40, 'source_unchanged': True, 'exit_code': 0,
            'command': ['cargo', 'build', '--locked', '-p', 'orch-api']} | self.clean()
        with patch('qa.read', return_value=record), patch('qa.sha', return_value='digest'):
            self.assertEqual(build_record(Path('memory'), 'a' * 40)['selection'], 'normal')
            with self.assertRaises(ValueError): build_record(Path('memory'), 'a' * 40, 'orch-api')
            record['command'] += ['--features', 'orch-api/test-observers']
            # A command alone is deliberately no longer executable provenance.
            with self.assertRaises(ValueError): build_record(Path('memory'), 'a' * 40, 'orch-api')
            with self.assertRaises(ValueError): build_record(Path('memory'), 'a' * 40)

    def test_changed_failed_check_only_and_all_features_receipts_are_rejected(self):
        base = {'source': 'a' * 40, 'source_unchanged': True, 'exit_code': 0, 'command': ['cargo', 'build']} | self.clean()
        for changes in ({'source': 'b' * 40}, {'source_unchanged': False}, {'exit_code': 1},
                {'command': ['cargo', 'xtask', 'package', '--check']}, {'command': ['cargo', 'build', '--all-features']},
                {'source_before': {'head': 'a' * 40, 'diff_sha256': 'dirty', 'untracked_source': {}}}):
            with patch('qa.read', return_value=base | changes), patch('qa.sha', return_value='digest'):
                with self.assertRaises(ValueError): build_record(Path('memory'), 'a' * 40)

    def test_qa_environment_is_metadata_only_and_has_no_product_setting(self):
        values = environment(preflight(), 'ownedqa', 'machine')
        self.assertTrue(all(name.startswith('ORCH_TEST_WSS_') for name in values))
        self.assertEqual(values['ORCH_TEST_WSS_OBSERVER_DIR'], '/qa-observers')


class ProcessIdentity(unittest.TestCase):
    def test_each_actual_instance_matches_its_binary_source_claim_and_compiled_build(self):
        for role in ROLES:
            row = snapshot(role)
            self.assertEqual(validate_snapshot(row, preflight(), 'ownedqa', row['identity']['machine'], role), row['identity']['instance'])

    def test_stopped_missing_changed_hash_source_clock_and_instance_never_count_as_current(self):
        original = snapshot()
        mutations = [lambda row: row.update(state='finished'), lambda row: row.update(complete_wss_release_gate=True),
            lambda row: row['identity'].update(executable_sha256='2' * 64),
            lambda row: row['identity'].update(declared_source_commit='b' * 40),
            lambda row: row['identity']['build'].update(version='0.2.0'),
            lambda row: row.update(clock_domain=str(uuid.uuid4())),
            lambda row: row['identity'].update(process=None),
            lambda row: row['observations'].update(queues=[])]
        for mutate in mutations:
            row = deepcopy(original); mutate(row)
            with self.assertRaises(ValueError):
                validate_snapshot(row, preflight(), 'ownedqa', original['identity']['machine'], 'api', original['identity']['instance'])
        with self.assertRaises(ValueError):
            validate_snapshot(original, preflight(), 'ownedqa', original['identity']['machine'], 'api', str(uuid.uuid4()))

    def test_connector_must_report_the_actual_enrolled_identity(self):
        row = snapshot('connector')
        row['observations']['actual_connector_machine'] = str(uuid.uuid4())
        with self.assertRaises(ValueError):
            validate_snapshot(row, preflight(), 'ownedqa', row['identity']['machine'], 'connector')


if __name__ == '__main__':
    unittest.main()
