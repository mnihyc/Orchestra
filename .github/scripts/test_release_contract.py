"""Candidate records pin every schema file and the real transport/platform declarations."""

import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from release_contract import ledger, require_source, schema, seal_schema, source_contract

HEAD = "a" * 40
TAG = "v1.2.3"
ROOT = Path(__file__).resolve().parents[2]


class SourceContractTests(unittest.TestCase):
    def candidate(self, root):
        (root / "migrations").mkdir()
        (root / "migrations/RELEASED").write_text("")
        (root / "migrations/0001_first.sql").write_text("CREATE TABLE first (id bigint);\n")
        (root / "migrations/0002_second.sql").write_text("CREATE TABLE second (id bigint);\n")
        (root / "crates/contracts/api/src").mkdir(parents=True)
        (root / "crates/contracts/api/src/common.rs").write_text(
            'pub const BROWSER_BASE_PATH: &str = "/api/v1";\n'
            'pub const KEY_BASE_PATH: &str = "/openapi/v1";\n')
        (root / "deploy").mkdir()
        (root / "deploy/nginx.conf").write_text("\n".join(
            f"location {route} {{ proxy_pass $upstream$request_uri; }}"
            for route in ["/api/", "/api/v1/ws", "/openapi/", "/openapi/v1/ws"]))
        target = root / "crates/tooling/xtask/src/release/connectors.rs"
        target.parent.mkdir(parents=True)
        target.write_text((ROOT / "crates/tooling/xtask/src/release/connectors.rs").read_text())

    def test_all_schema_files_are_sealed_and_existing_checksums_are_never_rewritten(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            with self.assertRaisesRegex(ValueError, "seal the exact") as refused:
                source_contract(root, HEAD, TAG)
            self.assertIn("python3 .github/scripts/release_contract.py seal-schema", str(refused.exception))
            self.assertIn("snapshot checkout", str(refused.exception))
            self.assertIn("review and commit migrations/RELEASED", str(refused.exception))
            seal_schema(root)
            self.assertEqual(ledger(root), schema(root))
            record = source_contract(root, HEAD, TAG)
            self.assertEqual(len(record["schema"]["files"]), 2)
            (root / "migrations/0003_later.sql").write_text("SELECT 3;\n")
            with self.assertRaisesRegex(ValueError, "seal the exact"):
                source_contract(root, HEAD, TAG)
            seal_schema(root)
            self.assertEqual(len(ledger(root)), 3)
            (root / "migrations/0001_first.sql").write_text("SELECT 'changed';\n")
            with self.assertRaisesRegex(ValueError, "recorded migration changed"):
                seal_schema(root)

    def test_cache_warming_records_unsealed_source_but_release_and_rehearsal_refuse_it(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            self.assertFalse(source_contract(root, HEAD, TAG, require_sealed=False)["schema"]["sealed"])
            with self.assertRaises(ValueError):
                source_contract(root, HEAD, TAG)

    def test_matrix_comes_from_the_native_catalogue_and_keeps_all_six_targets(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            record = source_contract(root, HEAD, TAG)
            self.assertEqual(len(record["native_matrix"]["include"]), 6)
            self.assertEqual({item["arch"] for item in record["package_matrix"]["include"]}, {"x86_64", "aarch64"})
            self.assertIn("orch-connector-windows-x86_64.exe", record["connectors"])
            self.assertIn("orch-connector-windows-aarch64.exe", record["connectors"])
            self.assertIn({"os": "windows-11-arm", "target": "aarch64-pc-windows-msvc"}, record["native_matrix"]["include"])
            self.assertEqual(len(record["nodes"]), 2)
            self.assertEqual(json.loads(json.dumps(record)), record)

    def test_unknown_native_targets_and_incomplete_declarations_fail_before_building(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            target = root / "crates/tooling/xtask/src/release/connectors.rs"
            original = target.read_text()
            target.write_text(original.replace("x86_64-pc-windows-msvc", "unassigned-native-target"))
            with self.assertRaisesRegex(ValueError, "unassigned"):
                source_contract(root, HEAD, TAG)
            target.write_text(original.replace("TARGETS: [(&str, &str, &str, &str); 6]", "TARGETS: [(&str, &str, &str, &str); 7]", 1))
            with self.assertRaisesRegex(ValueError, "catalogue expression"):
                source_contract(root, HEAD, TAG)

    def test_a_missing_key_proxy_or_changed_authentication_root_is_refused(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            common = root / "crates/contracts/api/src/common.rs"
            original = common.read_text()
            common.write_text(original.replace('"/openapi/v1"', '"/api/v1"'))
            with self.assertRaisesRegex(ValueError, "transport roots"):
                source_contract(root, HEAD, TAG)
            common.write_text(original)
            edge = root / "deploy/nginx.conf"
            edge.write_text(edge.read_text().replace("location /openapi/", "location /old-keys/"))
            with self.assertRaisesRegex(ValueError, "unchanged /openapi/"):
                source_contract(root, HEAD, TAG)

    def test_a_different_or_dirty_checkout_cannot_claim_the_pinned_sha(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            with patch("release_contract.command", return_value="b" * 40), self.assertRaisesRegex(ValueError, "pinned"):
                require_source(root, HEAD, TAG)
            with patch("release_contract.command", side_effect=[HEAD, " M source.rs"]), self.assertRaisesRegex(ValueError, "source changed"):
                require_source(root, HEAD, TAG)


if __name__ == "__main__":
    unittest.main()
