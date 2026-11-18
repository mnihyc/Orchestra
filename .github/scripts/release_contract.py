#!/usr/bin/env python3
"""Record the exact source schema, transport roots and native release catalogue."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re

from release_gate import command, version


# Native runner bindings for the targets supported by the packaging catalogue.
# Unknown additions require a reviewed runner instead of silently losing a platform.
RUNNERS = {
    "x86_64-unknown-linux-musl": "ubuntu-24.04",
    "aarch64-unknown-linux-musl": "ubuntu-24.04-arm",
    "x86_64-apple-darwin": "macos-15-intel",
    "aarch64-apple-darwin": "macos-15",
    "x86_64-pc-windows-msvc": "windows-2022",
    "aarch64-pc-windows-msvc": "windows-11-arm",
}
SQL_NAME = re.compile(r"[0-9]+_[A-Za-z0-9_]+\.sql\Z")


def schema(root):
    directory = root / "migrations"
    if directory.is_symlink():
        raise ValueError("the schema directory must not be a link")
    files = {}
    for path in sorted(directory.rglob("*.sql")):
        if path.parent != directory or not SQL_NAME.fullmatch(path.name) or path.is_symlink() or not path.is_file():
            raise ValueError("the schema must contain plain numbered SQL files")
        files[path.name] = hashlib.sha384(path.read_bytes()).hexdigest()
    if not files:
        raise ValueError("the candidate has no schema")
    return files


def ledger(root):
    if (root / "migrations/RELEASED").is_symlink():
        raise ValueError("the schema ledger must not be a link")
    rows = {}
    for line in (root / "migrations/RELEASED").read_text().splitlines():
        if not line.strip():
            continue
        fields = line.split()
        if len(fields) != 2 or not re.fullmatch(r"[0-9a-fA-F]{96}", fields[0]) or not SQL_NAME.fullmatch(fields[1]):
            raise ValueError("RELEASED must list SHA-384 checksums and plain SQL filenames")
        digest, name = fields
        if name in rows:
            raise ValueError("RELEASED lists a migration twice")
        rows[name] = digest.lower()
    return rows


def ledger_text(files):
    return "".join(f"{digest}  {name}\n" for name, digest in sorted(files.items()))


def seal_schema(root):
    """Seal before the snapshot commit; never rewrite a previously recorded checksum."""
    files = schema(root)
    if any(files.get(name) != digest for name, digest in ledger(root).items()):
        raise ValueError("a recorded migration changed or disappeared; restore its released bytes")
    target = root / "migrations/RELEASED"
    staging = target.with_name("RELEASED.new")
    with staging.open("x", encoding="utf-8") as output:
        output.write(ledger_text(files))
    staging.replace(target)
    return files


def catalogue(root, name):
    text = (root / "crates/tooling/xtask/src/release/connectors.rs").read_text()
    declaration = re.search(rf"pub const {name}:.*?;\s*([0-9]+)\s*\]\s*=\s*\[(.*?)^\];", text, re.M | re.S)
    if not declaration:
        raise ValueError(f"{name} is not a literal native target catalogue")
    item = re.compile(r'\(\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,\s*"([^"]+)"\s*,?\s*\)')
    rows = item.findall(declaration[2])
    if len(rows) != int(declaration[1]) or item.sub("", declaration[2]).strip(" \t\r\n,"):
        raise ValueError(f"{name} contains an unsupported catalogue expression")
    if len({row[0] for row in rows}) != len(rows) or any(row[0] not in RUNNERS for row in rows):
        raise ValueError(f"{name} contains duplicate or unassigned native targets")
    return rows


def https_edge_contract(root):
    """Keep the shipped HTTPS path configurable, resumable and safe to replay only for reads."""
    required = {
        "deploy/frontend.sh": ["include /etc/orch/frontend-tls/transport.include;", "nginx -t && nginx -s reload"],
        "deploy/compose.frontend-https.yml": [":443/udp"],
        "deploy/nginx.conf": ['$ssl_early_data:$http_early_data', "proxy_set_header Early-Data $orch_early_data;",
                              "map $http_host $orch_https_port"],
        "crates/control/api/base/src/certificates/transport.rs": ["ssl_session_cache", "ssl_session_timeout",
            "ssl_session_tickets", "ssl_early_data", "http2", "listen 443 quic reuseport;", "Alt-Svc"],
        "crates/control/api/base/src/registry.rs": ['get_all("early-data")', "!parts.method.is_safe()",
            'route.path.starts_with("/auth/oidc/")', "ErrorCode::RequestTooEarly"],
        "apps/api/src/composition.rs": ["publish_frontend_transport", "keep_frontend_transport"],
    }
    for relative, fragments in required.items():
        path = root / relative
        if not path.is_file():
            raise ValueError(f"the HTTPS edge is missing {relative}")
        text = path.read_text()
        for fragment in fragments:
            if fragment not in text:
                raise ValueError(f"the HTTPS edge lost {fragment!r} in {relative}")
    registry = (root / "crates/control/api/base/src/registry.rs").read_text()
    opened = registry[registry.index("async fn open("):]
    if opened.index('get_all("early-data")') > opened.index("authentication::on_surface"):
        raise ValueError("the HTTPS edge must refuse early writes before authentication can have an effect")


def source_contract(root, commit, tag, *, require_sealed=True):
    version(tag)
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("the source commit must be a full SHA")
    files = schema(root)
    sealed = ledger(root) == files
    if require_sealed and not sealed:
        raise ValueError(
            "seal the exact candidate schema in RELEASED before the snapshot commit: run "
            "`python3 .github/scripts/release_contract.py seal-schema` "
            "in that snapshot checkout, then review and commit migrations/RELEASED"
        )
    common = (root / "crates/contracts/api/src/common.rs").read_text()
    surfaces = {}
    for surface, constant in [("browser", "BROWSER_BASE_PATH"), ("keys", "KEY_BASE_PATH")]:
        found = re.search(rf'pub const {constant}: &str = "([^"]+)";', common)
        if not found:
            raise ValueError(f"missing canonical transport root {constant}")
        surfaces[surface] = found[1]
    if surfaces != {"browser": "/api/v1", "keys": "/openapi/v1"}:
        raise ValueError("the candidate changed the reviewed browser/key transport roots")
    nginx = (root / "deploy/nginx.conf").read_text()
    # The API's group keeps its connections and looks the service's name up again as its address changes, so an API
    # made again by an update is found without the edge being restarted.
    groups = re.findall(r"upstream\s+orch_api\s*\{([^{}]*)\}", nginx)
    if len(groups) != 1 or not re.search(r"\bserver\s+api:8443\s+resolve;", groups[0]):
        raise ValueError("the stock edge must look the API's address up again (upstream orch_api with `resolve`)")
    for route in ["/api/", "/api/v1/ws", "/openapi/", "/openapi/v1/ws"]:
        blocks = re.findall(rf"location\s+{re.escape(route)}\s*\{{([^{{}}]*)\}}", nginx)
        # A `proxy_pass` without a URI part hands the request's own URI on as it was received.
        if len(blocks) != 1 or not re.search(r"\bproxy_pass\s+https://orch_api;", blocks[0]):
            raise ValueError(f"the stock edge must forward the unchanged {route} route")
    https_edge_contract(root)
    native = catalogue(root, "TARGETS")
    nodes = catalogue(root, "NODE_TARGETS")
    if any(row[1] != "linux" or row[0] not in {target[0] for target in native} for row in nodes):
        raise ValueError("a node target is not a supported native Linux target")
    return {
        "commit": commit, "tag": tag, "version": tag[1:],
        "schema": {"algorithm": "sha384", "files": files, "sealed": sealed,
                   "fingerprint": hashlib.sha384(ledger_text(files).encode()).hexdigest()},
        "surfaces": surfaces,
        "native_matrix": {"include": [{"os": RUNNERS[target], "target": target} for target, *_ in native]},
        "package_matrix": {"include": [{"os": RUNNERS[target], "arch": arch} for target, _, arch, _ in nodes]},
        "connectors": [name for *_, name in native],
        "nodes": [name for *_, name in nodes],
    }


def require_source(root, commit, tag, *, require_sealed=True):
    if command("git", "-C", str(root), "rev-parse", "HEAD") != commit:
        raise ValueError("the checkout is not the pinned candidate commit")
    if command("git", "-C", str(root), "status", "--porcelain"):
        raise ValueError("release source changed after the candidate was pinned")
    return source_contract(root, commit, tag, require_sealed=require_sealed)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["seal-schema", "record"])
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--commit")
    parser.add_argument("--tag")
    parser.add_argument("--mode", choices=["release", "rehearsal", "warm"], default="release")
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    if args.action == "seal-schema":
        print(f"Sealed {len(seal_schema(args.root))} schema files; review and commit RELEASED before building")
        return
    if not args.commit or not args.tag or not args.out:
        parser.error("record requires --commit, --tag and --out")
    record = require_source(args.root, args.commit, args.tag, require_sealed=args.mode != "warm")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(record, indent=2) + "\n")
    if output := os.environ.get("GITHUB_OUTPUT"):
        with open(output, "a", encoding="utf-8") as file:
            for name in ("native_matrix", "package_matrix"):
                file.write(f"{name}={json.dumps(record[name], separators=(',', ':'))}\n")
    print(f"Pinned {record['commit']}, schema {record['schema']['fingerprint']}; no tag or release created")


if __name__ == "__main__":
    main()
