#!/usr/bin/env python3
"""Carry an ordinary Orch release across a network boundary without publishing another artifact."""

import argparse
import fnmatch
import hashlib
import ipaddress
import json
import os
import platform
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib
import urllib.parse
import urllib.request

# A silent peer gets a bounded attempt; a large transfer may keep making progress.
SILENCE_SECONDS = 30
# Image saves and loads can be large; operators may change this command deadline.
COMMAND_SECONDS = int(os.environ.get("ORCH_OFFLINE_COMMAND_TIMEOUT", "1800"))
# Stream hashes instead of holding release archives in memory.
HASH_CHUNK = 1024 * 1024
# Vendor documents and the carried checksum import fit the API's one-MiB JSON request.
METADATA_BYTES = 1024 * 1024
PROGRAM_MANIFESTS = Path(__file__).resolve().parent.parent.parent / "crates/contracts/agent-programs/manifests"
HEX = re.compile(r"[0-9a-f]{64}\Z")
# Bearer token characters come from the HTTP bearer credential grammar; reject malformed values without echoing them.
BEARER_TOKEN = re.compile(r"[A-Za-z0-9._~+/-]+=*\Z")


def run(*args, capture=True):
    return subprocess.run(args, check=True, text=True, capture_output=capture,
                          timeout=COMMAND_SECONDS).stdout


def sha256(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def plain(name):
    if not name or name != Path(name).name or "\\" in name or name.startswith("."):
        raise ValueError(f"not a plain file name: {name!r}")
    return name


def local(root, name):
    path = root / name
    if Path(name).is_absolute() or ".." in Path(name).parts or path.is_symlink():
        raise ValueError("a carried-in path escapes its directory or is a symbolic link")
    if not path.resolve().is_relative_to(root.resolve()):
        raise ValueError("a carried-in path escapes its directory")
    return path


def fetch(url, path, *, limit=None):
    address = urllib.parse.urlsplit(url)
    if address.scheme != "https" or address.username or address.password:
        raise ValueError("gather downloads need HTTPS without credentials in the address")
    request = urllib.request.Request(url, headers={"User-Agent": "orch-offline"})
    with urllib.request.urlopen(request, timeout=SILENCE_SECONDS) as answer:
        if urllib.parse.urlsplit(answer.url).scheme != "https":
            raise ValueError("a download redirected away from HTTPS")
        with path.open("wb") as out:
            if limit is None:
                shutil.copyfileobj(answer, out, HASH_CHUNK)
            else:
                remaining = limit
                while chunk := answer.read(min(HASH_CHUNK, remaining + 1)):
                    if len(chunk) > remaining:
                        raise ValueError("vendor metadata exceeds the JSON size limit")
                    out.write(chunk)
                    remaining -= len(chunk)


def read_json(path):
    with path.open("rb") as source:
        data = source.read(METADATA_BYTES + 1)
    if len(data) > METADATA_BYTES:
        raise ValueError("metadata exceeds the JSON size limit; gather fewer builds")
    return json.loads(data)


def blake3(path, cli):
    command = ["b3sum", "--no-names"] if shutil.which("b3sum") else [str(cli), "checksum"]
    digest = run(*command, str(path)).split()[0]
    if not HEX.fullmatch(digest):
        raise ValueError("the release checksum tool did not return BLAKE3")
    return digest


def verify_release(directory, listing, arch):
    cli = directory / f"orchctl-linux-{arch}-musl"
    if not shutil.which("b3sum"):
        asset = next(item for item in listing["assets"] if item["name"] == cli.name)
        if cli.stat().st_size != asset["size"]:
            raise ValueError("the release checksum tool has the wrong size")
        if asset.get("sha256") and sha256(cli) != asset["sha256"]:
            raise ValueError("the release checksum tool has the wrong SHA-256")
        cli.chmod(0o755)
    for asset in listing["assets"]:
        path = local(directory, plain(asset["name"]))
        if path.stat().st_size != asset["size"] or blake3(path, cli) != asset["blake3"]:
            raise ValueError(f"release checksum mismatch: {path.name}")


def vendor_checksum(build, channel, document):
    """Interpret the program manifest's channel without depending on a program's name."""
    version, arch = build["version"], build["arch"]
    kind = channel["type"]
    if kind == "github_release":
        tag = channel.get("tag_prefix", "") + version
        url = channel.get("api", "https://api.github.com").rstrip("/") + "/repos/" + channel["repository"] + "/releases/tags/" + urllib.parse.quote(tag, safe="")
        published = document(url)
        if published.get("draft") or (published.get("prerelease") and not channel.get("prereleases", False)):
            raise ValueError("a vendor build is not a stable release")
        pattern = next(platform["name"] for platform in channel["assets"] if platform["arch"] == arch).format(version=version, tag=tag, arch=arch)
        entry = next(item for item in published["assets"] if fnmatch.fnmatchcase(item["name"], pattern))
        if channel["checksum"]["from"] != "digest":
            raise ValueError("gather needs a vendor SHA-256 digest for this channel")
        return entry.get("digest", "").removeprefix("sha256:"), entry["browser_download_url"]
    if kind == "pypi":
        url = channel.get("index", "https://pypi.org").rstrip("/") + "/pypi/" + urllib.parse.quote(channel["project"], safe="") + "/" + urllib.parse.quote(version, safe="") + "/json"
        published = document(url)
        pattern = next(platform["name"] for platform in channel["files"] if platform["arch"] == arch).format(version=version, arch=arch, tag=version)
        entry = next(item for item in published["urls"] if not item.get("yanked") and fnmatch.fnmatchcase(item["filename"], pattern))
        return entry.get("digests", {}).get("sha256", ""), entry["url"]
    if kind == "signed_manifest":
        if channel.get("signature"):
            raise ValueError("gather cannot verify this channel's detached signature")
        arch = next(platform["word"] for platform in channel["platforms"] if platform["arch"] == arch)
        base = channel["base"].rstrip("/")
        published = document(channel.get("manifest", "{base}/{version}/manifest.json").format(base=base, version=version, arch=arch))
        named_version = published
        for part in channel.get("version_pointer", "/version").lstrip("/").split("/"):
            named_version = named_version[part.replace("~1", "/").replace("~0", "~")]
        if named_version != version:
            raise ValueError("the vendor manifest names another version")
        entry = published["platforms"][arch]
        file = entry.get(channel.get("file_member", "binary"), channel.get("file_default", ""))
        address = channel.get("file", "{base}/{version}/{arch}/{file}").format(base=base, version=version, arch=arch, file=file)
        return entry[channel.get("checksum_member", "checksum")], address
    raise ValueError("gather needs a vendor SHA-256 channel supported by the checksum reader")


def vendor_builds(builds, out):
    """Only the program's declared vendor channel establishes its expected checksum."""
    verified, metadata = [], {}
    for build in builds:
        kind, version = build["kind"], build["version"]
        if not re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9_-]{0,63}", kind) or not re.fullmatch(r"[0-9][A-Za-z0-9.+_-]{0,63}", version):
            raise ValueError("gather needs a plain program name and an exact version")
        manifest = read_json(PROGRAM_MANIFESTS / (kind + ".json"))
        if manifest["kind"] != kind:
            raise ValueError("the program manifest names another program")
        print("Reading vendor checksum", kind, version, build["arch"], flush=True)
        def document(url):
            if url not in metadata:
                # One exact release at a time; a growing selection never retains every vendor reply.
                metadata.clear()
                path = out / (kind + "-" + version + "-" + hashlib.sha256(url.encode()).hexdigest() + ".json")
                fetch(url, path, limit=METADATA_BYTES)
                metadata[url] = read_json(path)
            return metadata[url]
        reasons = []
        for channel in manifest.get("builds", {}).get("channels", []):
            try:
                expected, address = vendor_checksum(build, channel, document)
                if not isinstance(expected, str) or not HEX.fullmatch(expected) or expected != build["sha256"]:
                    raise ValueError("the vendor checksum differs from the selected release build")
                break
            except (KeyError, StopIteration, ValueError) as error:
                reasons.append(str(error) or "the vendor has no matching platform file")
        else:
            raise ValueError("; ".join(reasons) or "the program has no vendor checksum channel")
        verified.append({**build, "url": address, "sha256": expected})
    document = json.dumps(verified, indent=2) + "\n"
    if len(json.dumps({"document": document}).encode()) > METADATA_BYTES:
        raise ValueError("the checksum list exceeds the import request size; gather fewer builds")
    for build in verified:
        address, expected = build["url"], build["sha256"]
        parsed = urllib.parse.urlsplit(address)
        path = local(out.parent / "mirror", parsed.netloc + "/" + parsed.path.lstrip("/"))
        path.parent.mkdir(parents=True, exist_ok=True)
        print("Saving agent program", build["kind"], build["version"], build["arch"], flush=True)
        fetch(address, path)
        if sha256(path) != expected:
            raise ValueError("the agent program does not match its vendor checksum")
    return verified


def saved_image_config(path):
    with tarfile.open(path) as archive:
        selected = json.load(archive.extractfile("manifest.json"))
        if len(selected) != 1:
            raise ValueError("an image save must contain one selected platform")
        config = archive.extractfile(selected[0]["Config"]).read()
        return "sha256:" + hashlib.sha256(config).hexdigest()


def loaded_image_config(reference):
    # Containerd uses a manifest ID where older engines use a config ID. Re-export only in that case
    # so the exact config bytes can still be checked without trusting a mutable registry alias.
    with tempfile.NamedTemporaryFile(prefix="orch-offline-", suffix=".tar") as temporary:
        path = Path(temporary.name)
        run("docker", "image", "save", "-o", str(path), reference)
        return saved_image_config(path)


def image_save(reference, out, index, arch):
    if not re.fullmatch(r"sha256:[0-9a-f]{64}", reference.get("digest", "")):
        raise ValueError("each image needs its release or operator-selected SHA-256 digest")
    source = reference["reference"]
    pinned = source if "@" in source else f"{source}@{reference['digest']}"
    try:
        image = json.loads(run("docker", "image", "inspect", pinned))[0]
    except subprocess.CalledProcessError:
        # Locally built release images are pinned by image ID rather than a registry index.
        try:
            candidate = json.loads(run("docker", "image", "inspect", source))[0]
            image = candidate if candidate["Id"] == reference["digest"] else None
        except subprocess.CalledProcessError:
            image = None
        if image is None:
            run("docker", "pull", pinned, capture=False)
            image = json.loads(run("docker", "image", "inspect", pinned))[0]
    expected_arch = {"x86_64": "amd64", "aarch64": "arm64"}[arch]
    if image.get("Os") != "linux" or image.get("Architecture") != expected_arch:
        raise ValueError(f"{source} is not Linux {arch}; load that platform on the gathering daemon first")
    if image["Id"] != reference["digest"] and not any(item.endswith("@" + reference["digest"]) for item in image.get("RepoDigests", [])):
        raise ValueError("the local image is not the selected digest")
    path = out / "images" / f"{index}.tar"
    archive_tag = "orch-offline-transfer:" + reference["digest"].removeprefix("sha256:")
    run("docker", "image", "tag", image["Id"], archive_tag)
    run("docker", "image", "save", "-o", str(path), archive_tag)
    content = saved_image_config(path)
    return {"source": source, "digest": reference["digest"], "id": content,
            "archive": path.relative_to(out).as_posix(), "archive_tag": archive_tag,
            "name": reference.get("name")}


def gather(args):
    out = args.out.resolve()
    if out.exists() and any(out.iterdir()):
        raise ValueError("the gathering directory must be empty")
    out.mkdir(parents=True, exist_ok=True)
    for name in ("release", "images", "vendor", "mirror"):
        (out / name).mkdir()
    release = out / "release"
    if args.release_dir:
        shutil.copyfile(args.release_dir / "version.json", release / "version.json")
    else:
        fetch(args.release_url.rstrip("/") + "/version.json", release / "version.json")
    listing = read_json(release / "version.json")
    if listing.get("project") != "orch" or listing.get("schema_version") != 1:
        raise ValueError("not an Orch release manifest")
    for asset in listing["assets"]:
        name = plain(asset["name"])
        if args.release_dir:
            shutil.copyfile(args.release_dir / name, release / name)
        else:
            fetch(urllib.parse.urljoin(args.release_url.rstrip("/") + "/", asset["download_url"]), release / name)
    verify_release(release, listing, args.arch)
    images = list(listing["images"])
    # The provider template builds on this pinned base; carry it even before a provider is added.
    recipe = Path(__file__).resolve().parents[2] / "images/orch-provider-base/Dockerfile"
    provider = next(line.split()[1] for line in recipe.read_text().splitlines() if line.startswith("FROM "))
    images.append({"reference": provider, "digest": provider.split("@", 1)[1]})
    for value in args.image + ([args.local_embeddings] if args.local_embeddings else []):
        reference, digest = value.rsplit("@", 1)
        images.append({"reference": reference, "digest": digest, "registry": False})
    saved, seen = [], set()
    for image in images:
        if image["reference"] not in seen:
            print("Saving image", image["reference"], flush=True)
            saved.append(image_save(image, out, len(saved), args.arch))
            seen.add(image["reference"])
    builds = listing.get("harness_builds", [])
    if args.harness_builds:
        builds = read_json(args.harness_builds)
    builds = [build for build in builds if build["os"] == "linux" and build["arch"] == args.arch]
    checksums = vendor_builds(builds, out / "vendor")
    (out / "vendor-checksums.json").write_text(json.dumps(checksums, indent=2) + "\n")
    for name in ("install.sh", "transfer.py"):
        shutil.copyfile(Path(__file__).with_name(name), out / name)
    (out / "install.sh").chmod(0o755)
    files = [{"path": path.relative_to(out).as_posix(), "sha256": sha256(path)}
             for path in sorted(out.rglob("*")) if path.is_file()]
    record = {"format": 1, "arch": args.arch, "tag": listing["tag"], "images": saved, "files": files}
    (out / "transfer.json").write_text(json.dumps(record, indent=2) + "\n")
    digest = sha256(out / "transfer.json")
    (out / "transfer.sha256").write_text(digest + "\n")
    print(f"Gathered {listing['tag']}; transfer manifest SHA-256: {digest}")


def verify(directory, expected=None):
    digest = sha256(directory / "transfer.json")
    expected = expected or (directory / "transfer.sha256").read_text().strip()
    if digest != expected:
        raise ValueError("transfer manifest checksum mismatch")
    record = json.loads((directory / "transfer.json").read_text())
    if record.get("format") != 1:
        raise ValueError("unknown transfer format")
    for item in record["files"]:
        if sha256(local(directory, item["path"])) != item["sha256"]:
            raise ValueError(f"carried-in file checksum mismatch: {item['path']}")
    print(f"Verified transfer manifest SHA-256: {digest}", flush=True)
    return record


def import_checksums(args, directory):
    if not args.api_url or not args.token_file:
        raise ValueError("checksum import needs --api-url and --token-file for a system administrator session")
    address = urllib.parse.urlsplit(args.api_url)
    try:
        loopback = address.hostname == "localhost" or ipaddress.ip_address(address.hostname).is_loopback
    except ValueError:
        loopback = False
    if address.username or address.password or address.query or address.fragment:
        raise ValueError("the API base address has no credentials, query or fragment")
    if address.scheme != "https" and not (address.scheme == "http" and loopback):
        raise ValueError("the API needs HTTPS; loopback HTTP is allowed for a local installation")
    token = args.token_file.read_text().strip()
    if not BEARER_TOKEN.fullmatch(token):
        raise ValueError("the token file must contain one bearer token")
    document = (directory / "vendor-checksums.json").read_bytes().decode("utf-8")
    digest = hashlib.sha256(document.encode()).hexdigest()
    print(f"Importing vendor checksum list SHA-256: {digest}", flush=True)
    request = urllib.request.Request(args.api_url.rstrip("/") + "/admin/harnesses/checksums",
        data=json.dumps({"document": document}).encode(), method="POST",
        headers={"Content-Type": "application/json", "Authorization": "Bearer " + token})
    # An authenticated import must not follow a redirect to a different authority.
    class NoRedirect(urllib.request.HTTPRedirectHandler):
        def redirect_request(self, req, fp, code, msg, headers, newurl):
            return None
    handlers = [NoRedirect]
    if loopback:
        handlers.append(urllib.request.ProxyHandler({}))
    opener = urllib.request.build_opener(*handlers)
    with opener.open(request, timeout=SILENCE_SECONDS) as response:
        reply = json.load(response)
    if reply["sha256"] != digest:
        raise ValueError("the server recorded a different checksum list")
    print(f"Imported {reply['count']} vendor checksums; the administrator act is audited")


def image_settings(text, mapping):
    """Replace only the node's image map, preserving other operator configuration verbatim."""
    expected = tomllib.loads(text)
    expected.setdefault("node", {}).setdefault("docker", {})["offline_images"] = mapping
    table = re.compile(r"(?m)^[ \t]*\[node\.docker\.offline_images\][^\n]*(?:\n|$)")
    match = table.search(text)
    if match:
        following = re.search(r"(?m)^[ \t]*\[", text[match.end():])
        end = match.end() + following.start() if following else len(text)
        text = text[:match.start()] + text[end:]
    header = re.search(r"(?m)^[ \t]*\[node\.docker\][^\n]*(?:\n|$)", text)
    if header is None:
        text += "\n[node.docker]\n"
        header = re.search(r"(?m)^\[node\.docker\]\n", text)
    following = re.search(r"(?m)^[ \t]*\[", text[header.end():])
    end = header.end() + following.start() if following else len(text)
    body = re.sub(r"(?m)^[ \t]*offline_images\s*=.*(?:\n|$)", "", text[header.end():end])
    setting = "offline_images = { " + ", ".join(json.dumps(key) + " = " + json.dumps(value) for key, value in mapping.items()) + " }\n"
    prefix = text[:header.end()]
    if not prefix.endswith("\n"):
        prefix += "\n"
    updated = prefix + setting + body + text[end:]
    if tomllib.loads(updated) != expected:
        raise ValueError("the local image map could not be changed without altering another configuration value")
    return updated


def install(args):
    directory = args.from_dir.resolve()
    record = verify(directory, args.sha256)
    if args.mode == "verify":
        return
    if args.mode == "import":
        import_checksums(args, directory)
        return
    if args.mode == "install" and not callable(getattr(tarfile, "data_filter", None)):
        raise ValueError("first install needs Python's safe archive data filters; update Python 3.11 or newer / 首次安装需要安全归档过滤器，请更新 Python")
    for image in record["images"]:
        print("Loading image", image["source"], flush=True)
        run("docker", "image", "load", "-i", str(local(directory, image["archive"])), capture=False)
        selector = image.get("archive_tag", image["id"])
        found = json.loads(run("docker", "image", "inspect", selector))[0]
        if found["Id"] != image["id"] and loaded_image_config(selector) != image["id"]:
            raise ValueError("a loaded image has a different content ID")
        if not found["Id"].startswith("sha256:") or not HEX.fullmatch(found["Id"].removeprefix("sha256:")):
            raise ValueError("the Docker daemon returned no immutable image ID")
        image["local_id"] = found["Id"]
        run("docker", "image", "tag", image["local_id"], image["source"].split("@", 1)[0])
        run("docker", "image", "tag", image["local_id"], "orch-offline:" + image["local_id"].removeprefix("sha256:"))
        # The stock template uses this local release tag even when the published image lives in a registry.
        if image.get("name") == "orch-env-base":
            image["stock_reference"] = "orch-env-base:" + record["tag"].removeprefix("v")
            run("docker", "image", "tag", image["local_id"], image["stock_reference"])
    if args.mode == "load":
        return
    if not args.target:
        raise ValueError("install and update need --target")
    target = args.target.resolve()
    if args.mode == "install":
        if target.exists() and any(target.iterdir()):
            raise ValueError("a first installation needs an empty target")
        if not args.config or not args.env:
            raise ValueError("a first installation needs --config and --env prepared for the internal network")
        target.mkdir(parents=True, exist_ok=True)
        with tarfile.open(directory / "release" / f"orch-deploy-{record['tag']}.tar.gz") as archive:
            archive.extractall(target, filter="data")
        target = target / "deploy"
        shutil.copyfile(args.config, target / "config/orch.toml")
        shutil.copyfile(args.env, target / ".env")
        (target / ".env").chmod(0o600)
    elif not (target / "update.sh").is_file():
        raise ValueError("update target must be the existing deploy directory")
    previous_mapping = target / "offline-images.json"
    mapping = json.loads(previous_mapping.read_text()) if previous_mapping.exists() else {}
    config = target / "config/orch.toml"
    text = config.read_text()
    mapping.update(tomllib.loads(text).get("node", {}).get("docker", {}).get("offline_images", {}))
    for image in record["images"]:
        for source in {image["source"], image.get("stock_reference", image["source"])}:
            tag = source.split("@", 1)[0]
            last_slash = tag.rfind("/")
            repository = tag[:tag.rfind(":")] if tag.rfind(":") > last_slash else tag
            for reference in (source, tag, f"{tag}@{image['digest']}", f"{tag}@{image['id']}",
                              f"{repository}@{image['digest']}", f"{repository}@{image['id']}",
                              f"{tag}@{image['local_id']}", f"{repository}@{image['local_id']}"):
                mapping[reference] = image["local_id"]
    (target / "offline-images.json").write_text(json.dumps(mapping, indent=2) + "\n")
    # Compose accepts JSON as YAML. Content IDs survive save/load even when registry aliases do not.
    compose = (target / "compose.yml").read_text()
    by_service, inherited = {}, None
    for line in compose.splitlines():
        match = re.match(r"\s*image:\s*([^\s#]+)", line)
        if match and inherited is None:
            inherited = match[1]
    service = None
    for line in compose.splitlines():
        match = re.match(r"^  ([a-z][a-z0-9_-]*):$", line)
        if match:
            service = match[1]
        image = re.match(r"^    image:\s*([^\s#]+)", line)
        if service and (image or line.strip() == "<<: *service"):
            source = image[1] if image else inherited
            if source not in mapping:
                raise ValueError(f"no gathered image for Compose service {service}")
            by_service[service] = {"image": mapping[source], "pull_policy": "never"}
    (target / "compose.offline.yml").write_text(json.dumps({"services": by_service}, indent=2) + "\n")
    # Keep the ordinary update command and service restarts offline after this script exits, too.
    config.write_text(image_settings(text, mapping))
    env_path = target / ".env"
    env_text = env_path.read_text()
    compose_files = os.environ.get("COMPOSE_FILE")
    if compose_files is None:
        matches = re.findall(r"^COMPOSE_FILE=(.*)$", env_text, re.MULTILINE)
        compose_files = matches[-1].strip("\"'") if matches else "compose.yml"
    if "compose.offline.yml" not in compose_files.split(":"):
        compose_files += ":compose.offline.yml"
    env_text = re.sub(r"^(?:COMPOSE_FILE|ORCH_OFFLINE_IMAGE_MAP)=.*\n", "", env_text, flags=re.MULTILINE)
    env_path.write_text(env_text + f"\nCOMPOSE_FILE={compose_files}\nORCH_OFFLINE_IMAGE_MAP=./offline-images.json\n")
    env_path.chmod(0o600)
    environment = dict(os.environ, ORCH_OFFLINE_IMAGE_MAP=str(target / "offline-images.json"))
    if "COMPOSE_FILE" in os.environ:
        environment["COMPOSE_FILE"] = compose_files
    else:
        environment.pop("COMPOSE_FILE", None)
    print(f"Using the ordinary updater in {target}", flush=True)
    subprocess.run(["bash", str(target / "update.sh"), "first-start" if args.mode == "install" else record["tag"],
                    "--from", str(directory / "release"), *args.updater_args],
                   cwd=target, env=environment, check=True, timeout=COMMAND_SECONDS)


def main():
    parser = argparse.ArgumentParser(description=__doc__, epilog="中文：gather 在联网机器收集，install 在内网校验并安装或更新；--mode verify 仅校验。所有令牌只从文件读取。")
    modes = parser.add_subparsers(dest="command", required=True)
    gathering = modes.add_parser("gather", prog="gather.sh", help="gather on a connected host / 在联网机器收集", description="Save an ordinary release on a connected machine / 在联网机器保存常规发布")
    source = gathering.add_mutually_exclusive_group(required=True)
    source.add_argument("--release-dir", type=Path, help="existing ordinary release / 已有发布目录")
    source.add_argument("--release-url", help="HTTPS directory containing version.json / 发布文件的 HTTPS 目录")
    gathering.add_argument("--out", required=True, type=Path, help="empty output directory / 空的输出目录")
    gathering.add_argument("--arch", choices=["x86_64", "aarch64"], default=platform.machine(), help="Linux architecture to carry / 要携带的 Linux 架构")
    gathering.add_argument("--harness-builds", type=Path, help="exact builds with vendor checksums / 厂商版本列表")
    gathering.add_argument("--image", action="append", default=[], help="additional image@sha256:digest / 额外镜像")
    gathering.add_argument("--local-embeddings", help="opt-in finished image@sha256:digest / 可选的本地嵌入镜像")
    installing = modes.add_parser("install", prog="install.sh", help="verify and use the ordinary installer / 校验后使用常规安装器", description="Verify carried files, then use the ordinary installer / 校验携带的文件，再使用常规安装器")
    installing.add_argument("--from", dest="from_dir", required=True, type=Path, help="carried directory / 携带的目录")
    installing.add_argument("--sha256", help="manifest digest carried separately / 单独携带的清单摘要")
    installing.add_argument("--mode", choices=["install", "update", "load", "verify", "import"], default="install", help="install, update, load images, verify, or import checksums / 安装、更新、加载镜像、校验或导入校验和")
    installing.add_argument("--target", type=Path, help="empty install directory or existing deploy directory / 安装目录或已有 deploy 目录")
    installing.add_argument("--config", type=Path, help="prepared orch.toml for a first install / 首次安装准备好的 orch.toml")
    installing.add_argument("--env", type=Path, help="private .env for a first install / 首次安装的私有 .env")
    installing.add_argument("--api-url", help="internal API URL ending in /api/v1 / 内网 API 地址")
    installing.add_argument("--token-file", type=Path, help="administrator browser-session token file / 管理员会话令牌文件")
    installing.add_argument("updater_args", nargs=argparse.REMAINDER, help="ordinary updater options after -- / -- 后传入常规更新器选项")
    args = parser.parse_args()
    if args.command == "install" and args.updater_args[:1] == ["--"]:
        args.updater_args = args.updater_args[1:]
    try:
        (gather if args.command == "gather" else install)(args)
        return 0
    except (OSError, ValueError, KeyError, StopIteration, subprocess.SubprocessError, tarfile.TarError) as error:
        # Do not print command output or credentials returned by an authenticated endpoint.
        print(f"offline: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
