//! Stream a verified node tool cache into a bounded, digest-checked staging archive.
//!
//! The cache is a private, node-owned tree. This code refuses untrusted ownership,
//! escaping links and special files, but it does not make concurrent modification by
//! the node's own operator safe. A failed or expired transfer must be discarded.

#![cfg(target_os = "linux")]

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, Metadata};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Instant;

use orch_driver_contract::driver::DriverError;
use orch_driver_contract::runtime::ToolRef;
use sha2::{Digest, Sha256};

use crate::stage_protocol::ArchiveReceipt;

/// These are the node tool cache's own limits for one verified unpacked artifact.
const MAX_ARTIFACT_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const MAX_ARTIFACT_ENTRIES: usize = 200_000;
/// Each tar entry can carry Linux's longest path and link target with their headers and padding.
const MAX_ENTRY_OVERHEAD: u64 = 16 * 1024;
const CHECKSUM_HEX_DIGITS: usize = 64;
const MARKER: &str = ".complete";
const CONNECTOR_NAME: &str = "orch-connector";
const SUPERVISOR_NAME: &str = "orch-kube-supervisor";

fn unavailable() -> DriverError {
    DriverError::Unavailable("the verified tool cache could not be staged".into())
}

fn invalid() -> DriverError {
    DriverError::InvalidSpec("the selected tool cache cannot be staged safely".into())
}

/// An entry of the node's tool cache that other users may change is refused, never repaired: it may already have been
/// changed. The node's operator is the one who can fix it, so the refusal says which entry and what to do.
fn exposed(root: &Path, relative: &Path) -> DriverError {
    let name = |path: Option<&Path>| {
        path.and_then(Path::file_name)
            .map_or_else(String::new, |name| name.to_string_lossy().into_owned())
    };
    let entry = format!("{}/{}", name(root.parent()), name(Some(root)));
    DriverError::InvalidSpec(format!(
        "the tool cache entry {entry} cannot be staged: {relative:?} can be changed by other users; \
         remove the entry from the node's tools directory so that the node fetches it again"
    ))
}

/// The same for a directory of the cache that is not an artifact: the cache's own directory or a tool's.
fn exposed_directory(path: &Path) -> DriverError {
    let name = path
        .file_name()
        .map_or_else(String::new, |name| name.to_string_lossy().into_owned());
    DriverError::InvalidSpec(format!(
        "the tool cache directory {name:?} cannot be staged from: other users can change it; \
         remove it from the node's data directory so that the node makes it again"
    ))
}

fn io_error(error: io::Error) -> DriverError {
    if error.kind() == io::ErrorKind::TimedOut {
        DriverError::Timeout
    } else {
        unavailable()
    }
}

fn check_deadline(deadline: Instant) -> Result<(), DriverError> {
    if Instant::now() >= deadline {
        Err(DriverError::Timeout)
    } else {
        Ok(())
    }
}

/// Matches the node tool cache's names and version bounds.
fn plain_name(text: &str) -> bool {
    const MAX_NAME_BYTES: usize = 128;
    !text.contains("..")
        && text.len() <= MAX_NAME_BYTES
        && text
            .chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphanumeric())
        && text.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '+' | '-')
        })
}

fn trusted_owner(metadata: &Metadata) -> bool {
    let uid = metadata.uid();
    uid == 0 || uid == rustix::process::geteuid().as_raw()
}

/// A process other than the node owner or root must not be able to alter the cached artifact.
fn trusted(metadata: &Metadata) -> bool {
    trusted_owner(metadata) && metadata.mode() & 0o6022 == 0
}

/// A relative symlink is allowed only while its lexical target stays inside its own artifact.
fn link_inside(relative: &Path, target: &Path) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth = relative
        .parent()
        .map_or(0, |parent| parent.components().count());
    for part in target.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => match depth.checked_sub(1) {
                Some(next) => depth = next,
                None => return false,
            },
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn marker(root: &Path) -> Result<(), DriverError> {
    let path = root.join(MARKER);
    let metadata = fs::symlink_metadata(&path).map_err(|_| unavailable())?;
    if metadata.is_file() && !trusted(&metadata) {
        return Err(exposed(root, Path::new(MARKER)));
    }
    if !metadata.is_file() || metadata.len() > 128 {
        return Err(invalid());
    }
    let mut text = String::new();
    fs::File::open(path)
        .map_err(|_| unavailable())?
        .take(129)
        .read_to_string(&mut text)
        .map_err(|_| unavailable())?;
    let hash = text.trim();
    if hash.len() != CHECKSUM_HEX_DIGITS
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    Ok(())
}

fn validate_artifact(root: &Path, connector: bool, deadline: Instant) -> Result<(), DriverError> {
    marker(root)?;
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0_usize;
    let mut bytes = 0_u64;
    let mut hard_links: BTreeMap<(u64, u64), (u64, u64)> = BTreeMap::new();
    while let Some(path) = pending.pop() {
        check_deadline(deadline)?;
        let metadata = fs::symlink_metadata(&path).map_err(|_| unavailable())?;
        let relative = path.strip_prefix(root).map_err(|_| invalid())?;
        entries = entries
            .checked_add(1)
            .filter(|count| *count <= MAX_ARTIFACT_ENTRIES)
            .ok_or_else(invalid)?;
        if connector && relative == Path::new(SUPERVISOR_NAME) {
            return Err(invalid());
        }
        if metadata.file_type().is_symlink() {
            if !trusted_owner(&metadata) {
                return Err(invalid());
            }
            let target = fs::read_link(&path).map_err(|_| unavailable())?;
            let resolved = fs::canonicalize(&path).map_err(|_| invalid())?;
            if !link_inside(relative, &target) || !resolved.starts_with(root) {
                return Err(invalid());
            }
            continue;
        }
        if !trusted(&metadata) {
            return Err(exposed(root, relative));
        }
        if metadata.is_dir() {
            for entry in fs::read_dir(path).map_err(|_| unavailable())? {
                check_deadline(deadline)?;
                if entries
                    .checked_add(pending.len())
                    .and_then(|count| count.checked_add(1))
                    .is_none_or(|count| count > MAX_ARTIFACT_ENTRIES)
                {
                    return Err(invalid());
                }
                pending.push(entry.map_err(|_| unavailable())?.path());
            }
        } else if metadata.is_file() {
            bytes = bytes
                .checked_add(metadata.len())
                .filter(|size| *size <= MAX_ARTIFACT_BYTES)
                .ok_or_else(invalid)?;
            let entry = hard_links
                .entry((metadata.dev(), metadata.ino()))
                .or_insert((metadata.nlink(), 0));
            if entry.0 != metadata.nlink() {
                return Err(invalid());
            }
            entry.1 = entry.1.checked_add(1).ok_or_else(invalid)?;
        } else {
            return Err(invalid());
        }
    }
    if hard_links
        .values()
        .any(|(expected, found)| expected != found)
    {
        return Err(invalid());
    }
    check_deadline(deadline)
}

fn root_within(path: &Path, parent: &Path) -> Result<PathBuf, DriverError> {
    let root = fs::canonicalize(path).map_err(|_| unavailable())?;
    let metadata = fs::metadata(&root).map_err(|_| unavailable())?;
    if root.parent() != Some(parent) || !metadata.is_dir() {
        return Err(invalid());
    }
    if !trusted(&metadata) {
        return Err(exposed_directory(&root));
    }
    Ok(root)
}

fn actual_directory(path: &Path) -> Result<(), DriverError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_dir() {
        return Err(invalid());
    }
    if !trusted(&metadata) {
        return Err(exposed_directory(path));
    }
    Ok(())
}

fn archive_limit(tool_count: usize) -> Result<u64, DriverError> {
    let roots = u64::try_from(tool_count)
        .map_err(|_| invalid())?
        .checked_add(1)
        .ok_or_else(invalid)?;
    let per_root = MAX_ARTIFACT_BYTES
        .checked_add(
            u64::try_from(MAX_ARTIFACT_ENTRIES)
                .map_err(|_| invalid())?
                .checked_mul(MAX_ENTRY_OVERHEAD)
                .ok_or_else(invalid)?,
        )
        .ok_or_else(invalid)?;
    roots.checked_mul(per_root).ok_or_else(invalid)
}

/// Refuse an oversized or malformed transfer before receiving its archive body.
pub(crate) fn validate_receipt(
    receipt: &ArchiveReceipt,
    tools: &[ToolRef],
) -> Result<(), DriverError> {
    if receipt.bytes == 0
        || receipt.bytes > archive_limit(tools.len())?
        || receipt.sha256.len() != CHECKSUM_HEX_DIGITS
        || !receipt
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(invalid());
    }
    let mut names = BTreeSet::new();
    for tool in tools {
        if !plain_name(&tool.name)
            || !plain_name(&tool.version)
            || tool.name == CONNECTOR_NAME
            || tool.version == "current"
            || !names.insert(&tool.name)
        {
            return Err(invalid());
        }
    }
    Ok(())
}

struct CheckedWriter<'a, W> {
    inner: &'a mut W,
    sha256: Sha256,
    written: u64,
    limit: u64,
    deadline: Instant,
}

impl<W: Write> Write for CheckedWriter<'_, W> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        let proposed = self
            .written
            .checked_add(u64::try_from(bytes.len()).map_err(|_| io::Error::other("staging limit"))?)
            .ok_or_else(|| io::Error::other("staging limit"))?;
        if proposed > self.limit {
            return Err(io::Error::other("staging limit"));
        }
        let count = self.inner.write(bytes)?;
        self.sha256.update(&bytes[..count]);
        self.written += u64::try_from(count).map_err(|_| io::Error::other("staging limit"))?;
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        Ok(count)
    }
    fn flush(&mut self) -> io::Result<()> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        self.inner.flush()?;
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        Ok(())
    }
}

/// Archive the current connector and each named build, without assuming a shared node path on the worker.
/// The receiver must verify the returned digest before exposing the read-only staged tree to a workload.
pub(crate) fn archive<W: Write>(
    cache_dir: &Path,
    connector_dir: &Path,
    tools: &[ToolRef],
    output: &mut W,
    deadline: Instant,
) -> Result<ArchiveReceipt, DriverError> {
    check_deadline(deadline)?;
    if !cache_dir.is_absolute() || !connector_dir.is_absolute() {
        return Err(invalid());
    }
    let cache = fs::canonicalize(cache_dir).map_err(|_| unavailable())?;
    if !trusted(&fs::metadata(&cache).map_err(|_| unavailable())?) {
        return Err(exposed_directory(&cache));
    }
    actual_directory(&cache.join(CONNECTOR_NAME))?;
    let connector_parent = root_within(&cache.join(CONNECTOR_NAME), &cache)?;
    let connector = root_within(connector_dir, &connector_parent)?;
    validate_artifact(&connector, true, deadline)?;
    let binary = fs::canonicalize(connector.join(CONNECTOR_NAME)).map_err(|_| unavailable())?;
    let metadata = fs::metadata(&binary).map_err(|_| unavailable())?;
    if !binary.starts_with(&connector)
        || !metadata.is_file()
        || !trusted(&metadata)
        || metadata.mode() & 0o111 == 0
    {
        return Err(unavailable());
    }
    let mut names = BTreeSet::new();
    let mut artifacts = Vec::with_capacity(tools.len());
    for tool in tools {
        if !plain_name(&tool.name)
            || !plain_name(&tool.version)
            || tool.name == CONNECTOR_NAME
            || tool.version == "current"
            || !names.insert(&tool.name)
        {
            return Err(invalid());
        }
        actual_directory(&cache.join(&tool.name))?;
        let parent = root_within(&cache.join(&tool.name), &cache)?;
        actual_directory(&parent.join(&tool.version))?;
        let root = root_within(&parent.join(&tool.version), &parent)?;
        validate_artifact(&root, false, deadline)?;
        artifacts.push((&tool.name, root));
    }
    let limit = archive_limit(artifacts.len())?;
    let mut output = CheckedWriter {
        inner: output,
        sha256: Sha256::new(),
        written: 0,
        limit,
        deadline,
    };
    {
        let mut tar = tar::Builder::new(&mut output);
        tar.follow_symlinks(false);
        tar.sparse(false);
        tar.mode(tar::HeaderMode::Deterministic);
        tar.append_dir_all("bin", connector).map_err(io_error)?;
        for (name, root) in artifacts {
            tar.append_dir_all(Path::new("tools").join(name), root)
                .map_err(io_error)?;
        }
        tar.finish().map_err(io_error)?;
    }
    output.flush().map_err(io_error)?;
    check_deadline(deadline)?;
    Ok(ArchiveReceipt {
        sha256: hex::encode(output.sha256.finalize()),
        bytes: output.written,
    })
}

struct CheckedReader<'a, R> {
    inner: &'a mut R,
    deadline: Instant,
}

impl<R: Read> Read for CheckedReader<'_, R> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        let count = self.inner.read(bytes)?;
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "staging deadline"));
        }
        Ok(count)
    }
}

/// A private destination is needed because tar's confinement assumes no concurrent writer.
fn private_destination(path: &Path) -> Result<(), DriverError> {
    let metadata = fs::symlink_metadata(path).map_err(|_| unavailable())?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || fs::read_dir(path)
            .map_err(|_| unavailable())?
            .next()
            .is_some()
    {
        return Err(invalid());
    }
    Ok(())
}

fn parent_is_real_directory(destination: &Path, path: &Path) -> Result<(), DriverError> {
    let mut directory = destination.to_path_buf();
    for part in path.parent().into_iter().flat_map(Path::components) {
        let Component::Normal(name) = part else {
            return Err(invalid());
        };
        directory.push(name);
        let metadata = fs::symlink_metadata(&directory).map_err(|_| invalid())?;
        if !metadata.is_dir() {
            return Err(invalid());
        }
    }
    Ok(())
}

/// Verify the complete stream before unpacking into an empty private directory.
/// The caller must keep the input pinned and immutable across both passes and keep the
/// destination private until it has installed the trusted supervisor. A digest cannot
/// protect an input that another process changes after the first pass.
pub(crate) fn extract<R: Read + Seek>(
    input: &mut R,
    destination: &Path,
    tools: &[ToolRef],
    receipt: &ArchiveReceipt,
    deadline: Instant,
) -> Result<(), DriverError> {
    check_deadline(deadline)?;
    if !destination.is_absolute() {
        return Err(invalid());
    }
    private_destination(destination)?;
    let canonical_destination = fs::canonicalize(destination).map_err(|_| unavailable())?;
    let destination = canonical_destination.as_path();
    validate_receipt(receipt, tools)?;
    let names: BTreeSet<_> = tools.iter().map(|tool| tool.name.clone()).collect();
    input.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let mut digest = Sha256::new();
    let mut total = 0_u64;
    let mut buffer = [0_u8; 8 * 1024];
    loop {
        check_deadline(deadline)?;
        let count = input.read(&mut buffer).map_err(io_error)?;
        if count == 0 {
            break;
        }
        total = total
            .checked_add(u64::try_from(count).map_err(|_| invalid())?)
            .filter(|size| *size <= receipt.bytes)
            .ok_or_else(invalid)?;
        digest.update(&buffer[..count]);
    }
    if total != receipt.bytes || hex::encode(digest.finalize()) != receipt.sha256 {
        return Err(invalid());
    }
    check_deadline(deadline)?;
    input.seek(SeekFrom::Start(0)).map_err(io_error)?;
    let tools_dir = destination.join("tools");
    fs::create_dir(&tools_dir).map_err(|_| unavailable())?;
    fs::set_permissions(&tools_dir, fs::Permissions::from_mode(0o755))
        .map_err(|_| unavailable())?;
    let mut archive = tar::Archive::new(CheckedReader {
        inner: input,
        deadline,
    });
    archive.set_preserve_permissions(false);
    archive.set_preserve_mtime(false);
    archive.set_unpack_xattrs(false);
    let mut budgets: BTreeMap<PathBuf, (usize, u64)> = BTreeMap::new();
    let mut roots = BTreeSet::new();
    for entry in archive.entries().map_err(io_error)? {
        check_deadline(deadline)?;
        let mut entry = entry.map_err(io_error)?;
        let path = entry.path().map_err(io_error)?.into_owned();
        let parts: Vec<_> = path
            .components()
            .map(|part| match part {
                Component::Normal(name) => Ok(name),
                _ => Err(invalid()),
            })
            .collect::<Result<_, _>>()?;
        let root = if parts.first().is_some_and(|part| *part == "bin") {
            PathBuf::from("bin")
        } else if parts.first().is_some_and(|part| *part == "tools") && parts.len() >= 2 {
            let name = parts[1]
                .to_str()
                .filter(|name| names.contains(*name))
                .ok_or_else(invalid)?;
            Path::new("tools").join(name)
        } else {
            return Err(invalid());
        };
        if path == Path::new("bin").join(SUPERVISOR_NAME) {
            return Err(invalid());
        }
        parent_is_real_directory(destination, &path)?;
        match fs::symlink_metadata(destination.join(&path)) {
            Ok(_) => return Err(invalid()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return Err(unavailable()),
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink()) {
            return Err(invalid());
        }
        if path == root {
            if !kind.is_dir() {
                return Err(invalid());
            }
            roots.insert(root.clone());
        }
        let budget = budgets.entry(root.clone()).or_default();
        budget.0 = budget
            .0
            .checked_add(1)
            .filter(|count| *count <= MAX_ARTIFACT_ENTRIES)
            .ok_or_else(invalid)?;
        budget.1 = budget
            .1
            .checked_add(entry.size())
            .filter(|size| *size <= MAX_ARTIFACT_BYTES)
            .ok_or_else(invalid)?;
        if kind.is_symlink() {
            let target = entry.link_name().map_err(io_error)?.ok_or_else(invalid)?;
            let relative = path.strip_prefix(&root).map_err(|_| invalid())?;
            if !link_inside(relative, &target) {
                return Err(invalid());
            }
        } else {
            let mode = entry.header().mode().map_err(io_error)?;
            if mode & 0o6022 != 0 || entry.link_name().map_err(io_error)?.is_some() {
                return Err(invalid());
            }
        }
        if !entry.unpack_in(destination).map_err(io_error)? {
            return Err(invalid());
        }
        if !kind.is_symlink() {
            let mode = entry.header().mode().map_err(io_error)?;
            let plain = if kind.is_dir() || mode & 0o111 != 0 {
                0o755
            } else {
                0o644
            };
            fs::set_permissions(destination.join(&path), fs::Permissions::from_mode(plain))
                .map_err(|_| unavailable())?;
        }
    }
    if !roots.contains(Path::new("bin"))
        || names
            .iter()
            .any(|name| !roots.contains(&Path::new("tools").join(name)))
    {
        return Err(invalid());
    }
    for root in roots {
        let root_path = destination.join(&root);
        validate_artifact(&root_path, root.as_path() == Path::new("bin"), deadline)?;
    }
    let bin = destination.join("bin");
    let connector = fs::canonicalize(bin.join(CONNECTOR_NAME)).map_err(|_| unavailable())?;
    let metadata = fs::metadata(&connector).map_err(|_| unavailable())?;
    if !connector.starts_with(&bin) || !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err(invalid());
    }
    check_deadline(deadline)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::time::Duration;

    fn private_parent() -> tempfile::TempDir {
        let parent = tempfile::tempdir().unwrap();
        fs::set_permissions(parent.path(), fs::Permissions::from_mode(0o700)).unwrap();
        parent
    }

    fn artifact(root: &Path) {
        fs::create_dir_all(root).unwrap();
        fs::set_permissions(root, fs::Permissions::from_mode(0o755)).unwrap();
        fs::write(root.join(MARKER), format!("{}\n", "a".repeat(64))).unwrap();
        fs::set_permissions(root.join(MARKER), fs::Permissions::from_mode(0o644)).unwrap();
        fs::write(root.join("program"), "fixture program").unwrap();
        fs::set_permissions(root.join("program"), fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let parent = private_parent();
        let cache = parent.path().join("cache");
        let connector = cache.join(CONNECTOR_NAME).join("1.0.0");
        artifact(&connector);
        fs::write(connector.join(CONNECTOR_NAME), "fixture connector").unwrap();
        fs::set_permissions(
            connector.join(CONNECTOR_NAME),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        let tool = cache.join("codex").join("1.0.0");
        artifact(&tool);
        for path in [
            cache.clone(),
            cache.join(CONNECTOR_NAME),
            cache.join("codex"),
        ] {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        (parent, cache, connector)
    }

    #[tokio::test]
    async fn the_real_node_cache_can_be_staged_and_an_exposed_marker_is_refused() {
        use axum::{Router, body::Bytes, routing::get};
        use orch_link_contract::commands::NodeToolEnsure;
        use orch_node_core::{settings::NodeSettings, tools::ToolCache};
        use tokio_util::sync::CancellationToken;

        const CONNECTOR: &[u8] = b"#!/bin/sh\necho connector\n";
        const TOOL: &[u8] = b"#!/bin/sh\necho tool\n";
        let app = Router::new()
            .route(
                "/connector",
                get(|| async { Bytes::from_static(CONNECTOR) }),
            )
            .route("/tool", get(|| async { Bytes::from_static(TOOL) }));
        // The worker may start services only in this assigned port range.
        let first = std::process::id() as usize % 1000;
        let mut listener = None;
        for offset in 0..1000 {
            let port = 41000 + (first + offset) % 1000;
            if let Ok(bound) = tokio::net::TcpListener::bind(("127.0.0.1", port as u16)).await {
                listener = Some(bound);
                break;
            }
        }
        let listener = listener.expect("an assigned test port is available");
        let base = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let parent = private_parent();
        let cache = ToolCache::open(&NodeSettings::in_dir(parent.path())).unwrap();
        let request = |name: &str, endpoint: &str, bytes: &[u8]| NodeToolEnsure {
            job_id: None,
            name: name.into(),
            version: "1".into(),
            url: format!("{base}/{endpoint}"),
            sha256: hex::encode(Sha256::digest(bytes)),
        };
        let connector = cache
            .ensure(
                &request(CONNECTOR_NAME, "connector", CONNECTOR),
                &CancellationToken::new(),
            )
            .await
            .unwrap();
        let tool = cache
            .ensure(&request("codex", "tool", TOOL), &CancellationToken::new())
            .await
            .unwrap();
        server.abort();

        let tools = [ToolRef {
            name: "codex".into(),
            version: "1".into(),
        }];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bytes = Vec::new();
        let receipt = archive(cache.dir(), &connector, &tools, &mut bytes, deadline).unwrap();
        let output = destination(parent.path(), "published");
        extract(
            &mut io::Cursor::new(bytes),
            &output,
            &tools,
            &receipt,
            deadline,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("bin/orch-connector")).unwrap(),
            CONNECTOR
        );
        assert_eq!(fs::read(output.join("tools/codex/codex")).unwrap(), TOOL);

        fs::set_permissions(tool.join(MARKER), fs::Permissions::from_mode(0o664)).unwrap();
        let Err(DriverError::InvalidSpec(said)) =
            archive(cache.dir(), &connector, &tools, &mut Vec::new(), deadline)
        else {
            panic!("a marker that others can write is refused");
        };
        assert!(
            said.contains("codex/1") && said.contains(MARKER),
            "the refusal names the entry and the file: {said}"
        );
    }

    #[test]
    fn verified_artifacts_are_staged_once_with_internal_links_and_a_matching_digest() {
        let (_parent, cache, connector) = fixture();
        symlink("program", cache.join("codex/1.0.0/current")).unwrap();
        fs::hard_link(
            cache.join("codex/1.0.0/program"),
            cache.join("codex/1.0.0/program-copy"),
        )
        .unwrap();
        let mut bytes = Vec::new();
        let tools = [ToolRef {
            name: "codex".into(),
            version: "1.0.0".into(),
        }];
        let receipt = archive(
            &cache,
            &connector,
            &tools,
            &mut bytes,
            Instant::now() + Duration::from_secs(5),
        )
        .unwrap();
        assert_eq!(receipt.bytes, u64::try_from(bytes.len()).unwrap());
        assert_eq!(receipt.sha256, hex::encode(Sha256::digest(&bytes)));
        let mut reader = tar::Archive::new(bytes.as_slice());
        assert!(!String::from_utf8_lossy(&bytes).contains(cache.to_str().unwrap()));
        let paths: Vec<_> = reader
            .entries()
            .unwrap()
            .map(|entry| {
                let entry = entry.unwrap();
                let path = entry.path().unwrap().into_owned();
                if path == Path::new("tools/codex/current") {
                    assert!(entry.header().entry_type().is_symlink());
                    assert_eq!(
                        entry.link_name().unwrap().unwrap().as_ref(),
                        Path::new("program")
                    );
                }
                path
            })
            .collect();
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("bin/orch-connector"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("tools/codex/current"))
        );
        assert!(
            paths
                .iter()
                .any(|path| path == Path::new("tools/codex/program-copy"))
        );
    }

    #[test]
    fn unverified_escaping_or_reserved_sources_are_never_staged() {
        let (_parent, cache, connector) = fixture();
        let tool = cache.join("codex/1.0.0");
        let tools = [ToolRef {
            name: "codex".into(),
            version: "1.0.0".into(),
        }];
        let deadline = Instant::now() + Duration::from_secs(5);
        assert!(
            archive(
                &cache,
                &connector,
                &[ToolRef {
                    name: "../outside".into(),
                    version: "1".into()
                }],
                &mut Vec::new(),
                deadline
            )
            .is_err()
        );
        symlink("../../outside", tool.join("escape")).unwrap();
        assert!(archive(&cache, &connector, &tools, &mut Vec::new(), deadline).is_err());
        fs::write(connector.join(SUPERVISOR_NAME), "not trusted").unwrap();
        assert!(archive(&cache, &connector, &[], &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        symlink("program", connector.join(SUPERVISOR_NAME)).unwrap();
        assert!(archive(&cache, &connector, &[], &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        fs::write(connector.join(MARKER), "not a verified digest").unwrap();
        assert!(archive(&cache, &connector, &[], &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        fs::set_permissions(cache.join("codex/1.0.0"), fs::Permissions::from_mode(0o777)).unwrap();
        assert!(archive(&cache, &connector, &tools, &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        symlink("1.0.0", cache.join("codex/alias")).unwrap();
        let alias = [ToolRef {
            name: "codex".into(),
            version: "alias".into(),
        }];
        assert!(archive(&cache, &connector, &alias, &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        let tool = cache.join("codex/1.0.0");
        fs::write(cache.join("codex/outside"), "not in this artifact").unwrap();
        symlink(".", tool.join("inside")).unwrap();
        symlink("inside/../outside", tool.join("chained-escape")).unwrap();
        assert!(archive(&cache, &connector, &tools, &mut Vec::new(), deadline).is_err());
        let (_parent, cache, connector) = fixture();
        let outside = cache.join("codex/outside");
        fs::write(&outside, "not in this artifact").unwrap();
        fs::set_permissions(&outside, fs::Permissions::from_mode(0o644)).unwrap();
        fs::hard_link(&outside, cache.join("codex/1.0.0/external-link")).unwrap();
        assert!(archive(&cache, &connector, &tools, &mut Vec::new(), deadline).is_err());
        assert!(matches!(
            archive(&cache, &connector, &[], &mut Vec::new(), Instant::now()),
            Err(DriverError::Timeout)
        ));
    }

    fn destination(parent: &Path, name: &str) -> PathBuf {
        let path = parent.join(name);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        path
    }

    #[test]
    fn the_receiver_verifies_the_digest_before_unpacking_a_private_tree() {
        let (parent, cache, connector) = fixture();
        symlink("program", cache.join("codex/1.0.0/current")).unwrap();
        fs::hard_link(
            cache.join("codex/1.0.0/program"),
            cache.join("codex/1.0.0/program-copy"),
        )
        .unwrap();
        let tools = [ToolRef {
            name: "codex".into(),
            version: "1.0.0".into(),
        }];
        let deadline = Instant::now() + Duration::from_secs(10);
        let mut bytes = Vec::new();
        let receipt = archive(&cache, &connector, &tools, &mut bytes, deadline).unwrap();
        let output = destination(parent.path(), "output");
        extract(
            &mut io::Cursor::new(bytes.clone()),
            &output,
            &tools,
            &receipt,
            deadline,
        )
        .unwrap();
        assert_eq!(
            fs::read(output.join("bin/orch-connector")).unwrap(),
            b"fixture connector"
        );
        assert_eq!(
            fs::read(output.join("tools/codex/current")).unwrap(),
            b"fixture program"
        );
        assert_eq!(
            fs::read(output.join("tools/codex/program-copy")).unwrap(),
            b"fixture program"
        );
        assert_eq!(fs::metadata(&output).unwrap().mode() & 0o077, 0);
        assert_ne!(
            fs::metadata(output.join("bin/orch-connector"))
                .unwrap()
                .mode()
                & 0o111,
            0
        );

        let damaged = destination(parent.path(), "damaged");
        bytes[0] ^= 1;
        assert!(
            extract(
                &mut io::Cursor::new(bytes),
                &damaged,
                &tools,
                &receipt,
                deadline
            )
            .is_err()
        );
        assert!(fs::read_dir(damaged).unwrap().next().is_none());
    }

    #[test]
    fn the_receiver_refuses_an_escaping_link_even_with_a_matching_digest() {
        let parent = private_parent();
        let output = destination(parent.path(), "output");
        let mut bytes = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut bytes);
            let mut directory = tar::Header::new_gnu();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(0o755);
            directory.set_cksum();
            archive
                .append_data(&mut directory, "bin", io::empty())
                .unwrap();
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("../../outside").unwrap();
            link.set_cksum();
            archive
                .append_data(&mut link, "bin/escape", io::empty())
                .unwrap();
            archive.finish().unwrap();
        }
        let receipt = ArchiveReceipt {
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: u64::try_from(bytes.len()).unwrap(),
        };
        assert!(
            extract(
                &mut io::Cursor::new(bytes),
                &output,
                &[],
                &receipt,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
        assert!(fs::symlink_metadata(output.join("bin/escape")).is_err());
    }

    #[test]
    fn the_receiver_never_writes_through_an_earlier_symlink() {
        let parent = private_parent();
        let output = destination(parent.path(), "output");
        let mut bytes = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut bytes);
            for path in ["bin", "bin/real"] {
                let mut directory = tar::Header::new_gnu();
                directory.set_entry_type(tar::EntryType::Directory);
                directory.set_size(0);
                directory.set_mode(0o755);
                directory.set_cksum();
                archive
                    .append_data(&mut directory, path, io::empty())
                    .unwrap();
            }
            let mut link = tar::Header::new_gnu();
            link.set_entry_type(tar::EntryType::Symlink);
            link.set_size(0);
            link.set_mode(0o777);
            link.set_link_name("real").unwrap();
            link.set_cksum();
            archive
                .append_data(&mut link, "bin/alias", io::empty())
                .unwrap();
            let mut file = tar::Header::new_gnu();
            file.set_entry_type(tar::EntryType::Regular);
            file.set_size(1);
            file.set_mode(0o644);
            file.set_cksum();
            archive
                .append_data(&mut file, "bin/alias/payload", &b"x"[..])
                .unwrap();
            archive.finish().unwrap();
        }
        let receipt = ArchiveReceipt {
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: u64::try_from(bytes.len()).unwrap(),
        };
        assert!(
            extract(
                &mut io::Cursor::new(bytes),
                &output,
                &[],
                &receipt,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
        assert!(!output.join("bin/real/payload").exists());
    }

    #[test]
    fn the_receiver_checks_the_resolved_target_of_a_link_chain() {
        let parent = private_parent();
        let output = destination(parent.path(), "output");
        let mut bytes = Vec::new();
        {
            let mut archive = tar::Builder::new(&mut bytes);
            let mut directory = tar::Header::new_gnu();
            directory.set_entry_type(tar::EntryType::Directory);
            directory.set_size(0);
            directory.set_mode(0o755);
            directory.set_cksum();
            archive
                .append_data(&mut directory, "bin", io::empty())
                .unwrap();
            let mut connector = tar::Header::new_gnu();
            connector.set_entry_type(tar::EntryType::Regular);
            connector.set_size(1);
            connector.set_mode(0o755);
            connector.set_cksum();
            archive
                .append_data(&mut connector, "bin/orch-connector", &b"x"[..])
                .unwrap();
            let marker = format!("{}\n", "a".repeat(64));
            let mut header = tar::Header::new_gnu();
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(u64::try_from(marker.len()).unwrap());
            header.set_mode(0o644);
            header.set_cksum();
            archive
                .append_data(&mut header, "bin/.complete", marker.as_bytes())
                .unwrap();
            for (name, target) in [("bin/inside", "."), ("bin/escape", "inside/../tools")] {
                let mut link = tar::Header::new_gnu();
                link.set_entry_type(tar::EntryType::Symlink);
                link.set_size(0);
                link.set_mode(0o777);
                link.set_link_name(target).unwrap();
                link.set_cksum();
                archive.append_data(&mut link, name, io::empty()).unwrap();
            }
            archive.finish().unwrap();
        }
        let receipt = ArchiveReceipt {
            sha256: hex::encode(Sha256::digest(&bytes)),
            bytes: u64::try_from(bytes.len()).unwrap(),
        };
        assert!(
            extract(
                &mut io::Cursor::new(bytes),
                &output,
                &[],
                &receipt,
                Instant::now() + Duration::from_secs(5)
            )
            .is_err()
        );
    }
}
