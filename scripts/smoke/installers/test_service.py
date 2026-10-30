#!/usr/bin/env python3
"""Exercise the Unix installer with a verified stand-in connector and service managers."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest
import xml.etree.ElementTree as ET


INSTALLER = Path(__file__).resolve().parents[3] / "deploy/install/install.sh"
CONNECTOR = b"""#!/bin/sh
printf 'call' >> "$TEST_CALLS"
for word in "$@"; do printf '\\t%s' "$word" >> "$TEST_CALLS"; done
printf '\\n' >> "$TEST_CALLS"
"""
CONNECTOR_BLAKE3 = "c4af63428c874d2eb171fd12cf2a1bda590d510a8aecad8121f7d21cbc3876bb"


class InstallerService(unittest.TestCase):
    def run_installer(self, system, *, named=False, service=True, manager=True):
        with tempfile.TemporaryDirectory(prefix="orch-installer-") as temporary:
            root = Path(temporary)
            home = root / "home with spaces"
            binaries = root / "tools"
            home.mkdir()
            binaries.mkdir()
            calls = root / "calls"
            connector = root / "connector"
            connector.write_bytes(CONNECTOR)
            manifest = root / "manifest.json"
            manifest.write_text(json.dumps({
                "project": "orch", "version": "fixture", "assets": [{
                    "download_url": f"/api/v1/downloads/connector/{system.lower() if system == 'Linux' else 'macos'}/x86_64",
                    "size": len(CONNECTOR), "sha256": hashlib.sha256(CONNECTOR).hexdigest(),
                    "blake3": CONNECTOR_BLAKE3,
                }],
            }))
            scripts = {
                "uname": f"#!/bin/sh\ncase $1 in -s) echo {system};; -m) echo x86_64;; esac\n",
                "curl": """#!/bin/sh
while [ $# -gt 0 ]; do
  case $1 in -o) out=$2; shift 2;; --proto|--proto-redir) shift 2;; *://*) url=$1; shift;; *) shift;; esac
done
case $url in */version.json) cp "$TEST_MANIFEST" "$out";; *) cp "$TEST_CONNECTOR" "$out";; esac
""",
                "systemctl": "#!/bin/sh\n[ \"${TEST_MANAGER}\" = yes ]\n",
                "loginctl": "#!/bin/sh\necho yes\n",
                "launchctl": "#!/bin/sh\nexit 0\n",
                "b3sum": f"#!/bin/sh\n[ \"$1\" = --no-names ] && cmp \"$2\" \"$TEST_CONNECTOR\" || exit 1\necho {CONNECTOR_BLAKE3}\n",
            }
            for name, contents in scripts.items():
                path = binaries / name
                path.write_text(contents)
                path.chmod(0o755)
            env = {
                "PATH": f"{binaries}:{os.defpath}", "HOME": str(home), "LC_ALL": "C",
                "TEST_CALLS": str(calls), "TEST_MANIFEST": str(manifest),
                "TEST_CONNECTOR": str(connector), "TEST_MANAGER": "yes" if manager else "no",
                "HTTPS_PROXY": "http://user:p%40ss$@proxy.example:3128", "http_proxy": "http://other.example:8080",
                "NO_PROXY": ".internal.example,.example&suffix",
            }
            args = ["sh", str(INSTALLER), "--url", "https://orch.example", "--token", "fixture-token"]
            data = home / "named data"
            if named:
                args += ["--data-dir", str(data)]
            if not service:
                args.append("--no-service")
            result = subprocess.run(args, env=env, text=True, capture_output=True, check=False)
            lines = calls.read_text().splitlines() if calls.exists() else []
            suffix = hashlib.sha256(str(data).encode()).hexdigest()[:8] if named else ""
            if system == "Linux":
                unit = home / ".config/systemd/user" / f"orch-connector{'-' + suffix if suffix else ''}.service"
            else:
                unit = home / "Library/LaunchAgents" / f"dev.orch.connector{'.' + suffix if suffix else ''}.plist"
            contents = unit.read_text() if unit.exists() else None
            mode = unit.stat().st_mode & 0o777 if unit.exists() else None
            return result, lines, contents, mode

    def test_default_and_named_services_receive_the_installing_shells_proxy(self):
        for system in ("Linux", "Darwin"):
            for named in (False, True):
                with self.subTest(system=system, named=named):
                    result, lines, contents, mode = self.run_installer(system, named=named)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    self.assertIsNotNone(contents)
                    self.assertEqual(mode, 0o600, "proxy credentials must not be exposed to other accounts")
                    if system == "Linux":
                        self.assertIn('Environment="HTTPS_PROXY=http://user:p%%40ss$@proxy.example:3128"\n', contents)
                        self.assertIn('Environment="http_proxy=http://other.example:8080"\n', contents)
                        self.assertIn('Environment="NO_PROXY=.internal.example,.example&suffix"\n', contents)
                    else:
                        root = ET.fromstring(contents)
                        dictionaries = root.findall(".//dict")
                        environment = next(item for item in dictionaries if any(child.text == "HTTPS_PROXY" for child in item))
                        values = {environment[i].text: environment[i + 1].text for i in range(0, len(environment), 2)}
                        self.assertEqual(values["HTTPS_PROXY"], "http://user:p%40ss$@proxy.example:3128")
                        self.assertEqual(values["http_proxy"], "http://other.example:8080")
                        self.assertEqual(values["NO_PROXY"], ".internal.example,.example&suffix")
                    self.assertTrue(any("__record-installation\t--service" in line for line in lines), lines)

    def test_missing_user_manager_and_foreground_leave_starting_to_the_person(self):
        for manager, service in ((False, True), (True, False)):
            with self.subTest(manager=manager, service=service):
                result, _, contents, _ = self.run_installer("Linux", manager=manager, service=service)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIsNone(contents)
                self.assertIn("Start the connector", result.stderr)


if __name__ == "__main__":
    unittest.main()
