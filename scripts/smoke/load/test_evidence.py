#!/usr/bin/env python3
"""No successful load result may hide duplicated work or changed payloads."""
import unittest
from copy import deepcopy
from common import check_work, memory_bytes


class WorkEvidence(unittest.TestCase):
    def snapshot(self):
        return {'seq': 9, 'inputs': [{'id': 'input'}],
            'turns': [{'id': 'turn', 'input_id': 'input', 'state': 'completed'}],
            'items': [{'kind': 'agent_message', 'payload': {'text': 'unique'}}]}

    def test_an_exact_completion_is_required(self):
        sync = self.snapshot()
        self.assertEqual(check_work(sync, 'input', 'unique')['outputs'], 1)
        for field in ('inputs', 'turns', 'items'):
            duplicate = deepcopy(sync)
            duplicate[field] += duplicate[field]
            with self.assertRaises(ValueError):
                check_work(duplicate, 'input', 'unique')
        sync['items'][0]['payload']['text'] = 'another stream'
        with self.assertRaises(ValueError):
            check_work(sync, 'input', 'unique')

    def test_a_faults_terminal_failure_stays_a_named_failure(self):
        sync = self.snapshot()
        sync['turns'][0]['state'] = 'interrupted'
        sync['items'] = []
        with self.assertRaises(ValueError):
            check_work(sync, 'input', 'unique')
        self.assertEqual(check_work(sync, 'input', 'unique', allow_interruption=True)['state'], 'interrupted')

    def test_docker_units_are_not_confused(self):
        self.assertEqual(memory_bytes('1.5MiB / 8GiB'), 1572864)
        self.assertEqual(memory_bytes('1.5MB / 8GB'), 1500000)


if __name__ == '__main__':
    unittest.main()
