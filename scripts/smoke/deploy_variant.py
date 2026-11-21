#!/usr/bin/env python3
"""deploy_variant.py: a copy of a packaged release whose orch-deploy bundle differs, for the update scenarios.

  deploy_variant.py DIR --generation N --minimum M [--replace PATH=FILE]... [--strip-openapi] [--updater FILE] [--publish RELEASE]...

DIR is a copy of a release (the files that change are rewritten in it, so the others may be links to the original's). Its
bundle gets the files given, a FILE_SETS of the generation and minimum, the rows of every release given to --publish (as the
published releases of the bundle) and its own, and version.json says the same; update.sh beside the bundle follows the bundle.
The manifest's hashes are written afterwards by `cargo xtask package --out DIR --manifest-only`.
--strip-openapi takes the two key routes out of nginx.conf: the nginx.conf of v0.1.1, which the routes of v0.1.2 replaced.
"""
import argparse, gzip, hashlib, io, json, os, re, sys, tarfile
from pathlib import Path


def followed(path):
    return not path.startswith("config/") and path != "FILE_SETS"


def read_bundle(path):
    with tarfile.open(path, "r:gz") as archive:
        return [(info, archive.extractfile(info).read() if info.isfile() else b"") for info in archive.getmembers()]


def rows_of(tag, generation, members):
    out = []
    for info, data in members:
        if info.isfile() and info.name.startswith("deploy/") and followed(info.name[len("deploy/"):]):
            out.append(f"file {tag} {generation} {hashlib.sha256(data).hexdigest()} {info.name[len('deploy/'):]}")
    return sorted(out, key=lambda line: line.split()[4])


def replace_file(path, data):
    path = Path(path)
    if path.exists() or path.is_symlink():
        path.unlink()
    path.write_bytes(data)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("directory")
    parser.add_argument("--generation", type=int, required=True)
    parser.add_argument("--minimum", type=int, required=True)
    parser.add_argument("--replace", action="append", default=[])
    parser.add_argument("--strip-openapi", action="store_true")
    parser.add_argument("--updater")
    parser.add_argument("--publish", action="append", default=[])
    args = parser.parse_args()
    directory = Path(args.directory)
    manifest = json.loads((directory / "version.json").read_text())
    tag = manifest["tag"]
    bundle = directory / f"orch-deploy-{tag}.tar.gz"
    members = read_bundle(bundle)
    replacements = {}
    for item in args.replace:
        path, _, source = item.partition("=")
        replacements[f"deploy/{path}"] = Path(source).read_bytes()
    if args.updater:
        replacements["deploy/update.sh"] = Path(args.updater).read_bytes()
    published = []
    for release in args.publish:
        other = Path(release)
        other_tag = json.loads((other / "version.json").read_text())["tag"]
        other_members = read_bundle(other / f"orch-deploy-{other_tag}.tar.gz")
        generation = 1
        for info, data in other_members:
            if info.name == "deploy/FILE_SETS":
                generation = int(re.search(r"^generation (\d+)$", data.decode(), re.M)[1])
                published.append(f"file {other_tag} {generation} {hashlib.sha256(data).hexdigest()} FILE_SETS")
        published += rows_of(other_tag, generation, other_members)
    out = []
    for info, data in members:
        if info.name == "deploy/nginx.conf" and args.strip_openapi:
            text = data.decode()
            text = re.sub(r"\n[ \t]*# [^\n]*key[^\n]*\n(?=[ \t]*location /openapi)", "\n", text)
            for route in ("/openapi/v1/ws", "/openapi/"):
                text = re.sub(r"[ \t]*location " + re.escape(route) + r" \{[^{}]*\}\n*", "", text)
            if "/openapi/" in text:
                sys.exit("deploy_variant: the key routes are still in nginx.conf")
            data = text.encode()
        data = replacements.pop(info.name, data)
        out.append((info, data))
    if replacements:
        sys.exit(f"deploy_variant: the bundle has no {', '.join(replacements)}")
    files = [(info, data) for info, data in out if info.name != "deploy/FILE_SETS"]
    old = next(data for info, data in out if info.name == "deploy/FILE_SETS").decode()
    keep = [line for line in old.splitlines() if line and not line.startswith("file ")]
    keep = [f"generation {args.generation}" if line.startswith("generation ") else f"minimum {args.minimum}" if line.startswith("minimum ") else line for line in keep]
    kept = {line for line in old.splitlines() if line.startswith("file ") and line.split()[1] != tag}
    table = "\n".join(keep + sorted(kept | set(published)) + rows_of(tag, args.generation, files)) + "\n"
    buffer = io.BytesIO()
    with gzip.GzipFile(fileobj=buffer, mode="wb", mtime=0) as zipped:
        with tarfile.open(fileobj=zipped, mode="w") as archive:
            for info, data in files:
                info = tarfile.TarInfo(info.name) if False else info
                info.size = len(data)
                archive.addfile(info, io.BytesIO(data) if info.isfile() else None)
            entry = tarfile.TarInfo("deploy/FILE_SETS")
            entry.size = len(table.encode())
            entry.mode = 0o644
            archive.addfile(entry, io.BytesIO(table.encode()))
    replace_file(bundle, buffer.getvalue())
    updater = next(data for info, data in files if info.name == "deploy/update.sh")
    path = directory / "update.sh"
    replace_file(path, updater)
    os.chmod(path, 0o755)
    manifest["deploy_files"] = {"generation": args.generation, "minimum": args.minimum}
    replace_file(directory / "version.json", (json.dumps(manifest, indent=2) + "\n").encode())


if __name__ == "__main__":
    main()
