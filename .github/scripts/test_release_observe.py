"""Platform evidence must describe both real observations of the exact candidate."""

import copy
import json
from pathlib import Path
import subprocess
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from release_observe import CASE, ROOT, candidate, observe, validate_record

COMMIT = "a" * 40
TARGET = "aarch64-pc-windows-msvc"


def evidence():
    platform = {"system": "Windows", "version": "native version", "machine": "ARM64"}
    return {"format": 1, "target": TARGET, "commit": COMMIT, "status": "passed",
            "platform": platform, "native_os": "windows", "native_arch": "aarch64",
            "connector": {"identity": "connector 0.1.2 (build 1) [v0.1.2]", "sha256": "b" * 64},
            "observations": [
                {"name": "connected_program_attachment", "status": "passed", "commit": COMMIT,
                 "platform": platform, "elapsed_ms": 100, "evidence": [
                     {"event": "enrolled"}, {"event": "scripted_program_described"},
                     {"event": "scripted_program_started", "method": "agent.start"}]},
                {"name": "connector_credential_lifecycle", "status": "passed", "commit": COMMIT,
                 "platform": platform, "elapsed_ms": 200, "evidence": [
                     {"event": "session_ready", "fingerprint": "initial"},
                     {"event": "session_rejected", "code": "revoked", "fingerprint": "initial"},
                     {"event": "renewal_issued", "previous_fingerprint": "eligible", "fingerprint": "renewed", "new_public_key": True},
                     {"event": "session_ready", "fingerprint": "renewed"}]}]}


class ObservationTests(unittest.TestCase):
    def test_both_native_observations_and_the_candidate_provenance_are_required(self):
        record = evidence()
        self.assertIs(validate_record(record, TARGET, COMMIT), record)
        for mutate in [lambda r: r.update(commit="c" * 40), lambda r: r.update(target="x86_64-pc-windows-msvc"),
                       lambda r: r.update(native_arch="x86_64"), lambda r: r.update(native_os="linux"),
                       lambda r: r.update(status="failed", reason="native rejection missing"),
                       lambda r: r["connector"].update(sha256="not measured"),
                       lambda r: r["platform"].pop("version"), lambda r: r["observations"].pop(),
                       lambda r: r["observations"][1].update(name="connected_program_attachment"),
                       lambda r: r["observations"][0].update(commit="d" * 40),
                       lambda r: r["observations"][0].update(platform={"system": "elsewhere"})]:
            changed = copy.deepcopy(record)
            mutate(changed)
            with self.subTest(changed=changed), self.assertRaises(ValueError):
                validate_record(changed, TARGET, COMMIT)

    def test_missing_fabricated_or_failed_measurements_are_not_observations(self):
        for value in [None, -1, float("inf"), float("nan"), True, "10 ms"]:
            record = evidence()
            record["observations"][0]["elapsed_ms"] = value
            with self.subTest(value=value), self.assertRaises(ValueError):
                validate_record(record, TARGET, COMMIT)
        for change in [{"status": "failed"}, {"evidence": []}]:
            record = evidence()
            record["observations"][0].update(change)
            with self.assertRaises(ValueError):
                validate_record(record, TARGET, COMMIT)

    def test_refusal_rotation_and_new_native_acceptance_are_independent_evidence(self):
        for mutate in [lambda r: r["observations"][0]["evidence"].pop(),
                       lambda r: r["observations"][1]["evidence"][1].update(code="unreachable"),
                       lambda r: r["observations"][1]["evidence"][2].update(new_public_key=False),
                       lambda r: r["observations"][1]["evidence"][2].update(previous_fingerprint="renewed"),
                       lambda r: r["observations"][1]["evidence"].pop()]:
            record = evidence()
            mutate(record)
            with self.assertRaises(ValueError):
                validate_record(record, TARGET, COMMIT)

    def test_the_executed_file_is_the_hashed_packaged_candidate(self):
        import hashlib
        with tempfile.TemporaryDirectory() as name:
            directory = Path(name)
            path = directory / "orch-connector-windows-aarch64.exe"
            path.write_bytes(b"MZ native test candidate")
            manifest = {"commit": COMMIT, "tag": "v0.1.2", "version": "0.1.2", "assets": [
                {"name": path.name, "size": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}]}
            (directory / "version.json").write_text(json.dumps(manifest))
            version = subprocess.CompletedProcess([], 0, stdout="connector 0.1.2 (build 1) [v0.1.2]\n")
            with patch("release_observe.subprocess.run", return_value=version) as run:
                binary, facts = candidate(ROOT, directory, TARGET, COMMIT)
            self.assertEqual(binary, path.resolve())
            self.assertEqual(run.call_args.args[0], [str(path.resolve()), "version"])
            self.assertEqual(facts["sha256"], manifest["assets"][0]["sha256"])
            path.write_bytes(b"MZ changed test candidate")
            with self.assertRaisesRegex(ValueError, "differs"):
                candidate(ROOT, directory, TARGET, COMMIT)
            with self.assertRaisesRegex(ValueError, "another candidate"):
                candidate(ROOT, directory, TARGET, "c" * 40)

    def test_an_early_native_failure_still_writes_the_two_named_failed_records(self):
        with tempfile.TemporaryDirectory() as name:
            out = Path(name)
            args = SimpleNamespace(target=TARGET, commit=COMMIT, native=out, out=out, timeout=10)
            head = subprocess.CompletedProcess([], 0, stdout=COMMIT + "\n")
            with patch("release_observe.subprocess.run", return_value=head), \
                 patch("release_observe.candidate", side_effect=ValueError("native manifest mismatch")), \
                 self.assertRaisesRegex(ValueError, "native manifest mismatch"):
                observe(args)
            record = json.loads((out / f"{TARGET}.json").read_text())
            self.assertEqual(record["status"], "failed")
            self.assertEqual({row["name"] for row in record["observations"]},
                             {"connected_program_attachment", "connector_credential_lifecycle"})
            self.assertTrue(all(row["reason"] == "native manifest mismatch" for row in record["observations"]))
            self.assertIn("finished_at", record)

    def test_a_skipped_or_absent_native_case_cannot_pass_the_release_step(self):
        with tempfile.TemporaryDirectory() as name:
            out = Path(name)
            args = SimpleNamespace(target=TARGET, commit=COMMIT, native=out, out=out, timeout=10)
            head = subprocess.CompletedProcess([], 0, stdout=COMMIT + "\n")
            skipped = subprocess.CompletedProcess([], 0)
            with patch("release_observe.candidate", return_value=(out / "candidate.exe", evidence()["connector"])), \
                 patch("release_observe.subprocess.run", side_effect=[head, skipped]) as called, \
                 self.assertRaisesRegex(ValueError, "produced no result"):
                observe(args)
            command = called.call_args_list[1].args[0]
            self.assertIn(CASE, command)
            self.assertIn("--ignored", command)
            self.assertIn("--exact", command)
            self.assertEqual(json.loads((out / f"{TARGET}.json").read_text())["status"], "failed")


if __name__ == "__main__":
    unittest.main()
