//! Images built from a Dockerfile: the content hash, the context, the host's limits, progress and log.
//!
//! A build is run by the daemon's own builder, with nothing privileged, from the Dockerfile text of the spec and a
//! context assembled here: the files of an uploaded archive (a tar, a tar.gz or a zip) or of a Git repository at a
//! commit, with the Dockerfile added. Its result is tagged with the hash of everything that defines it (the
//! Dockerfile, the context's own identity, the arguments, the target), so that the same definition is built once on
//! a host and found again; a second tag, of the spec alone, points at the newest result for that spec, which is the
//! name a unit is made from.
//!
//! The host's limits are the driver's settings: a build ends after `build_timeout_secs` and the daemon is told to
//! drop it, and no more than `build_parallel` run at once. While a build runs its output is reported line by line,
//! and when it fails the error carries the last 64 KiB of that output.

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Mutex;
use std::time::Duration;

use bollard::query_parameters::BuildImageOptionsBuilder;
use futures::StreamExt;
use orch_driver_contract::driver::DriverError;
use orch_driver_contract::sources::{
    BaseImage, BuildContext, BuildRecord, BuildSource, ImageEnsured,
};
use orch_types::Hash;

use crate::archive::spool;
use crate::client::map_error;
use crate::config::DockerConfig;
use crate::driver::DockerDriver;
use crate::images::{digest_of, is_step, parse_reference};
use crate::imports::base_references;
use crate::progress::{LogTail, Phase, ProgressSink};

/// The name the Dockerfile has inside the context the daemon is given: the spec's text, kept apart from any file
/// of the context's own.
pub const DOCKERFILE_NAME: &str = "Dockerfile.orch";

/// The label of an image that holds the hash of its definition.
pub const LABEL_DEFINITION: &str = "orch.build.definition";

/// The label that holds the commit a Git context resolved to.
pub const LABEL_COMMIT: &str = "orch.build.commit";

/// The label that holds the base images the build started from, as a JSON list of `{ref, digest}`.
pub const LABEL_BASES: &str = "orch.build.bases";

/// How long looking up what a Git reference points to may take: a lookup is one request.
const GIT_LOOKUP_TIMEOUT: Duration = Duration::from_secs(60);

/// The longest a context entry may be that is read into memory to be copied from a zip: a context file larger
/// than this is streamed, so this is only the size of the copy buffer.
const COPY_BUFFER_BYTES: usize = 64 * 1024;

/// What kind of archive a context is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveKind {
    /// A tar.
    Tar,
    /// A tar compressed with gzip.
    Gzip,
    /// A zip.
    Zip,
}

/// Which kind of archive begins with these bytes.
#[must_use]
pub fn detect(head: &[u8]) -> ArchiveKind {
    if head.starts_with(&[0x1f, 0x8b]) {
        ArchiveKind::Gzip
    } else if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        ArchiveKind::Zip
    } else {
        ArchiveKind::Tar
    }
}

/// The name of the image that a definition is found again by.
#[must_use]
pub fn definition_alias(config: &DockerConfig, definition: &Hash) -> String {
    format!("{}:def-{definition}", config.image_repository)
}

/// The name of the image that a spec's newest build has: the name a unit is made from.
#[must_use]
pub fn spec_alias(config: &DockerConfig, build: &BuildSource) -> String {
    let text = serde_json::to_string(build).unwrap_or_default();
    format!(
        "{}:spec-{}",
        config.image_repository,
        Hash::of(text.as_bytes())
    )
}

/// Writes the entries of a tar into another, one by one.
fn copy_tar<R: Read, W: Write>(source: R, into: &mut tar::Builder<W>) -> Result<(), DriverError> {
    let invalid = |error: std::io::Error| {
        DriverError::InvalidSpec(format!(
            "the context archive cannot be read: {}",
            crate::archive::printable(&error.to_string())
        ))
    };
    let mut archive = tar::Archive::new(source);
    for entry in archive.entries().map_err(invalid)? {
        let mut entry = entry.map_err(invalid)?;
        if matches!(entry.header().entry_type(), tar::EntryType::XGlobalHeader) {
            continue;
        }
        let mut header = entry.header().clone();
        let path = entry.path().map_err(invalid)?.into_owned();
        // A file of the context with the name of the Dockerfile the driver adds would be shadowed.
        if path.as_os_str() == DOCKERFILE_NAME {
            continue;
        }
        if let Some(target) = entry.link_name().map_err(invalid)? {
            let target = target.into_owned();
            into.append_link(&mut header, &path, &target)
                .map_err(invalid)?;
        } else {
            into.append_data(&mut header, &path, &mut entry)
                .map_err(invalid)?;
        }
    }
    Ok(())
}

/// Writes the entries of a zip into a tar.
fn copy_zip(file: std::fs::File, into: &mut tar::Builder<impl Write>) -> Result<(), DriverError> {
    let invalid = |error: String| {
        DriverError::InvalidSpec(format!(
            "the context archive cannot be read: {}",
            crate::archive::printable(&error)
        ))
    };
    let mut archive = zip::ZipArchive::new(file).map_err(|error| invalid(error.to_string()))?;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|error| invalid(error.to_string()))?;
        // `enclosed_name` is none for a name that would reach out of the context.
        let Some(path) = entry.enclosed_name() else {
            return Err(invalid(format!(
                "the entry {:?} lies outside the context",
                entry.name()
            )));
        };
        if path.as_os_str() == DOCKERFILE_NAME {
            continue;
        }
        let mut header = tar::Header::new_gnu();
        header.set_mode(
            entry
                .unix_mode()
                .unwrap_or(if entry.is_dir() { 0o755 } else { 0o644 }),
        );
        if entry.is_dir() {
            header.set_entry_type(tar::EntryType::Directory);
            header.set_size(0);
            into.append_data(&mut header, &path, std::io::empty())
                .map_err(|error| invalid(error.to_string()))?;
        } else {
            header.set_entry_type(tar::EntryType::Regular);
            header.set_size(entry.size());
            let mut buffered = std::io::BufReader::with_capacity(COPY_BUFFER_BYTES, &mut entry);
            into.append_data(&mut header, &path, &mut buffered)
                .map_err(|error| invalid(error.to_string()))?;
        }
    }
    Ok(())
}

/// Where the files a Dockerfile needs come from, as files on the disk.
#[derive(Debug)]
pub enum ContextFiles {
    /// None: the Dockerfile needs no other file.
    Nothing,
    /// An archive of the uploaded kind, at a path.
    Archive(PathBuf),
    /// A tar made from a repository.
    Tar(PathBuf),
}

/// Assembles the tar the daemon builds from: the files of the context, and the Dockerfile of the spec under
/// [`DOCKERFILE_NAME`]. The result is in a temporary file that goes with the handle.
pub fn assemble(
    dockerfile: &str,
    files: &ContextFiles,
) -> Result<tempfile::NamedTempFile, DriverError> {
    let io = |error: std::io::Error| {
        DriverError::Internal(format!("the build context could not be written: {error}"))
    };
    let out = tempfile::NamedTempFile::new().map_err(io)?;
    let mut builder = tar::Builder::new(std::io::BufWriter::new(out.reopen().map_err(io)?));
    match files {
        ContextFiles::Nothing => {}
        ContextFiles::Tar(path) => {
            copy_tar(std::fs::File::open(path).map_err(io)?, &mut builder)?;
        }
        ContextFiles::Archive(path) => {
            let mut head = [0_u8; 4];
            let mut file = std::fs::File::open(path).map_err(io)?;
            let read = file.read(&mut head).map_err(io)?;
            drop(file);
            let file = std::fs::File::open(path).map_err(io)?;
            match detect(&head[..read]) {
                ArchiveKind::Tar => copy_tar(file, &mut builder)?,
                ArchiveKind::Gzip => copy_tar(flate2::read::GzDecoder::new(file), &mut builder)?,
                ArchiveKind::Zip => copy_zip(file, &mut builder)?,
            }
        }
    }
    let mut header = tar::Header::new_gnu();
    header.set_size(dockerfile.len() as u64);
    header.set_mode(0o644);
    header.set_entry_type(tar::EntryType::Regular);
    builder
        .append_data(&mut header, DOCKERFILE_NAME, dockerfile.as_bytes())
        .map_err(io)?;
    builder
        .into_inner()
        .map_err(io)?
        .into_inner()
        .map_err(|error| io(error.into_error()))?;
    Ok(out)
}

/// Runs the Git client with nothing that could ask a person a question or reach for a credential.
async fn git(
    arguments: &[&str],
    dir: Option<&Path>,
    timeout: Duration,
    allowed: &str,
) -> Result<String, DriverError> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "/bin/false")
        .env("GIT_ALLOW_PROTOCOL", allowed)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_CONFIG_GLOBAL", "/dev/null");
    if let Some(dir) = dir {
        command.current_dir(dir);
    }
    let child = command
        .spawn()
        .map_err(|error| DriverError::Image(format!("git could not be run: {error}")))?;
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| {
            DriverError::Image(format!(
                "git {} took longer than {} seconds",
                arguments.first().unwrap_or(&""),
                timeout.as_secs()
            ))
        })?
        .map_err(|error| DriverError::Image(format!("git failed: {error}")))?;
    if !output.status.success() {
        return Err(DriverError::Image(format!(
            "git {} failed: {}",
            arguments.first().unwrap_or(&""),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Whether a text is a whole commit identifier.
fn is_commit(text: &str) -> bool {
    text.len() == 40 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// Whether the Git address is one the driver reads: it has a scheme the configuration allows.
fn allowed_scheme(url: &str, schemes: &[String]) -> bool {
    url.split_once("://")
        .is_some_and(|(scheme, _)| schemes.iter().any(|allowed| allowed == scheme))
}

/// The commit a reference points to in the repository.
async fn resolve_commit(
    url: &str,
    reference: &str,
    schemes: &[String],
) -> Result<String, DriverError> {
    if !allowed_scheme(url, schemes) {
        return Err(DriverError::InvalidSpec(format!(
            "the Git address {url:?} is not one that needs no credential ({})",
            schemes.join(", ")
        )));
    }
    if is_commit(reference) {
        return Ok(reference.to_ascii_lowercase());
    }
    let allowed = schemes.join(":");
    let peeled = format!("{reference}^{{}}");
    let listing = git(
        &["ls-remote", "--exit-code", "--", url, reference, &peeled],
        None,
        GIT_LOOKUP_TIMEOUT,
        &allowed,
    )
    .await?;
    // A tag lists itself and the commit it points to (`^{}`): the commit is the one to build.
    let mut found: Option<String> = None;
    for line in listing.lines() {
        let Some((commit, name)) = line.split_once('\t') else {
            continue;
        };
        if name.ends_with("^{}") || found.is_none() {
            found = Some(commit.to_owned());
        }
    }
    found.filter(|commit| is_commit(commit)).ok_or_else(|| {
        DriverError::Image(format!(
            "the Git reference {reference:?} points to nothing in {url}"
        ))
    })
}

/// Fetches the commit and writes the tree below `path` as a tar.
async fn export_commit(
    url: &str,
    commit: &str,
    path: &str,
    schemes: &[String],
    timeout: Duration,
) -> Result<tempfile::NamedTempFile, DriverError> {
    let allowed = schemes.join(":");
    let work = tempfile::tempdir().map_err(|error| DriverError::Internal(error.to_string()))?;
    git(&["init", "--quiet"], Some(work.path()), timeout, &allowed).await?;
    git(
        &["fetch", "--quiet", "--depth", "1", "--", url, commit],
        Some(work.path()),
        timeout,
        &allowed,
    )
    .await?;
    let tree = if path.is_empty() || path == "." {
        "FETCH_HEAD".to_owned()
    } else {
        format!("FETCH_HEAD:{path}")
    };
    let out =
        tempfile::NamedTempFile::new().map_err(|error| DriverError::Internal(error.to_string()))?;
    let output_path = out.path().to_string_lossy().into_owned();
    git(
        &["archive", "--format=tar", "-o", &output_path, &tree],
        Some(work.path()),
        timeout,
        &allowed,
    )
    .await?;
    Ok(out)
}

/// The labels an image of a build carries.
fn image_labels(
    config: &DockerConfig,
    definition: &Hash,
    commit: Option<&str>,
    bases: &[BaseImage],
) -> HashMap<String, String> {
    let mut labels: HashMap<String, String> =
        crate::units::driver_labels(config).into_iter().collect();
    labels.insert(LABEL_DEFINITION.to_owned(), definition.to_string());
    if let Some(commit) = commit {
        labels.insert(LABEL_COMMIT.to_owned(), commit.to_owned());
    }
    if !bases.is_empty() {
        labels.insert(
            LABEL_BASES.to_owned(),
            serde_json::to_string(bases).unwrap_or_default(),
        );
    }
    labels
}

/// What the labels of a finished build say it was made from.
#[must_use]
pub fn record_of(labels: &HashMap<String, String>) -> BuildRecord {
    BuildRecord {
        commit: labels.get(LABEL_COMMIT).cloned(),
        bases: labels
            .get(LABEL_BASES)
            .and_then(|text| serde_json::from_str(text).ok())
            .unwrap_or_default(),
    }
}

/// The identifier of an intermediate image in a line of the classic builder's output (` ---> 82e80f9a792f`).
#[must_use]
pub fn intermediate_image(line: &str) -> Option<&str> {
    let id = line.trim().strip_prefix("---> ")?.trim();
    (id.len() == 12 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// How early before a build began an image may have been made and still count as made by it: the daemon's clock
/// and the driver's are one on a host, but the daemon stamps to the nanosecond and rounds are not worth a
/// mistake.
const BUILD_CLOCK_SLACK_SECS: i64 = 2;

/// How long to wait before looking for what a build that failed on its own left: the daemon has stopped it.
const BUILD_FAILED_SETTLE: Duration = Duration::from_millis(250);

/// How long the daemon is watched after a build was dropped (timed out): it does not stop at once, it kills the step
/// that was running and goes on for a few seconds to commit that step and the ones after it, and what it makes in
/// those seconds is the build's own. Eight seconds is more than the daemon was seen to need, with room.
const BUILD_DROPPED_WINDOW: Duration = Duration::from_secs(8);

/// How often the daemon is looked at within that window.
const BUILD_DROPPED_POLL: Duration = Duration::from_secs(1);

/// How many times an image that is still in use is asked to be removed.
const REMOVE_ATTEMPTS: u32 = 10;

/// How many passes are made over the images of a dropped build: one image can wait for the ones above it.
const REMOVE_PASSES: u32 = 4;

/// How long to wait between two of them: the daemon needs a moment to remove the container of a step it killed.
const REMOVE_RETRY_PAUSE: Duration = Duration::from_millis(500);

/// Applies nonsecret host defaults while preserving each template’s explicit choices.
fn with_defaults(build: &BuildSource, defaults: &BTreeMap<String, String>) -> BuildSource {
    let mut effective = build.clone();
    effective.args = defaults
        .iter()
        .chain(build.args.iter())
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect();
    effective
}

/// Rewrites exact base tokens, retaining directives, comments, continuations and unrelated command text.
fn local_bases(dockerfile: &str, images: &BTreeMap<String, String>) -> String {
    let mut continued = false;
    let mut from = false;
    let mut argument = false;
    let mut result = String::new();
    for line in dockerfile.split_inclusive('\n') {
        let trimmed = line.trim_start();
        if trimmed.starts_with('#') {
            result.push_str(line);
            continue;
        }
        if !continued {
            let keyword = trimmed.split_whitespace().next().unwrap_or_default();
            from = keyword.eq_ignore_ascii_case("FROM");
            argument = keyword.eq_ignore_ascii_case("ARG");
        }
        if from || argument {
            for token in line.split_inclusive(char::is_whitespace) {
                let value = if argument {
                    token.split_once('=').map(|(_, value)| value.trim())
                } else {
                    Some(token.trim())
                };
                let value = value.unwrap_or_default().trim_matches(['\'', '"']);
                if let Some(content) = images.get(value) {
                    let mut token = token.to_owned();
                    if let Some(start) = token.rfind(value) {
                        token.replace_range(
                            start..start + value.len(),
                            &format!("orch-offline:{}", content.trim_start_matches("sha256:")),
                        );
                    }
                    result.push_str(&token);
                } else {
                    result.push_str(token);
                }
            }
        } else {
            result.push_str(line);
        }
        continued = line.trim_end().ends_with('\\');
    }
    result
}

impl DockerDriver {
    /// Builds the source of a spec once for each definition; see [`DockerDriver::ensure_image_reporting`].
    pub(crate) async fn ensure_build(
        &self,
        build: &BuildSource,
        progress: &dyn ProgressSink,
    ) -> Result<ImageEnsured, DriverError> {
        self.build_image(build, progress, false).await
    }

    /// Builds the source of a spec again, pulling its base images first, whether or not the definition is
    /// already built: how a build that is behind its bases is brought up to date.
    pub async fn rebuild_image(
        &self,
        spec: &orch_types::Json,
        progress: &dyn ProgressSink,
    ) -> Result<ImageEnsured, DriverError> {
        match orch_driver_contract::sources::Source::from_spec(spec)
            .map_err(|error| DriverError::InvalidSpec(error.to_string()))?
        {
            orch_driver_contract::sources::Source::Build(build) => {
                self.build_image(&build, progress, true).await
            }
            _ => Err(DriverError::InvalidSpec(
                "only a build is built again".to_owned(),
            )),
        }
    }

    async fn build_image(
        &self,
        build: &BuildSource,
        progress: &dyn ProgressSink,
        force: bool,
    ) -> Result<ImageEnsured, DriverError> {
        let config = &*self.config;
        let spec_tag = spec_alias(config, build);
        // Include the effective arguments in the identity so a mirror change cannot reuse a stale build.
        let mut build = with_defaults(build, &config.build_args);
        build.dockerfile = local_bases(&build.dockerfile, &config.offline_images);
        for (reference, content) in &config.offline_images {
            let alias = format!("orch-offline:{}", content.trim_start_matches("sha256:"));
            for value in build.args.values_mut() {
                if value == reference {
                    *value = alias.clone();
                }
            }
        }
        let build = &build;
        let limit = Duration::from_secs(config.build_timeout_secs);
        // The identity of the context: a blob is its hash, a repository is the commit its reference names.
        let (identity, commit) = match &build.context {
            None => (None, None),
            Some(BuildContext::Blob { blob }) => (Some(blob.to_string()), None),
            Some(BuildContext::Git { git, reference, .. }) => {
                let commit = resolve_commit(git, reference, &config.git_schemes).await?;
                (Some(commit.clone()), Some(commit))
            }
        };
        let definition = build.definition_hash(identity.as_deref());
        let definition_tag = definition_alias(config, &definition);
        if !force && let Some(found) = self.daemon.image(&definition_tag).await? {
            // Found again: the newest build of this spec is this one.
            self.tag_as(&definition_tag, &spec_tag).await?;
            let labels = found
                .config
                .as_ref()
                .and_then(|config| config.labels.clone())
                .unwrap_or_default();
            return Ok(ImageEnsured {
                programs: self.image_programs(&found).await,
                digest: digest_of(&found, &definition_tag),
                built: Some(record_of(&labels)),
            });
        }
        // What the definition's name pointed to before, which a build made again takes the name from. It is given
        // a name of its own first, so that if a unit stands on it and it cannot go it is still found by the
        // pruning of old builds and never left without a name.
        let previous = if force {
            match self
                .daemon
                .image(&definition_tag)
                .await?
                .and_then(|image| image.id)
            {
                Some(id) => {
                    let name = format!(
                        "{}:old-{}",
                        config.image_repository,
                        id.trim_start_matches("sha256:")
                            .chars()
                            .take(12)
                            .collect::<String>()
                    );
                    self.tag_as(&id, &name).await?;
                    Some((id, name))
                }
                None => None,
            }
        } else {
            None
        };
        // One build at a time, or as many as the host allows.
        let _permit = self
            .builds
            .acquire()
            .await
            .map_err(|_| DriverError::Internal("the builds were shut down".to_owned()))?;
        let started = tokio::time::Instant::now();
        let began = self.clock.time();
        // The intermediate images that the builder reports as it goes: when the build does not finish they are
        // what it leaves behind, and are removed.
        let intermediates: Mutex<Vec<String>> = Mutex::new(Vec::new());
        let work = async {
            // The files of the context.
            let spooled;
            let exported;
            let files = match &build.context {
                None => ContextFiles::Nothing,
                Some(BuildContext::Blob { blob }) => {
                    let source = self.blobs.as_ref().ok_or_else(|| {
                        DriverError::Image(
                            "this host has no way to fetch the build context".to_owned(),
                        )
                    })?;
                    spooled = spool(source.as_ref(), blob).await?;
                    ContextFiles::Archive(spooled.path().to_owned())
                }
                Some(BuildContext::Git { git, path, .. }) => {
                    let commit = commit.clone().unwrap_or_default();
                    exported =
                        export_commit(git, &commit, path, &config.git_schemes, limit).await?;
                    ContextFiles::Tar(exported.path().to_owned())
                }
            };
            let dockerfile = build.dockerfile.clone();
            let context = tokio::task::spawn_blocking(move || assemble(&dockerfile, &files))
                .await
                .map_err(|error| DriverError::Internal(error.to_string()))??;
            // The images the build starts from are made available first, so that what is recorded of them is what
            // the build used.
            let bases = self
                .resolve_bases(&base_references(&build.dockerfile), force, progress)
                .await?;
            self.run_builder(
                build,
                &definition,
                &definition_tag,
                commit.as_deref(),
                &bases,
                context,
                progress,
                &intermediates,
            )
            .await
        };
        let remaining = limit.saturating_sub(started.elapsed());
        let mut dropped = false;
        let outcome = match tokio::time::timeout(remaining, work).await {
            Ok(outcome) => outcome,
            Err(_) => {
                dropped = true;
                Err(DriverError::Image(format!(
                    "the build took longer than the {} seconds this host allows, and was stopped",
                    config.build_timeout_secs
                )))
            }
        };
        let record = match outcome {
            Ok(record) => record,
            Err(error) => {
                let seen = std::mem::take(
                    &mut *intermediates
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner),
                );
                self.discard_intermediates(&seen, &definition, began, dropped)
                    .await;
                return Err(error);
            }
        };
        self.tag_as(&definition_tag, &spec_tag).await?;
        let image =
            self.daemon.image(&definition_tag).await?.ok_or_else(|| {
                DriverError::Image("the built image is not on the host".to_owned())
            })?;
        // A superseded build keeps its own name until the server chooses it from inventory. The
        // duplicate alias of an identical rebuild is metadata only: the image keeps its current names.
        if let Some((id, name)) = previous
            && image.id.as_ref() == Some(&id)
        {
            // The same image was built again: it keeps its names and needs no other.
            self.remove_image_tag(&name).await?;
        }
        // A completed build reports its artifacts. Their removal is selected by the server from
        // the installation inventory, so a local quota cannot bypass its disabled cleanup policy.
        Ok(ImageEnsured {
            programs: self.image_programs(&image).await,
            digest: digest_of(&image, &definition_tag),
            built: Some(record),
        })
    }

    /// Makes the base images of a build available, pulling those the host does not hold (or all of them, for a build
    /// that is made again to bring it up to date), and answers each with the digest it has now.
    async fn resolve_bases(
        &self,
        references: &[String],
        refresh: bool,
        progress: &dyn ProgressSink,
    ) -> Result<Vec<BaseImage>, DriverError> {
        let mut bases = Vec::new();
        for reference in references {
            let held = self.daemon.image(reference).await?;
            if reference.starts_with("orch-offline:") && held.is_none() {
                return Err(DriverError::Image(
                    "a carried-in build base is missing; load its verified archive on this daemon"
                        .to_owned(),
                ));
            }
            let image = match held {
                Some(image) if !refresh || reference.starts_with("orch-offline:") => image,
                held => match self.pull_image(reference, None, progress).await {
                    Ok(()) => self.daemon.image(reference).await?,
                    // A base that cannot be fetched again is used as the host has it.
                    Err(DriverError::Image(why)) if held.is_some() => {
                        tracing::warn!(%reference, %why, "a base image could not be refreshed; the one the host holds is used");
                        held
                    }
                    Err(error) => return Err(error),
                }
                .ok_or_else(|| {
                    DriverError::Image(format!("the base image {reference} is not on this host"))
                })?,
            };
            let digest = if reference.starts_with("orch-offline:") {
                image.id.clone()
            } else {
                digest_of(&image, reference)
            };
            if let Some(digest) = digest {
                let parsed = parse_reference(reference);
                bases.push(BaseImage {
                    reference: parsed.tag.map_or_else(
                        || parsed.name.clone(),
                        |tag| format!("{}:{tag}", parsed.name),
                    ),
                    digest,
                });
            }
        }
        Ok(bases)
    }

    #[allow(
        clippy::too_many_arguments,
        reason = "the steps of one build, kept together to be read in order"
    )]
    async fn run_builder(
        &self,
        build: &BuildSource,
        definition: &Hash,
        definition_tag: &str,
        commit: Option<&str>,
        bases: &[BaseImage],
        context: tempfile::NamedTempFile,
        progress: &dyn ProgressSink,
        intermediates: &Mutex<Vec<String>>,
    ) -> Result<BuildRecord, DriverError> {
        let config = &*self.config;
        let labels = image_labels(config, definition, commit, bases);
        let args: HashMap<String, String> = build
            .args
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let mut options = BuildImageOptionsBuilder::default()
            .dockerfile(DOCKERFILE_NAME)
            .t(definition_tag)
            .rm(true)
            .forcerm(true)
            // The base images were made available above; the builder is not to go to a registry for them.
            .pull("false")
            .buildargs(&args)
            .labels(&labels);
        if let Some(target) = build.target.as_deref().filter(|target| !target.is_empty()) {
            options = options.target(target);
        }
        let file = tokio::fs::File::open(context.path())
            .await
            .map_err(|error| {
                DriverError::Internal(format!("the build context could not be read: {error}"))
            })?;
        let body = bollard::body_try_stream(tokio_util::io::ReaderStream::new(file));
        let patient = self
            .daemon
            .patient(Duration::from_secs(config.build_timeout_secs.max(1)));
        let mut stream = Box::pin(patient.build_image(options.build(), None, Some(body)));
        let mut log = LogTail::new();
        let mut pending = String::new();
        while let Some(item) = stream.next().await {
            match item {
                Ok(info) => {
                    if let Some(text) = info.stream {
                        pending.push_str(&text);
                        // Report whole lines; the builder sends pieces.
                        while let Some(end) = pending.find('\n') {
                            let line: String = pending.drain(..=end).collect();
                            let line = line.trim_end();
                            if !line.is_empty() {
                                progress.line(Phase::Build, line);
                            }
                            if let Some(image) = intermediate_image(line) {
                                let mut seen = intermediates
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if !seen.iter().any(|known| known == image) {
                                    seen.push(image.to_owned());
                                }
                            }
                            log.push(&format!("{line}\n"));
                        }
                    }
                    if let Some(status) = info.status {
                        progress.line(Phase::Build, &status);
                        log.push(&format!("{status}\n"));
                    }
                }
                Err(bollard::errors::Error::DockerStreamError { error }) => {
                    return Err(DriverError::Image(format!(
                        "the build failed: {error}\n{}",
                        log.as_str()
                    )));
                }
                Err(other) => {
                    return Err(match map_error(other) {
                        DriverError::NotFound(message) | DriverError::InvalidSpec(message) => {
                            DriverError::Image(format!(
                                "the build was refused: {message}\n{}",
                                log.as_str()
                            ))
                        }
                        error => error,
                    });
                }
            }
        }
        if !pending.trim().is_empty() {
            progress.line(Phase::Build, pending.trim_end());
            log.push(&pending);
        }
        Ok(BuildRecord {
            commit: commit.map(str::to_owned),
            bases: bases.to_vec(),
        })
    }

    /// Removes the intermediate images that a build which did not finish left. The build's own images are found
    /// by what only they have: the ones the builder reported by name as it went, and the ones that carry the
    /// label of this build's definition; from each the images beneath it are followed up to the first that has a
    /// name (the image the build started from). Only an image that carries no name and was made after the build
    /// began is removed, and an image that something still stands on is refused by the daemon and left. A build
    /// that was dropped is watched for a while, since the daemon goes on making images for a few seconds.
    async fn discard_intermediates(
        &self,
        reported: &[String],
        definition: &Hash,
        began: orch_types::Time,
        dropped: bool,
    ) {
        if !dropped {
            tokio::time::sleep(BUILD_FAILED_SETTLE).await;
            self.discard_pass(reported, definition, began).await;
            return;
        }
        let deadline = tokio::time::Instant::now() + BUILD_DROPPED_WINDOW;
        while tokio::time::Instant::now() < deadline {
            tokio::time::sleep(BUILD_DROPPED_POLL).await;
            self.discard_pass(reported, definition, began).await;
        }
    }

    /// One look at the daemon's images, and the removal of the build's own among them.
    async fn discard_pass(&self, reported: &[String], definition: &Hash, began: orch_types::Time) {
        let not_before = began.get().timestamp() - BUILD_CLOCK_SLACK_SECS;
        let Ok(listed) = self
            .daemon
            .docker()
            .list_images(Some(
                bollard::query_parameters::ListImagesOptionsBuilder::default()
                    .all(true)
                    .build(),
            ))
            .await
        else {
            return;
        };
        let fresh =
            |image: &bollard::models::ImageSummary| image.created >= not_before && is_step(image);
        let by_id: HashMap<&str, &bollard::models::ImageSummary> = listed
            .iter()
            .map(|image| (image.id.as_str(), image))
            .collect();
        let wanted = definition.to_string();
        let mut ours: Vec<&bollard::models::ImageSummary> = Vec::new();
        for leaf in listed.iter().filter(|image| fresh(image)) {
            let reported_by_builder = reported.iter().any(|seen| {
                leaf.id
                    .trim_start_matches("sha256:")
                    .starts_with(seen.as_str())
            });
            if !(reported_by_builder || leaf.labels.get(LABEL_DEFINITION) == Some(&wanted)) {
                continue;
            }
            let mut current = Some(leaf);
            while let Some(image) = current {
                if !fresh(image) || ours.iter().any(|known| known.id == image.id) {
                    break;
                }
                ours.push(image);
                current = by_id.get(image.parent_id.as_str()).copied();
            }
        }
        // Newest first, so that an image goes before the one it stands on; what is refused is tried again once
        // the images above it have gone.
        ours.sort_by_key(|image| std::cmp::Reverse(image.created));
        let mut left: Vec<String> = ours.iter().map(|image| image.id.clone()).collect();
        for _ in 0..REMOVE_PASSES {
            let mut stuck = Vec::new();
            for id in &left {
                if !self.remove_when_free(id).await {
                    stuck.push(id.clone());
                }
            }
            if stuck.is_empty() || stuck.len() == left.len() {
                break;
            }
            left = stuck;
        }
    }

    /// Removes an image the daemon still has a container on, which it has for a moment after a build was stopped:
    /// asks again, a few times, for as long as the answer is that it is in use.
    async fn remove_when_free(&self, id: &str) -> bool {
        for attempt in 0..REMOVE_ATTEMPTS {
            match self.remove_image_alone(id).await {
                Ok(_) => return true,
                Err(error)
                    if crate::client::status_of(&error) == Some(409)
                        && attempt + 1 < REMOVE_ATTEMPTS =>
                {
                    tokio::time::sleep(REMOVE_RETRY_PAUSE).await;
                }
                Err(error) => {
                    tracing::debug!(image = %id, %error, "an intermediate image of a failed build could not be removed");
                    return false;
                }
            }
        }
        false
    }

    /// Gives an image a second name.
    async fn tag_as(&self, from: &str, name: &str) -> Result<(), DriverError> {
        let (repository, tag) = name.split_once(':').unwrap_or((name, "latest"));
        self.daemon
            .docker()
            .tag_image(
                from,
                Some(
                    bollard::query_parameters::TagImageOptionsBuilder::default()
                        .repo(repository)
                        .tag(tag)
                        .build(),
                ),
            )
            .await
            .map_err(map_error)
    }
}

/// A table of what the labels of a build record, for a test of the round trip.
#[must_use]
pub fn labels_of_record(
    config: &DockerConfig,
    definition: &Hash,
    record: &BuildRecord,
) -> BTreeMap<String, String> {
    image_labels(config, definition, record.commit.as_deref(), &record.bases)
        .into_iter()
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build() -> BuildSource {
        BuildSource {
            dockerfile: "FROM busybox\nRUN true\n".into(),
            context: None,
            args: BTreeMap::from([("A".to_owned(), "1".to_owned())]),
            target: None,
        }
    }

    fn entries(tar_bytes: &[u8]) -> Vec<(String, Vec<u8>)> {
        let mut archive = tar::Archive::new(tar_bytes);
        archive
            .entries()
            .unwrap()
            .map(|entry| {
                let mut entry = entry.unwrap();
                let path = entry.path().unwrap().to_string_lossy().into_owned();
                let mut content = Vec::new();
                entry.read_to_end(&mut content).unwrap();
                (path, content)
            })
            .collect()
    }

    fn tar_with(files: &[(&str, &str)]) -> Vec<u8> {
        let mut builder = tar::Builder::new(Vec::new());
        for (path, content) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(content.len() as u64);
            header.set_mode(0o644);
            header.set_entry_type(tar::EntryType::Regular);
            builder
                .append_data(&mut header, path, content.as_bytes())
                .unwrap();
        }
        builder.into_inner().unwrap()
    }

    #[test]
    fn carried_in_bases_replace_exact_image_tokens_without_changing_commands_or_other_tags() {
        let images = BTreeMap::from([("busybox:chosen".to_owned(), "sha256:abc".to_owned())]);
        let original = "# syntax=docker/dockerfile:1\nFROM busybox:chosen AS base\nFROM busybox:chosen-extra\nARG BASE=busybox:chosen\nRUN echo busybox:chosen\nFROM \\\n busybox:chosen\n";
        let changed = local_bases(original, &images);
        assert!(changed.starts_with("# syntax=docker/dockerfile:1\n"));
        assert!(changed.contains("FROM orch-offline:abc AS base\n"));
        assert!(changed.contains("FROM busybox:chosen-extra\n"));
        assert!(changed.contains("ARG BASE=orch-offline:abc\n"));
        assert!(changed.contains("RUN echo busybox:chosen\n"));
        assert!(changed.ends_with(" orch-offline:abc\n"));
    }

    #[test]
    fn host_mirrors_reach_the_build_and_change_its_cache_identity() {
        let build: BuildSource = serde_json::from_value(serde_json::json!({
            "dockerfile":"FROM scratch", "context":null, "target":null,
            "args":{"APT_MIRROR":"http://chosen.internal"}
        }))
        .unwrap();
        let defaults = BTreeMap::from([
            (
                "HTTP_PROXY".to_owned(),
                "http://proxy.internal:3128".to_owned(),
            ),
            (
                "APT_MIRROR".to_owned(),
                "http://default.internal".to_owned(),
            ),
        ]);
        let effective = with_defaults(&build, &defaults);
        assert_eq!(effective.args["HTTP_PROXY"], "http://proxy.internal:3128");
        assert_eq!(effective.args["APT_MIRROR"], "http://chosen.internal");
        assert_ne!(effective.definition_hash(None), build.definition_hash(None));
    }

    #[test]
    fn the_images_a_build_reports_are_read_from_its_arrows() {
        assert_eq!(
            intermediate_image(" ---> 82e80f9a792f"),
            Some("82e80f9a792f")
        );
        assert_eq!(intermediate_image("---> Running in 1d7cb2e84a45"), None);
        assert_eq!(intermediate_image(" ---> Using cache"), None);
        assert_eq!(
            intermediate_image(" ---> Removed intermediate container 1d7cb2e84a45"),
            None
        );
        assert_eq!(intermediate_image("Step 2/4 : RUN true"), None);
        assert_eq!(intermediate_image(" ---> 82E80F9A792Z"), None);
    }

    #[test]
    fn a_build_is_named_by_its_definition_and_a_spec_by_its_text() {
        let config = DockerConfig::default();
        let one = build();
        let mut other = build();
        other.args.insert("A".into(), "2".into());
        assert_ne!(spec_alias(&config, &one), spec_alias(&config, &other));
        assert_eq!(spec_alias(&config, &one), spec_alias(&config, &one.clone()));
        assert_ne!(
            definition_alias(&config, &one.definition_hash(None)),
            definition_alias(&config, &other.definition_hash(None))
        );
        assert!(
            definition_alias(&config, &one.definition_hash(None)).starts_with("orch-build:def-")
        );
        assert!(spec_alias(&config, &one).len() < 128 + "orch-build:".len());
    }

    #[test]
    fn a_dockerfile_alone_is_a_context_of_one_file() {
        let context = assemble("FROM busybox\n", &ContextFiles::Nothing).unwrap();
        let found = entries(&std::fs::read(context.path()).unwrap());
        assert_eq!(
            found,
            vec![(DOCKERFILE_NAME.to_owned(), b"FROM busybox\n".to_vec())]
        );
    }

    #[test]
    fn the_files_of_an_archive_and_the_dockerfile_of_the_spec_make_the_context_and_the_spec_wins() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("context.tar");
        std::fs::write(
            &path,
            tar_with(&[
                ("app/main.c", "int main(){}"),
                ("Dockerfile.orch", "FROM evil"),
            ]),
        )
        .unwrap();
        let context = assemble("FROM busybox\n", &ContextFiles::Archive(path)).unwrap();
        let found = entries(&std::fs::read(context.path()).unwrap());
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0],
            ("app/main.c".to_owned(), b"int main(){}".to_vec())
        );
        assert_eq!(
            found[1],
            (DOCKERFILE_NAME.to_owned(), b"FROM busybox\n".to_vec())
        );
    }

    #[test]
    fn a_compressed_archive_and_a_zip_are_read_as_well() {
        let dir = tempfile::tempdir().unwrap();
        let gz = dir.path().join("context.tar.gz");
        let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        encoder
            .write_all(&tar_with(&[("data/a.txt", "a")]))
            .unwrap();
        std::fs::write(&gz, encoder.finish().unwrap()).unwrap();
        let from_gz = assemble("FROM x\n", &ContextFiles::Archive(gz)).unwrap();
        assert_eq!(
            entries(&std::fs::read(from_gz.path()).unwrap())[0].0,
            "data/a.txt"
        );

        let zipped = dir.path().join("context.zip");
        {
            let mut writer = zip::ZipWriter::new(std::fs::File::create(&zipped).unwrap());
            let options = zip::write::SimpleFileOptions::default();
            writer.add_directory("src/", options).unwrap();
            writer.start_file("src/b.txt", options).unwrap();
            writer.write_all(b"b").unwrap();
            writer.finish().unwrap();
        }
        let from_zip = assemble("FROM x\n", &ContextFiles::Archive(zipped)).unwrap();
        let found = entries(&std::fs::read(from_zip.path()).unwrap());
        assert!(
            found
                .iter()
                .any(|(path, content)| path == "src/b.txt" && content == b"b")
        );
        assert_eq!(found.last().unwrap().0, DOCKERFILE_NAME);
    }

    #[test]
    fn something_that_is_no_archive_is_refused_as_a_context() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("junk");
        std::fs::write(&path, vec![7_u8; 2000]).unwrap();
        assert!(matches!(
            assemble("FROM x\n", &ContextFiles::Archive(path)),
            Err(DriverError::InvalidSpec(_))
        ));
        assert_eq!(detect(&[0x1f, 0x8b, 8]), ArchiveKind::Gzip);
        assert_eq!(detect(b"PK\x03\x04rest"), ArchiveKind::Zip);
        assert_eq!(detect(b"ustar"), ArchiveKind::Tar);
    }

    #[test]
    fn only_git_addresses_that_need_no_credential_are_read() {
        let schemes = DockerConfig::default().git_schemes;
        assert!(allowed_scheme("https://example.com/r.git", &schemes));
        assert!(!allowed_scheme("file:///etc", &schemes));
        assert!(!allowed_scheme("ssh://git@example.com/r.git", &schemes));
        assert!(!allowed_scheme("/srv/repo", &schemes));
        assert!(is_commit(&"ab".repeat(20)) && !is_commit("main") && !is_commit(&"ab".repeat(19)));
    }

    #[test]
    fn what_a_build_records_is_kept_in_the_labels_of_its_image_and_read_back() {
        let config = DockerConfig::default();
        let record = BuildRecord {
            commit: Some("a".repeat(40)),
            bases: vec![BaseImage {
                reference: "busybox:latest".into(),
                digest: "sha256:ab".into(),
            }],
        };
        let definition = build().definition_hash(None);
        let labels: HashMap<String, String> = labels_of_record(&config, &definition, &record)
            .into_iter()
            .collect();
        assert_eq!(labels[LABEL_DEFINITION], definition.to_string());
        assert_eq!(labels["orch.driver"], "docker");
        assert_eq!(record_of(&labels), record);
        assert_eq!(
            record_of(&HashMap::new()),
            BuildRecord {
                commit: None,
                bases: vec![]
            }
        );
    }
}
