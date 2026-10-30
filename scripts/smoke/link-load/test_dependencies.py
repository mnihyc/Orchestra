"""A dependency check cannot be borrowed from a different candidate or policy."""
import copy
import unittest

from dependencies import verified_result


class DependencyEvidence(unittest.TestCase):
    def test_only_the_exact_clean_candidate_inputs_and_green_check_attest(self):
        inputs = {"deny.toml": "policy", "Cargo.lock": "rust", "frontend/package-lock.json": "web"}
        proof = {"candidate": {"commit": "candidate", "clean": True, "dependency_inputs": inputs},
                 "steps": [{"step": "deny", "exit": 0}]}
        self.assertTrue(verified_result(proof, "candidate", inputs))
        for key, value in (("commit", "earlier"), ("clean", False), ("dependency_inputs", {})):
            stale = copy.deepcopy(proof)
            stale["candidate"][key] = value
            self.assertFalse(verified_result(stale, "candidate", inputs))
        proof["steps"][0]["exit"] = 7
        self.assertFalse(verified_result(proof, "candidate", inputs))
        self.assertFalse(verified_result({}, "candidate", inputs))
