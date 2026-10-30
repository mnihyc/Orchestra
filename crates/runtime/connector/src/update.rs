//! Verified atomic update and idle policy.
//!
//! A connector on a person's machine takes its new build from its own system, as the installers do: the system's
//! `version.json` lists, for each platform, the address of its connector with its size and SHA-256. The build is
//! downloaded beside the program, checked against that listing (its size and its SHA-256, both, before anything
//! uses it) and put in the program's place by one rename, so that the program is never half written; on Windows,
//! where a running program cannot be replaced, the running file is renamed `.old` first and removed at the next
//! start. The service manager then starts the new program. A connector of a runtime the system made does not update
//! itself: its host's node brings its builds. A newer build is taken only: a listing that is older than the running
//! build, or the same, changes nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use url::Url;

use crate::https::{HttpsClient, HttpsError};

/// Where a system lists its release for the machines.
pub const LISTING_PATH: &str = "/api/v1/downloads/version.json";
/// How long the download of a build may take: a connector is some tens of MiB, which a slow link of the link's
/// own measure carries within this.
pub const DOWNLOAD_LIMIT: Duration = Duration::from_secs(30 * 60);
/// How long reading the listing may take.
pub const LISTING_LIMIT: Duration = Duration::from_secs(30);
/// The most a connector build may be: well above any build, and a bound on what a wrong answer can fill.
pub const MAX_BUILD_BYTES: u64 = 512 * 1024 * 1024;
/// The name the downloaded build has in the data directory until it replaces the program.
pub const STAGED_NAME: &str = "orch-connector.new";

/// The physical program to update. Managed unit connectors never select either of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Program {
    /// A person's connector.
    Connector,
    /// A host's node.
    Node,
}

impl Program {
    /// The command a local operator can run even with update.auto disabled.
    #[must_use]
    pub const fn command(self) -> &'static str {
        match self {
            Self::Connector => "orch-connector update",
            Self::Node => "orch-node update",
        }
    }

    /// The single download route for this component and platform.
    pub fn download_path(self, os: &str, arch: &str) -> Result<String, UpdateError> {
        match self {
            Self::Connector => Ok(format!("/api/v1/downloads/connector/{os}/{arch}")),
            Self::Node if os == "linux" => Ok(format!("/api/v1/downloads/node/{arch}")),
            Self::Node => Err(UpdateError::Listing(format!(
                "this release has no node for {os} on {arch}"
            ))),
        }
    }
}

/// Why an update did not take place.
#[derive(Debug, thiserror::Error)]
pub enum UpdateError {
    /// The system could not be asked.
    #[error(transparent)]
    Https(#[from] HttpsError),
    /// The system's answer is not what an update needs.
    #[error("{0}")]
    Listing(String),
    /// The download is not the build the listing names.
    #[error("{0}")]
    Mismatch(String),
    /// A file could not be written, renamed or removed.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What failed.
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> UpdateError + '_ {
    move |source| UpdateError::Io {
        path: path.to_owned(),
        source,
    }
}

/// The build a system offers this platform.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// Its release.
    pub version: String,
    /// The path on the system it is downloaded from.
    pub download_path: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its SHA-256, in lowercase hexadecimal.
    pub sha256: String,
}

/// The operating system and the processor of this build, as the downloads name them.
#[must_use]
pub const fn platform() -> (&'static str, &'static str) {
    let os = if cfg!(target_os = "macos") {
        "macos"
    } else if cfg!(target_os = "windows") {
        "windows"
    } else {
        "linux"
    };
    let arch = if cfg!(target_arch = "aarch64") {
        "aarch64"
    } else {
        "x86_64"
    };
    (os, arch)
}

/// The connector for `os` and `arch` in a system's listing.
pub fn offer_in(listing: &serde_json::Value, os: &str, arch: &str) -> Result<Offer, UpdateError> {
    offer_for(listing, Program::Connector, os, arch)
}

/// Selects a connector or node without ever accepting an address outside this system.
pub fn offer_for(
    listing: &serde_json::Value,
    program: Program,
    os: &str,
    arch: &str,
) -> Result<Offer, UpdateError> {
    if listing.get("project").and_then(serde_json::Value::as_str) != Some("orch") {
        return Err(UpdateError::Listing(
            "the answer is not the release listing of an Orch system".to_owned(),
        ));
    }
    let version = listing
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| UpdateError::Listing("the listing names no release".to_owned()))?
        .to_owned();
    let wanted = program.download_path(os, arch)?;
    let asset = listing
        .get("assets")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .find(|asset| {
            asset
                .get("download_url")
                .and_then(serde_json::Value::as_str)
                == Some(wanted.as_str())
        })
        .ok_or_else(|| {
            UpdateError::Listing(format!(
                "release {version} of this system has no connector for {os} on {arch}"
            ))
        })?;
    let size = asset
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| {
            UpdateError::Listing("the listing gives the connector no size".to_owned())
        })?;
    let sha256 = asset
        .get("sha256")
        .and_then(serde_json::Value::as_str)
        .filter(|hash| hash.len() == 64 && hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .ok_or_else(|| {
            UpdateError::Listing(format!(
                "release {version} lists no SHA-256 for the connector, which an update checks"
            ))
        })?
        .to_ascii_lowercase();
    if size == 0 || size > MAX_BUILD_BYTES {
        return Err(UpdateError::Listing(
            "the offered program's size is outside the bounded download size".into(),
        ));
    }
    Ok(Offer {
        version,
        download_path: wanted,
        size,
        sha256,
    })
}

/// Whether two canonical semantic releases place the offered build after the running one.
#[must_use]
pub fn newer(offered: &str, running: &str) -> bool {
    match (
        semver::Version::parse(offered.trim_start_matches('v')),
        semver::Version::parse(running.trim_start_matches('v')),
    ) {
        (Ok(offered), Ok(running)) => offered > running,
        _ => false,
    }
}

/// Reads the system's listing at `base` (its address) and the build it offers this platform.
pub async fn fetch_offer(client: &HttpsClient, base: &Url) -> Result<Offer, UpdateError> {
    fetch_offer_for(client, base, Program::Connector).await
}

/// Reads the same checked listing for either physical program.
pub async fn fetch_offer_for(
    client: &HttpsClient,
    base: &Url,
    program: Program,
) -> Result<Offer, UpdateError> {
    let reply = client
        .send(
            base,
            hyper::Method::GET,
            LISTING_PATH,
            &[],
            bytes::Bytes::new(),
            LISTING_LIMIT,
        )
        .await?;
    if !reply.status.is_success() {
        return Err(UpdateError::Listing(format!(
            "{LISTING_PATH} answered {}: was the system started from a release?",
            reply.status
        )));
    }
    let listing: serde_json::Value = serde_json::from_slice(&reply.body)
        .map_err(|error| UpdateError::Listing(format!("the listing does not read: {error}")))?;
    let (os, arch) = platform();
    offer_for(&listing, program, os, arch)
}

/// The identity/trust selection shared by local commands and automatic updates of both programs.
pub async fn inspect(
    dir: &Path,
    program: Program,
) -> Result<(HttpsClient, Url, Offer), UpdateError> {
    use orch_pki::verification::{ServerTrust, machine_client_config};
    use std::sync::Arc;
    let failure = |error: String| UpdateError::Listing(error);
    let data = crate::enrollment::DataDir::open(dir).map_err(|e| failure(e.to_string()))?;
    let record = data.record().map_err(|e| failure(e.to_string()))?;
    let config = data
        .config(|name| std::env::var(name).ok())
        .map_err(|e| failure(e.to_string()))?;
    let is_node =
        crate::enrollment::roles_of(&config).contains(&orch_link_contract::session::Role::Node);
    if record.managed && !is_node {
        return Err(failure(
            "a managed runtime takes its connector through its environment or provider Update"
                .into(),
        ));
    }
    if is_node != (program == Program::Node) {
        return Err(failure(
            "the enrolled roles do not belong to this updater".into(),
        ));
    }
    let identity = data.identity().map_err(|e| failure(e.to_string()))?;
    let clock: Arc<dyn orch_types::Clock> = Arc::new(orch_types::SystemClock);
    let endpoint = orch_link::machine::last_endpoint(data.root()).map_err(|error| {
        failure(format!(
            "the machine has no verified connection address: {error}"
        ))
    })?;
    let base = crate::https::https_base(&endpoint.url)?;
    let pinned = endpoint.trust == orch_link_contract::enrollment::Trust::Pinned;
    let tls_identity = identity.tls().map_err(|error| failure(error.to_string()))?;
    let client = HttpsClient::new(
        machine_client_config(
            Some(ServerTrust::Authority(Box::new(identity.authority.clone()))),
            if pinned {
                orch_pki::certificates::ServiceName::Gateway
            } else {
                orch_pki::certificates::ServiceName::Frontend
            },
            pinned.then_some(&tls_identity),
            clock,
        )
        .map_err(|error| failure(error.to_string()))?,
    );
    let offer = fetch_offer_for(&client, &base, program).await?;
    Ok((client, base, offer))
}

/// Both executable compositions use this verified transport and atomic installer.
pub struct ProgramUpdater {
    /// Kept before a replacement can change current_exe's answer.
    pub program_path: PathBuf,
    dir: PathBuf,
    version: String,
    program: Program,
}

impl ProgramUpdater {
    /// Captures the currently running executable, without selecting a download yet.
    pub fn new(dir: PathBuf, version: &str, program: Program) -> Result<Self, UpdateError> {
        let program_path = std::env::current_exe().map_err(io(&dir))?;
        Ok(Self {
            program_path,
            dir,
            version: version.to_owned(),
            program,
        })
    }
}

#[async_trait::async_trait]
impl crate::automatic::Updates for ProgramUpdater {
    fn running_version(&self) -> &str {
        &self.version
    }
    async fn prepare(&self) -> Result<Option<crate::automatic::Prepared>, UpdateError> {
        let (client, base, offer) = inspect(&self.dir, self.program).await?;
        if !newer(&offer.version, &self.version) {
            return Ok(None);
        }
        crate::automatic::prepare(&client, &base, offer, &self.dir.join("cache"))
            .await
            .map(Some)
    }
    fn install(&self, prepared: &crate::automatic::Prepared) -> Result<(), UpdateError> {
        install_verified(&self.program_path, &prepared.path, &prepared.offer)
    }
}

/// Rechecks staged bytes immediately before replacement, shared by both programs and both entry points.
/// The download's streamed check is still performed; this also catches a changed staging file.
pub fn install_verified(program: &Path, staged: &Path, offer: &Offer) -> Result<(), UpdateError> {
    install_inner(program, staged, Some(offer))
}

fn verify_staged(staged: &Path, offer: &Offer) -> Result<(), UpdateError> {
    use sha2::{Digest, Sha256};
    use std::io::Read;
    let mut file = std::fs::File::open(staged).map_err(io(staged))?;
    if file.metadata().map_err(io(staged))?.len() != offer.size {
        return Err(UpdateError::Mismatch(
            "the staged program does not match the listing's size; the running program is kept"
                .into(),
        ));
    }
    let mut hash = Sha256::new();
    // 64 KiB bounds memory and matches the download route's file hashing chunk.
    let mut chunk = [0_u8; 64 * 1024];
    loop {
        let size = file.read(&mut chunk).map_err(io(staged))?;
        if size == 0 {
            break;
        }
        hash.update(&chunk[..size]);
    }
    if hex::encode(hash.finalize()) != offer.sha256 {
        return Err(UpdateError::Mismatch(
            "the staged program does not match the listing's SHA-256; the running program is kept"
                .into(),
        ));
    }
    Ok(())
}

/// Downloads the build `offer` names into `dir` and checks it against the listing; gives where it is, ready to
/// replace the program. A build that does not match is removed.
pub async fn download(
    client: &HttpsClient,
    base: &Url,
    offer: &Offer,
    dir: &Path,
) -> Result<PathBuf, UpdateError> {
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    let staged = dir.join(STAGED_NAME);
    let (status, _, size, sha256) = client
        .download(
            base,
            &offer.download_path,
            &staged,
            MAX_BUILD_BYTES.min(offer.size.saturating_add(1)),
            DOWNLOAD_LIMIT,
        )
        .await?;
    let refuse = |why: String| {
        let _ = std::fs::remove_file(&staged);
        Err(UpdateError::Mismatch(why))
    };
    if !status.is_success() {
        return refuse(format!("{} answered {status}", offer.download_path));
    }
    if size != offer.size {
        return refuse(format!(
            "the download is {size} bytes and the listing says {}: it was cut or changed on the way",
            offer.size
        ));
    }
    if sha256 != offer.sha256 {
        return refuse(
            "the download does not have the SHA-256 the listing gives: it was damaged or changed on the way"
                .to_owned(),
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .map_err(io(&staged))?;
    }
    Ok(staged)
}

/// The name the running program has on Windows while its replacement takes its place.
fn old_of(program: &Path) -> PathBuf {
    let mut name = program.as_os_str().to_owned();
    name.push(".old");
    PathBuf::from(name)
}

/// Puts `staged` in the place of `program` in one rename, so that whoever starts the program finds either the
/// old build or the new one whole. On Windows the running program is renamed `.old` first.
pub fn install(program: &Path, staged: &Path) -> Result<(), UpdateError> {
    install_inner(program, staged, None)
}

fn install_inner(program: &Path, staged: &Path, offer: Option<&Offer>) -> Result<(), UpdateError> {
    if staged == program {
        return Err(UpdateError::Listing(
            "the staged program must be separate from the running executable".into(),
        ));
    }
    let parent = program.parent().unwrap_or_else(|| Path::new("."));
    let mut lock_name = program.as_os_str().to_owned();
    lock_name.push(".update-lock");
    let lock_path = PathBuf::from(lock_name);
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&lock_path)
        .map_err(io(&lock_path))?;
    lock.try_lock().map_err(|error|UpdateError::Listing(format!("another update is installing this program, or its install lock could not be taken: {error}")))?;
    // A unique same-filesystem file prevents manual/automatic copies from sharing partial bytes.
    // Keep the lock until the Windows old/new sequence has completed as well.
    let pending = tempfile::Builder::new()
        .prefix(".orch-update-")
        .tempfile_in(parent)
        .map_err(io(parent))?;
    std::fs::copy(staged, pending.path()).map_err(io(pending.path()))?;
    if let Some(offer) = offer {
        verify_staged(pending.path(), offer)?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(pending.path(), std::fs::Permissions::from_mode(0o755))
            .map_err(io(pending.path()))?;
    }
    pending.as_file().sync_all().map_err(io(pending.path()))?;
    // Close the staged file handle before the Windows rename; the owned path cleans up on failure.
    let beside = pending.into_temp_path();
    if cfg!(windows) {
        let old = old_of(program);
        let _ = std::fs::remove_file(&old);
        std::fs::rename(program, &old).map_err(io(program))?;
        if let Err(source) = std::fs::rename(&beside, program) {
            // The program is put back where the new build could not go.
            let _ = std::fs::rename(&old, program);
            return Err(UpdateError::Io {
                path: program.to_owned(),
                source,
            });
        }
    } else {
        std::fs::rename(&beside, program).map_err(io(program))?;
    }
    let _ = std::fs::remove_file(staged);
    Ok(())
}

/// Removes what an update on Windows left of the program it replaced; nothing where there is none.
pub fn remove_old(program: &Path) {
    let _ = std::fs::remove_file(old_of(program));
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn listing() -> serde_json::Value {
        json!({
            "project": "orch",
            "version": "1.4.0",
            "assets": [
                { "name": "orch-connector-linux-x86_64-musl", "download_url": "/api/v1/downloads/connector/linux/x86_64",
                  "size": 7, "blake3": "00", "sha256": "AB".repeat(32) },
                { "name": "orch-connector-macos-aarch64", "download_url": "/api/v1/downloads/connector/macos/aarch64",
                  "size": 9, "blake3": "00" }
            ]
        })
    }

    #[test]
    fn the_listing_gives_the_platforms_build_with_its_size_and_sha256() {
        let offer = offer_in(&listing(), "linux", "x86_64").unwrap();
        assert_eq!(offer.version, "1.4.0");
        assert_eq!(
            offer.download_path,
            "/api/v1/downloads/connector/linux/x86_64"
        );
        assert_eq!(offer.size, 7);
        assert_eq!(offer.sha256, "ab".repeat(32));
        assert!(
            offer_in(&listing(), "macos", "aarch64").is_err(),
            "a build without a SHA-256 is not taken"
        );
        assert!(offer_in(&listing(), "windows", "x86_64").is_err());
        assert!(offer_in(&json!({ "version": "1.4.0" }), "linux", "x86_64").is_err());
    }

    #[test]
    fn only_a_later_release_is_newer() {
        assert!(newer("1.4.0", "1.3.9"));
        assert!(newer("1.10.0", "1.9.0"));
        assert!(newer("1.4.0", "1.4.0-rc.2"));
        assert!(!newer("1.4.0", "1.4.0"));
        assert!(!newer("1.3.0", "1.4.0"));
        assert!(!newer("1.4.0-rc.1", "1.4.0"));
        assert!(!newer("v2", "1.99.99"));
        assert!(newer("1.4.0-rc.10", "1.4.0-rc.2"));
    }

    #[test]
    fn a_node_with_the_wrong_hash_cannot_replace_the_running_program() {
        use sha2::{Digest, Sha256};
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("orch-node");
        let staged = dir.path().join("orch-node.new");
        std::fs::write(&program, b"old node").unwrap();
        std::fs::write(&staged, b"bad node").unwrap();
        let listing = json!({"project":"orch","version":"1.4.0","assets":[{
            "download_url":"/api/v1/downloads/node/aarch64","size":8,
            "sha256":hex::encode(Sha256::digest(b"new node"))
        }]});
        let offer = offer_for(&listing, Program::Node, "linux", "aarch64").unwrap();
        assert!(matches!(
            install_verified(&program, &staged, &offer),
            Err(UpdateError::Mismatch(_))
        ));
        assert_eq!(std::fs::read(&program).unwrap(), b"old node");
        std::fs::write(&staged, b"cut").unwrap();
        assert!(matches!(
            install_verified(&program, &staged, &offer),
            Err(UpdateError::Mismatch(_))
        ));
        assert_eq!(std::fs::read(&program).unwrap(), b"old node");
        std::fs::write(&staged, b"new node").unwrap();
        install_verified(&program, &staged, &offer).unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"new node");
        assert!(offer_for(&listing, Program::Connector, "linux", "aarch64").is_err());
        assert!(offer_for(&listing, Program::Node, "windows", "aarch64").is_err());
    }

    #[test]
    fn a_build_takes_the_programs_place_whole() {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("bin").join("orch-connector");
        std::fs::create_dir_all(program.parent().unwrap()).unwrap();
        std::fs::write(&program, b"old build").unwrap();
        let staged = dir.path().join("data").join(STAGED_NAME);
        std::fs::create_dir_all(staged.parent().unwrap()).unwrap();
        std::fs::write(&staged, b"new build").unwrap();
        install(&program, &staged).unwrap();
        assert_eq!(std::fs::read(&program).unwrap(), b"new build");
        assert!(!staged.exists(), "the staged build is gone");
        remove_old(&program);
    }
}
