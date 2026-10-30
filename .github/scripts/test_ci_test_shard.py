"""Test plans cover every workspace target once and continue after failures."""

from collections import Counter
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ci_test_shard


def package(name, tests):
    targets = [{"name": name.replace("-", "_"), "kind": ["lib"], "test": True, "doctest": True}]
    targets += [{"name": f"case_{n}", "kind": ["test"], "test": True, "doctest": False} for n in range(tests)]
    targets.append({"name": "disabled", "kind": ["test"], "test": False, "doctest": False})
    return {"name": name, "targets": targets}


class ShardTests(unittest.TestCase):
    def metadata(self):
        return {"packages": [package("orch-api-core", 17), package("orch-engine", 8),
                             *(package(f"other-{n}", n) for n in range(1, 9))]}

    def test_large_package_targets_are_disjoint_and_cover_every_enabled_test(self):
        metadata = self.metadata()
        for kind, (package_name, count) in ci_test_shard.PARTS.items():
            selected = []
            for index in range(count):
                commands = ci_test_shard.commands(metadata, f"{kind}-{index}")
                integration = commands[0]
                selected += [integration[i + 1] for i, part in enumerate(integration) if part == "--test"]
                self.assertTrue(all("--no-fail-fast" in command for command in commands))
                self.assertEqual(len(commands), 3 if index == 0 else 1)
                if index == 0:
                    self.assertIn("--lib", commands[1])
                    self.assertIn("--doc", commands[2])
            expected = ci_test_shard.integration_targets(next(p for p in metadata["packages"] if p["name"] == package_name))
            self.assertEqual(Counter(selected), Counter(expected))

    def test_other_packages_are_each_assigned_once(self):
        metadata = self.metadata()
        selected = []
        for index in range(ci_test_shard.OTHER_SHARDS):
            command, = ci_test_shard.commands(metadata, f"rest-{index}")
            self.assertIn("--no-fail-fast", command)
            selected += [command[i + 1] for i, part in enumerate(command) if part == "-p"]
        expected = [p["name"] for p in metadata["packages"] if p["name"].startswith("other-")]
        self.assertEqual(Counter(selected), Counter(expected))

    def test_invalid_shards_are_refused(self):
        for shard in ("api-6", "engine-3", "rest-4", "other-0", "api-x"):
            with self.subTest(shard=shard), self.assertRaises(ValueError):
                ci_test_shard.commands(self.metadata(), shard)

    def test_a_failed_program_does_not_hide_the_unit_or_documentation_tests(self):
        metadata = SimpleNamespace(stdout=json.dumps(self.metadata()))
        results = [metadata, SimpleNamespace(returncode=1), SimpleNamespace(returncode=0), SimpleNamespace(returncode=0)]
        with patch("sys.argv", ["ci_test_shard.py", "--shard", "api-0"]), patch.object(ci_test_shard.subprocess, "run", side_effect=results) as run:
            self.assertEqual(ci_test_shard.main(), 1)
            self.assertEqual(run.call_count, 4)


if __name__ == "__main__":
    unittest.main()
