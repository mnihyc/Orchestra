//! Verified tool downloads and read-only cache publication.
//!
//! Managed runtimes get Orch's own programs and the harness builds from the host's tool cache, which the
//! driver makes visible inside a unit read-only, so that any image works and none has to carry anything of
//! Orch's. `node.tool.ensure` puts a tool there: it downloads the address the system names, checks the SHA-256
//! the system names against what arrived, and publishes the result as `tools/<name>/<version>/`.
//!
//! **Publication is a rename.** The download is unpacked in a scratch directory of the cache, given its markers
//! (`.complete`, holding the hash it was verified against, and `.version`, which says which version it is: a unit
//! sees the directory by the tool's name alone) and moved into place in one step, so a directory of
//! the cache is either whole and marked or not there; the scratch directory of a download that was cut off is
//! removed when the node starts. A version that is in the cache is never replaced, not even by a build with
//! another hash: units stand on it, and nothing changes behind a unit's back. What the system names again with
//! the same hash is a no-op.
//!
//! **What a download is.** An address that ends in `.tar.gz`, `.tgz`, `.tar`, `.zip` or `.whl` (a wheel) is an archive, unpacked
//! with every path held inside the directory it unpacks to; anything else is one program, saved as
//! `<name>` and made executable. The connector's program is the tool called [`CONNECTOR_TOOL`]: it must
//! be at the top of the directory, and once it is published new units are given that version (the link
//! `tools/orch-connector/current` is moved to it), while the units that exist keep the build they were made
//! with.

use std::io::{Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use futures::StreamExt;
use orch_link_contract::commands::{CommandError, ErrorCode, NodeToolEnsure};
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

use orch_connector_core::builds::VERSION_MARKER;

use crate::drivers::INVALID;
use crate::facts::Reports;
use crate::settings::{CONNECTOR_TOOL, CURRENT_LINK, NodeSettings};

/// The provisioning phase owns tool-download progress, using the common job byte counters.
const TOOL_PHASE: &str = "tools";

struct DownloadReport<'a> {
    reports: Option<&'a Reports>,
    job: Option<orch_types::Id>,
    done: u64,
    total: Option<u64>,
    last: Option<tokio::time::Instant>,
}

impl DownloadReport<'_> {
    async fn emit(&mut self, finished: Option<&str>, force: bool) {
        let (Some(reports), Some(job)) = (self.reports, self.job) else {
            return;
        };
        if !force
            && self
                .last
                .is_some_and(|last| last.elapsed() < crate::images::PROGRESS_EVERY)
        {
            return;
        }
        self.last = Some(tokio::time::Instant::now());
        let percent = if finished.is_some() {
            100
        } else {
            self.total.filter(|total| *total > 0).map_or(0, |total| {
                ((u128::from(self.done) * 100 / u128::from(total)).min(99)) as u32
            })
        };
        let progress = orch_api_contract::workspace::JobProgress {
            activity: None,
            phase: TOOL_PHASE.into(),
            phases: vec![orch_api_contract::workspace::JobPhase {
                name: TOOL_PHASE.into(),
                weight: 100,
                state: if finished.is_some() {
                    orch_api_contract::workspace::PhaseState::Done
                } else {
                    orch_api_contract::workspace::PhaseState::Running
                },
            }],
            percent,
            files_done: 0,
            files_total: None,
            bytes_done: i64::try_from(self.done).unwrap_or(i64::MAX),
            bytes_total: self.total.and_then(|total| i64::try_from(total).ok()),
            rate_bps: None,
            eta_seconds: None,
            message: Some(finished.map_or_else(
                || "Downloading and verifying the tool".into(),
                |hash| format!("Verified SHA-256 {hash}"),
            )),
        };
        reports.job(job, &progress, None).await;
    }
}

/// The file that marks a tool's directory complete; it holds the SHA-256 the download was verified against.
pub const COMPLETE_MARKER: &str = ".complete";

/// The verified payload's byte count, recorded once at publication so inventory never walks its files.
pub const SIZE_MARKER: &str = ".orch-bytes";

/// The directory of the cache where downloads are unpacked before they are published.
const PARTIAL_DIR: &str = ".partial";

/// The most a download may be: 2 GiB, far above any harness build and below what fills a disk by accident.
pub const MAX_DOWNLOAD_BYTES: u64 = 2 * 1024 * 1024 * 1024;

/// The most an archive may unpack to: four times the most a download may be, since a compressed program
/// shrinks to a quarter of itself at the very least, and an archive that claims more is a bomb.
pub const MAX_UNPACKED_BYTES: u64 = 4 * MAX_DOWNLOAD_BYTES;

/// The most entries an archive may hold.
pub const MAX_ENTRIES: usize = 200_000;

/// How long the connection to the server may take to be made.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the server may stay silent in the middle of a download before it is given up.
const STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// The mode of a program that is saved as it was downloaded.
const PROGRAM_MODE: u32 = 0o755;

/// Why a tool could not be put in the cache.
#[derive(Debug, thiserror::Error)]
pub enum ToolError {
    /// The machine's kept or handed setting refuses a new download.
    #[error("{0}")]
    Policy(String),
    /// The name, the version, the hash or the address is not one.
    #[error("{0}")]
    Invalid(String),
    /// The server could not be reached or answered something else than the file.
    #[error("the tool could not be downloaded: {0}")]
    Download(String),
    /// What arrived is not what the system said it would be.
    #[error("the download has the hash {found}, and the system named {expected}")]
    HashMismatch {
        /// What the system named.
        expected: String,
        /// What arrived.
        found: String,
    },
    /// The version is in the cache with another hash.
    #[error(
        "{name} {version} is in the cache with the hash {held}, and the system named {named}: a version in the cache is never replaced"
    )]
    Held {
        /// The tool.
        name: String,
        /// The version.
        version: String,
        /// The hash it is in the cache with.
        held: String,
        /// The hash the system named now.
        named: String,
    },
    /// The download is not the shape a tool is.
    #[error("{0}")]
    Shape(String),
    /// Something on the disk failed.
    #[error("{what}: {source}")]
    Io {
        /// What was being done.
        what: String,
        /// What the system said.
        source: std::io::Error,
    },
    /// The system asked for the download not to be made after all.
    #[error("the download was cancelled")]
    Cancelled,
}

impl From<ToolError> for CommandError {
    fn from(error: ToolError) -> Self {
        let (code, retryable) = match &error {
            ToolError::Policy(_) => (ErrorCode::Unsupported.as_str(), false),
            ToolError::Invalid(_) | ToolError::Shape(_) => (INVALID, false),
            ToolError::Held { .. } => (ErrorCode::Conflict.as_str(), false),
            ToolError::Download(_) | ToolError::HashMismatch { .. } => {
                (ErrorCode::Internal.as_str(), true)
            }
            ToolError::Io { .. } => (ErrorCode::Internal.as_str(), false),
            ToolError::Cancelled => (ErrorCode::Timeout.as_str(), false),
        };
        // A download that is not what its vendor published is said as such, so that the build is refused, not tried
        // again.
        let details = match &error {
            ToolError::HashMismatch { expected, found } => Some(serde_json::json!({
                "checksum": "mismatch", "expected": expected, "found": found,
            })),
            _ => None,
        };
        CommandError {
            code: code.to_owned(),
            detail: error.to_string(),
            retryable,
            details,
        }
    }
}

fn io(what: impl Into<String>) -> impl FnOnce(std::io::Error) -> ToolError {
    let what = what.into();
    move |source| ToolError::Io { what, source }
}

/// Make new cache directories without granting write access to another user.
/// Existing directories retain their permissions: an old cache cannot be made
/// trustworthy merely by changing its mode after it may have been exposed.
fn make_cache_directory(path: &Path, mode: u32) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::fs::DirBuilder::new().mode(mode).create(path) {
        Ok(()) => std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists && path.is_dir() => Ok(()),
        Err(error) => Err(error),
    }
}

/// Markers belong to Orch, not the downloaded archive. Create them without
/// following an archive-provided link or briefly granting group write access.
fn write_marker(path: &Path, value: &str) -> Result<(), ToolError> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o644)
        .open(path)
        .map_err(io("creating a tool marker"))?;
    file.write_all(value.as_bytes())
        .map_err(io("writing a tool marker"))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o644))
        .map_err(io("setting the mode of a tool marker"))
}

/// Whether `text` is a name or a version that can be a directory of the cache: letters, digits and the
/// characters `.`, `_`, `+` and `-`, starting with a letter or a digit, and not so long as to be a trick.
fn is_plain_name(text: &str) -> bool {
    const LONGEST: usize = 128;
    let mut chars = text.chars();
    chars
        .next()
        .is_some_and(|first| first.is_ascii_alphanumeric())
        && text.len() <= LONGEST
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && !text.contains("..")
}

/// How an address says its content is packed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Packing {
    TarGz,
    Tar,
    Zip,
    Program,
}

fn packing_of(url: &reqwest::Url) -> Packing {
    let path = url.path().to_ascii_lowercase();
    if path.ends_with(".tar.gz") || path.ends_with(".tgz") {
        Packing::TarGz
    } else if path.ends_with(".tar") {
        Packing::Tar
    } else if path.ends_with(".zip") || path.ends_with(".whl") {
        // A Python wheel is a zip: the build of a program that PyPI carries.
        Packing::Zip
    } else {
        Packing::Program
    }
}

/// The tool cache of a node.
#[derive(Debug, Clone)]
pub struct ToolCache {
    policy: Option<std::sync::Arc<orch_connector_core::policy::PolicyStore>>,
    dir: PathBuf,
    http: Option<reqwest::Client>,
    changes: std::sync::Arc<tokio::sync::Mutex<()>>,
    data_dir: PathBuf,
    link: Option<orch_link::machine::MachineLink>,
    clock: std::sync::Arc<dyn orch_types::Clock>,
}

impl ToolCache {
    /// The cache of the node configured as `settings`. The directory is made, and the scratch directories of
    /// downloads that were cut off by an earlier run are removed.
    pub fn open(settings: &NodeSettings) -> Result<Self, ToolError> {
        let mut cache = Self::inspect(settings)?;
        cache.http = Some(
            reqwest::Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(STALL_TIMEOUT)
                .build()
                .map_err(|error| ToolError::Download(error.to_string()))?,
        );
        cache.check_root()?;
        let dir = settings.tools_dir();
        make_cache_directory(&dir, 0o755).map_err(io(format!("making {}", dir.display())))?;
        cache.check_root()?;
        let partial = dir.join(PARTIAL_DIR);
        if partial.exists() {
            std::fs::remove_dir_all(&partial)
                .map_err(io(format!("removing {}", partial.display())))?;
        }
        Ok(cache)
    }

    /// Reads the verified cache without creating it or clearing interrupted downloads.
    pub fn inspect(settings: &NodeSettings) -> Result<Self, ToolError> {
        let dir = settings.tools_dir();
        Ok(Self {
            dir,
            http: None,
            data_dir: settings.data_dir.clone(),
            link: None,
            clock: std::sync::Arc::new(orch_types::SystemClock),
            policy: None,
            changes: std::sync::Arc::new(tokio::sync::Mutex::new(())),
        })
    }

    /// A substituted cache root is not permission to use or erase somebody else's files.
    pub(crate) fn check_root(&self) -> Result<(), ToolError> {
        match std::fs::symlink_metadata(&self.dir) {
            Ok(metadata) if metadata.is_dir() => Ok(()),
            Ok(_) => Err(ToolError::Invalid(
                "the tool cache root is not a real directory".into(),
            )),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(io("reading the tool cache root")(error)),
        }
    }

    /// The hosting program's one policy; this changes no cache entries or build choices.
    #[must_use]
    pub fn with_policy(
        &self,
        policy: std::sync::Arc<orch_connector_core::policy::PolicyStore>,
    ) -> Self {
        Self {
            dir: self.dir.clone(),
            data_dir: self.data_dir.clone(),
            link: self.link.clone(),
            clock: std::sync::Arc::clone(&self.clock),
            http: self.http.clone(),
            policy: Some(policy),
            changes: self.changes.clone(),
        }
    }

    /// The node's live connection supplies the origin of files served by this Orch installation.
    #[must_use]
    pub fn with_link(
        mut self,
        link: orch_link::machine::MachineLink,
        clock: std::sync::Arc<dyn orch_types::Clock>,
    ) -> Self {
        self.link = Some(link);
        self.clock = clock;
        self
    }

    fn download_source(&self, address: &str) -> Result<(reqwest::Client, reqwest::Url), ToolError> {
        let data = orch_connector_core::enrollment::DataDir::open(&self.data_dir)
            .map_err(|error| ToolError::Download(error.to_string()))?;
        let absolute = reqwest::Url::parse(address).ok();
        let own = address.starts_with("/api/v1/downloads/")
            || absolute.as_ref().is_some_and(|url| {
                url.path().starts_with("/api/v1/downloads/")
                    && data
                        .record()
                        .ok()
                        .and_then(|record| reqwest::Url::parse(&record.system.public_url).ok())
                        .is_some_and(|public| public.origin() == url.origin())
            });
        if !own {
            let url = absolute
                .filter(|url| matches!(url.scheme(), "http" | "https"))
                .ok_or_else(|| {
                    ToolError::Invalid("a tool is downloaded from an http or https address".into())
                })?;
            return Ok((self.client()?.clone(), url));
        }
        let endpoint = self
            .link
            .as_ref()
            .and_then(orch_link::machine::MachineLink::connected_endpoint)
            .ok_or_else(|| {
                ToolError::Download("the node has no verified connection address".into())
            })?;
        let identity = data
            .identity()
            .map_err(|error| ToolError::Download(error.to_string()))?;
        let pinned = endpoint.trust == orch_link_contract::enrollment::Trust::Pinned;
        let tls_identity = identity
            .tls()
            .map_err(|error| ToolError::Download(error.to_string()))?;
        let tls = orch_pki::verification::machine_client_config(
            Some(orch_pki::verification::ServerTrust::Authority(Box::new(
                identity.authority.clone(),
            ))),
            if pinned {
                orch_pki::certificates::ServiceName::Gateway
            } else {
                orch_pki::certificates::ServiceName::Frontend
            },
            pinned.then_some(&tls_identity),
            std::sync::Arc::clone(&self.clock),
        )
        .map_err(|error| ToolError::Download(error.to_string()))?;
        let client = reqwest::Client::builder()
            .tls_backend_preconfigured(tls.as_ref().clone())
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(STALL_TIMEOUT)
            .build()
            .map_err(|error| ToolError::Download(error.to_string()))?;
        let base = orch_connector_core::https::https_base(&endpoint.url)
            .map_err(|error| ToolError::Download(error.to_string()))?;
        let path = absolute.as_ref().map_or(address, |url| url.path());
        let url = base
            .join(path)
            .map_err(|error| ToolError::Invalid(error.to_string()))?;
        Ok((client, url))
    }

    fn require_fetch(&self) -> Result<(), ToolError> {
        self.client()?;
        self.policy.as_ref().map_or(Ok(()), |policy| {
            policy
                .require_tools_fetch()
                .map_err(|error| ToolError::Policy(error.detail))
        })
    }

    fn client(&self) -> Result<&reqwest::Client, ToolError> {
        self.http.as_ref().ok_or_else(|| {
            ToolError::Invalid("the read-only tool inventory cannot fetch tools".into())
        })
    }

    /// The directory of the cache.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Where a version of a tool is, or would be.
    #[must_use]
    pub fn path_of(&self, name: &str, version: &str) -> PathBuf {
        self.dir.join(name).join(version)
    }

    /// Whether a version of a tool is in the cache, whole.
    #[must_use]
    pub fn has(&self, name: &str, version: &str) -> bool {
        is_plain_name(name)
            && is_plain_name(version)
            && self.path_of(name, version).join(COMPLETE_MARKER).is_file()
    }

    /// The tools in the cache, whole, as `(name, version)` in order. A directory without its marker is not
    /// there.
    #[must_use]
    pub fn list(&self) -> Vec<(String, String)> {
        let mut found = Vec::new();
        if self.check_root().is_err() {
            return found;
        }
        let Ok(names) = std::fs::read_dir(&self.dir) else {
            return found;
        };
        for name in names.flatten() {
            let tool = name.file_name().to_string_lossy().into_owned();
            if !is_plain_name(&tool) || !name.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let Ok(versions) = std::fs::read_dir(name.path()) else {
                continue;
            };
            for version in versions.flatten() {
                let real = version.file_type().is_ok_and(|kind| kind.is_dir());
                let version = version.file_name().to_string_lossy().into_owned();
                if real && version != CURRENT_LINK && self.has(&tool, &version) {
                    found.push((tool.clone(), version));
                }
            }
        }
        found.sort();
        found
    }

    /// The recorded payload bytes; older or unreadable versions stay unknown until measured.
    #[must_use]
    pub fn size_of(&self, name: &str, version: &str) -> Option<i64> {
        if !self.has(name, version) {
            return None;
        }
        let path = self.path_of(name, version).join(SIZE_MARKER);
        if !std::fs::symlink_metadata(&path).ok()?.is_file() {
            return None;
        }
        std::fs::read_to_string(path)
            .ok()?
            .trim()
            .parse::<i64>()
            .ok()
            .filter(|size| *size >= 0)
    }

    /// The version the link `current` of the connector tool points at.
    #[must_use]
    pub fn current_connector(&self) -> Option<String> {
        let link = self.dir.join(CONNECTOR_TOOL).join(CURRENT_LINK);
        std::fs::read_link(link)
            .ok()
            .and_then(|target| target.file_name().map(|v| v.to_string_lossy().into_owned()))
    }

    /// Removes one verified real cache directory, serialized with publication and never following a link.
    pub async fn remove_cached(&self, name: &str, version: &str) -> Result<(), ToolError> {
        let _change = self.changes.lock().await;
        self.check_root()?;
        if version == CURRENT_LINK || !self.list().contains(&(name.to_owned(), version.to_owned()))
        {
            return Err(ToolError::Invalid(
                "the tool version is not in the verified cache".into(),
            ));
        }
        let path = self.path_of(name, version);
        // Publication shares this guard, so an ensure cannot replace what is being removed.
        std::fs::remove_dir_all(path).map_err(io("removing the cached tool"))?;
        if name == CONNECTOR_TOOL && self.current_connector().as_deref() == Some(version) {
            let link = self.dir.join(name).join(CURRENT_LINK);
            std::fs::remove_file(link).map_err(io("removing the cached connector link"))?;
        }
        Ok(())
    }

    /// Makes sure the tool is in the cache: as it is if it is there with that hash, downloaded and verified
    /// otherwise. Answers where it is.
    pub async fn ensure(
        &self,
        request: &NodeToolEnsure,
        cancel: &CancellationToken,
    ) -> Result<PathBuf, ToolError> {
        self.ensure_with_reports(request, cancel, None).await
    }

    /// Downloads and publishes a complete verified tool, reporting bytes to its job on the existing stream.
    pub async fn ensure_reporting(
        &self,
        request: &NodeToolEnsure,
        cancel: &CancellationToken,
        reports: &Reports,
    ) -> Result<PathBuf, ToolError> {
        self.ensure_with_reports(request, cancel, Some(reports))
            .await
    }

    async fn ensure_with_reports(
        &self,
        request: &NodeToolEnsure,
        cancel: &CancellationToken,
        reports: Option<&Reports>,
    ) -> Result<PathBuf, ToolError> {
        let _change = self.changes.lock().await;
        self.check_root()?;
        let mut progress = DownloadReport {
            reports,
            job: request.job_id,
            done: 0,
            total: None,
            last: None,
        };
        let answer = self.ensure_inner(request, cancel, &mut progress).await;
        if answer.is_ok() {
            progress
                .emit(Some(&request.sha256.trim().to_ascii_lowercase()), true)
                .await;
        }
        answer
    }

    async fn ensure_inner(
        &self,
        request: &NodeToolEnsure,
        cancel: &CancellationToken,
        progress: &mut DownloadReport<'_>,
    ) -> Result<PathBuf, ToolError> {
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        let (name, version) = (request.name.as_str(), request.version.as_str());
        if !is_plain_name(name) || !is_plain_name(version) {
            return Err(ToolError::Invalid(format!(
                "{name:?} {version:?} is not a name and a version a tool can have"
            )));
        }
        let wanted = request.sha256.trim().to_ascii_lowercase();
        if wanted.len() != 64 || !wanted.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(ToolError::Invalid(
                "the hash of a tool is 64 hexadecimal digits of a SHA-256".to_owned(),
            ));
        }
        let target = self.path_of(name, version);
        if let Some(held) = self.hash_held(&target)? {
            if held != wanted {
                return Err(ToolError::Held {
                    name: name.to_owned(),
                    version: version.to_owned(),
                    held,
                    named: wanted,
                });
            }
            self.point_current(name, version)?;
            return Ok(target);
        }
        self.require_fetch()?;
        let scratch = tempfile::Builder::new()
            .prefix(&format!("{name}-{version}-"))
            .tempdir_in(self.partial()?)
            .map_err(io("making a scratch directory"))?;
        let download = scratch.path().join("download");
        let (client, url) = self.download_source(&request.url)?;
        let found = self
            .download(&client, &url, &download, cancel, progress)
            .await?;
        self.require_fetch()?;
        if found != wanted {
            return Err(ToolError::HashMismatch {
                expected: wanted,
                found,
            });
        }
        // A second ask for the same tool may have published it while this one downloaded: what is there is not
        // replaced, whatever stands on it.
        if let Some(held) = self.hash_held(&target)? {
            return if held == wanted {
                self.point_current(name, version)?;
                Ok(target)
            } else {
                Err(ToolError::Held {
                    name: name.to_owned(),
                    version: version.to_owned(),
                    held,
                    named: wanted,
                })
            };
        }
        let content = scratch.path().join("content");
        let (packing, download_path, content_path) =
            (packing_of(&url), download.clone(), content.clone());
        let tool_name = name.to_owned();
        tokio::task::spawn_blocking(move || {
            unpack(packing, &download_path, &content_path, &tool_name)?;
            let bytes = crate::resources::directory_bytes(&content_path)
                .map_err(io("measuring the verified tool payload"))?;
            // Like the other markers, whatever the process's umask is.
            write_marker(&content_path.join(SIZE_MARKER), &bytes.to_string())
        })
        .await
        .map_err(|error| ToolError::Shape(format!("unpacking failed: {error}")))??;
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        if name == CONNECTOR_TOOL {
            check_connector(&content)?;
        }
        write_marker(&content.join(COMPLETE_MARKER), &format!("{wanted}\n"))?;
        write_marker(&content.join(VERSION_MARKER), &format!("{version}\n"))?;
        if cancel.is_cancelled() {
            return Err(ToolError::Cancelled);
        }
        self.publish(&content, &target, name, version, &wanted)?;
        self.point_current(name, version)?;
        Ok(target)
    }

    fn partial(&self) -> Result<PathBuf, ToolError> {
        let partial = self.dir.join(PARTIAL_DIR);
        make_cache_directory(&partial, 0o700).map_err(io("making the scratch directory"))?;
        let meta =
            std::fs::symlink_metadata(&partial).map_err(io("checking the scratch directory"))?;
        if !meta.is_dir()
            || meta.uid() != rustix::process::geteuid().as_raw()
            || meta.mode() & 0o7077 != 0
        {
            return Err(ToolError::Shape(
                "the tool scratch directory is not private".into(),
            ));
        }
        Ok(partial)
    }

    /// The hash a version in the cache was verified against, if it is there whole.
    fn hash_held(&self, target: &Path) -> Result<Option<String>, ToolError> {
        match std::fs::read_to_string(target.join(COMPLETE_MARKER)) {
            Ok(text) => Ok(Some(text.trim().to_owned())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(io("reading the marker")(error)),
        }
    }

    async fn download(
        &self,
        client: &reqwest::Client,
        url: &reqwest::Url,
        into: &Path,
        cancel: &CancellationToken,
        progress: &mut DownloadReport<'_>,
    ) -> Result<String, ToolError> {
        self.require_fetch()?;
        // The address is shown without its query: a signed address carries a secret there.
        let shown = format!(
            "{}://{}{}",
            url.scheme(),
            url.host_str().unwrap_or_default(),
            url.path()
        );
        let reply = tokio::select! {
            reply = client.get(url.clone()).send() => reply
                .map_err(|error| ToolError::Download(format!("{shown}: {}", error.without_url())))?,
            () = cancel.cancelled() => return Err(ToolError::Cancelled),
        };
        if !reply.status().is_success() {
            return Err(ToolError::Download(format!(
                "{shown} answered {}",
                reply.status()
            )));
        }
        progress.total = reply.content_length();
        progress.emit(None, true).await;
        let mut file = tokio::fs::File::create(into)
            .await
            .map_err(io("making the download file"))?;
        let mut hasher = Sha256::new();
        let mut size: u64 = 0;
        let mut body = reply.bytes_stream();
        loop {
            let piece = tokio::select! {
                piece = body.next() => piece,
                () = cancel.cancelled() => return Err(ToolError::Cancelled),
            };
            let Some(piece) = piece else { break };
            self.require_fetch()?;
            let piece = piece.map_err(|error| {
                ToolError::Download(format!("{shown}: {}", error.without_url()))
            })?;
            size += piece.len() as u64;
            if size > MAX_DOWNLOAD_BYTES {
                return Err(ToolError::Shape(format!(
                    "the download is over the {} GiB a tool may be",
                    MAX_DOWNLOAD_BYTES >> 30
                )));
            }
            hasher.update(&piece);
            file.write_all(&piece)
                .await
                .map_err(io("writing the download"))?;
            progress.done = size;
            progress.emit(None, false).await;
        }
        file.flush().await.map_err(io("writing the download"))?;
        Ok(hex::encode(hasher.finalize()))
    }

    fn publish(
        &self,
        content: &Path,
        target: &Path,
        name: &str,
        version: &str,
        wanted: &str,
    ) -> Result<(), ToolError> {
        if let Some(parent) = target.parent() {
            make_cache_directory(parent, 0o755).map_err(io("making the directory of the tool"))?;
        }
        // Publish without replacing any existing entry. Another request may
        // have completed between the preceding hash check and this rename.
        match rustix::fs::renameat_with(
            rustix::fs::CWD,
            content,
            rustix::fs::CWD,
            target,
            rustix::fs::RenameFlags::NOREPLACE,
        ) {
            Ok(()) => Ok(()),
            Err(rustix::io::Errno::EXIST) => match self.hash_held(target)? {
                Some(held) if held == wanted => Ok(()),
                Some(held) => Err(ToolError::Held {
                    name: name.to_owned(),
                    version: version.to_owned(),
                    held,
                    named: wanted.to_owned(),
                }),
                None => Err(ToolError::Shape(
                    "the tool cache has an incomplete entry; the operator must check it before retrying"
                        .into(),
                )),
            },
            Err(error) => Err(io("publishing the tool")(error.into())),
        }
    }

    /// Points the link `current` of the connector tool at `version`, in one step.
    fn point_current(&self, name: &str, version: &str) -> Result<(), ToolError> {
        if name != CONNECTOR_TOOL {
            return Ok(());
        }
        let directory = self.dir.join(CONNECTOR_TOOL);
        let temporary = directory.join(format!(".{CURRENT_LINK}.{}", orch_types::new_id()));
        std::os::unix::fs::symlink(version, &temporary).map_err(io("making the link"))?;
        std::fs::rename(&temporary, directory.join(CURRENT_LINK)).map_err(io("moving the link"))
    }
}

/// The connector's program is at the top of its directory and can be run.
fn check_connector(content: &Path) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt;
    let program = content.join(CONNECTOR_TOOL);
    let meta = std::fs::metadata(&program).map_err(|_| {
        ToolError::Shape(format!(
            "{CONNECTOR_TOOL} is not at the top of what was downloaded"
        ))
    })?;
    if !meta.is_file() {
        return Err(ToolError::Shape(format!("{CONNECTOR_TOOL} is not a file")));
    }
    if meta.permissions().mode() & 0o111 == 0 {
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(PROGRAM_MODE))
            .map_err(io("making the program executable"))?;
    }
    Ok(())
}

fn unpack(packing: Packing, download: &Path, content: &Path, name: &str) -> Result<(), ToolError> {
    make_cache_directory(content, 0o755).map_err(io("making the directory of the tool"))?;
    match packing {
        Packing::Program => {
            use std::os::unix::fs::PermissionsExt;
            let program = content.join(name);
            std::fs::rename(download, &program).map_err(io("saving the program"))?;
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(PROGRAM_MODE))
                .map_err(io("making the program executable"))
        }
        Packing::Tar => unpack_tar(
            std::fs::File::open(download).map_err(io("opening the download"))?,
            content,
        ),
        Packing::TarGz => unpack_tar(
            flate2::read::GzDecoder::new(
                std::fs::File::open(download).map_err(io("opening the download"))?,
            ),
            content,
        ),
        Packing::Zip => unpack_zip(download, content),
    }?;
    plain_modes(content)?;
    // The root is still inside the private scratch tree until publication.
    std::fs::set_permissions(content, std::fs::Permissions::from_mode(0o755))
        .map_err(io("setting the mode of the tool directory"))
}

/// The mode of a file of the cache once it is unpacked: what an owner may do with it and what anyone may read
/// and run. An archive's setuid and setgid bits, and the write bits of group and others, are not carried over:
/// a tool is a program to run and nothing in it should change who runs it.
fn plain_modes(directory: &Path) -> Result<(), ToolError> {
    use std::os::unix::fs::PermissionsExt;
    for entry in std::fs::read_dir(directory).map_err(io("reading the unpacked tool"))? {
        let entry = entry.map_err(io("reading the unpacked tool"))?;
        let meta =
            std::fs::symlink_metadata(entry.path()).map_err(io("reading the unpacked tool"))?;
        if meta.file_type().is_symlink() {
            continue;
        }
        let mode = meta.permissions().mode();
        let plain = if meta.is_dir() || mode & 0o111 != 0 {
            0o755
        } else {
            0o644
        };
        std::fs::set_permissions(entry.path(), std::fs::Permissions::from_mode(plain))
            .map_err(io("setting the mode of an unpacked file"))?;
        if meta.is_dir() {
            plain_modes(&entry.path())?;
        }
    }
    Ok(())
}

/// Unpacks a tar. Every path is held inside `content`: an entry that would leave it is an error, not skipped,
/// because an archive that tries is not one to trust the rest of.
fn unpack_tar(reader: impl Read, content: &Path) -> Result<(), ToolError> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    archive.set_unpack_xattrs(false);
    let (mut entries, mut bytes) = (0_usize, 0_u64);
    for entry in archive.entries().map_err(io("reading the archive"))? {
        let mut entry = entry.map_err(io("reading an entry of the archive"))?;
        entries += 1;
        bytes = bytes.saturating_add(entry.size());
        if entries > MAX_ENTRIES || bytes > MAX_UNPACKED_BYTES {
            return Err(ToolError::Shape(
                "the archive holds more than a tool can".to_owned(),
            ));
        }
        let path = entry
            .path()
            .map_err(io("reading a path of the archive"))?
            .into_owned();
        if path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            return Err(ToolError::Shape(format!(
                "the archive has the path {} which leaves the tool's directory",
                path.display()
            )));
        }
        if let Some(target) = entry
            .link_name()
            .map_err(io("reading a link of the archive"))?
        {
            let hard = entry.header().entry_type().is_hard_link();
            if !stays_inside(&path, &target, hard) {
                return Err(ToolError::Shape(format!(
                    "the archive has a link from {} to {} which leaves the tool's directory",
                    path.display(),
                    target.display()
                )));
            }
        }
        if !entry
            .unpack_in(content)
            .map_err(io("unpacking an entry of the archive"))?
        {
            return Err(ToolError::Shape(
                "the archive has an entry that leaves the tool's directory".to_owned(),
            ));
        }
    }
    Ok(())
}

/// Whether a link from `entry` to `target` stays inside the directory the archive unpacks to. A hard link's
/// target is a path from the top of the archive; a symbolic link's is from the directory of the link.
fn stays_inside(entry: &Path, target: &Path, hard: bool) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth: i64 = if hard {
        0
    } else {
        entry.parent().map_or(0, |parent| {
            i64::try_from(
                parent
                    .components()
                    .filter(|part| matches!(part, Component::Normal(_)))
                    .count(),
            )
            .unwrap_or(i64::MAX)
        })
    };
    for part in target.components() {
        match part {
            Component::Normal(_) => depth += 1,
            Component::CurDir => {}
            Component::ParentDir => {
                depth -= 1;
                if depth < 0 {
                    return false;
                }
            }
            Component::RootDir | Component::Prefix(_) => return false,
        }
    }
    true
}

fn unpack_zip(download: &Path, content: &Path) -> Result<(), ToolError> {
    let file = std::fs::File::open(download).map_err(io("opening the download"))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|error| ToolError::Shape(format!("the archive cannot be read: {error}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(ToolError::Shape(
            "the archive holds more than a tool can".to_owned(),
        ));
    }
    let mut bytes = 0_u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| ToolError::Shape(format!("the archive cannot be read: {error}")))?;
        bytes = bytes.saturating_add(entry.size());
        if bytes > MAX_UNPACKED_BYTES {
            return Err(ToolError::Shape(
                "the archive holds more than a tool can".to_owned(),
            ));
        }
        if entry.enclosed_name().is_none() {
            return Err(ToolError::Shape(
                "the archive has a path that leaves the tool's directory".to_owned(),
            ));
        }
    }
    archive
        .extract(content)
        .map_err(|error| ToolError::Shape(format!("the archive cannot be unpacked: {error}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn competing_publications_never_replace_a_completed_entry() {
        use std::sync::{Arc, Barrier};
        let parent = tempfile::tempdir().unwrap();
        let cache = ToolCache::open(&NodeSettings::in_dir(parent.path())).unwrap();
        let target = cache.path_of("codex", "1");
        let barrier = Arc::new(Barrier::new(2));
        let mut workers = Vec::new();
        for label in ['a', 'b'] {
            let source = parent.path().join(format!("source-{label}"));
            std::fs::create_dir(&source).unwrap();
            let hash = label.to_string().repeat(64);
            std::fs::write(source.join(COMPLETE_MARKER), &hash).unwrap();
            std::fs::write(source.join("program"), label.to_string()).unwrap();
            let cache = cache.clone();
            let target = target.clone();
            let barrier = Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                (label, cache.publish(&source, &target, "codex", "1", &hash))
            }));
        }
        let results: Vec<_> = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect();
        assert_eq!(
            results.iter().filter(|(_, result)| result.is_ok()).count(),
            1
        );
        assert_eq!(
            results
                .iter()
                .filter(|(_, result)| matches!(result, Err(ToolError::Held { .. })))
                .count(),
            1
        );
        let winner = results.iter().find(|(_, result)| result.is_ok()).unwrap().0;
        assert_eq!(
            std::fs::read_to_string(target.join("program")).unwrap(),
            winner.to_string()
        );
        assert_eq!(
            std::fs::read_to_string(target.join(COMPLETE_MARKER)).unwrap(),
            winner.to_string().repeat(64)
        );
    }

    #[test]
    fn an_unmarked_destination_is_refused_without_removing_it() {
        let parent = tempfile::tempdir().unwrap();
        let cache = ToolCache::open(&NodeSettings::in_dir(parent.path())).unwrap();
        let target = cache.path_of("codex", "1");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("preserve"), b"old content").unwrap();
        let source = parent.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join(COMPLETE_MARKER), "a".repeat(64)).unwrap();
        assert!(matches!(
            cache.publish(&source, &target, "codex", "1", &"a".repeat(64)),
            Err(ToolError::Shape(_))
        ));
        assert_eq!(
            std::fs::read(target.join("preserve")).unwrap(),
            b"old content"
        );
        assert!(source.exists());
    }

    #[test]
    fn a_name_that_could_leave_the_cache_is_not_a_name() {
        for good in ["codex", "orch-connector", "0.160.1", "1.2.3+build.5", "a_b"] {
            assert!(is_plain_name(good), "{good}");
        }
        for bad in [
            "",
            "..",
            "../x",
            "a/b",
            ".hidden",
            "a b",
            "a..b",
            "-x",
            "x\0",
            &"a".repeat(129),
        ] {
            assert!(!is_plain_name(bad), "{bad:?}");
        }
    }

    #[test]
    fn an_address_says_how_its_content_is_packed() {
        let packing = |text: &str| packing_of(&reqwest::Url::parse(text).unwrap());
        assert_eq!(
            packing("https://x.example/codex-1.tar.gz?sig=a"),
            Packing::TarGz
        );
        assert_eq!(packing("https://x.example/codex-1.TGZ"), Packing::TarGz);
        assert_eq!(packing("https://x.example/a.tar"), Packing::Tar);
        assert_eq!(packing("https://x.example/a.zip"), Packing::Zip);
        assert_eq!(
            packing(
                "https://files.pythonhosted.org/packages/ab/cd/dsh-0.2.1rc2-py3-none-manylinux_2_28_x86_64.whl"
            ),
            Packing::Zip,
            "a wheel is a zip"
        );
        assert_eq!(
            packing("https://x.example/orch-connector"),
            Packing::Program
        );
    }

    #[test]
    fn a_link_in_an_archive_may_point_inside_the_tool_and_never_outside_it() {
        let inside = |entry: &str, target: &str, hard: bool| {
            stays_inside(Path::new(entry), Path::new(target), hard)
        };
        assert!(inside("bin/codex", "../lib/codex", false));
        assert!(inside("codex", "bin/codex", false));
        assert!(!inside("bin/codex", "../../etc/passwd", false));
        assert!(!inside("codex", "/etc/passwd", false));
        assert!(inside("a/b", "c/d", true), "from the top of the archive");
        assert!(!inside("a/b", "../c", true));
    }

    #[test]
    fn a_tool_error_is_a_command_error_with_a_code_that_says_whether_asking_again_helps() {
        let mismatch: CommandError = ToolError::HashMismatch {
            expected: "a".into(),
            found: "b".into(),
        }
        .into();
        assert_eq!(mismatch.code, "internal");
        assert!(
            mismatch.retryable,
            "a transfer that was cut may arrive whole the next time"
        );
        let held: CommandError = ToolError::Held {
            name: "codex".into(),
            version: "1".into(),
            held: "a".into(),
            named: "b".into(),
        }
        .into();
        assert_eq!(held.code, "conflict");
        assert!(!held.retryable);
        let invalid: CommandError = ToolError::Invalid("x".into()).into();
        assert_eq!(invalid.code, INVALID);
    }
}
