#!/usr/bin/env python3
"""Run the node installer in an isolated filesystem with a stand-in release and manager."""

import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest


INSTALLER = Path(__file__).resolve().parents[3] / "deploy/install/install-node.sh"
NODE = b"#!/bin/sh\nexit 0\n"
NODE_BLAKE3 = "e87b065684e1003950e1996781ed3cf0a6c88751726df8b9963694f204905b3d"


class NodeInstallerService(unittest.TestCase):
    def install(self, foreground=False):
        with tempfile.TemporaryDirectory(prefix="orch-node-installer-") as temporary:
            root = Path(temporary)
            (root / "node").write_bytes(NODE)
            (root / "manifest.json").write_text(json.dumps({
                "project": "orch", "version": "fixture", "assets": [{
                    "download_url": "/api/v1/downloads/node/x86_64", "size": len(NODE),
                    "sha256": hashlib.sha256(NODE).hexdigest(), "blake3": NODE_BLAKE3,
                }],
            }))
            tools = root / "tools"
            tools.mkdir()
            scripts = {
                "uname": "#!/bin/sh\ncase $1 in -s) echo Linux;; -m) echo x86_64;; esac\n",
                "curl": """#!/bin/sh
while [ $# -gt 0 ]; do
  case $1 in -o) out=$2; shift 2;; --proto|--proto-redir) shift 2;; *://*) url=$1; shift;; *) shift;; esac
done
case $url in */version.json) cp /fixture/manifest.json "$out";; *) cp /fixture/node "$out";; esac
""",
                "systemctl": "#!/bin/sh\nexit 0\n",
                "b3sum": f"#!/bin/sh\n[ \"$1\" = --no-names ] && cmp \"$2\" /fixture/node || exit 1\necho {NODE_BLAKE3}\n",
            }
            for name, contents in scripts.items():
                path = tools / name
                path.write_text(contents)
                path.chmod(0o755)
            runner = root / "run.sh"
            runner.write_text("""#!/bin/sh
set -eu
mkdir -p /run/systemd/system /etc/systemd/system
sh /installer --from https://orch.example --url wss://gateway.example/link --token fixture-token \\
  --config '/etc/orch/config with spaces.toml' --data-dir '/var/lib/orch 100% ${DATA} data' "$@"
if [ -f /etc/systemd/system/orch-node.service ]; then
  stat -c 'MODE=%a' /etc/systemd/system/orch-node.service
  cat /etc/systemd/system/orch-node.service
else
  echo NO_UNIT
fi
""")
            command = [
                "docker", "run", "--rm", "--network", "none", "--pull", "missing",
                "--volume", f"{root}:/fixture:ro", "--volume", f"{INSTALLER}:/installer:ro",
                "--env", "PATH=/fixture/tools:/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin",
                "--env", "HTTPS_PROXY=http://user:p%40ss$@proxy.example:3128",
                "--env", "http_proxy=http://other.example:8080",
                "--env", "NO_PROXY=.internal.example,.example&suffix",
                "busybox@sha256:a46257e06c82ad85242d0d38077d0a52d9896389ca60a1a59d3463db586e5fa4",
                "sh", "/fixture/run.sh",
            ]
            if foreground:
                command.append("--no-service")
            return subprocess.run(command, text=True, capture_output=True, check=False, timeout=60)

    def test_node_service_keeps_the_installing_shells_proxy_private(self):
        result = self.install()
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn('Environment="HTTPS_PROXY=http://user:p%%40ss$@proxy.example:3128"\n', result.stdout)
        self.assertIn('Environment="http_proxy=http://other.example:8080"\n', result.stdout)
        self.assertIn('Environment="NO_PROXY=.internal.example,.example&suffix"\n', result.stdout)
        self.assertIn("MODE=600\n", result.stdout)
        self.assertIn('ExecStart=/usr/local/bin/orch-node --config "/etc/orch/config with spaces.toml" --data-dir "/var/lib/orch 100%% $${DATA} data" run', result.stdout)

    def test_foreground_install_does_not_create_a_service(self):
        result = self.install(foreground=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("NO_UNIT", result.stdout)
        self.assertIn("No service was installed", result.stderr)


if __name__ == "__main__":
    unittest.main()
