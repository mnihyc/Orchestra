//! The files of a release that a machine with nothing on it fetches: its connector, the installers and the manifest.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod support;

use orch_api_contract::common::ErrorCode;
use orch_api_contract::downloads::{
    BLAKE3_HEADER, DownloadConnectorGet, DownloadInstallNodeShGet, DownloadInstallPs1Get,
    DownloadInstallShGet, DownloadNodeGet, DownloadVersionGet, VERSION_HEADER,
};
use orch_api_core::testing::RawReply;
use orch_config::ServerConfig;
use orch_types::Hash;
use support::World;

/// A release whose payload holds `files`, in the directory beside the migrations that the
/// configuration names.
fn release(files: &[(&str, &[u8])]) -> (tempfile::TempDir, ServerConfig) {
    let directory = tempfile::tempdir().unwrap();
    let payload = directory.path().join("connectors");
    std::fs::create_dir(&payload).unwrap();
    for (name, bytes) in files {
        std::fs::write(payload.join(name), bytes).unwrap();
    }
    let mut config = ServerConfig::default();
    config.database.migrations_dir = directory
        .path()
        .join("migrations")
        .to_str()
        .unwrap()
        .to_owned();
    (directory, config)
}

async fn connector(world: &World, os: &str, arch: &str) -> RawReply {
    world
        .app
        .call(DownloadConnectorGet, &[&os, &arch])
        .send_bytes()
        .await
}

fn header<'a>(reply: &'a RawReply, name: &str) -> &'a str {
    reply.headers.get(name).unwrap().to_str().unwrap()
}

fn blake3_of(bytes: &[u8]) -> String {
    let mut hasher = Hash::hasher();
    hasher.update(bytes);
    Hash::finish(&hasher).to_string()
}

/// Bytes of a size that takes several reads of the file, none of them alike.
fn binary() -> Vec<u8> {
    (0..300_000_u32)
        .map(|index| u8::try_from((index * 31 + index / 251) % 256).unwrap())
        .collect()
}

#[tokio::test]
async fn a_connector_is_served_as_it_is_to_anybody_with_its_release_and_the_hash_of_the_bytes_that_follow()
 {
    let bytes = binary();
    let manifest =
        br#"{"version":"1.4.0","assets":[{"name":"orch-connector-linux-x86_64","blake3":"0000"}]}"#;
    let (_directory, config) = release(&[
        ("orch-connector-linux-x86_64", &bytes),
        ("version.json", manifest),
    ]);
    let world = World::with(config).await;

    let reply = connector(&world, "linux", "x86_64").await;

    assert_eq!(reply.status, 200, "no sign-in is needed");
    assert_eq!(reply.raw.as_ref(), bytes.as_slice(), "the very file");
    assert_eq!(header(&reply, "content-type"), "application/octet-stream");
    assert_eq!(header(&reply, "content-length"), bytes.len().to_string());
    assert_eq!(
        header(&reply, VERSION_HEADER),
        "1.4.0",
        "the release the payload says it is"
    );
    assert_eq!(
        header(&reply, BLAKE3_HEADER),
        blake3_of(&bytes),
        "worked out from the bytes, not taken from the manifest"
    );
    assert!(header(&reply, "content-disposition").contains("orch-connector-linux-x86_64"));
}

#[tokio::test]
async fn a_hosts_node_is_served_for_its_processor_with_its_release_and_its_hash() {
    let bytes = binary();
    let (_directory, config) = release(&[
        ("orch-node-linux-aarch64-musl", &bytes),
        ("version.json", br#"{"version":"1.4.0"}"#),
    ]);
    let world = World::with(config).await;

    let reply = world
        .app
        .call(DownloadNodeGet, &[&"aarch64"])
        .send_bytes()
        .await;
    assert_eq!(reply.status, 200, "no sign-in is needed");
    assert_eq!(reply.raw.as_ref(), bytes.as_slice());
    assert_eq!(header(&reply, VERSION_HEADER), "1.4.0");
    assert_eq!(header(&reply, BLAKE3_HEADER), blake3_of(&bytes));
    assert!(header(&reply, "content-disposition").contains("orch-node-linux-aarch64-musl"));

    let missing = world
        .app
        .call(DownloadNodeGet, &[&"x86_64"])
        .send_bytes()
        .await
        .problem();
    assert_eq!(missing.status, 404);
    // The sentence is the catalogue's for the code: the request itself names what is not there.
    assert_eq!(
        missing.detail,
        orch_api_contract::messages::for_code(ErrorCode::NotFound)
    );
    let unknown = world
        .app
        .call(DownloadNodeGet, &[&"riscv64"])
        .send_bytes()
        .await
        .problem();
    assert_eq!(unknown.status, 404);
}

#[tokio::test]
async fn a_release_without_a_manifest_names_the_release_that_is_running() {
    let (_directory, config) = release(&[("orch-connector-linux-x86_64", b"elf")]);
    let world = World::with(config).await;

    let reply = connector(&world, "linux", "x86_64").await;

    assert_eq!(reply.status, 200);
    assert_eq!(header(&reply, VERSION_HEADER), "0.0.0-test");
}

#[tokio::test]
async fn a_build_for_musl_or_for_windows_is_found_under_the_name_the_release_gives_it() {
    let (_directory, config) = release(&[
        ("orch-connector-linux-aarch64-musl", b"static"),
        ("orch-connector-windows-x86_64.exe", b"portable executable"),
    ]);
    let world = World::with(config).await;

    let linux = connector(&world, "linux", "aarch64").await;
    let windows = connector(&world, "windows", "x86_64").await;
    let windows_arm64 = connector(&world, "windows", "aarch64").await;

    assert_eq!(linux.raw.as_ref(), b"static");
    assert!(header(&linux, "content-disposition").contains("orch-connector-linux-aarch64-musl"));
    assert_eq!(windows.raw.as_ref(), b"portable executable");
    assert!(header(&windows, "content-disposition").contains("orch-connector-windows-x86_64.exe"));
    assert_eq!(
        windows_arm64.status, 404,
        "an x86-64 build is not a native ARM64 download"
    );
}

#[tokio::test]
async fn the_installers_and_the_manifest_are_served_as_they_are_to_anybody() {
    let (_directory, config) = release(&[
        ("install.sh", b"#!/bin/sh\necho installing\n"),
        ("install.ps1", b"Write-Output 'installing'\r\n"),
        ("install-node.sh", b"#!/bin/sh\necho installing the node\n"),
        ("version.json", br#"{"version":"1.4.0"}"#),
    ]);
    let world = World::with(config).await;

    let sh = world.app.call(DownloadInstallShGet, &[]).send_bytes().await;
    let node = world
        .app
        .call(DownloadInstallNodeShGet, &[])
        .send_bytes()
        .await;
    let ps1 = world
        .app
        .call(DownloadInstallPs1Get, &[])
        .send_bytes()
        .await;
    let version = world.app.call(DownloadVersionGet, &[]).send_bytes().await;

    assert_eq!(sh.raw.as_ref(), b"#!/bin/sh\necho installing\n");
    assert_eq!(header(&sh, "content-type"), "text/plain; charset=utf-8");
    assert_eq!(ps1.raw.as_ref(), b"Write-Output 'installing'\r\n");
    assert_eq!(header(&ps1, "content-type"), "text/plain; charset=utf-8");
    assert_eq!(node.raw.as_ref(), b"#!/bin/sh\necho installing the node\n");
    assert_eq!(header(&node, "content-type"), "text/plain; charset=utf-8");
    assert_eq!(version.raw.as_ref(), br#"{"version":"1.4.0"}"#);
    assert_eq!(header(&version, "content-type"), "application/json");
    for reply in [&sh, &ps1, &node, &version] {
        assert_eq!(reply.status, 200);
        assert_eq!(
            header(reply, "cache-control"),
            "no-cache",
            "a release is replaced at an address"
        );
    }
}

#[tokio::test]
async fn a_system_that_was_not_started_from_a_release_has_nothing_to_download() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = ServerConfig::default();
    config.database.migrations_dir = directory
        .path()
        .join("migrations")
        .to_str()
        .unwrap()
        .to_owned();
    let world = World::with(config).await;

    let replies = [
        connector(&world, "linux", "x86_64").await,
        world.app.call(DownloadInstallShGet, &[]).send_bytes().await,
        world
            .app
            .call(DownloadInstallPs1Get, &[])
            .send_bytes()
            .await,
        world
            .app
            .call(DownloadInstallNodeShGet, &[])
            .send_bytes()
            .await,
        world.app.call(DownloadVersionGet, &[]).send_bytes().await,
    ];

    for reply in &replies {
        let problem = reply.problem();
        assert_eq!(problem.status, 404);
        assert_eq!(problem.code, ErrorCode::NotFound);
        // The sentence is the catalogue's for the code, the same for every file of the release.
        assert_eq!(
            problem.detail,
            orch_api_contract::messages::for_code(ErrorCode::NotFound)
        );
    }
}

#[tokio::test]
async fn a_release_that_lacks_a_file_answers_that_the_file_is_not_there() {
    let (_directory, config) = release(&[("orch-connector-linux-x86_64", b"elf")]);
    let world = World::with(config).await;

    let installer = world
        .app
        .call(DownloadInstallPs1Get, &[])
        .send_bytes()
        .await;
    let connector_missing = connector(&world, "macos", "aarch64").await;

    // Each answers that the file the request names is not there, in the catalogue's sentence.
    let installer = installer.problem();
    assert_eq!(
        (installer.status, installer.code),
        (404, ErrorCode::NotFound)
    );
    assert_eq!(
        installer.detail,
        orch_api_contract::messages::for_code(ErrorCode::NotFound)
    );
    let missing = connector_missing.problem();
    assert_eq!((missing.status, missing.code), (404, ErrorCode::NotFound));
    assert_eq!(
        missing.detail,
        orch_api_contract::messages::for_code(ErrorCode::NotFound)
    );
}

#[tokio::test]
async fn only_the_systems_and_processors_there_are_builds_for_are_asked_after() {
    let (_directory, config) = release(&[("orch-connector-linux-x86_64", b"elf")]);
    let world = World::with(config).await;

    let unknown_system = connector(&world, "plan9", "x86_64").await;
    let unknown_processor = connector(&world, "linux", "riscv64").await;
    let climbing = connector(&world, "..%2F..%2Fetc", "x86_64").await;

    for reply in [&unknown_system, &unknown_processor] {
        assert_eq!(reply.status, 404);
        assert_eq!(reply.problem().code, ErrorCode::NotFound);
    }
    assert_eq!(
        climbing.status, 404,
        "a name cannot climb out of the directory"
    );
}
