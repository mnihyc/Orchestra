"""Pure workload/budget controls; no real transfer, shaping, service or clock acceleration proof."""
import asyncio
import unittest

from faults import classify, verify_history
from workload import MIB, LARGE, PRIMARY, PHASES, HarnessBudgetExpired, failure, phase, payload_floor_seconds, within_budget


class Allocation(unittest.TestCase):
    def test_primary_keeps_full_floor_and_no_duplicate_large_reconnect_stream(self):
        for name in ('concurrent', 'floor'):
            selected = phase(PRIMARY, name)
            self.assertEqual((selected['bytes'], selected['streams']), (LARGE, 1))
        self.assertEqual(phase(PRIMARY, 'reconnect')['bytes'], MIB)
        self.assertTrue(phase(PRIMARY, 'cut')['cut'])
        self.assertEqual(phase(PRIMARY, 'cut')['bytes'], LARGE)  # Abort after a MiB; not claimed complete.
        self.assertGreater(payload_floor_seconds(LARGE, 'floor'), 7200)
        self.assertGreater(phase(PRIMARY, 'floor')['budget_seconds'], payload_floor_seconds(LARGE, 'floor'))

    def test_short_legs_are_real_profile_probes_not_extra_full_bulk_repetitions(self):
        for placement in ('client', 'backplane'):
            for name in PHASES:
                selected = phase(placement, name)
                self.assertEqual(selected['streams'], 1)
                if name != 'cut': self.assertEqual(selected['bytes'], MIB)
            history = [{'run': 'owned', 'generation': 0, 'active': False}]
            verify_history(history, 'owned', blackout=False)
            self.assertEqual(classify({'fault_before': history[0], 'fault_after': history[0]}, history), 'steady')
            with self.assertRaises(ValueError): verify_history(history, 'owned')
        with self.assertRaises(ValueError): phase('host', 'floor')


class Budgets(unittest.IsolatedAsyncioTestCase):
    async def test_budget_stop_keeps_observed_progress_and_cleanup_without_a_product_verdict(self):
        progress, cleaned = {}, asyncio.Event()
        async def ongoing():
            progress['one'] = {'seed': 'one', 'upload_bytes': 64, 'download_bytes': 32, 'last_progress_seconds': 0}
            try: await asyncio.Event().wait()
            finally: cleaned.set()
        with self.assertRaises(HarnessBudgetExpired) as stopped:
            await within_budget(ongoing(), .001)
        self.assertTrue(cleaned.is_set())
        result = failure(stopped.exception, 21600, progress)
        self.assertFalse(result['passed'])
        self.assertEqual(result['failure_domain'], 'harness_budget')
        self.assertEqual(result['assessment'], 'incomplete_no_product_verdict')
        self.assertEqual(result['last_payload_observations'][0]['download_bytes'], 32)

    async def test_inner_observation_timeout_is_not_relabelled_as_outer_work_budget(self):
        async def inner(): raise TimeoutError('inner observation')
        with self.assertRaises(TimeoutError) as stopped:
            await within_budget(inner(), 1)
        self.assertNotIsInstance(stopped.exception, HarnessBudgetExpired)
        self.assertEqual(failure(stopped.exception, 1, {})['failure_domain'], 'observation')
        async def complete(): return 'done'
        self.assertEqual(await within_budget(complete(), 1), 'done')


if __name__ == '__main__':
    unittest.main()
