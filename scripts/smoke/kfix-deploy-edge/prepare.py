"""Prepare verified frontend failure payloads without changing the supplied release."""
import io
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile

source, target, variant, hasher = sys.argv[1:]
source, target = Path(source), Path(target)
target.mkdir(parents=True)
for file in source.iterdir():
    if file.is_file():
        shutil.copy2(file, target / file.name)
archive = target / "orch-frontend-dist.tar.gz"
temporary = target / "frontend.new.tar.gz"
with tarfile.open(source / archive.name, "r:gz") as before, tarfile.open(temporary, "w:gz") as after:
    for entry in before:
        if variant == "broken" and entry.name == "dist/index.html":
            continue
        after.addfile(entry, before.extractfile(entry) if entry.isfile() else None)
    if variant == "missing":
        marker = tarfile.TarInfo("dist/kfix-deploy-disable-edge")
        marker.mode = 0o644
        marker.size = 0
        after.addfile(marker, io.BytesIO())
temporary.replace(archive)
manifest = json.loads((source / "version.json").read_text())
digest = subprocess.check_output([hasher, "checksum", str(archive)], text=True).split()[0]
for asset in manifest["assets"]:
    if asset["name"] == archive.name:
        asset["size"] = archive.stat().st_size
        asset["blake3"] = digest
temporary = target / "version.new.json"
temporary.write_text(json.dumps(manifest, indent=2) + "\n")
temporary.replace(target / "version.json")
