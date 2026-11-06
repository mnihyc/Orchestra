"""Static default/packaging feature guards; these do not execute Cargo or prove a binary."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[3]


def manifest(name):
    return tomllib.loads((ROOT / name / 'Cargo.toml').read_text())


class ObserverFeatures(unittest.TestCase):
    def test_observers_are_explicit_and_default_off_without_a_normal_dependency_cycle(self):
        link = manifest('crates/platform/link')
        api = manifest('crates/control/api')
        gateway = manifest('crates/control/gateway')
        for package in (link, api, gateway):
            self.assertEqual(package['features']['default'], [])
            self.assertIn('test-observers', package['features'])
        self.assertEqual(link['features']['test-observers'], [])
        self.assertEqual(api['features']['test-observers'], ['orch-link/test-observers'])
        self.assertEqual(gateway['features']['test-observers'],
            ['orch-link/test-observers', 'orch-api-core/test-observers'])
        self.assertNotIn('orch-api-core', gateway['dependencies'])
        self.assertIn('orch-api-core', gateway['dev-dependencies'])
        self.assertNotIn('orch-gateway-core', api['dependencies'])
        self.assertNotIn('orch-gateway-core', api['dev-dependencies'])
        for package in (api, gateway):
            self.assertNotIn('test-observers', package['dependencies']['orch-link'].get('features', []))

    def test_current_release_builder_does_not_select_test_features(self):
        source = (ROOT / 'crates/tooling/xtask/src/release/package.rs').read_text()
        for name in ('server_arguments', 'static_arguments'):
            match = re.search(r'let mut ' + name + r' = vec!\[(.*?)builder\.cargo\(&' + name,
                source, re.S)
            self.assertIsNotNone(match, 'review the changed release build vector before trusting this guard')
            for forbidden in ('--all-features', '--features', 'test-observers'):
                self.assertNotIn(forbidden, match[1])
        ci = (ROOT / '.github/workflows/ci.yml').read_text()
        native_commands = re.findall(r'run:\s*(cargo build --release[^\n]*)', ci)
        self.assertTrue(native_commands, 'review a changed native release builder before trusting this guard')
        for command in native_commands:
            for forbidden in ('--all-features', '--features', 'test-observers'):
                self.assertNotIn(forbidden, command)
        # The newer packaging layout builds these in package.rs; older candidates also
        # have a dedicated connector builder. Inspect it when part of that source tree.
        legacy_native = ROOT / 'crates/tooling/xtask/src/release/connectors.rs'
        if legacy_native.exists():
            native = legacy_native.read_text()
            self.assertNotIn('--all-features', native)
            self.assertNotIn('test-observers', native)
        exports = (ROOT / 'crates/platform/link/src/lib.rs').read_text()
        self.assertRegex(exports, r'#\[cfg\(feature = "test-observers"\)\]\s*pub mod test_observers;')


if __name__ == '__main__':
    unittest.main()
