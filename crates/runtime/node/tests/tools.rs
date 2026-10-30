//! The tool cache, against a server on loopback: a program is downloaded, verified against the hash the system named and published whole, an archive is unpacked with every path held inside its directory, a version in the cache is never replaced, and units are given the connector the system named last.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    reason = "a test fails by panicking"
)]

mod common;

use std::collections::HashMap;
use std::io::Write;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::http::{StatusCode, Uri};
use common::Rig;
use orch_link_contract::commands::{
    NodeProbe, NodeProbeResult, NodeToolEnsure, NodeToolEnsureResult, SerialKey,
};
use orch_node_core::settings::NodeSettings;
use orch_node_core::tools::ToolCache;
use sha2::{Digest, Sha256};
use tokio_util::sync::CancellationToken;

/// A server of files on loopback that counts what it is asked.
struct Server {
    base: String,
    hits: Arc<Mutex<Vec<String>>>,
    task: tokio::task::JoinHandle<()>,
}

/// Reserve one port in the worker's assigned range, allowing concurrent tests
/// and other workers to hold their own ports without coordinating in advance.
async fn listener() -> tokio::net::TcpListener {
    static NEXT: AtomicUsize = AtomicUsize::new(0);
    const FIRST: usize = 41000;
    const COUNT: usize = 1000;
    let first = (std::process::id() as usize + NEXT.fetch_add(1, Ordering::Relaxed)) % COUNT;
    for offset in 0..COUNT {
        let port = FIRST + (first + offset) % COUNT;
        if let Ok(listener) = tokio::net::TcpListener::bind(("127.0.0.1", port as u16)).await {
            return listener;
        }
    }
    panic!("no test port is available in 41000–41999")
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    // Sixteen delayed chunks span several reporting intervals without a large fixture or a real network.
    async fn shaped(bytes: Vec<u8>) -> Self {
        const CHUNK: usize = 256 * 1024;
        let files = bytes::Bytes::from(bytes);
        let hits = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&hits);
        let app = Router::new().fallback(move |uri: Uri| {
            let (files, seen) = (files.clone(), Arc::clone(&seen));
            async move {
                seen.lock().unwrap().push(uri.path().to_owned());
                let length = files.len();
                let body = axum::body::Body::from_stream(futures::stream::unfold(
                    (files, 0),
                    |(files, offset)| async move {
                        if offset >= files.len() {
                            return None;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
                        let end = (offset + CHUNK).min(files.len());
                        let piece = files.slice(offset..end);
                        Some((Ok::<_, std::io::Error>(piece), (files, end)))
                    },
                ));
                axum::http::Response::builder()
                    .header("content-length", length.to_string())
                    .body(body)
                    .unwrap()
            }
        });
        let listener = listener().await;
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { base, hits, task }
    }

    async fn with(files: Vec<(&str, Vec<u8>)>) -> Self {
        let files: Arc<HashMap<String, bytes::Bytes>> = Arc::new(
            files
                .into_iter()
                .map(|(path, bytes)| (path.to_owned(), bytes::Bytes::from(bytes)))
                .collect(),
        );
        let hits = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&hits);
        let app = Router::new().fallback(move |uri: Uri| {
            let (files, seen) = (Arc::clone(&files), Arc::clone(&seen));
            async move {
                seen.lock().unwrap().push(uri.path().to_owned());
                files
                    .get(uri.path())
                    .map_or((StatusCode::NOT_FOUND, bytes::Bytes::new()), |bytes| {
                        (StatusCode::OK, bytes.clone())
                    })
            }
        });
        let listener = listener().await;
        let base = format!("http://{}", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        Self { base, hits, task }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    fn hits(&self) -> usize {
        self.hits.lock().unwrap().len()
    }
}

fn sha256(bytes: &[u8]) -> String {
    hex::encode(Sha256::digest(bytes))
}

fn ask(name: &str, version: &str, url: String, bytes: &[u8]) -> NodeToolEnsure {
    NodeToolEnsure {
        job_id: None,
        name: name.to_owned(),
        version: version.to_owned(),
        url,
        sha256: sha256(bytes),
    }
}

fn cache() -> (ToolCache, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    (
        ToolCache::open(&NodeSettings::in_dir(dir.path())).unwrap(),
        dir,
    )
}

fn tar_gz(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    let mut archive = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::fast(),
    ));
    for (path, content, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(content.len() as u64);
        header.set_mode(*mode);
        archive.append_data(&mut header, path, *content).unwrap();
    }
    archive.into_inner().unwrap().finish().unwrap()
}

fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, content) in entries {
        writer
            .start_file(*path, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(content).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

/// A tar with an entry whose path climbs out of the directory it is unpacked to, which the tar library will
/// not write and an attacker can.
fn tar_that_climbs() -> Vec<u8> {
    let mut archive = tar::Builder::new(Vec::new());
    let mut header = tar::Header::new_gnu();
    header.set_size(4);
    header.set_mode(0o644);
    let name = b"../escaped.txt";
    header.as_old_mut().name[..name.len()].copy_from_slice(name);
    header.set_cksum();
    archive.append(&header, &b"evil"[..]).unwrap();
    archive.into_inner().unwrap()
}

#[tokio::test]
async fn a_program_is_downloaded_verified_and_published_whole_and_executable() {
    let program = b"#!/bin/sh\necho hello\n".to_vec();
    let server = Server::with(vec![("/codex", program.clone())]).await;
    let (cache, _dir) = cache();
    let path = cache
        .ensure(
            &ask("codex", "0.160.1", server.url("/codex"), &program),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert!(path.ends_with("tools/codex/0.160.1"));
    let file = path.join("codex");
    assert_eq!(std::fs::read(&file).unwrap(), program);
    #[cfg(unix)]
    assert_ne!(
        std::fs::metadata(&file).unwrap().permissions().mode() & 0o111,
        0,
        "it can be run"
    );
    assert!(cache.has("codex", "0.160.1"));
    assert_eq!(cache.list(), [("codex".to_owned(), "0.160.1".to_owned())]);
    assert!(
        cache.current_connector().is_none(),
        "only the connector's program is the one units are given"
    );
    // Nothing is left in the scratch directory.
    let partial = cache.dir().join(".partial");
    assert!(std::fs::read_dir(partial).unwrap().next().is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn newly_published_cache_paths_cannot_be_written_by_other_users() {
    let program = b"#!/bin/sh\necho hello\n".to_vec();
    let server = Server::with(vec![("/codex", program.clone())]).await;
    let (cache, _dir) = cache();
    let path = cache
        .ensure(
            &ask("codex", "1", server.url("/codex"), &program),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    for path in [
        cache.dir().to_owned(),
        cache.dir().join("codex"),
        path.clone(),
        path.join(".complete"),
        path.join(".version"),
        path.join(".orch-bytes"),
        path.join("codex"),
    ] {
        let mode = std::fs::symlink_metadata(&path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o6022, 0, "{} has mode {mode:o}", path.display());
    }
    let scratch = std::fs::symlink_metadata(cache.dir().join(".partial")).unwrap();
    assert_eq!(scratch.permissions().mode() & 0o7777, 0o700);
}

#[tokio::test]
async fn an_existing_cache_entry_is_not_relabelled_or_replaced() {
    let program = b"#!/bin/sh\necho old\n".to_vec();
    let server = Server::with(vec![("/codex", program.clone())]).await;
    let (cache, _dir) = cache();
    let old = cache.dir().join("codex/1");
    std::fs::create_dir_all(&old).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o775)).unwrap();
    std::fs::write(old.join("codex"), &program).unwrap();
    std::fs::write(old.join(".complete"), format!("{}\n", sha256(&program))).unwrap();
    let result = cache
        .ensure(
            &ask("codex", "1", server.url("/codex"), &program),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(result, old);
    #[cfg(unix)]
    assert_eq!(
        std::fs::metadata(&old).unwrap().permissions().mode() & 0o777,
        0o775
    );
    assert_eq!(std::fs::read(old.join("codex")).unwrap(), program);
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn archive_entries_cannot_supply_or_redirect_orch_markers() {
    for marker in [".complete", ".version"] {
        let archive = tar_gz(&[(marker, b"untrusted", 0o644)]);
        let server = Server::with(vec![("/tool.tar.gz", archive.clone())]).await;
        let (cache, _dir) = cache();
        let error = cache
            .ensure(
                &ask("tool", "1", server.url("/tool.tar.gz"), &archive),
                &CancellationToken::new(),
            )
            .await
            .unwrap_err();
        assert!(
            error.to_string().contains("creating a tool marker"),
            "{error}"
        );
        assert!(!cache.has("tool", "1"));
    }

    let mut archive = tar::Builder::new(Vec::new());
    let mut payload = tar::Header::new_gnu();
    payload.set_size(7);
    payload.set_mode(0o644);
    archive
        .append_data(&mut payload, "payload", &b"payload"[..])
        .unwrap();
    let mut header = tar::Header::new_gnu();
    header.set_entry_type(tar::EntryType::Symlink);
    header.set_size(0);
    header.set_mode(0o777);
    header.set_link_name("payload").unwrap();
    archive
        .append_data(&mut header, ".complete", &b""[..])
        .unwrap();
    let archive = archive.into_inner().unwrap();
    let server = Server::with(vec![("/tool.tar", archive.clone())]).await;
    let (cache, _dir) = cache();
    let error = cache
        .ensure(
            &ask("tool", "1", server.url("/tool.tar"), &archive),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(
        error.to_string().contains("creating a tool marker"),
        "{error}"
    );
    assert!(!cache.has("tool", "1"));
}

#[tokio::test]
async fn a_tool_in_the_cache_says_which_version_it_is_in_its_own_directory() {
    // A unit sees the directory of a tool by the tool's name alone, so what the connector there learns of the
    // version is what the directory says; a program and an archive are alike in that.
    let program = b"#!/bin/sh\n".to_vec();
    let archive = tar_gz(&[("bin/codex", b"#!/bin/sh\n", 0o755)]);
    let server = Server::with(vec![
        ("/claude", program.clone()),
        ("/codex.tar.gz", archive.clone()),
    ])
    .await;
    let (cache, _dir) = cache();
    let cancel = CancellationToken::new();
    let one = cache
        .ensure(
            &ask("claude-code", "2.1.296", server.url("/claude"), &program),
            &cancel,
        )
        .await
        .unwrap();
    let two = cache
        .ensure(
            &ask("codex", "0.162.1", server.url("/codex.tar.gz"), &archive),
            &cancel,
        )
        .await
        .unwrap();
    let says = |directory: &std::path::Path| {
        std::fs::read_to_string(directory.join(orch_connector_core::builds::VERSION_MARKER))
            .unwrap()
    };
    assert_eq!(says(&one), "2.1.296\n");
    assert_eq!(says(&two), "0.162.1\n");
}

#[tokio::test]
async fn a_tool_that_is_in_the_cache_is_not_downloaded_again() {
    let program = b"program".to_vec();
    let server = Server::with(vec![("/tool", program.clone())]).await;
    let (cache, _dir) = cache();
    let request = ask("tool", "1", server.url("/tool"), &program);
    let cancel = CancellationToken::new();
    let mut config = orch_config::ConnectorConfig::default();
    config.policy.delegated = vec!["*".into()];
    let policy =
        Arc::new(orch_connector_core::policy::PolicyStore::connected(config, None).unwrap());
    let cache = cache.with_policy(Arc::clone(&policy));
    let first = cache.ensure(&request, &cancel).await.unwrap();
    policy
        .set_here(
            orch_link_contract::policy::PolicyKey::ToolsFetch,
            &serde_json::json!(false),
        )
        .unwrap();
    let again = cache.ensure(&request, &cancel).await.unwrap();
    assert_eq!(first, again);
    assert_eq!(server.hits(), 1);
    let refused = cache
        .ensure(&ask("tool", "2", server.url("/tool"), &program), &cancel)
        .await
        .unwrap_err();
    assert!(
        refused.to_string().contains("tools.fetch") && refused.to_string().contains("operator")
    );
    assert_eq!(
        server.hits(),
        1,
        "a kept fetch refusal permits the verified cache but opens no download"
    );
}

#[tokio::test]
async fn what_arrives_with_another_hash_than_the_system_named_is_thrown_away() {
    let server = Server::with(vec![("/tool", b"something else".to_vec())]).await;
    let (cache, _dir) = cache();
    let error = cache
        .ensure(
            &ask(
                "tool",
                "1",
                server.url("/tool"),
                b"what the system expected",
            ),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let command: orch_link_contract::commands::CommandError = error.into();
    assert_eq!(command.code, "internal");
    assert!(
        command.retryable,
        "a transfer that was cut may arrive whole the next time"
    );
    assert!(command.detail.contains("hash"), "{}", command.detail);
    assert!(!cache.has("tool", "1"));
    assert!(!cache.path_of("tool", "1").exists());
}

#[tokio::test]
async fn a_version_in_the_cache_is_never_replaced_not_even_by_a_build_with_another_hash() {
    let (one, other) = (b"one".to_vec(), b"another build".to_vec());
    let server = Server::with(vec![("/one", one.clone()), ("/other", other.clone())]).await;
    let (cache, _dir) = cache();
    let cancel = CancellationToken::new();
    cache
        .ensure(&ask("tool", "1", server.url("/one"), &one), &cancel)
        .await
        .unwrap();
    let error = cache
        .ensure(&ask("tool", "1", server.url("/other"), &other), &cancel)
        .await
        .unwrap_err();
    let command: orch_link_contract::commands::CommandError = error.into();
    assert_eq!(command.code, "conflict");
    assert_eq!(
        std::fs::read(cache.path_of("tool", "1").join("tool")).unwrap(),
        one,
        "units stand on it"
    );
}

#[tokio::test]
async fn a_download_that_the_server_refuses_fails_without_leaving_anything_in_the_cache() {
    let server = Server::with(vec![]).await;
    let (cache, _dir) = cache();
    let error = cache
        .ensure(
            &ask("tool", "1", server.url("/missing"), b"x"),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("404"), "{error}");
    assert!(cache.list().is_empty());
}

#[tokio::test]
async fn an_archive_is_unpacked_with_its_files_and_their_modes() {
    let archive = tar_gz(&[
        ("bin/codex", b"#!/bin/sh\n", 0o755),
        ("lib/data.txt", b"data", 0o644),
    ]);
    let server = Server::with(vec![("/codex.tar.gz", archive.clone())]).await;
    let (cache, _dir) = cache();
    let path = cache
        .ensure(
            &ask("codex", "2", server.url("/codex.tar.gz"), &archive),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(path.join("lib/data.txt")).unwrap(), b"data");
    #[cfg(unix)]
    assert_ne!(
        std::fs::metadata(path.join("bin/codex"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );

    let zipped = zip_of(&[("bin/tool", b"binary"), ("README", b"read me")]);
    let server = Server::with(vec![("/tool.zip", zipped.clone())]).await;
    let path = cache
        .ensure(
            &ask("tool", "3", server.url("/tool.zip"), &zipped),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    assert_eq!(std::fs::read(path.join("README")).unwrap(), b"read me");
}

#[cfg(unix)]
#[tokio::test]
async fn nothing_in_an_unpacked_archive_can_change_who_runs_it_or_be_written_by_anyone_else() {
    let archive = tar_gz(&[
        ("bin/setuid-tool", b"#!/bin/sh\n", 0o4755),
        ("share/writable.txt", b"data", 0o666),
    ]);
    let server = Server::with(vec![("/tool.tar.gz", archive.clone())]).await;
    let (cache, _dir) = cache();
    let path = cache
        .ensure(
            &ask("tool", "1", server.url("/tool.tar.gz"), &archive),
            &CancellationToken::new(),
        )
        .await
        .unwrap();
    let mode = |relative: &str| {
        std::fs::metadata(path.join(relative))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777
    };
    assert_eq!(mode("bin/setuid-tool"), 0o755);
    assert_eq!(mode("share/writable.txt"), 0o644);
    assert_eq!(mode("bin"), 0o755);
}

#[tokio::test]
async fn an_archive_with_a_path_that_leaves_the_directory_is_refused_and_writes_nothing_outside_it()
{
    let evil = tar_that_climbs();
    let server = Server::with(vec![("/evil.tar", evil.clone())]).await;
    let (cache, dir) = cache();
    let error = cache
        .ensure(
            &ask("evil", "1", server.url("/evil.tar"), &evil),
            &CancellationToken::new(),
        )
        .await
        .unwrap_err();
    let command: orch_link_contract::commands::CommandError = error.into();
    assert_eq!(command.code, "invalid");
    assert!(command.detail.contains("leaves"), "{}", command.detail);
    assert!(!cache.has("evil", "1"));
    assert!(
        !dir.path().join("tools/escaped.txt").exists() && !dir.path().join("escaped.txt").exists()
    );
}

#[tokio::test]
async fn a_name_a_hash_or_an_address_that_cannot_be_a_tool_is_refused_before_anything_is_downloaded()
 {
    let server = Server::with(vec![("/x", b"x".to_vec())]).await;
    let (cache, _dir) = cache();
    let cancel = CancellationToken::new();
    for (name, version, url, hash) in [
        ("../etc", "1", server.url("/x"), sha256(b"x")),
        ("tool", "..", server.url("/x"), sha256(b"x")),
        ("tool", "1", server.url("/x"), "not a hash".to_owned()),
        ("tool", "1", "file:///etc/passwd".to_owned(), sha256(b"x")),
        ("tool", "1", "ftp://example.org/x".to_owned(), sha256(b"x")),
    ] {
        let request = NodeToolEnsure {
            job_id: None,
            name: name.to_owned(),
            version: version.to_owned(),
            url,
            sha256: hash,
        };
        let error: orch_link_contract::commands::CommandError =
            cache.ensure(&request, &cancel).await.unwrap_err().into();
        assert_eq!(error.code, "invalid", "{name} {version}");
    }
    assert_eq!(server.hits(), 0);
}

#[tokio::test]
async fn the_connectors_program_has_to_be_at_the_top_and_units_are_given_the_version_named_last() {
    let (first, second) = (
        b"#!/bin/sh\necho 1\n".to_vec(),
        b"#!/bin/sh\necho 2\n".to_vec(),
    );
    let nested = tar_gz(&[("bin/orch-connector", b"#!/bin/sh\n", 0o755)]);
    let server = Server::with(vec![
        ("/v1/orch-connector", first.clone()),
        ("/v2/orch-connector", second.clone()),
        ("/nested.tar.gz", nested.clone()),
    ])
    .await;
    let (cache, _dir) = cache();
    let cancel = CancellationToken::new();

    let error: orch_link_contract::commands::CommandError = cache
        .ensure(
            &ask(
                "orch-connector",
                "0.0.9",
                server.url("/nested.tar.gz"),
                &nested,
            ),
            &cancel,
        )
        .await
        .unwrap_err()
        .into();
    assert!(error.detail.contains("top"), "{}", error.detail);
    assert!(cache.current_connector().is_none());

    cache
        .ensure(
            &ask(
                "orch-connector",
                "0.1.0",
                server.url("/v1/orch-connector"),
                &first,
            ),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(cache.current_connector().as_deref(), Some("0.1.0"));
    cache
        .ensure(
            &ask(
                "orch-connector",
                "0.2.0",
                server.url("/v2/orch-connector"),
                &second,
            ),
            &cancel,
        )
        .await
        .unwrap();
    assert_eq!(
        cache.current_connector().as_deref(),
        Some("0.2.0"),
        "new units get the version named last"
    );
    // A unit made before still stands on the build it was made with.
    assert_eq!(
        std::fs::read(
            cache
                .path_of("orch-connector", "0.1.0")
                .join("orch-connector")
        )
        .unwrap(),
        first
    );
    // What a driver mounts is what the link points at now.
    let through_link = std::fs::canonicalize(cache.dir().join("orch-connector/current")).unwrap();
    assert!(through_link.ends_with("orch-connector/0.2.0"));
}

#[tokio::test]
async fn the_scratch_of_a_download_that_was_cut_off_is_gone_when_the_node_starts_again() {
    let dir = tempfile::tempdir().unwrap();
    let settings = NodeSettings::in_dir(dir.path());
    let leftover = settings.tools_dir().join(".partial/codex-1-abc");
    std::fs::create_dir_all(&leftover).unwrap();
    std::fs::write(leftover.join("download"), "half").unwrap();
    // A directory of the cache without its marker is not a tool.
    let unmarked = settings.tools_dir().join("codex/2");
    std::fs::create_dir_all(&unmarked).unwrap();
    std::fs::write(unmarked.join("codex"), "half").unwrap();

    let cache = ToolCache::open(&settings).unwrap();
    assert!(!leftover.exists());
    assert!(!cache.has("codex", "2"));
    assert!(cache.list().is_empty());
}

#[tokio::test]
async fn the_command_answers_where_the_tool_is_and_a_probe_lists_what_the_cache_holds() {
    let program = b"#!/bin/sh\n".to_vec();
    let server = Server::with(vec![("/orch-connector", program.clone())]).await;
    let mut rig = Rig::new();
    rig.start().await;
    let result: NodeToolEnsureResult = serde_json::from_value(
        rig.succeed(
            &SerialKey::Tool("orch-connector".into()),
            &ask(
                "orch-connector",
                "0.1.0",
                server.url("/orch-connector"),
                &program,
            ),
        )
        .await,
    )
    .unwrap();
    assert!(
        result.path.ends_with("tools/orch-connector/0.1.0"),
        "{}",
        result.path
    );
    let probe: NodeProbeResult =
        serde_json::from_value(rig.succeed(&SerialKey::Machine, &NodeProbe {}).await).unwrap();
    assert_eq!(probe.tools.len(), 1);
    assert_eq!(probe.tools[0].name, "orch-connector");

    let error = rig
        .refused(
            &SerialKey::Tool("orch-connector".into()),
            &ask(
                "orch-connector",
                "0.1.0",
                server.url("/orch-connector"),
                b"another hash",
            ),
        )
        .await;
    assert_eq!(error.code, "conflict");
    rig.stop().await;
}

#[tokio::test]
async fn a_shaped_tool_download_shows_increasing_bytes_and_its_verified_hash_to_the_job() {
    let bytes = vec![b'x'; 4 * 1024 * 1024];
    let server = Server::shaped(bytes.clone()).await;
    let mut rig = Rig::new();
    rig.start().await;
    let job = orch_types::new_id();
    let mut request = ask("tool", "1", server.url("/tool"), &bytes);
    request.job_id = Some(job);
    rig.succeed(&SerialKey::Tool("tool".into()), &request).await;
    rig.wait_told("verified tool progress", |events| {
        events.iter().any(|event| {
            event.kind == "job.progress"
                && event.body["job_id"] == serde_json::json!(job)
                && event.body["progress"]["percent"] == 100
        })
    })
    .await;
    let told: Vec<_> = rig
        .applied()
        .into_iter()
        .filter(|event| {
            event.kind == "job.progress" && event.body["job_id"] == serde_json::json!(job)
        })
        .collect();
    let counts: Vec<_> = told
        .iter()
        .filter_map(|event| event.body["progress"]["bytes_done"].as_i64())
        .collect();
    assert!(
        counts.len() >= 3,
        "an initial, intermediate and verified report reach the person"
    );
    assert!(counts.windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(
        counts
            .windows(2)
            .any(|pair| pair[0] > 0 && pair[1] > pair[0])
    );
    let last = &told.last().unwrap().body["progress"];
    assert_eq!(last["bytes_done"], serde_json::json!(bytes.len()));
    assert_eq!(last["bytes_total"], serde_json::json!(bytes.len()));
    assert!(last["message"].as_str().unwrap().contains(&sha256(&bytes)));
    rig.stop().await;
}

#[tokio::test]
async fn cancelling_a_download_installs_nothing_and_retry_publishes_only_the_verified_program() {
    let bytes = vec![b'x'; 4 * 1024 * 1024];
    let server = Server::shaped(bytes.clone()).await;
    let (cache, _dir) = cache();
    let request = ask("tool", "1", server.url("/tool"), &bytes);
    let cancel = CancellationToken::new();
    let download = cache.ensure(&request, &cancel);
    let cancellation = async {
        tokio::time::timeout(std::time::Duration::from_secs(10), async {
            while server.hits() == 0 {
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        cancel.cancel();
    };
    let (result, ()) = tokio::join!(download, cancellation);
    assert!(matches!(
        result,
        Err(orch_node_core::tools::ToolError::Cancelled)
    ));
    assert!(!cache.has("tool", "1"));
    assert!(!cache.path_of("tool", "1").exists());
    let path = cache
        .ensure(&request, &CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(std::fs::read(path.join("tool")).unwrap(), bytes);
    assert_eq!(
        std::fs::read_to_string(path.join(orch_node_core::tools::COMPLETE_MARKER))
            .unwrap()
            .trim(),
        request.sha256
    );
}

#[cfg(unix)]
#[tokio::test]
async fn a_linked_cache_root_is_unknown_and_never_cleared_or_purged_as_owned_data() {
    let node = tempfile::tempdir().unwrap();
    let borrowed = tempfile::tempdir().unwrap();
    let settings = NodeSettings::in_dir(node.path());
    let version = borrowed.path().join("sample/1.0");
    std::fs::create_dir_all(&version).unwrap();
    std::fs::write(version.join(".complete"), b"a fixture marker").unwrap();
    std::fs::write(version.join("keep"), b"borrowed bytes").unwrap();
    std::fs::create_dir(borrowed.path().join(".partial")).unwrap();
    std::fs::write(borrowed.path().join(".partial/keep"), b"borrowed partial").unwrap();
    std::os::unix::fs::symlink(borrowed.path(), settings.tools_dir()).unwrap();
    assert!(
        ToolCache::open(&settings).is_err(),
        "startup never clears downloads through a substituted cache root"
    );
    let cache = ToolCache::inspect(&settings).unwrap();
    assert!(cache.list().is_empty());
    assert!(cache.remove_cached("sample", "1.0").await.is_err());
    let drivers = orch_node_core::drivers::DriverHost::of(
        &settings,
        Arc::new(orch_node_core::images::BlobDesk::new()),
        Vec::new(),
    );
    let ledger = orch_node_core::ledger::Ledger::open(node.path()).unwrap();
    let inventory =
        orch_node_core::resources::snapshot(&drivers, &cache, &ledger, &orch_types::SystemClock)
            .await;
    assert!(
        inventory
            .unavailable
            .iter()
            .any(|source| source.driver == "tools")
    );
    assert!(inventory.resources.is_empty());
    assert_eq!(
        std::fs::read(version.join("keep")).unwrap(),
        b"borrowed bytes"
    );
    assert_eq!(
        std::fs::read(borrowed.path().join(".partial/keep")).unwrap(),
        b"borrowed partial"
    );
}
