#!/usr/bin/env python3
"""A short run, duplicate resend or silently pending turn cannot become release proof."""
import unittest
from run import duration_result, ledger_check


class SoakEvidence(unittest.TestCase):
    def test_a_short_run_never_claims_a_day(self):
        for requested, elapsed in [(1800, 1801), (86400, 1800), (1800, 86401)]:
            self.assertFalse(duration_result(requested, elapsed)['full_24_hours'])
            self.assertTrue(duration_result(requested, elapsed)['owed_24_hours'])
        self.assertTrue(duration_result(86400, 86401)['full_24_hours'])

    def test_missing_duplicate_or_pending_database_work_is_refused(self):
        expected = [{'key': 'one', 'input': 'input'}]
        actual = {'key': 'one', 'input': 'input', 'turns': 1, 'state': 'completed', 'outputs': 1}
        ledger_check(expected, [actual])
        for wrong in [[], [actual, actual], [{**actual, 'turns': 2}], [{**actual, 'state': 'running'}],
            [{**actual, 'outputs': 0}], [{**actual, 'outputs': 2}]]:
            with self.assertRaises(ValueError):
                ledger_check(expected, wrong)


if __name__ == '__main__':
    unittest.main()
