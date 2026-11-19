//! Release metadata and asset hashes.
//!
//! `version.json` names a release and lists every file that belongs to it with
//! its size and BLAKE3 hash, so that what an operator downloads, or is handed on
//! a disk, can be checked before anything is installed. This module is the one
//! place that writes the manifest and the one place that reads it again: the
//! packaging command validates what it wrote with the same code an updater's
//! check goes by.

use crate::release::ReleaseError;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

/// The file that names a release and its assets.
pub const MANIFEST_FILE: &str = "version.json";
/// The version of the manifest's own format.
pub const SCHEMA_VERSION: u32 = 1;
/// The project a manifest belongs to; a file of another project is refused.
pub const PROJECT: &str = "orch";
/// The oldest connector release this release accepts. The gateway announces the
/// same figure to the connectors that reach it; both are raised together, when a
/// release stops speaking to a connector.
pub const CONNECTOR_MINIMUM: &str = "0.1.0";
/// How much of a file is read at a time while hashing: large enough that the
/// disk, not the loop, is the limit.
const HASH_CHUNK_BYTES: usize = 1 << 20;

/// The oldest and the newest link protocol a release speaks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LinkProtocol {
    /// The newest version the release speaks.
    pub current: u32,
    /// The oldest version it still speaks.
    pub minimum: u32,
}

/// One file of a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    /// The file's name, as it lies beside `version.json`.
    pub name: String,
    /// Where it is fetched from; a relative address is read against the
    /// address `version.json` itself was fetched from.
    pub download_url: String,
    /// Its size in bytes.
    pub size: u64,
    /// Its BLAKE3 hash, in lowercase hexadecimal.
    pub blake3: String,
    /// Its SHA-256 hash, in lowercase hexadecimal, for the files an installer fetches from a
    /// system (`connectors/version.json`): a POSIX shell and PowerShell have a SHA-256 tool
    /// everywhere and a BLAKE3 one almost nowhere. Absent from the other listings.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

/// An image that belongs to a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Image {
    /// What the image is for: `orch-env-base`.
    pub name: String,
    /// Where it is pulled from, or the local tag it was built under.
    pub reference: String,
    /// Its content address, `sha256:` and 64 hexadecimal digits.
    pub digest: String,
}

/// `version.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    /// The version of this format.
    pub schema_version: u32,
    /// Always `orch`.
    pub project: String,
    /// The release's tag, `v` and the version.
    pub tag: String,
    /// The release's version.
    pub version: String,
    /// The commit it was built from, 40 hexadecimal digits.
    pub commit: String,
    /// The link protocol the release speaks.
    pub link_protocol: LinkProtocol,
    /// The oldest connector release it accepts.
    pub connector_minimum: String,
    /// Every file of the release but this one.
    pub assets: Vec<Asset>,
    /// The images that belong to it.
    pub images: Vec<Image>,
    /// The pinned builds of the harnesses, as `harness-builds.json` holds them.
    pub harness_builds: Vec<serde_json::Value>,
}

/// The names of the files a release for one architecture is made of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetNames {
    /// The server programs and what they read.
    pub server: String,
    /// The browser application.
    pub frontend: String,
    /// The operator's command line.
    pub ctl: String,
    /// The machine connector.
    pub connector: String,
    /// The node, for hosts that only run managed work.
    pub node: String,
    /// The deploy directory and the operator's pages.
    pub deploy: String,
}

impl AssetNames {
    /// The names for `arch` (`x86_64` or `aarch64`) and the release `tag`.
    #[must_use]
    pub fn for_linux(arch: &str, tag: &str) -> Self {
        Self {
            server: format!("orch-server-linux-{arch}.zip"),
            frontend: "orch-frontend-dist.tar.gz".to_owned(),
            ctl: format!("orchctl-linux-{arch}-musl"),
            connector: format!("orch-connector-linux-{arch}-musl"),
            node: format!("orch-node-linux-{arch}-musl"),
            deploy: format!("orch-deploy-{tag}.tar.gz"),
        }
    }

    /// All six, in the order a manifest lists them.
    #[must_use]
    pub fn all(&self) -> [&str; 6] {
        [
            &self.server,
            &self.frontend,
            &self.ctl,
            &self.connector,
            &self.node,
            &self.deploy,
        ]
    }
}

/// Whether `name` is a file a release can have. Anything else beside a
/// manifest is a leftover of something else, and a release refuses it.
#[must_use]
pub fn is_release_asset_name(name: &str) -> bool {
    let arch = |text: &str| matches!(text, "x86_64" | "aarch64");
    if name == "orch-frontend-dist.tar.gz" {
        return true;
    }
    if let Some(rest) = name
        .strip_prefix("orch-server-linux-")
        .and_then(|rest| rest.strip_suffix(".zip"))
    {
        return arch(rest);
    }
    for program in ["orchctl", "orch-connector", "orch-node"] {
        if let Some(rest) = name
            .strip_prefix(program)
            .and_then(|rest| rest.strip_prefix("-linux-"))
            .and_then(|rest| rest.strip_suffix("-musl"))
        {
            return arch(rest);
        }
    }
    if let Some(rest) = name.strip_prefix("orch-connector-macos-") {
        return arch(rest);
    }
    if name == "orch-connector-windows-x86_64.exe" {
        return true;
    }
    name.strip_prefix("orch-deploy-v")
        .and_then(|rest| rest.strip_suffix(".tar.gz"))
        .is_some_and(is_release_version)
}

/// Whether a semantic release has no build metadata, as the ordinary manifest requires.
#[must_use]
pub fn is_release_version(version: &str) -> bool {
    orch_build_info::release::is_release_version(version)
}

impl Manifest {
    /// What is wrong with the manifest itself, one sentence per fault; empty
    /// when it is well formed. It looks at no file.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        // Packaging and the running installation use the same whole-manifest validator.
        serde_json::to_value(self)
            .ok()
            .and_then(|value| {
                serde_json::from_value::<orch_build_info::release::Manifest>(value).ok()
            })
            .map_or_else(
                || vec!["the release manifest does not read".to_owned()],
                |manifest| manifest.problems(),
            )
    }

    /// The asset called `name`.
    #[must_use]
    pub fn asset(&self, name: &str) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == name)
    }

    /// Reads the manifest in `dir`.
    pub fn read(dir: &Path) -> Result<Self, ReleaseError> {
        let path = dir.join(MANIFEST_FILE);
        let text = fs::read_to_string(&path).map_err(|source| ReleaseError::Io {
            path: path.clone(),
            source,
        })?;
        serde_json::from_str(&text).map_err(|source| ReleaseError::Json { path, source })
    }

    /// Writes the manifest into `dir`, pretty, with a closing line break.
    pub fn write(&self, dir: &Path) -> Result<(), ReleaseError> {
        let path = dir.join(MANIFEST_FILE);
        let mut text = serde_json::to_string_pretty(self).map_err(|source| ReleaseError::Json {
            path: path.clone(),
            source,
        })?;
        text.push('\n');
        // Written beside and renamed over, so that a file the manifest is a link
        // to (a release copied with its files linked) is left as it was.
        let staging = dir.join(format!("{MANIFEST_FILE}.new"));
        fs::write(&staging, text).map_err(|source| ReleaseError::Io {
            path: staging.clone(),
            source,
        })?;
        fs::rename(&staging, &path).map_err(|source| ReleaseError::Io { path, source })
    }
}

/// The size and BLAKE3 hash of the file at `path`.
pub fn describe_file(path: &Path) -> Result<(u64, String), ReleaseError> {
    let io = |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    };
    let mut file = fs::File::open(path).map_err(io)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    let mut size = 0_u64;
    loop {
        let read = file.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        size += read as u64;
    }
    Ok((size, hasher.finalize().to_hex().to_string()))
}

/// `bytes` in lowercase hexadecimal.
fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(text, "{byte:02x}");
    }
    text
}

/// The SHA-256 hash of `bytes`, in lowercase hexadecimal.
#[must_use]
pub fn sha256_of_bytes(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    hex_of(&sha2::Sha256::digest(bytes))
}

/// The SHA-256 hash of the file at `path`, in lowercase hexadecimal.
pub fn sha256_of(path: &Path) -> Result<String, ReleaseError> {
    use sha2::Digest as _;
    let io = |source| ReleaseError::Io {
        path: path.to_owned(),
        source,
    };
    let mut file = fs::File::open(path).map_err(io)?;
    let mut hasher = sha2::Sha256::new();
    let mut buffer = vec![0_u8; HASH_CHUNK_BYTES];
    loop {
        let read = file.read(&mut buffer).map_err(io)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_of(&hasher.finalize()))
}

/// The regular files in `dir`, by name, other than the manifest.
fn files_beside_manifest(dir: &Path) -> Result<Vec<(String, PathBuf)>, ReleaseError> {
    let io = |source| ReleaseError::Io {
        path: dir.to_owned(),
        source,
    };
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).map_err(io)? {
        let entry = entry.map_err(io)?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name != MANIFEST_FILE {
            files.push((name, entry.path()));
        }
    }
    files.sort();
    Ok(files)
}

/// Lists the files of `dir` as the assets of a manifest: each with its size and
/// hash, its address being its name beside `base_url` (or relative).
pub fn assets_of(dir: &Path, base_url: Option<&str>) -> Result<Vec<Asset>, ReleaseError> {
    let mut assets = Vec::new();
    for (name, path) in files_beside_manifest(dir)? {
        let (size, blake3) = describe_file(&path)?;
        let download_url = match base_url {
            Some(base) => format!("{}/{name}", base.trim_end_matches('/')),
            None => name.clone(),
        };
        assets.push(Asset {
            name,
            download_url,
            size,
            blake3,
            sha256: None,
        });
    }
    Ok(assets)
}

/// Compares the files in `dir` with what `manifest` says of them: each asset is
/// there with the size and hash it was given, and no file is there that the
/// manifest does not list. One sentence per fault; empty when the directory is
/// the release.
pub fn verify_files(dir: &Path, manifest: &Manifest) -> Result<Vec<String>, ReleaseError> {
    let mut found = Vec::new();
    let present = files_beside_manifest(dir)?;
    for asset in &manifest.assets {
        let Some((_, path)) = present.iter().find(|(name, _)| *name == asset.name) else {
            found.push(format!("{} is listed and not there", asset.name));
            continue;
        };
        let (size, blake3) = describe_file(path)?;
        if size != asset.size {
            found.push(format!(
                "{} is {size} bytes, and the manifest says {}",
                asset.name, asset.size
            ));
        } else if blake3 != asset.blake3 {
            found.push(format!(
                "{} does not have the hash the manifest lists",
                asset.name
            ));
        }
    }
    for (name, _) in &present {
        if manifest.asset(name).is_none() {
            found.push(format!("{name} is there and not listed"));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest() -> Manifest {
        Manifest {
            schema_version: SCHEMA_VERSION,
            project: PROJECT.to_owned(),
            tag: "v0.1.0".to_owned(),
            version: "0.1.0".to_owned(),
            commit: "0123456789abcdef0123456789abcdef01234567".to_owned(),
            link_protocol: LinkProtocol {
                current: 1,
                minimum: 1,
            },
            connector_minimum: CONNECTOR_MINIMUM.to_owned(),
            assets: vec![Asset {
                name: "orch-frontend-dist.tar.gz".to_owned(),
                download_url: "orch-frontend-dist.tar.gz".to_owned(),
                size: 10,
                blake3: "a".repeat(64),
                sha256: None,
            }],
            images: vec![],
            harness_builds: vec![],
        }
    }

    #[test]
    fn the_sha256_an_installer_checks_is_the_standard_one() {
        assert_eq!(
            sha256_of_bytes(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("installer");
        fs::write(&path, b"abc").expect("a file");
        assert_eq!(sha256_of(&path).expect("hashed"), sha256_of_bytes(b"abc"));
        let mut listed = manifest();
        listed.assets[0].sha256 = Some("XYZ".to_owned());
        assert!(listed.problems().iter().any(|text| text.contains("sha256")));
        listed.assets[0].sha256 = Some(sha256_of_bytes(b"abc"));
        assert_eq!(listed.problems(), Vec::<String>::new());
    }

    #[test]
    fn a_well_formed_manifest_has_nothing_to_say() {
        assert_eq!(manifest().problems(), Vec::<String>::new());
    }

    #[test]
    fn a_tag_that_is_not_the_version_is_refused() {
        let mut wrong = manifest();
        wrong.tag = "v0.2.0".to_owned();
        assert!(wrong.problems().iter().any(|text| text.contains("tag")));
    }

    #[test]
    fn a_pre_release_is_a_release_and_build_metadata_is_not() {
        assert!(is_release_version("1.2.3-rc.1"));
        assert!(!is_release_version("1.2.3+abc"));
        assert!(!is_release_version("1.2"));
    }

    #[test]
    fn an_asset_listed_twice_or_named_with_a_path_is_refused() {
        let mut wrong = manifest();
        wrong.assets.push(wrong.assets[0].clone());
        wrong.assets[0].name = "../escape".to_owned();
        let problems = wrong.problems().join("\n");
        assert!(problems.contains("plain file name"), "{problems}");
    }

    #[test]
    fn a_hash_that_is_not_hexadecimal_is_refused() {
        let mut wrong = manifest();
        wrong.assets[0].blake3 = "XYZ".to_owned();
        assert!(wrong.problems().iter().any(|text| text.contains("blake3")));
    }

    #[test]
    fn only_the_files_of_a_release_have_the_names_of_one() {
        let names = AssetNames::for_linux("x86_64", "v0.1.0");
        for name in names.all() {
            assert!(is_release_asset_name(name), "{name}");
        }
        assert!(is_release_asset_name("orch-connector-macos-aarch64"));
        assert!(!is_release_asset_name("orch-server-linux-riscv.zip"));
        assert!(!is_release_asset_name("notes.txt"));
    }

    #[test]
    fn a_changed_unlisted_or_missing_file_is_found_against_the_manifest() {
        let dir = tempfile::tempdir().expect("a directory");
        let path = dir.path().join("orch-frontend-dist.tar.gz");
        fs::write(&path, b"0123456789").expect("a file");
        let (size, blake3) = describe_file(&path).expect("hashed");
        let mut release = manifest();
        release.assets[0].size = size;
        release.assets[0].blake3 = blake3;
        assert_eq!(
            verify_files(dir.path(), &release).expect("verified"),
            Vec::<String>::new()
        );

        fs::write(&path, b"012345678X").expect("changed");
        fs::write(dir.path().join("stray.bin"), b"x").expect("a stray file");
        let found = verify_files(dir.path(), &release)
            .expect("verified")
            .join("\n");
        assert!(found.contains("does not have the hash"), "{found}");
        assert!(
            found.contains("stray.bin is there and not listed"),
            "{found}"
        );

        fs::remove_file(&path).expect("removed");
        let found = verify_files(dir.path(), &release)
            .expect("verified")
            .join("\n");
        assert!(found.contains("is listed and not there"), "{found}");
    }

    /// A build as the release lists it.
    fn build(kind: &str, version: &str, arch: &str, url: &str) -> serde_json::Value {
        serde_json::json!({
            "kind": kind, "version": version, "os": "linux", "arch": arch, "url": url,
            "sha256": "ab".repeat(32), "is_default": true, "source": "release"
        })
    }

    fn with_builds(builds: Vec<serde_json::Value>) -> Manifest {
        Manifest {
            harness_builds: builds,
            ..manifest()
        }
    }

    #[test]
    fn the_builds_the_repository_pins_are_ones_a_host_can_download_and_keep() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(3)
            .expect("the root of the repository");
        let text = fs::read_to_string(root.join("deploy/harness-builds.json")).expect("the pins");
        let pinned: Vec<serde_json::Value> = serde_json::from_str(&text).expect("a list");
        let found = with_builds(pinned.clone()).problems();
        assert_eq!(found, Vec::<String>::new());
        for kind in ["codex", "claude-code", "dsh"] {
            assert!(
                pinned.iter().any(|build| {
                    build["kind"] == kind && build["os"] == "linux" && build["arch"] == "x86_64"
                }),
                "{kind} has no build for Linux on x86_64, which is what the bundled host runs"
            );
        }
    }

    #[test]
    fn a_release_with_sound_builds_and_one_with_none_have_nothing_to_say() {
        let sound = with_builds(vec![
            build(
                "codex",
                "0.162.1",
                "x86_64",
                "https://h.example/codex-1.tar.gz",
            ),
            build(
                "codex",
                "0.162.1",
                "aarch64",
                "https://h.example/codex-2.tgz?sig=1",
            ),
            build(
                "claude-code",
                "2.1.296",
                "x86_64",
                "https://h.example/2.1.296/claude",
            ),
        ]);
        assert_eq!(sound.problems(), Vec::<String>::new());
        assert_eq!(with_builds(vec![]).problems(), Vec::<String>::new());
    }

    #[test]
    fn a_build_is_refused_for_each_thing_a_host_could_not_do_with_it() {
        let ok = || {
            build(
                "codex",
                "0.162.1",
                "x86_64",
                "https://h.example/codex.tar.gz",
            )
        };
        let refused = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut wrong = ok();
            change(&mut wrong);
            with_builds(vec![wrong]).problems().join("\n")
        };
        assert!(refused(&|b| b["sha256"] = "XYZ".into()).contains("sha256"));
        assert!(refused(&|b| b["sha256"] = "AB".repeat(32).into()).contains("sha256"));
        assert!(refused(&|b| b["url"] = "http://h.example/codex.tar.gz".into()).contains("https"));
        assert!(
            refused(&|b| b["url"] = "https://user@h.example/codex.tar.gz".into()).contains("host")
        );
        assert!(refused(&|b| b["url"] = "https://h.example".into()).contains("no host or no file"));
        for archive in [
            "https://h.example/codex.tar.zst",
            "https://h.example/codex.zst?x=1",
            "https://h.example/codex.tar.xz",
            "https://h.example/codex.gz",
        ] {
            assert!(
                refused(&|b| b["url"] = archive.into()).contains("cannot unpack"),
                "{archive}"
            );
        }
        assert!(refused(&|b| b["kind"] = "../codex".into()).contains("tool cache"));
        assert!(refused(&|b| b["version"] = "".into()).contains("tool cache"));
        assert!(refused(&|b| b["os"] = "plan9".into()).contains("os"));
        assert!(refused(&|b| b["arch"] = "riscv".into()).contains("arch"));
        assert!(refused(&|b| b["source"] = "admin".into()).contains("source"));
        assert!(refused(&|b| b["is_default"] = "yes".into()).contains("is_default"));
        assert!(
            with_builds(vec![serde_json::json!("codex")])
                .problems()
                .join("\n")
                .contains("not an object")
        );
    }

    #[test]
    fn a_build_listed_twice_or_two_defaults_for_one_system_are_refused() {
        let first = build("codex", "0.162.1", "x86_64", "https://h.example/a.tar.gz");
        let twice = with_builds(vec![first.clone(), first.clone()])
            .problems()
            .join("\n");
        assert!(twice.contains("listed twice"), "{twice}");
        assert!(twice.contains("2 default builds"), "{twice}");
        let other = build("codex", "0.163.0", "x86_64", "https://h.example/b.tar.gz");
        let both = with_builds(vec![first.clone(), other.clone()])
            .problems()
            .join("\n");
        assert!(
            both.contains("codex for linux x86_64 has 2 default builds"),
            "{both}"
        );
        let mut not_default = other;
        not_default["is_default"] = false.into();
        assert_eq!(
            with_builds(vec![first, not_default]).problems(),
            Vec::<String>::new(),
            "a second version that is not the default is one more to choose from"
        );
        let mut none = build("codex", "0.162.1", "x86_64", "https://h.example/a.tar.gz");
        none["is_default"] = false.into();
        assert!(
            with_builds(vec![none])
                .problems()
                .join("\n")
                .contains("0 default builds")
        );
    }

    #[test]
    fn a_manifest_read_back_is_the_manifest_written() {
        let dir = tempfile::tempdir().expect("a directory");
        let written = manifest();
        written.write(dir.path()).expect("written");
        assert_eq!(Manifest::read(dir.path()).expect("read"), written);
    }
}
