"""Exercise carried-in integrity, vendor authority and the ordinary installer boundary without external services."""

import argparse
import contextlib
import hashlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import tarfile
import tempfile
import tracemalloc
import unittest
import tomllib
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("transfer", Path(__file__).with_name("transfer.py"))
transfer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(transfer)


class TransferTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.carry = self.root / "carry"
        self.carry.mkdir()
        (self.carry / "images").mkdir()
        (self.carry / "release").mkdir()
        self.content = "sha256:" + "a" * 64
        self.pin = "sha256:" + "b" * 64
        self.source = "debian:stable@" + self.pin

    def record(self):
        path = self.carry / "images/0.tar"
        path.write_bytes(b"verified image archive")
        (self.carry / "vendor-checksums.json").write_text("[]\n")
        # A tiny installer stand-in lets the test observe the handoff without starting services.
        with tarfile.open(self.carry / "release/orch-deploy-v1.0.0.tar.gz", "w:gz") as archive:
            for name, data in {
                "deploy/update.sh": b"#!/bin/bash\nprintf '%s\\n' \"$@\" > updater-arguments\n",
                "deploy/config/orch.toml": b"[system]\npublic_url = 'https://internal.example'\n[node.docker]\n",
                "deploy/compose.yml": f"x-service: &service\n  image: {self.source}\nservices:\n  api:\n    <<: *service\n".encode(),
            }.items():
                header = tarfile.TarInfo(name)
                header.size = len(data)
                archive.addfile(header, io.BytesIO(data))
        files = [{"path": path.relative_to(self.carry).as_posix(), "sha256": transfer.sha256(path)}
                 for path in sorted(self.carry.rglob("*")) if path.is_file()]
        record = {"format": 1, "tag": "v1.0.0", "files": files, "images": [{
            "source": self.source, "digest": self.pin, "id": self.content, "archive": "images/0.tar"}]}
        (self.carry / "transfer.json").write_text(json.dumps(record))
        (self.carry / "transfer.sha256").write_text(transfer.sha256(self.carry / "transfer.json"))

    def args(self, **changes):
        args = dict(from_dir=self.carry, sha256=None, mode="verify", target=None, config=None,
                    env=None, api_url=None, token_file=None, updater_args=[])
        return argparse.Namespace(**(args | changes))

    def daemon(self, *args, **kwargs):
        if args[:3] == ("docker", "image", "inspect"):
            return json.dumps([{"Id": self.content}])
        return ""

    def test_a_gibibyte_download_never_reads_or_holds_more_than_one_chunk(self):
        total = 1024 * 1024 * 1024
        sizes = []
        written = 0
        test = self

        class Source:
            url = "https://vendor.example.test/program"
            left = total

            def read(self, size=-1):
                test.assertGreater(size, 0, "an unbounded read would hold the whole download")
                test.assertLessEqual(size, transfer.HASH_CHUNK)
                size = min(size, self.left)
                self.left -= size
                sizes.append(size)
                return b"x" * size

        class Sink:
            def write(self, chunk):
                nonlocal written
                test.assertLessEqual(len(chunk), transfer.HASH_CHUNK)
                written += len(chunk)

        source = Source()
        with patch.object(transfer.urllib.request, "urlopen", return_value=contextlib.nullcontext(source)) as opened:
            with patch.object(Path, "open", return_value=contextlib.nullcontext(Sink())):
                transfer.fetch(source.url, self.root / "streamed-program")
        self.assertEqual(written, total)
        self.assertEqual(source.left, 0)
        self.assertEqual(max(sizes), transfer.HASH_CHUNK)
        self.assertEqual(len(sizes), total // transfer.HASH_CHUNK + 1)
        self.assertEqual(opened.call_args.kwargs["timeout"], transfer.SILENCE_SECONDS)

    def test_a_changed_carried_in_file_is_refused_before_docker_or_the_installer(self):
        self.record()
        (self.carry / "images/0.tar").write_bytes(b"changed")
        with patch.object(transfer, "run") as command:
            with self.assertRaisesRegex(ValueError, "checksum mismatch"):
                transfer.install(self.args(mode="load"))
            command.assert_not_called()

    def test_a_changed_manifest_is_refused_against_the_separately_carried_digest(self):
        self.record()
        original = transfer.sha256(self.carry / "transfer.json")
        (self.carry / "transfer.json").write_text("{}")
        with self.assertRaisesRegex(ValueError, "manifest checksum mismatch"):
            transfer.install(self.args(sha256=original))

    def test_an_old_archive_runtime_is_refused_before_loading_or_extracting_anything(self):
        self.record()
        with patch.object(transfer.tarfile, "data_filter", None), patch.object(transfer, "run") as command:
            with self.assertRaisesRegex(ValueError, "safe archive data filters"):
                transfer.install(self.args(mode="install"))
            command.assert_not_called()

    def test_a_multiarch_digest_does_not_allow_saving_the_wrong_selected_platform(self):
        image={"Id":self.content,"RepoDigests":[self.source],"Os":"linux","Architecture":"arm64"}
        with patch.object(transfer,"run",return_value=json.dumps([image])) as command:
            with self.assertRaisesRegex(ValueError,"not Linux x86_64"):
                transfer.image_save({"reference":self.source,"digest":self.pin},self.carry,0,"x86_64")
        self.assertEqual(len(command.call_args_list),1,"a wrong platform is refused before any image is saved")

    def test_loading_checks_the_actual_image_content_and_never_pulls(self):
        self.record()
        with patch.object(transfer, "run", side_effect=self.daemon) as command:
            transfer.install(self.args(mode="load"))
        self.assertTrue(any(call.args[:3] == ("docker", "image", "load") for call in command.call_args_list))
        self.assertFalse(any("pull" in call.args for call in command.call_args_list))
        with patch.object(transfer, "run", return_value=json.dumps([{"Id": "sha256:" + "c" * 64}])), patch.object(transfer, "loaded_image_config", return_value="sha256:" + "c" * 64):
            with self.assertRaisesRegex(ValueError, "different content ID"):
                transfer.install(self.args(mode="load"))

    def test_containerd_manifest_ids_still_require_the_exact_carried_config_bytes(self):
        self.record()
        local_id = "sha256:" + "c" * 64
        with patch.object(transfer, "run", return_value=json.dumps([{"Id": local_id}])), patch.object(transfer, "loaded_image_config", return_value=self.content) as checked:
            transfer.install(self.args(mode="load"))
        checked.assert_called_once_with(self.content)

    def test_a_published_environment_image_is_also_available_under_the_stock_template_tag(self):
        self.record()
        record = json.loads((self.carry / "transfer.json").read_text())
        record["images"][0]["name"] = "orch-env-base"
        record["images"][0]["source"] = "registry.example.test/team/orch-env-base:1.0.0"
        (self.carry / "transfer.json").write_text(json.dumps(record))
        (self.carry / "transfer.sha256").write_text(transfer.sha256(self.carry / "transfer.json"))
        with patch.object(transfer, "run", side_effect=self.daemon) as command:
            transfer.install(self.args(mode="load"))
        self.assertTrue(any(call.args == ("docker", "image", "tag", self.content, "orch-env-base:1.0.0") for call in command.call_args_list))

    def test_install_and_update_delegate_to_the_ordinary_updater_from_local_files(self):
        self.record()
        config = self.root / "orch.toml"
        config.write_text("[system]\npublic_url = 'https://inside.example'\n[node.docker]\n")
        env = self.root / "env"
        env.write_text("POSTGRES_PASSWORD=kept-private\nCOMPOSE_FILE=compose.yml:local.yml\n")
        target = self.root / "installation"
        with patch.object(transfer, "run", side_effect=self.daemon):
            transfer.install(self.args(mode="install", config=config, env=env, target=target))
        deploy = target / "deploy"
        arguments = (deploy / "updater-arguments").read_text().splitlines()
        self.assertEqual(arguments, ["first-start", "--from", str(self.carry / "release")])
        self.assertIn("inside.example", (deploy / "config/orch.toml").read_text())
        self.assertIn(self.content, (deploy / "config/orch.toml").read_text())
        self.assertIn("compose.yml:local.yml:compose.offline.yml", (deploy / ".env").read_text())
        # The ordinary updater retains Compose. New carried images must retain its older verified pins.
        next_record = json.loads((self.carry / "transfer.json").read_text())
        next_record["images"][0]["source"] = "debian:new@" + self.pin
        (self.carry / "transfer.json").write_text(json.dumps(next_record))
        (self.carry / "transfer.sha256").write_text(transfer.sha256(self.carry / "transfer.json"))
        with patch.object(transfer, "run", side_effect=self.daemon):
            transfer.install(self.args(mode="update", target=deploy, updater_args=["--yes"]))
        self.assertEqual((deploy / "updater-arguments").read_text().splitlines(),
                         ["v1.0.0", "--from", str(self.carry / "release"), "--yes"])
        self.assertIn("POSTGRES_PASSWORD=kept-private", (deploy / ".env").read_text())
        self.assertEqual((deploy / ".env").stat().st_mode & 0o777, 0o600)
        mapping = json.loads((deploy / "offline-images.json").read_text())
        self.assertEqual(mapping[self.source], self.content)
        self.assertEqual(mapping["debian:new@" + self.pin], self.content)

    def test_gather_reads_checksum_metadata_only_from_the_vendor(self):
        vendor = self.carry / "vendor"
        vendor.mkdir()
        artifact = b"vendor executable"
        digest = hashlib.sha256(artifact).hexdigest()
        address = "https://github.com/openai/codex/releases/download/rust-v1.2.3/codex-package-x86_64-unknown-linux-musl.tar.gz"
        metadata = {"draft": False, "prerelease": False, "assets": [{
            "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": "sha256:" + digest,
            "browser_download_url": address}]}
        build = {"kind": "codex", "version": "1.2.3", "os": "linux", "arch": "x86_64", "sha256": digest}
        def fetched(url, path, **_):
            path.write_bytes(json.dumps(metadata).encode() if url.startswith("https://api.github.com/") else artifact)
        with patch.object(transfer, "fetch", side_effect=fetched) as fetch:
            kept = transfer.vendor_builds([build], vendor)
        self.assertEqual(kept[0]["sha256"], digest)
        self.assertEqual(fetch.call_args_list[0].args[0], "https://api.github.com/repos/openai/codex/releases/tags/rust-v1.2.3")
        self.assertEqual(fetch.call_args_list[1].args[0], address)
        with patch.object(transfer, "fetch", side_effect=fetched):
            with self.assertRaisesRegex(ValueError, "vendor checksum differs"):
                transfer.vendor_builds([{**build, "sha256": "0" * 64}], vendor)

    def test_file_paths_cannot_escape_the_gathering_directory(self):
        for name in ("../secret", "/tmp/outside", ".hidden", "nested/file", "nested\\file"):
            with self.assertRaises(ValueError):
                transfer.plain(name)

    def test_a_shipped_wheel_uses_its_declared_index_checksum_and_ignores_the_mirror_url(self):
        vendor = self.carry / "vendor"
        vendor.mkdir()
        artifact = b"a verified wheel"
        digest = hashlib.sha256(artifact).hexdigest()
        address = "https://files.pythonhosted.org/packages/p/runtime-py3-none-manylinux_2_28_x86_64.whl"
        metadata = {"info": {"version": "0.2.1rc2"}, "urls": [{
            "filename": "runtime-py3-none-manylinux_2_28_x86_64.whl", "url": address,
            "digests": {"sha256": digest}, "yanked": False}]}
        build = {"kind": "dsh", "version": "0.2.1-rc.2", "os": "linux", "arch": "x86_64",
                 "sha256": digest, "url": "https://mirror.example.test/invented.whl"}
        def fetched(url, path, **kwargs):
            if url.startswith("https://pypi.org/"):
                self.assertEqual(kwargs, {"limit": transfer.METADATA_BYTES})
                path.write_text(json.dumps(metadata))
            else:
                self.assertEqual(url, address)
                path.write_bytes(artifact)
        with patch.object(transfer, "fetch", side_effect=fetched) as fetch:
            kept = transfer.vendor_builds([build], vendor)
        self.assertEqual(kept, [{**build, "url": address}])
        self.assertEqual(fetch.call_args_list[0].args[0],
                         "https://pypi.org/pypi/deepseek-harness-runtime-bin/0.2.1-rc.2/json")
        self.assertEqual(len(fetch.call_args_list), 2)
        for changed in ({"sha256": "0" * 64}, {"arch": "aarch64"}):
            with self.subTest(changed=changed), patch.object(transfer, "fetch", side_effect=fetched) as fetch:
                with self.assertRaises(ValueError):
                    transfer.vendor_builds([{**build, **changed}], vendor)
                self.assertEqual(len(fetch.call_args_list), 1, "refuse before downloading a program")
        metadata["urls"][0]["yanked"] = True
        with patch.object(transfer, "fetch", side_effect=fetched) as fetch:
            with self.assertRaisesRegex(ValueError, "no matching platform file"):
                transfer.vendor_builds([build], vendor)
            self.assertEqual(len(fetch.call_args_list), 1)

    def test_a_manifest_channel_uses_its_declared_platform_and_vendor_version(self):
        vendor = self.carry / "vendor"
        vendor.mkdir()
        artifact = b"verified manifest program"
        digest = hashlib.sha256(artifact).hexdigest()
        metadata = {"version": "1.2.3", "platforms": {"linux-arm64": {"checksum": digest}}}
        build = {"kind": "claude-code", "version": "1.2.3", "os": "linux", "arch": "aarch64", "sha256": digest}
        def fetched(url, path, **_):
            path.write_bytes(json.dumps(metadata).encode() if url.endswith("manifest.json") else artifact)
        with patch.object(transfer, "fetch", side_effect=fetched) as fetch:
            kept = transfer.vendor_builds([build], vendor)
        self.assertEqual(kept[0]["url"], "https://downloads.claude.ai/claude-code-releases/1.2.3/linux-arm64/claude")
        self.assertEqual(len(fetch.call_args_list), 2)
        metadata["version"] = "9.9.9"
        with patch.object(transfer, "fetch", side_effect=fetched) as fetch:
            with self.assertRaisesRegex(ValueError, "another version"):
                transfer.vendor_builds([build], vendor)
            self.assertEqual(len(fetch.call_args_list), 1)

    def test_vendor_metadata_stops_at_the_size_limit_before_reading_or_writing_more(self):
        source = io.BytesIO(b"x" * (transfer.METADATA_BYTES + 1))
        source.url = "https://vendor.example.test/metadata"
        path = self.root / "too-large.json"
        with patch.object(transfer.urllib.request, "urlopen", return_value=contextlib.nullcontext(source)):
            with self.assertRaisesRegex(ValueError, "size limit"):
                transfer.fetch(source.url, path, limit=transfer.METADATA_BYTES)
        self.assertEqual(path.stat().st_size, transfer.METADATA_BYTES)
        path.write_bytes(b"x" * (transfer.METADATA_BYTES + 1))
        with self.assertRaisesRegex(ValueError, "size limit"):
            transfer.read_json(path)

    def test_many_exact_versions_keep_only_one_vendor_document_in_memory(self):
        vendor = self.carry / "vendor"
        vendor.mkdir()
        artifact = b"verified"
        digest = hashlib.sha256(artifact).hexdigest()
        builds = [{"kind": "codex", "version": f"1.2.{n}", "os": "linux", "arch": "x86_64", "sha256": digest}
                  for n in range(300)]
        def fetched(url, path, **_):
            if url.startswith("https://api.github.com/"):
                version = url.rsplit("rust-v", 1)[1]
                path.write_text(json.dumps({"padding": "x" * 65536, "assets": [{
                    "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": "sha256:" + digest,
                    "browser_download_url": "https://vendor.example.test/" + version + "/program"}]}))
            else:
                path.write_bytes(artifact)
        tracemalloc.start()
        try:
            with patch.object(transfer, "fetch", side_effect=fetched), patch("builtins.print"):
                kept = transfer.vendor_builds(builds, vendor)
            _, peak = tracemalloc.get_traced_memory()
        finally:
            tracemalloc.stop()
        self.assertEqual(len(kept), 300)
        # The 300 replies contain 19 MiB of padding; retain one reply and the bounded import list.
        self.assertLess(peak, 4 * 1024 * 1024)

    def test_a_vendor_artifact_address_cannot_write_outside_the_mirror_before_its_hash_is_checked(self):
        vendor = self.carry / "vendor"
        vendor.mkdir()
        digest = hashlib.sha256(b"vendor executable").hexdigest()
        build = {"kind": "codex", "version": "1.2.3", "os": "linux", "arch": "x86_64", "sha256": digest}
        for address in ("https://github.com/../../release/version.json", "https://github.com/../../../../outside"):
            with self.subTest(address=address):
                metadata = {"draft": False, "prerelease": False, "assets": [{
                    "name": "codex-package-x86_64-unknown-linux-musl.tar.gz", "digest": "sha256:" + digest,
                    "browser_download_url": address}]}
                with patch.object(transfer, "fetch", side_effect=lambda url, path, **_: path.write_text(json.dumps(metadata))) as fetched:
                    with self.assertRaisesRegex(ValueError, "path escapes"):
                        transfer.vendor_builds([build], vendor)
                self.assertEqual(fetched.call_count, 1, "only the vendor metadata was fetched; no artifact was opened")
        self.assertFalse((self.carry / "release/version.json").exists())

    def test_operator_build_arguments_and_other_tables_are_preserved_when_image_maps_change(self):
        original = "[node.docker]\nbuilds = true\n[node.docker.offline_images]\nold = 'sha256:old'\n[node.docker.build_args]\noffline_images = 'keep-this-argument'\n"
        changed = transfer.image_settings(original, {"chosen": self.content})
        parsed = tomllib.loads(changed)["node"]["docker"]
        self.assertTrue(parsed["builds"])
        self.assertEqual(parsed["offline_images"], {"chosen": self.content})
        self.assertEqual(parsed["build_args"]["offline_images"], "keep-this-argument")

    def test_a_malformed_session_token_is_refused_without_requesting_or_echoing_it(self):
        self.record()
        token = self.root / "session"
        token.write_text("private-session-value\ninjected-header")
        with patch.object(transfer.urllib.request, "build_opener") as request:
            with self.assertRaises(ValueError) as refused:
                transfer.import_checksums(self.args(api_url="https://orch.internal/api/v1", token_file=token), self.carry)
        self.assertNotIn("private-session-value", str(refused.exception))
        request.assert_not_called()

    def test_the_import_receipt_binds_the_exact_carried_utf8_bytes_including_line_endings(self):
        self.record()
        document = b'[{"kind":"codex","version":"1.2.3","os":"linux","arch":"x86_64","url":"https://vendor.example/build","sha256":"' + b'a' * 64 + b'","source":"release","is_default":false}]\r\n'
        (self.carry / "vendor-checksums.json").write_bytes(document)
        token = self.root / "session"
        token.write_text("oat_example")
        class Opener:
            request = None
            def open(self, request, timeout):
                self.request = request
                return io.BytesIO(json.dumps({"sha256": hashlib.sha256(document).hexdigest(), "count": 1}).encode())
        opener = Opener()
        with patch.object(transfer.urllib.request, "build_opener", return_value=opener):
            transfer.import_checksums(self.args(api_url="https://orch.internal/api/v1", token_file=token), self.carry)
        self.assertEqual(json.loads(opener.request.data)["document"].encode(), document)

    def test_gather_keeps_release_images_and_provider_base_and_only_includes_embeddings_when_asked(self):
        source = self.root / "release-source"
        source.mkdir()
        asset = source / "payload.bin"
        asset.write_bytes(b"ordinary release bytes")
        listing = {"schema_version": 1, "project": "orch", "tag": "v1.0.0", "harness_builds": [],
            "assets": [{"name": asset.name, "size": asset.stat().st_size, "blake3": "a" * 64}],
            "images": [{"reference": self.source, "digest": self.pin}]}
        (source / "version.json").write_text(json.dumps(listing))
        def saved(image, out, index, arch):
            self.assertEqual(arch,"x86_64")
            path = out / "images" / f"{index}.tar"
            path.write_bytes(image["reference"].encode())
            return {"source": image["reference"], "digest": image["digest"], "id": self.content,
                    "archive": path.relative_to(out).as_posix()}
        for optional in (None, "embedding:chosen@" + self.pin):
            out = self.root / ("with-model" if optional else "without-model")
            args = argparse.Namespace(out=out, release_dir=source, release_url=None, arch="x86_64",
                                      image=[], local_embeddings=optional, harness_builds=None)
            with patch.object(transfer, "verify_release") as verified, patch.object(transfer, "image_save", side_effect=saved), patch.object(transfer, "vendor_builds", return_value=[]):
                transfer.gather(args)
            verified.assert_called_once()
            record = transfer.verify(out)
            names = [image["source"] for image in record["images"]]
            self.assertIn(self.source, names)
            self.assertTrue(any("distroless" in name for name in names))
            self.assertEqual("embedding:chosen" in names, optional is not None)
            self.assertEqual((out / "release/payload.bin").read_bytes(), asset.read_bytes())
            self.assertTrue((out / "install.sh").is_file())


if __name__ == "__main__":
    unittest.main()
