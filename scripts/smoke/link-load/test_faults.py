"""The deliberate blackout remains functional evidence, never steady-profile latency proof."""
from copy import deepcopy
import unittest
import uuid

from faults import classify, verify_history
from observations import verify_phase
from test_observations import fixture


class FaultScopes(unittest.TestCase):
    def test_bounded_generation_observations_need_no_cross_clock_subtraction(self):
        history = [{'run': 'ownedqa', 'generation': i, 'active': i == 1} for i in range(3)]
        verify_history(history, 'ownedqa')
        self.assertEqual(classify({'fault_before': history[0], 'fault_after': history[0]}, history), 'steady')
        self.assertEqual(classify({'fault_before': history[0], 'fault_after': history[2]}, history), 'intentional_fault')
        self.assertEqual(classify({'fault_before': history[1], 'fault_after': history[1]}, history), 'intentional_fault')
        with self.assertRaises(ValueError): verify_history(history[:2], 'ownedqa')
        with self.assertRaises(ValueError): classify({'fault_before': history[2], 'fault_after': history[0]}, history)

    def test_fault_probe_still_needs_real_correlation_and_a_steady_loaded_peer(self):
        client, before, after, inputs, pinned = fixture()
        sample = deepcopy(client['mixed']['probes'][0])
        request, command = str(uuid.uuid4()), str(uuid.uuid4())
        sample.update(request_id=request, fault_before={'run': 'ownedqa', 'generation': 0, 'active': False})
        client['mixed']['probes'].append(sample)
        api = deepcopy(after['processes']['api']['observations']['commands'][0])
        gateway = deepcopy(after['processes']['gateway']['observations']['commands'][0])
        api.update(id=command, request_id=request, api_ack_micros=4000000, seen=[2, 0, 0, 2])
        gateway.update(id=command, carrier_ack_micros=3500000, seen=[0, 2, 2, 0])
        after['processes']['api']['observations']['commands'].append(api)
        after['processes']['gateway']['observations']['commands'].append(gateway)
        for process in after['processes'].values(): process['elapsed_micros'] = 5000000
        result = verify_phase(client, before, after, inputs, pinned, 'concurrent')
        self.assertEqual(result['own_clock_ack_samples'][1]['fault_status'], 'intentional_fault')
        client['mixed']['probes'] = [sample]
        with self.assertRaises(ValueError): verify_phase(client, before, after, inputs, pinned, 'concurrent')


if __name__ == '__main__':
    unittest.main()
