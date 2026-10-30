//! Where a program's builds are published, and how each build is verified.
//!
//! A channel is data: the generic readers (GitHub release, npm, PyPI and a manifest published by the vendor)
//! walk it, and nothing in Orch names the vendor. Every channel lists Linux builds only, one per processor, and
//! keeps a build only when its vendor gives something to verify it by: a SHA-256 for GitHub assets, for PyPI files
//! and for a vendor manifest, and the registry's own integrity value with its signature for npm. A build with
//! nothing to verify is never kept.
//!
//! Templates name places with braces: `{version}` is the release's version, `{tag}` its tag, `{arch}` the
//! processor in the vendor's words (see [`Platform`]), and `{file}` the asset or file being verified.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The processors a build is made for.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
pub enum Arch {
    /// Intel and AMD, 64 bit.
    #[serde(rename = "x86_64")]
    X86_64,
    /// Arm, 64 bit.
    #[serde(rename = "aarch64")]
    Aarch64,
}

impl Arch {
    /// Every processor Orch reads builds for.
    pub const ALL: [Self; 2] = [Self::X86_64, Self::Aarch64];

    /// The word the rows and the API write.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::X86_64 => "x86_64",
            Self::Aarch64 => "aarch64",
        }
    }
}

/// How a vendor writes one processor: the word its file names, tags or manifest keys use for it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Platform {
    /// The processor.
    pub arch: Arch,
    /// The vendor's word for it, such as `x86_64-unknown-linux-musl` or `linux-x64`; it replaces `{arch}`.
    pub word: String,
}

/// How the checksum of a GitHub release asset is found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "from", rename_all = "snake_case")]
pub enum GithubChecksum {
    /// The `digest` GitHub keeps on each asset (`sha256:<hex>`).
    Digest,
    /// A file in the same release: its name is a template (`{file}.sha256`, or one `SHA256SUMS` for all), and it
    /// holds `<hex>  <name>` lines or the hex alone.
    Asset {
        /// The checksum file's name.
        name: String,
    },
}

/// A program published as GitHub releases.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct GithubRelease {
    /// The channel's name in lists and logs.
    pub id: String,
    /// The repository, as `owner/name`.
    pub repository: String,
    /// The API's address; the default is GitHub's.
    #[serde(default = "github_api")]
    pub api: String,
    /// What a release's tag has before the version.
    #[serde(default)]
    pub tag_prefix: String,
    /// How many of the newest releases are read.
    #[serde(default = "listed_default")]
    pub listed: u32,
    /// Whether releases marked pre-release, and versions with a pre-release part, are kept.
    #[serde(default)]
    pub prereleases: bool,
    /// The asset of each processor: a name template.
    pub assets: Vec<PlatformName>,
    /// How an asset's checksum is found.
    pub checksum: GithubChecksum,
}

/// A name a processor's file has, as a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct PlatformName {
    /// The processor.
    pub arch: Arch,
    /// The name or pattern: `{version}` and `{tag}` stand for the release's. Patterns may use `*`.
    pub name: String,
}

/// A program published as an npm package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Npm {
    /// The channel's name in lists and logs.
    pub id: String,
    /// The package's name. A processor-specific package puts `{arch}` in it.
    pub package: String,
    /// The registry's address; the default is the public one.
    #[serde(default = "npm_registry")]
    pub registry: String,
    /// The processors, when the package name carries one; none when one package serves every processor.
    #[serde(default)]
    pub platforms: Vec<Platform>,
    /// The tag whose version is the newest; the default is `latest`.
    #[serde(default = "npm_tag")]
    pub dist_tag: String,
    /// How many of the newest versions are read besides the tag's.
    #[serde(default = "listed_default")]
    pub listed: u32,
    /// Whether a version without the registry's signature over its integrity value is refused. The default is
    /// yes; a registry that signs nothing is declared here, and the build then rests on the integrity value alone.
    #[serde(default = "yes")]
    pub signature_required: bool,
}

/// A program published to PyPI, in a wheel that carries one executable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Pypi {
    /// The channel's name in lists and logs.
    pub id: String,
    /// The project's name.
    pub project: String,
    /// The index's address; the default is the public one.
    #[serde(default = "pypi_index")]
    pub index: String,
    /// The wheel of each processor: a file name pattern, with `*` for what varies.
    pub files: Vec<PlatformName>,
    /// How many of the newest releases are read.
    #[serde(default = "listed_default")]
    pub listed: u32,
    /// Whether versions with a pre-release part are kept; yanked files never are.
    #[serde(default)]
    pub prereleases: bool,
}

/// A signature the vendor publishes beside its manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ManifestSignature {
    /// The signature scheme: only `ed25519` is read.
    pub scheme: String,
    /// The vendor's public key, in base64.
    pub public_key: String,
    /// Where the detached signature is: a template with `{manifest}` for the manifest's address.
    pub url: String,
}

/// A program whose vendor publishes, for each version, a manifest that lists the files of each platform with a
/// SHA-256, and one file per channel that names its newest version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct SignedManifest {
    /// The channel's name in lists and logs.
    pub id: String,
    /// The vendor's address under which everything below lies.
    pub base: String,
    /// The channels whose file (`{base}/{channel}`) holds a version as text; each is read.
    pub channels: Vec<String>,
    /// Where a version's manifest is: `{base}` and `{version}` are replaced.
    #[serde(default = "manifest_template")]
    pub manifest: String,
    /// The processors, by the key the manifest's `platforms` object uses for each.
    pub platforms: Vec<Platform>,
    /// The manifest member that holds the version.
    #[serde(default = "version_pointer")]
    pub version_pointer: String,
    /// The member of a platform's entry that holds its SHA-256.
    #[serde(default = "checksum_member")]
    pub checksum_member: String,
    /// The member of a platform's entry that names the file; `file` is used when it is absent.
    #[serde(default = "file_member")]
    pub file_member: String,
    /// The file's name when the entry names none.
    #[serde(default)]
    pub file_default: String,
    /// Where a platform's file is: `{base}`, `{version}`, `{arch}` (the platform's key) and `{file}`.
    #[serde(default = "file_template")]
    pub file: String,
    /// The vendor's signature over the manifest, where it publishes one; the build is then kept only when it
    /// verifies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signature: Option<ManifestSignature>,
}

/// One place a program's builds are published.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Channel {
    /// GitHub releases, with assets verified by GitHub's digest or a checksum file.
    GithubRelease(GithubRelease),
    /// An npm package, verified by the registry's integrity value and signature.
    Npm(Npm),
    /// A PyPI project, verified by the index's SHA-256.
    Pypi(Pypi),
    /// A manifest the vendor publishes.
    SignedManifest(SignedManifest),
}

impl Channel {
    /// The channel's name in lists and logs.
    #[must_use]
    pub fn id(&self) -> &str {
        match self {
            Self::GithubRelease(channel) => &channel.id,
            Self::Npm(channel) => &channel.id,
            Self::Pypi(channel) => &channel.id,
            Self::SignedManifest(channel) => &channel.id,
        }
    }

    /// The word of the channel's type, as it is written.
    #[must_use]
    pub const fn type_word(&self) -> &'static str {
        match self {
            Self::GithubRelease(_) => "github_release",
            Self::Npm(_) => "npm",
            Self::Pypi(_) => "pypi",
            Self::SignedManifest(_) => "signed_manifest",
        }
    }

    /// Where the channel's builds are published, as a person reads it: the host and the project.
    #[must_use]
    pub fn source(&self) -> String {
        match self {
            Self::GithubRelease(channel) => format!("github.com/{}", channel.repository),
            Self::Npm(channel) => format!("{} ({})", channel.package, host_of(&channel.registry)),
            Self::Pypi(channel) => format!("{} ({})", channel.project, host_of(&channel.index)),
            Self::SignedManifest(channel) => host_of(&channel.base).to_owned(),
        }
    }

    /// Every address the channel reads from, so a mirror and a network policy can be told what to expect.
    #[must_use]
    pub fn origins(&self) -> Vec<&str> {
        match self {
            Self::GithubRelease(channel) => vec![channel.api.as_str()],
            Self::Npm(channel) => vec![channel.registry.as_str()],
            Self::Pypi(channel) => vec![channel.index.as_str()],
            Self::SignedManifest(channel) => vec![channel.base.as_str()],
        }
    }

    /// Whether one named version can be fetched on its own. Every kind of channel can: a tag, a version of
    /// the package, a release of the project, a manifest of that version.
    #[must_use]
    pub const fn exact_fetch(&self) -> bool {
        true
    }

    /// What is wrong with the channel's own data, in words for the author of the manifest.
    #[must_use]
    pub fn faults(&self) -> Vec<String> {
        let mut faults = Vec::new();
        let place = |label: &str, value: &str| {
            if value.trim().is_empty() {
                Some(format!("channel {}: {label} is empty", self.id()))
            } else {
                None
            }
        };
        faults.extend(place("id", self.id()));
        let mut address = |label: &str, value: &str| {
            if !(value.starts_with("https://") || value.starts_with("http://")) {
                faults.push(format!(
                    "channel {}: {label} is not an http address",
                    self.id()
                ));
            }
        };
        match self {
            Self::GithubRelease(channel) => {
                address("api", &channel.api);
                if channel
                    .repository
                    .split('/')
                    .filter(|part| !part.is_empty())
                    .count()
                    != 2
                {
                    faults.push(format!(
                        "channel {}: the repository is owner/name",
                        channel.id
                    ));
                }
                if channel.assets.is_empty() {
                    faults.push(format!("channel {}: no asset is named", channel.id));
                }
            }
            Self::Npm(channel) => {
                address("registry", &channel.registry);
                if channel.package.is_empty() {
                    faults.push(format!("channel {}: no package is named", channel.id));
                }
                if channel.package.contains("{arch}") && channel.platforms.is_empty() {
                    faults.push(format!(
                        "channel {}: the package names a processor and lists none",
                        channel.id
                    ));
                }
            }
            Self::Pypi(channel) => {
                address("index", &channel.index);
                if channel.files.is_empty() {
                    faults.push(format!("channel {}: no file is named", channel.id));
                }
            }
            Self::SignedManifest(channel) => {
                address("base", &channel.base);
                if channel.channels.is_empty() {
                    faults.push(format!("channel {}: no channel is named", channel.id));
                }
                if channel.platforms.is_empty() {
                    faults.push(format!("channel {}: no platform is named", channel.id));
                }
                if let Some(signature) = &channel.signature
                    && signature.scheme != "ed25519"
                {
                    faults.push(format!(
                        "channel {}: only ed25519 signatures are read",
                        channel.id
                    ));
                }
            }
        }
        faults
    }
}

/// The host of an address, or the address when it has none.
fn host_of(address: &str) -> &str {
    address
        .split_once("://")
        .map_or(address, |(_, rest)| rest)
        .split('/')
        .next()
        .unwrap_or(address)
}

fn github_api() -> String {
    "https://api.github.com".to_owned()
}

/// How many of the newest releases a reader looks at: twenty reach back well past the build a release ships.
const LISTED_DEFAULT: u32 = 20;

fn listed_default() -> u32 {
    LISTED_DEFAULT
}

fn npm_registry() -> String {
    "https://registry.npmjs.org".to_owned()
}

fn npm_tag() -> String {
    "latest".to_owned()
}

fn pypi_index() -> String {
    "https://pypi.org".to_owned()
}

fn yes() -> bool {
    true
}

fn manifest_template() -> String {
    "{base}/{version}/manifest.json".to_owned()
}

fn version_pointer() -> String {
    "/version".to_owned()
}

fn checksum_member() -> String {
    "checksum".to_owned()
}

fn file_member() -> String {
    "binary".to_owned()
}

fn file_template() -> String {
    "{base}/{version}/{arch}/{file}".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_channel_is_written_with_its_type_and_reads_its_defaults() {
        let channel: Channel = serde_json::from_value(json!({
            "type": "pypi", "id": "pypi", "project": "example-runtime",
            "files": [{ "arch": "x86_64", "name": "*manylinux*_x86_64.whl" }]
        }))
        .unwrap();
        let Channel::Pypi(pypi) = &channel else {
            panic!("not a PyPI channel");
        };
        assert_eq!(pypi.index, "https://pypi.org");
        assert_eq!(pypi.listed, 20);
        assert_eq!(channel.type_word(), "pypi");
        assert_eq!(channel.source(), "example-runtime (pypi.org)");
        assert!(channel.faults().is_empty());
    }

    #[test]
    fn a_channel_that_cannot_work_says_why() {
        let channel: Channel = serde_json::from_value(json!({
            "type": "github_release", "id": "gh", "repository": "nobody", "assets": [],
            "checksum": { "from": "digest" }
        }))
        .unwrap();
        let faults = channel.faults();
        assert!(faults.iter().any(|fault| fault.contains("owner/name")));
        assert!(faults.iter().any(|fault| fault.contains("no asset")));
    }
}
