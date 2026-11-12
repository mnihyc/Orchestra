"""Static default/packaging feature guards; these do not execute Cargo or prove a binary."""
from pathlib import Path
import re
import tomllib
import unittest

ROOT = Path(__file__).resolve().parents[3]


def manifest(name):
    return tomllib.loads((ROOT / name / 'Cargo.toml').read_text())


class ObserverFeatures(unittest.TestCase):
    def test_added_queue_observers_keep_existing_channels_and_feature_scoped_size_code(self):
        cases = [
            ('crates/platform/link/src/controller.rs', 'Notice', 'mpsc::UnboundedReceiver<DownFrame>'),
            ('crates/platform/link/src/machine.rs', 'Input', 'mpsc::UnboundedReceiver<Input>'),
            ('crates/platform/link/src/tunnels/terminal.rs', 'ApiOut', 'mpsc::UnboundedSender<ApiOut>'),
        ]
        for path, payload, channel in cases:
            source = (ROOT / path).read_text()
            self.assertIn(channel, source)
            self.assertRegex(source, r'#\[cfg\(feature = "test-observers"\)\]\s*impl ' + payload + r'\b')
        for path, fields in [
            ('crates/platform/link/src/controller.rs', ['observed_down', 'observed_notices']),
            ('crates/platform/link/src/machine.rs', ['observed_inputs']),
            ('crates/platform/link/src/commands.rs', ['observed']),
            ('crates/platform/link/src/tunnels/terminal.rs', ['observed']),
            ('crates/control/api/base/src/backplane.rs', ['terminal_observed']),
        ]:
            source = (ROOT / path).read_text()
            for field in fields:
                self.assertRegex(source, r'#\[cfg\(feature = "test-observers"\)\]\s*(?:pub )?' + field + r':')

    def test_observers_are_explicit_and_default_off_without_a_normal_dependency_cycle(self):
        link = manifest('crates/platform/link')
        api = manifest('crates/control/api')
        gateway = manifest('crates/control/gateway')
        for package in (link, api, gateway):
            self.assertEqual(package['features']['default'], [])
            self.assertIn('test-observers', package['features'])
        self.assertEqual(link['features']['test-observers'], ['dep:sha2'])
        self.assertTrue(link['dependencies']['sha2']['optional'])
        workspace = tomllib.loads((ROOT / 'Cargo.toml').read_text())['workspace']['dependencies']
        backend_names = sorted(name for name in api['dependencies']
            if isinstance(workspace.get(name), dict)
            and workspace[name].get('path', '').startswith('crates/control/api/'))
        self.assertIn('orch-api-base', backend_names)
        self.assertEqual(sorted(api['features']['test-observers']),
            sorted(['orch-link/test-observers', *(name + '/test-observers' for name in backend_names)]))
        for name in backend_names:
            backend = manifest(workspace[name]['path'])
            self.assertEqual(backend['features']['default'], [])
            expected = ['orch-link/test-observers']
            if name != 'orch-api-base':
                expected.append('orch-api-base/test-observers')
            self.assertEqual(backend['features']['test-observers'], expected)
            self.assertNotIn('test-observers', backend['dependencies']['orch-link'].get('features', []))
        self.assertEqual(gateway['features']['test-observers'],
            ['orch-link/test-observers', 'orch-api-core/test-observers'])
        self.assertNotIn('orch-api-core', gateway['dependencies'])
        self.assertIn('orch-api-core', gateway['dev-dependencies'])
        self.assertNotIn('orch-gateway-core', api['dependencies'])
        self.assertNotIn('orch-gateway-core', api['dev-dependencies'])
        for package in (api, gateway):
            self.assertNotIn('test-observers', package['dependencies']['orch-link'].get('features', []))

    def test_qa_process_features_forward_only_when_explicitly_selected(self):
        for app, dependency in [('api', 'orch-api-core'), ('gateway', 'orch-gateway-core'), ('connector', 'orch-connector-core')]:
            package = manifest('apps/' + app)
            self.assertEqual(package['features']['default'], [])
            self.assertEqual(package['features']['test-observers'], [dependency + '/test-observers'])
            self.assertNotIn('test-observers', package['dependencies'][dependency].get('features', []))
            source = (ROOT / 'apps' / app / 'src/main.rs').read_text()
            self.assertRegex(source, r'#\[cfg\(feature = "test-observers"\)\]\s*let observer')
        connector = manifest('crates/runtime/connector')
        self.assertEqual(connector['features']['default'], [])
        self.assertEqual(connector['features']['test-observers'], ['orch-link/test-observers'])
        source = (ROOT / 'crates/platform/link/src/test_observers/process.rs').read_text()
        self.assertIn('"complete_wss_release_gate":false', source.replace(' ', ''))
        self.assertIn('declared_source_commit', source)
        self.assertIn('executable_blake3', source)

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
