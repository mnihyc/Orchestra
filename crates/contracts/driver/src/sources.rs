//! Where a unit's file system comes from: a reference, a build, an archive; and what a host offers of them.
//!
//! A container needs an image, and an image can be had in more than one way. A spec names exactly one of
//! three **sources** ([`Source`]):
//!
//! | Source | The spec holds | The host |
//! |--------|----------------|----------|
//! | `image` | A registry reference, and optionally a digest that pins it | Pulls it |
//! | `build` | A Dockerfile as text; optionally a context (an archive kept as a blob, or a Git address with a ref and a path), build arguments and a target stage | Builds it once for each content hash and keeps the result |
//! | `archive` | An image archive kept as a blob: what `docker save` writes, or an OCI layout as a tar file | Loads it once for each digest |
//!
//! Other drivers have their own sources or none: the `host` driver has no image, and an external driver
//! declares what its specs can name. A driver declares the kinds it has ([`SourceKind`]) and a host reports
//! those it offers, since an operator may turn building off; scheduling considers only hosts that offer the
//! kind a template names.

use std::collections::BTreeMap;

/// What a long operation making an image available is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ImageStep {
    /// Pulling a registry image.
    Pull,
    /// Building from a source definition.
    Build,
    /// Loading an image archive.
    Load,
}

/// Receives image progress for every implementation of the driver contract.
pub trait ImageProgress: Send + Sync {
    /// One line of progress without a trailing line break.
    fn line(&self, step: ImageStep, text: &str);
}

use orch_types::{Hash, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

/// The longest Dockerfile a build source holds: 256 KiB, which is far more than any Dockerfile and keeps a
/// template small.
pub const DOCKERFILE_MAX_BYTES: usize = 256 * 1024;

/// A way a unit's file system can be had.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// A registry reference.
    Image,
    /// A Dockerfile, built on the host.
    Build,
    /// An image archive, loaded on the host.
    Archive,
}

impl SourceKind {
    /// The stable source name used by declarations and availability reasons.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Build => "build",
            Self::Archive => "archive",
        }
    }
}

/// Where the files a build needs besides its Dockerfile come from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum BuildContext {
    /// A tar, tar.gz or zip archive kept as a blob, uploaded like any file.
    Blob {
        /// The archive's hash.
        blob: Hash,
    },
    /// A Git repository the host fetches, at a ref. Only repositories that need no credential.
    Git {
        /// The repository's address.
        git: String,
        /// The ref; a commit pins it, a branch or a tag is resolved at each build.
        #[serde(rename = "ref")]
        reference: String,
        /// The path inside it.
        path: String,
    },
}

/// A Dockerfile and what goes with it.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct BuildSource {
    /// The Dockerfile as text, at most [`DOCKERFILE_MAX_BYTES`].
    pub dockerfile: String,
    /// The other files it needs; `null` when it needs none.
    #[serde(default)]
    pub context: Option<BuildContext>,
    /// Build arguments, stored and shown in clear: never a secret.
    #[serde(default)]
    pub args: BTreeMap<String, String>,
    /// A stage to stop at.
    #[serde(default)]
    pub target: Option<String>,
}

/// An image archive.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
pub struct ArchiveSource {
    /// The archive's hash.
    pub blob: Hash,
    /// Which of several images in the archive, by its name.
    #[serde(default)]
    pub image: Option<String>,
}

/// The one source a spec names.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Source {
    /// A registry reference, with a digest that pins it when there is one (`sha256:…`).
    Image {
        /// The reference.
        reference: String,
        /// The digest.
        digest: Option<String>,
    },
    /// A Dockerfile.
    Build(BuildSource),
    /// An image archive.
    Archive(ArchiveSource),
}

/// Why a spec does not name one source.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SourceError {
    /// None of `image`, `build` and `archive` is given.
    #[error("a spec names where its file system comes from: one of image, build and archive")]
    NoSource,
    /// More than one is.
    #[error("a spec names exactly one of image, build and archive")]
    SeveralSources,
    /// A member is not the shape of its source.
    #[error("{member}: {message}")]
    Shape {
        /// The member.
        member: &'static str,
        /// What is wrong with it.
        message: String,
    },
    /// The Dockerfile is longer than [`DOCKERFILE_MAX_BYTES`].
    #[error("the Dockerfile is over {} KiB", DOCKERFILE_MAX_BYTES / 1024)]
    DockerfileTooLong,
}

impl Source {
    /// Reads the source out of a spec as the docker driver reads it: exactly one of `image`, `build` and
    /// `archive` is given (`null` is not given), and `image_digest` goes with `image`.
    pub fn from_spec(spec: &Json) -> Result<Self, SourceError> {
        let given = |member: &str| spec.get(member).is_some_and(|value| !value.is_null());
        let (image, build, archive) = (given("image"), given("build"), given("archive"));
        match [image, build, archive]
            .into_iter()
            .filter(|given| *given)
            .count()
        {
            0 => return Err(SourceError::NoSource),
            1 => {}
            _ => return Err(SourceError::SeveralSources),
        }
        let shape = |member: &'static str| {
            move |error: serde_json::Error| SourceError::Shape {
                member,
                message: error.to_string(),
            }
        };
        if image {
            let reference: String =
                serde_json::from_value(spec["image"].clone()).map_err(shape("image"))?;
            let digest: Option<String> = spec
                .get("image_digest")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()
                .map_err(shape("image_digest"))?
                .flatten();
            Ok(Self::Image { reference, digest })
        } else if build {
            let source: BuildSource =
                serde_json::from_value(spec["build"].clone()).map_err(shape("build"))?;
            if source.dockerfile.len() > DOCKERFILE_MAX_BYTES {
                return Err(SourceError::DockerfileTooLong);
            }
            Ok(Self::Build(source))
        } else {
            Ok(Self::Archive(
                serde_json::from_value(spec["archive"].clone()).map_err(shape("archive"))?,
            ))
        }
    }

    /// Which kind of source it is.
    #[must_use]
    pub const fn kind(&self) -> SourceKind {
        match self {
            Self::Image { .. } => SourceKind::Image,
            Self::Build(_) => SourceKind::Build,
            Self::Archive(_) => SourceKind::Archive,
        }
    }

    /// Whether a host that offers `offered` can use this source; scheduling considers only such hosts.
    #[must_use]
    pub fn is_offered(&self, offered: &[SourceKind]) -> bool {
        offered.contains(&self.kind())
    }
}

impl BuildSource {
    /// The hash of everything that defines the build: the Dockerfile, the context's own identity (its blob's
    /// hash, or the commit a Git ref resolved to), the arguments and the target. The result of a build is
    /// named after it, so that the same definition is built once on each host and found again afterwards.
    #[must_use]
    pub fn definition_hash(&self, context_identity: Option<&str>) -> Hash {
        let mut hasher = Hash::hasher();
        let mut part = |label: &str, bytes: &[u8]| {
            hasher.update(label.as_bytes());
            hasher.update(&u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
            hasher.update(bytes);
        };
        part("dockerfile", self.dockerfile.as_bytes());
        part("context", context_identity.unwrap_or_default().as_bytes());
        for (name, value) in &self.args {
            part("arg-name", name.as_bytes());
            part("arg-value", value.as_bytes());
        }
        part(
            "target",
            self.target.as_deref().unwrap_or_default().as_bytes(),
        );
        Hash::finish(&hasher)
    }
}

/// A base image a build started from, and the digest it had.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BaseImage {
    /// The reference.
    #[serde(rename = "ref")]
    pub reference: String,
    /// Its digest then.
    pub digest: String,
}

/// What a build did, kept on the runtime so that a base that moved on is noticed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct BuildRecord {
    /// The commit a Git ref resolved to.
    pub commit: Option<String>,
    /// The base images it was built from.
    pub bases: Vec<BaseImage>,
}

/// The answer to making a spec's source available on a host.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct ImageEnsured {
    /// The digest of the image; `null` for a driver without one.
    pub digest: Option<String>,
    /// For a build: what it was built from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub built: Option<BuildRecord>,
    /// Programs observed in the available image; an absent entry means it was not checked.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub programs: BTreeMap<String, bool>,
}

impl ImageEnsured {
    /// A driver without images has nothing to make available: the call is a no-op.
    #[must_use]
    pub fn nothing() -> Self {
        Self::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_spec_names_exactly_one_source() {
        let image = Source::from_spec(
            &json!({ "image": "ghcr.io/o/base:0.1.0", "image_digest": "sha256:abc", "build": null, "archive": null }),
        );
        assert_eq!(
            image,
            Ok(Source::Image {
                reference: "ghcr.io/o/base:0.1.0".into(),
                digest: Some("sha256:abc".into())
            })
        );
        assert_eq!(
            Source::from_spec(&json!({ "image": "x", "image_digest": null })).map(|s| s.kind()),
            Ok(SourceKind::Image)
        );
        assert_eq!(
            Source::from_spec(&json!({ "runtime": "runc" })),
            Err(SourceError::NoSource)
        );
        assert_eq!(
            Source::from_spec(&json!({ "image": "x", "build": { "dockerfile": "FROM x" } })),
            Err(SourceError::SeveralSources)
        );
    }

    #[test]
    fn a_build_reads_with_its_context_arguments_and_target() {
        let spec = json!({ "build": {
            "dockerfile": "FROM debian:bookworm-slim\nRUN true", "context": { "git": "https://example.com/r.git", "ref": "main", "path": "." },
            "args": { "NODE_VERSION": "22" }, "target": "runtime" } });
        let Source::Build(build) = Source::from_spec(&spec).unwrap() else {
            panic!("a build");
        };
        assert_eq!(build.args["NODE_VERSION"], "22");
        assert!(
            matches!(build.context, Some(BuildContext::Git { ref reference, .. }) if reference == "main")
        );
        let by_blob: BuildSource = serde_json::from_value(
            json!({ "dockerfile": "FROM x", "context": { "blob": "0".repeat(64) } }),
        )
        .unwrap();
        assert!(matches!(by_blob.context, Some(BuildContext::Blob { .. })));
        assert_eq!(
            serde_json::to_value(&build).unwrap()["context"]["ref"],
            "main"
        );
    }

    #[test]
    fn a_dockerfile_over_256_kib_is_refused() {
        let long = "#".repeat(DOCKERFILE_MAX_BYTES + 1);
        assert_eq!(
            Source::from_spec(&json!({ "build": { "dockerfile": long } })),
            Err(SourceError::DockerfileTooLong)
        );
        let exactly = "#".repeat(DOCKERFILE_MAX_BYTES);
        assert!(Source::from_spec(&json!({ "build": { "dockerfile": exactly } })).is_ok());
    }

    #[test]
    fn an_archive_may_name_which_of_its_images() {
        let archive =
            Source::from_spec(&json!({ "archive": { "blob": "1".repeat(64), "image": null } }))
                .unwrap();
        assert_eq!(archive.kind(), SourceKind::Archive);
        assert!(matches!(
            Source::from_spec(&json!({ "archive": { "blob": "nothex" } })),
            Err(SourceError::Shape {
                member: "archive",
                ..
            })
        ));
    }

    #[test]
    fn a_host_is_considered_only_when_it_offers_the_source_a_template_names() {
        let build = Source::from_spec(&json!({ "build": { "dockerfile": "FROM x" } })).unwrap();
        assert!(build.is_offered(&[SourceKind::Image, SourceKind::Build]));
        assert!(
            !build.is_offered(&[SourceKind::Image, SourceKind::Archive]),
            "building is turned off on that host"
        );
        assert!(!build.is_offered(&[]));
    }

    #[test]
    fn a_build_is_found_again_by_what_defines_it_and_by_nothing_else() {
        let build = BuildSource {
            dockerfile: "FROM x".into(),
            context: None,
            args: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
            target: None,
        };
        let same = build.clone();
        assert_eq!(build.definition_hash(None), same.definition_hash(None));
        let mut other_arg = build.clone();
        other_arg.args.insert("A".into(), "2".into());
        assert_ne!(build.definition_hash(None), other_arg.definition_hash(None));
        assert_ne!(
            build.definition_hash(None),
            build.definition_hash(Some("commit-1"))
        );
        assert_ne!(
            build.definition_hash(Some("commit-1")),
            build.definition_hash(Some("commit-2"))
        );
        let mut staged = build.clone();
        staged.target = Some("runtime".into());
        assert_ne!(build.definition_hash(None), staged.definition_hash(None));
        // The boundary between two parts is part of the definition.
        let a = BuildSource {
            dockerfile: "AB".into(),
            context: None,
            args: BTreeMap::new(),
            target: Some("C".into()),
        };
        let b = BuildSource {
            dockerfile: "A".into(),
            context: None,
            args: BTreeMap::new(),
            target: Some("BC".into()),
        };
        assert_ne!(a.definition_hash(None), b.definition_hash(None));
    }

    #[test]
    fn what_a_build_did_is_kept_with_its_bases() {
        let ensured: ImageEnsured = serde_json::from_value(json!({
            "digest": "sha256:def", "built": { "commit": "abc123", "bases": [ { "ref": "debian:bookworm-slim", "digest": "sha256:111" } ] }
        }))
        .unwrap();
        assert_eq!(
            ensured.built.as_ref().unwrap().bases[0].reference,
            "debian:bookworm-slim"
        );
        assert_eq!(
            serde_json::to_value(ImageEnsured::nothing()).unwrap(),
            json!({ "digest": null })
        );
    }
}
