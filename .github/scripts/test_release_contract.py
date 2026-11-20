"""Candidate records pin every schema file and the real transport/platform declarations."""

import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

from release_contract import (
    ledger, read_file_sets, record_file_sets, require_source, schema, seal_schema, source_contract, file_sets_contract,
)

HEAD = "a" * 40
TAG = "v1.2.3"
ROOT = Path(__file__).resolve().parents[2]
EDGE_FILES = ["deploy/frontend.sh", "deploy/compose.frontend-https.yml",
              "crates/control/api/base/src/certificates/transport.rs",
              "crates/control/api/base/src/registry.rs", "apps/api/src/composition.rs"]
FILE_SETS = "format 1\ngeneration 1\nminimum 1\nset main compose.yml nginx.conf\n"


def sha(data):
    return hashlib.sha256(data).hexdigest()


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
            ["upstream orch_api { zone orch_api 64k; server api:8443 resolve; keepalive 32; }"] + [
                f"location {route} {{ proxy_pass https://orch_api; }}"
                for route in ["/api/", "/api/v1/ws", "/openapi/", "/openapi/v1/ws"]]))
        edge = root / "deploy/nginx.conf"
        edge.write_text(edge.read_text() + '\nmap "$ssl_early_data:$http_early_data" $orch_early_data { default $http_early_data; ~^1: 1; }\n'
                        'map $http_host $orch_https_port { default 443; ~:(?<orch_port>[0-9]+)$ $orch_port; }\n'
                        'proxy_set_header Early-Data $orch_early_data;\n')
        for relative in EDGE_FILES:
            target = root / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text((ROOT / relative).read_text())
        (root / "deploy/compose.yml").write_text("name: orch\n")
        (root / "deploy/update.sh").write_text("#!/usr/bin/env bash\n")
        (root / "deploy/FILE_SETS").write_text(FILE_SETS)
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

    def test_an_edge_that_rewrites_a_route_or_keeps_a_stale_address_is_refused(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            edge = root / "deploy/nginx.conf"
            original = edge.read_text()
            # A URI part after the group's name replaces the matched prefix: the route would no longer be unchanged.
            edge.write_text(original.replace(
                "location /api/ { proxy_pass https://orch_api; }", "location /api/ { proxy_pass https://orch_api/; }"))
            with self.assertRaisesRegex(ValueError, "unchanged /api/ "):
                source_contract(root, HEAD, TAG)
            # Without `resolve` the name is looked up once: an API made again by an update is not found.
            edge.write_text(original.replace("server api:8443 resolve;", "server api:8443;"))
            with self.assertRaisesRegex(ValueError, "look the API's address up again"):
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

    def test_a_candidate_keeps_resumption_early_write_refusal_and_reachable_http3(self):
        fragments = [
            ("deploy/frontend.sh", "include /etc/orch/frontend-tls/transport.include;"),
            ("deploy/frontend.sh", "nginx -t && nginx -s reload"),
            ("deploy/compose.frontend-https.yml", ":443/udp"),
            ("deploy/nginx.conf", "proxy_set_header Early-Data $orch_early_data;"),
            ("deploy/nginx.conf", '$ssl_early_data:$http_early_data'),
            ("deploy/nginx.conf", 'map $http_host $orch_https_port'),
            ("crates/control/api/base/src/certificates/transport.rs", "ssl_session_cache"),
            ("crates/control/api/base/src/certificates/transport.rs", "ssl_session_timeout"),
            ("crates/control/api/base/src/certificates/transport.rs", "ssl_session_tickets"),
            ("crates/control/api/base/src/certificates/transport.rs", "ssl_early_data"),
            ("crates/control/api/base/src/certificates/transport.rs", "listen 443 quic reuseport;"),
            ("crates/control/api/base/src/certificates/transport.rs", "Alt-Svc"),
            ("crates/control/api/base/src/registry.rs", 'get_all("early-data")'),
            ("crates/control/api/base/src/registry.rs", 'route.path.starts_with("/auth/oidc/")'),
            ("apps/api/src/composition.rs", "keep_frontend_transport"),
        ]
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.candidate(root)
            seal_schema(root)
            source_contract(root, HEAD, TAG)
            for relative, fragment in fragments:
                with self.subTest(file=relative, behavior=fragment):
                    path = root / relative
                    original = path.read_text()
                    self.assertIn(fragment, original)
                    path.write_text(original.replace(fragment, "removed_transport_behavior"))
                    with self.assertRaisesRegex(ValueError, "HTTPS"):
                        source_contract(root, HEAD, TAG)
                    path.write_text(original)


class DeploymentFilesTests(unittest.TestCase):
    def tree(self, root, table=FILE_SETS):
        (root / "deploy").mkdir(parents=True, exist_ok=True)
        (root / "deploy/nginx.conf").write_text("events {}\n")
        (root / "deploy/compose.yml").write_text("name: orch\n")
        (root / "deploy/update.sh").write_text("#!/usr/bin/env bash\n")
        (root / "deploy/config").mkdir(exist_ok=True)
        (root / "deploy/config/orch.toml").write_text("version = 1\n")
        (root / "deploy/FILE_SETS").write_text(table)

    def rows(self, tag, generation, nginx="events {}\n", compose="name: orch\n", updater="#!/usr/bin/env bash\n"):
        return "".join(f"file {tag} {generation} {sha(data.encode())} {path}\n" for path, data in
                       [("compose.yml", compose), ("nginx.conf", nginx), ("update.sh", updater)])

    def test_a_table_is_read_as_the_updater_reads_it_and_what_it_would_refuse_is_refused(self):
        table = read_file_sets(FILE_SETS + self.rows("v0.1.2", 1))
        self.assertEqual((table["generation"], table["minimum"], table["main"]), (1, 1, ["compose.yml", "nginx.conf"]))
        self.assertEqual(len(table["rows"]), 3)
        for bad, said in [
            ("generation 1\nminimum 1\nset main a\n", "format 1"),
            (FILE_SETS.replace("minimum 1", "minimum 2"), "minimum generation above"),
            (FILE_SETS + "file v0.1.2 1 nothex nginx.conf\n", "cannot be read"),
            (FILE_SETS + "surprise 1\n", "cannot be read"),
        ]:
            with self.assertRaisesRegex(ValueError, said):
                read_file_sets(bad)

    def test_a_candidate_has_a_table_whose_main_set_exists_and_an_updater(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.tree(root)
            self.assertEqual(file_sets_contract(root, TAG)["generation"], 1)
            (root / "deploy/compose.yml").unlink()
            with self.assertRaisesRegex(ValueError, "compose.yml in the main set"):
                file_sets_contract(root, TAG)
            (root / "deploy/compose.yml").write_text("name: orch\n")
            (root / "deploy/FILE_SETS").unlink()
            with self.assertRaisesRegex(ValueError, "no deploy/FILE_SETS"):
                file_sets_contract(root, TAG)

    def test_a_release_that_changes_the_main_set_raises_the_generation_and_one_that_does_not_keeps_it(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.tree(root, FILE_SETS + self.rows("v0.1.2", 1))
            self.assertEqual(file_sets_contract(root, TAG)["generation"], 1)
            (root / "deploy/nginx.conf").write_text("events {}\n# a route\n")
            with self.assertRaisesRegex(ValueError, "generation must be raised"):
                file_sets_contract(root, TAG)
            (root / "deploy/FILE_SETS").write_text(FILE_SETS.replace("generation 1", "generation 2") + self.rows("v0.1.2", 1))
            self.assertEqual(file_sets_contract(root, TAG)["generation"], 2)
            (root / "deploy/nginx.conf").write_text("events {}\n")
            with self.assertRaisesRegex(ValueError, "is generation 1, and deploy/FILE_SETS says 2"):
                file_sets_contract(root, TAG)

    def test_a_release_does_not_carry_its_own_rows_before_it_is_made(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.tree(root, FILE_SETS + self.rows(TAG, 1))
            with self.assertRaisesRegex(ValueError, "already has rows"):
                file_sets_contract(root, TAG)

    def test_the_notes_give_the_updaters_digest_from_the_release_after_v0_1_3_and_it_has_to_be_right(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.tree(root)
            digest = sha(b"#!/usr/bin/env bash\n")
            notes = root / ".github/release-notes"
            notes.mkdir(parents=True)
            self.assertEqual(file_sets_contract(root, TAG)["update_sh_sha256"], digest)
            (notes / f"{TAG}.md").write_text("# Orch\n")
            with self.assertRaisesRegex(ValueError, "must give the SHA-256"):
                file_sets_contract(root, TAG)
            (notes / f"{TAG}.md").write_text("# Orch\n\nupdate.sh SHA-256: " + "0" * 64 + "\n")
            with self.assertRaisesRegex(ValueError, "not deploy/update.sh's"):
                file_sets_contract(root, TAG)
            (notes / f"{TAG}.md").write_text(f"# Orch\n\nupdate.sh SHA-256: {digest}\n")
            self.assertEqual(file_sets_contract(root, TAG)["generation"], 1)
            (notes / "v0.1.3.md").write_text("# Orch v0.1.3\n")
            self.assertEqual(file_sets_contract(root, "v0.1.3")["generation"], 1)

    def test_a_published_release_adds_its_files_to_the_table_from_git(self):
        with tempfile.TemporaryDirectory() as name:
            root = Path(name)
            self.tree(root)
            for args in (["init", "-q"], ["add", "-A"], ["-c", "user.name=t", "-c", "user.email=t@example.org", "commit", "-qm", "files"],
                         ["tag", "v0.1.4"]):
                subprocess.run(["git", "-C", str(root), *args], check=True)
            packed = (FILE_SETS + self.rows("v0.1.4", 1)).encode()
            (root / "deploy/FILE_SETS").write_text(FILE_SETS.replace("generation 1", "generation 2"))
            self.assertEqual(record_file_sets(root, "v0.1.4", "v0.1.4"), 4)
            table = read_file_sets((root / "deploy/FILE_SETS").read_text())
            self.assertEqual({row[3] for row in table["rows"]}, {"compose.yml", "nginx.conf", "update.sh", "FILE_SETS"})
            self.assertIn(("v0.1.4", 1, sha(packed), "FILE_SETS"), table["rows"])
            self.assertNotIn("orch.toml", (root / "deploy/FILE_SETS").read_text())
            with self.assertRaisesRegex(ValueError, "already"):
                record_file_sets(root, "v0.1.4", "v0.1.4")


if __name__ == "__main__":
    unittest.main()
