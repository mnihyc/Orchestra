"""A filtered or ignored candidate cannot be counted as a repeated passing business case."""

import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("repeat_under_load", Path(__file__).with_name("repeat-under-load.py"))
repeat = importlib.util.module_from_spec(spec)
spec.loader.exec_module(repeat)


class RepeatedCaseEvidenceTests(unittest.TestCase):
    def test_only_an_executed_exact_case_is_evidence(self):
        with tempfile.TemporaryDirectory() as name:
            log = Path(name) / "run.log"
            for output, expected in [
                ("test result: ok. 0 passed; 0 failed; 0 ignored; 5 filtered out", False),
                ("test result: ok. 0 passed; 0 failed; 1 ignored; 4 filtered out", False),
                ("test result: ok. 1 passed; 0 failed; 0 ignored; 4 filtered out", True),
                ("test result: \x1b[32mok\x1b[0m. 1 passed; 0 failed; 0 ignored; 4 filtered out", True),
            ]:
                with self.subTest(output=output):
                    log.write_text(output)
                    self.assertEqual(repeat.one_case_ran(log), expected)


if __name__ == "__main__":
    unittest.main()
