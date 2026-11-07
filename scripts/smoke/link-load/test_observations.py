"""Synthetic verifier controls; never actual transport or release qualification."""
from copy import deepcopy
import json
from pathlib import PurePosixPath
import unittest
from unittest.mock import patch
import uuid

from observations import COUNTERS, queues, verify_phase, verify_final
from qa import ROLES
from test_qa import preflight, snapshot
from verify import release_index


def fixture(phase='concurrent'):
    machine = str(uuid.uuid4())
    pinned = {'run': 'ownedqa', 'machine': machine, 'instances': {role: str(uuid.uuid4()) for role in ROLES}}
    records = []
    for boundary, elapsed in [('start', 100), ('drained', 2000000)]:
        request = {'run': 'ownedqa', 'id': str(uuid.uuid4()), 'phase': phase, 'boundary': boundary}
        processes = {}
        for role in ROLES:
            row = snapshot(role)
            row['identity'].update(machine=machine, instance=pinned['instances'][role])
            row.update(clock_domain=pinned['instances'][role], elapsed_micros=elapsed, checkpoint=request, open_drained=boundary != 'start')
            row['observations'].update(actual_connector_machine=machine, overflowed=False, commands=[])
            for queue in row['observations']['queues']:
                queue.update({key: 0 for key in COUNTERS})
                queue.update(receiver_closed=False, inconsistent=False, indeterminate_close_race=False)
                if boundary == 'drained':
                    queue.update(enqueue_attempts=3, selected=3, sent=2, processed=1, peak_messages=2,
                        peak_payload_bytes=64, peak_retained_payload_bytes=128)
            processes[role] = row
        records.append({'request': request, 'processes': processes})
    request_id, command_id, session = (str(uuid.uuid4()) for _ in range(3))
    command = {'id': command_id, 'request_id': request_id, 'machine_id': machine, 'gateway_session_id': session,
        'seen': [1, 0, 0, 1], 'api_ack_micros': 900000, 'carrier_ack_micros': None}
    records[1]['processes']['api']['observations']['commands'] = [command]
    records[1]['processes']['gateway']['observations']['commands'] = [command | {
        'request_id': None, 'seen': [0, 1, 1, 0], 'api_ack_micros': None, 'carrier_ack_micros': 800000}]
    client = {'profile': 'floor' if phase == 'floor' else 'profile', 'mixed': {
        'probes': [{'request_id': request_id, 'active_bulk_streams': 1, 'completed': True,
            'fault_before': {'run': 'ownedqa', 'generation': 2, 'active': False},
            'fault_after': {'run': 'ownedqa', 'generation': 2, 'active': False}}]}}
    return client, records[0], records[1], preflight(), pinned


class ActualObservation(unittest.TestCase):
    def test_own_clock_pairs_and_positive_open_drain_are_kept_separate_from_release(self):
        result = verify_phase(*fixture(), 'concurrent')
        self.assertTrue(result['qa_phase_observed'])
        self.assertFalse(result['complete_wss_release_gate'])
        self.assertEqual(result['own_clock_ack_samples'][0]['carrier_ack_micros'], 800000)

    def test_inflight_unknown_closed_discard_failed_and_incomplete_never_masquerade_as_drain(self):
        _, _, end, _, _ = fixture()
        for key, value in [('in_flight_payload_bytes', 1), ('in_flight_unknown_sizes', 1),
                ('unknown_sizes_seen', 1), ('receiver_closed', True), ('discarded_on_receiver_close', 1),
                ('failed_enqueues', 1), ('selected_without_completion', 1), ('processed', 0)]:
            row = deepcopy(end['processes']['api']); row['observations']['queues'][0][key] = value
            with self.assertRaises(ValueError): queues(row, True)

    def test_other_request_machine_session_or_clock_and_slow_ack_are_rejected(self):
        for change in ('request', 'machine', 'session', 'clock', 'late', 'api_late', 'duplicate'):
            client, before, after, inputs, pinned = fixture()
            command = after['processes']['gateway']['observations']['commands'][0]
            if change == 'request': client['mixed']['probes'][0]['request_id'] = str(uuid.uuid4())
            if change == 'machine': command['machine_id'] = str(uuid.uuid4())
            if change == 'session': command['gateway_session_id'] = str(uuid.uuid4())
            if change == 'clock': after['processes']['gateway']['clock_domain'] = pinned['instances']['api']
            if change == 'late': command['carrier_ack_micros'] = 1000001
            if change == 'api_late': after['processes']['api']['observations']['commands'][0]['api_ack_micros'] = 1000001
            if change == 'duplicate': command['seen'][1] = 2
            with self.assertRaises(ValueError): verify_phase(client, before, after, inputs, pinned, 'concurrent')

    def test_floor_keeps_exact_join_and_completion_but_exempts_latency(self):
        data = fixture('floor')
        data[2]['processes']['api']['observations']['commands'][0]['api_ack_micros'] = 1500000
        result = verify_phase(*data, 'floor')
        self.assertEqual(result['command_ack_budget'], 'not_required_at_floor')
        self.assertEqual(len(result['own_clock_ack_samples']), 1)
        for change in ('empty', 'request', 'command', 'machine', 'session', 'incomplete'):
            client, before, after, inputs, pinned = deepcopy(data)
            command = after['processes']['gateway']['observations']['commands'][0]
            if change == 'empty':
                for role in ROLES: after['processes'][role]['observations']['commands'] = []
            if change == 'request': client['mixed']['probes'][0]['request_id'] = None
            if change == 'command': command['id'] = str(uuid.uuid4())
            if change == 'machine': command['machine_id'] = str(uuid.uuid4())
            if change == 'session': command['gateway_session_id'] = str(uuid.uuid4())
            if change == 'incomplete': command['seen'][2] = 0
            with self.assertRaises(ValueError): verify_phase(client, before, after, inputs, pinned, 'floor')

    def test_reconnected_clean_retirement_remains_evidence_with_active_replacement(self):
        data = fixture()
        after = data[2]
        for role in ROLES:
            rows = after['processes'][role]['observations']['queues']
            original = rows[0]
            rows.append(deepcopy(original))
            original['receiver_closed'] = True
            original['drained_retirement'] = {key: original[key] for key in ('enqueue_attempts', 'selected', 'sent', 'processed')}
        result = verify_phase(*data, 'concurrent')
        self.assertEqual(result['queue_peaks']['api'][0]['lifecycle'], 'retired_drained')
        for change in ('no_replacement', 'inflight', 'discard', 'late_completion', 'unknown', 'overflow', 'no_close_witness'):
            client, before, after, inputs, pinned = deepcopy(data)
            process = after['processes']['api']; row = process['observations']['queues'][0]
            if change == 'no_replacement': process['observations']['queues'].pop()
            if change == 'inflight': row['in_flight_payload_bytes'] = 1
            if change == 'discard': row['discarded_on_receiver_close'] = 1
            if change == 'late_completion': row['drained_retirement']['processed'] -= 1
            if change == 'unknown': row['unknown_sizes_seen'] = 1
            if change == 'overflow': process['observations']['overflowed'] = True
            if change == 'no_close_witness': row['drained_retirement'] = None
            with self.assertRaises(ValueError): verify_phase(client, before, after, inputs, pinned, 'concurrent')

    def test_final_shutdown_requires_prior_open_drain_and_same_finished_process(self):
        _, _, opened, inputs, pinned = fixture()
        opened['request'].update(phase='shutdown', boundary='pre_stop')
        for row in opened['processes'].values(): row['checkpoint'] = opened['request']
        final = {'complete': True, 'failures': [], 'processes': deepcopy(opened['processes'])}
        for row in final['processes'].values(): row.update(state='finished', elapsed_micros=3000000)
        verify_final(opened, final, inputs, pinned)
        final['processes']['api']['state'] = 'abandoned'
        with self.assertRaises(ValueError): verify_final(opened, final, inputs, pinned)


class MemoryPath:
    def __init__(self, path, index): self.path, self.index = PurePosixPath(path), index
    def read_text(self): return json.dumps(self.index)
    def resolve(self, **_): return self
    @property
    def parent(self): return MemoryPath(self.path.parent, self.index)
    def __truediv__(self, part): return MemoryPath(self.path / part, self.index)
    def is_relative_to(self, other): return self.path.is_relative_to(other.path)
    def __hash__(self): return hash(self.path)
    def __eq__(self, other): return isinstance(other, MemoryPath) and self.path == other.path


class ThreeLegIndex(unittest.TestCase):
    def index(self):
        return {'source': 'a' * 40, 'placements': {leg: {mode: leg + mode for mode in ('normal', 'qa')}
            for leg in ('client', 'connector', 'backplane')}}
    def case(self, path, source, leg, mode):
        return {'run': leg + mode, 'mode': mode, 'source': source, 'manifest_sha256': 'same-package',
            'normal_programs': {role: {'sha256': 'normal-' + role} for role in ROLES},
            'qa_programs': None if mode == 'normal' else {role: 'qa-' + role for role in ROLES}}
    def test_all_attributed_legs_still_leave_the_combined_concurrency_gate_open(self):
        with patch('verify.read_case', side_effect=self.case):
            result = release_index(MemoryPath('/owned/index.json', self.index()), 'a' * 40)
            self.assertTrue(result['attributed_mixed_cases_observed'])
            self.assertFalse(result['wss_release_gate_qualified'])
            self.assertIn('combined_64_live_16_waiting_on_actual_impaired_wss', result['missing_or_incomplete'])
            for omitted in ('client', 'connector', 'backplane'):
                index = self.index(); del index['placements'][omitted]
                with self.assertRaises(ValueError): release_index(MemoryPath('/owned/index.json', index), 'a' * 40)
            index = self.index(); index['placements']['backplane']['qa'] = index['placements']['connector']['qa']
            with self.assertRaises(ValueError): release_index(MemoryPath('/owned/index.json', index), 'a' * 40)


if __name__ == '__main__':
    unittest.main()
