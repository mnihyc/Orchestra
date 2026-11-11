"""Memory reporting observes host pressure without changing the build command."""

import io
import json
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import ci_memory_watch


def memory(available, swap_free):
    return {"MemTotal": 16 * 1024, "MemAvailable": available,
            "SwapTotal": 8 * 1024, "SwapFree": swap_free}


class MemoryWatchTests(unittest.TestCase):
    def test_meminfo_uses_available_ram_and_converts_kibibytes_to_bytes(self):
        values = ci_memory_watch.parse_memory(
            "MemTotal: 16 kB\nMemAvailable: 6 kB\nSwapTotal: 8 kB\nSwapFree: 5 kB\nHugePages_Total: 0\n")
        self.assertEqual(values, memory(6 * 1024, 5 * 1024))

    def test_missing_or_inconsistent_accounting_is_not_a_zero_measurement(self):
        for text in ("MemTotal: 16 kB", "MemTotal: 16 bytes", "MemTotal: -1 kB\nMemAvailable: 0 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB",
                     "MemTotal: 1 kB\nMemAvailable: 2 kB\nSwapTotal: 0 kB\nSwapFree: 0 kB"):
            with self.subTest(text=text), self.assertRaises(ValueError):
                ci_memory_watch.parse_memory(text)
        usage = ci_memory_watch.MemoryUsage()
        with patch.object(ci_memory_watch, "read_memory", side_effect=OSError("missing")):
            usage.sample()
        self.assertEqual(usage.samples, 0)
        self.assertEqual(usage.read_errors, 1)
        self.assertIsNone(usage.ram_total_bytes)
        self.assertIsNone(usage.sampled_peak_ram_used_bytes)

    def test_peaks_include_transient_pressure_even_after_memory_is_released(self):
        usage = ci_memory_watch.MemoryUsage()
        with patch.object(ci_memory_watch, "read_memory", side_effect=[
                memory(12 * 1024, 8 * 1024), memory(1024, 2 * 1024), memory(13 * 1024, 7 * 1024)]):
            for _ in range(3):
                usage.sample()
        self.assertEqual(usage.samples, 3)
        self.assertEqual(usage.initial_ram_used_bytes, 4 * 1024)
        self.assertEqual(usage.sampled_peak_ram_used_bytes, 15 * 1024)
        self.assertEqual(usage.sampled_peak_swap_used_bytes, 6 * 1024)

    def test_command_status_and_arguments_are_preserved_and_sampler_is_stopped(self):
        command = ["python3", "release_package.py", "--out", "package with spaces"]
        for returncode, expected in ((0, 0), (101, 101), (-9, 137)):
            output = io.StringIO()
            with self.subTest(returncode=returncode), \
                 patch.object(ci_memory_watch, "read_memory", return_value=memory(10 * 1024, 8 * 1024)), \
                 patch.object(ci_memory_watch, "Thread") as thread, \
                 patch.object(ci_memory_watch, "Event") as event, \
                 patch.object(ci_memory_watch.subprocess, "run", return_value=SimpleNamespace(returncode=returncode)) as child, \
                 patch("sys.stdout", output):
                self.assertEqual(ci_memory_watch.run(command), expected)
                child.assert_called_once_with(command, check=False)
                thread.return_value.start.assert_called_once_with()
                event.return_value.set.assert_called_once_with()
                thread.return_value.join.assert_called_once_with()
            reports = [json.loads(line.removeprefix("CI memory ")) for line in output.getvalue().splitlines()]
            self.assertEqual([report["stage"] for report in reports], ["before command", "after command"])
            self.assertEqual(reports[-1]["samples"], 2)
            self.assertIn("whole runner", reports[-1]["scope"])

    def test_a_failed_launch_still_stops_the_sampler_and_reports(self):
        output = io.StringIO()
        with patch.object(ci_memory_watch, "read_memory", return_value=memory(10 * 1024, 8 * 1024)), \
             patch.object(ci_memory_watch, "Thread") as thread, \
             patch.object(ci_memory_watch, "Event") as event, \
             patch.object(ci_memory_watch.subprocess, "run", side_effect=FileNotFoundError("command missing")), \
             patch("sys.stdout", output), self.assertRaises(FileNotFoundError):
            ci_memory_watch.run(["missing-command"])
        event.return_value.set.assert_called_once_with()
        thread.return_value.join.assert_called_once_with()
        self.assertIn('"stage": "after command"', output.getvalue())


if __name__ == "__main__":
    unittest.main()
