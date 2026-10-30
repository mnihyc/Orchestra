//! Release manifest types and validation shared by packaging and a read-only version check.

use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The ordinary release manifest name.
pub const MANIFEST_FILE: &str = "version.json";
/// The first supported manifest format.
pub const SCHEMA_VERSION: u32 = 1;
/// A release of this product names this project.
pub const PROJECT: &str = "orch";

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

/// Whether `version` is `MAJOR.MINOR.PATCH` with an optional pre-release
/// suffix, and nothing else: a release is identified by its tag alone.
#[must_use]
pub fn is_release_version(version: &str) -> bool {
    semver::Version::parse(version).is_ok_and(|parsed| parsed.build.is_empty())
}

fn is_hex(text: &str, length: usize) -> bool {
    text.len() == length
        && text
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// The operating systems a harness build is for, as the API names them.
const BUILD_SYSTEMS: [&str; 3] = ["linux", "macos", "windows"];
/// The processors a harness build is for, as the API names them.
const BUILD_PROCESSORS: [&str; 2] = ["x86_64", "aarch64"];
/// How long the name or the version of a build may be: it becomes a directory of a host's tool cache, which
/// takes no more than this.
const BUILD_NAME_MAX: usize = 128;
/// The endings of an address whose content a host's tool cache cannot unpack: it would keep the archive as if
/// it were the program, and the harness would not be found in it.
const UNPACKABLE: [&str; 10] = [
    ".tar.zst", ".zst", ".tar.xz", ".xz", ".tar.bz2", ".bz2", ".7z", ".rar", ".deb", ".rpm",
];

/// Whether `text` can be a directory of a host's tool cache: a letter or a digit first, then letters, digits
/// and `.`, `_`, `+`, `-`, and no `..`.
fn is_cache_name(text: &str) -> bool {
    text.chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphanumeric())
        && text.len() <= BUILD_NAME_MAX
        && text
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '+' | '-'))
        && !text.contains("..")
}

/// What is wrong with an address a host is to download a build from, or nothing.
fn address_problem(url: &str) -> Option<&'static str> {
    let rest = url.strip_prefix("https://")?;
    let (authority, tail) = rest.split_once('/').unwrap_or((rest, ""));
    let path = tail.split(['?', '#']).next().unwrap_or_default();
    if authority.is_empty() || authority.contains(['@', ' ']) || path.is_empty() {
        return Some("names no host or no file");
    }
    let lower = path.to_ascii_lowercase();
    if UNPACKABLE.iter().any(|ending| lower.ends_with(ending))
        || (lower.ends_with(".gz") && !lower.ends_with(".tar.gz"))
    {
        return Some(
            "is an archive of a kind a host cannot unpack (a `.tar.gz`, `.tgz`, `.tar`, `.zip` or `.whl`, or the program itself)",
        );
    }
    None
}

/// What is wrong with the harness builds a release lists, one sentence per fault. Each is what a host's tool
/// cache is told to download and keep (`kind` and `version` name its directory), so an entry that could be
/// refused there, or fetched over a channel that cannot be trusted, or that the cache would keep as a heap of
/// bytes where a program is looked for, is refused when the release is made. The hash is the only thing that
/// vouches for the file: it is 64 lowercase hexadecimal digits, and the address is `https`.
fn harness_build_problems(builds: &[serde_json::Value]) -> Vec<String> {
    let mut found = Vec::new();
    let mut identities = BTreeSet::new();
    let mut defaults: BTreeMap<(String, String, String), u32> = BTreeMap::new();
    for (index, build) in builds.iter().enumerate() {
        let text = |member: &str| build.get(member).and_then(serde_json::Value::as_str);
        let (kind, version, os, arch) = (
            text("kind").unwrap_or_default(),
            text("version").unwrap_or_default(),
            text("os").unwrap_or_default(),
            text("arch").unwrap_or_default(),
        );
        let label = format!("harness build {} ({kind} {version} {os} {arch})", index + 1);
        let mut fault = |what: String| found.push(format!("{label}: {what}"));
        if !build.is_object() {
            fault("is not an object".to_owned());
            continue;
        }
        for (member, value) in [("kind", kind), ("version", version)] {
            if !is_cache_name(value) {
                fault(format!(
                    "{member} {value:?} is not a name a tool cache can keep a build under"
                ));
            }
        }
        if !BUILD_SYSTEMS.contains(&os) {
            fault(format!("os {os:?} is none of {BUILD_SYSTEMS:?}"));
        }
        if !BUILD_PROCESSORS.contains(&arch) {
            fault(format!("arch {arch:?} is none of {BUILD_PROCESSORS:?}"));
        }
        let url = text("url").unwrap_or_default();
        if !url.starts_with("https://") {
            fault(format!("url {url:?} is not an https address"));
        } else if let Some(why) = address_problem(url) {
            fault(format!("url {url:?} {why}"));
        }
        if !is_hex(text("sha256").unwrap_or_default(), 64) {
            fault("sha256 is not 64 lowercase hexadecimal digits".to_owned());
        }
        if text("source") != Some("release") {
            fault("source must be \"release\": a release lists what it ships".to_owned());
        }
        let default = build.get("is_default").and_then(serde_json::Value::as_bool);
        if default.is_none() {
            fault("is_default is missing or not true or false".to_owned());
        }
        if !identities.insert((kind, version, os, arch)) {
            fault("is listed twice".to_owned());
        }
        *defaults
            .entry((kind.to_owned(), os.to_owned(), arch.to_owned()))
            .or_default() += u32::from(default == Some(true));
    }
    for ((kind, os, arch), count) in defaults {
        if count != 1 {
            found.push(format!(
                "{kind} for {os} {arch} has {count} default builds, and each harness has exactly one for a system and processor"
            ));
        }
    }
    found
}

impl Manifest {
    /// What is wrong with the manifest itself, one sentence per fault; empty
    /// when it is well formed. It looks at no file.
    #[must_use]
    pub fn problems(&self) -> Vec<String> {
        let mut found = Vec::new();
        let mut fault = |text: String| found.push(text);
        if self.schema_version != SCHEMA_VERSION {
            fault(format!(
                "schema_version is {}, and this reads {SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        if self.project != PROJECT {
            fault(format!(
                "project is {:?}, and a release of Orch says {PROJECT:?}",
                self.project
            ));
        }
        if !is_release_version(&self.version) {
            fault(format!(
                "version {:?} is not MAJOR.MINOR.PATCH with an optional pre-release",
                self.version
            ));
        }
        if self.tag != format!("v{}", self.version) {
            fault(format!(
                "tag {:?} is not v and the version {:?}",
                self.tag, self.version
            ));
        }
        if !is_hex(&self.commit, 40) {
            fault(format!(
                "commit {:?} is not 40 lowercase hexadecimal digits",
                self.commit
            ));
        }
        if self.link_protocol.minimum == 0
            || self.link_protocol.minimum > self.link_protocol.current
        {
            fault(format!(
                "link_protocol minimum {} is not between 1 and the current {}",
                self.link_protocol.minimum, self.link_protocol.current
            ));
        }
        if !is_release_version(&self.connector_minimum) {
            fault(format!(
                "connector_minimum {:?} is not a release version",
                self.connector_minimum
            ));
        }
        if self.assets.is_empty() {
            fault("assets is empty".to_owned());
        }
        let mut seen = BTreeSet::new();
        for asset in &self.assets {
            let name = asset.name.as_str();
            if name.is_empty()
                || name == MANIFEST_FILE
                || name.contains(['/', '\\'])
                || name.starts_with('.')
            {
                fault(format!("asset name {name:?} is not a plain file name"));
            }
            if !seen.insert(name) {
                fault(format!("asset {name:?} is listed twice"));
            }
            if asset.download_url.is_empty() {
                fault(format!("asset {name:?} has no download_url"));
            }
            if asset.size == 0 {
                fault(format!("asset {name:?} has size 0"));
            }
            if !is_hex(&asset.blake3, 64) {
                fault(format!(
                    "asset {name:?} has a blake3 that is not 64 lowercase hexadecimal digits"
                ));
            }
            if let Some(sha256) = &asset.sha256
                && !is_hex(sha256, 64)
            {
                fault(format!(
                    "asset {name:?} has a sha256 that is not 64 lowercase hexadecimal digits"
                ));
            }
        }
        for image in &self.images {
            let digest = image.digest.strip_prefix("sha256:").unwrap_or_default();
            if image.name.is_empty() || image.reference.is_empty() || !is_hex(digest, 64) {
                fault(format!(
                    "image {:?} needs a name, a reference and a sha256 digest",
                    image.name
                ));
            }
        }
        found.extend(harness_build_problems(&self.harness_builds));
        found
    }
}
